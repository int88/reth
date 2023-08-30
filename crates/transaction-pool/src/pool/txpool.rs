//! The internal transaction pool implementation.
//! 内部的transaction pool的实现
use crate::{
    config::TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
    error::{InvalidPoolTransactionError, PoolError},
    identifier::{SenderId, TransactionId},
    metrics::TxPoolMetrics,
    pool::{
        best::BestTransactions,
        parked::{BasefeeOrd, ParkedPool, QueuedOrd},
        pending::PendingPool,
        state::{SubPool, TxState},
        update::{Destination, PoolUpdate},
        AddedPendingTransaction, AddedTransaction, OnNewCanonicalStateOutcome,
    },
    traits::{BlockInfo, PoolSize},
    PoolConfig, PoolResult, PoolTransaction, TransactionOrdering, ValidPoolTransaction, U256,
};
use fnv::FnvHashMap;
use reth_primitives::{
    constants::{ETHEREUM_BLOCK_GAS_LIMIT, MIN_PROTOCOL_BASE_FEE},
    Address, TxHash, H256,
};
use std::{
    cmp::Ordering,
    collections::{btree_map::Entry, hash_map, BTreeMap, HashMap, HashSet},
    fmt,
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};

/// A pool that manages transactions.
/// 一个pool管理transactions
///
/// This pool maintains the state of all transactions and stores them accordingly.
/// 这个pool维护所有transactions的state并且对应地保存它们

#[cfg_attr(doc, aquamarine::aquamarine)]
/// ```mermaid
/// graph TB
///   subgraph TxPool
///     direction TB
///     pool[(All Transactions)]
///     subgraph Subpools
///         direction TB
///         B3[(Queued)]
///         B1[(Pending)]
///         B2[(Basefee)]
///     end
///   end
///   discard([discard])
///   production([Block Production])
///   new([New Block])
///   A[Incoming Tx] --> B[Validation] -->|insert| pool
///   pool --> |if ready| B1
///   pool --> |if ready + basfee too low| B2
///   pool --> |nonce gap or lack of funds| B3
///   pool --> |update| pool
///   B1 --> |best| production
///   B2 --> |worst| discard
///   B3 --> |worst| discard
///   B1 --> |increased fee| B2
///   B2 --> |decreased fee| B1
///   B3 --> |promote| B1
///   B3 -->  |promote| B2
///   new -->  |apply state changes| pool
/// ```
pub struct TxPool<T: TransactionOrdering> {
    /// Contains the currently known information about the senders.
    /// 包含当前已知的关于senders的信息
    sender_info: FnvHashMap<SenderId, SenderInfo>,
    /// pending subpool
    ///
    /// Holds transactions that are ready to be executed on the current state.
    /// 维护已经准备好在当前状态执行的transactions
    pending_pool: PendingPool<T>,
    /// Pool settings to enforce limits etc.
    /// Pool的设置来执行limits
    config: PoolConfig,
    /// queued subpool
    ///
    /// Holds all parked transactions that depend on external changes from the sender:
    /// 维护所有parked txs，依赖来自sender的外部变更
    ///
    ///    - blocked by missing ancestor transaction (has nonce gaps)
    ///    - 被缺失的ancestor txs阻塞（有nonce gap）
    ///    - sender lacks funds to pay for this transaction.
    ///    - sender缺乏fund来支付tx
    queued_pool: ParkedPool<QueuedOrd<T::Transaction>>,
    /// base fee subpool
    ///
    /// Holds all parked transactions that currently violate the dynamic fee requirement but could
    /// be moved to pending if the base fee changes in their favor (decreases) in future blocks.
    /// 维护所有parked transactions，当前违背了dynamic fee
    /// requirement，但是可以被移到pending，如果base fee在未来的blocks 发生了改变
    basefee_pool: ParkedPool<BasefeeOrd<T::Transaction>>,
    /// All transactions in the pool.
    /// pool中的所有transactions
    all_transactions: AllTransactions<T::Transaction>,
    /// Transaction pool metrics
    metrics: TxPoolMetrics,
}

// === impl TxPool ===

impl<T: TransactionOrdering> TxPool<T> {
    /// Create a new graph pool instance.
    /// 创建一个新的graph pool的实例
    pub(crate) fn new(ordering: T, config: PoolConfig) -> Self {
        Self {
            sender_info: Default::default(),
            pending_pool: PendingPool::new(ordering),
            queued_pool: Default::default(),
            basefee_pool: Default::default(),
            all_transactions: AllTransactions::new(config.max_account_slots),
            config,
            metrics: Default::default(),
        }
    }

    /// Returns access to the [`AllTransactions`] container.
    /// 返回访问[`AllTransactions`]的container
    pub(crate) fn all(&self) -> &AllTransactions<T::Transaction> {
        &self.all_transactions
    }

    /// Returns all senders in the pool
    /// 返回pool中所有的senders
    pub(crate) fn unique_senders(&self) -> HashSet<Address> {
        self.all_transactions.txs.values().map(|tx| tx.transaction.sender()).collect()
    }

    /// Returns stats about the size of pool.
    /// 返回size of pool的数据
    pub(crate) fn size(&self) -> PoolSize {
        PoolSize {
            pending: self.pending_pool.len(),
            pending_size: self.pending_pool.size(),
            basefee: self.basefee_pool.len(),
            basefee_size: self.basefee_pool.size(),
            queued: self.queued_pool.len(),
            queued_size: self.queued_pool.size(),
            total: self.all_transactions.len(),
        }
    }

    /// Returns the currently tracked block values
    /// 返回当前追踪的block的values
    pub(crate) fn block_info(&self) -> BlockInfo {
        BlockInfo {
            last_seen_block_hash: self.all_transactions.last_seen_block_hash,
            last_seen_block_number: self.all_transactions.last_seen_block_number,
            pending_basefee: self.all_transactions.pending_basefee,
        }
    }

