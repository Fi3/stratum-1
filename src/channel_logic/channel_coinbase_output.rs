//! Stable channel markers in pool-created coinbase outputs.

use crate::Error;
use bitcoin::{
    consensus::{deserialize_partial, serialize},
    Amount, ScriptBuf, TxOut, VarInt,
};
use mining_sv2::NewExtendedMiningJobOwned;

/// Stable output for one pool extended channel. The zero-valued OP_RETURN pushes `CHSEQ`
/// followed by a fixed 16-byte random factory prefix and an 8-byte big-endian channel counter.
/// The counter starts at 1; the prefix is shared by all channels for the factory's lifetime.
/// This tagged format is reserved for the factory; other outputs are preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelCoinbaseOutput(pub(crate) [u8; 24]);

impl ChannelCoinbaseOutput {
    /// Returns this channel marker as a zero-valued coinbase output.
    pub fn tx_out(self) -> TxOut {
        let mut payload = [0; 29];
        payload[..5].copy_from_slice(b"CHSEQ");
        payload[5..].copy_from_slice(&self.0);
        TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(payload),
        }
    }

    /// Conservative extra coinbase weight: the output plus two bytes for CompactSize output
    /// count growth. Larger count boundaries cannot fit in an SV2 coinbase suffix.
    pub fn additional_coinbase_weight() -> usize {
        (serialize(&Self([0; 24]).tx_out()).len() + 2) * 4
    }

    /// Materializes a pool-created job, including shared jobs returned by extended fanout.
    /// Call before adding local outputs and registering the per-job output.
    /// Repeated application replaces an existing CHSEQ output in place and removes duplicates.
    /// Other outputs, the input sequence, extranonce split, witness and locktime remain intact.
    /// Malformed or oversized suffixes return an error without changing the job.
    pub fn apply(self, job: &mut NewExtendedMiningJobOwned) -> Result<(), Error> {
        rewrite_coinbase_outputs(job, |outputs| {
            let mut has_channel_output = false;
            outputs.retain_mut(|output| {
                if is_channel_output(output) {
                    if has_channel_output {
                        false
                    } else {
                        *output = self.tx_out();
                        has_channel_output = true;
                        true
                    }
                } else {
                    true
                }
            });
            if !has_channel_output {
                outputs.push(self.tx_out());
            }
        })
    }
}

fn is_channel_output(output: &TxOut) -> bool {
    output.value == Amount::ZERO
        && output.script_pubkey.len() == 31
        && output
            .script_pubkey
            .as_bytes()
            .starts_with(b"\x6a\x1dCHSEQ")
}

/// Adds an output before the witness and locktime, retaining the native extranonce split.
pub(crate) fn append_coinbase_output(
    job: &mut NewExtendedMiningJobOwned,
    output: TxOut,
) -> Result<(), Error> {
    rewrite_coinbase_outputs(job, |outputs| outputs.push(output))
}

fn rewrite_coinbase_outputs(
    job: &mut NewExtendedMiningJobOwned,
    rewrite: impl FnOnce(&mut Vec<TxOut>),
) -> Result<(), Error> {
    let suffix = job.coinbase_tx_suffix.as_bytes();
    // Pool-created suffixes begin immediately after the coinbase script, with the input sequence.
    let encoded_outputs = suffix.get(4..).ok_or(Error::InvalidCoinbase)?;
    let (mut outputs, consumed): (Vec<TxOut>, usize) =
        deserialize_partial(encoded_outputs).map_err(|_| Error::InvalidCoinbase)?;
    let tail = &encoded_outputs[consumed..];
    validate_coinbase_tail(job.coinbase_tx_prefix.as_bytes(), tail)?;
    rewrite(&mut outputs);
    let new_suffix = [&suffix[..4], serialize(&outputs).as_slice(), tail].concat();
    job.coinbase_tx_suffix = new_suffix.try_into()?;
    Ok(())
}

