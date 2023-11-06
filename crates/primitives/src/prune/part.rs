use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Part of the data that can be pruned.
/// 可以被pruned的data部分
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PrunePart {
    /// Prune part responsible for the `TxSenders` table.
    /// 负责`TxSenders`table的prune
    SenderRecovery,
    /// Prune part responsible for the `TxHashNumber` table.
    /// 负责`TxHashNumber` table的prune
    TransactionLookup,
    /// Prune part responsible for all `Receipts`.
    /// 对于所有`Receipts`的prune负责
    Receipts,
    /// Prune part responsible for some `Receipts` filtered by logs.
    ContractLogs,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    /// 负责`AccountChangeSet`和`AccountHistory` tables的prune
    AccountHistory,
    /// Prune part responsible for the `StorageChangeSet` and `StorageHistory` tables.
    /// 负责`StorageChangeSet`和`StorageHash` tables的prune
    StorageHistory,
}

/// PrunePart error type.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum PrunePartError {
    /// Invalid configuration of a prune part.
    #[error("The configuration provided for {0} is invalid.")]
    Configuration(PrunePart),
}

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
