use crate::{
    identifier::TransactionId, pool::size::SizeTracker, PoolTransaction, ValidPoolTransaction,
};
use fnv::FnvHashMap;
use std::{cmp::Ordering, collections::BTreeSet, ops::Deref, sync::Arc};

/// A pool of transactions that are currently parked and are waiting for external changes (e.g.
/// basefee, ancestor transactions, balance) that eventually move the transaction into the pending
/// pool.
/// 当前处于parked的pool of transactions并且等待外部的改变（例如，basefee, ancesotr
/// txs，balance）最终被移动到 pending pool
///
/// This pool is a bijection: at all times each set (`best`, `by_id`) contains the same
/// transactions.
/// 这个pool是一个双向的：在所有时间，每个set（`best`，`by_id`）包含通用的tx
///
/// Note: This type is generic over [ParkedPool] which enforces that the underlying transaction type
/// is [ValidPoolTransaction] wrapped in an [Arc].
/// 注意：这个类型比[ParkedPool]更通用，它强制底层的tx类型是[ValidPoolTransaction]被封装在一个[Arc]
#[derive(Clone)]
pub(crate) struct ParkedPool<T: ParkedOrd> {
    /// Keeps track of transactions inserted in the pool.
    /// 追踪插入到pool的txs
    ///
    /// This way we can determine when transactions where submitted to the pool.
    /// 这种方法我们能确定什么时候txs被加载到pool中
    submission_id: u64,
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    /// 按照id进行聚合的，pool中所有的txs
    by_id: FnvHashMap<TransactionId, ParkedPoolTransaction<T>>,
    /// All transactions sorted by their order function.
    /// 通过order函数进行排序的所有的tx
    ///
    /// The higher, the better.
    /// 越高越好
    best: BTreeSet<ParkedPoolTransaction<T>>,
    /// Keeps track of the size of this pool.
    /// 追踪这个pool的size
    ///
    /// See also [`PoolTransaction::size`].
    size_of: SizeTracker,
}

// === impl ParkedPool ===

impl<T: ParkedOrd> ParkedPool<T> {
    /// Adds a new transactions to the pending queue.
    /// 添加一个新的tx到pending queue
    ///
    /// # Panics
    ///
    /// If the transaction is already included.
    /// 如果tx已经存在就panic
    pub(crate) fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = *tx.id();
        assert!(
            !self.by_id.contains_key(&id),
            "transaction already included {:?}",
            self.by_id.contains_key(&id)
        );
        let submission_id = self.next_id();

        // keep track of size
        // 追踪size
        self.size_of += tx.size();

        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };

        // 用id插入
        self.by_id.insert(id, transaction.clone());
        self.best.insert(transaction);
    }

    /// Returns an iterator over all transactions in the pool
    /// 返回一个iterator，遍历pool中的所有txs
    pub(crate) fn all(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.by_id.values().map(|tx| tx.transaction.clone().into())
    }

    /// Removes the transaction from the pool
    /// 从pool中移除tx
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // remove from queues
        // 从队列中移除
        let tx = self.by_id.remove(id)?;
        self.best.remove(&tx);

        // keep track of size
        // 追踪size
        self.size_of -= tx.transaction.size();

        Some(tx.transaction.into())
    }

    /// Removes the worst transaction from this pool.
    /// 从这个pool中移除最差的tx
    pub(crate) fn pop_worst(&mut self) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let worst = self.best.iter().next().map(|tx| *tx.transaction.id())?;
        self.remove_transaction(&worst)
    }

    // 下一个id
    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// The reported size of all transactions in this pool.
    /// 当前pool中所有txs的size
    pub(crate) fn size(&self) -> usize {
        self.size_of.into()
    }

    /// Number of transactions in the entire pool
    /// 整个pool中的txs的数目
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the pool is empty
    /// pool是否为empty
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

impl<T: PoolTransaction> ParkedPool<BasefeeOrd<T>> {
    /// Returns all transactions that satisfy the given basefee.
    /// 返回所有的txs，满足给定的basefee
    ///
    /// Note: this does _not_ remove the transactions
    /// 注意：这不会移除txs
    pub(crate) fn satisfy_base_fee_transactions(
        &self,
        basefee: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let ids = self.satisfy_base_fee_ids(basefee);
        let mut txs = Vec::with_capacity(ids.len());
        for id in ids {
            txs.push(self.by_id.get(&id).expect("transaction exists").transaction.clone().into());
        }
        txs
    }

