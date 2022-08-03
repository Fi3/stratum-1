mod downstream_sv1;
mod proxy;
mod upstream_sv2;
use std::net::{IpAddr,SocketAddr};
use std::str::FromStr;

pub const LISTEN_ADDR: &str = "127.0.0.1:34255";

#[async_std::main]
async fn main() {
    let _ = proxy::Translator::new().await;
}
