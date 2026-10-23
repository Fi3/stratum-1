//! Independent little-endian wire fixtures for every supported native message.
use binary_sv2::{GetSize, Seq0255Owned, Seq064KOwned, Sv2OptionOwned};
use framing_sv2::framing::{EncodableFrame, MessageFrame};
use roles_logic_sv2::parsers::*;

fn hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
macro_rules! fixture {
    ($name:ident, $borrowed:ident, $owned:ident, $variant:ident, $native:expr, $id:expr, $channel:expr, $wire:literal) => {
        #[test]
        fn $name() {
            let expected = hex($wire);
            let message = $owned::$variant($native);
            assert_eq!(message.message_type(), $id);
            assert_eq!(message.channel_bit(), $channel);
            assert_eq!(message.get_size(), expected.len());
            assert_eq!(binary_sv2::to_bytes(message.clone()).unwrap(), expected);
            let frame: MessageFrame<_> = message.try_into().unwrap();
            let mut encoded = vec![0; frame.encoded_length()];
            frame.encode_into(&mut encoded).unwrap();
            assert_eq!(&encoded[..3], &[0, if $channel { 128 } else { 0 }, $id]);
            assert_eq!(
                &encoded[3..6],
                &((expected.len() as u32).to_le_bytes()[..3])
            );
            assert_eq!(&encoded[6..], &expected);
            let mut receive = expected.clone();
            let owned = $borrowed::try_from(($id, receive.as_mut_slice()))
                .unwrap()
                .into_owned();
            receive.fill(0xff);
            drop(receive);
            assert_eq!(binary_sv2::to_bytes(owned).unwrap(), expected);
            for len in 0..expected.len() {
                let mut truncated = expected[..len].to_vec();
                assert!(
                    $borrowed::try_from(($id, truncated.as_mut_slice())).is_err(),
                    "accepted truncated payload at {len}"
                );
            }
            let mut trailing = expected;
            trailing.push(0);
            assert!($borrowed::try_from(($id, trailing.as_mut_slice())).is_err());
        }
    };
}

fixture!(
    common_messages_channel_endpoint_changed,
    CommonMessages,
    CommonMessagesOwned,
    ChannelEndpointChanged,
    common_messages_sv2::ChannelEndpointChangedOwned {
        channel_id: 67305985
    },
    0x3,
    true,
    "01020304"
);

fixture!(
    common_messages_setup_connection,
    CommonMessages,
    CommonMessagesOwned,
    SetupConnection,
    common_messages_sv2::SetupConnectionOwned {
        protocol: common_messages_sv2::Protocol::MiningProtocol,
        min_version: 514,
        max_version: 515,
        flags: 67305988,
        endpoint_host: vec![5, 6].try_into().unwrap(),
        endpoint_port: 518,
        vendor: vec![7, 8].try_into().unwrap(),
        hardware_version: vec![8, 9].try_into().unwrap(),
        firmware: vec![9, 10].try_into().unwrap(),
        device_id: vec![10, 11].try_into().unwrap()
    },
    0x0,
    false,
    "000202030204020304020506060202070802080902090a020a0b"
);

fixture!(
    common_messages_setup_connection_error,
    CommonMessages,
    CommonMessagesOwned,
    SetupConnectionError,
    common_messages_sv2::SetupConnectionErrorOwned {
        flags: 67305985,
        error_code: vec![2, 3].try_into().unwrap()
    },
    0x2,
    false,
    "01020304020203"
);

fixture!(
    common_messages_setup_connection_success,
    CommonMessages,
    CommonMessagesOwned,
    SetupConnectionSuccess,
    common_messages_sv2::SetupConnectionSuccessOwned {
        used_version: 513,
        flags: 67305986
    },
    0x1,
    false,
    "010202020304"
);

fixture!(
    common_messages_reconnect,
    CommonMessages,
    CommonMessagesOwned,
    Reconnect,
    common_messages_sv2::ReconnectOwned {
        new_host: vec![1, 2].try_into().unwrap(),
        new_port: 514
    },
    0x04,
    false,
    "0201020202"
);