    /// Returns all transactions that satisfy the given basefee.
    /// 返回所有的txs，满足给定的basefee
    fn satisfy_base_fee_ids(&self, basefee: u64) -> Vec<TransactionId> {
        let mut transactions = Vec::new();
        {
            let mut iter = self.by_id.iter().peekable();

            while let Some((id, tx)) = iter.next() {
                if tx.transaction.transaction.max_fee_per_gas() < basefee as u128 {
                    // still parked -> skip descendant transactions
                    // 依然parked -> 跳过descendant txs
                    'this: while let Some((peek, _)) = iter.peek() {
                        if peek.sender != id.sender {
                            break 'this
                        }
                        iter.next();
                    }
                } else {
                    transactions.push(*id);
                }
            }
        }
        transactions
    }

    /// Removes all transactions and their dependent transaction from the subpool that no longer
    /// satisfy the given basefee.
    /// 移除所有的txs以及它们依赖的tx，从subpool中，不再满足给定的basefee
    ///
    /// Note: the transactions are not returned in a particular order.
    /// 注意：tx不按照特定的顺序返回
    pub(crate) fn enforce_basefee(&mut self, basefee: u64) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let to_remove = self.satisfy_base_fee_ids(basefee);

        let mut removed = Vec::with_capacity(to_remove.len());
        for id in to_remove {
            removed.push(self.remove_transaction(&id).expect("transaction exists"));
        }

        removed
    }
}

impl<T: ParkedOrd> Default for ParkedPool<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            best: Default::default(),
            size_of: Default::default(),
        }
    }
}

/// Represents a transaction in this pool.
/// 代表这个pool中的一个tx
struct ParkedPoolTransaction<T: ParkedOrd> {
    /// Identifier that tags when transaction was submitted in the pool.
    /// id用于标识tx什么时候被提交到pool
    submission_id: u64,
    /// Actual transaction.
    /// 真正的tx
    transaction: T,
}

impl<T: ParkedOrd> Clone for ParkedPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self { submission_id: self.submission_id, transaction: self.transaction.clone() }
    }
}

impl<T: ParkedOrd> Eq for ParkedPoolTransaction<T> {}

impl<T: ParkedOrd> PartialEq<Self> for ParkedPoolTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: ParkedOrd> PartialOrd<Self> for ParkedPoolTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> Ord for ParkedPoolTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by the transactions first, and only if two tx are equal this compares
        // the unique `submission_id`.
        // 首先比较txs，并且如果两个tx相等，则匹配唯一的`submission_id`
        // "better" transactions are Greater
        // "better" txs更好
        self.transaction
            .cmp(&other.transaction)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// Helper trait used for custom `Ord` wrappers around a transaction.
/// Helper trait用于自定义的`Ord` wrappers，对于一个tx
///
/// This is effectively a wrapper for `Arc<ValidPoolTransaction>` with custom `Ord` implementation.
/// 这是一个对于`Arc<ValidPoolTransaction>`，有着自定义的`Ord`实现
pub(crate) trait ParkedOrd:
    Ord
    + Clone
    + From<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Into<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Deref<Target = Arc<ValidPoolTransaction<Self::Transaction>>>
{
    /// The wrapper transaction type.
    type Transaction: PoolTransaction;
}

/// Helper macro to implement necessary conversions for `ParkedOrd` trait
/// Hepler宏用于实现对于`ParkedOrd`的必要实现
macro_rules! impl_ord_wrapper {
    ($name:ident) => {
        impl<T: PoolTransaction> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<T: PoolTransaction> Eq for $name<T> {}

        impl<T: PoolTransaction> PartialEq<Self> for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == Ordering::Equal
            }
        }

        impl<T: PoolTransaction> PartialOrd<Self> for $name<T> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl<T: PoolTransaction> Deref for $name<T> {
            type Target = Arc<ValidPoolTransaction<T>>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<T: PoolTransaction> ParkedOrd for $name<T> {
            type Transaction = T;
        }

        impl<T: PoolTransaction> From<Arc<ValidPoolTransaction<T>>> for $name<T> {
            fn from(value: Arc<ValidPoolTransaction<T>>) -> Self {
                Self(value)
            }
        }

        impl<T: PoolTransaction> From<$name<T>> for Arc<ValidPoolTransaction<T>> {
            fn from(value: $name<T>) -> Arc<ValidPoolTransaction<T>> {
                value.0
            }
        }
    };
}

