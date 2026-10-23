use binary_sv2::{Seq0255Owned, Seq064KOwned, Sv2OptionOwned};
use bitcoin::{consensus, hashes::Hash, Amount, Block, ScriptBuf, Transaction, TxOut};
use mining_sv2::*;
use roles_logic_sv2::{
    channel_logic::channel_factory::{
        ExtendedChannelKind, PoolChannelFactory, ProxyExtendedChannelFactory,
    },
    handlers::template_distribution::{ParseClientTemplateDistributionMessages, SendTo},
    job_creator::{self, JobsCreators},
    job_declaration::{negotiated_mode, transaction_wtxids, JobDeclarationMode},
    mining::{ExtendedExtranonce, Target},
    utils::{BlockCreator, GroupId, Mutex},
    version::*,
    Error,
};
use std::sync::Arc;

fn outputs() -> Vec<TxOut> {
    vec![
        TxOut {
            value: Amount::from_sat(123456),
            script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
        },
        TxOut {
            value: Amount::from_sat(987654),
            script_pubkey: ScriptBuf::from_bytes(vec![0x6a, 0x01, 0x42]),
        },
    ]
}
fn custom() -> SetCustomMiningJobOwned {
    SetCustomMiningJobOwned {
        channel_id: 100,
        request_id: 7,
        token: vec![42].try_into().unwrap(),
        version: 0x20000000,
        prev_hash: [9; 32].into(),
        min_ntime: 1700000000,
        nbits: 0x207fffff,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03].try_into().unwrap(),
        coinbase_tx_input_n_sequence: u32::MAX,
        coinbase_tx_outputs: consensus::serialize(&outputs()).try_into().unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: Seq0255Owned::new(vec![]).unwrap(),
    }
}
fn assignment() -> OpenExtendedMiningChannelSuccessOwned {
    OpenExtendedMiningChannelSuccessOwned {
        request_id: 7,
        channel_id: 100,
        target: [255; 32].into(),
        extranonce_prefix: vec![7; 8].try_into().unwrap(),
        extranonce_size: 8,
        group_channel_id: 42,
    }
}
fn pool() -> PoolChannelFactory {
    PoolChannelFactory::new(
        Arc::new(Mutex::new(GroupId::new())),
        ExtendedExtranonce::new(0..0, 0..8, 8..16),
        JobsCreators::new(16),
        1.,
        ExtendedChannelKind::Pool,
        outputs(),
        vec![],
    )
    .unwrap()
}
fn coinbase(job: &NewExtendedMiningJobOwned, nonce: &[u8]) -> Transaction {
    consensus::deserialize(
        &[
            job.coinbase_tx_prefix.as_bytes(),
            nonce,
            job.coinbase_tx_suffix.as_bytes(),
        ]
        .concat(),
    )
    .unwrap()
}
#[test]
fn native_custom_outputs_reconstruct_amounts_order_and_hashes() {
    let message = custom();
    let mut factory = pool();
    let replies = factory
        .restore_extended_channel(assignment(), vec![])
        .unwrap();
    match &replies[0] {
        roles_logic_sv2::parsers::MiningOwned::OpenExtendedMiningChannelSuccess(m) => {
            assert_eq!(m.group_channel_id, 42)
        }
        _ => unreachable!(),
    }
    assert_eq!(factory.full_extranonce_len(100).unwrap(), 16);
    assert!(factory.full_extranonce_len(999).is_err());
    let success = factory
        .on_new_set_custom_mining_job(message.clone())
        .unwrap();
    assert_eq!(success.request_id, 7);
    let job = job_creator::extended_job_from_custom_job(
        &message,
        0,
        factory.full_extranonce_len(100).unwrap() as u8,
    )
    .unwrap();
    let nonce = [7; 16];
    let tx = coinbase(&job, &nonce);
    assert!(tx.is_coinbase());
    assert_eq!(tx.output, outputs());
    assert_eq!(tx.input[0].script_sig.len(), 20);
    assert_eq!(&tx.input[0].script_sig.as_bytes()[4..], &nonce);
    let stripped = job_creator::extended_job_to_non_segwit(job.clone(), 16).unwrap();
    let hashing_tx = coinbase(&stripped, &nonce);
    assert_eq!(hashing_tx.compute_txid(), tx.compute_txid());
    assert_eq!(
        hashing_tx.compute_wtxid().to_byte_array(),
        tx.compute_txid().to_byte_array()
    );
    assert_ne!(
        tx.compute_wtxid().to_byte_array(),
        tx.compute_txid().to_byte_array()
    );
    assert_eq!(
        job_creator::decode_outputs(
            job_creator::coinbase_outputs_from_job(&job, 16)
                .unwrap()
                .as_bytes()
        )
        .unwrap(),
        outputs()
    );
    let original = message.coinbase_tx_outputs.as_bytes();
    let malformed = [
        vec![],
        vec![0],
        original[..original.len() - 1].to_vec(),
        [original, &[0]].concat(),
        consensus::serialize(&outputs()[0]),
    ];
    for bytes in malformed {
        let mut bad = message.clone();
        bad.coinbase_tx_outputs = bytes.try_into().unwrap();
        assert!(factory.on_new_set_custom_mining_job(bad).is_err());
    }
}
#[test]
fn template_outputs_use_their_separate_count_and_remaining_reward() {
    let server_outputs = outputs();
    let raw = server_outputs
        .iter()
        .flat_map(consensus::serialize)
        .collect::<Vec<_>>();
    assert_eq!(
        job_creator::decode_template_outputs(&raw, 2).unwrap(),
        server_outputs
    );
    for count in [0, 1, 3, u32::MAX] {
        assert!(job_creator::decode_template_outputs(&raw, count).is_err());
    }
    let mut template = template_distribution_sv2::NewTemplateOwned {
        template_id: u64::MAX,
        future_template: true,
        version: 0x20000000,
        coinbase_tx_version: 2,
        coinbase_prefix: vec![3, 0x40, 0x0d, 0x03].try_into().unwrap(),
        coinbase_tx_input_sequence: u32::MAX,
        coinbase_tx_value_remaining: 555,
        coinbase_tx_outputs_count: 2,
        coinbase_tx_outputs: raw.try_into().unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: Seq0255Owned::new(vec![]).unwrap(),
    };
    let mut creator = JobsCreators::new(16);
    let job = creator
        .on_new_template(&mut template, true, vec![outputs()[0].clone()], 0)
        .unwrap();
    assert_eq!(creator.get_template_id_from_job(job.job_id), Some(u64::MAX));
    let tx = coinbase(&job, &[0; 16]);
    assert_eq!(tx.output[0].value, Amount::from_sat(555));
    assert_eq!(&tx.output[1..], server_outputs);
}
#[test]
fn native_proxy_accepts_assigned_group_work_and_membership_changes() {
    let mut factory = ProxyExtendedChannelFactory::new(
        Arc::new(Mutex::new(GroupId::new())),
        ExtendedExtranonce::new(0..0, 0..8, 8..16),
        None,
        1.,
        ExtendedChannelKind::Proxy {
            upstream_target: Target::new(u128::MAX, u128::MAX),
        },
        None,
        assignment(),
    );
    assert_eq!(factory.upstream_channel().group_channel_id, 42);
    let mut job = job_creator::extended_job_from_custom_job(&custom(), 0, 16).unwrap();
    job.channel_id = 43;
    assert!(factory.on_new_extended_mining_job(job.clone()).is_err());
    job.channel_id = 42;
    job.min_ntime = Sv2OptionOwned::new(None);
    factory.on_new_extended_mining_job(job.clone()).unwrap();
    factory.set_group_channel(&SetGroupChannel {
        group_channel_id: 99,
        channel_ids: binary_sv2::Seq064K::new(vec![100]).unwrap(),
    });
    assert_eq!(factory.upstream_channel().group_channel_id, 99);
    assert!(factory.on_new_extended_mining_job(job.clone()).is_err());
    job.channel_id = 99;
    job.job_id += 1;
    factory.on_new_extended_mining_job(job).unwrap();
}
#[test]
fn job_declaration_modes_use_full_template_requirements() {
    assert_eq!(
        negotiated_mode(0, 0),
        Some(JobDeclarationMode::CoinbaseOnly)
    );
    assert_eq!(
        negotiated_mode(1, 0),
        Some(JobDeclarationMode::CoinbaseOnly)
    );
    assert_eq!(negotiated_mode(0, 1), None);
    assert_eq!(
        negotiated_mode(1, 1),
        Some(JobDeclarationMode::FullTemplate)
    );
}
fn from_hex(s: &str) -> Vec<u8> {
    s.trim()
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn segwit_declaration_uses_wtxids_while_block_merkle_uses_txids() {
    let tx: Transaction = consensus::deserialize(&from_hex(include_str!(
        "../fixtures/segwit-transaction.hex"
    )))
    .unwrap();
    assert_eq!(
        tx.compute_txid().to_string(),
        "f5864806e3565c34d1b41e716f72609d00b55ea5eac5b924c9719a842ef42206"
    );
    assert_eq!(
        tx.compute_wtxid().to_string(),
        "80b7d8a82d5d5bf92905b06f2014dd699e03837ca172e3a59d51426ebbe3e7f5"
    );
    let ids = transaction_wtxids(std::slice::from_ref(&tx)).unwrap();
    assert_eq!(
        ids.iter().next().unwrap().as_bytes(),
        tx.compute_wtxid().to_byte_array()
    );
    let job = job_creator::extended_job_from_custom_job(&custom(), 0, 16).unwrap();
    let declaration = job_declaration_sv2::DeclareMiningJobOwned {
        request_id: 7,
        mining_job_token: vec![42].try_into().unwrap(),
        version: job.version,
        coinbase_tx_prefix: job.coinbase_tx_prefix.clone(),
        coinbase_tx_suffix: job.coinbase_tx_suffix.clone(),
        wtxid_list: ids,
        excess_data: vec![].try_into().unwrap(),
    };
    let solution = job_declaration_sv2::PushSolutionOwned {
        extranonce: vec![7; 16].try_into().unwrap(),
        prev_hash: [9; 32].into(),
        nonce: 1,
        ntime: 1700000000,
        nbits: 0x207fffff,
        version: job.version,
    };
    let block = Block::try_from(BlockCreator::new(
        declaration.clone(),
        vec![tx.clone()],
        solution.clone(),
    ))
    .unwrap();
    let expected = bitcoin::merkle_tree::calculate_root(
        [
            block.txdata[0].compute_txid().to_raw_hash(),
            tx.compute_txid().to_raw_hash(),
        ]
        .into_iter(),
    )
    .unwrap();
    assert_eq!(block.header.merkle_root.to_raw_hash(), expected);
    let witness_root = bitcoin::merkle_tree::calculate_root(
        [
            block.txdata[0].compute_txid().to_raw_hash(),
            tx.compute_wtxid().to_raw_hash(),
        ]
        .into_iter(),
    )
    .unwrap();
    assert_ne!(expected, witness_root);
    let mut bad = declaration;
    bad.wtxid_list = Seq064KOwned::new(vec![tx.compute_txid().to_byte_array().into()]).unwrap();
    assert!(Block::try_from(BlockCreator::new(bad, vec![tx], solution)).is_err());
}
#[test]
fn bip323_respects_job_rolling_and_non_rollable_bits() {
    let version = 0x20000000;
    assert!(validate_version(version, version, false).is_ok());
    for bit in 0..32 {
        let changed = version ^ (1 << bit);
        assert_eq!(
            validate_version(version, changed, true).is_ok(),
            (5..=28).contains(&bit)
        );
        assert_eq!(
            validate_version(version, changed, false),
            Err(ERROR_CODE_VERSION_ROLLING_NOT_ALLOWED)
        );
    }
}
#[derive(Default)]
struct Constraints {
    budgets: Option<(u32, u16)>,
}
impl ParseClientTemplateDistributionMessages for Constraints {
    fn handle_coinbase_output_constraints(
        &mut self,
        m: template_distribution_sv2::CoinbaseOutputConstraints,
    ) -> Result<SendTo, Error> {
        self.budgets = Some((
            m.coinbase_output_max_additional_size,
            m.coinbase_output_max_additional_sigops,
        ));
        Ok(SendTo::None(None))
    }
    fn handle_request_tx_data(
        &mut self,
        _: template_distribution_sv2::RequestTransactionData,
    ) -> Result<SendTo, Error> {
        unreachable!()
    }
    fn handle_request_submit_solution(
        &mut self,
        _: template_distribution_sv2::SubmitSolution,
    ) -> Result<SendTo, Error> {
        unreachable!()
    }
}
#[test]
fn consumer_accepts_native_constraints_with_both_budgets() {
    let consumer = Arc::new(Mutex::new(Constraints::default()));
    let native = template_distribution_sv2::CoinbaseOutputConstraints {
        coinbase_output_max_additional_size: 1000,
        coinbase_output_max_additional_sigops: 17,
    };
    let mut wire = binary_sv2::to_bytes(native).unwrap();
    Constraints::handle_message_template_distribution(
        consumer.clone(),
        template_distribution_sv2::MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS,
        &mut wire,
    )
    .unwrap();
    assert_eq!(consumer.safe_lock(|c| c.budgets).unwrap(), Some((1000, 17)));
}

#[test]
fn standard_group_work_is_scoped_to_members_and_follows_reassignment() {
    use roles_logic_sv2::channel_logic::proxy_group_channel::GroupChannels;
    let mut groups = GroupChannels::new();
    for (id, group) in [(100, 42), (101, 43)] {
        groups
            .on_channel_success_for_hom_downtream(&OpenStandardMiningChannelSuccess {
                request_id: id,
                channel_id: id,
                target: (&[255; 32]).into(),
                extranonce_prefix: (&[7; 16][..]).try_into().unwrap(),
                group_channel_id: group,
            })
            .unwrap();
    }
    let mut job = job_creator::extended_job_from_custom_job(&custom(), 0, 16).unwrap();
    job.channel_id = 42;
    let mut bytes = binary_sv2::to_bytes(job).unwrap();
    let native: NewExtendedMiningJob = binary_sv2::from_bytes(&mut bytes).unwrap();
    groups.on_new_extended_mining_job(&native);
    assert!(groups.last_received_job_to_standard_job(100, 42).is_ok());
    assert!(groups.last_received_job_to_standard_job(101, 43).is_err());
    groups
        .set_group_channel(&SetGroupChannel {
            group_channel_id: 42,
            channel_ids: binary_sv2::Seq064K::new(vec![101]).unwrap(),
        })
        .unwrap();
    assert_eq!(
        groups
            .last_received_job_to_standard_job(101, 42)
            .unwrap()
            .channel_id,
        101
    );
    assert!(groups.last_received_job_to_standard_job(101, 43).is_err());
}

#[test]
fn share_validation_uses_each_native_jobs_version_rolling_permission() {
    use roles_logic_sv2::{channel_logic::channel_factory::OnNewShare, parsers::MiningOwned};
    let mut factory = ProxyExtendedChannelFactory::new(
        Arc::new(Mutex::new(GroupId::new())),
        ExtendedExtranonce::new(0..0, 0..8, 8..16),
        None,
        1.,
        ExtendedChannelKind::Proxy {
            upstream_target: Target::new(u128::MAX, u128::MAX),
        },
        None,
        assignment(),
    );
    let replies = factory.new_extended_channel(1, 1000., 8).unwrap();
    let channel_id = match &replies[0] {
        MiningOwned::OpenExtendedMiningChannelSuccess(m) => m.channel_id,
        _ => panic!("expected channel"),
    };
    for (id, rolling, bit, expected) in [
        (1, false, 5, Some(ERROR_CODE_VERSION_ROLLING_NOT_ALLOWED)),
        (2, true, 5, None),
        (
            3,
            true,
            1,
            Some(ERROR_CODE_SUBMIT_SHARES_INVALID_NON_ROLLABLE_VERSION_BIT),
        ),
    ] {
        let mut job = job_creator::extended_job_from_custom_job(&custom(), 0, 16).unwrap();
        job.channel_id = 42;
        job.job_id = id;
        job.min_ntime = Sv2OptionOwned::new(None);
        job.version_rolling_allowed = rolling;
        let version = job.version;
        factory.on_new_extended_mining_job(job).unwrap();
        factory
            .on_new_prev_hash(SetNewPrevHashOwned {
                channel_id: 42,
                job_id: id,
                prev_hash: [9; 32].into(),
                min_ntime: 1700000000,
                nbits: 0x207fffff,
            })
            .unwrap();
        let outcome = factory
            .on_submit_shares_extended(SubmitSharesExtendedOwned {
                channel_id,
                sequence_number: id,
                job_id: id,
                nonce: 0,
                ntime: 1700000000,
                version: version ^ (1 << bit),
                extranonce: vec![0; 8].try_into().unwrap(),
            })
            .unwrap();
        match (expected, outcome) {
            (Some(code), OnNewShare::SendErrorDownstream(error)) => {
                assert_eq!(error.error_code.as_bytes(), code.as_bytes())
            }
            (None, OnNewShare::SendErrorDownstream(error)) => {
                panic!("unexpected share rejection: {error:?}")
            }
            (None, _) => (),
            (Some(code), other) => panic!("expected {code}, got {other:?}"),
        }
    }
}

#[test]
fn jd_restoration_and_custom_shares_use_native_channel_nonce_size() {
    use roles_logic_sv2::channel_logic::channel_factory::OnNewShare;
    let mut factory = pool();
    assert_eq!(factory.get_extranonce_len(), 16);
    let mut channel = assignment();
    channel.extranonce_prefix = vec![7; 6].try_into().unwrap();
    channel.extranonce_size = 3;
    factory
        .replicate_upstream_extended_channel_only_jd(channel)
        .unwrap();
    assert_eq!(factory.extended_channel(100).unwrap().group_channel_id, 42);
    assert_eq!(factory.full_extranonce_len(100).unwrap(), 9);
    let message = custom();
    factory
        .on_new_set_custom_mining_job(message.clone())
        .unwrap();
    factory
        .on_new_prev_hash_from_tp(&template_distribution_sv2::SetNewPrevHashOwned {
            template_id: 7,
            prev_hash: [9; 32].into(),
            header_timestamp: 1700000000,
            n_bits: 0x207fffff,
            target: [255; 32].into(),
        })
        .unwrap();
    let share = SubmitSharesExtendedOwned {
        channel_id: 100,
        sequence_number: 1,
        job_id: 1,
        nonce: 0,
        ntime: 1700000000,
        version: message.version,
        extranonce: vec![8; 3].try_into().unwrap(),
    };
    let mut bad = share.clone();
    bad.extranonce = vec![8; 4].try_into().unwrap();
    assert!(factory.on_submit_shares_extended(bad).is_err());
    let bytes = match factory.on_submit_shares_extended(share).unwrap() {
        OnNewShare::ShareMeetBitcoinTarget((_, _, bytes, _)) => bytes,
        other => panic!("expected valid custom coinbase, got {other:?}"),
    };
    let tx: Transaction = consensus::deserialize(&bytes).unwrap();
    assert_eq!(tx.input[0].script_sig.len(), 13);
    assert_eq!(tx.output, outputs());
    let expected = coinbase(
        &job_creator::extended_job_from_custom_job(&message, 0, 9).unwrap(),
        &[vec![7; 6], vec![8; 3]].concat(),
    );
    assert_eq!(tx.compute_txid(), expected.compute_txid());
}

#[test]
fn native_restoration_preserves_group_and_advertised_additional_data_prefix() {
    use roles_logic_sv2::parsers::MiningOwned;
    let data = vec![0x44, 0x4d, 0x4e];
    let create = || {
        PoolChannelFactory::new(
            Arc::new(Mutex::new(GroupId::new())),
            ExtendedExtranonce::new(0..0, 0..8, 8..16),
            JobsCreators::new(16),
            1.,
            ExtendedChannelKind::Pool,
            outputs(),
            data.clone(),
        )
        .unwrap()
    };
    let mut original = create();
    let mut native = match original
        .new_extended_channel(7, 1000., 8)
        .unwrap()
        .remove(0)
    {
        MiningOwned::OpenExtendedMiningChannelSuccess(m) => m,
        _ => panic!("expected native assignment"),
    };
    native.group_channel_id = 42;
    let mut restored = create();
    let mut bad = native.clone();
    bad.extranonce_prefix = vec![0; 11].try_into().unwrap();
    assert!(restored
        .restore_extended_channel(bad, data.clone())
        .is_err());
    assert!(restored.extended_channel(native.channel_id).is_none());
    let reply = restored
        .restore_extended_channel(native.clone(), data)
        .unwrap()
        .remove(0);
    match reply {
        MiningOwned::OpenExtendedMiningChannelSuccess(m) => assert_eq!(m, native),
        _ => panic!("expected native assignment"),
    }
    assert_eq!(restored.full_extranonce_len(native.channel_id).unwrap(), 16);
    assert_eq!(
        restored
            .extended_channel(native.channel_id)
            .unwrap()
            .group_channel_id,
        42
    );
}

#[test]
fn restored_channel_ids_are_reserved_before_extended_and_standard_allocation() {
    use roles_logic_sv2::parsers::MiningOwned;
    let mut factory = pool();
    let restored = [1, 3].map(|id| {
        let mut channel = assignment();
        channel.channel_id = id;
        channel.extranonce_prefix = vec![id as u8; 8].try_into().unwrap();
        channel.target = [id as u8; 32].into();
        factory
            .restore_extended_channel(channel.clone(), vec![])
            .unwrap();
        channel
    });
    for expected_id in [2, 4] {
        let reply = factory
            .new_extended_channel(80, 1000., 8)
            .unwrap()
            .remove(0);
        match reply {
            MiningOwned::OpenExtendedMiningChannelSuccess(channel) => {
                assert_eq!(channel.channel_id, expected_id)
            }
            other => panic!("expected open channel, got {other:?}"),
        }
    }
    for channel in restored {
        assert_eq!(
            factory.extended_channel(channel.channel_id).unwrap(),
            &channel
        );
    }
    assert_eq!(factory.new_standard_id_for_hom(), 5);
}

#[test]
fn restoring_a_large_id_does_not_exhaust_lower_channel_ids() {
    use roles_logic_sv2::parsers::MiningOwned;
    let mut factory = pool();
    let mut channel = assignment();
    channel.channel_id = u32::MAX;
    factory
        .restore_extended_channel(channel.clone(), vec![])
        .unwrap();
    let opened = factory.new_extended_channel(1, 1000., 8).unwrap().remove(0);
    match opened {
        MiningOwned::OpenExtendedMiningChannelSuccess(opened) => assert_eq!(opened.channel_id, 1),
        other => panic!("expected open channel, got {other:?}"),
    }
    assert_eq!(factory.extended_channel(u32::MAX).unwrap(), &channel);
}

fn custom_share(channel_id: u32, job_id: u32, version: u32) -> SubmitSharesExtendedOwned {
    SubmitSharesExtendedOwned {
        channel_id,
        sequence_number: job_id,
        job_id,
        nonce: 0,
        ntime: 1700000000,
        version,
        extranonce: vec![8; 8].try_into().unwrap(),
    }
}

fn set_easy_block_target(factory: &mut PoolChannelFactory) {
    factory
        .on_new_prev_hash_from_tp(&template_distribution_sv2::SetNewPrevHashOwned {
            template_id: 7,
            prev_hash: [9; 32].into(),
            header_timestamp: 1700000000,
            n_bits: 0x207fffff,
            target: [255; 32].into(),
        })
        .unwrap();
}

#[test]
fn custom_shares_reject_unacknowledged_and_other_channels_job_ids() {
    let mut factory = pool();
    factory
        .restore_extended_channel(assignment(), vec![])
        .unwrap();
    set_easy_block_target(&mut factory);
    let first = factory.on_new_set_custom_mining_job(custom()).unwrap();
    assert_eq!(first.job_id, 1);
    assert!(matches!(
        factory.on_submit_shares_extended(custom_share(100, 1001, custom().version)),
        Err(Error::ShareDoNotMatchAnyJob)
    ));
    let mut second_channel = assignment();
    second_channel.channel_id = 200;
    second_channel.extranonce_prefix = vec![9; 8].try_into().unwrap();
    factory
        .restore_extended_channel(second_channel, vec![])
        .unwrap();
    let mut other = custom();
    other.channel_id = 200;
    let second = factory.on_new_set_custom_mining_job(other).unwrap();
    assert_ne!(first.job_id, second.job_id);
    for (channel_id, wrong_job_id) in [(100, second.job_id), (200, first.job_id)] {
        assert!(matches!(
            factory.on_submit_shares_extended(custom_share(
                channel_id,
                wrong_job_id,
                custom().version
            )),
            Err(Error::ShareDoNotMatchAnyJob)
        ));
    }
}

#[test]
fn later_custom_jobs_preserve_earlier_jobs_coinbase_and_version_context() {
    use roles_logic_sv2::channel_logic::channel_factory::OnNewShare;
    let mut factory = pool();
    factory
        .restore_extended_channel(assignment(), vec![])
        .unwrap();
    set_easy_block_target(&mut factory);
    let first = custom();
    let first_reply = factory.on_new_set_custom_mining_job(first.clone()).unwrap();
    let mut second = custom();
    second.request_id += 1;
    second.version ^= 2; // A non-rollable bit distinguishes the validation contexts.
    let mut second_outputs = outputs();
    second_outputs[0].value = Amount::from_sat(77777);
    second.coinbase_tx_outputs = consensus::serialize(&second_outputs).try_into().unwrap();
    let second_reply = factory
        .on_new_set_custom_mining_job(second.clone())
        .unwrap();
    assert_ne!(first_reply.job_id, second_reply.job_id);
    for (job_id, message) in [(second_reply.job_id, &second), (first_reply.job_id, &first)] {
        let result = factory
            .on_submit_shares_extended(custom_share(100, job_id, message.version))
            .unwrap();
        let bytes = match result {
            OnNewShare::ShareMeetBitcoinTarget((_, _, bytes, _)) => bytes,
            other => panic!("expected accepted custom job {job_id}, got {other:?}"),
        };
        let actual: Transaction = consensus::deserialize(&bytes).unwrap();
        let expected = coinbase(
            &job_creator::extended_job_from_custom_job(message, 0, 16).unwrap(),
            &[vec![7; 8], vec![8; 8]].concat(),
        );
        assert_eq!(actual.output, expected.output);
        assert_eq!(actual.compute_txid(), expected.compute_txid());
    }
}

#[test]
fn closing_a_channel_discards_all_its_custom_job_contexts() {
    let mut factory = pool();
    factory
        .restore_extended_channel(assignment(), vec![])
        .unwrap();
    let old = factory.on_new_set_custom_mining_job(custom()).unwrap();
    factory.close_channel(100);
    assert!(matches!(
        factory.on_submit_shares_extended(custom_share(100, old.job_id, custom().version)),
        Err(Error::ShareDoNotMatchAnyJob)
    ));
    factory
        .restore_extended_channel(assignment(), vec![])
        .unwrap();
    let new = factory.on_new_set_custom_mining_job(custom()).unwrap();
    assert_ne!(old.job_id, new.job_id);
    assert!(matches!(
        factory.on_submit_shares_extended(custom_share(100, old.job_id, custom().version)),
        Err(Error::ShareDoNotMatchAnyJob)
    ));
}
