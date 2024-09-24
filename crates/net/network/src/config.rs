//! Network config support

use std::{collections::HashSet, net::SocketAddr, sync::Arc};

use reth_chainspec::{ChainSpec, MAINNET};
use reth_discv4::{Discv4Config, Discv4ConfigBuilder, NatResolver, DEFAULT_DISCOVERY_ADDRESS};
use reth_discv5::NetworkStackId;
use reth_dns_discovery::DnsDiscoveryConfig;
use reth_eth_wire::{HelloMessage, HelloMessageWithProtocols, Status};
use reth_network_peers::{mainnet_nodes, pk2id, sepolia_nodes, PeerId, TrustedPeer};
use reth_network_types::{PeersConfig, SessionsConfig};
use reth_primitives::{ForkFilter, Head};
use reth_storage_api::{BlockNumReader, BlockReader, HeaderProvider};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use secp256k1::SECP256K1;

use crate::{
    error::NetworkError,
    import::{BlockImport, ProofOfStakeBlockImport},
    transactions::TransactionsManagerConfig,
    NetworkHandle, NetworkManager,
};

// re-export for convenience
use crate::protocol::{IntoRlpxSubProtocol, RlpxSubProtocols};
pub use secp256k1::SecretKey;

/// Convenience function to create a new random [`SecretKey`]
/// 方便的函数用来创建一个新的随机的[`SecretKey`]
pub fn rng_secret_key() -> SecretKey {
    SecretKey::new(&mut rand::thread_rng())
}

/// All network related initialization settings.
/// 所有network相关的初始设置
#[derive(Debug)]
pub struct NetworkConfig<C> {
    /// The client type that can interact with the chain.
    /// client类型用于和chain交互
    ///
    /// This type is used to fetch the block number after we established a session and received the
    /// [Status] block hash.
    /// 这个类型用于获取block number，在我们建立一个session并且接收到[Status] block hash之后
    pub client: C,
    /// The node's secret key, from which the node's identity is derived.
    /// node的secret key，从中衍生出node的identity
    pub secret_key: SecretKey,
    /// All boot nodes to start network discovery with.
    /// 所有的boot nodes，开始network discovery
    pub boot_nodes: HashSet<TrustedPeer>,
    /// How to set up discovery over DNS.
    /// 如何通过DNS设置discovery
    pub dns_discovery_config: Option<DnsDiscoveryConfig>,
    /// Address to use for discovery v4.
    /// 用于discovery V4的地址
    pub discovery_v4_addr: SocketAddr,
    /// How to set up discovery.
    /// 如何设置discovery
    pub discovery_v4_config: Option<Discv4Config>,
    /// How to set up discovery version 5.
    /// 如何设置discovery v5
    pub discovery_v5_config: Option<reth_discv5::Config>,
    /// Address to listen for incoming connections
    /// 用于监听incoming connections的地址
    pub listener_addr: SocketAddr,
    /// How to instantiate peer manager.
    /// 如何初始化peer manager
    pub peers_config: PeersConfig,
    /// How to configure the [`SessionManager`](crate::session::SessionManager).
    /// 如何配置[`SessionManager`]
    pub sessions_config: SessionsConfig,
    /// The chain spec
    pub chain_spec: Arc<ChainSpec>,
    /// The [`ForkFilter`] to use at launch for authenticating sessions.
    /// [`ForkFilter`]在启动的时候用于认证sessions
    ///
    /// See also <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md#stale-software-examples>
    ///
    /// For sync from block `0`, this should be the default chain [`ForkFilter`] beginning at the
    /// first hardfork, `Frontier` for mainnet.
    /// 对于从block `0`开始同步，这应该为默认的chain的[`ForkFilter`]从第一个hardfork，
    /// 即`Frontier`，对于mainnet来说，开始
    pub fork_filter: ForkFilter,
    /// The block importer type.
    /// block importer类型
    pub block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    pub network_mode: NetworkMode,
    /// The executor to use for spawning tasks.
    /// 用于生成tasks的executor
    pub executor: Box<dyn TaskSpawner>,
    /// The `Status` message to send to peers at the beginning.
    /// 在开始的时候发送给peers的`Status`
    pub status: Status,
    /// Sets the hello message for the p2p handshake in `RLPx`
    /// 用于在`RLPx`的p2p握手时设置hello message
    pub hello_message: HelloMessageWithProtocols,
    /// Additional protocols to announce and handle in `RLPx`
    /// 在`RLPx`中处理和声明的额外的protocols
    pub extra_protocols: RlpxSubProtocols,
    /// Whether to disable transaction gossip
    /// 是否禁止transaction gossip
    pub tx_gossip_disabled: bool,
    /// How to instantiate transactions manager.
    /// 如何初始化txs managers
    pub transactions_manager_config: TransactionsManagerConfig,
}

