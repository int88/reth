use crate::{
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError},
    identifier::{SenderId, TransactionId},
    pool::pending::PendingTransaction,
    PoolTransaction, TransactionOrdering, ValidPoolTransaction,
};
use alloy_consensus::Transaction;
use alloy_eips::Typed2718;
use alloy_primitives::Address;
use core::fmt;
use reth_primitives::InvalidTransactionError;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::Arc,
};
use tokio::sync::broadcast::{error::TryRecvError, Receiver};
use tracing::debug;

/// An iterator that returns transactions that can be executed on the current state (*best*
/// transactions).
/// 一个iterator，返回txs可以在当前的state执行
///
/// This is a wrapper around [`BestTransactions`] that also enforces a specific basefee.
/// 这是对于[`BestTransactions`]的一个wrapper，同时执行一个特定的basefee
///
/// This iterator guarantees that all transaction it returns satisfy both the base fee and blob fee!
/// 这个iterator保证所有它返回的txs满足base fee和blob fee
pub(crate) struct BestTransactionsWithFees<T: TransactionOrdering> {
    pub(crate) best: BestTransactions<T>,
    pub(crate) base_fee: u64,
    pub(crate) base_fee_per_blob_gas: u64,
}

impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactionsWithFees<T> {
    fn mark_invalid(&mut self, tx: &Self::Item, kind: InvalidPoolTransactionError) {
        BestTransactions::mark_invalid(&mut self.best, tx, kind)
    }

    fn no_updates(&mut self) {
        self.best.no_updates()
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true)
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.best.set_skip_blobs(skip_blobs)
    }
}

impl<T: TransactionOrdering> Iterator for BestTransactionsWithFees<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        // find the next transaction that satisfies the base fee
        loop {
            let best = Iterator::next(&mut self.best)?;
            // If both the base fee and blob fee (if applicable for EIP-4844) are satisfied, return
            // the transaction
            if best.transaction.max_fee_per_gas() >= self.base_fee as u128 &&
                best.transaction
                    .max_fee_per_blob_gas()
                    .is_none_or(|fee| fee >= self.base_fee_per_blob_gas as u128)
            {
                return Some(best);
            }
            crate::traits::BestTransactions::mark_invalid(
                self,
                &best,
                InvalidPoolTransactionError::Underpriced,
            );
        }
    }
}

/// An iterator that returns transactions that can be executed on the current state (*best*
/// transactions).
/// 一个iterator，返回txs，可以在当前的state执行（最好的txs）
///
/// The [`PendingPool`](crate::pool::pending::PendingPool) contains transactions that *could* all
/// be executed on the current state, but only yields transactions that are ready to be executed
/// now. While it contains all gapless transactions of a sender, it _always_ only returns the
/// transaction with the current on chain nonce.
/// 尽管它拥有一个sender的gapless txs，它总是只返回tx，有着当前的chain nonce
#[derive(Debug)]
pub struct BestTransactions<T: TransactionOrdering> {
    /// Contains a copy of _all_ transactions of the pending pool at the point in time this
    /// iterator was created.
    /// 包含pending pool中所有的txs的一个拷贝，在iterator被创建的时候
    pub(crate) all: BTreeMap<TransactionId, PendingTransaction<T>>,
    /// Transactions that can be executed right away: these have the expected nonce.
    /// 可以被立即执行的txs，他们有期望的nonce
    ///
    /// Once an `independent` transaction with the nonce `N` is returned, it unlocks `N+1`, which
    /// then can be moved from the `all` set to the `independent` set.
    /// 一旦有着nonce `N`的`independent` tx返回，它解锁`N+1`，之后它从`all`移动到`independent` set
    pub(crate) independent: BTreeSet<PendingTransaction<T>>,
    /// There might be the case where a yielded transactions is invalid, this will track it.
    pub(crate) invalid: HashSet<SenderId>,
    /// Used to receive any new pending transactions that have been added to the pool after this
    /// iterator was static fileted
    /// 用于接受任何新的pending txs，在iterator被创建之后
    ///
    /// These new pending transactions are inserted into this iterator's pool before yielding the
    /// next value
    /// 这些新的pending txs被插入到iterator的pool，在产生next value之前
    pub(crate) new_transaction_receiver: Option<Receiver<PendingTransaction<T>>>,
    /// Flag to control whether to skip blob transactions (EIP4844).
    /// Flag用于控制是否跳过blob txs
    pub(crate) skip_blobs: bool,
}