    /// Updates the tracked basefee
    /// 更新追踪的basefee
    ///
    /// Depending on the change in direction of the basefee, this will promote or demote
    /// transactions from the basefee pool.
    /// 取决于basefee的变更方向，这会提升或者降低来自basefee pool的txs
    fn update_basefee(&mut self, pending_basefee: u64) {
        match pending_basefee.cmp(&self.all_transactions.pending_basefee) {
            Ordering::Equal => {
                // fee unchanged, nothing to update
            }
            Ordering::Greater => {
                // increased base fee: recheck pending pool and remove all that are no longer valid
                // 增加base fee：重新检查pending pool并且移除不再合法的txs
                let removed = self.pending_pool.update_base_fee(pending_basefee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
            Ordering::Less => {
                // decreased base fee: recheck basefee pool and promote all that are now valid
                // 降低base fee：重新检查basefee pool并且提升他们如果合法的话
                let removed = self.basefee_pool.enforce_basefee(pending_basefee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
        }
    }

    /// Sets the current block info for the pool.
    ///
    /// This will also apply updates to the pool based on the new base fee
    /// 这也会更新pool，基于新的base fee
    pub(crate) fn set_block_info(&mut self, info: BlockInfo) {
        let BlockInfo { last_seen_block_hash, last_seen_block_number, pending_basefee } = info;
        self.all_transactions.last_seen_block_hash = last_seen_block_hash;
        self.all_transactions.last_seen_block_number = last_seen_block_number;
        self.all_transactions.pending_basefee = pending_basefee;
        self.update_basefee(pending_basefee)
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    /// 返回一个iterator，产生txs，准备好被包含进block
    pub(crate) fn best_transactions(&self) -> BestTransactions<T> {
        self.pending_pool.best()
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the given base fee.
    /// 返回一个iterator，产生txs准备好被包含进block，用给定的base fee
    pub(crate) fn best_transactions_with_base_fee(
        &self,
        basefee: u64,
    ) -> Box<dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T::Transaction>>>>
    {
        match basefee.cmp(&self.all_transactions.pending_basefee) {
            Ordering::Equal => {
                // fee unchanged, nothing to shift
                Box::new(self.best_transactions())
            }
            Ordering::Greater => {
                // base fee increased, we only need to enforces this on the pending pool
                Box::new(self.pending_pool.best_with_basefee(basefee))
            }
            Ordering::Less => {
                // base fee decreased, we need to move transactions from the basefee pool to the
                // pending pool
                let unlocked = self.basefee_pool.satisfy_base_fee_transactions(basefee);
                Box::new(
                    self.pending_pool
                        .best_with_unlocked(unlocked, self.all_transactions.pending_basefee),
                )
            }
        }
    }

    /// Returns all transactions from the pending sub-pool
    /// 返回所有的txs，从pending sub-pool
    pub(crate) fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pending_pool.all().collect()
    }

    /// Returns all transactions from parked pools
    /// 从parked pools返回所有的txs
    pub(crate) fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        // basefee_pool加上queued_pool
        let mut queued = self.basefee_pool.all().collect::<Vec<_>>();
        queued.extend(self.queued_pool.all());
        queued
    }

    /// Returns `true` if the transaction with the given hash is already included in this pool.
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.all_transactions.contains(tx_hash)
    }

    /// Returns the transaction for the given hash.
    pub(crate) fn get(
        &self,
        tx_hash: &TxHash,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.all_transactions.by_hash.get(tx_hash).cloned()
    }

    /// Returns transactions for the multiple given hashes, if they exist.
    /// 返回txs，对于多个给定的hashes，如果他们存在的话
    pub(crate) fn get_all(
        &self,
        txs: Vec<TxHash>,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        txs.into_iter().filter_map(|tx| self.get(&tx))
    }

    /// Returns all transactions sent from the given sender.
    /// 返回给定sender的所有txs
    pub(crate) fn get_transactions_by_sender(
        &self,
        sender: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        // 收集senders
        self.all_transactions.txs_iter(sender).map(|(_, tx)| Arc::clone(&tx.transaction)).collect()
    }

    /// Updates the transactions for the changed senders.
    /// 对于changed senders更新transactions
    pub(crate) fn update_accounts(
        &mut self,
        changed_senders: HashMap<SenderId, SenderInfo>,
    ) -> UpdateOutcome<T::Transaction> {
        // track changed accounts
        // 追踪changed accounts
        self.sender_info.extend(changed_senders.clone());
        // Apply the state changes to the total set of transactions which triggers sub-pool updates.
        // 应用state changes到total set of txs，会触发sub-pool的更新
        let updates = self.all_transactions.update(changed_senders);
        // Process the sub-pool updates
        // 处理sub-pool的更新
        let update = self.process_updates(updates);
        // update the metrics after the update
        // 更新metrics，在更新之后
        self.update_size_metrics();
        update
    }

    /// Updates the entire pool after a new block was mined.
    /// 更新整个pool，在一个新的block被mined之后
    ///
    /// This removes all mined transactions, updates according to the new base fee and rechecks
    /// sender allowance.
    /// 这移除所有mined txs，根据新的base fee进行更新并且重新检查sender allowance
    pub(crate) fn on_canonical_state_change(
        &mut self,
        block_info: BlockInfo,
        mined_transactions: Vec<TxHash>,
        changed_senders: HashMap<SenderId, SenderInfo>,
    ) -> OnNewCanonicalStateOutcome<T::Transaction> {
        // update block info
        // 更新block info
        let block_hash = block_info.last_seen_block_hash;
        // 设置block信息
        self.all_transactions.set_block_info(block_info);

        // Remove all transaction that were included in the block
        // 移除所有包含在这个block内的tx
        for tx_hash in mined_transactions.iter() {
            if self.prune_transaction_by_hash(tx_hash).is_some() {
                // Update removed transactions metric
                // 更新移除的txs的metrics
                self.metrics.removed_transactions.increment(1);
            }
        }

        let UpdateOutcome { promoted, discarded } = self.update_accounts(changed_senders);

        self.metrics.performed_state_updates.increment(1);

        OnNewCanonicalStateOutcome { block_hash, mined: mined_transactions, promoted, discarded }
    }

    /// Update sub-pools size metrics.
    /// 更新sub-pool的zie metrics
    pub(crate) fn update_size_metrics(&mut self) {
        let stats = self.size();
        self.metrics.pending_pool_transactions.set(stats.pending as f64);
        self.metrics.pending_pool_size_bytes.set(stats.pending_size as f64);
        self.metrics.basefee_pool_transactions.set(stats.basefee as f64);
        self.metrics.basefee_pool_size_bytes.set(stats.basefee_size as f64);
        self.metrics.queued_pool_transactions.set(stats.queued as f64);
        self.metrics.queued_pool_size_bytes.set(stats.queued_size as f64);
        self.metrics.total_transactions.set(stats.total as f64);
    }

    /// Adds the transaction into the pool.
    /// 添加tx到pool
    ///
    /// This pool consists of two three-pools: `Queued`, `Pending` and `BaseFee`.
    ///
    /// The `Queued` pool contains transactions with gaps in its dependency tree: It requires
    /// additional transactions that are note yet present in the pool. And transactions that the
    /// sender can not afford with the current balance.
    /// `Queued` pool包含txs，在dependency
    /// tree中有gaps，它需要还没在pool中的额外的txs，以及txs，sender不能承担当前的txs
    ///
    /// The `Pending` pool contains all transactions that have no nonce gaps, and can be afforded by
    /// the sender. It only contains transactions that are ready to be included in the pending
    /// block. The pending pool contains all transactions that could be listed currently, but not
    /// necessarily independently. However, this pool never contains transactions with nonce gaps. A
    /// transaction is considered `ready` when it has the lowest nonce of all transactions from the
    /// same sender. Which is equals to the chain nonce of the sender in the pending pool.
    /// 一个tx被认为是`ready`，当它有来自同一个sender并且有最小的nonce，它和pending
    /// pool中的sender的chain nonce相等
    ///
    /// The `BaseFee` pool contains transactions that currently can't satisfy the dynamic fee
    /// requirement. With EIP-1559, transactions can become executable or not without any changes to
    /// the sender's balance or nonce and instead their `feeCap` determines whether the
    /// transaction is _currently_ (on the current state) ready or needs to be parked until the
    /// `feeCap` satisfies the block's `baseFee`.
    /// `BaseFee` pool包含txs，当前不满足dynamic
    /// fee要求，基于EIP-1559,txs会变成可执行，对于sender的balance或者nonce没有任何改变
    /// 而是`feeCap`决定是否tx当前准备好或者需要被parked，直到`feeCap`满足block的`baseFee`
    pub(crate) fn add_transaction(
        &mut self,
        tx: ValidPoolTransaction<T::Transaction>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> PoolResult<AddedTransaction<T::Transaction>> {
        if self.contains(tx.hash()) {
            return Err(PoolError::AlreadyImported(*tx.hash()))
        }

        // Update sender info with balance and nonce
        // 用balance和nonce更新sender信息
        self.sender_info
            .entry(tx.sender_id())
            .or_default()
            .update(on_chain_nonce, on_chain_balance);

        match self.all_transactions.insert_tx(tx, on_chain_balance, on_chain_nonce) {
            Ok(InsertOk { transaction, move_to, replaced_tx, updates, .. }) => {
                self.add_new_transaction(transaction.clone(), replaced_tx.clone(), move_to);
                // Update inserted transactions metric
                // 更新插入的transactions的metric
                self.metrics.inserted_transactions.increment(1);
                let UpdateOutcome { promoted, discarded } = self.process_updates(updates);

                // This transaction was moved to the pending pool.
                // 这个transaction移动到pending pool
                let replaced = replaced_tx.map(|(tx, _)| tx);
                let res = if move_to.is_pending() {
                    AddedTransaction::Pending(AddedPendingTransaction {
                        transaction,
                        promoted,
                        discarded,
                        replaced,
                    })
                } else {
                    AddedTransaction::Parked { transaction, subpool: move_to, replaced }
                };

                Ok(res)
            }
            Err(e) => {
                // Update invalid transactions metric
                // 更新invalid transactions的metric
                self.metrics.invalid_transactions.increment(1);
                match e {
                    InsertErr::Underpriced { existing, transaction: _ } => {
                        Err(PoolError::ReplacementUnderpriced(existing))
                    }
                    InsertErr::FeeCapBelowMinimumProtocolFeeCap { transaction, fee_cap } => Err(
                        PoolError::FeeCapBelowMinimumProtocolFeeCap(*transaction.hash(), fee_cap),
                    ),
                    InsertErr::ExceededSenderTransactionsCapacity { transaction } => {
                        Err(PoolError::SpammerExceededCapacity(
                            transaction.sender(),
                            *transaction.hash(),
                        ))
                    }
                    InsertErr::TxGasLimitMoreThanAvailableBlockGas {
                        transaction,
                        block_gas_limit,
                        tx_gas_limit,
                    } => Err(PoolError::InvalidTransaction(
                        *transaction.hash(),
                        InvalidPoolTransactionError::ExceedsGasLimit(block_gas_limit, tx_gas_limit),
                    )),
                }
            }
        }
    }

    /// Maintenance task to apply a series of updates.
    /// 维护任务，用于应用一系列的updates
    ///
    /// This will move/discard the given transaction according to the `PoolUpdate`
    /// 这会移动/丢弃给定的tx，根据`PoolUpdate`
    fn process_updates(&mut self, updates: Vec<PoolUpdate>) -> UpdateOutcome<T::Transaction> {
        let mut outcome = UpdateOutcome::default();
        for update in updates {
            let PoolUpdate { id, hash, current, destination } = update;
            match destination {
                Destination::Discard => {
                    // remove the transaction from the pool and subpool
                    // 从pool以及subpool中移除tx
                    self.prune_transaction_by_hash(&hash);
                    outcome.discarded.push(hash);
                    self.metrics.removed_transactions.increment(1);
                }
                Destination::Pool(move_to) => {
                    // destination必须不同
                    debug_assert!(!move_to.eq(&current), "destination must be different");
                    // 移动tx
                    let moved = self.move_transaction(current, move_to, &id);
                    // 移动到了pending
                    if matches!(move_to, SubPool::Pending) {
                        if let Some(tx) = moved {
                            outcome.promoted.push(tx);
                        }
                    }
                }
            }
        }
        outcome
    }

    /// Moves a transaction from one sub pool to another.
    /// 从一个tx从sub pool移动到另一个
    ///
    /// This will remove the given transaction from one sub-pool and insert it into the other
    /// sub-pool.
    /// 这会移除给定的tx，从一个sub-pool并且插入到另一个sub-pool
    fn move_transaction(
        &mut self,
        from: SubPool,
        to: SubPool,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let tx = self.remove_from_subpool(from, id)?;
        self.add_transaction_to_subpool(to, tx.clone());
        Some(tx)
    }

    /// Removes and returns all matching transactions from the pool.
    /// 移除并且返回所有匹配的txs，从poola
    ///
    /// Note: this does not advance any descendants of the removed transactions and does not apply
    /// any additional updates.
    /// 注意：这不会移动任何removed txs的descendants并且不会应用任何额外的updates
    pub(crate) fn remove_transactions(
        &mut self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        hashes.into_iter().filter_map(|hash| self.remove_transaction_by_hash(&hash)).collect()
    }

    /// Remove the transaction from the entire pool.
    /// 从整个pool移除tx
    ///
    /// This includes the total set of transaction and the subpool it currently resides in.
    /// 这包含完整的tx集合以及它当前处于的subpool
    fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction(id)?;
        self.remove_from_subpool(pool, tx.id())
    }

    /// Remove the transaction from the entire pool via its hash.
    /// 通过hash从整个pool移除tx
    ///
    /// This includes the total set of transactions and the subpool it currently resides in.
    /// 这包含所有的set of txs以及它们当前所处的subpool
    fn remove_transaction_by_hash(
        &mut self,
        tx_hash: &H256,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction_by_hash(tx_hash)?;
        self.remove_from_subpool(pool, tx.id())
    }

    /// This removes the transaction from the pool and advances any descendant state inside the
    /// subpool.
    /// 从pool中移除tx并且将任何subpool内的descendant的state状态
    ///
    /// This is intended to be used when a transaction is included in a block,
    /// 这在一个tx被包含进一个block的时候使用
    /// [Self::on_canonical_state_change]
    fn prune_transaction_by_hash(
        &mut self,
        tx_hash: &H256,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction_by_hash(tx_hash)?;
        self.prune_from_subpool(pool, tx.id())
    }

    /// Removes the transaction from the given pool.
    /// 从给定pool移除tx
    ///
    /// Caution: this only removes the tx from the sub-pool and not from the pool itself
    /// 注意：这只从sub-pool中移除tx但是不是pool
    fn remove_from_subpool(
        &mut self,
        pool: SubPool,
        tx: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        match pool {
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::Pending => self.pending_pool.remove_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
        }
    }

    /// Removes the transaction from the given pool and advance sub-pool internal state, with the
    /// expectation that the given transaction is included in a block.
    /// 从给定的pool移除tx并且推进sub-pool的internal state，期望给定的tx已经包含在一个block内
    fn prune_from_subpool(
        &mut self,
        pool: SubPool,
        tx: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        match pool {
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::Pending => self.pending_pool.prune_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
        }
    }

    /// Removes _only_ the descendants of the given transaction from the entire pool.
    /// 只从整个pool中移除给定tx的后代
    ///
    /// All removed transactions are added to the `removed` vec.
    /// 所有被移除的txs都被添加到`removed` vec
    fn remove_descendants(
        &mut self,
        tx: &TransactionId,
        removed: &mut Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        let mut id = *tx;

        // this will essentially pop _all_ descendant transactions one by one
        // 这会本质上弹出所有的后代txs，one by one
        loop {
            let descendant =
                self.all_transactions.descendant_txs_exclusive(&id).map(|(id, _)| *id).next();
            if let Some(descendant) = descendant {
                // 移除tx
                if let Some(tx) = self.remove_transaction(&descendant) {
                    removed.push(tx)
                }
                id = descendant;
            } else {
                return
            }
        }
    }

    /// Inserts the transaction into the given sub-pool.
    /// 添加tx到给定的sub-pool
    fn add_transaction_to_subpool(
        &mut self,
        pool: SubPool,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
    ) {
        match pool {
            SubPool::Queued => {
                self.queued_pool.add_transaction(tx);
            }
            SubPool::Pending => {
                self.pending_pool.add_transaction(tx, self.all_transactions.pending_basefee);
            }
            SubPool::BaseFee => {
                self.basefee_pool.add_transaction(tx);
            }
        }
    }

    /// Inserts the transaction into the given sub-pool.
    /// 插入tx到给定的sub-pool
    /// Optionally, removes the replacement transaction.
    /// 可选地，移除replacement tx
    fn add_new_transaction(
        &mut self,
        transaction: Arc<ValidPoolTransaction<T::Transaction>>,
        replaced: Option<(Arc<ValidPoolTransaction<T::Transaction>>, SubPool)>,
        pool: SubPool,
    ) {
        if let Some((replaced, replaced_pool)) = replaced {
            // Remove the replaced transaction
            // 移除replaced tx
            self.remove_from_subpool(replaced_pool, replaced.id());
        }

        // 添加tx到sub pool
        self.add_transaction_to_subpool(pool, transaction)
    }

    /// Ensures that the transactions in the sub-pools are within the given bounds.
    /// 确保sub-pools中的transactions在给定的范围内
    ///
    /// If the current size exceeds the given bounds, the worst transactions are evicted from the
    /// pool and returned.
    /// 如果当前的size超过了给定的bounds，则最差的transactions会被从pool中驱逐并且返回
    pub(crate) fn discard_worst(&mut self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();

        // Helper macro that discards the worst transactions for the pools
        // Helper宏用于丢弃pools后者那个最差的transactions
        macro_rules! discard_worst {
            ($this:ident, $removed:ident,  [$($limit:ident => $pool:ident),*]  ) => {
                $ (
                while $this
                        .config
                        .$limit
                        .is_exceeded($this.$pool.len(), $this.$pool.size())
                    {
                        if let Some(tx) = $this.$pool.pop_worst() {
                            let id = tx.transaction_id;
                            removed.push(tx);
                            $this.remove_descendants(&id, &mut $removed);
                        }
                    }

                )*
            };
        }

        discard_worst!(
            self, removed, [
                pending_limit  => pending_pool,
                basefee_limit  => basefee_pool,
                queued_limit  => queued_pool
            ]
        );

        removed
    }

    /// Number of transactions in the entire pool
    /// 在整个pool中所有txs的数目
    pub(crate) fn len(&self) -> usize {
        self.all_transactions.len()
    }

    /// Whether the pool is empty
    /// pool是否为空
    pub(crate) fn is_empty(&self) -> bool {
        self.all_transactions.is_empty()
    }
}

// Additional test impls
// 额外的test实现
#[cfg(any(test, feature = "test-utils"))]
#[allow(missing_docs)]
impl<T: TransactionOrdering> TxPool<T> {
    pub(crate) fn pending(&self) -> &PendingPool<T> {
        &self.pending_pool
    }

    pub(crate) fn base_fee(&self) -> &ParkedPool<BasefeeOrd<T::Transaction>> {
        &self.basefee_pool
    }

    pub(crate) fn queued(&self) -> &ParkedPool<QueuedOrd<T::Transaction>> {
        &self.queued_pool
    }
}

impl<T: TransactionOrdering> fmt::Debug for TxPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxPool").field("config", &self.config).finish_non_exhaustive()
    }
}

/// Container for _all_ transaction in the pool.
/// 包含pool中的所有transaction
///
/// This is the sole entrypoint that's guarding all sub-pools, all sub-pool actions are always
/// derived from this set. Updates returned from this type must be applied to the sub-pools.
/// 这是维护所有sub-pools的唯一入口，所有的sub-pool
/// actions总是从这个集合衍生而来，这个类型返回的Updates必须应用到sub-pools
pub(crate) struct AllTransactions<T: PoolTransaction> {
    /// Minimum base fee required by the protocol.
    /// 这个pool需要的最小的base fee
    ///
    /// Transactions with a lower base fee will never be included by the chain
    /// 有着更小的base fee的txs不会被加入到chain中
    minimal_protocol_basefee: u64,
    /// The max gas limit of the block
    /// block最大的gas limit
    block_gas_limit: u64,
    /// Max number of executable transaction slots guaranteed per account
    /// 每个account保证的最多的可执行的transaction的slots
    max_account_slots: usize,
    /// _All_ transactions identified by their hash.
    /// 所有通过它们的hash标识的txs
    by_hash: HashMap<TxHash, Arc<ValidPoolTransaction<T>>>,
    /// _All_ transaction in the pool sorted by their sender and nonce pair.
    /// pool中所有的transactions，通过sender以及nonce pair排序
    txs: BTreeMap<TransactionId, PoolInternalTransaction<T>>,
    /// Tracks the number of transactions by sender that are currently in the pool.
    /// 追踪sender的transactions的数目，当前在pool中
    tx_counter: FnvHashMap<SenderId, usize>,
    /// The current block number the pool keeps track of.
    /// pool当前追踪的block number
    last_seen_block_number: u64,
    /// The current block hash the pool keeps track of.
    /// pool当前追踪的block hash
    last_seen_block_hash: H256,
    /// Expected base fee for the pending block.
    /// 对于pending block期望的base fee
    pending_basefee: u64,
}

impl<T: PoolTransaction> AllTransactions<T> {
    /// Create a new instance
    fn new(max_account_slots: usize) -> Self {
        Self { max_account_slots, ..Default::default() }
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    /// 返回一个iterator，遍历所有唯一的hashes，在pool中
    #[allow(unused)]
    pub(crate) fn hashes_iter(&self) -> impl Iterator<Item = TxHash> + '_ {
        self.by_hash.keys().copied()
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    pub(crate) fn transactions_iter(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T>>> + '_ {
        self.by_hash.values().cloned()
    }

    /// Returns if the transaction for the given hash is already included in this pool
    /// 返回tx，对于给定的hash，如果已经在pool中
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.by_hash.contains_key(tx_hash)
    }

