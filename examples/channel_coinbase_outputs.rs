use binary_sv2::Seq0255Owned;
use bitcoin::{consensus, Amount, ScriptBuf, Transaction, TxOut};
use mining_sv2::{NewExtendedMiningJobOwned, SubmitSharesExtendedOwned};
use roles_logic_sv2::{
    channel_logic::channel_factory::{ExtendedChannelKind, OnNewShare, PoolChannelFactory},
    job_creator::JobsCreators,
    mining::ExtendedExtranonce,
    utils::{GroupId, Mutex},
    Error,
};
use std::sync::Arc;
use template_distribution_sv2::{NewTemplateOwned, SetNewPrevHashOwned};

// Rebuild the output vector without treating the witness as part of the output records.
fn append_local_output(
    job: &mut NewExtendedMiningJobOwned,
    full_extranonce: &[u8],
    output: TxOut,
) -> Result<Vec<u8>, Error> {
    let bytes = [
        job.coinbase_tx_prefix.as_bytes(),
        full_extranonce,
        job.coinbase_tx_suffix.as_bytes(),
    ]
    .concat();
    let mut tx: Transaction = consensus::deserialize(&bytes).map_err(|_| Error::InvalidCoinbase)?;
    tx.output.push(output);
    let bytes = consensus::serialize(&tx);
    let split = job.coinbase_tx_prefix.len() + full_extranonce.len();
    job.coinbase_tx_suffix = bytes[split..].to_vec().try_into()?;
    Ok(bytes)
}

fn main() -> Result<(), Error> {
    let mut factory = PoolChannelFactory::new(
        Arc::new(Mutex::new(GroupId::new())),
        ExtendedExtranonce::new(0..0, 0..8, 8..16),
        JobsCreators::new(16),
        1.,
        ExtendedChannelKind::Pool,
        vec![TxOut {
            value: Amount::ZERO, // Filled from the template's remaining reward.
            script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
        }],
        vec![],
    )?
    .with_channel_coinbase_outputs();
    for request_id in [1, 2] {
        factory.new_extended_channel(request_id, 1_000_000., 8)?;
    }

    let mut template = NewTemplateOwned {
        template_id: 1,
        future_template: true,
        version: 0x2000_0000,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03].try_into()?,
        coinbase_tx_input_sequence: u32::MAX,
        coinbase_tx_value_remaining: 312_500_000,
        coinbase_tx_outputs_count: 0,
        coinbase_tx_outputs: vec![].try_into()?,
        coinbase_tx_locktime: 0,
        merkle_path: Seq0255Owned::new(vec![])?,
    };
    let (shared_job, channel_ids, standard_messages) =
        factory.on_new_template_for_extended_fanout(&mut template)?;
    assert!(standard_messages.is_empty());
    factory.on_new_prev_hash_from_tp(&SetNewPrevHashOwned {
        template_id: 1,
        prev_hash: [9; 32].into(),
        header_timestamp: 1_700_000_000,
        n_bits: 0x207f_ffff,
        // This example accepts every hash to inspect reconstructed solution coinbases.
        target: [255; 32].into(),
    })?;

    for channel_id in channel_ids {
        let mut job = shared_job.clone();
        job.channel_id = channel_id;
        let marker = factory.channel_coinbase_output(channel_id).unwrap();
        marker.apply(&mut job)?;
        let channel = factory.extended_channel(channel_id).unwrap();
        let miner_extranonce = vec![0; usize::from(channel.extranonce_size)];
        let full_extranonce = [channel.extranonce_prefix.as_bytes(), &miner_extranonce].concat();
        let local_output = TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(b"local"),
        };
        let sent_coinbase = append_local_output(&mut job, &full_extranonce, local_output.clone())?;
        factory.register_extended_job_coinbase_output(channel_id, job.job_id, local_output);

        let outcome = factory.on_submit_shares_extended(SubmitSharesExtendedOwned {
            channel_id,
            sequence_number: 1,
            job_id: job.job_id,
            nonce: 0,
            ntime: 1_700_000_000,
            version: job.version,
            extranonce: miner_extranonce.try_into()?,
        })?;
        match outcome {
            OnNewShare::ShareMeetBitcoinTarget((_, _, validated_coinbase, _)) => {
                assert_eq!(validated_coinbase, sent_coinbase);
            }
            other => panic!("expected an example solution, got {other:?}"),
        }
    }
    println!("Each channel's share reconstructs its marked coinbase and local output exactly.");
    Ok(())
}
