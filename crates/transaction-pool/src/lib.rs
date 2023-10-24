//! Reth's transaction pool implementation.
//! Reth的tx pool实现
//!
//! This crate provides a generic transaction pool implementation.
//! 这个crate提供了一个通用的tx pool的实现
//!
//! ## Functionality
//! ## 功能
//!
//! The transaction pool is responsible for
//! tx pool负责实现：
//!
//!    - recording incoming transactions
//!    - 记录incoming txs
//!    - providing existing transactions
//!    - 提供已经存在的txs
//!    - ordering and providing the best transactions for block production
//!    - 排序并且提供最好的txs用于block production
//!    - monitoring memory footprint and enforce pool size limits
//!    - 监控memory footprint并且执行pool size limits
//!    - storing blob data for transactions in a separate blobstore on insertion
//!    - 存储blob data用于txs，在一个独立的blobstore，在插入时
//!
//! ## Assumptions
//!
//! ### Transaction type
//!
//! The pool expects certain ethereum related information from the generic transaction type of the
//! pool ([`PoolTransaction`]), this includes gas price, base fee (EIP-1559 transactions), nonce
//! etc. It makes no assumptions about the encoding format, but the transaction type must report its
//! size so pool size limits (memory) can be enforced.
//! pool期望从通用的tx type `PoolTranasctin`获取ethereum相关的信息，包括gas price，base
//! feee（EIP-1559 txs），nonce等，它不假设encoding format，但是txs type必须报告它的size，这样pool
//! size limits可以被执行
//!
//!
//! ### Transaction ordering
//!
//! The pending pool contains transactions that can be mined on the current state.
//! The order in which they're returned are determined by a `Priority` value returned by the
//! `TransactionOrdering` type this pool is configured with.
//! txs的顺序由返回的`Priority`值决定，这个pool的`TransationOrdering`值会配置
//!
//! This is only used in the _pending_ pool to yield the best transactions for block production. The
//! _base pool_ is ordered by base fee, and the _queued pool_ by current distance.
//! 这只用于_pending_ pool产生用于block production的best transactions，_base pool_由base
//! fee排序，_queued pool_由当前的distance排序
//!
//! ### Validation
//!
//! The pool itself does not validate incoming transactions, instead this should be provided by
//! implementing `TransactionsValidator`. Only transactions that the validator returns as valid are
//! included in the pool. It is assumed that transaction that are in the pool are either valid on
//! the current state or could become valid after certain state changes. transaction that can never
//! become valid (e.g. nonce lower than current on chain nonce) will never be added to the pool and
//! instead are discarded right away.
//! pool自身并不校验incoming
//! transactions，这应该由实现`TransactionsValidator`提供，
//! 只有validator返回的合法的txs被包含进pool，
//! 这假设pool中的tx要么在当前状态是合法的或者在特定的state
//! changes之后可以变得合法，不可能变得合法的tx（例如nonce小于当前的chain
//! nonce）不会被加入到pool并且应该立刻被丢弃
//!
//! ### State Changes
//!
//! Once a new block is mined, the pool needs to be updated with a changeset in order to:
//! 一旦一个新的block被mined，pool应该用一个changeset更新，为了：
//!
//!   - remove mined transactions
//!   - 移除mined txs
//!   - update using account changes: balance changes
//!   - 使用account changes: balance changes更新
//!   - base fee updates
//!   - base fee的更新
//!
//! ## Implementation details
//!
//! The `TransactionPool` trait exposes all externally used functionality of the pool, such as
//! inserting, querying specific transactions by hash or retrieving the best transactions.
//! In addition, it enables the registration of event listeners that are notified of state changes.
//! Events are communicated via channels.
//! 另外，它允许event listeners的注册，能够被通知state changes，Events通过channels交互
//!
//! ### Architecture
//!
//! The final `TransactionPool` is made up of two layers:
//! 最终的`TransactionPool`由两层组成：
//!
//! The lowest layer is the actual pool implementations that manages (validated) transactions:
//! 最底层是真正pool的实现，用于管理（合法的）txs：[`TxPool`](crate::pool::txpool::TxPool)
//! [`TxPool`](crate::pool::txpool::TxPool). This is contained in a higher level pool type that
//! guards the low level pool and handles additional listeners or metrics: [`PoolInner`].
//! 这被包含在高一级的pool，看守额外的listeners或者metrics: [`PoolInner`]
//!
//! The transaction pool will be used by separate consumers (RPC, P2P), to make sharing easier, the
//! [`Pool`] type is just an `Arc` wrapper around `PoolInner`. This is the usable type that provides
//! the `TransactionPool` interface.
//! tx pool会被分开的consumers（RPC, P2P）使用，来让共享更轻松，`Pool`只是`PoolInner`的一个`Arc`
//! wrapper，这是usable type用于提供`TransactionPool`接口
//!
//!
//! ## Blob Transactions
//!
//! Blob transaction can be quite large hence they are stored in a separate blobstore. The pool is
//! responsible for inserting blob data for new transactions into the blobstore.
//! Blob tx可以非常大，因此它们被存储在另外的blobstore，这个pool负责插入新的tx的blob
//! data到blobstore中 See also [ValidTransaction](validate::ValidTransaction)
//!
//!
//! ## Examples
//!
//! Listen for new transactions and print them:
//!
//! ```
//! use reth_primitives::MAINNET;
//! use reth_provider::{ChainSpecProvider, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool, TransactionPool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//!  async fn t<C>(client: C)  where C: StateProviderFactory + ChainSpecProvider + Clone + 'static{
//!     let blob_store = InMemoryBlobStore::default();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client, MAINNET.clone(), blob_store.clone(), TokioTaskExecutor::default()),
//!         blob_store,
//!         Default::default(),
//!     );
//!   let mut transactions = pool.pending_transactions_listener();
//!   tokio::task::spawn( async move {
//!      while let Some(tx) = transactions.recv().await {
//!          println!("New transaction: {:?}", tx);
//!      }
//!   });
//!
//!   // do something useful with the pool, like RPC integration
//!
//! # }
//! ```
//!
//! Spawn maintenance task to keep the pool updated
//!
//! ```
//! use futures_util::Stream;
//! use reth_primitives::MAINNET;
//! use reth_provider::{BlockReaderIdExt, CanonStateNotification, ChainSpecProvider, StateProviderFactory};
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool};
//! use reth_transaction_pool::blobstore::InMemoryBlobStore;
//! use reth_transaction_pool::maintain::maintain_transaction_pool_future;
//!  async fn t<C, St>(client: C, stream: St)
//!    where C: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
//!     St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
//!     {
//!     let blob_store = InMemoryBlobStore::default();
//!     let pool = Pool::eth_pool(
//!         TransactionValidationTaskExecutor::eth(client.clone(), MAINNET.clone(), blob_store.clone(), TokioTaskExecutor::default()),
//!         blob_store,
//!         Default::default(),
//!     );
//!
//!   // spawn a task that listens for new blocks and updates the pool's transactions, mined transactions etc..
//!   tokio::task::spawn(  maintain_transaction_pool_future(client, pool, stream, TokioTaskExecutor::default(), Default::default()));
//!
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! - `serde` (default): Enable serde support
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::pool::PoolInner;
use aquamarine as _;
use reth_primitives::{Address, BlobTransactionSidecar, PooledTransactionsElement, TxHash, U256};
use reth_provider::StateProviderFactory;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::mpsc::Receiver;
use tracing::{instrument, trace};

