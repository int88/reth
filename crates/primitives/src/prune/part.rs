use reth_codecs::{main_codec, Compact};

/// Part of the data that can be pruned.
/// 可以被清理的data的部分
#[main_codec]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum PrunePart {
    /// Prune part responsible for the `TxSenders` table.
    /// 负责`TxSenders` table的部分
    SenderRecovery,
    /// Prune part responsible for the `TxHashNumber` table.
    /// 负责`TxHashNumber` table的部分
    TransactionLookup,
    /// Prune part responsible for the `Receipts` table.
    /// 负责`Receipts` table的部分
    Receipts,
    /// Prune part responsible for the `AccountChangeSet` and `AccountHistory` tables.
    /// 负责`AccountChangeSet`和`AccountHistory` tables的部分
    AccountHistory,
    /// Prune part responsible for the `StorageChangeSet` and `StorageHistory` tables.
    /// 负责`StorageChangeSet`和`StorageHistory` tables的部分
    StorageHistory,
}

#[cfg(test)]
impl Default for PrunePart {
    fn default() -> Self {
        Self::SenderRecovery
    }
}