impl<T: TransactionOrdering> BestTransactions<T> {
    /// Mark the transaction and it's descendants as invalid.
    /// 标记tx以及它的后代为非法
    pub(crate) fn mark_invalid(
        &mut self,
        tx: &Arc<ValidPoolTransaction<T::Transaction>>,
        _kind: InvalidPoolTransactionError,
    ) {
        self.invalid.insert(tx.sender_id());
    }

    /// Returns the ancestor the given transaction, the transaction with `nonce - 1`.
    /// 返回给定tx的ancestor，tx有着`nonce - 1`
    ///
    /// Note: for a transaction with nonce higher than the current on chain nonce this will always
    /// return an ancestor since all transaction in this pool are gapless.
    /// 注意：对于一个tx有着高于当前chain
    /// nonce的nonce，它总是返回一个ancestor，因为这个Pool中的所有tx都是gapless
    pub(crate) fn ancestor(&self, id: &TransactionId) -> Option<&PendingTransaction<T>> {
        self.all.get(&id.unchecked_ancestor()?)
    }

    /// Non-blocking read on the new pending transactions subscription channel
    /// 非阻塞读，对于新的pending tx的订阅channel
    fn try_recv(&mut self) -> Option<PendingTransaction<T>> {
        loop {
            match self.new_transaction_receiver.as_mut()?.try_recv() {
                Ok(tx) => return Some(tx),
                // note TryRecvError::Lagged can be returned here, which is an error that attempts
                // to correct itself on consecutive try_recv() attempts

                // the cost of ignoring this error is allowing old transactions to get
                // overwritten after the chan buffer size is met
                Err(TryRecvError::Lagged(_)) => {
                    // Handle the case where the receiver lagged too far behind.
                    // `num_skipped` indicates the number of messages that were skipped.
                }

                // this case is still better than the existing iterator behavior where no new
                // pending txs are surfaced to consumers
                Err(_) => return None,
            }
        }
    }

    /// Removes the currently best independent transaction from the independent set and the total
    /// set.
    /// 移除当前最佳的independent tx，从independent set以及total set
    fn pop_best(&mut self) -> Option<PendingTransaction<T>> {
        self.independent.pop_last().inspect(|best| {
            // 从all中移除
            let removed = self.all.remove(best.transaction.id());
            debug_assert!(removed.is_some(), "must be present in both sets");
        })
    }

    /// Checks for new transactions that have come into the `PendingPool` after this iterator was
    /// created and inserts them
    /// 检查新插入到`PendingPool`的txs，在这个iterator被创建并且插入之后
    fn add_new_transactions(&mut self) {
        // 试着接收一些新的txs
        while let Some(pending_tx) = self.try_recv() {
            //  same logic as PendingPool::add_transaction/PendingPool::best_with_unlocked
            let tx_id = *pending_tx.transaction.id();
            if self.ancestor(&tx_id).is_none() {
                self.independent.insert(pending_tx.clone());
            }
            self.all.insert(tx_id, pending_tx);
        }
    }
}

impl<T: TransactionOrdering> crate::traits::BestTransactions for BestTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item, kind: InvalidPoolTransactionError) {
        Self::mark_invalid(self, tx, kind)
    }

    fn no_updates(&mut self) {
        self.new_transaction_receiver.take();
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true);
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.skip_blobs = skip_blobs;
    }
}

