use crate::{prune::PruneMode, BlockNumber, TxNumber};
use reth_codecs::{main_codec, Compact};

/// Saves the pruning progress of a stage.
/// 维护一个stage的pruning进程
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[cfg_attr(test, derive(Default))]
pub struct PruneCheckpoint {
    /// Highest pruned block number.
    /// 最高的被pruned的block number
    /// If it's [None], the pruning for block `0` is not finished yet.
    /// 如果为[Node]，则block `0`的pruning还没有完成
    pub block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    /// 最高的pruned tx number，如果适用的话
    pub tx_number: Option<TxNumber>,
    /// Prune mode.
    pub prune_mode: PruneMode,
}