    /// Returns the internal transaction with additional metadata
    /// 返回内部的tx，有着额外的metadata
    #[cfg(test)]
    pub(crate) fn get(&self, id: &TransactionId) -> Option<&PoolInternalTransaction<T>> {
        self.txs.get(id)
    }

    /// Increments the transaction counter for the sender
    /// 增加sender的tx counter
    pub(crate) fn tx_inc(&mut self, sender: SenderId) {
        // 添加count
        let count = self.tx_counter.entry(sender).or_default();
        *count += 1;
    }

    /// Decrements the transaction counter for the sender
    /// 减小sender的tx counter
    pub(crate) fn tx_decr(&mut self, sender: SenderId) {
        if let hash_map::Entry::Occupied(mut entry) = self.tx_counter.entry(sender) {
            let count = entry.get_mut();
            if *count == 1 {
                entry.remove();
                return
            }
            *count -= 1;
        }
    }

    /// Updates the block specific info
    /// 更新block特定的信息
    fn set_block_info(&mut self, block_info: BlockInfo) {
        let BlockInfo { last_seen_block_hash, last_seen_block_number, pending_basefee } =
            block_info;
        self.last_seen_block_number = last_seen_block_number;
        self.last_seen_block_hash = last_seen_block_hash;
        self.pending_basefee = pending_basefee;
    }

