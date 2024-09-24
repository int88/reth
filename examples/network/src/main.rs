//! Example of how to use the network as a standalone component
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p network
//! ```

use futures::StreamExt;
use reth_network::{
    config::rng_secret_key, NetworkConfig, NetworkEventListenerProvider, NetworkManager,
};
use reth_provider::test_utils::NoopProvider;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // This block provider implementation is used for testing purposes.
    let client = NoopProvider::default();

    // The key that's used for encrypting sessions and to identify our node.
    // key用于加密sessions并且标识我们的node
    let local_key = rng_secret_key();

    // Configure the network
    // 配置network
    let config = NetworkConfig::builder(local_key).mainnet_boot_nodes().build(client);

    // create the network instance
    // 创建network instance
    let network = NetworkManager::new(config).await?;

    // get a handle to the network to interact with it
    // 获取一个到network的handle来和它进行交互
    let handle = network.handle().clone();

    // spawn the network
    // 生成network
    tokio::task::spawn(network);

    // interact with the network
    // 和Netowrk交互
    let mut events = handle.event_listener();
    while let Some(event) = events.next().await {
        println!("Received event: {:?}", event);
    }

    Ok(())
}
