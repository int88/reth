use crate::{prune::PruneMode, BlockNumber};
use reth_codecs::{main_codec, Compact};

/// Saves the pruning progress of a stage.
/// 保存一个stage的pruning进度
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(test, derive(Default))]
pub struct PruneCheckpoint {
    /// Highest pruned block number.
    /// 最高被pruned block number
    block_number: BlockNumber,
    /// Prune mode.
    /// Prune模式
    prune_mode: PruneMode,
}