// === impl NetworkConfig ===

impl NetworkConfig<()> {
    /// Convenience method for creating the corresponding builder type
    /// 用于创建对应的builder类型
    pub fn builder(secret_key: SecretKey) -> NetworkConfigBuilder {
        NetworkConfigBuilder::new(secret_key)
    }

    /// Convenience method for creating the corresponding builder type with a random secret key.
    pub fn builder_with_rng_secret_key() -> NetworkConfigBuilder {
        NetworkConfigBuilder::with_rng_secret_key()
    }
}

impl<C> NetworkConfig<C> {
    /// Create a new instance with all mandatory fields set, rest is field with defaults.
    pub fn new(client: C, secret_key: SecretKey) -> Self {
        NetworkConfig::builder(secret_key).build(client)
    }

    /// Sets the config to use for the discovery v4 protocol.
    pub fn set_discovery_v4(mut self, discovery_config: Discv4Config) -> Self {
        self.discovery_v4_config = Some(discovery_config);
        self
    }

    /// Sets the address for the incoming `RLPx` connection listener.
    pub const fn set_listener_addr(mut self, listener_addr: SocketAddr) -> Self {
        self.listener_addr = listener_addr;
        self
    }

    /// Returns the address for the incoming `RLPx` connection listener.
    pub const fn listener_addr(&self) -> &SocketAddr {
        &self.listener_addr
    }
}

impl<C> NetworkConfig<C>
where
    C: BlockNumReader + 'static,
{
    /// Convenience method for calling [`NetworkManager::new`].
    pub async fn manager(self) -> Result<NetworkManager, NetworkError> {
        NetworkManager::new(self).await
    }
}

impl<C> NetworkConfig<C>
where
    C: BlockReader + HeaderProvider + Clone + Unpin + 'static,
{
    /// Starts the networking stack given a [`NetworkConfig`] and returns a handle to the network.
    pub async fn start_network(self) -> Result<NetworkHandle, NetworkError> {
        let client = self.client.clone();
        let (handle, network, _txpool, eth) = NetworkManager::builder::<C>(self)
            .await?
            .request_handler::<C>(client)
            .split_with_handle();

        tokio::task::spawn(network);
        tokio::task::spawn(eth);
        Ok(handle)
    }
}

/// Builder for [`NetworkConfig`](struct.NetworkConfig.html).
/// 对于[`NetworkConfig`]的Builder
#[derive(Debug)]
pub struct NetworkConfigBuilder {
    /// The node's secret key, from which the node's identity is derived.
    secret_key: SecretKey,
    /// How to configure discovery over DNS.
    dns_discovery_config: Option<DnsDiscoveryConfig>,
    /// How to set up discovery version 4.
    discovery_v4_builder: Option<Discv4ConfigBuilder>,
    /// How to set up discovery version 5.
    discovery_v5_builder: Option<reth_discv5::ConfigBuilder>,
    /// All boot nodes to start network discovery with.
    /// 启动network discovery的boot nodes类型
    boot_nodes: HashSet<TrustedPeer>,
    /// Address to use for discovery
    discovery_addr: Option<SocketAddr>,
    /// Listener for incoming connections
    listener_addr: Option<SocketAddr>,
    /// How to instantiate peer manager.
    /// 如何实例化peer manager
    peers_config: Option<PeersConfig>,
    /// How to configure the sessions manager
    /// 如何配置sessions manager
    sessions_config: Option<SessionsConfig>,
    /// The network's chain spec
    chain_spec: Arc<ChainSpec>,
    /// The default mode of the network.
    network_mode: NetworkMode,
    /// The executor to use for spawning tasks.
    executor: Option<Box<dyn TaskSpawner>>,
    /// Sets the hello message for the p2p handshake in `RLPx`
    hello_message: Option<HelloMessageWithProtocols>,
    /// The executor to use for spawning tasks.
    extra_protocols: RlpxSubProtocols,
    /// Head used to start set for the fork filter and status.
    /// Head用于开始设置fork filter以及status
    head: Option<Head>,
    /// Whether tx gossip is disabled
    tx_gossip_disabled: bool,
    /// The block importer type
    /// block importer类型
    block_import: Option<Box<dyn BlockImport>>,
    /// How to instantiate transactions manager.
    /// 如何初始化tx manager
    transactions_manager_config: TransactionsManagerConfig,
}

