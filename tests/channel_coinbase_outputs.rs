use binary_sv2::Seq0255Owned;
use bitcoin::{
    block::{Header, Version},
    consensus,
    hashes::Hash,
    Amount, BlockHash, CompactTarget, ScriptBuf, Transaction, TxOut,
};
use mining_sv2::{
    NewExtendedMiningJobOwned, OpenExtendedMiningChannelSuccessOwned, SetCustomMiningJobOwned,
    SubmitSharesExtendedOwned, SubmitSharesStandard,
};
use roles_logic_sv2::{
    channel_logic::channel_factory::{
        ChannelCoinbaseOutput, ExtendedChannelKind, OnNewShare, PoolChannelFactory,
    },
    job_creator::JobsCreators,
    mining::{ExtendedExtranonce, Target},
    parsers::MiningOwned,
    utils::{GroupId, Mutex},
    Error,
};
use std::sync::Arc;
use template_distribution_sv2::{NewTemplateOwned, SetNewPrevHashOwned};

const VERSION: u32 = 0x2000_0000;
const TIME: u32 = 1_700_000_000;
const NBITS: u32 = 0x207f_ffff;

fn payout() -> TxOut {
    TxOut {
        value: Amount::from_sat(50_000),
        script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
    }
}

fn pool() -> PoolChannelFactory {
    PoolChannelFactory::new(
        Arc::new(Mutex::new(GroupId::new())),
        ExtendedExtranonce::new(0..0, 0..8, 8..16),
        JobsCreators::new(16),
        1.,
        ExtendedChannelKind::Pool,
        vec![payout()],
        vec![],
    )
    .unwrap()
}

fn template(id: u64) -> NewTemplateOwned {
    NewTemplateOwned {
        template_id: id,
        future_template: true,
        version: VERSION,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03].try_into().unwrap(),
        coinbase_tx_input_sequence: u32::MAX,
        coinbase_tx_value_remaining: 50_000,
        coinbase_tx_outputs_count: 0,
        coinbase_tx_outputs: vec![].try_into().unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: Seq0255Owned::new(vec![]).unwrap(),
    }
}

fn prev_hash(id: u64) -> SetNewPrevHashOwned {
    SetNewPrevHashOwned {
        template_id: id,
        prev_hash: [9; 32].into(),
        header_timestamp: TIME,
        n_bits: NBITS,
        target: [255; 32].into(),
    }
}

fn open(factory: &mut PoolChannelFactory) -> OpenExtendedMiningChannelSuccessOwned {
    match factory
        .new_extended_channel(7, 1_000., 8)
        .unwrap()
        .remove(0)
    {
        MiningOwned::OpenExtendedMiningChannelSuccess(m) => m,
        other => panic!("expected extended-channel assignment, got {other:?}"),
    }
}

fn job(message: MiningOwned) -> NewExtendedMiningJobOwned {
    match message {
        MiningOwned::NewExtendedMiningJob(job) => job,
        other => panic!("expected extended job, got {other:?}"),
    }
}

fn transaction(job: &NewExtendedMiningJobOwned, extranonce: &[u8]) -> Transaction {
    consensus::deserialize(
        &[
            job.coinbase_tx_prefix.as_bytes(),
            extranonce,
            job.coinbase_tx_suffix.as_bytes(),
        ]
        .concat(),
    )
    .unwrap()
}

fn markers(tx: &Transaction) -> Vec<&TxOut> {
    tx.output
        .iter()
        .filter(|output| {
            output
                .script_pubkey
                .as_bytes()
                .starts_with(b"\x6a\x1dCHSEQ")
        })
        .collect()
}

fn sequence(output: ChannelCoinbaseOutput) -> [u8; 24] {
    let tx_out = output.tx_out();
    assert_eq!(tx_out.value, Amount::ZERO);
    assert!(tx_out
        .script_pubkey
        .as_bytes()
        .starts_with(b"\x6a\x1dCHSEQ"));
    tx_out.script_pubkey.as_bytes()[7..].try_into().unwrap()
}

fn assert_marker(job: &NewExtendedMiningJobOwned, marker: ChannelCoinbaseOutput) {
    let tx = transaction(job, &[0; 16]);
    assert_eq!(markers(&tx), [&marker.tx_out()]);
}

fn replace_transaction(job: &mut NewExtendedMiningJobOwned, tx: &Transaction) {
    let encoded = consensus::serialize(tx);
    let split = job.coinbase_tx_prefix.as_bytes().len() + 16;
    job.coinbase_tx_suffix = encoded[split..].to_vec().try_into().unwrap();
}

fn share(channel_id: u32, job_id: u32) -> SubmitSharesExtendedOwned {
    SubmitSharesExtendedOwned {
        channel_id,
        sequence_number: 1,
        job_id,
        nonce: 0,
        ntime: TIME,
        version: VERSION,
        extranonce: vec![0; 8].try_into().unwrap(),
    }
}

