//! Support for pruning.

use crate::PrunerError;
use reth_primitives::BlockNumber;
use tracing::debug;

/// Result of [Pruner::run] execution
/// [Pruner::run]执行的结果
pub type PrunerResult = Result<(), PrunerError>;

/// The pipeline type itself with the result of [Pruner::run]
/// pipeline类型，结果为[Pruner::run]
pub type PrunerWithResult = (Pruner, PrunerResult);

/// Pruning routine. Main pruning logic happens in [Pruner::run].
pub struct Pruner {
    /// Minimum pruning interval measured in blocks. All prune parts are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    /// blocks中测量的最小的pruning interval，所有的prune
    /// parts被检查，如果需要的话，pruned，当chain移动 特定的blocks数目
    min_block_interval: u64,
    /// Maximum prune depth. Used to determine the pruning target for parts that are needed during
    /// the reorg, e.g. changesets.
    /// 最大的prune depth，用于决定pruning parts，对于在reorg中需要的部分
    #[allow(dead_code)]
    max_prune_depth: u64,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    /// 最后被pruned block number，用于`min_block_interval`来决定什么时候pruning需要被初始化
    last_pruned_block_number: Option<BlockNumber>,
}

impl Pruner {
    /// Creates a new [Pruner].
    /// 创建一个新的[Prunner]
    pub fn new(min_block_interval: u64, max_prune_depth: u64) -> Self {
        Self { min_block_interval, max_prune_depth, last_pruned_block_number: None }
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        // Pruning logic

        self.last_pruned_block_number = Some(tip_block_number);
        Ok(())
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// 返回`true`，如果pruning在提供的tip block number上是需要的
    /// This determined by the check against minimum pruning interval and last pruned block number.
    /// 这取决于警察minimum pruning interval以及last pruned block number
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        if self.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previously pruned block number. If
            // that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(last_pruned_block_number) >= self.min_block_interval
        }) {
            debug!(
                target: "pruner",
                // 打印last pruned block number
                last_pruned_block_number = ?self.last_pruned_block_number,
                %tip_block_number,
                "Minimum pruning interval reached"
            );
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;

    #[test]
    fn pruner_is_pruning_needed() {
        let pruner = Pruner::new(5, 0);

        // No last pruned block number was set before
        // 之前没有设置last pruned block number
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));

        // Delta is not less than min block interval
        // Delta没有小于min block interval
        let second_block_number = first_block_number + pruner.min_block_interval;
        assert!(pruner.is_pruning_needed(second_block_number));

        // Delta is less than min block interval
        // Delta小于min block interval
        let third_block_number = second_block_number;
        assert!(pruner.is_pruning_needed(third_block_number));
    }
}
