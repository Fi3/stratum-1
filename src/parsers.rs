//! Native borrowed message parsers and owned replies.
//!
//! Each variant contains an upstream message. Encoding delegates to binary_sv2;
//! frame construction uses MessageFrame. No wire-message types are defined here.

use crate::Error;
use binary_sv2::{encodable::EncodableField, GetSize};
use framing_sv2::framing::MessageFrame;

pub trait IsSv2Message {
    fn message_type(&self) -> u8;
    fn channel_bit(&self) -> bool;
}

macro_rules! protocol {
    ($borrowed:ident, $owned:ident, $types:ident, $($variant:ident($message:ty, $owned_message:ty) => ($id:path, $channel:path)),+ $(,)?) => {
        #[derive(Clone, Debug)]
        pub enum $borrowed<'a> { $($variant($message)),+ }
        #[derive(Clone, Debug)]
        pub enum $owned { $($variant($owned_message)),+ }
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum $types { $($variant = $id),+ }
        impl $types {
            pub fn channel_bit(self) -> bool { match self { $(Self::$variant => $channel),+ } }
        }
        impl TryFrom<u8> for $types {
            type Error = Error;
            fn try_from(id: u8) -> Result<Self, Error> {
                match id { $($id => Ok(Self::$variant),)+ _ => Err(Error::UnexpectedMessage(id)) }
            }
        }
        impl<'a> TryFrom<(u8, &'a mut [u8])> for $borrowed<'a> {
            type Error = Error;
            fn try_from((id, payload): (u8, &'a mut [u8])) -> Result<Self, Error> {
                let len = payload.len();
                let message = match id {
                    $($id => Self::$variant(binary_sv2::from_bytes(payload)?),)+
                    _ => return Err(Error::UnexpectedMessage(id)),
                };
                if message.get_size() != len { return Err(Error::BadPayloadSize); }
                Ok(message)
            }
        }
        impl<'a> $borrowed<'a> {
            pub fn into_owned(self) -> $owned {
                match self { $(Self::$variant(m) => $owned::$variant(m.into_owned())),+ }
            }
            pub fn as_owned(&self) -> $owned { self.clone().into_owned() }
        }
        impl IsSv2Message for $borrowed<'_> {
            fn message_type(&self) -> u8 { match self { $(Self::$variant(_) => $id),+ } }
            fn channel_bit(&self) -> bool { match self { $(Self::$variant(_) => $channel),+ } }
        }
        impl IsSv2Message for $owned {
            fn message_type(&self) -> u8 { match self { $(Self::$variant(_) => $id),+ } }
            fn channel_bit(&self) -> bool { match self { $(Self::$variant(_) => $channel),+ } }
        }
        impl GetSize for $borrowed<'_> {
            fn get_size(&self) -> usize { match self { $(Self::$variant(m) => m.get_size()),+ } }
        }
        impl GetSize for $owned {
            fn get_size(&self) -> usize { match self { $(Self::$variant(m) => m.get_size()),+ } }
        }
        impl<'a> From<$borrowed<'a>> for EncodableField<'a> {
            fn from(m: $borrowed<'a>) -> Self { match m { $($borrowed::$variant(m) => m.into()),+ } }
        }
        impl From<$owned> for EncodableField<'static> {
            fn from(m: $owned) -> Self { match m { $($owned::$variant(m) => m.into()),+ } }
        }
        impl<'a> TryFrom<$borrowed<'a>> for MessageFrame<$borrowed<'a>> {
            type Error = Error;
            fn try_from(m: $borrowed<'a>) -> Result<Self, Error> {
                let (id, channel) = (m.message_type(), m.channel_bit());
                Self::from_message(m, id, 0, channel).map_err(|_| Error::BadPayloadSize)
            }
        }
        impl TryFrom<$owned> for MessageFrame<$owned> {
            type Error = Error;
            fn try_from(m: $owned) -> Result<Self, Error> {
                let (id, channel) = (m.message_type(), m.channel_bit());
                Self::from_message(m, id, 0, channel).map_err(|_| Error::BadPayloadSize)
            }
        }
        $(impl<'a> From<$message> for $borrowed<'a> {
            fn from(m: $message) -> Self { Self::$variant(m) }
        }
        impl From<$owned_message> for $owned {
            fn from(m: $owned_message) -> Self { Self::$variant(m) }
        })+
    };
}