// === impl NetworkConfigBuilder ===

#[allow(missing_docs)]
impl NetworkConfigBuilder {
    /// Create a new builder instance with a random secret key.
    /// 创建一个新的builder，有着随机的secret key
    pub fn with_rng_secret_key() -> Self {
        Self::new(rng_secret_key())
    }

    /// Create a new builder instance with the given secret key.
    /// 创建一个新的builder instance，有着给定的secret key
    pub fn new(secret_key: SecretKey) -> Self {
        Self {
            secret_key,
            // 都是默认配置
            dns_discovery_config: Some(Default::default()),
            discovery_v4_builder: Some(Default::default()),
            discovery_v5_builder: None,
            boot_nodes: Default::default(),
            discovery_addr: None,
            listener_addr: None,
            peers_config: None,
            sessions_config: None,
            // 克隆MAINNET
            chain_spec: MAINNET.clone(),
            network_mode: Default::default(),
            executor: None,
            hello_message: None,
            extra_protocols: Default::default(),
            head: None,
            tx_gossip_disabled: false,
            block_import: None,
            transactions_manager_config: Default::default(),
        }
    }

    /// Apply a function to the builder.
    pub fn apply<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    /// Returns the configured [`PeerId`]
    pub fn get_peer_id(&self) -> PeerId {
        pk2id(&self.secret_key.public_key(SECP256K1))
    }

    /// Returns the configured [`SecretKey`], from which the node's identity is derived.
    pub const fn secret_key(&self) -> &SecretKey {
        &self.secret_key
    }

    /// Sets the chain spec.
    pub fn chain_spec(mut self, chain_spec: Arc<ChainSpec>) -> Self {
        self.chain_spec = chain_spec;
        self
    }

    /// Sets the [`NetworkMode`].
    pub const fn network_mode(mut self, network_mode: NetworkMode) -> Self {
        self.network_mode = network_mode;
        self
    }

    /// Sets the highest synced block.
    ///
    /// This is used to construct the appropriate [`ForkFilter`] and [`Status`] message.
    ///
    /// If not set, this defaults to the genesis specified by the current chain specification.
    pub const fn set_head(mut self, head: Head) -> Self {
        self.head = Some(head);
        self
    }

    /// Sets the `HelloMessage` to send when connecting to peers.
    ///
    /// ```
    /// # use reth_eth_wire::HelloMessage;
    /// # use reth_network::NetworkConfigBuilder;
    /// # fn builder(builder: NetworkConfigBuilder) {
    /// let peer_id = builder.get_peer_id();
    /// builder.hello_message(HelloMessage::builder(peer_id).build());
    /// # }
    /// ```
    pub fn hello_message(mut self, hello_message: HelloMessageWithProtocols) -> Self {
        self.hello_message = Some(hello_message);
        self
    }

    /// Set a custom peer config for how peers are handled
    pub fn peer_config(mut self, config: PeersConfig) -> Self {
        self.peers_config = Some(config);
        self
    }

    /// Sets the executor to use for spawning tasks.
    ///
    /// If `None`, then [`tokio::spawn`] is used for spawning tasks.
    pub fn with_task_executor(mut self, executor: Box<dyn TaskSpawner>) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Sets a custom config for how sessions are handled.
    pub const fn sessions_config(mut self, config: SessionsConfig) -> Self {
        self.sessions_config = Some(config);
        self
    }

