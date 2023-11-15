use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use futures_util::StreamExt;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{
    p2p::headers::{
        downloader::{HeaderDownloader, SyncTarget},
        error::HeadersDownloaderError,
    },
    provider::ProviderError,
};
use reth_primitives::{
    stage::{
        CheckpointBlockRange, EntitiesCheckpoint, HeadersCheckpoint, StageCheckpoint, StageId,
    },
    BlockHashOrNumber, BlockNumber, SealedHeader, H256,
};
use reth_provider::DatabaseProviderRW;
use tokio::sync::watch;
use tracing::*;

/// The header sync mode.
/// heaer的同步模式
#[derive(Debug)]
pub enum HeaderSyncMode {
    /// A sync mode in which the stage continuously requests the downloader for
    /// next blocks.
    /// 一种同步模式，stage持续请求downloader下载下一个block
    Continuous,
    /// A sync mode in which the stage polls the receiver for the next tip
    /// to download from.
    /// 一种同步模式，stage轮询receiver，对于下一个要下载的tip
    Tip(watch::Receiver<H256>),
}

/// The headers stage.
///
/// The headers stage downloads all block headers from the highest block in the local database to
/// the perceived highest block on the network.
/// header stage从本地的db中的highest block下载所有的block headers，到网络中感知的highest block
///
/// The headers are processed and data is inserted into these tables:
/// headers被处理并且data被插入到这些tables
///
/// - [`HeaderNumbers`][reth_db::tables::HeaderNumbers]
/// - [`Headers`][reth_db::tables::Headers]
/// - [`CanonicalHeaders`][reth_db::tables::CanonicalHeaders]
///
/// NOTE: This stage downloads headers in reverse. Upon returning the control flow to the pipeline,
/// the stage checkpoint is not updated until this stage is done.
/// 注意：这个stage反向下载headers，直到返回control flow到pipeline，stage
/// checkpoint不被更新，直到stage完成
#[derive(Debug)]
pub struct HeaderStage<D: HeaderDownloader> {
    /// Strategy for downloading the headers
    /// 用于下载headers的策略
    downloader: D,
    /// The sync mode for the stage.
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
        tx: &<DB as reth_db::database::DatabaseGAT<'_>>::TXMut,
        checkpoint: u64,
    ) -> Result<bool, StageError> {
        let mut header_cursor = tx.cursor_read::<tables::CanonicalHeaders>()?;
        let (head_num, _) = header_cursor
            .seek_exact(checkpoint)?
            .ok_or_else(|| ProviderError::HeaderNotFound(checkpoint.into()))?;
        // Check if the next entry is congruent
        // 检查下一个entry是不是一致的
        Ok(header_cursor.next()?.map(|(next_num, _)| head_num + 1 == next_num).unwrap_or_default())
    }

    /// Get the head and tip of the range we need to sync
    /// 获取我们需要同步的head以及tip of the range
    ///
    /// See also [SyncTarget]
    async fn get_sync_gap<DB: Database>(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        checkpoint: u64,
    ) -> Result<SyncGap, StageError> {
        // Create a cursor over canonical header hashes
        // 创建一个cursor，对于canonical header hashes
        let mut cursor = provider.tx_ref().cursor_read::<tables::CanonicalHeaders>()?;
        let mut header_cursor = provider.tx_ref().cursor_read::<tables::Headers>()?;

        // Get head hash and reposition the cursor
        // 获取head hash并且对cursor重新定位
        let (head_num, head_hash) = cursor
            .seek_exact(checkpoint)?
            // 没有找到对应的header
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
        // 决定tip或者对于非法的input直接error
        // If the next element found in the cursor is not the "expected" next block per our current
        // checkpoint, then there is a gap in the database and we should start downloading in
        // reverse from there. Else, it should use whatever the forkchoice state reports.
        // 如果cursor中找到的下一个element不是期望的下一个block，对于我们当前的checkpoint，
        // 那么在db中有一个gap，我们应该从这里开始反向下载，否则使用任何报告的forkchoice state
        let target = match next_header {
            Some(header) if checkpoint + 1 != header.number => SyncTarget::Gap(header),
            None => self
                .next_sync_target(head_num)
                .await
                .ok_or(StageError::StageCheckpoint(checkpoint))?,
            _ => return Err(StageError::StageCheckpoint(checkpoint)),
        };

        Ok(SyncGap { local_head, target })
    }

    async fn next_sync_target(&mut self, head: BlockNumber) -> Option<SyncTarget> {
        match self.mode {
            HeaderSyncMode::Tip(ref mut rx) => {
                // 等待接收tip
                let tip = rx.wait_for(|tip| !tip.is_zero()).await.ok()?;
                Some(SyncTarget::Tip(*tip))
            }
            HeaderSyncMode::Continuous => {
                // 使用continuous sync target，使用下一个head
                trace!(target: "sync::stages::headers", head, "No next header found, using continuous sync strategy");
                Some(SyncTarget::TipNum(head + 1))
            }
        }
    }

    /// Write downloaded headers to the given transaction
    /// 将下载的headers写入到给定的tx
    ///
    /// Note: this writes the headers with rising block numbers.
    /// 注意：这写入headers，按照升序
    fn write_headers<DB: Database>(
        &self,
        tx: &<DB as reth_db::database::DatabaseGAT<'_>>::TXMut,
        headers: Vec<SealedHeader>,
    ) -> Result<Option<BlockNumber>, StageError> {
        trace!(target: "sync::stages::headers", len = headers.len(), "writing headers");

        let mut cursor_header = tx.cursor_write::<tables::Headers>()?;
        let mut cursor_canonical = tx.cursor_write::<tables::CanonicalHeaders>()?;

        let mut latest = None;
        // Since the headers were returned in descending order,
        // iterate them in the reverse order
        // 因为headers按照降序返回，以相反的顺序遍历
        for header in headers.into_iter().rev() {
            if header.number == 0 {
                continue
            }

            let header_hash = header.hash();
            let header_number = header.number;
            let header = header.unseal();
            latest = Some(header.number);

            // NOTE: HeaderNumbers are not sorted and can't be inserted with cursor.
            // 注意：HeaderNumbers没有被排序并且不能用cursor插入
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
    /// 返回stage的id
    fn id(&self) -> StageId {
        StageId::Headers
    }

    /// Download the headers in reverse order (falling block numbers)
    /// starting from the tip of the chain
    /// 按照反序下载headers（block numbers下降）
    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let tx = provider.tx_ref();
        let current_checkpoint = input.checkpoint();

        // Lookup the head and tip of the sync range
        // 查找head以及sync range的tip
        let gap = self.get_sync_gap(provider, current_checkpoint.block_number).await?;
        let local_head = gap.local_head.number;
        let tip = gap.target.tip();

        // Nothing to sync
        // 没有东西需要同步
        if gap.is_closed() {
            info!(target: "sync::stages::headers", checkpoint = %current_checkpoint, target = ?tip, "Target block already reached");
            return Ok(ExecOutput::done(current_checkpoint))
        }

        debug!(target: "sync::stages::headers", ?tip, head = ?gap.local_head.hash(), "Commencing sync");

        // let the downloader know what to sync
        // 让downloader知道同步什么
        self.downloader.update_sync_gap(gap.local_head, gap.target);

        // The downloader returns the headers in descending order starting from the tip
        // down to the local head (latest block in db).
        // downloader返回headers，按照降序，从tip到local head（即db中的latest block）
        // Task downloader can return `None` only if the response relaying channel was closed. This
        // is a fatal error to prevent the pipeline from running forever.
        // Task downloader可以返回`None`，只有在response relaying channel被关闭的时候，这是一个fatal
        // error来防止pipeline一直运行
        let downloaded_headers = match self.downloader.next().await {
            Some(Ok(headers)) => headers,
            Some(Err(HeadersDownloaderError::DetachedHead { local_head, header, error })) => {
                error!(target: "sync::stages::headers", ?error, "Cannot attach header to head");
                return Err(StageError::DetachedHead { local_head, header, error })
            }
            None => return Err(StageError::ChannelClosed),
        };

        info!(target: "sync::stages::headers", len = downloaded_headers.len(), "Received headers");

        let tip_block_number = match tip {
            // If tip is hash and it equals to the first downloaded header's hash, we can use
            // the block number of this header as tip.
            // 如果tip是hash并且匹配第一个下载的header的hash，我们可以使用这个header的block
            // number作为tip
            BlockHashOrNumber::Hash(hash) => downloaded_headers.first().and_then(|header| {
                if header.hash == hash {
                    Some(header.number)
                } else {
                    None
                }
            }),
            // If tip is number, we can just grab it and not resolve using downloaded headers.
            // 如果tip是number，我们可以只是抓取它并且不使用downloaded headers解析
            BlockHashOrNumber::Number(number) => Some(number),
        };

        // Since we're syncing headers in batches, gap tip will move in reverse direction towards
        // our local head with every iteration. To get the actual target block number we're
        // syncing towards, we need to take into account already synced headers from the database.
        // 为了获取我们前往的真正的target block number，我们需要考虑来自db的synced headers
        // It is `None`, if tip didn't change and we're still downloading headers for previously
        // calculated gap.
        // 这是`None`，如果tip不改变，并且我们依然下载headers，对于之前计算的gap
        let target_block_number = if let Some(tip_block_number) = tip_block_number {
            let local_max_block_number = tx
                .cursor_read::<tables::CanonicalHeaders>()?
                .last()?
                .map(|(canonical_block, _)| canonical_block);

            Some(tip_block_number.max(local_max_block_number.unwrap_or(tip_block_number)))
        } else {
            None
        };

        let mut stage_checkpoint = match current_checkpoint.headers_stage_checkpoint() {
            // If checkpoint block range matches our range, we take the previously used
            // stage checkpoint as-is.
            // 如果checkpoint block range匹配我们的range，我们使用之前的stage checkpoint
            Some(stage_checkpoint)
                if stage_checkpoint.block_range.from == input.checkpoint().block_number =>
            {
                stage_checkpoint
            }
            // Otherwise, we're on the first iteration of new gap sync, so we recalculate the number
            // of already processed and total headers.
            // `target_block_number` is guaranteed to be `Some`, because on the first iteration
            // we download the header for missing tip and use its block number.
            _ => {
                HeadersCheckpoint {
                    block_range: CheckpointBlockRange {
                        from: input.checkpoint().block_number,
                        to: target_block_number.expect("No downloaded header for tip found"),
                    },
                    progress: EntitiesCheckpoint {
                        // Set processed to the local head block number + number
                        // of block already filled in the gap.
                        processed: local_head +
                            (target_block_number.unwrap_or_default() -
                                tip_block_number.unwrap_or_default()),
                        total: target_block_number.expect("No downloaded header for tip found"),
                    },
                }
            }
        };

        // Total headers can be updated if we received new tip from the network, and need to fill
        // the local gap.
        // Total headers可以被更新，如果我们从network接收到新的tip，并且需要填充local gap
        if let Some(target_block_number) = target_block_number {
            stage_checkpoint.progress.total = target_block_number;
        }
        stage_checkpoint.progress.processed += downloaded_headers.len() as u64;

        // Write the headers to db
        // 将headers写入db
        self.write_headers::<DB>(tx, downloaded_headers)?.unwrap_or_default();

        if self.is_stage_done::<DB>(tx, current_checkpoint.block_number)? {
            let checkpoint = current_checkpoint.block_number.max(
                tx.cursor_read::<tables::CanonicalHeaders>()?
                    .last()?
                    .map(|(num, _)| num)
                    .unwrap_or_default(),
            );
            Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(checkpoint)
                    .with_headers_stage_checkpoint(stage_checkpoint),
                // 设置stage is done
                done: true,
            })
        } else {
            Ok(ExecOutput {
                // 设置checkpoint
                checkpoint: current_checkpoint.with_headers_stage_checkpoint(stage_checkpoint),
                done: false,
            })
        }
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // TODO: handle bad block
        provider.unwind_table_by_walker::<tables::CanonicalHeaders, tables::HeaderNumbers>(
            input.unwind_to + 1,
        )?;
        provider.unwind_table_by_num::<tables::CanonicalHeaders>(input.unwind_to)?;
        let unwound_headers = provider.unwind_table_by_num::<tables::Headers>(input.unwind_to)?;

        let stage_checkpoint =
            input.checkpoint.headers_stage_checkpoint().map(|stage_checkpoint| HeadersCheckpoint {
                block_range: stage_checkpoint.block_range,
                progress: EntitiesCheckpoint {
                    processed: stage_checkpoint
                        .progress
                        .processed
                        .saturating_sub(unwound_headers as u64),
                    total: stage_checkpoint.progress.total,
                },
            });

        let mut checkpoint = StageCheckpoint::new(input.unwind_to);
        if let Some(stage_checkpoint) = stage_checkpoint {
            checkpoint = checkpoint.with_headers_stage_checkpoint(stage_checkpoint);
        }

        Ok(UnwindOutput { checkpoint })
    }
}

