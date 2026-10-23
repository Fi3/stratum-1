use binary_sv2::GetSize;
use codec_sv2::{Decoded, Decoder};
use framing_sv2::framing::{EncodableFrame, MessageFrame, SerializedFrame};
use mining_sv2::{OpenStandardMiningChannelOwned, SubmitSharesStandard};
use roles_logic_sv2::{
    frames::RequestIdUpdate,
    parsers::{Mining, MiningOwned},
};

fn frame(message: MiningOwned) -> Vec<u8> {
    let frame: MessageFrame<_> = message.try_into().unwrap();
    let mut bytes = vec![0; frame.encoded_length()];
    frame.encode_into(&mut bytes).unwrap();
    bytes
}
fn shares() -> MiningOwned {
    MiningOwned::SubmitSharesStandard(SubmitSharesStandard {
        channel_id: 1,
        sequence_number: 2,
        job_id: 3,
        nonce: 4,
        ntime: 5,
        version: 0x20000000,
    })
}
#[test]
fn fragmented_frames_and_consecutive_frames_decode_without_extra_reads() {
    let first = frame(shares());
    let second = frame(shares());
    let wire = [first.clone(), second.clone()].concat();
    let mut decoder = Decoder::new();
    let mut cursor = 0;
    let mut received = vec![];
    loop {
        match decoder.next_frame().unwrap() {
            Decoded::Frame(mut f) => {
                assert!(matches!(
                    Mining::try_from((f.header().msg_type(), f.payload())).unwrap(),
                    Mining::SubmitSharesStandard(_)
                ));
                received.push(f);
                if received.len() == 2 {
                    break;
                }
            }
            Decoded::Incomplete(0) => continue,
            Decoded::Incomplete(n) => {
                assert_eq!(n, decoder.writable_len());
                let destination = decoder.writable();
                // Simulated fragmented reads fill the writable slice before decoding.
                for chunk in destination.chunks_mut(2) {
                    chunk.copy_from_slice(&wire[cursor..cursor + chunk.len()]);
                    cursor += chunk.len();
                }
            }
        }
    }
    assert_eq!(cursor, wire.len());
    // Prebuffer two consecutive frames in header-sized pieces. After the first frame,
    // the next complete frame is buffered and writable_len is zero: decode again.
    let mut decoder = Decoder::new();
    for chunk in wire.chunks_exact(6) {
        decoder.writable().copy_from_slice(chunk);
    }
    assert!(matches!(decoder.next_frame().unwrap(), Decoded::Frame(_)));
    assert_eq!(decoder.writable_len(), 0);
    assert!(matches!(decoder.next_frame().unwrap(), Decoded::Frame(_)));
    assert_eq!(decoder.writable_len(), 6);
}
#[test]
fn to_writer_count_limits_the_transmitted_bytes() {
    let message = shares();
    let expected = binary_sv2::to_bytes(message.clone()).unwrap();
    let mut destination = vec![0xcc; message.get_size() + 128];
    let written = binary_sv2::to_writer(message, &mut destination).unwrap();
    assert_eq!(&destination[..written], expected);
    assert!(destination[written..].iter().all(|b| *b == 0xcc));
}
#[test]
fn checked_id_edits_reject_wrong_types_lengths_ids_and_extensions() {
    let message = MiningOwned::OpenStandardMiningChannel(OpenStandardMiningChannelOwned {
        request_id: 77,
        user_identity: b"miner".to_vec().try_into().unwrap(),
        nominal_hash_rate: 1.,
        max_target: [255; 32].into(),
    });
    let wire = frame(message);
    let update = RequestIdUpdate {
        message_type: mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        original_id: 77,
        new_id: 42,
    };
    let mut frame = SerializedFrame::from_bytes(wire.clone()).unwrap();
    update.apply(&mut frame).unwrap();
    assert_eq!(&frame.payload()[..4], &42u32.to_le_bytes());
    assert_eq!(&frame.payload()[4..], &wire[10..]);
    let mut payload = wire[6..].to_vec();
    let original = payload.clone();
    assert!(update
        .apply_payload(mining_sv2::MESSAGE_TYPE_NEW_MINING_JOB, &mut payload)
        .is_err());
    assert_eq!(payload, original);
    for len in 0..payload.len() {
        let mut bad = payload[..len].to_vec();
        let snapshot = bad.clone();
        assert!(update.apply_payload(update.message_type, &mut bad).is_err());
        assert_eq!(bad, snapshot);
    }
    payload.push(0);
    let snapshot = payload.clone();
    assert!(update
        .apply_payload(update.message_type, &mut payload)
        .is_err());
    assert_eq!(payload, snapshot);
    let mut bad = SerializedFrame::from_bytes(wire.clone()).unwrap();
    assert!(RequestIdUpdate {
        original_id: 999,
        ..update
    }
    .apply(&mut bad)
    .is_err());
    let mut wrong_extension = wire;
    wrong_extension[0] = 1;
    let mut bad = SerializedFrame::from_bytes(wrong_extension).unwrap();
    assert!(update.apply(&mut bad).is_err());
}