fixture!(
    mining_close_channel,
    Mining,
    MiningOwned,
    CloseChannel,
    mining_sv2::CloseChannelOwned {
        channel_id: 67305985,
        reason_code: vec![2, 3].try_into().unwrap()
    },
    0x18,
    true,
    "01020304020203"
);

fixture!(mining_new_extended_mining_job, Mining, MiningOwned, NewExtendedMiningJob,
    mining_sv2::NewExtendedMiningJobOwned {channel_id: 67305985, job_id: 67305986, min_ntime: Sv2OptionOwned::new(Some(3)), version: 67305988, version_rolling_allowed: true, merkle_path: Seq0255Owned::new(vec![[6;32].into()]).unwrap(), coinbase_tx_prefix: vec![7, 8].try_into().unwrap(), coinbase_tx_suffix: vec![8, 9].try_into().unwrap()}, 0x1f, true, "0102030402020304010300000004020304010106060606060606060606060606060606060606060606060606060606060606060200070802000809");

fixture!(mining_new_mining_job, Mining, MiningOwned, NewMiningJob,
    mining_sv2::NewMiningJobOwned {channel_id: 67305985, job_id: 67305986, min_ntime: Sv2OptionOwned::new(Some(3)), version: 67305988, merkle_root: [5;32].into()}, 0x15, true, "01020304020203040103000000040203040505050505050505050505050505050505050505050505050505050505050505");

fixture!(
    mining_open_extended_mining_channel,
    Mining,
    MiningOwned,
    OpenExtendedMiningChannel,
    mining_sv2::OpenExtendedMiningChannelOwned {
        request_id: 67305985,
        user_identity: vec![2, 3].try_into().unwrap(),
        nominal_hash_rate: 1.5,
        max_target: [4; 32].into(),
        min_extranonce_size: 517
    },
    0x13,
    false,
    "010203040202030000c03f04040404040404040404040404040404040404040404040404040404040404040502"
);

fixture!(mining_open_extended_mining_channel_success, Mining, MiningOwned, OpenExtendedMiningChannelSuccess,
    mining_sv2::OpenExtendedMiningChannelSuccessOwned {request_id: 67305985, channel_id: 67305986, target: [3;32].into(), extranonce_size: 516, extranonce_prefix: vec![5, 6].try_into().unwrap(), group_channel_id: 67305990}, 0x14, false, "01020304020203040303030303030303030303030303030303030303030303030303030303030303040202050606020304");

fixture!(
    mining_open_mining_channel_error,
    Mining,
    MiningOwned,
    OpenMiningChannelError,
    mining_sv2::OpenMiningChannelErrorOwned {
        request_id: 67305985,
        error_code: vec![2, 3].try_into().unwrap()
    },
    0x12,
    false,
    "01020304020203"
);

fixture!(
    mining_open_standard_mining_channel,
    Mining,
    MiningOwned,
    OpenStandardMiningChannel,
    mining_sv2::OpenStandardMiningChannelOwned {
        request_id: 67305985,
        user_identity: vec![2, 3].try_into().unwrap(),
        nominal_hash_rate: 1.5,
        max_target: [4; 32].into()
    },
    0x10,
    false,
    "010203040202030000c03f0404040404040404040404040404040404040404040404040404040404040404"
);

fixture!(mining_open_standard_mining_channel_success, Mining, MiningOwned, OpenStandardMiningChannelSuccess,
    mining_sv2::OpenStandardMiningChannelSuccessOwned {request_id: 67305985, channel_id: 67305986, target: [3;32].into(), extranonce_prefix: vec![4, 5].try_into().unwrap(), group_channel_id: 67305989}, 0x11, false, "0102030402020304030303030303030303030303030303030303030303030303030303030303030302040505020304");