impl<T: TransactionOrdering> Iterator for BestTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T::Transaction>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.add_new_transactions();
            // Remove the next independent tx with the highest priority
            // 移除下一个独立的tx，有着最高的priority
            let best = self.pop_best()?;
            let sender_id = best.transaction.sender_id();

            // skip transactions for which sender was marked as invalid
            // 跳过sender被标记为非法的txs
            if self.invalid.contains(&sender_id) {
                debug!(
                    target: "txpool",
                    "[{:?}] skipping invalid transaction",
                    best.transaction.hash()
                );
                continue
            }

            // Insert transactions that just got unlocked.
            // 插入刚刚解锁的txs
            if let Some(unlocked) = self.all.get(&best.unlocks()) {
                // 如果存在当前的sender的下一个tx，加入independent
                self.independent.insert(unlocked.clone());
            }

            if self.skip_blobs && best.transaction.transaction.is_eip4844() {
                // blobs should be skipped, marking them as invalid will ensure that no dependent
                // transactions are returned
                // blobs应该被跳过，将他们标记为非法，能确保没有依赖的txs被返回
                self.mark_invalid(
                    &best.transaction,
                    InvalidPoolTransactionError::Eip4844(
                        Eip4844PoolTransactionError::NoEip4844Blobs,
                    ),
                )
            } else {
                return Some(best.transaction)
            }
        }
    }
}

/// A [`BestTransactions`](crate::traits::BestTransactions) implementation that filters the
/// transactions of iter with predicate.
/// 一个[`BestTransactions`]实现，只返回predicate需要的txs
///
/// Filter out transactions are marked as invalid:
/// [`BestTransactions::mark_invalid`](crate::traits::BestTransactions::mark_invalid).
pub struct BestTransactionFilter<I, P> {
    pub(crate) best: I,
    pub(crate) predicate: P,
}

impl<I, P> BestTransactionFilter<I, P> {
    /// Create a new [`BestTransactionFilter`] with the given predicate.
    /// 创建一个新的[`BestTransactionFilter`]，有着给定的predicate
    pub const fn new(best: I, predicate: P) -> Self {
        Self { best, predicate }
    }
}

impl<I, P> Iterator for BestTransactionFilter<I, P>
where
    I: crate::traits::BestTransactions,
    P: FnMut(&<I as Iterator>::Item) -> bool,
{
    type Item = <I as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let best = self.best.next()?;
            if (self.predicate)(&best) {
                return Some(best)
            }
            self.best.mark_invalid(
                &best,
                InvalidPoolTransactionError::Consensus(InvalidTransactionError::TxTypeNotSupported),
            );
        }
    }
}

impl<I, P> crate::traits::BestTransactions for BestTransactionFilter<I, P>
where
    I: crate::traits::BestTransactions,
    P: FnMut(&<I as Iterator>::Item) -> bool + Send,
{
    fn mark_invalid(&mut self, tx: &Self::Item, kind: InvalidPoolTransactionError) {
        crate::traits::BestTransactions::mark_invalid(&mut self.best, tx, kind)
    }

    fn no_updates(&mut self) {
        self.best.no_updates()
    }

    fn skip_blobs(&mut self) {
        self.set_skip_blobs(true)
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.best.set_skip_blobs(skip_blobs)
    }
}

impl<I: fmt::Debug, P> fmt::Debug for BestTransactionFilter<I, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BestTransactionFilter").field("best", &self.best).finish()
    }
}

/// Wrapper over [`crate::traits::BestTransactions`] that prioritizes transactions of certain
/// senders capping total gas used by such transactions.
/// 对于[`crate::traits::BestTransactions`]的封装，优先特定的senders
#[derive(Debug)]
pub struct BestTransactionsWithPrioritizedSenders<I: Iterator> {
    /// Inner iterator
    inner: I,
    /// A set of senders which transactions should be prioritized
    /// 一系列的senders，他们的txs需要被优待
    prioritized_senders: HashSet<Address>,
    /// Maximum total gas limit of prioritized transactions
    max_prioritized_gas: u64,
    /// Buffer with transactions that are not being prioritized. Those will be the first to be
    /// included after the prioritized transactions
    /// 缓存没有被优待的txs，他们是prioritized txs之后的第一批txs
    buffer: VecDeque<I::Item>,
    /// Tracker of total gas limit of prioritized transactions. Once it reaches
    /// `max_prioritized_gas` no more transactions will be prioritized
    /// 追踪prioritized txs的gas limit，一旦到达`max_prioritized_gas`，没有tx有优先级
    prioritized_gas: u64,
}

impl<I: Iterator> BestTransactionsWithPrioritizedSenders<I> {
    /// Constructs a new [`BestTransactionsWithPrioritizedSenders`].
    pub fn new(prioritized_senders: HashSet<Address>, max_prioritized_gas: u64, inner: I) -> Self {
        Self {
            inner,
            prioritized_senders,
            max_prioritized_gas,
            buffer: Default::default(),
            prioritized_gas: Default::default(),
        }
    }
}

