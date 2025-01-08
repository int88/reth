//! Example of how to use the network as a standalone component
//! 例子关于如何使用网络作为一个独立的组件
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p example-network
//! ```

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use futures::StreamExt;
use reth_network::{
    config::rng_secret_key, EthNetworkPrimitives, NetworkConfig, NetworkEventListenerProvider,
    NetworkManager,
};
use reth_provider::test_utils::NoopProvider;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // This block provider implementation is used for testing purposes.
    let client = NoopProvider::default();

    // The key that's used for encrypting sessions and to identify our node.
    let local_key = rng_secret_key();

    // Configure the network
    // 配置network
    let config = NetworkConfig::builder(local_key)
        .mainnet_boot_nodes()
        .listener_port(30302)
        .discovery_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 30302)))
        .disable_discovery()
        .build(client);

    // create the network instance
    // 创建network instance
    let network = NetworkManager::<EthNetworkPrimitives>::new(config).await?;

    // get a handle to the network to interact with it
    // 获取一个handle来与network交互
    let handle = network.handle().clone();

    // spawn the network
    // 生成network
    tokio::task::spawn(network);

    // interact with the network
    // 和network进行交互
    let mut events = handle.event_listener();
    while let Some(event) = events.next().await {
        // println!("Received event: {:?}", event);
    }

    Ok(())
}
