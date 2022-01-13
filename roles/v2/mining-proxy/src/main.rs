//! Configurable Sv2 it support extended and group channel
//! Upstream means another proxy or a pool
//! Downstream means another proxy or a mining device
//!
//! ## From messages_sv2
//! UpstreamMining is the (sub)protocol that a proxy must implement in order to
//! understant Downstream mining messages.
//!
//! DownstreamMining is the (sub)protocol that a proxy must implement in order to
//! understand Upstream mining messages
//!
//! Same thing for DownstreamCommon and UpstreamCommon
//!
//! ## Internal
//! DownstreamMiningNode rapresent the Downstream as defined above as the proxy need to understand
//! some message (TODO which one?) from downstream it DownstreamMiningNode it implement
//! UpstreamMining. DownstreamMiningNode implement UpstreamCommon in order to setup a connection
//! with the downstream node.
//!
//! UpstreamMiningNode rapresent the upstream as defined above as the proxy only need to relay
//! downstream messages coming from downstream UpstreamMiningNode do not (for now) implement
//! DownstreamMining. UpstreamMiningNode implement DownstreamCommon (TODO) in order to setup a
//! connection with with the upstream node.
//!
//! A Downstream that signal the capacity to handle group channels can open more than one channel.
//! A Downstream that signal the incapacity to handle group channels can open only one channel.
//!
mod error;
mod lib;
use error::Result;
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use lib::upstream_mining::{UpstreamMiningNode, UpstreamMiningNodes};
use serde::Deserialize;

// TODO make them configurable via flags or config file
pub const MAX_SUPPORTED_VERSION: u16 = 2;
pub const MIN_SUPPORTED_VERSION: u16 = 2;
pub use messages_sv2::Mutex;
use std::sync::Arc;

#[derive(Debug)]
pub struct Id {
    state: u32,
}

impl Id {
    pub fn new() -> Self {
        Self { state: 0 }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        self.state += 1;
        self.state
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct UpstreamValues {
    address: String,
    port: u16,
    pub_key: [u8; 32],
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Config {
    upstreams: Vec<UpstreamValues>,
    listen_address: String,
    listen_mining_port: u16,
}

/// Reads and returns file found at `path`
fn read_file(path: String) -> Result<String> {
    let contents = std::fs::read_to_string(&path)?;
    Ok(contents)
}

/// Reads proxy-config.toml file containing mock proxy config data, which is the miner's
/// configuration file.
fn read_config(path: String) -> Result<Config> {
    let config_contents = read_file(path)?;
    let config = toml::from_str(&config_contents)?;

    Ok(config)
}

/// 1. The proxy scan all the upstreams points provided by the proxy-config.toml file and maps them
/// 2. Downstream open a connection with proxy
/// 3. Downstream send SetupConnection
/// 4. A mining_channel::Upstream is created
/// 5. Upstream_mining::UpstreamMiningNodes is used to pair this downstream with the most suitable
///    upstream
/// 6. Mining_channel::Upstream create a new downstream_mining::DownstreamMiningNode embedding
///    itself in it
/// 7. Normal operation between the paired downstream_mining::DownstreamMiningNode and
///    upstream_mining::UpstreamMiningNode begin
#[async_std::main]
async fn main() {
    // Scan all the upstreams and map them
    let path = String::from("proxy-config.toml");
    let config = read_config(path).unwrap();
    let upstreams = config.upstreams;
    let upstream_mining_nodes = upstreams
        .iter()
        .map(|upstream| {
            let socket =
                SocketAddr::new(IpAddr::from_str(&upstream.address).unwrap(), upstream.port);
            Arc::new(Mutex::new(UpstreamMiningNode::new(
                socket,
                upstream.pub_key,
            )))
        })
        .collect();
    let mut upstream_mining_nodes = UpstreamMiningNodes {
        nodes: upstream_mining_nodes,
    };
    upstream_mining_nodes.scan().await;

    // Wait for downstream connection
    let socket = SocketAddr::new(
        IpAddr::from_str(&config.listen_address).unwrap(),
        config.listen_mining_port,
    );
    crate::lib::downstream_mining::listen_for_downstream_mining(socket, upstream_mining_nodes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_success() -> Result<()> {
        let actual = read_file(String::from("proxy-config.toml"))?;
        let expect = r#"upstreams = [{ address = "127.0.0.1", port = 34254, pub_key = [215, 11, 47, 78, 34, 232, 25, 192, 195, 168, 170, 209, 95, 181, 40, 114, 154, 226, 176, 190, 90, 169, 238, 89, 191, 183, 97, 63, 194, 119, 11, 31]}]
listen_address = "127.0.0.1"
listen_mining_port = 34255
"#;
        assert_eq!(actual, expect);

        Ok(())
    }

    #[test]
    fn read_file_fail() {
        assert!(read_file(String::from("bad.file")).is_err());
    }

    #[test]
    fn read_proxy_config_toml() -> Result<()> {
        let actual = read_config(String::from("proxy-config.toml"))?;
        let expect_str = r#"upstreams = [{ address = "127.0.0.1", port = 34254, pub_key = [215, 11, 47, 78, 34, 232, 25, 192, 195, 168, 170, 209, 95, 181, 40, 114, 154, 226, 176, 190, 90, 169, 238, 89, 191, 183, 97, 63, 194, 119, 11, 31]}]
listen_address = "127.0.0.1"
listen_mining_port = 34255
"#;
        let expect = toml::from_str(&expect_str)?;
        assert_eq!(actual, expect);

        Ok(())
    }
}
