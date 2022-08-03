use v1::utils::{HexBytes, HexU32Be};

pub mod downstream;
pub mod downstream_connection;
pub use downstream::Downstream;
pub use downstream_connection::DownstreamConnection;

pub fn new_extranonce() -> HexBytes {
    "08000002".try_into().unwrap()
}

pub fn new_extranonce2_size() -> usize {
    4
}

pub fn new_version_rolling_mask() -> HexU32Be {
    HexU32Be(0xffffffff)
}

pub fn new_version_rolling_min() -> HexU32Be {
    HexU32Be(0x00000000)
}
