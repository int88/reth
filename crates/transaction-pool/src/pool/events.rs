use crate::{traits::PropagateKind, PoolTransaction, ValidPoolTransaction};
use reth_primitives::{TxHash, H256};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An event that happened to a transaction and contains its full body where possible.
/// 一个event，发生到一个tx并且包含完整的body，如果可能的话
#[derive(Debug)]
pub enum FullTransactionEvent<T: PoolTransaction> {
    /// Transaction has been added to the pending pool.
    Pending(TxHash),
    /// Transaction has been added to the queued pool.
    Queued(TxHash),
    /// Transaction has been included in the block belonging to this hash.
    Mined {
        /// The hash of the mined transaction.
        tx_hash: TxHash,
        /// The hash of the mined block that contains the transaction.
        block_hash: H256,
    },
    /// Transaction has been replaced by the transaction belonging to the hash.
    ///
    /// E.g. same (sender + nonce) pair
    Replaced {
        /// The transaction that was replaced.
        transaction: Arc<ValidPoolTransaction<T>>,
        /// The transaction that replaced the event subject.
        replaced_by: TxHash,
    },
    /// Transaction was dropped due to configured limits.
    Discarded(TxHash),
    /// Transaction became invalid indefinitely.
    Invalid(TxHash),
    /// Transaction was propagated to peers.
    Propagated(Arc<Vec<PropagateKind>>),
}

impl<T: PoolTransaction> Clone for FullTransactionEvent<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Pending(hash) => Self::Pending(*hash),
            Self::Queued(hash) => Self::Queued(*hash),
            Self::Mined { tx_hash, block_hash } => {
                Self::Mined { tx_hash: *tx_hash, block_hash: *block_hash }
            }
            Self::Replaced { transaction, replaced_by } => {
                Self::Replaced { transaction: Arc::clone(transaction), replaced_by: *replaced_by }
            }
            Self::Discarded(hash) => Self::Discarded(*hash),
            Self::Invalid(hash) => Self::Invalid(*hash),
            Self::Propagated(propagated) => Self::Propagated(Arc::clone(propagated)),
        }
    }
}

/// Various events that describe status changes of a transaction.
/// 各种events，描述一个tx的状态变更
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TransactionEvent {
    /// Transaction has been added to the pending pool.
    Pending,
    /// Transaction has been added to the queued pool.
    Queued,
    /// Transaction has been included in the block belonging to this hash.
    /// Tx被包含进block，属于这个hash
    Mined(H256),
    /// Transaction has been replaced by the transaction belonging to the hash.
    /// Tx已经被属于这个hash的tx替换
    ///
    /// E.g. same (sender + nonce) pair
    Replaced(TxHash),
    /// Transaction was dropped due to configured limits.
    /// tx因为配置的limit被丢弃
    Discarded,
    /// Transaction became invalid indefinitely.
    /// tx变为永久非法
    Invalid,
    /// Transaction was propagated to peers.
    /// tx被传播到了peers
    Propagated(Arc<Vec<PropagateKind>>),
}

impl TransactionEvent {
    /// Returns `true` if the event is final and no more events are expected for this transaction
    /// hash.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            TransactionEvent::Replaced(_) |
                TransactionEvent::Mined(_) |
                TransactionEvent::Discarded
        )
    }
}
