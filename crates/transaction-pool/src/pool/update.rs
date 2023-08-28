//! Support types for updating the pool.
use crate::{identifier::TransactionId, pool::state::SubPool};
use reth_primitives::TxHash;

/// A change of the transaction's location
/// tx的location的改变
///
/// NOTE: this guarantees that `current` and `destination` differ.
/// 注意：这确保`current`和`destination`不同
#[derive(Debug)]
pub(crate) struct PoolUpdate {
    /// Internal tx id.
    /// 内部的tx id
    pub(crate) id: TransactionId,
    /// Hash of the transaction.
    pub(crate) hash: TxHash,
    /// Where the transaction is currently held.
    /// 当前tx所处的subpool
    pub(crate) current: SubPool,
    /// Where to move the transaction to.
    /// 这个tx被移动到哪里
    pub(crate) destination: Destination,
}

/// Where to move an existing transaction.
/// 移动一个已经存在的tx到哪里
#[derive(Debug)]
pub(crate) enum Destination {
    /// Discard the transaction.
    /// 丢弃tx
    Discard,
    /// Move transaction to pool
    /// 移动tx到pool
    Pool(SubPool),
}