    /// Rechecks all transactions in the pool against the changes.
    /// 重新检查pool中的txs，对于changes
    ///
    /// Possible changes are:
    ///
    /// For all transactions:
    ///   - decreased basefee: promotes from `basefee` to `pending` sub-pool.
    ///   - increased basefee: demotes from `pending` to `basefee` sub-pool.
    /// Individually:
    ///   - decreased sender allowance: demote from (`basefee`|`pending`) to `queued`.
    ///   - increased sender allowance: promote from `queued` to
    ///       - `pending` if basefee condition is met.
    ///       - `basefee` if basefee condition is _not_ met.
    ///       - `basefee`如果basefee的条件不满足
    ///
    /// Additionally, this will also update the `cumulative_gas_used` for transactions of a sender
    /// that got transaction included in the block.
    /// 另外，这会更新`cumulative_gas_used`对于一个sender的tx，如果tx被包含进block
    pub(crate) fn update(
        &mut self,
        changed_accounts: HashMap<SenderId, SenderInfo>,
    ) -> Vec<PoolUpdate> {
        // pre-allocate a few updates
        // 提前申请一些updates
        let mut updates = Vec::with_capacity(64);

        // 遍历txs
        let mut iter = self.txs.iter_mut().peekable();

        // Loop over all individual senders and update all affected transactions.
        // 遍历所有单个的senders并且更新受影响的txs
        // One sender may have up to `max_account_slots` transactions here, which means, worst case
        // `max_accounts_slots` need to be updated, for example if the first transaction is blocked
        // due to too low base fee.
        // 一个sender可能有`max_account_slots`个tx，这意味着，最坏的情况，
        // `max_accounts_slots`需要更新，例如，第一个 tx被阻塞，因为太低的base fee
        // However, we don't have to necessarily check every transaction of a sender. If no updates
        // are possible (nonce gap) then we can skip to the next sender.
        // 然而，我们不是一定要检查一个sender的所有tx，如果没有更新的可能（nonce
        // gap），那么我们可以跳到下一个sender

        // The `unique_sender` loop will process the first transaction of all senders, update its
        // state and internally update all consecutive transactions
        // `unique_sender`循环会处理所有senders的第一个tx，更新它的状态并且内部更新所有连续的txs
        'transactions: while let Some((id, tx)) = iter.next() {
            macro_rules! next_sender {
                // 切换到下一个sender
                ($iter:ident) => {
                    'this: while let Some((peek, _)) = iter.peek() {
                        if peek.sender != id.sender {
                            break 'this
                        }
                        iter.next();
                    }
                };
            }
            // tracks the balance if the sender was changed in the block
            // 追踪balance，如果sender在block中改变
            let mut changed_balance = None;

