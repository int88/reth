use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use futures_util::StreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    p2p::headers::downloader::{HeaderDownloader, SyncTarget},
    provider::ProviderError,
};
use reth_primitives::{
    stage::{EntitiesCheckpoint, StageCheckpoint, StageId},
    BlockHashOrNumber, BlockNumber, SealedHeader, H256,
};
use reth_provider::Transaction;
use tokio::sync::watch;
use tracing::*;

/// The header sync mode.
/// header的同步模式
#[derive(Debug)]
pub enum HeaderSyncMode {
    /// A sync mode in which the stage continuously requests the downloader for
    /// next blocks.
    /// 一种sync模式，在这个stage中，stage不断地请求downloader获取下一个block。
    Continuous,
    /// A sync mode in which the stage polls the receiver for the next tip
    /// to download from.
    /// 一种sync模式，在这个stage中，stage轮询receiver获取下一个tip来下载。
    Tip(watch::Receiver<H256>),
}

/// The headers stage.
///
/// The headers stage downloads all block headers from the highest block in the local database to
/// the perceived highest block on the network.
/// headers stage下载所有的block headers，从本地数据库中的最高block到网络中感知到的最高block。
///
/// The headers are processed and data is inserted into these tables:
/// headers被处理并且data被插入这些tables中：
///
/// - [`HeaderNumbers`][reth_db::tables::HeaderNumbers]
/// - [`Headers`][reth_db::tables::Headers]
/// - [`CanonicalHeaders`][reth_db::tables::CanonicalHeaders]
///
/// NOTE: This stage downloads headers in reverse. Upon returning the control flow to the pipeline,
/// the stage checkpoint is not updated until this stage is done.
/// 注意：这个stage反向下载headers。在将控制流返回到pipeline之后，stage checkpoint不会更新，直到这个stage完成。
#[derive(Debug)]
pub struct HeaderStage<D: HeaderDownloader> {
    /// Strategy for downloading the headers
    /// 下载headers的策略
    downloader: D,
    /// The sync mode for the stage.
    /// 这个stage的sync mode
    mode: HeaderSyncMode,
}

// === impl HeaderStage ===

impl<D> HeaderStage<D>
where
    D: HeaderDownloader,
{
    /// Create a new header stage
    /// 创建一个新的header stage
    pub fn new(downloader: D, mode: HeaderSyncMode) -> Self {
        Self { downloader, mode }
    }

    fn is_stage_done<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        checkpoint: u64,
    ) -> Result<bool, StageError> {
        let mut header_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        // 获取当年的header canonical number
        let (head_num, _) = header_cursor
            .seek_exact(checkpoint)?
            .ok_or_else(|| ProviderError::HeaderNotFound(checkpoint.into()))?;
        // Check if the next entry is congruent
        // 检查是否下一个entry是congruent的
        Ok(header_cursor.next()?.map(|(next_num, _)| head_num + 1 == next_num).unwrap_or_default())
    }

    /// Get the head and tip of the range we need to sync
    /// 获取head和tip，我们需要同步的range
    ///
    /// See also [SyncTarget]
    async fn get_sync_gap<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        checkpoint: u64,
    ) -> Result<SyncGap, StageError> {
        // Create a cursor over canonical header hashes
        // 创建一个cursor，遍历canonical header hashes
        let mut cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let mut header_cursor = tx.cursor_read::<tables::Headers>()?;

        // Get head hash and reposition the cursor
        // 获取header hash以及重新定位cursor
        let (head_num, head_hash) = cursor
            .seek_exact(checkpoint)?
            .ok_or_else(|| ProviderError::HeaderNotFound(checkpoint.into()))?;

        // Construct head
        // 构建head
        let (_, head) = header_cursor
            .seek_exact(head_num)?
            .ok_or_else(|| ProviderError::HeaderNotFound(head_num.into()))?;
        let local_head = head.seal(head_hash);

        // Look up the next header
        // 查找下一个header
        let next_header = cursor
            .next()?
            .map(|(next_num, next_hash)| -> Result<SealedHeader, StageError> {
                let (_, next) = header_cursor
                    .seek_exact(next_num)?
                    .ok_or_else(|| ProviderError::HeaderNotFound(next_num.into()))?;
                Ok(next.seal(next_hash))
            })
            .transpose()?;

        // Decide the tip or error out on invalid input.
        // 决定tip或者在无效的输入上报错。
        // If the next element found in the cursor is not the "expected" next block per our current
        // checkpoint, then there is a gap in the database and we should start downloading in
        // reverse from there. Else, it should use whatever the forkchoice state reports.
        // 如果在cursor中找到的下一个元素不是我们当前checkpoint的“预期”下一个block，那么数据库中就有一个gap，
        // 我们应该从那里开始反向下载。否则，它应该使用forkchoice state报告的任何内容。
        let target = match next_header {
            Some(header) if checkpoint + 1 != header.number => SyncTarget::Gap(header),
            None => self.next_sync_target(head_num).await,
            _ => return Err(StageError::StageCheckpoint(checkpoint)),
        };

        Ok(SyncGap { local_head, target })
    }

    async fn next_sync_target(&mut self, head: BlockNumber) -> SyncTarget {
        match self.mode {
            HeaderSyncMode::Tip(ref mut rx) => {
                loop {
                    let _ = rx.changed().await; // TODO: remove this await?
                    let tip = rx.borrow();
                    if !tip.is_zero() {
                        return SyncTarget::Tip(*tip)
                    }
                }
            }
            HeaderSyncMode::Continuous => {
                // 没有找到next header，使用continuous sync strategy
                tracing::trace!(target: "sync::stages::headers", head, "No next header found, using continuous sync strategy");
                SyncTarget::TipNum(head + 1)
            }
        }
    }

    /// Write downloaded headers to the given transaction
    /// 写入下载的headers到给定的transaction
    ///
    /// Note: this writes the headers with rising block numbers.
    /// 注意：写入headers，block number是上升的。
    fn write_headers<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        headers: Vec<SealedHeader>,
    ) -> Result<Option<BlockNumber>, StageError> {
        trace!(target: "sync::stages::headers", len = headers.len(), "writing headers");

        let mut cursor_header = tx.cursor_write::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor_write::<tables::CanonicalHeaders>()?;

        let mut latest = None;
        // Since the headers were returned in descending order,
        // iterate them in the reverse order
        for header in headers.into_iter().rev() {
            if header.number == 0 {
                continue
            }

            let header_hash = header.hash();
            let header_number = header.number;
            let header = header.unseal();
            latest = Some(header.number);

            // NOTE: HeaderNumbers are not sorted and can't be inserted with cursor.
            tx.put::<tables::HeaderNumbers>(header_hash, header_number)?;
            cursor_header.insert(header_number, header)?;
            cursor_canonical.insert(header_number, header_hash)?;
        }

        Ok(latest)
    }
}

