//! Builder support for configuring the entire setup.

use reth_network_api::test_utils::PeersHandleProvider;
use reth_transaction_pool::TransactionPool;
use tokio::sync::mpsc;

use crate::{
    eth_requests::EthRequestHandler,
    transactions::{TransactionsManager, TransactionsManagerConfig},
    NetworkHandle, NetworkManager,
};

/// We set the max channel capacity of the `EthRequestHandler` to 256
/// 我们设置`EthRequestHandler`的channel capacity到256
/// 256 requests with malicious 10MB body requests is 2.6GB which can be absorbed by the node.
/// 有着10MB的body requests的256个请求是2.6GB，可以被节点吸收
pub(crate) const ETH_REQUEST_CHANNEL_CAPACITY: usize = 256;

/// A builder that can configure all components of the network.
#[allow(missing_debug_implementations)]
pub struct NetworkBuilder<Tx, Eth> {
    // 包括network manager，tx以及eth request handler
    pub(crate) network: NetworkManager,
    pub(crate) transactions: Tx,
    pub(crate) request_handler: Eth,
}

// === impl NetworkBuilder ===

impl<Tx, Eth> NetworkBuilder<Tx, Eth> {
    /// Consumes the type and returns all fields.
    pub fn split(self) -> (NetworkManager, Tx, Eth) {
        let Self { network, transactions, request_handler } = self;
        (network, transactions, request_handler)
    }

    /// Returns the network manager.
    pub const fn network(&self) -> &NetworkManager {
        &self.network
    }

    /// Returns the mutable network manager.
    pub fn network_mut(&mut self) -> &mut NetworkManager {
        &mut self.network
    }

    /// Returns the handle to the network.
    pub fn handle(&self) -> NetworkHandle {
        self.network.handle().clone()
    }

    /// Consumes the type and returns all fields and also return a [`NetworkHandle`].
    /// 消费type并且返回所有的字段，同时也返回[`NetworkHandle`]
    pub fn split_with_handle(self) -> (NetworkHandle, NetworkManager, Tx, Eth) {
        let Self { network, transactions, request_handler } = self;
        let handle = network.handle().clone();
        (handle, network, transactions, request_handler)
    }

    /// Creates a new [`TransactionsManager`] and wires it to the network.
    /// 创建一个新的[`TransactionsManager`]并且将它连接到network
    pub fn transactions<Pool: TransactionPool>(
        self,
        pool: Pool,
        transactions_manager_config: TransactionsManagerConfig,
    ) -> NetworkBuilder<TransactionsManager<Pool>, Eth> {
        let Self { mut network, request_handler, .. } = self;
        let (tx, rx) = mpsc::unbounded_channel();
        // 设置tx
        network.set_transactions(tx);
        let handle = network.handle().clone();
        // 设置tx manager
        let transactions = TransactionsManager::new(handle, pool, rx, transactions_manager_config);
        NetworkBuilder { network, request_handler, transactions }
    }

    /// Creates a new [`EthRequestHandler`] and wires it to the network.
    /// 创建一个新的[`EthRequestHandler`]并且关联到network
    pub fn request_handler<Client>(
        self,
        client: Client,
    ) -> NetworkBuilder<Tx, EthRequestHandler<Client>> {
        let Self { mut network, transactions, .. } = self;
        let (tx, rx) = mpsc::channel(ETH_REQUEST_CHANNEL_CAPACITY);
        network.set_eth_request_handler(tx);
        let peers = network.handle().peers_handle().clone();
        let request_handler = EthRequestHandler::new(client, peers, rx);
        NetworkBuilder { network, request_handler, transactions }
    }
}
