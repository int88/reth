//! Support types for updating the pool.
//! 支持类型用于更新Pool

use crate::{identifier::TransactionId, pool::state::SubPool};
use alloy_primitives::TxHash;

/// A change of the transaction's location
/// 对于tx的location的改变
///
/// NOTE: this guarantees that `current` and `destination` differ.
/// 主意：这确保`current`和`destination`不同
#[derive(Debug)]
pub(crate) struct PoolUpdate {
    /// Internal tx id.
    pub(crate) id: TransactionId,
    /// Hash of the transaction.
    pub(crate) hash: TxHash,
    /// Where the transaction is currently held.
    /// tx当前所在的subpool
    pub(crate) current: SubPool,
    /// Where to move the transaction to.
    /// tx移动的目的地
    pub(crate) destination: Destination,
}

/// Where to move an existing transaction.
/// 移动一个已经存在的tx到什么地方
#[derive(Debug)]
pub(crate) enum Destination {
    /// Discard the transaction.
    /// 丢弃tx
    Discard,
    /// Move transaction to pool
    /// 移动tx到pool
    Pool(SubPool),
}

impl From<SubPool> for Destination {
    fn from(sub_pool: SubPool) -> Self {
        Self::Pool(sub_pool)
    }
}