fn accepted_coinbase(outcome: OnNewShare) -> Transaction {
    match outcome {
        OnNewShare::ShareMeetBitcoinTarget((_, _, bytes, _)) => {
            consensus::deserialize(&bytes).unwrap()
        }
        other => panic!("expected bitcoin-target share, got {other:?}"),
    }
}

fn custom(channel_id: u32, outputs: Vec<TxOut>) -> SetCustomMiningJobOwned {
    SetCustomMiningJobOwned {
        channel_id,
        request_id: 42,
        token: vec![1].try_into().unwrap(),
        version: VERSION,
        prev_hash: [9; 32].into(),
        min_ntime: TIME,
        nbits: NBITS,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03].try_into().unwrap(),
        coinbase_tx_input_n_sequence: u32::MAX,
        coinbase_tx_outputs: consensus::serialize(&outputs).try_into().unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: Seq0255Owned::new(vec![]).unwrap(),
    }
}

#[test]
fn marker_is_opt_in_and_duplicate_prefixes_remain_rejected_without_it() {
    let mut factory = pool();
    let first = open(&mut factory);
    assert!(factory.channel_coinbase_output(first.channel_id).is_none());
    let mut restored = first.clone();
    restored.channel_id = 99;
    assert!(matches!(
        factory.restore_extended_channel(restored, vec![]),
        Err(Error::ExtranoncePrefixAlreadyInUse)
    ));
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let canonical = job(messages[&first.channel_id].clone());
    assert!(markers(&transaction(&canonical, &[0; 16])).is_empty());
    assert!(factory
        .on_new_set_custom_mining_job(custom(first.channel_id, vec![payout()]))
        .is_ok());
}

#[test]
fn markers_share_a_factory_prefix_and_count_successful_opens_and_restores() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let first_marker = factory.channel_coinbase_output(first.channel_id).unwrap();
    factory = factory.with_channel_coinbase_outputs();
    assert_eq!(
        factory.channel_coinbase_output(first.channel_id),
        Some(first_marker)
    );
    assert!(matches!(
        factory.new_extended_channel(8, 1_000., 9).unwrap()[0],
        MiningOwned::OpenMiningChannelError(_)
    ));
    let second = open(&mut factory);
    let second_marker = factory.channel_coinbase_output(second.channel_id).unwrap();
    let mut restored = first.clone();
    restored.channel_id = 99;
    factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    let third_marker = factory.channel_coinbase_output(99).unwrap();
    for (counter, marker) in [(1_u64, first_marker), (2, second_marker), (3, third_marker)] {
        let bytes = sequence(marker);
        assert_eq!(&bytes[..16], &sequence(first_marker)[..16]);
        assert_eq!(&bytes[16..], &counter.to_be_bytes());
    }
    factory.close_channel(second.channel_id);
    let fourth = open(&mut factory);
    assert_eq!(
        &sequence(factory.channel_coinbase_output(fourth.channel_id).unwrap())[16..],
        &4_u64.to_be_bytes()
    );
    let mut restarted = pool().with_channel_coinbase_outputs();
    restarted
        .restore_extended_channel(restored, vec![])
        .unwrap();
    let restarted_sequence = sequence(restarted.channel_coinbase_output(99).unwrap());
    assert_ne!(&restarted_sequence[..16], &sequence(first_marker)[..16]);
    assert_eq!(&restarted_sequence[16..], &1_u64.to_be_bytes());
}

#[test]
fn markers_cover_template_delivery_cached_open_restore_and_external_fanout() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    assert_marker(
        &job(messages[&first.channel_id].clone()),
        factory.channel_coinbase_output(first.channel_id).unwrap(),
    );
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    factory.on_new_template(&mut template(11)).unwrap();
    let current = factory
        .current_extended_job_for_channel(first.channel_id)
        .unwrap();
    assert_marker(
        &current,
        factory.channel_coinbase_output(first.channel_id).unwrap(),
    );

    let opened = factory.new_extended_channel(8, 1_000., 8).unwrap();
    let second_id = match &opened[0] {
        MiningOwned::OpenExtendedMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected extended-channel assignment"),
    };
    let mut restored = first;
    restored.channel_id = 99;
    restored.group_channel_id = 77;
    let restored_messages = factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    assert_eq!(factory.extended_channel(99), Some(&restored));
    for (channel_id, messages) in [(second_id, opened), (99, restored_messages)] {
        let cached: Vec<_> = messages
            .into_iter()
            .filter_map(|message| match message {
                MiningOwned::NewExtendedMiningJob(job) => Some(job),
                _ => None,
            })
            .collect();
        assert_eq!(cached.len(), 2);
        for job in cached {
            assert_marker(&job, factory.channel_coinbase_output(channel_id).unwrap());
        }
    }
    let (base, ids, standard_messages) = factory
        .on_new_template_for_extended_fanout(&mut template(12))
        .unwrap();
    assert!(standard_messages.is_empty());
    assert!(markers(&transaction(&base, &[0; 16])).is_empty());
    assert_eq!(ids.len(), 3);
    for id in ids {
        let marker = factory.channel_coinbase_output(id).unwrap();
        let mut materialized = base.clone();
        marker.apply(&mut materialized).unwrap();
        marker.apply(&mut materialized).unwrap();
        assert_marker(&materialized, marker);
    }
}