/// Represents a gap to sync: from `local_head` to `target`
/// 代表一个需要同步的gap：从`local_head`到`target`
#[derive(Debug)]
pub struct SyncGap {
    /// The local head block. Represents lower bound of sync range.
    /// 本地的head block，代表sync range的lower bound
    pub local_head: SealedHeader,

    /// The sync target. Represents upper bound of sync range.
    /// sync target，代表sync range的upper bound
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
    };
    use assert_matches::assert_matches;
    use reth_interfaces::test_utils::{generators, generators::random_header};
    use reth_primitives::{stage::StageUnitCheckpoint, H256, MAINNET};
    use reth_provider::ProviderFactory;
    use test_runner::HeadersTestRunner;

    mod test_runner {
        use super::*;
        use crate::test_utils::{TestRunnerError, TestTransaction};
        use reth_downloaders::headers::reverse_headers::{
            ReverseHeadersDownloader, ReverseHeadersDownloaderBuilder,
        };
        use reth_interfaces::test_utils::{
            generators, generators::random_header_range, TestConsensus, TestHeaderDownloader,
            TestHeadersClient,
        };
        use reth_primitives::U256;
        use reth_provider::{BlockHashReader, BlockNumReader, HeaderProvider};
        use std::sync::Arc;

        // 定义HeadersTestRunner结构
        pub(crate) struct HeadersTestRunner<D: HeaderDownloader> {
            pub(crate) client: TestHeadersClient,
            channel: (watch::Sender<H256>, watch::Receiver<H256>),
            downloader_factory: Box<dyn Fn() -> D + Send + Sync + 'static>,
            tx: TestTransaction,
        }

        impl Default for HeadersTestRunner<TestHeaderDownloader> {
            fn default() -> Self {
                // 构建headers client
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(H256::zero()),
                    downloader_factory: Box::new(move || {
                        // 构建一个downloader factory
                        TestHeaderDownloader::new(
                            client.clone(),
                            Arc::new(TestConsensus::default()),
                            // 参数为limit和batch_size
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
                // 构建Header Stage
                HeaderStage {
                    mode: HeaderSyncMode::Tip(self.channel.1.clone()),
                    // 构建downloader
                    downloader: (*self.downloader_factory)(),
                }
            }
        }

        #[async_trait::async_trait]
        impl<D: HeaderDownloader + 'static> ExecuteStageTestRunner for HeadersTestRunner<D> {
            type Seed = Vec<SealedHeader>;

            fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
                let mut rng = generators::rng();
                let start = input.checkpoint().block_number;
                let head = random_header(&mut rng, start, None);
                // 插入headers
                self.tx.insert_headers(std::iter::once(&head))?;
                // patch td table for `update_head` call
                // 对于`update_head`调用，对td table进行patch
                self.tx.commit(|tx| tx.put::<tables::HeaderTD>(head.number, U256::ZERO.into()))?;

                // use previous checkpoint as seed size
                // 使用之前的checkpoint作为seed
                let end = input.target.unwrap_or_default() + 1;

                if start + 1 >= end {
                    return Ok(Vec::default())
                }

                // 生成随机的headers
                let mut headers = random_header_range(&mut rng, start + 1..end, head.hash());
                headers.insert(0, head);
                Ok(headers)
            }

            /// Validate stored headers
            /// 校验存储的headers
            fn validate_execution(
                &self,
                input: ExecInput,
                output: Option<ExecOutput>,
            ) -> Result<(), TestRunnerError> {
                let initial_checkpoint = input.checkpoint().block_number;
                match output {
                    Some(output) if output.checkpoint.block_number > initial_checkpoint => {
                        let provider = self.tx.factory.provider()?;
                        for block_num in (initial_checkpoint..output.checkpoint.block_number).rev()
                        {
                            // look up the header hash
                            // 查找header hash
                            let hash = provider.block_hash(block_num)?.expect("no header hash");

                            // validate the header number
                            // 校验header number
                            assert_eq!(provider.block_number(hash)?, Some(block_num));

                            // validate the header
                            // 校验header
                            let header = provider.header_by_number(block_num)?;
                            assert!(header.is_some());
                            let header = header.unwrap().seal_slow();
                            assert_eq!(header.hash(), hash);
                        }
                    }
                    _ => self.check_no_header_entry_above(initial_checkpoint)?,
                };
                Ok(())
            }

            async fn after_execution(&self, headers: Self::Seed) -> Result<(), TestRunnerError> {
                // 扩展client
                self.client.extend(headers.iter().map(|h| h.clone().unseal())).await;
                let tip = if !headers.is_empty() {
                    headers.last().unwrap().hash()
                } else {
                    // 插入header
                    let tip = random_header(&mut generators::rng(), 0, None);
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
                // 生成TestHeadersClient
                let client = TestHeadersClient::default();
                Self {
                    client: client.clone(),
                    channel: watch::channel(H256::zero()),
                    // 构建downloader factory，用于生产downloader
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
                // 发送tip
                self.channel.0.send(tip).expect("failed to send tip");
            }
        }
    }

    stage_test_suite!(HeadersTestRunner, headers);

    /// Execute the stage with linear downloader
    /// 执行stage，用linear downloader
    #[tokio::test]
    async fn execute_with_linear_downloader() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        let (checkpoint, previous_stage) = (1000, 1200);
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        // 扩展client的headers
        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        // 跳过`after_execution` hook，对于linear downloader
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        let result = rx.await.unwrap();
        assert_matches!( result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: true }) if block_number == tip.number &&
            from == checkpoint && to == previous_stage &&
            // -1 because we don't need to download the local head
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }

    /// Test the head and tip range lookup
    /// 测试head以及tip的range lookup
    #[tokio::test]
    async fn head_and_tip_lookup() {
        let runner = HeadersTestRunner::default();
        let factory = ProviderFactory::new(runner.tx().tx.as_ref(), MAINNET.clone());
        let provider = factory.provider_rw().unwrap();
        let tx = provider.tx_ref();
        let mut stage = runner.stage();

        let mut rng = generators::rng();

        let consensus_tip = H256::random();
        runner.send_tip(consensus_tip);

        // Genesis
        // Genesis block
        let checkpoint = 0;
        // 生成随机的header
        let head = random_header(&mut rng, 0, None);
        let gap_fill = random_header(&mut rng, 1, Some(head.hash()));
        let gap_tip = random_header(&mut rng, 2, Some(gap_fill.hash()));

        // Empty database
        // 空的db
        assert_matches!(
            stage.get_sync_gap(&provider, checkpoint).await,
            Err(StageError::DatabaseIntegrity(ProviderError::HeaderNotFound(block_number)))
                if block_number.as_number().unwrap() == checkpoint
        );

        // Checkpoint and no gap
        // checkpoint并且没有gap
        tx.put::<tables::CanonicalHeaders>(head.number, head.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(head.number, head.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&provider, checkpoint).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), consensus_tip.into());

        // Checkpoint and gap
        // Checkpoint以及gap，插入gap tip
        tx.put::<tables::CanonicalHeaders>(gap_tip.number, gap_tip.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_tip.number, gap_tip.clone().unseal())
            .expect("failed to write header");

        let gap = stage.get_sync_gap(&provider, checkpoint).await.unwrap();
        assert_eq!(gap.local_head, head);
        assert_eq!(gap.target.tip(), gap_tip.parent_hash.into());

        // Checkpoint and gap closed
        // Checkpoint并且gap关闭，插入gap_fill
        tx.put::<tables::CanonicalHeaders>(gap_fill.number, gap_fill.hash())
            .expect("failed to write canonical");
        tx.put::<tables::Headers>(gap_fill.number, gap_fill.clone().unseal())
            .expect("failed to write header");

        assert_matches!(
            stage.get_sync_gap(&provider, checkpoint).await,
            Err(StageError::StageCheckpoint(_checkpoint)) if _checkpoint == checkpoint
        );
    }

    /// Execute the stage in two steps
    /// 两个步骤执行stage
    #[tokio::test]
    async fn execute_from_previous_checkpoint() {
        let mut runner = HeadersTestRunner::with_linear_downloader();
        // pick range that's larger than the configured headers batch size
        // 选择大于配置的header batch size的range
        let (checkpoint, previous_stage) = (600, 1200);
        let mut input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(checkpoint)),
        };
        let headers = runner.seed_execution(input).expect("failed to seed execution");
        let rx = runner.execute(input);

        // 扩展client
        runner.client.extend(headers.iter().rev().map(|h| h.clone().unseal())).await;

        // skip `after_execution` hook for linear downloader
        // 跳过`after_execution` hook，对于liner downloader
        let tip = headers.last().unwrap();
        runner.send_tip(tip.hash());

        // 进行unwrap
        let result = rx.await.unwrap();
        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: false }) if block_number == checkpoint &&
            from == checkpoint && to == previous_stage &&
            processed == checkpoint + 500 && total == tip.number);

        // 清理client
        runner.client.clear().await;
        runner.client.extend(headers.iter().take(101).map(|h| h.clone().unseal()).rev()).await;
        input.checkpoint = Some(result.unwrap().checkpoint);

        let rx = runner.execute(input);
        let result = rx.await.unwrap();

        assert_matches!(result, Ok(ExecOutput { checkpoint: StageCheckpoint {
            block_number,
            stage_checkpoint: Some(StageUnitCheckpoint::Headers(HeadersCheckpoint {
                block_range: CheckpointBlockRange {
                    from,
                    to
                },
                progress: EntitiesCheckpoint {
                    processed,
                    total,
                }
            }))
        }, done: true }) if block_number == tip.number &&
            from == checkpoint && to == previous_stage &&
            // -1 because we don't need to download the local head
            // -1 因为我们不需要下载local head
            processed == checkpoint + headers.len() as u64 - 1 && total == tip.number);
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "validation failed");
    }
}
