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
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct ExecInput {
    /// The target block number the stage needs to execute towards.
    /// stage需要执行到的target block number
    pub target: Option<BlockNumber>,
    /// The checkpoint of this stage the last time it was executed.
    /// 这个stage上一次执行的checkpoint
    pub checkpoint: Option<StageCheckpoint>,
}

impl ExecInput {
    /// Return the checkpoint of the stage or default.
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
    /// 返回下一个要执行的block range
    /// Return pair of the block range and if this is final block range.
    /// 返回block range pair以及它们是否是final block range
    pub fn next_block_range_with_threshold(
        &self,
        threshold: u64,
    ) -> (RangeInclusive<BlockNumber>, bool) {
        let current_block = self.checkpoint();
        let start = current_block.block_number + 1;
        let target = self.target();

        let end = min(target, current_block.block_number.saturating_add(threshold));

        let is_final_range = end == target;
        (start..=end, is_final_range)
    }

    /// Return the next block range determined the number of transactions within it.
    /// 返回下一个block range，通过其中的number of transactions确定
    /// This function walks the the block indices until either the end of the range is reached or
    /// the number of transactions exceeds the threshold.
    /// 这个函数遍历block indices直到range的end达到了或者transactions的数目超过了threshold
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
        // 剩余的需要执行的transactions
        let all_tx_cnt = target_block_body.next_tx_num() - first_tx_num;

        if all_tx_cnt == 0 {
            // if there is no more transaction return back.
            // 如果没有更多的transaction，直接返回
            return Ok((first_tx_num..first_tx_num, start_block..=target_block, true))
        }

        // get block of this tx
        // 获取这个tx的block
        let (end_block, is_final_range, next_tx_num) = if all_tx_cnt <= tx_threshold {
            (target_block, true, target_block_body.next_tx_num())
        } else {
            // get tx block number. next_tx_num in this case will be less thean all_tx_cnt.
            // So we are sure that transaction must exist.
            // 获取tx block
            // number，这里的next_tx_num会小于all_tx_cnt，因此我们确保transaction必须存在
            let end_block_number = provider
                .transaction_block(first_tx_num + tx_threshold)?
                .expect("block of tx must exist");
            // we want to get range of all transactions of this block, so we are fetching block
            // body.
            // 我们想要这个block的range of all transactions，因此我们正在抓取block body
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
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct UnwindInput {
    /// The current highest checkpoint of the stage.
    /// 这个stage当前最高的checkpoint
    pub checkpoint: StageCheckpoint,
    /// The block to unwind to.
    /// 要unwind到的block
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
    /// 返回我们要unwind的下一个block range以及我们正在unwinding的block
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
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnwindOutput {
    /// The checkpoint at which the stage has unwound to.
    pub checkpoint: StageCheckpoint,
}

/// A stage is a segmented part of the syncing process of the node.
/// 一个stage是一个node的同步过程的segmented part
///
/// Each stage takes care of a well-defined task, such as downloading headers or executing
/// transactions, and persist their results to a database.
/// 每个stage负责一个定义好的task，例如下载headers或者执行transactions，并且持久化结果到一个数据库
///
/// Stages must have a unique [ID][StageId] and implement a way to "roll forwards"
/// ([Stage::execute]) and a way to "roll back" ([Stage::unwind]).
///
/// Stages are executed as part of a pipeline where they are executed serially.
/// Stages作为一个pipeline的一部分执行，他们的执行是线性的
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
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