    /// Configures the transactions manager with the given config.
    pub const fn transactions_manager_config(mut self, config: TransactionsManagerConfig) -> Self {
        self.transactions_manager_config = config;
        self
    }

    /// Sets the discovery and listener address
    ///
    /// This is a convenience function for both [`NetworkConfigBuilder::listener_addr`] and
    /// [`NetworkConfigBuilder::discovery_addr`].
    ///
    /// By default, both are on the same port:
    /// [`DEFAULT_DISCOVERY_PORT`](reth_discv4::DEFAULT_DISCOVERY_PORT)
    pub const fn set_addrs(self, addr: SocketAddr) -> Self {
        self.listener_addr(addr).discovery_addr(addr)
    }

    /// Sets the socket address the network will listen on.
    ///
    /// By default, this is [`DEFAULT_DISCOVERY_ADDRESS`]
    pub const fn listener_addr(mut self, listener_addr: SocketAddr) -> Self {
        self.listener_addr = Some(listener_addr);
        self
    }

    /// Sets the port of the address the network will listen on.
    ///
    /// By default, this is [`DEFAULT_DISCOVERY_PORT`](reth_discv4::DEFAULT_DISCOVERY_PORT)
    pub fn listener_port(mut self, port: u16) -> Self {
        self.listener_addr.get_or_insert(DEFAULT_DISCOVERY_ADDRESS).set_port(port);
        self
    }

    /// Sets the socket address the discovery network will listen on
    pub const fn discovery_addr(mut self, discovery_addr: SocketAddr) -> Self {
        self.discovery_addr = Some(discovery_addr);
        self
    }

    /// Sets the port of the address the discovery network will listen on.
    ///
    /// By default, this is [`DEFAULT_DISCOVERY_PORT`](reth_discv4::DEFAULT_DISCOVERY_PORT)
    pub fn discovery_port(mut self, port: u16) -> Self {
        self.discovery_addr.get_or_insert(DEFAULT_DISCOVERY_ADDRESS).set_port(port);
        self
    }

    /// Sets the external ip resolver to use for discovery v4.
    ///
    /// If no [`Discv4ConfigBuilder`] is set via [`Self::discovery`], this will create a new one.
    ///
    /// This is a convenience function for setting the external ip resolver on the default
    /// [`Discv4Config`] config.
    pub fn external_ip_resolver(mut self, resolver: NatResolver) -> Self {
        self.discovery_v4_builder
            .get_or_insert_with(Discv4Config::builder)
            .external_ip_resolver(Some(resolver));
        self
    }

    /// Sets the discv4 config to use.
    pub fn discovery(mut self, builder: Discv4ConfigBuilder) -> Self {
        self.discovery_v4_builder = Some(builder);
        self
    }

    /// Sets the discv5 config to use.
    pub fn discovery_v5(mut self, builder: reth_discv5::ConfigBuilder) -> Self {
        self.discovery_v5_builder = Some(builder);
        self
    }

    /// Sets the dns discovery config to use.
    pub fn dns_discovery(mut self, config: DnsDiscoveryConfig) -> Self {
        self.dns_discovery_config = Some(config);
        self
    }

    /// Convenience function for setting [`Self::boot_nodes`] to the mainnet boot nodes.
    /// 将[`Self::boot_nodes`]设置为mainnet boot nodes
    pub fn mainnet_boot_nodes(self) -> Self {
        self.boot_nodes(mainnet_nodes())
    }

    /// Convenience function for setting [`Self::boot_nodes`] to the sepolia boot nodes.
    pub fn sepolia_boot_nodes(self) -> Self {
        self.boot_nodes(sepolia_nodes())
    }

    /// Sets the boot nodes to use to bootstrap the configured discovery services (discv4 + discv5).
    pub fn boot_nodes<T: Into<TrustedPeer>>(mut self, nodes: impl IntoIterator<Item = T>) -> Self {
        self.boot_nodes = nodes.into_iter().map(Into::into).collect();
        self
    }

