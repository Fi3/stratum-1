use super::{EitherFrame, StdFrame};
use async_channel::{Receiver, Sender};

/// Handles the sending and receiving of messages to and from an SV2 Upstream role (most typically
/// a SV2 Pool server).
/// On upstream, we have a sv2connection, so we use the connection from network helpers
/// use network_helpers::Connection;
/// this does the dirty work of reading byte by byte in the socket and puts them in a complete
/// Sv2Messages frame and when the message is ready then sends to our Upstream
/// sender_incoming + receiver_outgoing are in network_helpers::Connection
#[derive(Debug, Clone)]
pub struct UpstreamConnection {
    /// Receives messages from the SV2 Upstream role
    pub receiver: Receiver<EitherFrame>,
    /// Sends messages to the SV2 Upstream role
    pub sender: Sender<EitherFrame>,
    /// Sends to Translator::receiver_upstream
    pub sender_downstream: Sender<EitherFrame>,
    /// Receives from Translator::sender_upstream
    pub receiver_downstream: Receiver<EitherFrame>,
}

impl UpstreamConnection {
    pub async fn send(&mut self, sv2_frame: StdFrame) -> Result<(), ()> {
        let either_frame = sv2_frame.into();
        match self.sender.send(either_frame).await {
            Ok(_) => Ok(()),
            Err(_) => Err(()),
        }
    }
}