#[test]
fn failed_cached_job_materialization_does_not_open_a_channel_or_consume_a_marker() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let mut large = payout();
    large.script_pubkey = ScriptBuf::from_bytes(vec![0; 65_470]);
    factory.update_pool_outputs(vec![large]);
    factory.on_new_template(&mut template(10)).unwrap();
    assert!(factory.new_extended_channel(7, 1_000., 8).is_err());
    assert!(factory.get_extended_channels_ids().is_empty());

    factory.update_pool_outputs(vec![payout()]);
    factory.on_new_template(&mut template(11)).unwrap();
    factory.on_new_prev_hash_from_tp(&prev_hash(11)).unwrap();
    let channel = open(&mut factory);
    assert_eq!(
        &sequence(factory.channel_coinbase_output(channel.channel_id).unwrap())[16..],
        &1_u64.to_be_bytes()
    );
}

#[test]
fn failed_restoration_preserves_existing_group_reservations_and_leaves_channel_id_available() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let group = factory.new_group_id();
    let mut large = payout();
    large.script_pubkey = ScriptBuf::from_bytes(vec![0; 65_470]);
    factory.update_pool_outputs(vec![large]);
    factory.on_new_template(&mut template(10)).unwrap();
    let assignment = OpenExtendedMiningChannelSuccessOwned {
        request_id: 7,
        channel_id: 99,
        target: [255; 32].into(),
        extranonce_prefix: vec![7; 8].try_into().unwrap(),
        extranonce_size: 8,
        group_channel_id: group,
    };
    assert!(matches!(
        factory.restore_extended_channel(assignment.clone(), vec![]),
        Err(Error::BinarySv2Error(_))
    ));
    assert!(factory.extended_channel(99).is_none());
    assert!(factory.channel_coinbase_output(99).is_none());

    factory.update_pool_outputs(vec![payout()]);
    factory.on_new_template(&mut template(11)).unwrap();
    factory.on_new_prev_hash_from_tp(&prev_hash(11)).unwrap();
    let mut colliding = assignment;
    colliding.channel_id = group;
    colliding.group_channel_id = 0;
    assert!(matches!(
        factory.restore_extended_channel(colliding.clone(), vec![]),
        Err(Error::ChannelIdAlreadyInUse(id)) if id == group
    ));
    // Failed restoration must not leave a sparse reservation for channel 99 either.
    for expected in group + 1..=99 {
        assert_eq!(factory.new_standard_id_for_hom(), expected);
    }
    let opened = open(&mut factory);
    assert_eq!(
        &sequence(factory.channel_coinbase_output(opened.channel_id).unwrap())[16..],
        &1_u64.to_be_bytes()
    );
    factory.close_group(group);
    assert!(factory.restore_extended_channel(colliding, vec![]).is_ok());
}

#[test]
fn oversized_marked_templates_preserve_retained_jobs_input_and_next_job_id() {
    for external_fanout in [false, true] {
        for future_template in [false, true] {
            let mut factory = pool().with_channel_coinbase_outputs();
            let channel = open(&mut factory);
            let mut retained = Vec::new();
            for index in 0..3 {
                let mut next = template(10 + index);
                next.future_template = index == 0;
                next.coinbase_tx_value_remaining = 100 * (index + 1);
                let messages = factory.on_new_template(&mut next).unwrap();
                let created = job(messages[&channel.channel_id].clone());
                retained.push((created.job_id, next.coinbase_tx_value_remaining));
                if index == 0 {
                    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
                }
            }
            let current = factory
                .current_extended_job_for_channel(channel.channel_id)
                .unwrap();
            let mut rejected = template(13);
            rejected.future_template = future_template;
            let mut large = payout();
            large.script_pubkey = ScriptBuf::from_bytes(vec![0; 65_460]);
            rejected.coinbase_tx_outputs_count = 1;
            rejected.coinbase_tx_outputs = consensus::serialize(&large).try_into().unwrap();
            let before = rejected.clone();
            let error = if external_fanout {
                factory
                    .on_new_template_for_extended_fanout(&mut rejected)
                    .map(|_| ())
            } else {
                factory.on_new_template(&mut rejected).map(|_| ())
            };
            assert!(matches!(error, Err(Error::BinarySv2Error(_))),
                "overflow must reject before registration: external_fanout={external_fanout}, future_template={future_template}");
            assert_eq!(rejected, before);
            assert_eq!(
                factory.current_extended_job_for_channel(channel.channel_id),
                Some(current.clone())
            );
            for (job_id, amount) in retained {
                let tx = accepted_coinbase(
                    factory
                        .on_submit_shares_extended(share(channel.channel_id, job_id))
                        .unwrap(),
                );
                assert_eq!(tx.output[0].value.to_sat(), amount);
            }

            let mut retry = template(rejected.template_id);
            retry.future_template = future_template;
            let next_job = if external_fanout {
                factory
                    .on_new_template_for_extended_fanout(&mut retry)
                    .unwrap()
                    .0
            } else {
                job(factory
                    .on_new_template(&mut retry)
                    .unwrap()
                    .remove(&channel.channel_id)
                    .unwrap())
            };
            assert_eq!(next_job.job_id, current.job_id + 1);
            if future_template {
                assert_eq!(
                    factory.current_extended_job_for_channel(channel.channel_id),
                    Some(current)
                );
                assert_eq!(
                    factory.on_new_prev_hash_from_tp(&prev_hash(13)).unwrap(),
                    next_job.job_id
                );
            }
            let tx = accepted_coinbase(
                factory
                    .on_submit_shares_extended(share(channel.channel_id, next_job.job_id))
                    .unwrap(),
            );
            assert_eq!(tx.output[0].value, payout().value);
        }
    }
}