fixture!(mining_set_custom_mining_job, Mining, MiningOwned, SetCustomMiningJob,
    mining_sv2::SetCustomMiningJobOwned {channel_id: 67305985, request_id: 67305986, token: vec![3, 4].try_into().unwrap(), version: 67305988, prev_hash: [5;32].into(), min_ntime: 67305990, nbits: 67305991, coinbase_tx_version: 67305992, coinbase_prefix: vec![9, 10].try_into().unwrap(), coinbase_tx_input_n_sequence: 67305994, coinbase_tx_outputs: vec![11, 12].try_into().unwrap(), coinbase_tx_locktime: 67305996, merkle_path: Seq0255Owned::new(vec![[13;32].into()]).unwrap()}, 0x22, true, "010203040202030402030404020304050505050505050505050505050505050505050505050505050505050505050506020304070203040802030402090a0a02030402000b0c0c020304010d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d");

fixture!(
    mining_set_custom_mining_job_error,
    Mining,
    MiningOwned,
    SetCustomMiningJobError,
    mining_sv2::SetCustomMiningJobErrorOwned {
        channel_id: 67305985,
        request_id: 67305986,
        error_code: vec![3, 4].try_into().unwrap()
    },
    0x24,
    true,
    "0102030402020304020304"
);

fixture!(
    mining_set_custom_mining_job_success,
    Mining,
    MiningOwned,
    SetCustomMiningJobSuccess,
    mining_sv2::SetCustomMiningJobSuccessOwned {
        channel_id: 67305985,
        request_id: 67305986,
        job_id: 67305987
    },
    0x23,
    true,
    "010203040202030403020304"
);

fixture!(
    mining_set_extranonce_prefix,
    Mining,
    MiningOwned,
    SetExtranoncePrefix,
    mining_sv2::SetExtranoncePrefixOwned {
        channel_id: 67305985,
        extranonce_prefix: vec![2, 3].try_into().unwrap()
    },
    0x19,
    true,
    "01020304020203"
);

fixture!(
    mining_set_group_channel,
    Mining,
    MiningOwned,
    SetGroupChannel,
    mining_sv2::SetGroupChannelOwned {
        group_channel_id: 67305985,
        channel_ids: Seq064KOwned::new(vec![67305986]).unwrap()
    },
    0x25,
    false,
    "01020304010002020304"
);

fixture!(mining_set_new_prev_hash, Mining, MiningOwned, SetNewPrevHash,
    mining_sv2::SetNewPrevHashOwned {channel_id: 67305985, job_id: 67305986, prev_hash: [3;32].into(), min_ntime: 67305988, nbits: 67305989}, 0x20, true, "010203040202030403030303030303030303030303030303030303030303030303030303030303030402030405020304");

fixture!(
    mining_set_target,
    Mining,
    MiningOwned,
    SetTarget,
    mining_sv2::SetTargetOwned {
        channel_id: 67305985,
        maximum_target: [2; 32].into()
    },
    0x21,
    true,
    "010203040202020202020202020202020202020202020202020202020202020202020202"
);

fixture!(
    mining_submit_shares_error,
    Mining,
    MiningOwned,
    SubmitSharesError,
    mining_sv2::SubmitSharesErrorOwned {
        channel_id: 67305985,
        sequence_number: 67305986,
        error_code: vec![3, 4].try_into().unwrap()
    },
    0x1d,
    true,
    "0102030402020304020304"
);

fixture!(
    mining_submit_shares_extended,
    Mining,
    MiningOwned,
    SubmitSharesExtended,
    mining_sv2::SubmitSharesExtendedOwned {
        channel_id: 67305985,
        sequence_number: 67305986,
        job_id: 67305987,
        nonce: 67305988,
        ntime: 67305989,
        version: 67305990,
        extranonce: vec![7, 8].try_into().unwrap()
    },
    0x1b,
    true,
    "010203040202030403020304040203040502030406020304020708"
);

fixture!(
    mining_submit_shares_standard,
    Mining,
    MiningOwned,
    SubmitSharesStandard,
    mining_sv2::SubmitSharesStandardOwned {
        channel_id: 67305985,
        sequence_number: 67305986,
        job_id: 67305987,
        nonce: 67305988,
        ntime: 67305989,
        version: 67305990
    },
    0x1a,
    true,
    "010203040202030403020304040203040502030406020304"
);

