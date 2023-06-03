use crate::error::StageError;
use async_trait::async_trait;
use reth_db::database::Database;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};
use reth_provider::Transaction;
use std::{
    cmp::{max, min},
    ops::RangeInclusive,
};

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The stage that was run before the current stage and the progress it reached.
    pub previous_stage: Option<(StageId, StageCheckpoint)>,
    /// The progress of this stage the last time it was executed.
    pub checkpoint: Option<StageCheckpoint>,
}

impl ExecInput {
    /// Return the progress of the stage or default.
    pub fn checkpoint(&self) -> StageCheckpoint {
        self.checkpoint.unwrap_or_default()
    }

    /// Return the progress of the previous stage or default.
    pub fn previous_stage_checkpoint(&self) -> StageCheckpoint {
        self.previous_stage.map(|(_, checkpoint)| checkpoint).unwrap_or_default()
    }

    /// Return next block range that needs to be executed.
    pub fn next_block_range(&self) -> RangeInclusive<BlockNumber> {
        let (range, _) = self.next_block_range_with_threshold(u64::MAX);
        range
    }

    /// Return true if this is the first block range to execute.
    pub fn is_first_range(&self) -> bool {
        self.checkpoint.is_none()
    }

    /// Return the next block range to execute.
    /// Return pair of the block range and if this is final block range.
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        // +1 is to skip present block and always start from block number 1, not 0.
        let start = current_block.block_number + 1;
        let target = self.previous_stage_checkpoint().block_number;

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }
}

/// Stage unwind input, see [Stage::unwind].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest progress of the stage.
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    pub unwind_to: BlockNumber,
    /// The bad block that caused the unwind, if any.
    pub bad_block: Option<BlockNumber>,
}

impl UnwindInput {
    /// Return next block range that needs to be unwound.
    pub fn unwind_block_range(&self) -> RangeInclusive<BlockNumber> {
        self.unwind_block_range_with_threshold(u64::MAX).0
    }

    /// Return the next block range to unwind and the block we're unwinding to.
    pub fn unwind_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, BlockNumber, bool) {
        // +1 is to skip the block we're unwinding to
        let mut start = self.unwind_to + 1;
        let end = self.checkpoint;

        start = max(start, end.block_number.saturating_sub(threshold));

        let unwind_to = start - 1;

        let is_final_range = unwind_to == self.unwind_to;
        (start..=end.block_number, unwind_to, is_final_range)
    }
}

/// The output of a stage execution.
/// 一个stage execution的输出
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    /// stage走了多远
    pub checkpoint: StageCheckpoint,
    /// Whether or not the stage is done.
    /// stage是否完成
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given place.
    pub fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: true }
    }
}

/// The output of a stage unwinding.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The block at which the stage has unwound to.
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
/// 一个stage是节点同步过程的一个segment part
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
/// 每个stage负责一个明确定义的任务，例如下载headers或执行transactions，并将其结果持久化到数据库中
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
/// Stages必须有一个唯一的ID和实现一个"roll forwards"的方法和一个"roll back"的方法
///
/// Stages are executed as part of a pipeline where they are executed serially.
/// Stages作为一个pipeline的一部分被执行，它们被串行执行
///
/// Stages receive [`Transaction`] which manages the lifecycle of a transaction,
/// such as when to commit / reopen a new one etc.
/// Stages接收到Transaction，它管理一个transaction的生命周期，例如何时commit/reopen一个新的transaction等
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    /// 执行这个stage
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    /// 解开这个stage
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
