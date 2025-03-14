use crate::{traits::PropagateKind, PoolTransaction, ValidPoolTransaction};
use alloy_primitives::{TxHash, B256};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// An event that happened to a transaction and contains its full body where possible.
/// 一个event发生在一个tx并且包含它的full body，如果可能的话
#[derive(Debug)]
pub enum FullTransactionEvent<T: PoolTransaction> {
    /// Transaction has been added to the pending pool.
    /// Tx已经被添加到了pending pool
    Pending(TxHash),
    /// Transaction has been added to the queued pool.
    Queued(TxHash),
    /// Transaction has been included in the block belonging to this hash.
    /// tx已经被包含进了属于这个hash的block
    Mined {
        /// The hash of the mined transaction.
        tx_hash: TxHash,
        /// The hash of the mined block that contains the transaction.
        /// mined block的hash，包含这个tx
        block_hash: B256,
    },
    /// Transaction has been replaced by the transaction belonging to the hash.
    /// tx已经被替换，通过属于这个hash的tx
    ///
    /// E.g. same (sender + nonce) pair
    Replaced {
        /// The transaction that was replaced.
        transaction: Arc<ValidPoolTransaction<T>>,
        /// The transaction that replaced the event subject.
        replaced_by: TxHash,
    },
    /// Transaction was dropped due to configured limits.
    /// tx被丢弃，因为配置的limits
    Discarded(TxHash),
    /// Transaction became invalid indefinitely.
    /// tx变得永远非法
    Invalid(TxHash),
    /// Transaction was propagated to peers.
    /// tx被传播到其他的peers
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
/// 各种事件，描述一个tx的status changes
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum TransactionEvent {
    /// Transaction has been added to the pending pool.
    /// tx被添加进了pending pool
    Pending,
    /// Transaction has been added to the queued pool.
    Queued,
    /// Transaction has been included in the block belonging to this hash.
    Mined(B256),
    /// Transaction has been replaced by the transaction belonging to the hash.
    /// tx已经被属于这个hash的tx替换
    ///
    /// E.g. same (sender + nonce) pair
    Replaced(TxHash),
    /// Transaction was dropped due to configured limits.
    /// tx因为给定的limits被丢弃
    Discarded,
    /// Transaction became invalid indefinitely.
    /// tx变得永远非法了
    Invalid,
    /// Transaction was propagated to peers.
    /// tx被传播给了Peers
    Propagated(Arc<Vec<PropagateKind>>),
}

impl TransactionEvent {
    /// Returns `true` if the event is final and no more events are expected for this transaction
    /// hash.
    pub const fn is_final(&self) -> bool {
        matches!(self, Self::Replaced(_) | Self::Mined(_) | Self::Discarded)
    }
}
