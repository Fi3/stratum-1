use async_std::{
    sync::{Arc, Mutex},
    task,
};
use std::time;
use v1::ClientStatus;

pub mod client;
pub use client::Client;

async fn initialize_client(client: Arc<Mutex<Client>>) {
    loop {
        let mut client_ = client.lock().await;
        match client_.status {
            ClientStatus::Init => client_.send_configure().await,
            ClientStatus::Configured => client_.send_subscribe().await,
            ClientStatus::Subscribed => {
                client_.send_authorize().await;
                break;
            }
        }
        drop(client_);
        task::sleep(time::Duration::from_millis(100)).await;
    }
    task::sleep(time::Duration::from_millis(2000)).await;
    loop {
        let mut client_ = client.lock().await;
        client_.send_submit().await;
        task::sleep(time::Duration::from_millis(2000)).await;
    }
}

fn main() {
    task::block_on(async {
        let client = Client::new(80).await;
        initialize_client(client).await;
    });
}
