use crate::traits::PoolTransaction;
use reth_primitives::U256;
use std::{fmt, marker::PhantomData};

/// Priority of the transaction that can be missing.
/// tx的优先级可以丢失
///
/// Transactions with missing priorities are ranked lower.
/// 缺失priorities的txs会被标记得更低
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
pub enum Priority<T: Ord + Clone> {
    /// The value of the priority of the transaction.
    /// tx的优先级的值
    Value(T),
    /// Missing priority due to ordering internals.
    /// 因为ordering intervals，缺失优先级
    None,
}

impl<T: Ord + Clone> From<Option<T>> for Priority<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(val) => Priority::Value(val),
            None => Priority::None,
        }
    }
}

/// Transaction ordering trait to determine the order of transactions.
/// tx的排序trait，来决定txs的顺序
///
/// Decides how transactions should be ordered within the pool, depending on a `Priority` value.
/// 决定txs如何在pool中排序，取决于`Priority`的值
///
/// The returned priority must reflect [total order](https://en.wikipedia.org/wiki/Total_order).
pub trait TransactionOrdering: Send + Sync + 'static {
    /// Priority of a transaction.
    /// tx的一个priority
    ///
    /// Higher is better.
    type PriorityValue: Ord + Clone + Default + fmt::Debug + Send + Sync;

    /// The transaction type to determine the priority of.
    /// 决定priority的tx类型
    type Transaction: PoolTransaction;

    /// Returns the priority score for the given transaction.
    /// 返回给定的tx的priority score
    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue>;
}

/// Default ordering for the pool.
/// pool的默认排序
///
/// The transactions are ordered by their coinbase tip.
/// txs根据它们的coinbase tip排序
/// The higher the coinbase tip is, the higher the priority of the transaction.
/// coinbase tip越高，tx的priority越高
#[derive(Debug)]
#[non_exhaustive]
pub struct CoinbaseTipOrdering<T>(PhantomData<T>);

impl<T> TransactionOrdering for CoinbaseTipOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = U256;
    type Transaction = T;

    /// Source: <https://github.com/ethereum/go-ethereum/blob/7f756dc1185d7f1eeeacb1d12341606b7135f9ea/core/txpool/legacypool/list.go#L469-L482>.
    ///
    /// NOTE: The implementation is incomplete for missing base fee.
    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        transaction.effective_tip_per_gas(base_fee).map(U256::from).into()
    }
}

impl<T> Default for CoinbaseTipOrdering<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T> Clone for CoinbaseTipOrdering<T> {
    fn clone(&self) -> Self {
        Self::default()
    }
}
