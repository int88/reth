//! reth P2P networking.
//!
//! Ethereum's networking protocol is specified in [devp2p](https://github.com/ethereum/devp2p).
//!
//! In order for a node to join the ethereum p2p network it needs to know what nodes are already
//! part of that network. This includes public identities (public key) and addresses (where to reach
//! them).
//! 为了让一个node能够加入eth p2p network，它需要知道那个node是network的一部分，这包含public
//! id（public key）以及地址
//!
//! ## Bird's Eye View
//!
//! See also diagram in [`NetworkManager`]
//!
//! The `Network` is made up of several, separate tasks:
//!
//!    - `Transactions Task`: is a spawned
//!      [`TransactionsManager`](crate::transactions::TransactionsManager) future that:
//!    - `Transactions Task`被生成：
//!
//!        * Responds to incoming transaction related requests
//!        * 回复到来的tx相关的请求
//!        * Requests missing transactions from the `Network`
//!        * 请求来自`Network`的missing txs
//!        * Broadcasts new transactions received from the
//!          [`TransactionPool`](reth_transaction_pool::TransactionPool) over the `Network`
//!        * 广播从 [`TransactionPool`](reth_transaction_pool::TransactionPool)接收到的txs，
//!          到`Network`
//!
//!    - `ETH request Task`: is a spawned
//!      [`EthRequestHandler`](crate::eth_requests::EthRequestHandler) future that:
//!
//!        * Responds to incoming ETH related requests: `Headers`, `Bodies`
//!        * 回复到来的ETH相关的请求：`Headers`以及`Bodies`
//!
//!    - `Discovery Task`: is a spawned [`Discv4`](reth_discv4::Discv4) future that handles peer
//!      discovery and emits new peers to the `Network`
//!    - `Discovery Task`：用于处理peer discovery以及发出新的peers到`Network`
//!
//!    - [`NetworkManager`] task advances the state of the `Network`, which includes:
//!    - [`NetworkManager`]推进`Network`的状态的变更
//!
//!        * Initiating new _outgoing_ connections to discovered peers
//!        * 初始化新的_outgoing_ connections来发现peers
//!        * Handling _incoming_ TCP connections from peers
//!        * 处理来自peers的_incoming_ TCP连接
//!        * Peer management
//!        * Peer管理
//!        * Route requests:
//!        * 路由请求：
//!             - from remote peers to corresponding tasks
//!             - 从remote peers到对应的tasks
//!             - from local to remote peers
//!             - 从local到remote peers
//!
//! ## Usage
//!
//! ### Configure and launch a standalone network
//!
//! The [`NetworkConfig`] is used to configure the network.
//! It requires an instance of [`BlockReader`](reth_provider::BlockReader).
//!
//! ```
//! # async fn launch() {
//! use reth_network::config::rng_secret_key;
//! use reth_network::{NetworkConfig, NetworkManager};
//! use reth_provider::test_utils::NoopProvider;
//! use reth_primitives::mainnet_nodes;
//!
//! // This block provider implementation is used for testing purposes.
//! let client = NoopProvider::default();
//!
//! // The key that's used for encrypting sessions and to identify our node.
//! let local_key = rng_secret_key();
//!
//! let config = NetworkConfig::builder(local_key).boot_nodes(
//!     mainnet_nodes()
//! ).build(client);
//!
//! // create the network instance
//! let network = NetworkManager::new(config).await.unwrap();
//!
//! // keep a handle to the network and spawn it
//! let handle = network.handle().clone();
//! tokio::task::spawn(network);
//!
//! # }
//! ```
//!
//! ### Configure all components of the Network with the [`NetworkBuilder`]
//! ### 用[`NetworkBuilder`]配置Network的所有components
//!
//! ```
//! use reth_provider::test_utils::NoopProvider;
//! use reth_transaction_pool::TransactionPool;
//! use reth_primitives::mainnet_nodes;
//! use reth_network::config::rng_secret_key;
//! use reth_network::{NetworkConfig, NetworkManager};
//! async fn launch<Pool: TransactionPool>(pool: Pool) {
//!     // This block provider implementation is used for testing purposes.
//!     let client = NoopProvider::default();
//!
//!     // The key that's used for encrypting sessions and to identify our node.
//!     let local_key = rng_secret_key();
//!
//!     let config =
//!         NetworkConfig::builder(local_key).boot_nodes(mainnet_nodes()).build(client.clone());
//!
//!     // create the network instance
//!     let (handle, network, transactions, request_handler) = NetworkManager::builder(config)
//!         .await
//!         .unwrap()
//!         .transactions(pool)
//!         .request_handler(client)
//!         .split_with_handle();
//! }
//! ```
//!
//! # Feature Flags
//!
//! - `serde` (default): Enable serde support for configuration types.
//! - `test-utils`: Various utilities helpful for writing tests
//! - `geth-tests`: Runs tests that require Geth to be installed locally.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, rustdoc::all)] // TODO(danipopes): unreachable_pub
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[cfg(any(test, feature = "test-utils"))]
/// Common helpers for network testing.
pub mod test_utils;

mod builder;
mod cache;
pub mod config;
mod discovery;
pub mod error;
pub mod eth_requests;
mod fetch;
mod flattened_response;
mod import;
mod listener;
mod manager;
mod message;
mod metrics;
mod network;
pub mod peers;
mod session;
mod state;
mod swarm;
pub mod transactions;

pub use builder::NetworkBuilder;
pub use config::{NetworkConfig, NetworkConfigBuilder};
pub use discovery::{Discovery, DiscoveryEvent};
pub use fetch::FetchClient;
pub use manager::{NetworkEvent, NetworkManager};
pub use message::PeerRequest;
pub use network::NetworkHandle;
pub use peers::PeersConfig;
pub use session::{
    ActiveSessionHandle, ActiveSessionMessage, Direction, PeerInfo, PendingSessionEvent,
    PendingSessionHandle, PendingSessionHandshakeError, SessionCommand, SessionEvent, SessionId,
    SessionLimits, SessionManager, SessionsConfig,
};

pub use reth_eth_wire::{DisconnectReason, HelloBuilder, HelloMessage};
