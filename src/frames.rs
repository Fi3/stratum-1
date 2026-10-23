//! Checked edits to native serialized frames for allocation-free request-ID forwarding.

use crate::Error;
use framing_sv2::framing::SerializedFrame;
use mining_sv2::{
    MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR, MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
    MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
};

/// A remapping computed while handling a decoded native channel request or reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestIdUpdate {
    pub message_type: u8,
    pub original_id: u32,
    pub new_id: u32,
}

impl RequestIdUpdate {
    /// Updates exactly the first four payload bytes after checking the native schema and ID.
    pub fn apply_payload(self, message_type: u8, payload: &mut [u8]) -> Result<(), Error> {
        if message_type != self.message_type {
            return Err(Error::UnexpectedMessage(message_type));
        }
        let expected_len = match message_type {
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL => {
                41 + usize::from(*payload.get(4).ok_or(Error::BadPayloadSize)?)
            }
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS => {
                let prefix_len = usize::from(*payload.get(40).ok_or(Error::BadPayloadSize)?);
                if prefix_len > 32 {
                    return Err(Error::BadPayloadSize);
                }
                45 + prefix_len
            }
            MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR => {
                5 + usize::from(*payload.get(4).ok_or(Error::BadPayloadSize)?)
            }
            _ => return Err(Error::UnexpectedMessage(message_type)),
        };
        if payload.len() != expected_len {
            return Err(Error::BadPayloadSize);
        }
        let id = payload.get_mut(..4).ok_or(Error::BadPayloadSize)?;
        if id != self.original_id.to_le_bytes() {
            return Err(Error::RequestIdNotMapped(self.original_id));
        }
        id.copy_from_slice(&self.new_id.to_le_bytes());
        Ok(())
    }

    /// Edits a received native frame in place without allocating or encoding its message.
    pub fn apply<B: AsRef<[u8]> + AsMut<[u8]>>(
        self,
        frame: &mut SerializedFrame<B>,
    ) -> Result<(), Error> {
        let header = frame.header();
        if header.ext_type_without_channel_msg() != 0
            || crate::parsers::MiningTypes::try_from(header.msg_type())?.channel_bit()
                != header.channel_msg()
        {
            return Err(Error::UnexpectedMessage(header.msg_type()));
        }
        self.apply_payload(header.msg_type(), frame.payload())
    }
}