            // check if this is a changed account
            // 检查是否这是一个changed account
            if let Some(info) = changed_accounts.get(&id.sender) {
                // discard all transactions with a nonce lower than the current state nonce
                // 丢弃所有的txs，nonce小于当前的state nonce
                if id.nonce < info.state_nonce {
                    updates.push(PoolUpdate {
                        id: *tx.transaction.id(),
                        hash: *tx.transaction.hash(),
                        current: tx.subpool,
                        destination: Destination::Discard,
                    });
                    continue 'transactions
                }

                let ancestor = TransactionId::ancestor(id.nonce, info.state_nonce, id.sender);
                // If there's no ancestor then this is the next transaction.
                // 如果没有ancestor，则这是下一个tx
                if ancestor.is_none() {
                    tx.state.insert(TxState::NO_NONCE_GAPS);
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                    tx.cumulative_cost = U256::ZERO;
                    if tx.transaction.cost() > info.balance {
                        // sender lacks sufficient funds to pay for this transaction
                        // 如果sender缺乏足够的funds来支付这个tx
                        tx.state.remove(TxState::ENOUGH_BALANCE);
                    } else {
                        tx.state.insert(TxState::ENOUGH_BALANCE);
                    }
                }

                changed_balance = Some(info.balance);
            }

            // If there's a nonce gap, we can shortcircuit, because there's nothing to update yet.
            // 如果有一个nonce gap，我们可以短路，因为没有什么需要更新了
            if tx.state.has_nonce_gap() {
                next_sender!(iter);
                continue 'transactions
            }

            // Since this is the first transaction of the sender, it has no parked ancestors
            // 因为这是这个sender的第一个tx，它没有parked ancestors
            tx.state.insert(TxState::NO_PARKED_ANCESTORS);

            // Update the first transaction of this sender.
            // 更新这个sender的第一个tx
            Self::update_tx_base_fee(self.pending_basefee, tx);
            // Track if the transaction's sub-pool changed.
            // 追踪这个tx的sub-pool是否改变
            Self::record_subpool_update(&mut updates, tx);

            // Track blocking transactions.
            // 追踪阻塞的txs
            let mut has_parked_ancestor = !tx.state.is_pending();

            // 累计的cost
            let mut cumulative_cost = tx.next_cumulative_cost();

            // Update all consecutive transaction of this sender
            // 更新这个sender所有连续的tx
            while let Some((peek, ref mut tx)) = iter.peek_mut() {
                if peek.sender != id.sender {
                    // Found the next sender
                    // 找到下一个sender
                    continue 'transactions
                }

                // can short circuit
                // 可以短路
                if tx.state.has_nonce_gap() {
                    next_sender!(iter);
                    continue 'transactions
                }

                // update cumulative cost
                // 更新cumulative cost
                tx.cumulative_cost = cumulative_cost;
                // Update for next transaction
                // 更新下一个tx
                cumulative_cost = tx.next_cumulative_cost();

                // If the account changed in the block, check the balance.
                // 如果account在这个block改变了，检查balance
                if let Some(changed_balance) = changed_balance {
                    if cumulative_cost > changed_balance {
                        // sender lacks sufficient funds to pay for this transaction
                        // sendr缺乏足够的funds来支付这个tx
                        tx.state.remove(TxState::ENOUGH_BALANCE);
                    } else {
                        tx.state.insert(TxState::ENOUGH_BALANCE);
                    }
                }

                // Update ancestor condition.
                // 更新ancestor的情况
                if has_parked_ancestor {
                    tx.state.remove(TxState::NO_PARKED_ANCESTORS);
                } else {
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                }
                // 如果tx不是出于pending状态
                has_parked_ancestor = !tx.state.is_pending();

                // Update and record sub-pool changes.
                // 更新并且记录sub-pool的改变
                Self::update_tx_base_fee(self.pending_basefee, tx);
                Self::record_subpool_update(&mut updates, tx);

                // Advance iterator
                // 移动iterator
                iter.next();
            }
        }