#[test]
fn marker_preserves_compact_size_witness_sequence_locktime_and_other_outputs() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let second = open(&mut factory);
    let marker = factory.channel_coinbase_output(first.channel_id).unwrap();
    let other = factory.channel_coinbase_output(second.channel_id).unwrap();
    let (mut job, _, _) = factory
        .on_new_template_for_extended_fanout(&mut template(10))
        .unwrap();
    let mut original = transaction(&job, &[0; 16]);
    assert!(!original.input[0].witness.is_empty());
    original.output.resize(250, payout());
    original.output.extend([
        TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(b"unrelated"),
        },
        TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(b"DIFF32abcd"),
        },
    ]);
    replace_transaction(&mut job, &original);
    let before = job.clone();
    marker.apply(&mut job).unwrap();
    assert_eq!(&job.coinbase_tx_suffix.as_bytes()[4..7], &[0xfd, 0xfd, 0]);
    assert_eq!(
        job.coinbase_tx_suffix.as_bytes().len() - before.coinbase_tx_suffix.as_bytes().len(),
        ChannelCoinbaseOutput::additional_coinbase_weight() / 4
    );
    marker.apply(&mut job).unwrap();
    let composed = transaction(&job, &[0; 16]);
    assert_eq!(job.coinbase_tx_prefix, before.coinbase_tx_prefix);
    assert_eq!(composed.input, original.input);
    assert_eq!(composed.version, original.version);
    assert_eq!(composed.lock_time, original.lock_time);
    assert_eq!(composed.output[..252], original.output);
    assert_eq!(markers(&composed), [&marker.tx_out()]);

    let mut duplicates = composed;
    duplicates.output.extend([other.tx_out(), marker.tx_out()]);
    replace_transaction(&mut job, &duplicates);
    marker.apply(&mut job).unwrap();
    assert_eq!(transaction(&job, &[0; 16]).output.len(), 253);
    assert_marker(&job, marker);
}

#[test]
fn colliding_extranonces_validate_distinct_share_hashes_with_optional_job_outputs() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let mut second = first.clone();
    second.channel_id = 99;
    factory
        .restore_extended_channel(second.clone(), vec![])
        .unwrap();
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let first_job = job(messages[&first.channel_id].clone());
    let second_job = job(messages[&second.channel_id].clone());
    let mut tip = prev_hash(10);
    let mut target_bytes = [255; 32];
    target_bytes[31] = 127;
    tip.target = target_bytes.into();
    let target = Target::from(target_bytes);
    factory.on_new_prev_hash_from_tp(&tip).unwrap();
    for id in [first.channel_id, second.channel_id] {
        assert!(factory
            .update_target_for_channel(id, Target::from([0; 32]))
            .is_some());
    }
    let extranonce = [first.extranonce_prefix.as_bytes(), &[0; 8]].concat();
    for local_output in [
        None,
        Some(TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(b"DIFF32abcd"),
        }),
    ] {
        let mut first_tx = transaction(&first_job, &extranonce);
        let mut second_tx = transaction(&second_job, &extranonce);
        if let Some(output) = local_output {
            first_tx.output.push(output.clone());
            second_tx.output.push(output.clone());
            for id in [first.channel_id, second.channel_id] {
                factory.register_extended_job_coinbase_output(id, first_job.job_id, output.clone());
            }
        }
        assert_ne!(first_tx.compute_txid(), second_tx.compute_txid());
        let hash = |tx: &Transaction, nonce: u32| -> Target {
            Header {
                version: Version::from_consensus(VERSION as i32),
                prev_blockhash: BlockHash::from_byte_array([9; 32]),
                merkle_root: tx.compute_txid().to_raw_hash().into(),
                time: TIME,
                bits: CompactTarget::from_consensus(NBITS),
                nonce,
            }
            .block_hash()
            .to_byte_array()
            .into()
        };
        let nonce = (0..10_000)
            .find(|nonce| hash(&second_tx, *nonce) <= target && hash(&first_tx, *nonce) > target)
            .unwrap();
        let mut submitted = share(first.channel_id, first_job.job_id);
        submitted.nonce = nonce;
        assert!(matches!(
            factory
                .on_submit_shares_extended(submitted.clone())
                .unwrap(),
            OnNewShare::SendErrorDownstream(_)
        ));
        submitted.channel_id = second.channel_id;
        assert_eq!(
            accepted_coinbase(factory.on_submit_shares_extended(submitted).unwrap()),
            second_tx
        );
        let mut submitted = share(first.channel_id, first_job.job_id);
        submitted.nonce = (0..10_000)
            .find(|nonce| hash(&first_tx, *nonce) <= target)
            .unwrap();
        assert_eq!(
            accepted_coinbase(factory.on_submit_shares_extended(submitted).unwrap()),
            first_tx
        );
    }
}