#[async_trait::async_trait]
impl<DB, D> Stage<DB> for HeaderStage<D>
where
    DB: Database,
    D: HeaderDownloader,
{
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::Headers
    }

    /// Download the headers in reverse order (falling block numbers)
    /// starting from the tip of the chain
    /// 按照相反的顺序下载headers（下降的block numbers），从链的tip开始
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // 获取当前的checkpoint
        let current_checkpoint = input.checkpoint();

        // Lookup the head and tip of the sync range
        // 查找head以及sync range的tip
        let gap = self.get_sync_gap(tx, current_checkpoint.block_number).await?;
        let local_head = gap.local_head.number;
        let tip = gap.target.tip();

        // Nothing to sync
        // 没有什么需要同步
        if gap.is_closed() {
            info!(target: "sync::stages::headers", checkpoint = %current_checkpoint, target = ?tip, "Target block already reached");
            return Ok(ExecOutput { checkpoint: current_checkpoint, done: true })
        }

        debug!(target: "sync::stages::headers", ?tip, head = ?gap.local_head.hash(), "Commencing sync");

        // let the downloader know what to sync
        // 让downloader知道要同步什么
        self.downloader.update_sync_gap(gap.local_head, gap.target);

        // The downloader returns the headers in descending order starting from the tip
        // down to the local head (latest block in db).
        // downloader返回headers，从tip开始，按照降序排列，直到local head（db中的最新block）。
        // Task downloader can return `None` only if the response relaying channel was closed. This
        // is a fatal error to prevent the pipeline from running forever.
        // Task downloader只能返回`None`，如果response relaying channel被关闭。这是一个致命的错误，防止pipeline永远运行。
        let downloaded_headers = self.downloader.next().await.ok_or(StageError::ChannelClosed)?;

        info!(target: "sync::stages::headers", len = downloaded_headers.len(), "Received headers");

        let tip_block_number = match tip {
            // If tip is hash and it equals to the first downloaded header's hash, we can use
            // the block number of this header as tip.
            // 如果tip是hash，并且它等于第一个下载的header的hash，我们可以使用这个header的block number作为tip。
            BlockHashOrNumber::Hash(hash) => downloaded_headers.first().and_then(|header| {
                if header.hash == hash {
                    Some(header.number)
                } else {
                    None
                }
            }),
            // If tip is number, we can just grab it and not resolve using downloaded headers.
            // 如果tip是number，我们可以直接获取它，而不是使用下载的headers来解析。
            BlockHashOrNumber::Number(number) => Some(number),
        };

        // Since we're syncing headers in batches, gap tip will move in reverse direction towards
        // our local head with every iteration. To get the actual target block number we're
        // syncing towards, we need to take into account already synced headers from the database.
        // It is `None`, if tip didn't change and we're still downloading headers for previously
        // calculated gap.
        // 因为我们正在批量同步headers，gap tip将在每次迭代中向我们的本地head反向移动。
        // 为了获得我们正在同步的实际目标block number，我们需要考虑从数据库中同步的headers。
        // 如果是`None`，如果tip没有改变，并且我们仍然在为之前计算的gap下载headers。
        let target_block_number = if let Some(tip_block_number) = tip_block_number {
            // 本地最大的block number
            let local_max_block_number = tx
                .cursor_read::<tables::CanonicalHeaders>()?
                .last()?
                .map(|(canonical_block, _)| canonical_block);

            Some(tip_block_number.max(local_max_block_number.unwrap_or(tip_block_number)))
        } else {
            None
        };

        let mut stage_checkpoint = current_checkpoint
            .entities_stage_checkpoint()
            .unwrap_or(EntitiesCheckpoint {
            // If for some reason (e.g. due to DB migration) we don't have `processed`
            // in the middle of headers sync, set it to the local head block number +
            // number of block already filled in the gap.
            // 如果因为一些原因（例如由于DB migration）我们在headers sync的中间没有`processed`，
            // 设置到本地head block number + block number的数量，已经填充了gap。
            processed: local_head +
                (target_block_number.unwrap_or_default() - tip_block_number.unwrap_or_default()),
            // Shouldn't fail because on the first iteration, we download the header for missing
            // tip, and use its block number.
            // 在第一次迭代中，我们下载缺失tip的header，并使用它的block number，不应该失败。
            total: target_block_number.or_else(|| {
                warn!(target: "sync::stages::headers", ?tip, "No downloaded header for tip found");
                // Safe, because `Display` impl for `EntitiesCheckpoint` will fallback to displaying
                // just `processed`
                None
            }),
        });

        // Total headers can be updated if we received new tip from the network, and need to fill
        // the local gap.
        // Total headers可以被更新，如果我们从network接收到了新的tip，需要填充本地的gap。
        if let Some(target_block_number) = target_block_number {
            stage_checkpoint.total = Some(target_block_number);
        }
        // 加上processed headers
        stage_checkpoint.processed += downloaded_headers.len() as u64;

        // Write the headers to db
        // 将headers写入到db中
        self.write_headers::<DB>(tx, downloaded_headers)?.unwrap_or_default();

        if self.is_stage_done(tx, current_checkpoint.block_number)? {
            let checkpoint = current_checkpoint.block_number.max(
                tx.cursor_read::<tables::CanonicalHeaders>()?
                    .last()?
                    .map(|(num, _)| num)
                    .unwrap_or_default(),
            );
            Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(checkpoint)
                    .with_entities_stage_checkpoint(stage_checkpoint),
                done: true,
            })
        } else {
            Ok(ExecOutput {
                checkpoint: current_checkpoint.with_entities_stage_checkpoint(stage_checkpoint),
                done: false,
            })
        }
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // TODO: handle bad block
        tx.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            input.unwind_to + 1,
        )?;
        tx.unwind_table_by_num::<tables::CanonicalHeaders>(input.unwind_to)?;
        let unwound_headers = tx.unwind_table_by_num::<tables::Headers>(input.unwind_to)?;

        let stage_checkpoint =
            input.checkpoint.entities_stage_checkpoint().map(|checkpoint| EntitiesCheckpoint {
                processed: checkpoint.processed.saturating_sub(unwound_headers as u64),
                total: None,
            });

        let mut checkpoint = StageCheckpoint::new(input.unwind_to);
        if let Some(stage_checkpoint) = stage_checkpoint {
            checkpoint = checkpoint.with_entities_stage_checkpoint(stage_checkpoint);
        }

        info!(target: "sync::stages::headers", to_block = input.unwind_to, checkpoint = input.unwind_to, is_final_range = true, "Unwind iteration finished");
        Ok(UnwindOutput { checkpoint })
    }
}