        updates
    }

    /// This will update the transaction's `subpool` based on its state.
    /// 这会更新tx的`subpool`基于它的state
    ///
    /// If the sub-pool derived from the state differs from the current pool, it will record a
    /// `PoolUpdate` for this transaction to move it to the new sub-pool.
    /// 如果sub-pool衍生自不同于当前的pool的state，它会记录`PoolUpdate`对于这个tx，
    /// 将它移动到新的sub-pool
    fn record_subpool_update(updates: &mut Vec<PoolUpdate>, tx: &mut PoolInternalTransaction<T>) {
        let current_pool = tx.subpool;
        tx.subpool = tx.state.into();
        if current_pool != tx.subpool {
            updates.push(PoolUpdate {
                id: *tx.transaction.id(),
                hash: *tx.transaction.hash(),
                current: current_pool,
                destination: Destination::Pool(tx.subpool),
            })
        }
    }

    /// Rechecks the transaction's dynamic fee condition.
    /// 重新检查tx的dynamic fee的情况
    fn update_tx_base_fee(pending_block_base_fee: u64, tx: &mut PoolInternalTransaction<T>) {
        // Recheck dynamic fee condition.
        // 重新检查dynamic fee的情况
        match tx.transaction.max_fee_per_gas().cmp(&(pending_block_base_fee as u128)) {
            Ordering::Greater | Ordering::Equal => {
                tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
            }
            Ordering::Less => {
                tx.state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
            }
        }
    }

    /// Returns an iterator over all transactions for the given sender, starting with the lowest
    /// nonce
    /// 返回一个iterator用于遍历给定sender的所有的txs，从最小的nonce开始
    pub(crate) fn txs_iter(
        &self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&TransactionId, &PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns a mutable iterator over all transactions for the given sender, starting with the
    /// lowest nonce
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn txs_iter_mut(
        &mut self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&TransactionId, &mut PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range_mut((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    /// 返回所有的txs，在给定的id之后，但是有着同样的sender
    ///
    /// NOTE: The range is _exclusive_
    /// 注意：这个范围是排他性的
    pub(crate) fn descendant_txs_exclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + '_ {
        self.txs.range((Excluded(id), Unbounded)).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn descendant_txs<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + '_ {
        self.txs.range(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all mutable transactions that _follow_ after the given id but have the same sender.
    /// 返回所有mutable txs，跟随给定的id，但是有着同样的sender
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    /// 注意：range是包含的：如果tx属于`id`，它的字段变为第一个值
    pub(crate) fn descendant_txs_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + '_ {
        self.txs.range_mut(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Removes a transaction from the set using its hash.
    /// 从集合中移除一个tx，使用它的hash
    pub(crate) fn remove_transaction_by_hash(
        &mut self,
        tx_hash: &H256,
    ) -> Option<(Arc<ValidPoolTransaction<T>>, SubPool)> {
        let tx = self.by_hash.remove(tx_hash)?;
        let internal = self.txs.remove(&tx.transaction_id)?;
        // decrement the counter for the sender.
        // 减小sender的counter
        self.tx_decr(tx.sender_id());
        Some((tx, internal.subpool))
    }

    /// Removes a transaction from the set.
    /// 从集合中移除一个tx
    ///
    /// This will _not_ trigger additional updates, because descendants without nonce gaps are
    /// already in the pending pool, and this transaction will be the first transaction of the
    /// sender in this pool.
    /// 这不会触发额外的updates，因为没有nonce gaps的后来已经处于pending
    /// pool，并且这个tx回事这个sender在pool 中的第一个tx
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<(Arc<ValidPoolTransaction<T>>, SubPool)> {
        let internal = self.txs.remove(id)?;

        // decrement the counter for the sender.
        // 降低sender的counter
        self.tx_decr(internal.transaction.sender_id());

        self.by_hash.remove(internal.transaction.hash()).map(|tx| (tx, internal.subpool))
    }

    /// Additional checks for a new transaction.
    /// 对于一个新的tx的额外的检查
    ///
    /// This will enforce all additional rules in the context of this pool, such as:
    /// 这会在这个context的上下文执行所有额外的rules，例如
    ///   - Spam protection: reject new non-local transaction from a sender that exhausted its slot
    ///     capacity.
    ///   - Spam protection: 拒绝新的non-local的tx，如果超过了slot capacity
    ///   - Gas limit: reject transactions if they exceed a block's maximum gas.
    ///   - Gas limit：拒绝txs，如果他们超过了一个block的maximum gas
    fn ensure_valid(
        &self,
        transaction: ValidPoolTransaction<T>,
    ) -> Result<ValidPoolTransaction<T>, InsertErr<T>> {
        if !transaction.origin.is_local() {
            let current_txs =
                self.tx_counter.get(&transaction.sender_id()).copied().unwrap_or_default();
            if current_txs >= self.max_account_slots {
                return Err(InsertErr::ExceededSenderTransactionsCapacity {
                    transaction: Arc::new(transaction),
                })
            }
        }
        if transaction.gas_limit() > self.block_gas_limit {
            return Err(InsertErr::TxGasLimitMoreThanAvailableBlockGas {
                block_gas_limit: self.block_gas_limit,
                tx_gas_limit: transaction.gas_limit(),
                transaction: Arc::new(transaction),
            })
        }
        Ok(transaction)
    }

    /// Returns true if `transaction_a` is underpriced compared to `transaction_B`.
    fn is_underpriced(
        transaction_a: &ValidPoolTransaction<T>,
        transaction_b: &ValidPoolTransaction<T>,
        price_bump: u128,
    ) -> bool {
        let tx_a_max_priority_fee_per_gas =
            transaction_a.transaction.max_priority_fee_per_gas().unwrap_or(0);
        let tx_b_max_priority_fee_per_gas =
            transaction_b.transaction.max_priority_fee_per_gas().unwrap_or(0);

        transaction_a.max_fee_per_gas() <=
            transaction_b.max_fee_per_gas() * (100 + price_bump) / 100 ||
            (tx_a_max_priority_fee_per_gas <=
                tx_b_max_priority_fee_per_gas * (100 + price_bump) / 100 &&
                tx_a_max_priority_fee_per_gas != 0 &&
                tx_b_max_priority_fee_per_gas != 0)
    }

    /// Inserts a new transaction into the pool.
    ///
    /// If the transaction already exists, it will be replaced if not underpriced.
    /// Returns info to which sub-pool the transaction should be moved.
    /// 返回信息，关于这个transaction应该被移动到哪个sub-pool
    /// Also returns a set of pool updates triggered by this insert, that need to be handled by the
    /// caller.
    /// 同时返回一系列的pool updates，由这个插入触发，需要被caller处理
    ///
    /// These can include:
    ///      - closing nonce gaps of descendant transactions
    ///      - 关闭descendant txs的nonce gap
    ///      - enough balance updates
    ///      - 足够的balance updates
    pub(crate) fn insert_tx(
        &mut self,
        transaction: ValidPoolTransaction<T>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> InsertResult<T> {
        // transaction.nonce()必须大于等于on_chain_nonce
        assert!(on_chain_nonce <= transaction.nonce(), "Invalid transaction");

        let transaction = Arc::new(self.ensure_valid(transaction)?);
        let tx_id = *transaction.id();
        let mut state = TxState::default();
        let mut cumulative_cost = U256::ZERO;
        let mut updates = Vec::new();

        let ancestor =
            TransactionId::ancestor(transaction.transaction.nonce(), on_chain_nonce, tx_id.sender);

        // If there's no ancestor tx then this is the next transaction.
        // 如果没有ancestor tx，那么这是下一个tx
        if ancestor.is_none() {
            state.insert(TxState::NO_NONCE_GAPS);
            state.insert(TxState::NO_PARKED_ANCESTORS);
        }

        // Check dynamic fee
        // 检查动态的fee
        let fee_cap = transaction.max_fee_per_gas();

        if fee_cap < self.minimal_protocol_basefee as u128 {
            return Err(InsertErr::FeeCapBelowMinimumProtocolFeeCap { transaction, fee_cap })
        }
        if fee_cap >= self.pending_basefee as u128 {
            state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
        }

        // Ensure tx does not exceed block gas limit
        // 确保tx没有超过block gas limit
        if transaction.gas_limit() < self.block_gas_limit {
            state.insert(TxState::NOT_TOO_MUCH_GAS);
        }

        let mut replaced_tx = None;

        // 构建pool tx
        let pool_tx = PoolInternalTransaction {
            transaction: transaction.clone(),
            subpool: state.into(),
            state,
            cumulative_cost,
        };

        // try to insert the transaction
        // 试着插入transaction
        match self.txs.entry(*transaction.id()) {
            Entry::Vacant(entry) => {
                // Insert the transaction in both maps
                // 同时在两个maps镜像插入
                self.by_hash.insert(*pool_tx.transaction.hash(), pool_tx.transaction.clone());
                entry.insert(pool_tx);
            }
            Entry::Occupied(mut entry) => {
                // Transaction already exists
                // Ensure the new transaction is not underpriced
                // tx已经存在，确保新的tx没有underpriced

                if Self::is_underpriced(
                    transaction.as_ref(),
                    entry.get().transaction.as_ref(),
                    PoolConfig::default().price_bump,
                ) {
                    // 如果是underpriced，直接返回
                    return Err(InsertErr::Underpriced {
                        transaction: pool_tx.transaction,
                        existing: *entry.get().transaction.hash(),
                    })
                }
                let new_hash = *pool_tx.transaction.hash();
                let new_transaction = pool_tx.transaction.clone();
                let replaced = entry.insert(pool_tx);
                self.by_hash.remove(replaced.transaction.hash());
                // 插入新的tx
                self.by_hash.insert(new_hash, new_transaction);
                // also remove the hash
                // 同时移除hahs
                replaced_tx = Some((replaced.transaction, replaced.subpool));
            }
        }

        // The next transaction of this sender
        // 这个sender的下一个transaction
        let on_chain_id = TransactionId::new(transaction.sender_id(), on_chain_nonce);
        {
            // 获取后代
            let mut descendants = self.descendant_txs_mut(&on_chain_id).peekable();

            // Tracks the next nonce we expect if the transactions are gapless
            // 追踪我们期望的下一个nonce，如果tx是gapless
            let mut next_nonce = on_chain_id.nonce;

            // We need to find out if the next transaction of the sender is considered pending
            // 我们需要检查是否sender的下一个tx被认为处于Pending
            //
            let mut has_parked_ancestor = if ancestor.is_none() {
                // the new transaction is the next one
                // 如果新的tx是下一个
                false
            } else {
                // SAFETY: the transaction was added above so the _inclusive_ descendants iterator
                // returns at least 1 tx.
                // 安全：tx被加到上面，因此_inclusive_ descendants iterator至少返回一个tx
                let (id, tx) = descendants.peek().expect("Includes >= 1; qed.");
                if id.nonce < tx_id.nonce {
                    !tx.state.is_pending()
                } else {
                    true
                }
            };

            // Traverse all transactions of the sender and update existing transactions
            // 遍历这个sender的所有transactions并且更新已经存在的transactions
            for (id, tx) in descendants {
                let current_pool = tx.subpool;

                // If there's a nonce gap, we can shortcircuit
                // 如果有nonce gap，我们可以短路
                if next_nonce != id.nonce {
                    break
                }

                // close the nonce gap
                // 关闭nonce gap
                tx.state.insert(TxState::NO_NONCE_GAPS);

                // set cumulative cost
                // 设置累计的cost
                tx.cumulative_cost = cumulative_cost;

                // Update for next transaction
                // 更新下一个tx
                cumulative_cost = tx.next_cumulative_cost();

                if cumulative_cost > on_chain_balance {
                    // sender lacks sufficient funds to pay for this transaction
                    // sender缺乏足够的funds来支付这个transaction
                    tx.state.remove(TxState::ENOUGH_BALANCE);
                } else {
                    tx.state.insert(TxState::ENOUGH_BALANCE);
                }

                // Update ancestor condition.
                // 更新ancestor的状态
                if has_parked_ancestor {
                    tx.state.remove(TxState::NO_PARKED_ANCESTORS);
                } else {
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                }
                has_parked_ancestor = !tx.state.is_pending();

                // update the pool based on the state
                // 更新pool基于state
                tx.subpool = tx.state.into();

                if tx_id.eq(id) {
                    // if it is the new transaction, track the state
                    // 如果这是新的transaction，追踪state
                    state = tx.state;
                } else {
                    // check if anything changed
                    // 检查是否发生了任何变更
                    if current_pool != tx.subpool {
                        // 推送pool update事件
                        updates.push(PoolUpdate {
                            id: *id,
                            hash: *tx.transaction.hash(),
                            current: current_pool,
                            destination: Destination::Pool(tx.subpool),
                        })
                    }
                }

                // increment for next iteration
                // 添加下一个iteration
                next_nonce = id.next_nonce();
            }
        }

        // If this wasn't a replacement transaction we need to update the counter.
        // 如果这不是一个replacement transaction，我们需要更新counter
        if replaced_tx.is_none() {
            self.tx_inc(tx_id.sender);
        }

        Ok(InsertOk { transaction, move_to: state.into(), state, replaced_tx, updates })
    }

    /// Number of transactions in the entire pool
    /// 整个pool的txs的数目
    pub(crate) fn len(&self) -> usize {
        self.txs.len()
    }

    /// Whether the pool is empty
    /// pool是否为空
    pub(crate) fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
}

#[cfg(test)]
#[allow(missing_docs)]
impl<T: PoolTransaction> AllTransactions<T> {
    pub(crate) fn tx_count(&self, sender: SenderId) -> usize {
        self.tx_counter.get(&sender).copied().unwrap_or_default()
    }
}

impl<T: PoolTransaction> Default for AllTransactions<T> {
    fn default() -> Self {
        Self {
            max_account_slots: TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
            minimal_protocol_basefee: MIN_PROTOCOL_BASE_FEE,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            by_hash: Default::default(),
            txs: Default::default(),
            tx_counter: Default::default(),
            last_seen_block_number: 0,
            last_seen_block_hash: Default::default(),
            pending_basefee: Default::default(),
        }
    }
}

/// Result type for inserting a transaction
pub(crate) type InsertResult<T> = Result<InsertOk<T>, InsertErr<T>>;

/// Err variant of `InsertResult`
#[derive(Debug)]
pub(crate) enum InsertErr<T: PoolTransaction> {
    /// Attempted to replace existing transaction, but was underpriced
    /// 试着替换已经存在的transaction，但是underpriced
    Underpriced {
        #[allow(unused)]
        transaction: Arc<ValidPoolTransaction<T>>,
        existing: TxHash,
    },
    /// The transactions feeCap is lower than the chain's minimum fee requirement.
    /// tx的feeCap小于chain的最小的fee要求
    ///
    /// See also [`MIN_PROTOCOL_BASE_FEE`]
    FeeCapBelowMinimumProtocolFeeCap { transaction: Arc<ValidPoolTransaction<T>>, fee_cap: u128 },
    /// Sender currently exceeds the configured limit for max account slots.
    /// Sender当前超过了配置的max account slots
    ///
    /// The sender can be considered a spammer at this point.
    /// 这个时候sender可以认为是一个spammer
    ExceededSenderTransactionsCapacity { transaction: Arc<ValidPoolTransaction<T>> },
    /// Transaction gas limit exceeds block's gas limit
    /// transaction的gas limit超过了block的gas limit
    TxGasLimitMoreThanAvailableBlockGas {
        transaction: Arc<ValidPoolTransaction<T>>,
        block_gas_limit: u64,
        tx_gas_limit: u64,
    },
}

/// Transaction was successfully inserted into the pool
/// Transaction被成功插入到pool
#[derive(Debug)]
pub(crate) struct InsertOk<T: PoolTransaction> {
    /// Ref to the inserted transaction.
    /// 引用被插入的tx
    transaction: Arc<ValidPoolTransaction<T>>,
    /// Where to move the transaction to.
    /// 这个tx被移动到了哪里
    move_to: SubPool,
    /// Current state of the inserted tx.
    /// 插入的tx的当前状态
    #[allow(unused)]
    state: TxState,
    /// The transaction that was replaced by this.
    /// 被这个transaction移除的transaction
    replaced_tx: Option<(Arc<ValidPoolTransaction<T>>, SubPool)>,
    /// Additional updates to transactions affected by this change.
    /// 额外的updates，对于被这次change影响的txs
    updates: Vec<PoolUpdate>,
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
/// 内部的transaction类型，由`AllTransactions`使用，同时有额外的信息用于决定transaction的内部状态
#[derive(Debug)]
pub(crate) struct PoolInternalTransaction<T: PoolTransaction> {
    /// The actual transaction object.
    /// 真正的transaction对象
    pub(crate) transaction: Arc<ValidPoolTransaction<T>>,
    /// The `SubPool` that currently contains this transaction.
    /// 当前包含这个transaction的`SubPool`
    pub(crate) subpool: SubPool,
    /// Keeps track of the current state of the transaction and therefor in which subpool it should
    /// reside
    /// 追踪transaction的当前状态以及它应该在哪个subpool
    pub(crate) state: TxState,
    /// The total cost all transactions before this transaction.
    /// 这个transaction之前的所有transactions的cost
    ///
    /// This is the combined `cost` of all transactions from the same sender that currently
    /// come before this transaction.
    /// 这是combined `cost`，对于来自同一个sender的所有transactions，在这个transactions之前
    pub(crate) cumulative_cost: U256,
}

// === impl PoolInternalTransaction ===

impl<T: PoolTransaction> PoolInternalTransaction<T> {
    fn next_cumulative_cost(&self) -> U256 {
        self.cumulative_cost + self.transaction.cost()
    }
}

/// Tracks the result after updating the pool
/// 追踪在更新pool之后的结果
#[derive(Debug)]
pub(crate) struct UpdateOutcome<T: PoolTransaction> {
    /// transactions promoted to the pending pool
    /// 提交到pending pool的transactions
    pub(crate) promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transaction that failed and were discarded
    /// 失败并且被丢弃的transactions
    pub(crate) discarded: Vec<TxHash>,
}

impl<T: PoolTransaction> Default for UpdateOutcome<T> {
    fn default() -> Self {
        Self { promoted: vec![], discarded: vec![] }
    }
}

/// Represents the outcome of a prune
/// 代表依次Prune的结果
pub struct PruneResult<T: PoolTransaction> {
    /// A list of added transactions that a pruned marker satisfied
    /// 一系列添加的txs，被pruned marker满足
    pub promoted: Vec<AddedTransaction<T>>,
    /// all transactions that failed to be promoted and now are discarded
    /// 所有failed to promoted并且现在被丢弃的txs
    pub failed: Vec<TxHash>,
    /// all transactions that were pruned from the ready pool
    /// 从ready pool中清理的txs
    pub pruned: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> fmt::Debug for PruneResult<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "PruneResult {{ ")?;
        write!(
            fmt,
            "promoted: {:?}, ",
            self.promoted.iter().map(|tx| *tx.hash()).collect::<Vec<_>>()
        )?;
        write!(fmt, "failed: {:?}, ", self.failed)?;
        write!(
            fmt,
            "pruned: {:?}, ",
            self.pruned.iter().map(|tx| *tx.transaction.hash()).collect::<Vec<_>>()
        )?;
        write!(fmt, "}}")?;
        Ok(())
    }
}

/// Stores relevant context about a sender.
/// 存储关于一个sender的上下文
#[derive(Debug, Clone, Default)]
pub(crate) struct SenderInfo {
    /// current nonce of the sender.
    /// 当前sender的nonce
    pub(crate) state_nonce: u64,
    /// Balance of the sender at the current point.
    /// 当前状态sender的balance
    pub(crate) balance: U256,
}

// === impl SenderInfo ===

impl SenderInfo {
    /// Updates the info with the new values.
    /// 用新的值更新info
    fn update(&mut self, state_nonce: u64, balance: U256) {
        *self = Self { state_nonce, balance };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
        traits::TransactionOrigin,
    };

    #[test]
    fn test_insert_pending() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let valid_tx = f.validated(tx);
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        // 没有更新
        assert!(updates.is_empty());
        // 没有替换任何tx
        assert!(replaced_tx.is_none());
        // 没有nonce gap
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        // 有足够的balance
        assert!(state.contains(TxState::ENOUGH_BALANCE));
        assert_eq!(move_to, SubPool::Pending);

        // 根据id获取tx
        let inserted = pool.txs.get(&valid_tx.transaction_id).unwrap();
        assert_eq!(inserted.subpool, SubPool::Pending);
    }

    #[test]
    fn test_simple_insert() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let valid_tx = f.validated(tx.clone());
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        // 没有足够的balance
        assert!(!state.contains(TxState::ENOUGH_BALANCE));
        // 移动到了queued subpool
        assert_eq!(move_to, SubPool::Queued);

        assert_eq!(pool.len(), 1);
        assert!(pool.contains(valid_tx.hash()));
        let expected_state = TxState::ENOUGH_FEE_CAP_BLOCK | TxState::NO_NONCE_GAPS;
        let inserted = pool.get(valid_tx.id()).unwrap();
        assert!(inserted.state.intersects(expected_state));

        // insert the same tx again
        // 再次插入同样的tx
        let res = pool.insert_tx(valid_tx, on_chain_balance, on_chain_nonce);
        assert!(res.is_err());
        assert_eq!(pool.len(), 1);

        // tx变为next
        let valid_tx = f.validated(tx.next());
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(!state.contains(TxState::ENOUGH_BALANCE));
        // 都是位于Queued
        assert_eq!(move_to, SubPool::Queued);

        assert!(pool.contains(valid_tx.hash()));
        assert_eq!(pool.len(), 2);
        let inserted = pool.get(valid_tx.id()).unwrap();
        assert!(inserted.state.intersects(expected_state));
    }

    #[test]
    fn insert_already_imported() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let tx = f.validated(tx);
        // 添加tx
        pool.add_transaction(tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        match pool.add_transaction(tx, on_chain_balance, on_chain_nonce).unwrap_err() {
            PoolError::AlreadyImported(_) => {}
            _ => unreachable!(),
        }
    }

    #[test]
    fn insert_replace() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        // 第一次插入
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);
        // 提高price
        let replacement = f.validated(tx.rng_hash().inc_price());
        // 第二次插入
        let InsertOk { updates, replaced_tx, .. } =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        let replaced = replaced_tx.unwrap();
        assert_eq!(replaced.0.hash(), first.hash());

        // 不包含第一个hash，只包含replacement的hash
        assert!(!pool.contains(first.hash()));
        assert!(pool.contains(replacement.hash()));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn insert_replace_underpriced() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        // 对transaction进行clone
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first, on_chain_balance, on_chain_nonce);
        let mut replacement = f.validated(tx.rng_hash());
        // 降低price
        replacement.transaction = replacement.transaction.decr_price();
        let err = pool.insert_tx(replacement, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));
    }

    // insert nonce then nonce - 1
    #[test]
    fn insert_previous() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        // 增加了nonce
        let tx = MockTransaction::eip1559().inc_nonce().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);

        let first_in_pool = pool.get(first.id()).unwrap();

        // has nonce gap
        // 有nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));

        // 插入prev
        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        // no updates since still in queued pool
        // 没有更新，因为依然在queued pool
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::Queued);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        // 存在non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
    }

    // insert nonce then nonce - 1
    #[test]
    fn insert_with_updates() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_nonce().set_gas_price(100).inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce).unwrap();

        let first_in_pool = pool.get(first.id()).unwrap();
        // has nonce gap
        // 存在nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(SubPool::Queued, first_in_pool.subpool);

        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        // updated previous tx
        // 更新了之前的tx
        assert_eq!(updates.len(), 1);
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::Pending);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(SubPool::Pending, first_in_pool.subpool);
    }

    #[test]
    fn insert_previous_blocking() {
        // 现在有更高的on chain balance
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        pool.pending_basefee = pool.minimal_protocol_basefee.checked_add(1).unwrap();
        let tx = MockTransaction::eip1559().inc_nonce().inc_limit();
        let first = f.validated(tx.clone());

        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);

        let first_in_pool = pool.get(first.id()).unwrap();

        assert!(tx.get_gas_price() < pool.pending_basefee as u128);
        // has nonce gap
        // 存在nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));

        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        assert!(!state.contains(TxState::ENOUGH_FEE_CAP_BLOCK));
        // no updates since still in queued pool
        // 没有更新，因为依然在queued pool
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::BaseFee);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        // 有non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
    }

    #[test]
    fn rejects_spammer() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let mut tx = MockTransaction::eip1559();
        for _ in 0..pool.max_account_slots {
            tx = tx.next();
            // 插入max account slots个txs
            pool.insert_tx(f.validated(tx.clone()), on_chain_balance, on_chain_nonce).unwrap();
        }

        assert_eq!(
            pool.max_account_slots,
            // 获取sender的tx的数目
            pool.tx_count(f.ids.sender_id(&tx.get_sender()).unwrap())
        );

        // 再插入一个tx
        let err =
            pool.insert_tx(f.validated(tx.next()), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::ExceededSenderTransactionsCapacity { .. }));
    }

    #[test]
    fn allow_local_spamming() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let mut tx = MockTransaction::eip1559();
        for _ in 0..pool.max_account_slots {
            tx = tx.next();
            pool.insert_tx(
                f.validated_with_origin(TransactionOrigin::Local, tx.clone()),
                on_chain_balance,
                on_chain_nonce,
            )
            .unwrap();
        }

        assert_eq!(
            pool.max_account_slots,
            pool.tx_count(f.ids.sender_id(&tx.get_sender()).unwrap())
        );

        // 允许local的spamming
        pool.insert_tx(
            f.validated_with_origin(TransactionOrigin::Local, tx.next()),
            on_chain_balance,
            on_chain_nonce,
        )
        .unwrap();
    }

    #[test]
    fn reject_tx_over_gas_limit() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        // 设置超过gas limit的tx
        let tx = MockTransaction::eip1559().with_gas_limit(30_000_001);

        assert!(matches!(
            pool.insert_tx(f.validated(tx), on_chain_balance, on_chain_nonce),
            Err(InsertErr::TxGasLimitMoreThanAvailableBlockGas { .. })
        ));
    }

    #[test]
    fn update_basefee_subpools() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx = MockTransaction::eip1559().inc_price_by(10);
        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        assert_eq!(pool.pending_pool.len(), 1);

        // 更新base fee
        pool.update_basefee((tx.max_fee_per_gas() + 1) as u64);

        // pending pool变为空，basefee pool变为1
        assert!(pool.pending_pool.is_empty());
        assert_eq!(pool.basefee_pool.len(), 1);

        // transaction进入了base fee pool
        assert_eq!(pool.all_transactions.txs.get(&id).unwrap().subpool, SubPool::BaseFee)
    }

    #[test]
    fn discard_nonce_too_low() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx = MockTransaction::eip1559().inc_price_by(10);
        let validated = f.validated(tx.clone());
        let id = *validated.id();
        // 添加transaction
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        let next = tx.next();
        let validated = f.validated(next.clone());
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        assert_eq!(pool.pending_pool.len(), 2);

        let mut changed_senders = HashMap::new();
        // 插入changed senders
        changed_senders.insert(
            id.sender,
            SenderInfo { state_nonce: next.get_nonce(), balance: U256::from(1_000) },
        );
        // 更新accounts
        let outcome = pool.update_accounts(changed_senders);
        assert_eq!(outcome.discarded.len(), 1);
        assert_eq!(pool.pending_pool.len(), 1);
    }
}