#[test]
fn arbitrary_suffix_variants_are_preserved_alongside_stable_markers() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let second = open(&mut factory);
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let mut variant = job(messages[&first.channel_id].clone());
    let mut tx = transaction(&variant, &[0; 16]);
    tx.output[0].value = Amount::from_sat(23_456);
    tx.lock_time = bitcoin::absolute::LockTime::from_consensus(123);
    // A legacy caller may submit an unmarked suffix; validation still installs this channel's marker.
    tx.output.retain(|output| {
        !output
            .script_pubkey
            .as_bytes()
            .starts_with(b"\x6a\x1dCHSEQ")
    });
    replace_transaction(&mut variant, &tx);
    factory.register_extended_job_variant(
        first.channel_id,
        variant.job_id,
        variant.coinbase_tx_suffix.as_bytes().to_vec(),
    );
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    let actual = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(first.channel_id, variant.job_id))
            .unwrap(),
    );
    assert_eq!(actual.output[0].value, Amount::from_sat(23_456));
    assert_eq!(actual.lock_time.to_consensus_u32(), 123);
    assert_eq!(
        markers(&actual),
        [&factory
            .channel_coinbase_output(first.channel_id)
            .unwrap()
            .tx_out()]
    );
    let canonical = factory
        .current_extended_job_for_channel(first.channel_id)
        .unwrap();
    assert_eq!(
        transaction(&canonical, &[0; 16]).output[0].value,
        Amount::from_sat(50_000)
    );
    let unaffected = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(second.channel_id, variant.job_id))
            .unwrap(),
    );
    assert_eq!(unaffected.output[0].value, Amount::from_sat(50_000));
    assert_eq!(unaffected.lock_time.to_consensus_u32(), 0);
}

#[test]
fn explicit_outputs_are_scoped_to_channel_and_job_and_removed_when_the_channel_closes() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let second = open(&mut factory);
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let old_job = job(messages[&first.channel_id].clone());
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    let mut new_template = template(11);
    new_template.future_template = false;
    let messages = factory.on_new_template(&mut new_template).unwrap();
    let new_job = job(messages[&first.channel_id].clone());
    assert_ne!(new_job.job_id, old_job.job_id);
    let pairs = [
        (first.channel_id, old_job.job_id, b"DIFF32abcd"),
        (first.channel_id, new_job.job_id, b"DIFF32efgh"),
        (second.channel_id, old_job.job_id, b"DIFF32ijkl"),
        (second.channel_id, new_job.job_id, b"DIFF32mnop"),
    ];
    for (channel_id, job_id, payload) in pairs {
        factory.register_extended_job_coinbase_output(
            channel_id,
            job_id,
            TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_op_return(payload),
            },
        );
    }
    for (channel_id, job_id, payload) in pairs {
        let tx = accepted_coinbase(
            factory
                .on_submit_shares_extended(share(channel_id, job_id))
                .unwrap(),
        );
        assert_eq!(
            tx.output.last().unwrap().script_pubkey,
            ScriptBuf::new_op_return(payload)
        );
        assert_eq!(
            markers(&tx),
            [&factory
                .channel_coinbase_output(channel_id)
                .unwrap()
                .tx_out()]
        );
        let canonical = factory
            .current_extended_job_for_channel(channel_id)
            .unwrap();
        assert_eq!(transaction(&canonical, &[0; 16]).output.len(), 2);
    }
    factory.close_channel(second.channel_id);
    factory
        .restore_extended_channel(second.clone(), vec![])
        .unwrap();
    let tx = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(second.channel_id, old_job.job_id))
            .unwrap(),
    );
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        markers(&tx),
        [&factory
            .channel_coinbase_output(second.channel_id)
            .unwrap()
            .tx_out()]
    );
}