protocol!(CommonMessages, CommonMessagesOwned, CommonMessageTypes,
    ChannelEndpointChanged(common_messages_sv2::ChannelEndpointChanged, common_messages_sv2::ChannelEndpointChangedOwned) => (common_messages_sv2::MESSAGE_TYPE_CHANNEL_ENDPOINT_CHANGED, common_messages_sv2::CHANNEL_BIT_CHANNEL_ENDPOINT_CHANGED),
    SetupConnection(common_messages_sv2::SetupConnection<'a>, common_messages_sv2::SetupConnectionOwned) => (common_messages_sv2::MESSAGE_TYPE_SETUP_CONNECTION, common_messages_sv2::CHANNEL_BIT_SETUP_CONNECTION),
    SetupConnectionError(common_messages_sv2::SetupConnectionError<'a>, common_messages_sv2::SetupConnectionErrorOwned) => (common_messages_sv2::MESSAGE_TYPE_SETUP_CONNECTION_ERROR, common_messages_sv2::CHANNEL_BIT_SETUP_CONNECTION_ERROR),
    SetupConnectionSuccess(common_messages_sv2::SetupConnectionSuccess, common_messages_sv2::SetupConnectionSuccessOwned) => (common_messages_sv2::MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, common_messages_sv2::CHANNEL_BIT_SETUP_CONNECTION_SUCCESS),
    Reconnect(common_messages_sv2::Reconnect<'a>, common_messages_sv2::ReconnectOwned) => (common_messages_sv2::MESSAGE_TYPE_RECONNECT, common_messages_sv2::CHANNEL_BIT_RECONNECT),
);

protocol!(Mining, MiningOwned, MiningTypes,
    CloseChannel(mining_sv2::CloseChannel<'a>, mining_sv2::CloseChannelOwned) => (mining_sv2::MESSAGE_TYPE_CLOSE_CHANNEL, mining_sv2::CHANNEL_BIT_CLOSE_CHANNEL),
    NewExtendedMiningJob(mining_sv2::NewExtendedMiningJob<'a>, mining_sv2::NewExtendedMiningJobOwned) => (mining_sv2::MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB, mining_sv2::CHANNEL_BIT_NEW_EXTENDED_MINING_JOB),
    NewMiningJob(mining_sv2::NewMiningJob<'a>, mining_sv2::NewMiningJobOwned) => (mining_sv2::MESSAGE_TYPE_NEW_MINING_JOB, mining_sv2::CHANNEL_BIT_NEW_MINING_JOB),
    OpenExtendedMiningChannel(mining_sv2::OpenExtendedMiningChannel<'a>, mining_sv2::OpenExtendedMiningChannelOwned) => (mining_sv2::MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, mining_sv2::CHANNEL_BIT_OPEN_EXTENDED_MINING_CHANNEL),
    OpenExtendedMiningChannelSuccess(mining_sv2::OpenExtendedMiningChannelSuccess<'a>, mining_sv2::OpenExtendedMiningChannelSuccessOwned) => (mining_sv2::MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS, mining_sv2::CHANNEL_BIT_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS),
    OpenMiningChannelError(mining_sv2::OpenMiningChannelError<'a>, mining_sv2::OpenMiningChannelErrorOwned) => (mining_sv2::MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR, mining_sv2::CHANNEL_BIT_OPEN_MINING_CHANNEL_ERROR),
    OpenStandardMiningChannel(mining_sv2::OpenStandardMiningChannel<'a>, mining_sv2::OpenStandardMiningChannelOwned) => (mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL, mining_sv2::CHANNEL_BIT_OPEN_STANDARD_MINING_CHANNEL),
    OpenStandardMiningChannelSuccess(mining_sv2::OpenStandardMiningChannelSuccess<'a>, mining_sv2::OpenStandardMiningChannelSuccessOwned) => (mining_sv2::MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS, mining_sv2::CHANNEL_BIT_OPEN_STANDARD_MINING_CHANNEL_SUCCESS),
    SetCustomMiningJob(mining_sv2::SetCustomMiningJob<'a>, mining_sv2::SetCustomMiningJobOwned) => (mining_sv2::MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, mining_sv2::CHANNEL_BIT_SET_CUSTOM_MINING_JOB),
    SetCustomMiningJobError(mining_sv2::SetCustomMiningJobError<'a>, mining_sv2::SetCustomMiningJobErrorOwned) => (mining_sv2::MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, mining_sv2::CHANNEL_BIT_SET_CUSTOM_MINING_JOB_ERROR),
    SetCustomMiningJobSuccess(mining_sv2::SetCustomMiningJobSuccess, mining_sv2::SetCustomMiningJobSuccessOwned) => (mining_sv2::MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS, mining_sv2::CHANNEL_BIT_SET_CUSTOM_MINING_JOB_SUCCESS),
    SetExtranoncePrefix(mining_sv2::SetExtranoncePrefix<'a>, mining_sv2::SetExtranoncePrefixOwned) => (mining_sv2::MESSAGE_TYPE_SET_EXTRANONCE_PREFIX, mining_sv2::CHANNEL_BIT_SET_EXTRANONCE_PREFIX),
    SetGroupChannel(mining_sv2::SetGroupChannel<'a>, mining_sv2::SetGroupChannelOwned) => (mining_sv2::MESSAGE_TYPE_SET_GROUP_CHANNEL, mining_sv2::CHANNEL_BIT_SET_GROUP_CHANNEL),
    SetNewPrevHash(mining_sv2::SetNewPrevHash<'a>, mining_sv2::SetNewPrevHashOwned) => (mining_sv2::MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH, mining_sv2::CHANNEL_BIT_MINING_SET_NEW_PREV_HASH),
    SetTarget(mining_sv2::SetTarget<'a>, mining_sv2::SetTargetOwned) => (mining_sv2::MESSAGE_TYPE_SET_TARGET, mining_sv2::CHANNEL_BIT_SET_TARGET),
    SubmitSharesError(mining_sv2::SubmitSharesError<'a>, mining_sv2::SubmitSharesErrorOwned) => (mining_sv2::MESSAGE_TYPE_SUBMIT_SHARES_ERROR, mining_sv2::CHANNEL_BIT_SUBMIT_SHARES_ERROR),
    SubmitSharesExtended(mining_sv2::SubmitSharesExtended<'a>, mining_sv2::SubmitSharesExtendedOwned) => (mining_sv2::MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, mining_sv2::CHANNEL_BIT_SUBMIT_SHARES_EXTENDED),
    SubmitSharesStandard(mining_sv2::SubmitSharesStandard, mining_sv2::SubmitSharesStandardOwned) => (mining_sv2::MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, mining_sv2::CHANNEL_BIT_SUBMIT_SHARES_STANDARD),
    SubmitSharesSuccess(mining_sv2::SubmitSharesSuccess, mining_sv2::SubmitSharesSuccessOwned) => (mining_sv2::MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS, mining_sv2::CHANNEL_BIT_SUBMIT_SHARES_SUCCESS),
    UpdateChannel(mining_sv2::UpdateChannel<'a>, mining_sv2::UpdateChannelOwned) => (mining_sv2::MESSAGE_TYPE_UPDATE_CHANNEL, mining_sv2::CHANNEL_BIT_UPDATE_CHANNEL),
    UpdateChannelError(mining_sv2::UpdateChannelError<'a>, mining_sv2::UpdateChannelErrorOwned) => (mining_sv2::MESSAGE_TYPE_UPDATE_CHANNEL_ERROR, mining_sv2::CHANNEL_BIT_UPDATE_CHANNEL_ERROR),
);

protocol!(JobDeclaration, JobDeclarationOwned, JobDeclarationTypes,
    AllocateMiningJobToken(job_declaration_sv2::AllocateMiningJobToken<'a>, job_declaration_sv2::AllocateMiningJobTokenOwned) => (job_declaration_sv2::MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN, job_declaration_sv2::CHANNEL_BIT_ALLOCATE_MINING_JOB_TOKEN),
    AllocateMiningJobTokenSuccess(job_declaration_sv2::AllocateMiningJobTokenSuccess<'a>, job_declaration_sv2::AllocateMiningJobTokenSuccessOwned) => (job_declaration_sv2::MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS, job_declaration_sv2::CHANNEL_BIT_ALLOCATE_MINING_JOB_TOKEN_SUCCESS),
    DeclareMiningJob(job_declaration_sv2::DeclareMiningJob<'a>, job_declaration_sv2::DeclareMiningJobOwned) => (job_declaration_sv2::MESSAGE_TYPE_DECLARE_MINING_JOB, job_declaration_sv2::CHANNEL_BIT_DECLARE_MINING_JOB),
    DeclareMiningJobError(job_declaration_sv2::DeclareMiningJobError<'a>, job_declaration_sv2::DeclareMiningJobErrorOwned) => (job_declaration_sv2::MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR, job_declaration_sv2::CHANNEL_BIT_DECLARE_MINING_JOB_ERROR),
    DeclareMiningJobSuccess(job_declaration_sv2::DeclareMiningJobSuccess<'a>, job_declaration_sv2::DeclareMiningJobSuccessOwned) => (job_declaration_sv2::MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS, job_declaration_sv2::CHANNEL_BIT_DECLARE_MINING_JOB_SUCCESS),
    ProvideMissingTransactions(job_declaration_sv2::ProvideMissingTransactions<'a>, job_declaration_sv2::ProvideMissingTransactionsOwned) => (job_declaration_sv2::MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS, job_declaration_sv2::CHANNEL_BIT_PROVIDE_MISSING_TRANSACTIONS),
    ProvideMissingTransactionsSuccess(job_declaration_sv2::ProvideMissingTransactionsSuccess<'a>, job_declaration_sv2::ProvideMissingTransactionsSuccessOwned) => (job_declaration_sv2::MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS, job_declaration_sv2::CHANNEL_BIT_PROVIDE_MISSING_TRANSACTIONS_SUCCESS),
    PushSolution(job_declaration_sv2::PushSolution<'a>, job_declaration_sv2::PushSolutionOwned) => (job_declaration_sv2::MESSAGE_TYPE_PUSH_SOLUTION, job_declaration_sv2::CHANNEL_BIT_PUSH_SOLUTION),
);

protocol!(TemplateDistribution, TemplateDistributionOwned, TemplateDistributionTypes,
    CoinbaseOutputConstraints(template_distribution_sv2::CoinbaseOutputConstraints, template_distribution_sv2::CoinbaseOutputConstraintsOwned) => (template_distribution_sv2::MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS, template_distribution_sv2::CHANNEL_BIT_COINBASE_OUTPUT_CONSTRAINTS),
    NewTemplate(template_distribution_sv2::NewTemplate<'a>, template_distribution_sv2::NewTemplateOwned) => (template_distribution_sv2::MESSAGE_TYPE_NEW_TEMPLATE, template_distribution_sv2::CHANNEL_BIT_NEW_TEMPLATE),
    RequestTransactionData(template_distribution_sv2::RequestTransactionData, template_distribution_sv2::RequestTransactionDataOwned) => (template_distribution_sv2::MESSAGE_TYPE_REQUEST_TRANSACTION_DATA, template_distribution_sv2::CHANNEL_BIT_REQUEST_TRANSACTION_DATA),
    RequestTransactionDataError(template_distribution_sv2::RequestTransactionDataError<'a>, template_distribution_sv2::RequestTransactionDataErrorOwned) => (template_distribution_sv2::MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR, template_distribution_sv2::CHANNEL_BIT_REQUEST_TRANSACTION_DATA_ERROR),
    RequestTransactionDataSuccess(template_distribution_sv2::RequestTransactionDataSuccess<'a>, template_distribution_sv2::RequestTransactionDataSuccessOwned) => (template_distribution_sv2::MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS, template_distribution_sv2::CHANNEL_BIT_REQUEST_TRANSACTION_DATA_SUCCESS),
    SetNewPrevHash(template_distribution_sv2::SetNewPrevHash<'a>, template_distribution_sv2::SetNewPrevHashOwned) => (template_distribution_sv2::MESSAGE_TYPE_SET_NEW_PREV_HASH, template_distribution_sv2::CHANNEL_BIT_SET_NEW_PREV_HASH),
    SubmitSolution(template_distribution_sv2::SubmitSolution<'a>, template_distribution_sv2::SubmitSolutionOwned) => (template_distribution_sv2::MESSAGE_TYPE_SUBMIT_SOLUTION, template_distribution_sv2::CHANNEL_BIT_SUBMIT_SOLUTION),
);

macro_rules! container {
    ($borrowed:ident, $owned:ident, $($variant:ident($inner:ident, $inner_owned:ident, $types:ident)),+ $(,)?) => {
        #[derive(Clone, Debug)]
        pub enum $borrowed<'a> { $($variant($inner<'a>)),+ }
        #[derive(Clone, Debug)]
        pub enum $owned { $($variant($inner_owned)),+ }
        impl<'a> TryFrom<(u8, &'a mut [u8])> for $borrowed<'a> {
            type Error = Error;
            fn try_from(v: (u8, &'a mut [u8])) -> Result<Self, Error> {
                $(if $types::try_from(v.0).is_ok() { return Ok(Self::$variant(v.try_into()?)); })+
                Err(Error::UnexpectedMessage(v.0))
            }
        }
        impl<'a> $borrowed<'a> {
            pub fn into_owned(self) -> $owned {
                match self { $(Self::$variant(m) => $owned::$variant(m.into_owned())),+ }
            }
            pub fn as_owned(&self) -> $owned { self.clone().into_owned() }
        }
        impl IsSv2Message for $borrowed<'_> {
            fn message_type(&self) -> u8 { match self { $(Self::$variant(m) => m.message_type()),+ } }
            fn channel_bit(&self) -> bool { match self { $(Self::$variant(m) => m.channel_bit()),+ } }
        }
        impl IsSv2Message for $owned {
            fn message_type(&self) -> u8 { match self { $(Self::$variant(m) => m.message_type()),+ } }
            fn channel_bit(&self) -> bool { match self { $(Self::$variant(m) => m.channel_bit()),+ } }
        }
        impl GetSize for $borrowed<'_> {
            fn get_size(&self) -> usize { match self { $(Self::$variant(m) => m.get_size()),+ } }
        }
        impl GetSize for $owned {
            fn get_size(&self) -> usize { match self { $(Self::$variant(m) => m.get_size()),+ } }
        }
        impl<'a> From<$borrowed<'a>> for EncodableField<'a> {
            fn from(m: $borrowed<'a>) -> Self { match m { $($borrowed::$variant(m) => m.into()),+ } }
        }
        impl From<$owned> for EncodableField<'static> {
            fn from(m: $owned) -> Self { match m { $($owned::$variant(m) => m.into()),+ } }
        }
        impl<'a> TryFrom<$borrowed<'a>> for MessageFrame<$borrowed<'a>> {
            type Error = Error;
            fn try_from(m: $borrowed<'a>) -> Result<Self, Error> {
                let (id, channel) = (m.message_type(), m.channel_bit());
                Self::from_message(m, id, 0, channel).map_err(|_| Error::BadPayloadSize)
            }
        }
        impl TryFrom<$owned> for MessageFrame<$owned> {
            type Error = Error;
            fn try_from(m: $owned) -> Result<Self, Error> {
                let (id, channel) = (m.message_type(), m.channel_bit());
                Self::from_message(m, id, 0, channel).map_err(|_| Error::BadPayloadSize)
            }
        }
    };
}
container!(
    PoolMessages,
    PoolMessagesOwned,
    Common(CommonMessages, CommonMessagesOwned, CommonMessageTypes),
    Mining(Mining, MiningOwned, MiningTypes),
    JobDeclaration(JobDeclaration, JobDeclarationOwned, JobDeclarationTypes),
    TemplateDistribution(
        TemplateDistribution,
        TemplateDistributionOwned,
        TemplateDistributionTypes
    ),
);
container!(
    MiningDeviceMessages,
    MiningDeviceMessagesOwned,
    Common(CommonMessages, CommonMessagesOwned, CommonMessageTypes),
    Mining(Mining, MiningOwned, MiningTypes),
);
pub type AnyMessage<'a> = PoolMessages<'a>;
pub type AnyMessageOwned = PoolMessagesOwned;

impl<'a> From<MiningDeviceMessages<'a>> for PoolMessages<'a> {
    fn from(m: MiningDeviceMessages<'a>) -> Self {
        match m {
            MiningDeviceMessages::Common(m) => Self::Common(m),
            MiningDeviceMessages::Mining(m) => Self::Mining(m),
        }
    }
}
impl<'a> TryFrom<PoolMessages<'a>> for MiningDeviceMessages<'a> {
    type Error = Error;
    fn try_from(m: PoolMessages<'a>) -> Result<Self, Error> {
        match m {
            PoolMessages::Common(m) => Ok(Self::Common(m)),
            PoolMessages::Mining(m) => Ok(Self::Mining(m)),
            _ => Err(Error::UnexpectedPoolMessage),
        }
    }
}
#[cfg(test)]
mod test {
    use crate::{
        mining_sv2::NewMiningJobOwned,
        parsers::{MiningOwned, PoolMessagesOwned},
    };
    use binary_sv2::{Sv2OptionOwned, U256Owned};
    use framing_sv2::framing::{EncodableFrame, MessageFrame};
    use std::convert::{TryFrom, TryInto};

    pub type Message = PoolMessagesOwned;
    pub type StdFrame = MessageFrame<Message>;

    #[test]
    fn new_mining_job_serialization() {
        const CORRECTLY_SERIALIZED_MSG: &[u8] = &[
            0, 128, 21, 49, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 1, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
            19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40,
            41, 42, 43, 44, 45, 46, 47, 48,
        ];
        let mining_message =
            PoolMessagesOwned::Mining(MiningOwned::NewMiningJob(NewMiningJobOwned {
                channel_id: u32::from_le_bytes([1, 2, 3, 4]),
                job_id: u32::from_le_bytes([5, 6, 7, 8]),
                min_ntime: Sv2OptionOwned::new(Some(u32::from_le_bytes([9, 10, 11, 12]))),
                version: u32::from_le_bytes([13, 14, 15, 16]),
                merkle_root: U256Owned::try_from((17_u8..(17 + 32)).collect::<Vec<u8>>()).unwrap(),
            }));
        message_serialization_check(mining_message, CORRECTLY_SERIALIZED_MSG);
    }

    fn message_serialization_check(message: PoolMessagesOwned, expected_result: &[u8]) {
        let frame = StdFrame::try_from(message).unwrap();
        let encoded_frame_length = frame.encoded_length();

        let mut buffer = [0; 0xffff];
        frame
            .encode_into(&mut buffer[..encoded_frame_length])
            .unwrap();
        check_length_consistency(&buffer[..encoded_frame_length]);
        check_length_consistency(expected_result);
        assert_eq!(
            is_channel_msg(&buffer),
            is_channel_msg(expected_result),
            "Unexpected channel_message flag",
        );
        assert_eq!(
            extract_extension_type(&buffer),
            extract_extension_type(expected_result),
            "Unexpected extension type",
        );
        assert_eq!(
            extract_message_type(&buffer),
            extract_message_type(expected_result),
            "Unexpected message type",
        );
        assert_eq!(
            extract_payload_length(&buffer),
            extract_payload_length(expected_result),
            "Unexpected message length",
        );
        assert_eq!(
            encoded_frame_length as u32,
            expected_result.len() as u32,
            "Method encoded_length() returned different length than the actual message length",
        );
        assert_eq!(
            extract_payload(&buffer[..encoded_frame_length]),
            extract_payload(expected_result),
            "Unexpected payload",
        )
    }

    fn is_channel_msg(serialized_frame: &[u8]) -> bool {
        let array_repre = serialized_frame[..2].try_into().unwrap();
        let decoded_extension_type = u16::from_le_bytes(array_repre);
        (decoded_extension_type & (1 << 15)) != 0
    }
    fn extract_extension_type(serialized_frame: &[u8]) -> u16 {
        let array_repre = serialized_frame[..2].try_into().unwrap();
        let decoded_extension_type = u16::from_le_bytes(array_repre);
        decoded_extension_type & (u16::MAX >> 1)
    }
    fn extract_message_type(serialized_frame: &[u8]) -> u8 {
        serialized_frame[2]
    }
    fn extract_payload_length(serialized_frame: &[u8]) -> u32 {
        let mut array_repre = [0; 4];
        array_repre[..3].copy_from_slice(&serialized_frame[3..6]);
        u32::from_le_bytes(array_repre)
    }
    fn extract_payload(serialized_frame: &[u8]) -> &[u8] {
        assert!(serialized_frame.len() > 6);
        &serialized_frame[6..]
    }

    fn check_length_consistency(serialized_frame: &[u8]) {
        let message_length = extract_payload_length(serialized_frame) as usize;
        let payload_length = extract_payload(serialized_frame).len();
        assert_eq!(
            message_length, payload_length,
            "Header declared length [{} bytes] differs from the actual payload length [{} bytes]",
            message_length, payload_length,
        );
    }
}
