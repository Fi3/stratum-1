//! Job-declaration mode information and native transaction identifiers.

use binary_sv2::{Seq064KOwned, U256Owned};
use bitcoin::{hashes::Hash, Transaction};
use common_messages_sv2::{Protocol, SetupConnection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobDeclarationMode {
    CoinbaseOnly,
    FullTemplate,
}

/// Interpret the actual setup flags: client bit 0 allows full-template mode;
/// server configuration bit 0 requires it. This is not a SetupConnectionSuccess
/// flag interpretation. Unsupported requirements yield no negotiated mode.
pub fn negotiated_mode(client_flags: u32, server_requirements: u32) -> Option<JobDeclarationMode> {
    if !SetupConnection::check_flags(
        Protocol::JobDeclarationProtocol,
        server_requirements,
        client_flags,
    ) {
        return None;
    }
    Some(if server_requirements & 1 != 0 {
        JobDeclarationMode::FullTemplate
    } else {
        JobDeclarationMode::CoinbaseOnly
    })
}

/// Compute the declaration's wtxids from full transactions, preserving order.
/// Block-header Merkle roots continue to use txids.
pub fn transaction_wtxids(
    transactions: &[Transaction],
) -> Result<Seq064KOwned<U256Owned>, binary_sv2::Error> {
    Seq064KOwned::new(
        transactions
            .iter()
            .map(|tx| tx.compute_wtxid().to_byte_array().into())
            .collect(),
    )
}
