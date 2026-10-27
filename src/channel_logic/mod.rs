//! # Channel Logic
//!
//! A module for managing channels on applications.
//!
//! Divided in two submodules:
//! - [`channel_factory`]
//! - [`proxy_group_channel`]

pub mod channel_coinbase_output;
pub mod channel_factory;
pub mod proxy_group_channel;

use mining_sv2::{NewExtendedMiningJobOwned, NewMiningJobOwned};
use std::convert::TryInto;

/// Convert extended to standard job by calculating the merkle root
pub fn extended_to_standard_job(
    extended: &NewExtendedMiningJobOwned,
    extranonce: &[u8],
    channel_id: u32,
    job_id: Option<u32>,
    additional_coinbase_script_data: Option<&[u8]>,
) -> Option<NewMiningJobOwned> {
    let merkle_root = crate::utils::merkle_root_from_path(
        extended.coinbase_tx_prefix.as_bytes(),
        extended.coinbase_tx_suffix.as_bytes(),
        extranonce,
        extended.merkle_path.as_slice(),
        additional_coinbase_script_data.unwrap_or(&[]),
    );

    Some(NewMiningJobOwned {
        channel_id,
        job_id: job_id.unwrap_or(extended.job_id),
        min_ntime: extended.min_ntime.clone(),
        version: extended.version,
        merkle_root: merkle_root?.try_into().ok()?,
    })
}
