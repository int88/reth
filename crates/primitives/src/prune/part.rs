use derive_more::Display;
use reth_codecs::{main_codec, Compact};
use thiserror::Error;

/// Part of the data that can be pruned.
/// 内部清单的部分的数据
#[main_codec]
#[derive(Debug, Display, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PrunePart {
    /// Prune part responsible for the `TxSenders` table.
    SenderRecovery,
    /// Prune part responsible for the `TxHashNumber` table.
    TransactionLookup,
    /// Prune part responsible for all `Receipts`.
    Receipts,
    /// Prune part responsible for some `Receipts` filtered by logs.
    ContractLogs,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    AccountHistory,
    /// Prune part responsible for the `StorageChangeSet` and `StorageHistory` tables.
    StorageHistory,
}

/// PrunePart error type.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrunePartError {
    /// Invalid configuration of a prune part.
    /// 对于一个prune part的非法配置
    #[error("The configuration provided for {0} is invalid.")]
    Configuration(PrunePart),
}

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
