use crate::{
    prune::PrunePartError, serde_helper::deserialize_opt_prune_mode_with_min_blocks, BlockNumber,
    PruneMode, PrunePart, ReceiptsLogPruneConfig,
};
use paste::paste;
use serde::{Deserialize, Serialize};

/// Minimum distance necessary from the tip so blockchain tree can work correctly.
/// 从tip开始必要的minimuu distance，这样blockchain tree可以work
pub const MINIMUM_PRUNING_DISTANCE: u64 = 128;

/// Pruning configuration for every part of the data that can be pruned.
/// Pruning配置，对于每个可用被pruned部分的数据
#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneModes {
    /// Sender Recovery pruning configuration.
    /// Sender Recovery的pruning配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_recovery: Option<PruneMode>,
    /// Transaction Lookup pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_lookup: Option<PruneMode>,
    /// Receipts pruning configuration. This setting overrides `receipts_log_filter`
    /// and offers improved performance.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    /// Account History的清理配置
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    /// Storage History的清理配置
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub storage_history: Option<PruneMode>,
    /// Receipts pruning configuration by retaining only those receipts that contain logs emitted
    /// by the specified addresses, discarding others. This setting is overridden by `receipts`.
    /// Receipts清理配置，通过只保留那些特定地址发出的logs的receipts，丢弃其他的，
    /// 这个settigns被`receipts`覆盖
    ///
    /// The [`BlockNumber`] represents the starting block from which point onwards the receipts are
    /// preserved.
    /// [`BlockNumber`]代表starting block，从这开始，receipts被保留
    pub receipts_log_filter: ReceiptsLogPruneConfig,
}

macro_rules! impl_prune_parts {
    ($(($part:ident, $variant:ident, $min_blocks:expr)),+) => {
        $(
            paste! {
                #[doc = concat!(
                    "Check if ",
                    stringify!($variant),
                    " should be pruned at the target block according to the provided tip."
                )]
                pub fn [<should_prune_ $part>](&self, block: BlockNumber, tip: BlockNumber) -> bool {
                    if let Some(mode) = &self.$part {
                        return mode.should_prune(block, tip)
                    }
                    false
                }
            }
        )+

        $(
            paste! {
                #[doc = concat!(
                    "Returns block up to which ",
                    stringify!($variant),
                    " pruning needs to be done, inclusive, according to the provided tip."
                )]
                pub fn [<prune_target_block_ $part>](&self, tip: BlockNumber) -> Result<Option<(BlockNumber, PruneMode)>, PrunePartError> {
                     match self.$part {
                        Some(mode) => mode.prune_target_block(tip, $min_blocks.unwrap_or_default(), PrunePart::$variant),
                        None => Ok(None)
                    }
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            Self {
                $(
                    $part: Some(PruneMode::Full),
                )+
                receipts_log_filter: Default::default()
            }
        }

    };
}

impl PruneModes {
    /// Sets pruning to no target.
    /// 设置pruning为no target
    pub fn none() -> Self {
        PruneModes::default()
    }

    impl_prune_parts!(
        (sender_recovery, SenderRecovery, None),
        (transaction_lookup, TransactionLookup, None),
        (receipts, Receipts, Some(64)),
        (account_history, AccountHistory, Some(64)),
        (storage_history, StorageHistory, Some(64))
    );
}