#[test]
fn custom_markers_are_validated_without_consuming_ids_or_discarding_earlier_jobs() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let mut second = first.clone();
    second.channel_id = 99;
    factory.restore_extended_channel(second, vec![]).unwrap();
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    let correct = factory
        .channel_coinbase_output(first.channel_id)
        .unwrap()
        .tx_out();
    let wrong = factory.channel_coinbase_output(99).unwrap().tx_out();
    let mut nonzero = correct.clone();
    nonzero.value = Amount::from_sat(1);
    let original = custom(first.channel_id, vec![payout(), correct.clone()]);
    let accepted = factory.on_new_set_custom_mining_job(original).unwrap();
    for invalid in [
        vec![],
        vec![wrong.clone()],
        vec![correct.clone(), correct.clone()],
        vec![correct.clone(), wrong],
        vec![nonzero],
    ] {
        let mut outputs = vec![payout()];
        outputs.extend(invalid);
        let rejected = custom(first.channel_id, outputs);
        let error = factory
            .try_on_new_set_custom_mining_job(rejected.clone())
            .unwrap_err();
        assert_eq!(error.channel_id, first.channel_id);
        assert_eq!(error.request_id, 42);
        assert_eq!(
            error.error_code.as_bytes(),
            mining_sv2::ERROR_CODE_SET_CUSTOM_MINING_JOB_INVALID_COINBASE_TX_OUTPUTS.as_bytes()
        );
        assert!(factory.on_new_set_custom_mining_job(rejected).is_err());
    }
    let mut malformed = custom(first.channel_id, vec![payout(), correct.clone()]);
    malformed.coinbase_tx_outputs = [malformed.coinbase_tx_outputs.as_bytes(), &[0]]
        .concat()
        .try_into()
        .unwrap();
    assert!(factory.on_new_set_custom_mining_job(malformed).is_err());
    let mut changed_payout = payout();
    changed_payout.value = Amount::from_sat(123);
    let newer = factory
        .on_new_set_custom_mining_job(custom(
            first.channel_id,
            vec![changed_payout.clone(), correct.clone()],
        ))
        .unwrap();
    assert_eq!(newer.job_id, accepted.job_id + 1);
    for (id, expected) in [(newer.job_id, changed_payout), (accepted.job_id, payout())] {
        let tx = accepted_coinbase(
            factory
                .on_submit_shares_extended(share(first.channel_id, id))
                .unwrap(),
        );
        assert_eq!(tx.output, [expected, correct.clone()]);
    }
    assert!(matches!(
        factory.on_submit_shares_extended(share(first.channel_id, newer.job_id + 1)),
        Err(Error::ShareDoNotMatchAnyJob)
    ));
    let second_marker = factory.channel_coinbase_output(99).unwrap().tx_out();
    let other = factory
        .on_new_set_custom_mining_job(custom(99, vec![payout(), second_marker.clone()]))
        .unwrap();
    let other_tx = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(99, other.job_id))
            .unwrap(),
    );
    assert_eq!(other_tx.output, [payout(), second_marker]);
    for (channel_id, wrong_job_id) in [(first.channel_id, other.job_id), (99, accepted.job_id)] {
        assert!(matches!(
            factory.on_submit_shares_extended(share(channel_id, wrong_job_id)),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
    }
    assert!(factory
        .current_extended_job_for_channel(first.channel_id)
        .is_none());
}

#[test]
fn marked_custom_jobs_keep_the_existing_cache_bound_and_expire_on_tip_changes() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let channel = open(&mut factory);
    factory.on_new_template(&mut template(10)).unwrap();
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    let marker = factory
        .channel_coinbase_output(channel.channel_id)
        .unwrap()
        .tx_out();
    let mut acknowledged = Vec::new();
    for amount in 1..=40 {
        let mut output = payout();
        output.value = Amount::from_sat(amount);
        let success = factory
            .on_new_set_custom_mining_job(custom(channel.channel_id, vec![output, marker.clone()]))
            .unwrap();
        acknowledged.push((success.job_id, amount));
    }
    for (id, amount) in &acknowledged[8..] {
        let tx = accepted_coinbase(
            factory
                .on_submit_shares_extended(share(channel.channel_id, *id))
                .unwrap(),
        );
        assert_eq!(tx.output[0].value.to_sat(), *amount);
    }
    for (id, _) in &acknowledged[..8] {
        assert!(matches!(
            factory.on_submit_shares_extended(share(channel.channel_id, *id)),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
    }
    let mut next_tip = prev_hash(11);
    next_tip.prev_hash = [10; 32].into();
    factory.on_new_prev_hash_from_tp(&next_tip).unwrap();
    for (id, _) in acknowledged {
        assert!(matches!(
            factory.on_submit_shares_extended(share(channel.channel_id, id)),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
    }
    assert!(factory
        .current_extended_job_for_channel(channel.channel_id)
        .is_none());
}

#[test]
fn user_lookup_rejects_conflicts_and_is_cleared_when_channels_close() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let user = b"miner\x00\xff";
    assert!(matches!(
        factory.register_channel_user(u32::MAX, user),
        Err(Error::NotFoundChannelId)
    ));
    let first = open(&mut factory);
    let second = open(&mut factory);
    factory
        .register_channel_user(first.channel_id, user)
        .unwrap();
    factory
        .register_channel_user(first.channel_id, user)
        .unwrap();
    assert!(factory
        .register_channel_user(second.channel_id, user)
        .is_err());
    assert_eq!(factory.channel_id_for_user(user), Some(first.channel_id));
    factory
        .register_channel_user(second.channel_id, b"other")
        .unwrap();
    factory.close_channel(first.channel_id);
    assert_eq!(factory.channel_id_for_user(user), None);
    assert_eq!(
        factory.channel_id_for_user(b"other"),
        Some(second.channel_id)
    );
    assert!(factory.channel_coinbase_output(first.channel_id).is_none());
    let mut restored = first;
    restored.channel_id = 99;
    factory.restore_extended_channel(restored, vec![]).unwrap();
    factory.register_channel_user(99, user).unwrap();
    factory.close_channel(99);
    assert_eq!(factory.channel_id_for_user(user), None);
}

#[test]
fn grouped_standard_jobs_stay_unmarked_and_group_closure_preserves_extended_only_groups() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let group = factory.new_group_id();
    let opened = factory
        .add_standard_channel(8, 1_000., false, group)
        .unwrap();
    let standard_id = match &opened[0] {
        MiningOwned::OpenStandardMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected standard-channel assignment"),
    };
    let extended_group = factory.new_group_id();
    let mut restored = first.clone();
    restored.channel_id = 99;
    restored.group_channel_id = extended_group;
    factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    factory.register_channel_user(99, b"group-member").unwrap();
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let standard_job = job(messages[&group].clone());
    assert!(markers(&transaction(&standard_job, &[0; 16])).is_empty());
    assert_marker(
        &job(messages[&first.channel_id].clone()),
        factory.channel_coinbase_output(first.channel_id).unwrap(),
    );
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    let standard_share = SubmitSharesStandard {
        channel_id: standard_id,
        sequence_number: 1,
        job_id: standard_job.job_id,
        nonce: 0,
        ntime: TIME,
        version: VERSION,
    };
    assert!(matches!(
        factory.on_submit_shares_standard(standard_share.clone()),
        Ok(OnNewShare::ShareMeetBitcoinTarget(_))
    ));
    factory.close_group(group);
    factory.close_group(group);
    factory.close_group(extended_group);
    factory.close_group(extended_group);
    assert_eq!(factory.extended_channel(99), Some(&restored));
    assert_eq!(factory.channel_id_for_user(b"group-member"), Some(99));
    assert!(factory.on_submit_shares_standard(standard_share).is_err());
    factory.close_group(0);
    factory.close_group(u32::MAX);
    assert!(factory.extended_channel(first.channel_id).is_some());
    let tx = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(99, standard_job.job_id))
            .unwrap(),
    );
    assert_eq!(
        markers(&tx),
        [&factory.channel_coinbase_output(99).unwrap().tx_out()]
    );
}