fn validate_coinbase_tail(prefix: &[u8], mut tail: &[u8]) -> Result<(), Error> {
    match prefix.get(4..) {
        // A pool coinbase has one input, so its SegWit form carries one witness stack.
        Some([0, 1, 1, ..]) => {
            let (count, consumed): (VarInt, usize) =
                deserialize_partial(tail).map_err(|_| Error::InvalidCoinbase)?;
            tail = &tail[consumed..];
            // An empty witness stack would make the SegWit marker/flag superfluous.
            if count.0 == 0 || count.0 > tail.len() as u64 {
                return Err(Error::InvalidCoinbase);
            }
            for _ in 0..count.0 {
                let (length, consumed): (VarInt, usize) =
                    deserialize_partial(tail).map_err(|_| Error::InvalidCoinbase)?;
                tail = &tail[consumed..];
                let length: usize = length.0.try_into().map_err(|_| Error::InvalidCoinbase)?;
                tail = tail.get(length..).ok_or(Error::InvalidCoinbase)?;
            }
        }
        Some([1, ..]) => {}
        _ => return Err(Error::InvalidCoinbase),
    }
    if tail.len() != 4 {
        return Err(Error::InvalidCoinbase);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_sv2::{Seq0255Owned, Sv2OptionOwned};
    use bitcoin::{
        absolute, consensus::deserialize, transaction, OutPoint, Sequence, Transaction, TxIn,
        Witness,
    };

    const EXTRANONCE: [u8; 16] = [0; 16];

    fn transaction(witness: bool) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::from_consensus(123456),
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes([&[3, 1, 0, 0][..], &EXTRANONCE].concat()),
                sequence: Sequence(0xa1b2c3d4),
                witness: if witness {
                    Witness::from_slice(&[vec![0x42; 32]])
                } else {
                    Witness::default()
                },
            }],
            output: vec![TxOut {
                value: Amount::from_sat(312_500_000),
                script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
            }],
        }
    }

    fn job(tx: &Transaction) -> NewExtendedMiningJobOwned {
        let encoded = serialize(tx);
        let prefix_len = 4 + usize::from(!tx.input[0].witness.is_empty()) * 2 + 1 + 36 + 1 + 4;
        NewExtendedMiningJobOwned {
            channel_id: 11,
            job_id: 22,
            min_ntime: Sv2OptionOwned::new(Some(1_700_000_000)),
            version: 0x2000_0000,
            version_rolling_allowed: true,
            merkle_path: Seq0255Owned::new(vec![[7; 32].into()]).unwrap(),
            coinbase_tx_prefix: encoded[..prefix_len].to_vec().try_into().unwrap(),
            coinbase_tx_suffix: encoded[prefix_len + EXTRANONCE.len()..]
                .to_vec()
                .try_into()
                .unwrap(),
        }
    }

    fn coinbase(job: &NewExtendedMiningJobOwned) -> Transaction {
        deserialize(
            &[
                job.coinbase_tx_prefix.as_bytes(),
                &EXTRANONCE,
                job.coinbase_tx_suffix.as_bytes(),
            ]
            .concat(),
        )
        .unwrap()
    }

    #[test]
    fn channel_output_has_reserved_tag_and_conservative_weight() {
        let marker = ChannelCoinbaseOutput([42; 24]);
        let output = marker.tx_out();
        assert_eq!(output.value, Amount::ZERO);
        assert_eq!(
            output.script_pubkey.as_bytes(),
            &[b"\x6a\x1dCHSEQ".as_slice(), &[42; 24]].concat()
        );
        assert_eq!(ChannelCoinbaseOutput::additional_coinbase_weight(), 168);
    }

    #[test]
    fn channel_output_preserves_outputs_witness_and_compact_size() {
        for witness in [false, true] {
            let mut tx = transaction(witness);
            tx.output.resize(250, tx.output[0].clone());
            tx.output
                .extend([b"other".as_slice(), b"DIFF32abcd"].map(|payload| TxOut {
                    value: Amount::ZERO,
                    script_pubkey: ScriptBuf::new_op_return(
                        <&bitcoin::script::PushBytes>::try_from(payload).unwrap(),
                    ),
                }));
            let mut job = job(&tx);
            let original_prefix = job.coinbase_tx_prefix.clone();
            let original_suffix_len = job.coinbase_tx_suffix.len();
            let marker = ChannelCoinbaseOutput([1; 24]);
            marker.apply(&mut job).unwrap();
            assert_eq!(&job.coinbase_tx_suffix.as_bytes()[4..7], &[0xfd, 0xfd, 0]);
            assert_eq!(
                job.coinbase_tx_suffix.len() - original_suffix_len,
                ChannelCoinbaseOutput::additional_coinbase_weight() / 4
            );
            let applied_suffix = job.coinbase_tx_suffix.clone();
            marker.apply(&mut job).unwrap();
            assert_eq!(job.coinbase_tx_suffix, applied_suffix);
            assert_eq!(job.coinbase_tx_prefix, original_prefix);
            assert_eq!(job.channel_id, 11);
            assert_eq!(job.job_id, 22);
            assert_eq!(job.version, 0x2000_0000);
            assert!(job.version_rolling_allowed);
            assert_eq!(job.min_ntime, Sv2OptionOwned::new(Some(1_700_000_000)));
            assert_eq!(job.merkle_path.as_slice().len(), 1);
            assert_eq!(job.merkle_path.as_slice()[0].as_bytes(), &[7; 32]);
            tx.output.push(marker.tx_out());
            assert_eq!(coinbase(&job), tx);
        }
    }

    #[test]
    fn channel_output_replaces_first_marker_and_removes_only_reserved_duplicates() {
        let mut tx = transaction(true);
        let original = ChannelCoinbaseOutput([1; 24]);
        let replacement = ChannelCoinbaseOutput([2; 24]);
        let mut funded_lookalike = original.tx_out();
        funded_lookalike.value = Amount::from_sat(1);
        let mut short_lookalike = original.tx_out();
        short_lookalike.script_pubkey = ScriptBuf::from_bytes(b"\x6a\x1dCHSEQ".to_vec());
        tx.output.extend([
            original.tx_out(),
            funded_lookalike,
            replacement.tx_out(),
            short_lookalike,
            original.tx_out(),
        ]);
        let mut job = job(&tx);
        replacement.apply(&mut job).unwrap();
        let expected = vec![
            tx.output[0].clone(),
            replacement.tx_out(),
            tx.output[2].clone(),
            tx.output[4].clone(),
        ];
        tx.output = expected;
        assert_eq!(coinbase(&job), tx);
        let applied = job.coinbase_tx_suffix.clone();
        replacement.apply(&mut job).unwrap();
        assert_eq!(job.coinbase_tx_suffix, applied);
    }

    #[test]
    fn appended_outputs_precede_witness_and_locktime() {
        for witness in [false, true] {
            let mut tx = transaction(witness);
            let mut job = job(&tx);
            let output = TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_op_return(b"DIFF32abcd"),
            };
            append_coinbase_output(&mut job, output.clone()).unwrap();
            tx.output.push(output);
            assert_eq!(coinbase(&job), tx);
        }
    }

    #[test]
    fn malformed_suffixes_fail_without_mutating_native_jobs() {
        for witness in [false, true] {
            let good = job(&transaction(witness));
            let suffix = good.coinbase_tx_suffix.as_bytes();
            // Every strict truncation loses some required sequence, output, witness, or locktime.
            for length in 0..suffix.len() {
                let mut broken = good.clone();
                broken.coinbase_tx_suffix = suffix[..length].to_vec().try_into().unwrap();
                let original = broken.coinbase_tx_suffix.clone();
                assert!(matches!(
                    ChannelCoinbaseOutput([1; 24]).apply(&mut broken),
                    Err(Error::InvalidCoinbase)
                ));
                assert_eq!(broken.coinbase_tx_suffix, original);
                assert_eq!(broken.coinbase_tx_prefix, good.coinbase_tx_prefix);
            }
            let mut malformed = vec![
                [&suffix[..4], &[0xfd, 1, 0], &suffix[5..]].concat(),
                [suffix, &[0]].concat(),
                [&suffix[..4], &[0xff; 9], &suffix[5..]].concat(),
            ];
            if witness {
                let (_, output_len): (Vec<TxOut>, usize) =
                    deserialize_partial(&suffix[4..]).unwrap();
                let witness_offset = 4 + output_len;
                malformed
                    .push([&suffix[..witness_offset], &[0], &suffix[suffix.len() - 4..]].concat());
                malformed.push(
                    [
                        &suffix[..witness_offset],
                        &[1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
                        &suffix[suffix.len() - 4..],
                    ]
                    .concat(),
                );
            }
            for suffix in malformed {
                let mut broken = good.clone();
                broken.coinbase_tx_suffix = suffix.try_into().unwrap();
                let original = broken.coinbase_tx_suffix.clone();
                assert!(matches!(
                    ChannelCoinbaseOutput([1; 24]).apply(&mut broken),
                    Err(Error::InvalidCoinbase)
                ));
                assert_eq!(broken.coinbase_tx_suffix, original);
            }
        }
    }

    #[test]
    fn oversized_native_suffix_fails_without_mutation() {
        let mut tx = transaction(false);
        tx.output[0].script_pubkey = ScriptBuf::from_bytes(vec![0x51; 65_500]);
        let mut job = job(&tx);
        let original_suffix = job.coinbase_tx_suffix.clone();
        assert!(matches!(
            ChannelCoinbaseOutput([1; 24]).apply(&mut job),
            Err(Error::BinarySv2Error(_))
        ));
        assert_eq!(job.coinbase_tx_suffix, original_suffix);
        assert_eq!(coinbase(&job), tx);
    }
}
