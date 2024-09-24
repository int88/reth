//! Example of how to use the network as a standalone component together with a transaction pool and
//! a custom pool validator.
//! 示例关于如何使用network作为独立的组件以及一个tx pool以及一个自定义的pool validator
//!
//! Run with
//!
//! ```not_rust
//! cargo run --release -p network-txpool -- node
//! ```

use reth_network::{config::rng_secret_key, NetworkConfig, NetworkManager};
use reth_provider::test_utils::NoopProvider;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, validate::ValidTransaction, CoinbaseTipOrdering,
    EthPooledTransaction, PoolTransaction, TransactionListenerKind, TransactionOrigin,
    TransactionPool, TransactionValidationOutcome, TransactionValidator,
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // This block provider implementation is used for testing purposes.
    // NOTE: This also means that we don't have access to the blockchain and are not able to serve
    // any requests for headers or bodies which can result in dropped connections initiated by
    // remote or able to validate transaction against the latest state.
    // 注意：这也意味着我们不会访问blockchain并且不能服务任何的header或者bodies请求，
    // 这回导致remote初始化的连接的断开或者能够对latest state校验tx
    let client = NoopProvider::default();

    // 构建pool
    let pool = reth_transaction_pool::Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // The key that's used for encrypting sessions and to identify our node.
    // 这个key用于加密sessions并且标识我们的node
    let local_key = rng_secret_key();

    // Configure the network
    // 配置network
    let config = NetworkConfig::builder(local_key).mainnet_boot_nodes().build(client);
    let transactions_manager_config = config.transactions_manager_config.clone();
    // create the network instance
    // 创建network instance
    let (_handle, network, txpool, _) = NetworkManager::builder(config)
        .await?
        .transactions(pool.clone(), transactions_manager_config)
        .split_with_handle();

    // this can be used to interact with the `txpool` service directly
    // 这可以用于直接和`txpool`进行交互
    let _txs_handle = txpool.handle();

    // spawn the network task
    // 生成network task
    tokio::task::spawn(network);
    // spawn the pool task
    // 生成pool task
    tokio::task::spawn(txpool);

    // listen for new transactions
    // 监听新的txs
    let mut txs = pool.pending_transactions_listener_for(TransactionListenerKind::All);

    while let Some(tx) = txs.recv().await {
        println!("Received new transaction: {:?}", tx);
    }

    Ok(())
}

/// A transaction validator that determines all transactions to be valid.
/// 一个tx validator决定所有的txs都是合法的
///
/// An actual validator impl like
/// [TransactionValidationTaskExecutor](reth_transaction_pool::TransactionValidationTaskExecutor)
/// would require up to date db access.
/// 一个真正的validator的实现，需要对于data db的访问
///
/// CAUTION: This validator is not safe to use since it doesn't actually validate the transaction's
/// properties such as chain id, balance, nonce, etc.
#[derive(Default)]
#[non_exhaustive]
struct OkValidator;

impl TransactionValidator for OkValidator {
    type Transaction = EthPooledTransaction;

    async fn validate_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        // Always return valid
        // 总是返回valid
        TransactionValidationOutcome::Valid {
            balance: transaction.cost(),
            state_nonce: transaction.nonce(),
            transaction: ValidTransaction::Valid(transaction),
            propagate: false,
        }
    }
}