#[test]
fn marked_extended_and_standard_groups_reject_mixing_before_changing_channel_state() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let marker = factory.channel_coinbase_output(first.channel_id).unwrap();
    let group = factory.new_group_id();
    let opened = factory
        .add_standard_channel(8, 1_000., false, group)
        .unwrap();
    let standard_id = match &opened[0] {
        MiningOwned::OpenStandardMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected standard-channel assignment"),
    };
    let mut restored = first.clone();
    restored.channel_id = 99;
    restored.group_channel_id = group;
    assert!(matches!(
        factory.restore_extended_channel(restored.clone(), vec![]),
        Err(Error::IncompatibleChannelGroup(id)) if id == group
    ));
    assert!(factory.extended_channel(99).is_none());
    assert_eq!(factory.extended_channel(first.channel_id), Some(&first));
    assert_eq!(
        factory.channel_coinbase_output(first.channel_id),
        Some(marker)
    );
    assert!(factory.channel_coinbase_output(99).is_none());
    restored.group_channel_id = 0;
    factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    assert_eq!(
        &sequence(factory.channel_coinbase_output(99).unwrap())[16..],
        &2_u64.to_be_bytes()
    );
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let standard_job = job(messages[&group].clone());
    assert!(markers(&transaction(&standard_job, &[0; 16])).is_empty());
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    assert!(matches!(
        factory.on_submit_shares_standard(SubmitSharesStandard {
            channel_id: standard_id,
            sequence_number: 1,
            job_id: standard_job.job_id,
            nonce: 0,
            ntime: TIME,
            version: VERSION,
        }),
        Ok(OnNewShare::ShareMeetBitcoinTarget(_))
    ));

    let mut factory = pool().with_channel_coinbase_outputs();
    let first = open(&mut factory);
    let group = factory.new_group_id();
    let mut restored = first.clone();
    restored.channel_id = 99;
    restored.group_channel_id = group;
    factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    let marker = factory.channel_coinbase_output(99).unwrap();
    assert!(
        matches!(factory.add_standard_channel(8, 1_000., false, group),
        Err(Error::IncompatibleChannelGroup(id)) if id == group)
    );
    assert_eq!(factory.extended_channel(99), Some(&restored));
    assert_eq!(factory.channel_coinbase_output(99), Some(marker));
    let next = open(&mut factory);
    assert_eq!(next.channel_id, group + 1);
    assert_eq!(
        &sequence(factory.channel_coinbase_output(next.channel_id).unwrap())[16..],
        &3_u64.to_be_bytes()
    );
}