pub use crate::{
    blobstore::{BlobStore, BlobStoreError},
    config::{
        PoolConfig, PriceBumpConfig, SubPoolLimit, DEFAULT_PRICE_BUMP, REPLACE_BLOB_PRICE_BUMP,
        TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER, TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT,
        TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
    },
    error::PoolResult,
    ordering::{CoinbaseTipOrdering, Priority, TransactionOrdering},
    pool::{
        state::SubPool, AllTransactionsEvents, FullTransactionEvent, TransactionEvent,
        TransactionEvents,
    },
    traits::{
        AllPoolTransactions, BestTransactions, BlockInfo, CanonicalStateUpdate, ChangedAccount,
        EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction,
        GetPooledTransactionLimit, NewBlobSidecar, NewTransactionEvent, PoolSize, PoolTransaction,
        PropagateKind, PropagatedTransactions, TransactionListenerKind, TransactionOrigin,
        TransactionPool, TransactionPoolExt,
    },
    validate::{
        EthTransactionValidator, TransactionValidationOutcome, TransactionValidationTaskExecutor,
        TransactionValidator, ValidPoolTransaction,
    },
};

pub mod error;
pub mod maintain;
pub mod metrics;
pub mod noop;
pub mod pool;
pub mod validate;

pub mod blobstore;
mod config;
mod identifier;
mod ordering;
mod traits;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking a pool
pub mod test_utils;