    /// Returns an iterator over all configured boot nodes.
    pub fn boot_nodes_iter(&self) -> impl Iterator<Item = &TrustedPeer> + '_ {
        self.boot_nodes.iter()
    }

    /// Disable the DNS discovery.
    pub fn disable_dns_discovery(mut self) -> Self {
        self.dns_discovery_config = None;
        self
    }

    /// Disables all discovery.
    pub fn disable_discovery(self) -> Self {
        self.disable_discv4_discovery().disable_dns_discovery()
    }

    /// Disables all discovery if the given condition is true.
    pub fn disable_discovery_if(self, disable: bool) -> Self {
        if disable {
            self.disable_discovery()
        } else {
            self
        }
    }

    /// Disable the Discv4 discovery.
    pub fn disable_discv4_discovery(mut self) -> Self {
        self.discovery_v4_builder = None;
        self
    }

    /// Disable the DNS discovery if the given condition is true.
    pub fn disable_dns_discovery_if(self, disable: bool) -> Self {
        if disable {
            self.disable_dns_discovery()
        } else {
            self
        }
    }

    /// Disable the Discv4 discovery if the given condition is true.
    pub fn disable_discv4_discovery_if(self, disable: bool) -> Self {
        if disable {
            self.disable_discv4_discovery()
        } else {
            self
        }
    }

    /// Adds a new additional protocol to the `RLPx` sub-protocol list.
    pub fn add_rlpx_sub_protocol(mut self, protocol: impl IntoRlpxSubProtocol) -> Self {
        self.extra_protocols.push(protocol);
        self
    }

    /// Sets whether tx gossip is disabled.
    pub const fn disable_tx_gossip(mut self, disable_tx_gossip: bool) -> Self {
        self.tx_gossip_disabled = disable_tx_gossip;
        self
    }

    /// Sets the block import type.
    pub fn block_import(mut self, block_import: Box<dyn BlockImport>) -> Self {
        self.block_import = Some(block_import);
        self
    }

    /// Convenience function for creating a [`NetworkConfig`] with a noop provider that does
    /// nothing.
    pub fn build_with_noop_provider(
        self,
    ) -> NetworkConfig<reth_storage_api::noop::NoopBlockReader> {
        self.build(Default::default())
    }

    /// Consumes the type and creates the actual [`NetworkConfig`]
    /// for the given client type that can interact with the chain.
    /// 消费类型并且创建真正的[`NetworkConfig`]，用给定的client类型和chain进行交互
    ///
    /// The given client is to be used for interacting with the chain, for example fetching the
    /// corresponding block for a given block hash we receive from a peer in the status message when
    /// establishing a connection.
    /// 给定的client用于和chain交互，例如获取对应的block，对于给定的block
    /// hash，我们从一个peer的status message接收得到，当建立一个连接的时候
    pub fn build<C>(self, client: C) -> NetworkConfig<C> {
        let peer_id = self.get_peer_id();
        let Self {
            secret_key,
            mut dns_discovery_config,
            discovery_v4_builder,
            mut discovery_v5_builder,
            boot_nodes,
            discovery_addr,
            listener_addr,
            peers_config,
            sessions_config,
            chain_spec,
            network_mode,
            executor,
            hello_message,
            extra_protocols,
            head,
            tx_gossip_disabled,
            block_import,
            transactions_manager_config,
        } = self;

        // 构建discovery v5 builder
        discovery_v5_builder = discovery_v5_builder.map(|mut builder| {
            if let Some(network_stack_id) = NetworkStackId::id(&chain_spec) {
                let fork_id = chain_spec.latest_fork_id();
                builder = builder.fork(network_stack_id, fork_id)
            }

            builder
        });

        let listener_addr = listener_addr.unwrap_or(DEFAULT_DISCOVERY_ADDRESS);

        // 构建hello message
        let mut hello_message =
            hello_message.unwrap_or_else(|| HelloMessage::builder(peer_id).build());
        hello_message.port = listener_addr.port();

        let head = head.unwrap_or_else(|| Head {
            hash: chain_spec.genesis_hash(),
            number: 0,
            timestamp: chain_spec.genesis.timestamp,
            difficulty: chain_spec.genesis.difficulty,
            total_difficulty: chain_spec.genesis.difficulty,
        });

        // set the status
        // 设置status
        let status = Status::spec_builder(&chain_spec, &head).build();

        // set a fork filter based on the chain spec and head
        // 设置一个fork filter，基于chain spec以及head
        let fork_filter = chain_spec.fork_filter(head);

        // If default DNS config is used then we add the known dns network to bootstrap from
        // 如果使用默认的DNS config，那么我们增加已知的dns network到bootstrap from
        if let Some(dns_networks) =
            dns_discovery_config.as_mut().and_then(|c| c.bootstrap_dns_networks.as_mut())
        {
            if dns_networks.is_empty() {
                if let Some(link) = chain_spec.chain().public_dns_network_protocol() {
                    dns_networks.insert(link.parse().expect("is valid DNS link entry"));
                }
            }
        }

        // 最终生成NetworkConfig
        NetworkConfig {
            client,
            secret_key,
            boot_nodes,
            dns_discovery_config,
            discovery_v4_config: discovery_v4_builder.map(|builder| builder.build()),
            discovery_v5_config: discovery_v5_builder.map(|builder| builder.build()),
            discovery_v4_addr: discovery_addr.unwrap_or(DEFAULT_DISCOVERY_ADDRESS),
            listener_addr,
            peers_config: peers_config.unwrap_or_default(),
            sessions_config: sessions_config.unwrap_or_default(),
            chain_spec,
            block_import: block_import.unwrap_or_else(|| Box::<ProofOfStakeBlockImport>::default()),
            network_mode,
            executor: executor.unwrap_or_else(|| Box::<TokioTaskExecutor>::default()),
            status,
            hello_message,
            extra_protocols,
            fork_filter,
            tx_gossip_disabled,
            transactions_manager_config,
        }
    }
}