#[test]
fn group_zero_broadcasts_reject_marked_extended_channels_while_header_only_channels_coexist() {
    let mut factory = pool().with_channel_coinbase_outputs();
    let opened = factory.add_standard_channel(7, 1_000., false, 0).unwrap();
    let standard_id = match &opened[0] {
        MiningOwned::OpenStandardMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected standard-channel assignment"),
    };
    let assignment = OpenExtendedMiningChannelSuccessOwned {
        request_id: 8,
        channel_id: 99,
        target: [255; 32].into(),
        extranonce_prefix: vec![7; 8].try_into().unwrap(),
        extranonce_size: 8,
        group_channel_id: 0,
    };
    assert!(matches!(
        factory.restore_extended_channel(assignment, vec![]),
        Err(Error::IncompatibleChannelGroup(0))
    ));
    assert!(matches!(
        factory.new_extended_channel(9, 1_000., 8),
        Err(Error::IncompatibleChannelGroup(0))
    ));
    assert!(factory.get_extended_channels_ids().is_empty());
    assert!(factory.channel_coinbase_output(99).is_none());
    factory.close_channel(standard_id);
    let extended = open(&mut factory);
    assert_eq!(extended.channel_id, standard_id + 1);
    assert_eq!(
        &sequence(
            factory
                .channel_coinbase_output(extended.channel_id)
                .unwrap()
        )[16..],
        &1_u64.to_be_bytes()
    );
    assert!(matches!(
        factory.add_standard_channel(10, 1_000., false, 0),
        Err(Error::IncompatibleChannelGroup(0))
    ));
    let hom_id = factory.new_standard_id_for_hom();
    let opened = factory
        .add_standard_channel(11, 1_000., true, hom_id)
        .unwrap();
    let hom_nonce = match &opened[0] {
        MiningOwned::OpenStandardMiningChannelSuccess(message) => {
            assert_eq!(message.group_channel_id, 0);
            message.extranonce_prefix.as_bytes().to_vec()
        }
        other => panic!("expected header-only channel assignment, got {other:?}"),
    };
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    assert_eq!(messages.len(), 2);
    let hom_job = match &messages[&hom_id] {
        MiningOwned::NewMiningJob(job) => job,
        other => panic!("expected individually addressed header-only job, got {other:?}"),
    };
    let extended_job = job(messages[&extended.channel_id].clone());
    assert_marker(
        &extended_job,
        factory
            .channel_coinbase_output(extended.channel_id)
            .unwrap(),
    );
    let mut hom_coinbase = transaction(&extended_job, &hom_nonce);
    hom_coinbase.output.retain(|output| {
        !output
            .script_pubkey
            .as_bytes()
            .starts_with(b"\x6a\x1dCHSEQ")
    });
    assert_eq!(
        hom_job.merkle_root.as_bytes(),
        hom_coinbase.compute_txid().to_byte_array()
    );
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    assert!(matches!(
        factory.on_submit_shares_extended(share(extended.channel_id, extended_job.job_id)),
        Ok(OnNewShare::ShareMeetBitcoinTarget(_))
    ));
}

#[test]
fn unmarked_native_mixed_groups_keep_extended_members_when_standard_members_close() {
    let mut factory = pool();
    let first = open(&mut factory);
    let group = factory.new_group_id();
    let opened = factory
        .add_standard_channel(8, 1_000., false, group)
        .unwrap();
    let standard_id = match &opened[0] {
        MiningOwned::OpenStandardMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected standard-channel assignment"),
    };
    let mut restored = first;
    restored.channel_id = 99;
    restored.extranonce_prefix = vec![7; 8].try_into().unwrap();
    restored.group_channel_id = group;
    factory
        .restore_extended_channel(restored.clone(), vec![])
        .unwrap();
    let messages = factory.on_new_template(&mut template(10)).unwrap();
    let standard_job = job(messages[&group].clone());
    let extended_job = job(messages[&99].clone());
    assert!(markers(&transaction(&extended_job, &[0; 16])).is_empty());
    assert_eq!(
        standard_job.coinbase_tx_suffix,
        extended_job.coinbase_tx_suffix
    );
    factory.on_new_prev_hash_from_tp(&prev_hash(10)).unwrap();
    factory.close_group(group);
    assert_eq!(factory.extended_channel(99), Some(&restored));
    assert!(factory
        .on_submit_shares_standard(SubmitSharesStandard {
            channel_id: standard_id,
            sequence_number: 1,
            job_id: standard_job.job_id,
            nonce: 0,
            ntime: TIME,
            version: VERSION,
        })
        .is_err());
    let tx = accepted_coinbase(
        factory
            .on_submit_shares_extended(share(99, extended_job.job_id))
            .unwrap(),
    );
    assert!(markers(&tx).is_empty());
}