/// A shareable, generic, customizable `TransactionPool` implementation.
#[derive(Debug)]
pub struct Pool<V, T: TransactionOrdering, S> {
    /// Arc'ed instance of the pool internals
    pool: Arc<PoolInner<V, T, S>>,
}

// === impl Pool ===

impl<V, T, S> Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    /// Create a new transaction pool instance.
    pub fn new(validator: V, ordering: T, blob_store: S, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(validator, ordering, blob_store, config)) }
    }

    /// Returns the wrapped pool.
    pub(crate) fn inner(&self) -> &PoolInner<V, T, S> {
        &self.pool
    }

    /// Get the config the pool was configured with.
    pub fn config(&self) -> &PoolConfig {
        self.inner().config()
    }

    /// Returns future that validates all transaction in the given iterator.
    async fn validate_all(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = V::Transaction>,
    ) -> PoolResult<HashMap<TxHash, TransactionValidationOutcome<V::Transaction>>> {
        let outcome = futures_util::future::join_all(
            transactions.into_iter().map(|tx| self.validate(origin, tx)),
        )
        .await
        .into_iter()
        .collect::<HashMap<_, _>>();

        Ok(outcome)
    }

    /// Validates the given transaction
    async fn validate(
        &self,
        origin: TransactionOrigin,
        transaction: V::Transaction,
    ) -> (TxHash, TransactionValidationOutcome<V::Transaction>) {
        let hash = *transaction.hash();

        let outcome = self.pool.validator().validate_transaction(origin, transaction).await;

        (hash, outcome)
    }

    /// Number of transactions in the entire pool
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    /// Whether the pool is empty
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

impl<Client, S>
    Pool<
        TransactionValidationTaskExecutor<EthTransactionValidator<Client, EthPooledTransaction>>,
        CoinbaseTipOrdering<EthPooledTransaction>,
        S,
    >
where
    Client: StateProviderFactory + Clone + 'static,
    S: BlobStore,
{
    /// Returns a new [Pool] that uses the default [TransactionValidationTaskExecutor] when
    /// validating [EthPooledTransaction]s and ords via [CoinbaseTipOrdering]
    ///
    /// # Example
    ///
    /// ```
    /// use reth_provider::StateProviderFactory;
    /// use reth_primitives::MAINNET;
    /// use reth_tasks::TokioTaskExecutor;
    /// use reth_transaction_pool::{TransactionValidationTaskExecutor, Pool};
    /// use reth_transaction_pool::blobstore::InMemoryBlobStore;
    /// # fn t<C>(client: C)  where C: StateProviderFactory + Clone + 'static {
    ///     let blob_store = InMemoryBlobStore::default();
    ///     let pool = Pool::eth_pool(
    ///         TransactionValidationTaskExecutor::eth(client, MAINNET.clone(), blob_store.clone(), TokioTaskExecutor::default()),
    ///         blob_store,
    ///         Default::default(),
    ///     );
    /// # }
    /// ```
    pub fn eth_pool(
        validator: TransactionValidationTaskExecutor<
            EthTransactionValidator<Client, EthPooledTransaction>,
        >,
        blob_store: S,
        config: PoolConfig,
    ) -> Self {
        Self::new(validator, CoinbaseTipOrdering::default(), blob_store, config)
    }
}