/// Represents a gap to sync: from `local_head` to `target`
/// 代表一个gap to sync：从`local_head`到`target`
#[derive(Debug)]
pub struct SyncGap {
    /// The local head block. Represents lower bound of sync range.
    /// 本地的head block，代表sync range的下限。
    pub local_head: SealedHeader,

    /// The sync target. Represents upper bound of sync range.
    /// sync target，代表sync range的上限。
    pub target: SyncTarget,
}

// === impl SyncGap ===

impl SyncGap {
    /// Returns `true` if the gap from the head to the target was closed
    #[inline]
    pub fn is_closed(&self) -> bool {
        match self.target.tip() {
            BlockHashOrNumber::Hash(hash) => self.local_head.hash() == hash,
            BlockHashOrNumber::Number(num) => self.local_head.number == num,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite, ExecuteStageTestRunner, StageTestRunner, UnwindStageTestRunner,
        PREV_STAGE_ID,
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::generators::random_header;
    use reth_primitives::{stage::StageUnitCheckpoint, H256};
    use test_runner::HeadersTestRunner;

    mod test_runner {
        use super::*;
        use crate::test_utils::{TestRunnerError, TestTransaction};
        use reth_downloaders::headers::reverse_headers::{
            ReverseHeadersDownloader, ReverseHeadersDownloaderBuilder,
        };
        use reth_interfaces::test_utils::{
            generators::random_header_range, TestConsensus, TestHeaderDownloader, TestHeadersClient,
        };
        use reth_primitives::U256;
        use std::sync::Arc;

        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            // 包含test header client
            pub(crate) client: TestHeadersClient,
            channel: (watch::Sender<H256>, watch::Receiver<H256>),
            downloader_factory: Box<dyn Fn() -> D + Send + Sync + 'static>,
            tx: TestTransaction,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(H256::zero()),
                    downloader_factory: Box::new(move || {
                        TestHeaderDownloader::new(
                            client.clone(),
                            Arc::new(TestConsensus::default()),
                            1000,
                            1000,
                        )
                    }),
                    tx: TestTransaction::default(),
                }
            }
        }

