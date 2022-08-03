pub mod client;
pub mod job;
pub mod miner;
pub use client::Client;

#[async_std::main]
async fn main() {
    Client::new(80).await
}