/// Describes the mode of the network wrt. POS or POW.
///
/// This affects block propagation in the `eth` sub-protocol [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
///
/// In POS `NewBlockHashes` and `NewBlock` messages become invalid.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NetworkMode {
    /// Network is in proof-of-work mode.
    Work,
    /// Network is in proof-of-stake mode
    #[default]
    Stake,
}

// === impl NetworkMode ===

impl NetworkMode {
    /// Returns true if network has entered proof-of-stake
    pub const fn is_stake(&self) -> bool {
        matches!(self, Self::Stake)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use reth_chainspec::Chain;
    use reth_dns_discovery::tree::LinkEntry;
    use reth_primitives::ForkHash;
    use reth_provider::test_utils::NoopProvider;

    fn builder() -> NetworkConfigBuilder {
        let secret_key = SecretKey::new(&mut thread_rng());
        NetworkConfigBuilder::new(secret_key)
    }

    #[test]
    fn test_network_dns_defaults() {
        let config = builder().build(NoopProvider::default());

        let dns = config.dns_discovery_config.unwrap();
        let bootstrap_nodes = dns.bootstrap_dns_networks.unwrap();
        let mainnet_dns: LinkEntry =
            Chain::mainnet().public_dns_network_protocol().unwrap().parse().unwrap();
        assert!(bootstrap_nodes.contains(&mainnet_dns));
        assert_eq!(bootstrap_nodes.len(), 1);
    }

    #[test]
    fn test_network_fork_filter_default() {
        let mut chain_spec = Arc::clone(&MAINNET);

        // remove any `next` fields we would have by removing all hardforks
        Arc::make_mut(&mut chain_spec).hardforks = Default::default();

        // check that the forkid is initialized with the genesis and no other forks
        let genesis_fork_hash = ForkHash::from(chain_spec.genesis_hash());

        // enforce that the fork_id set in the status is consistent with the generated fork filter
        let config = builder().chain_spec(chain_spec).build(NoopProvider::default());

        let status = config.status;
        let fork_filter = config.fork_filter;

        // assert that there are no other forks
        assert_eq!(status.forkid.next, 0);

        // assert the same thing for the fork_filter
        assert_eq!(fork_filter.current().next, 0);

        // check status and fork_filter forkhash
        assert_eq!(status.forkid.hash, genesis_fork_hash);
        assert_eq!(fork_filter.current().hash, genesis_fork_hash);
    }
}