        impl<D: HeaderDownloader + 'static> StageTestRunner for HeadersTestRunner<D> {
            type S = HeaderStage<D>;

            fn tx(&self) -> &TestTransaction {
                &self.tx
            }

            fn stage(&self) -> Self::S {
                HeaderStage {
                    mode: HeaderSyncMode::Tip(self.channel.1.clone()),
                    downloader: (*self.downloader_factory)(),
                }
            }
        }

        #[async_trait::async_trait]
        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                // 获取开始的block number
                let start = input.checkpoint().block_number;
                let head = random_header(start, None);
                self.tx.insert_headers(std::iter::once(&head))?;
                // patch td table for `update_head` call
                // 对于`update_head`调用，patch td table
                self.tx.commit(|tx| tx.put::<tables::HeaderTD>(head.number, U256::ZERO.into()))?;

                // use previous checkpoint as seed size
                // 使用之前的checkpoint作为seed size
                let end =
                    input.previous_stage.map(|(_, num)| num).unwrap_or_default().block_number + 1;

                if start + 1 >= end {
                    return Ok(Vec::default())
                }

                // 构建headers
                let mut headers = random_header_range(start + 1..end, head.hash());
                headers.insert(0, head);
                Ok(headers)
            }

            /// Validate stored headers
            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                let initial_checkpoint = input.checkpoint().block_number;
                match output {
                    Some(output) if output.checkpoint.block_number > initial_checkpoint => {
                        self.tx.query(|tx| {
                            for block_num in
                                (initial_checkpoint..output.checkpoint.block_number).rev()
                            {
                                // look up the header hash
                                let hash = tx
                                    .get::<tables::CanonicalHeaders>(block_num)?
                                    .expect("no header hash");

                                // validate the header number
                                assert_eq!(tx.get::<tables::HeaderNumbers>(hash)?, Some(block_num));

                                // validate the header
                                let header = tx.get::<tables::Headers>(block_num)?;
                                assert!(header.is_some());
                                let header = header.unwrap().seal_slow();
                                assert_eq!(header.hash(), hash);
                            }
                            Ok(())
                        })?;
                    }
                    _ => self.check_no_header_entry_above(initial_checkpoint)?,
                };
                Ok(())
            }

            async fn after_execution(&self, headers: Self::Seed) -> Result<(), TestRunnerError> {
                self.client.extend(headers.iter().map(|h| h.clone().unseal())).await;
                let tip = if !headers.is_empty() {
                    headers.last().unwrap().hash()
                } else {
                    let tip = random_header(0, None);
                    self.tx.insert_headers(std::iter::once(&tip))?;
                    tip.hash()
                };
                self.send_tip(tip);
                Ok(())
            }
        }

        impl<D: HeaderDownloader + 'static> UnwindStageTestRunner for HeadersTestRunner<D> {
            fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
                self.check_no_header_entry_above(input.unwind_to)
            }
        }

        impl HeadersTestRunner<ReverseHeadersDownloader<TestHeadersClient>> {
            pub(crate) fn with_linear_downloader() -> Self {
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(H256::zero()),
                    downloader_factory: Box::new(move || {
                        ReverseHeadersDownloaderBuilder::default()
                            .stream_batch_size(500)
                            .build(client.clone(), Arc::new(TestConsensus::default()))
                    }),
                    tx: TestTransaction::default(),
                }
            }
        }

        impl<D: HeaderDownloader> HeadersTestRunner<D> {
            pub(crate) fn check_no_header_entry_above(
                &self,
                block: BlockNumber,
            ) -> Result<(), TestRunnerError> {
                self.tx
                    .ensure_no_entry_above_by_value::<tables::HeaderNumbers, _>(block, |val| val)?;
                self.tx.ensure_no_entry_above::<tables::CanonicalHeaders, _>(block, |key| key)?;
                self.tx.ensure_no_entry_above::<tables::Headers, _>(block, |key| key)?;
                Ok(())
            }

            pub(crate) fn send_tip(&self, tip: H256) {
                self.channel.0.send(tip).expect("failed to send tip");
            }
        }
    }

    stage_test_suite!(HeadersTestRunner, headers);

    /// Execute the stage with linear downloader
    /// 用liner downloader执行stage
    #[tokio::test]
    async fn execute_with_linear_downloader() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        let (checkpoint, previous_stage) = (1000, 1200);
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        // 对于liner downloader，跳过`after_execution` hook
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!( result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                processed,
                total: Some(total),
            }))
        }, done: true }) if block_number == tip.number
            // -1 because we don't need to download the local head
            && processed == checkpoint + headers.len() as u64 - 1
            && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    /// Test the head and tip range lookup
    /// 测试head以及tip的range lookup
    #[tokio::test]
    async fn head_and_tip_lookup() {
        let runner = HeadersTestRunner::default();
        let tx = runner.tx().inner();
        let mut stage = runner.stage();

        let consensus_tip = H256::random();
        runner.send_tip(consensus_tip);

        // Genesis
        let checkpoint = 0;
        let head = random_header(0, None);
        let gap_fill = random_header(1, Some(head.hash()));
        let gap_tip = random_header(2, Some(gap_fill.hash()));

        // Empty database
        // 空的数据库
        assert_matches!(
            stage.get_sync_gap(&tx, checkpoint).await,
            Err(StageError::DatabaseIntegrity(ProviderError::HeaderNotFound(block_number)))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        // Checkpoint并且没有gap
        tx.put::<tables::CanonicalHeaders>(head.number, head.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(head.number, head.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&tx, checkpoint).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip.into());

        // Checkpoint and gap
        // Checkpoint并且有gap
        tx.put::<tables::CanonicalHeaders>(gap_tip.number, gap_tip.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_tip.number, gap_tip.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&tx, checkpoint).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), gap_tip.parent_hash.into());

        // Checkpoint and gap closed
        // Checkpoint并且gap被关闭
        tx.put::<tables::CanonicalHeaders>(gap_fill.number, gap_fill.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_fill.number, gap_fill.clone().unseal())
            .expect("failed to write header");

        assert_matches!(
            stage.get_sync_gap(&tx, checkpoint).await,
            Err(StageError::StageCheckpoint(_checkpoint)) if _checkpoint == checkpoint
        );
    }

    /// Execute the stage in two steps
    /// 分两步执行stage
    #[tokio::test]
    async fn execute_from_previous_checkpoint() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        // pick range that's larger than the configured headers batch size
        // 选择range，它比配置的headers batch size大
        let (checkpoint, previous_stage) = (600, 1200);
        let mut input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(previous_stage))),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        // 跳过`after_execution` hook，对于linear downloader
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                processed,
                total: Some(total),
            }))
        }, done: false }) if block_number == checkpoint &&
            processed == checkpoint + 500 &&
            total == tip.number);

        runner.client.clear().await;
        runner.client.extend(headers.iter().take(101).map(|h| h.clone().unseal()).rev()).await;
        input.checkpoint = Some(result.unwrap().checkpoint);

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                processed,
                total: Some(total),
            }))
        }, done: true }) if block_number == tip.number
            // -1 because we don't need to download the local head
            // -1因为我们不需要下载local head
            && processed == checkpoint + headers.len() as u64 - 1
            && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }
}