/// A new type wrapper for [`ValidPoolTransaction`]
///
/// This sorts transactions by their base fee.
/// 这通过它们的base fee对txs进行排序
///
/// Caution: This assumes all transaction in the `BaseFee` sub-pool have a fee value.
/// 注意：这假设`BaseFee` sub-pool中的所有tx都有fee value
#[derive(Debug)]
pub(crate) struct BasefeeOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(BasefeeOrd);

impl<T: PoolTransaction> Ord for BasefeeOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.transaction.max_fee_per_gas().cmp(&other.0.transaction.max_fee_per_gas())
    }
}

/// A new type wrapper for [`ValidPoolTransaction`]
/// 一个新的type wrapper，对于[`ValidPoolTransaction`]
///
/// This sorts transactions by their distance.
/// 通过distance对txs进行排序
///
/// `Queued` transactions are transactions that are currently blocked by other parked (basefee,
/// queued) or missing transactions.
/// `Queued` txs是那些被其他parked (basefee, queue)或者缺失txs阻塞的txs
///
/// The primary order function always compares the transaction costs first. In case these
/// are equal, it compares the timestamps when the transactions were created.
/// primary order函数总是首先比较tx costs，如果相等，则比较txs创建的时间戳
#[derive(Debug)]
pub(crate) struct QueuedOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(QueuedOrd);

// TODO: temporary solution for ordering the queued pool.
impl<T: PoolTransaction> Ord for QueuedOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher price is better
        // 更高的price更好
        self.max_fee_per_gas().cmp(&self.max_fee_per_gas()).then_with(||
            // Lower timestamp is better
            // 更小的时间戳更好
            other.timestamp.cmp(&self.timestamp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockTransaction, MockTransactionFactory};

    #[test]
    fn test_enforce_parked_basefee() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let tx = f.validated_arc(MockTransaction::eip1559().inc_price());
        pool.add_transaction(tx.clone());

        assert!(pool.by_id.contains_key(tx.id()));
        assert_eq!(pool.len(), 1);

        let removed = pool.enforce_basefee(u64::MAX);
        assert!(removed.is_empty());

        let removed = pool.enforce_basefee((tx.max_fee_per_gas() - 1) as u64);
        assert_eq!(removed.len(), 1);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_enforce_parked_basefee_descendant() {
        let mut f = MockTransactionFactory::default();
        let mut pool = ParkedPool::<BasefeeOrd<_>>::default();
        let t = MockTransaction::eip1559().inc_price_by(10);
        let root_tx = f.validated_arc(t.clone());
        // 添加tx
        pool.add_transaction(root_tx.clone());

        // 添加后代tx
        let descendant_tx = f.validated_arc(t.inc_nonce().inc_price());
        pool.add_transaction(descendant_tx.clone());

        assert!(pool.by_id.contains_key(root_tx.id()));
        assert!(pool.by_id.contains_key(descendant_tx.id()));
        assert_eq!(pool.len(), 2);

        let removed = pool.enforce_basefee(u64::MAX);
        assert!(removed.is_empty());

        // two dependent tx in the pool with decreasing fee
        // 两个依赖的tx，在pool中，有着降低的fee

        {
            let mut pool2 = pool.clone();
            let removed = pool2.enforce_basefee(descendant_tx.max_fee_per_gas() as u64);
            assert_eq!(removed.len(), 1);
            assert_eq!(pool2.len(), 1);
            // descendant got popped
            // 后代被popped
            assert!(pool2.by_id.contains_key(root_tx.id()));
            assert!(!pool2.by_id.contains_key(descendant_tx.id()));
        }

        // remove root transaction via root tx fee
        // 用root tx fee移除root tx
        let removed = pool.enforce_basefee(root_tx.max_fee_per_gas() as u64);
        assert_eq!(removed.len(), 2);
        assert!(pool.is_empty());
    }
}