/// implements the `TransactionPool` interface for various transaction pool API consumers.
#[async_trait::async_trait]
impl<V, T, S> TransactionPool for Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    type Transaction = T::Transaction;

    fn pool_size(&self) -> PoolSize {
        self.pool.size()
    }

    fn block_info(&self) -> BlockInfo {
        self.pool.block_info()
    }

    async fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents> {
        let (_, tx) = self.validate(origin, transaction).await;
        self.pool.add_transaction_and_subscribe(origin, tx)
    }

    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        let (_, tx) = self.validate(origin, transaction).await;
        self.pool.add_transactions(origin, std::iter::once(tx)).pop().expect("exists; qed")
    }

    async fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        let validated = self.validate_all(origin, transactions).await?;

        let transactions = self.pool.add_transactions(origin, validated.into_values());
        Ok(transactions)
    }

    fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents> {
        self.pool.add_transaction_event_listener(tx_hash)
    }

    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction> {
        self.pool.add_all_transactions_event_listener()
    }

    fn pending_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<TxHash> {
        self.pool.add_pending_listener(kind)
    }

    fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar> {
        self.pool.add_blob_sidecar_listener()
    }

    fn new_transactions_listener_for(
        &self,
        kind: TransactionListenerKind,
    ) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        self.pool.add_new_transaction_listener(kind)
    }

    fn pooled_transaction_hashes(&self) -> Vec<TxHash> {
        self.pool.pooled_transactions_hashes()
    }

    fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash> {
        self.pooled_transaction_hashes().into_iter().take(max).collect()
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pooled_transactions()
    }

    fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pooled_transactions().into_iter().take(max).collect()
    }

    fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<PooledTransactionsElement> {
        self.pool.get_pooled_transaction_elements(tx_hashes, limit)
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(self.pool.best_transactions())
    }

    fn best_transactions_with_base_fee(
        &self,
        base_fee: u64,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        self.pool.best_transactions_with_base_fee(base_fee)
    }

    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.pending_transactions()
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.queued_transactions()
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        self.pool.all_transactions()
    }

    fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.remove_transactions(hashes)
    }

    fn retain_unknown(&self, hashes: &mut Vec<TxHash>) {
        self.pool.retain_unknown(hashes)
    }

    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get(tx_hash)
    }

    fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner().get_all(txs)
    }

    fn on_propagated(&self, txs: PropagatedTransactions) {
        self.inner().on_propagated(txs)
    }

    fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.pool.get_transactions_by_sender(sender)
    }

    fn unique_senders(&self) -> HashSet<Address> {
        self.pool.unique_senders()
    }

    fn get_blob(&self, tx_hash: TxHash) -> Result<Option<BlobTransactionSidecar>, BlobStoreError> {
        self.pool.blob_store().get(tx_hash)
    }

    fn get_all_blobs(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<(TxHash, BlobTransactionSidecar)>, BlobStoreError> {
        self.pool.blob_store().get_all(tx_hashes)
    }

    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<BlobTransactionSidecar>, BlobStoreError> {
        self.pool.blob_store().get_exact(tx_hashes)
    }
}

impl<V: TransactionValidator, T: TransactionOrdering, S> TransactionPoolExt for Pool<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    #[instrument(skip(self), target = "txpool")]
    fn set_block_info(&self, info: BlockInfo) {
        trace!(target: "txpool", "updating pool block info");
        self.pool.set_block_info(info)
    }

    fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_>) {
        self.pool.on_canonical_state_change(update);
    }

    fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        self.pool.update_accounts(accounts);
    }

    fn delete_blob(&self, tx: TxHash) {
        self.pool.delete_blob(tx)
    }

    fn delete_blobs(&self, txs: Vec<TxHash>) {
        self.pool.delete_blobs(txs)
    }
}

impl<V, T: TransactionOrdering, S> Clone for Pool<V, T, S> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}
