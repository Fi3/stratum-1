use binary_sv2::GetSize;
use framing_sv2::framing::{EncodableFrame, MessageFrame, SerializedFrame};
use mining_sv2::OpenStandardMiningChannelOwned;
use roles_logic_sv2::{
    frames::RequestIdUpdate,
    parsers::{Mining, MiningOwned},
};

fn main() {
    let request = OpenStandardMiningChannelOwned {
        request_id: 77,
        user_identity: b"worker.1".to_vec().try_into().unwrap(),
        nominal_hash_rate: 1_000_000.,
        max_target: [255; 32].into(),
    };
    let frame: MessageFrame<_> = MiningOwned::OpenStandardMiningChannel(request)
        .try_into()
        .unwrap();
    let mut wire = vec![0; frame.encoded_length()];
    frame.encode_into(&mut wire).unwrap();
    let mut received = SerializedFrame::from_bytes(wire).unwrap();
    let mut decoded = Mining::try_from((received.header().msg_type(), received.payload())).unwrap();
    match &mut decoded {
        Mining::OpenStandardMiningChannel(request) => request.request_id = 42,
        _ => unreachable!(),
    }
    // Retain a native owned message only when it must outlive the receive buffer.
    let retained = decoded.into_owned();
    RequestIdUpdate {
        message_type: mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        original_id: 77,
        new_id: 42,
    }
    .apply(&mut received)
    .unwrap();
    let mut encoded = vec![0; retained.get_size() + 32];
    let written = binary_sv2::to_writer(retained, &mut encoded).unwrap();
    assert_eq!(&encoded[..written], received.payload());
}