fixture!(
    mining_submit_shares_success,
    Mining,
    MiningOwned,
    SubmitSharesSuccess,
    mining_sv2::SubmitSharesSuccessOwned {
        channel_id: 67305985,
        last_sequence_number: 67305986,
        new_submits_accepted_count: 67305987,
        new_shares_sum: 578437695752307204
    },
    0x1c,
    true,
    "0102030402020304030203040402030405060708"
);

fixture!(
    mining_update_channel,
    Mining,
    MiningOwned,
    UpdateChannel,
    mining_sv2::UpdateChannelOwned {
        channel_id: 67305985,
        nominal_hash_rate: 1.5,
        maximum_target: [3; 32].into()
    },
    0x16,
    true,
    "010203040000c03f0303030303030303030303030303030303030303030303030303030303030303"
);

fixture!(
    mining_update_channel_error,
    Mining,
    MiningOwned,
    UpdateChannelError,
    mining_sv2::UpdateChannelErrorOwned {
        channel_id: 67305985,
        error_code: vec![2, 3].try_into().unwrap()
    },
    0x17,
    true,
    "01020304020203"
);

fixture!(
    job_declaration_allocate_mining_job_token,
    JobDeclaration,
    JobDeclarationOwned,
    AllocateMiningJobToken,
    job_declaration_sv2::AllocateMiningJobTokenOwned {
        user_identifier: vec![1, 2].try_into().unwrap(),
        request_id: 67305986
    },
    0x50,
    false,
    "02010202020304"
);

fixture!(
    job_declaration_allocate_mining_job_token_success,
    JobDeclaration,
    JobDeclarationOwned,
    AllocateMiningJobTokenSuccess,
    job_declaration_sv2::AllocateMiningJobTokenSuccessOwned {
        request_id: 67305985,
        mining_job_token: vec![2, 3].try_into().unwrap(),
        coinbase_outputs: vec![3, 4].try_into().unwrap()
    },
    0x51,
    false,
    "0102030402020302000304"
);

fixture!(job_declaration_declare_mining_job, JobDeclaration, JobDeclarationOwned, DeclareMiningJob,
    job_declaration_sv2::DeclareMiningJobOwned {request_id: 67305985, mining_job_token: vec![2, 3].try_into().unwrap(), version: 67305987, coinbase_tx_prefix: vec![4, 5].try_into().unwrap(), coinbase_tx_suffix: vec![5, 6].try_into().unwrap(), wtxid_list: Seq064KOwned::new(vec![[6;32].into()]).unwrap(), excess_data: vec![7, 8].try_into().unwrap()}, 0x57, false, "010203040202030302030402000405020005060100060606060606060606060606060606060606060606060606060606060606060602000708");

fixture!(
    job_declaration_declare_mining_job_error,
    JobDeclaration,
    JobDeclarationOwned,
    DeclareMiningJobError,
    job_declaration_sv2::DeclareMiningJobErrorOwned {
        request_id: 67305985,
        error_code: vec![2, 3].try_into().unwrap(),
        error_details: vec![3, 4].try_into().unwrap()
    },
    0x59,
    false,
    "0102030402020302000304"
);

fixture!(
    job_declaration_declare_mining_job_success,
    JobDeclaration,
    JobDeclarationOwned,
    DeclareMiningJobSuccess,
    job_declaration_sv2::DeclareMiningJobSuccessOwned {
        request_id: 67305985,
        new_mining_job_token: vec![2, 3].try_into().unwrap()
    },
    0x58,
    false,
    "01020304020203"
);

fixture!(
    job_declaration_provide_missing_transactions,
    JobDeclaration,
    JobDeclarationOwned,
    ProvideMissingTransactions,
    job_declaration_sv2::ProvideMissingTransactionsOwned {
        request_id: 67305985,
        unknown_tx_position_list: Seq064KOwned::new(vec![514]).unwrap()
    },
    0x55,
    false,
    "0102030401000202"
);

fixture!(
    job_declaration_provide_missing_transactions_success,
    JobDeclaration,
    JobDeclarationOwned,
    ProvideMissingTransactionsSuccess,
    job_declaration_sv2::ProvideMissingTransactionsSuccessOwned {
        request_id: 67305985,
        transaction_list: Seq064KOwned::new(vec![vec![2, 3].try_into().unwrap()]).unwrap()
    },
    0x56,
    false,
    "0102030401000200000203"
);