impl<I, T> Iterator for BestTransactionsWithPrioritizedSenders<I>
where
    I: crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction,
{
    type Item = <I as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        // If we have space, try prioritizing transactions
        // 如果我们有空间，试着优先级化txs
        if self.prioritized_gas < self.max_prioritized_gas {
            for item in &mut self.inner {
                // 是否是prioritized sender以及是否超过了max prioritized gas
                if self.prioritized_senders.contains(&item.transaction.sender()) &&
                    self.prioritized_gas + item.transaction.gas_limit() <=
                        self.max_prioritized_gas
                {
                    // 添加prioritized_gas
                    self.prioritized_gas += item.transaction.gas_limit();
                    return Some(item)
                }
                self.buffer.push_back(item);
            }
        }

        if let Some(item) = self.buffer.pop_front() {
            Some(item)
        } else {
            self.inner.next()
        }
    }
}

impl<I, T> crate::traits::BestTransactions for BestTransactionsWithPrioritizedSenders<I>
where
    I: crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction,
{
    fn mark_invalid(&mut self, tx: &Self::Item, kind: InvalidPoolTransactionError) {
        self.inner.mark_invalid(tx, kind)
    }

    fn no_updates(&mut self) {
        self.inner.no_updates()
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        if skip_blobs {
            self.buffer.retain(|tx| !tx.transaction.is_eip4844())
        }
        self.inner.set_skip_blobs(skip_blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pool::pending::PendingPool,
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
        BestTransactions, Priority,
    };
    use alloy_primitives::U256;

    #[test]
    fn test_best_iter() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        // 插入10个gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            // 增加nonce
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best();
        assert_eq!(best.all.len(), num_tx as usize);
        assert_eq!(best.independent.len(), 1);

        // check tx are returned in order
        // 按照顺序检查返回的tx
        for nonce in 0..num_tx {
            assert_eq!(best.independent.len(), 1);
            let tx = best.next().unwrap();
            assert_eq!(tx.nonce(), nonce);
        }
    }

    #[test]
    fn test_best_iter_invalid() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best();

        // mark the first tx as invalid
        // 将第一个tx标记为非法
        let invalid = best.independent.iter().next().unwrap();
        best.mark_invalid(
            &invalid.transaction.clone(),
            InvalidPoolTransactionError::Consensus(InvalidTransactionError::TxTypeNotSupported),
        );

        // iterator is empty
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_transactions_iter_invalid() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 10;
        // insert 10 gapless tx
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best: Box<
            dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<MockTransaction>>>,
        > = Box::new(pool.best());

        let tx = Iterator::next(&mut best).unwrap();
        crate::traits::BestTransactions::mark_invalid(
            &mut *best,
            &tx,
            InvalidPoolTransactionError::Consensus(InvalidTransactionError::TxTypeNotSupported),
        );
        assert!(Iterator::next(&mut best).is_none());
    }

    #[test]
    fn test_best_with_fees_iter_base_fee_satisfied() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 15;

        // Insert transactions with a max_fee_per_gas greater than or equal to the base fee
        // Without blob fee
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip1559()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        for nonce in 0..num_tx {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.nonce(), nonce);
            assert!(tx.transaction.max_fee_per_gas() >= base_fee as u128);
        }
    }

    #[test]
    fn test_best_with_fees_iter_base_fee_violated() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 20;
        let base_fee_per_blob_gas: u64 = 15;

        // Insert transactions with a max_fee_per_gas less than the base fee
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip1559()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 - 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // No transaction should be returned since all violate the base fee
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_blob_fee_satisfied() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with a max_fee_per_blob_gas greater than or equal to the base fee per
        // blob gas
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip4844()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5)
                .with_blob_fee(base_fee_per_blob_gas as u128 + 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // All transactions should be returned in order since they satisfy both base fee and blob
        // fee
        for nonce in 0..num_tx {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.nonce(), nonce);
            assert!(tx.transaction.max_fee_per_gas() >= base_fee as u128);
            assert!(
                tx.transaction.max_fee_per_blob_gas().unwrap() >= base_fee_per_blob_gas as u128
            );
        }

        // No more transactions should be returned
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_blob_fee_violated() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let num_tx = 5;
        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with a max_fee_per_blob_gas less than the base fee per blob gas
        for nonce in 0..num_tx {
            let tx = MockTransaction::eip4844()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5)
                .with_blob_fee(base_fee_per_blob_gas as u128 - 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // No transaction should be returned since all violate the blob fee
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_mixed_fees() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 20;

        // Insert transactions with varying max_fee_per_gas and max_fee_per_blob_gas
        let tx1 =
            MockTransaction::eip1559().rng_hash().with_nonce(0).with_max_fee(base_fee as u128 + 5);
        let tx2 = MockTransaction::eip4844()
            .rng_hash()
            .with_nonce(1)
            .with_max_fee(base_fee as u128 + 5)
            .with_blob_fee(base_fee_per_blob_gas as u128 + 5);
        let tx3 = MockTransaction::eip4844()
            .rng_hash()
            .with_nonce(2)
            .with_max_fee(base_fee as u128 + 5)
            .with_blob_fee(base_fee_per_blob_gas as u128 - 5);
        let tx4 =
            MockTransaction::eip1559().rng_hash().with_nonce(3).with_max_fee(base_fee as u128 - 5);

        pool.add_transaction(Arc::new(f.validated(tx1.clone())), 0);
        pool.add_transaction(Arc::new(f.validated(tx2.clone())), 0);
        pool.add_transaction(Arc::new(f.validated(tx3)), 0);
        pool.add_transaction(Arc::new(f.validated(tx4)), 0);

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        let expected_order = vec![tx1, tx2];
        for expected_tx in expected_order {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.transaction, expected_tx);
        }

        // No more transactions should be returned
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_add_transaction_with_next_nonce() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a BestTransactions iterator from the pool
        let mut best = pool.best();

        // Use a broadcast channel for transaction updates
        // 使用一个broadcast channel用于tx updates
        let (tx_sender, tx_receiver) =
            tokio::sync::broadcast::channel::<PendingTransaction<MockOrdering>>(1000);
        best.new_transaction_receiver = Some(tx_receiver);

        // Create a new transaction with nonce 5 and validate it
        // 创建一个新的tx，有着nonce 5并且校验它
        let new_tx = MockTransaction::eip1559().rng_hash().with_nonce(5);
        let valid_new_tx = f.validated(new_tx);

        // Send the new transaction through the broadcast channel
        // 通过broadcast channel发送新的tx
        let pending_tx = PendingTransaction {
            submission_id: 10,
            transaction: Arc::new(valid_new_tx.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx.clone()).unwrap();

        // Add new transactions to the iterator
        // 添加新的txs到iterator
        best.add_new_transactions();

        // Verify that the new transaction has been added to the 'all' map
        // 校验新的tx已经被添加到'all' map
        assert_eq!(best.all.len(), 6);
        assert!(best.all.contains_key(valid_new_tx.id()));

        // Verify that the new transaction has been added to the 'independent' set
        // 校验新的tx已经被加入到'independent' set
        assert_eq!(best.independent.len(), 2);
        assert!(best.independent.contains(&pending_tx));
    }

    #[test]
    fn test_best_add_transaction_with_ancestor() {
        // Initialize a new PendingPool with default MockOrdering and MockTransactionFactory
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a BestTransactions iterator from the pool
        let mut best = pool.best();

        // Use a broadcast channel for transaction updates
        let (tx_sender, tx_receiver) =
            tokio::sync::broadcast::channel::<PendingTransaction<MockOrdering>>(1000);
        best.new_transaction_receiver = Some(tx_receiver);

        // Create a new transaction with nonce 5 and validate it
        let base_tx1 = MockTransaction::eip1559().rng_hash().with_nonce(5);
        let valid_new_tx1 = f.validated(base_tx1.clone());

        // Send the new transaction through the broadcast channel
        let pending_tx1 = PendingTransaction {
            submission_id: 10,
            transaction: Arc::new(valid_new_tx1.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx1.clone()).unwrap();

        // Add new transactions to the iterator
        best.add_new_transactions();

        // Verify that the new transaction has been added to the 'all' map
        assert_eq!(best.all.len(), 6);
        assert!(best.all.contains_key(valid_new_tx1.id()));

        // Verify that the new transaction has been added to the 'independent' set
        assert_eq!(best.independent.len(), 2);
        assert!(best.independent.contains(&pending_tx1));

        // Attempt to add a new transaction with a different nonce (not a duplicate)
        // 试着添加一个新的tx，有着不同的nonce（不是重复的）
        let base_tx2 = base_tx1.with_nonce(6);
        let valid_new_tx2 = f.validated(base_tx2);

        // Send the new transaction through the broadcast channel
        let pending_tx2 = PendingTransaction {
            submission_id: 11, // Different submission ID
            transaction: Arc::new(valid_new_tx2.clone()),
            priority: Priority::Value(U256::from(1000)),
        };
        tx_sender.send(pending_tx2.clone()).unwrap();

        // Add new transactions to the iterator
        // 添加新的txs到iterato
        best.add_new_transactions();

        // Verify that the new transaction has been added to 'all'
        assert_eq!(best.all.len(), 7);
        assert!(best.all.contains_key(valid_new_tx2.id()));

        // Verify that the new transaction has not been added to the 'independent' set
        // 校验新的txs没有被加入到'indepedent' set中
        assert_eq!(best.independent.len(), 2);
        assert!(!best.independent.contains(&pending_tx2));
    }

    #[test]
    fn test_best_transactions_filter_trait_object() {
        // Initialize a new PendingPool with default MockOrdering and MockTransactionFactory
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 transactions with increasing nonces to the pool
        let num_tx = 5;
        let tx = MockTransaction::eip1559();
        for nonce in 0..num_tx {
            let tx = tx.clone().rng_hash().with_nonce(nonce);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Create a trait object of BestTransactions iterator from the pool
        let best: Box<dyn crate::traits::BestTransactions<Item = _>> = Box::new(pool.best());

        // Create a filter that only returns transactions with even nonces
        // 创建一个filter，只返回有着偶数nonces的txs
        let filter =
            BestTransactionFilter::new(best, |tx: &Arc<ValidPoolTransaction<MockTransaction>>| {
                tx.nonce() % 2 == 0
            });

        // Verify that the filter only returns transactions with even nonces
        // 校验filter只返回偶数的nonces
        for tx in filter {
            assert_eq!(tx.nonce() % 2, 0);
        }
    }

    #[test]
    fn test_best_transactions_prioritized_senders() {
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add 5 plain transactions from different senders with increasing gas price
        // 添加5个plain txs，有着不同的senders，有着增加的gas price
        for gas_price in 0..5 {
            let tx = MockTransaction::eip1559().with_gas_price(gas_price);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        // Add another transaction with 0 gas price that's going to be prioritized by sender
        // 添加另一个tx，gas price为0，基于sender优先级更高？
        let prioritized_tx = MockTransaction::eip1559().with_gas_price(0);
        let valid_prioritized_tx = f.validated(prioritized_tx.clone());
        pool.add_transaction(Arc::new(valid_prioritized_tx), 0);

        let prioritized_senders = HashSet::from([prioritized_tx.sender()]);
        let best =
            BestTransactionsWithPrioritizedSenders::new(prioritized_senders, 200, pool.best());

        // Verify that the prioritized transaction is returned first
        // and the rest are returned in the reverse order of gas price
        // 校验prioritized tx被首先返回，剩下的按照gas price的反序返回
        let mut iter = best.into_iter();
        let top_of_block_tx = iter.next().unwrap();
        assert_eq!(top_of_block_tx.max_fee_per_gas(), 0);
        assert_eq!(top_of_block_tx.sender(), prioritized_tx.sender());
        for gas_price in (0..5).rev() {
            assert_eq!(iter.next().unwrap().max_fee_per_gas(), gas_price);
        }

        // TODO: Test that gas limits for prioritized transactions are respected
    }

    #[test]
    fn test_best_with_fees_iter_no_blob_fee_required() {
        // Tests transactions without blob fees where base fees are checked.
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 0; // No blob fee requirement

        // Insert transactions with max_fee_per_gas above the base fee
        for nonce in 0..5 {
            let tx = MockTransaction::eip1559()
                .rng_hash()
                .with_nonce(nonce)
                .with_max_fee(base_fee as u128 + 5);
            let valid_tx = f.validated(tx);
            pool.add_transaction(Arc::new(valid_tx), 0);
        }

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // All transactions should be returned as no blob fee requirement is imposed
        for nonce in 0..5 {
            let tx = best.next().expect("Transaction should be returned");
            assert_eq!(tx.nonce(), nonce);
        }

        // Ensure no more transactions are left
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_with_fees_iter_mix_of_blob_and_non_blob_transactions() {
        // Tests mixed scenarios with both blob and non-blob transactions.
        // 测试混合的场景，有blob和non-blob txs
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        let base_fee: u64 = 10;
        let base_fee_per_blob_gas: u64 = 15;

        // Add a non-blob transaction that satisfies the base fee
        // 添加一个non-blob tx，满足base fee
        let tx_non_blob =
            MockTransaction::eip1559().rng_hash().with_nonce(0).with_max_fee(base_fee as u128 + 5);
        pool.add_transaction(Arc::new(f.validated(tx_non_blob.clone())), 0);

        // Add a blob transaction that satisfies both base fee and blob fee
        // 添加一个blob tx，满足base fee和blob fee
        let tx_blob = MockTransaction::eip4844()
            .rng_hash()
            .with_nonce(1)
            .with_max_fee(base_fee as u128 + 5)
            .with_blob_fee(base_fee_per_blob_gas as u128 + 5);
        pool.add_transaction(Arc::new(f.validated(tx_blob.clone())), 0);

        let mut best = pool.best_with_basefee_and_blobfee(base_fee, base_fee_per_blob_gas);

        // Verify both transactions are returned
        // 校验两个txs都反悔了
        let tx = best.next().expect("Transaction should be returned");
        assert_eq!(tx.transaction, tx_non_blob);

        let tx = best.next().expect("Transaction should be returned");
        assert_eq!(tx.transaction, tx_blob);

        // Ensure no more transactions are left
        // 确保没有tx剩下
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_transactions_with_skipping_blobs() {
        // Tests the skip_blobs functionality to ensure blob transactions are skipped.
        // 测试skip_blobs功能确保blob txs被跳过
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add a blob transaction
        let tx_blob = MockTransaction::eip4844().rng_hash().with_nonce(0).with_blob_fee(100);
        let valid_blob_tx = f.validated(tx_blob);
        pool.add_transaction(Arc::new(valid_blob_tx), 0);

        // Add a non-blob transaction
        let tx_non_blob = MockTransaction::eip1559().rng_hash().with_nonce(1).with_max_fee(200);
        let valid_non_blob_tx = f.validated(tx_non_blob.clone());
        pool.add_transaction(Arc::new(valid_non_blob_tx), 0);

        let mut best = pool.best();
        best.skip_blobs();

        // Only the non-blob transaction should be returned
        // 只应该返回non-blob tx
        let tx = best.next().expect("Transaction should be returned");
        assert_eq!(tx.transaction, tx_non_blob);

        // Ensure no more transactions are left
        assert!(best.next().is_none());
    }

    #[test]
    fn test_best_transactions_no_updates() {
        // Tests the no_updates functionality to ensure it properly clears the
        // new_transaction_receiver.
        // 测试no_updates函数来确保它适当地清理了new_transaction_receiver
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Add a transaction
        let tx = MockTransaction::eip1559().rng_hash().with_nonce(0).with_max_fee(100);
        let valid_tx = f.validated(tx);
        pool.add_transaction(Arc::new(valid_tx), 0);

        let mut best = pool.best();

        // Use a broadcast channel for transaction updates
        let (_tx_sender, tx_receiver) =
            tokio::sync::broadcast::channel::<PendingTransaction<MockOrdering>>(1000);
        best.new_transaction_receiver = Some(tx_receiver);

        // Ensure receiver is set
        // 确保receiver被设置
        assert!(best.new_transaction_receiver.is_some());

        // Call no_updates to clear the receiver
        // 调用no_updatees来清理receiver
        best.no_updates();

        // Ensure receiver is cleared
        // 确保receiver被清理
        assert!(best.new_transaction_receiver.is_none());
    }

    // TODO: Same nonce test
}
