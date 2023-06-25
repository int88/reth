use crate::error::StageError;
use async_trait::async_trait;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, TxNumber,
};
use reth_provider::DatabaseProviderRW;
use std::{
    cmp::{max, min},
    ops::RangeInclusive,
};

/// Stage execution input, see [Stage::execute].
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    /// stage需要执行到的target block number
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    /// 这个stage上一次执行的检查点
    pub checkpoint: Option<StageCheckpoint>,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
    /// 返回stage的checkpoint或者默认值
    pub fn checkpoint(&self) -> StageCheckpoint {
        self.checkpoint.unwrap_or_default()
    }

    /// Return the next block number after the current
    /// +1 is needed to skip the present block and always start from block number 1, not 0.
    pub fn next_block(&self) -> BlockNumber {
        let current_block = self.checkpoint();
        current_block.block_number + 1
    }

    /// Returns `true` if the target block number has already been reached.
    /// 返回`true`如果目标块号已经被到达
    pub fn target_reached(&self) -> bool {
        self.checkpoint().block_number >= self.target()
    }

    /// Return the target block number or default.
    pub fn target(&self) -> BlockNumber {
        self.target.unwrap_or_default()
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
    /// 返回下一个需要执行的块范围
    /// Return pair of the block range and if this is final block range.
    /// 返回pair of the block range以及它是否是最后的块范围
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        // 如果end等于target，说明到了final
        let is_final_range = end == target;
        (start..=end, is_final_range)
    }

    /// Return the next block range determined the number of transactions within it.
    /// This function walks the the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    pub fn next_block_range_with_transaction_threshold<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        tx_threshold: u64,
    ) -> Result<(RangeInclusive<TxNumber>, RangeInclusive<BlockNumber>, bool), StageError> {
        let start_block = self.next_block();
        let start_block_body = provider.block_body_indices(start_block)?;

        let target_block = self.target();

        let first_tx_number = start_block_body.first_tx_num();
        let mut last_tx_number = start_block_body.last_tx_num();
        let mut end_block_number = start_block;
        let mut body_indices_cursor =
            provider.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;
        for entry in body_indices_cursor.walk_range(start_block..=target_block)? {
            let (block, body) = entry?;
            last_tx_number = body.last_tx_num();
            end_block_number = block;
            let tx_count = (first_tx_number..=last_tx_number).count() as u64;
            if tx_count > tx_threshold {
                break
            }
        }
        let is_final_range = end_block_number >= target_block;
        Ok((first_tx_number..=last_tx_number, start_block..=end_block_number, is_final_range))
    }
}

/// Stage unwind input, see [Stage::unwind].
/// Stage unwind的输入
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    /// 当前最高的stage检查点
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    /// unwind到的block
    pub unwind_to: BlockNumber,
    /// The bad block that caused the unwind, if any.
    /// 导致unwind的坏块，如果有的话
    pub bad_block: Option<BlockNumber>,
}

impl UnwindInput {
    /// Return next block range that needs to be unwound.
    pub fn unwind_block_range(&self) -> RangeInclusive<BlockNumber> {
        self.unwind_block_range_with_threshold(u64::MAX).0
    }

    /// Return the next block range to unwind and the block we're unwinding to.
    /// 返回下一个要unwind的block range以及我们准备unwind到的block
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
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    pub checkpoint: StageCheckpoint,
    /// Whether or not the stage is done.
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given place.
    pub fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: true }
    }
}

/// The output of a stage unwinding.
/// 一个stage unwinding的输出
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    /// stage unwind到的checkpoint
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
///
/// Stages receive [`DatabaseProviderRW`].
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
