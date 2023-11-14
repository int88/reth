use crate::error::StageError;
use async_trait::async_trait;
use reth_db::database::Database;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, TxNumber,
};
use reth_provider::{BlockReader, DatabaseProviderRW, ProviderError, TransactionsProvider};
use std::{
    cmp::{max, min},
    ops::{Range, RangeInclusive},
};

/// Stage execution input, see [Stage::execute].
/// Stage execution的输入
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    /// 需要执行前往的target block number
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    /// 上次执行的stage的checkpoint
    pub checkpoint: Option<StageCheckpoint>,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
    /// 返回这个stage的checkpoint或者默认值
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
    /// 返回`true`，如果target block number已经到了
    pub fn target_reached(&self) -> bool {
        self.checkpoint().block_number >= self.target()
    }

    /// Return the target block number or default.
    /// 返回target block number或者默认值
    pub fn target(&self) -> BlockNumber {
        self.target.unwrap_or_default()
    }

    /// Return next block range that needs to be executed.
    /// 返回需要被执行的下一个block range
    pub fn next_block_range(&self) -> RangeInclusive<BlockNumber> {
        // 忽略第二个返回值，表明是否是final range
        let (range, _) = self.next_block_range_with_threshold(u64::MAX);
        range
    }

    /// Return true if this is the first block range to execute.
    /// 返回true，如果这是执行的第一个block range
    pub fn is_first_range(&self) -> bool {
        self.checkpoint.is_none()
    }

    /// Return the next block range to execute.
    /// 返回下一个block range执行
    /// Return pair of the block range and if this is final block range.
    /// 返回pairt of block range并且是否是final block range
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        // 如果end和target一致，说明的final range
        (start..=end, is_final_range)
    }

    /// Return the next block range determined the number of transactions within it.
    /// This function walks the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    pub fn next_block_range_with_transaction_threshold<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        tx_threshold: u64,
    ) -> Result<(Range<TxNumber>, RangeInclusive<BlockNumber>, bool), StageError> {
        let start_block = self.next_block();
        let target_block = self.target();

        let start_block_body = provider
            .block_body_indices(start_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(start_block))?;
        let first_tx_num = start_block_body.first_tx_num();

        let target_block_body = provider
            .block_body_indices(target_block)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(target_block))?;

        // number of transactions left to execute.
        let all_tx_cnt = target_block_body.next_tx_num() - first_tx_num;

        if all_tx_cnt == 0 {
            // if there is no more transaction return back.
            return Ok((first_tx_num..first_tx_num, start_block..=target_block, true))
        }

        // get block of this tx
        let (end_block, is_final_range, next_tx_num) = if all_tx_cnt <= tx_threshold {
            (target_block, true, target_block_body.next_tx_num())
        } else {
            // get tx block number. next_tx_num in this case will be less thean all_tx_cnt.
            // So we are sure that transaction must exist.
            let end_block_number = provider
                .transaction_block(first_tx_num + tx_threshold)?
                .expect("block of tx must exist");
            // we want to get range of all transactions of this block, so we are fetching block
            // body.
            let end_block_body = provider
                .block_body_indices(end_block_number)?
                .ok_or(ProviderError::BlockBodyIndicesNotFound(target_block))?;
            (end_block_number, false, end_block_body.next_tx_num())
        };

        let tx_range = first_tx_num..next_tx_num;
        Ok((tx_range, start_block..=end_block, is_final_range))
    }
}

/// Stage unwind input, see [Stage::unwind].
/// Stage unwind的输入
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    /// stage当前最高的checkpoint
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    /// unwind到的block
    pub unwind_to: BlockNumber,
    /// The bad block that caused the unwind, if any.
    /// 导致unwind的bad block，如果有的话
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
/// stage执行的output
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecOutput {
    /// How far the stage got.
    /// stage走到多远了
    pub checkpoint: StageCheckpoint,
    /// Whether or not the stage is done.
    /// stage是否结束
    pub done: bool,
}

impl ExecOutput {
    /// Mark the stage as done, checkpointing at the given place.
    /// 将stage标记为done，在给定地方checkpointing
    pub fn done(checkpoint: StageCheckpoint) -> Self {
        Self { checkpoint, done: true }
    }
}

/// The output of a stage unwinding.
/// 一个stage unwind的结果
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    /// stage已经unwind to到的chekpoint
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
/// 一个stage是node的syncing process的segmented part
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
/// 每个stage负责一个well-defined
/// task，例如下载headers或者执行transactions，以及持久化他们的结果到db中
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
/// Stages必须有一个唯一的[ID][StageId]并且实现一种方式来“roll
/// forwards”([Stage::execute])以及一种方式来“roll back”([Stage::unwind])
///
/// Stages are executed as part of a pipeline where they are executed serially.
/// Stages作为一个pipeline执行的一部分，他们在其中顺序执行
///
/// Stages receive [`DatabaseProviderRW`].
/// Stages接受[`DatabaseProviderRW`]
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    /// 获取stage的ID
    ///
    /// Stage IDs must be unique.
    /// Stage IDs必须是唯一的
    fn id(&self) -> StageId;

    /// Execute the stage.
    /// 执行stage
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    /// 对stage进行unwind
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