fixture!(job_declaration_push_solution, JobDeclaration, JobDeclarationOwned, PushSolution,
    job_declaration_sv2::PushSolutionOwned {extranonce: vec![1, 2].try_into().unwrap(), prev_hash: [2;32].into(), nonce: 67305987, ntime: 67305988, nbits: 67305989, version: 67305990}, 0x60, false, "020102020202020202020202020202020202020202020202020202020202020202020203020304040203040502030406020304");

fixture!(
    template_distribution_coinbase_output_constraints,
    TemplateDistribution,
    TemplateDistributionOwned,
    CoinbaseOutputConstraints,
    template_distribution_sv2::CoinbaseOutputConstraintsOwned {
        coinbase_output_max_additional_size: 67305985,
        coinbase_output_max_additional_sigops: 514
    },
    0x70,
    false,
    "010203040202"
);

fixture!(template_distribution_new_template, TemplateDistribution, TemplateDistributionOwned, NewTemplate,
    template_distribution_sv2::NewTemplateOwned {template_id: 578437695752307201, future_template: true, version: 67305987, coinbase_tx_version: 67305988, coinbase_prefix: vec![5, 6].try_into().unwrap(), coinbase_tx_input_sequence: 67305990, coinbase_tx_value_remaining: 578437695752307207, coinbase_tx_outputs_count: 67305992, coinbase_tx_outputs: vec![9, 10].try_into().unwrap(), coinbase_tx_locktime: 67305994, merkle_path: Seq0255Owned::new(vec![[11;32].into()]).unwrap()}, 0x71, false, "0102030405060708010302030404020304020506060203040702030405060708080203040200090a0a020304010b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");

fixture!(
    template_distribution_request_transaction_data,
    TemplateDistribution,
    TemplateDistributionOwned,
    RequestTransactionData,
    template_distribution_sv2::RequestTransactionDataOwned {
        template_id: 578437695752307201
    },
    0x73,
    false,
    "0102030405060708"
);

fixture!(
    template_distribution_request_transaction_data_error,
    TemplateDistribution,
    TemplateDistributionOwned,
    RequestTransactionDataError,
    template_distribution_sv2::RequestTransactionDataErrorOwned {
        template_id: 578437695752307201,
        error_code: vec![2, 3].try_into().unwrap()
    },
    0x75,
    false,
    "0102030405060708020203"
);

fixture!(
    template_distribution_request_transaction_data_success,
    TemplateDistribution,
    TemplateDistributionOwned,
    RequestTransactionDataSuccess,
    template_distribution_sv2::RequestTransactionDataSuccessOwned {
        template_id: 578437695752307201,
        excess_data: vec![2, 3].try_into().unwrap(),
        transaction_list: Seq064KOwned::new(vec![vec![3, 4].try_into().unwrap()]).unwrap()
    },
    0x74,
    false,
    "01020304050607080200020301000200000304"
);

fixture!(template_distribution_set_new_prev_hash, TemplateDistribution, TemplateDistributionOwned, SetNewPrevHash,
    template_distribution_sv2::SetNewPrevHashOwned {template_id: 578437695752307201, prev_hash: [2;32].into(), header_timestamp: 67305987, n_bits: 67305988, target: [5;32].into()}, 0x72, false, "0102030405060708020202020202020202020202020202020202020202020202020202020202020203020304040203040505050505050505050505050505050505050505050505050505050505050505");

fixture!(
    template_distribution_submit_solution,
    TemplateDistribution,
    TemplateDistributionOwned,
    SubmitSolution,
    template_distribution_sv2::SubmitSolutionOwned {
        template_id: 578437695752307201,
        version: 67305986,
        header_timestamp: 67305987,
        header_nonce: 67305988,
        coinbase_tx: vec![5, 6].try_into().unwrap()
    },
    0x76,
    false,
    "010203040506070802020304030203040402030402000506"
);
