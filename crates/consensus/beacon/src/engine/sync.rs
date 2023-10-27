//! Sync management for the engine implementation.

use crate::{engine::metrics::EngineSyncMetrics, BeaconConsensus};
use futures::FutureExt;
use reth_db::database::Database;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    full_block::{FetchFullBlockFuture, FetchFullBlockRangeFuture, FullBlockClient},
    headers::client::HeadersClient,
};
use reth_primitives::{BlockNumber, ChainSpec, SealedBlock, H256};
use reth_stages::{ControlFlow, Pipeline, PipelineError, PipelineWithResult};
use reth_tasks::TaskSpawner;
use std::{
    cmp::{Ordering, Reverse},
    collections::{binary_heap::PeekMut, BinaryHeap},
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::trace;

/// Manages syncing under the control of the engine.
/// 管理在engine控制下的同步
///
/// This type controls the [Pipeline] and supports (single) full block downloads.
/// 这个类型控制[Pipeline]并且支持（单个）完整的block下载
///
/// Caution: If the pipeline is running, this type will not emit blocks downloaded from the network
/// [EngineSyncEvent::FetchedFullBlock] until the pipeline is idle to prevent commits to the
/// database while the pipeline is still active.
/// 注意：如果pipeline在运行，这个类型不会发出block，
/// 来自network的[EngineSyncEvent::FetchedFullBlock]，直到pipeline处于idle，来防止提交到db，
/// 当pipeline依然active的时候
pub(crate) struct EngineSyncController<DB, Client>
where
    DB: Database,
    Client: HeadersClient + BodiesClient,
{
    /// A downloader that can download full blocks from the network.
    /// 一个downloader可以下载完成的full block，从network
    full_block_client: FullBlockClient<Client>,
    /// The type that can spawn the pipeline task.
    /// 这个类型可以生成pipeline task
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    /// The current state of the pipeline.
    /// pipeline的当前状态
    /// The pipeline is used for large ranges.
    pipeline_state: PipelineState<DB>,
    /// Pending target block for the pipeline to sync
    /// 等待pipeline去同步的target block
    pending_pipeline_target: Option<H256>,
    /// In-flight full block requests in progress.
    /// 正在处理的full block requests
    inflight_full_block_requests: Vec<FetchFullBlockFuture<Client>>,
    /// In-flight full block _range_ requests in progress.
    inflight_block_range_requests: Vec<FetchFullBlockRangeFuture<Client>>,
    /// Buffered blocks from downloads - this is a min-heap of blocks, using the block number for
    /// ordering. This means the blocks will be popped from the heap with ascending block numbers.
    /// 下载来的缓存的blocks - 这是一个blocks的min-heap，使用block
    /// number排序，这意味着blocks会按照升序被popped
    range_buffered_blocks: BinaryHeap<Reverse<OrderedSealedBlock>>,
    /// If enabled, the pipeline will be triggered continuously, as soon as it becomes idle
    /// 如果为enabled，pipeline会持续出发，一旦它变为idle
    run_pipeline_continuously: bool,
    /// Max block after which the consensus engine would terminate the sync. Used for debugging
    /// purposes.
    /// Max block，之后consensus engine就会终止sync，用于debugging的目的
    max_block: Option<BlockNumber>,
    /// Engine sync metrics.
    /// // Engine Sync相关的metrics
    metrics: EngineSyncMetrics,
}

impl<DB, Client> EngineSyncController<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    /// Create a new instance
    /// 创建一个新实例
    pub(crate) fn new(
        pipeline: Pipeline<DB>,
        client: Client,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        run_pipeline_continuously: bool,
        max_block: Option<BlockNumber>,
        chain_spec: Arc<ChainSpec>,
    ) -> Self {
        Self {
            full_block_client: FullBlockClient::new(
                client,
                Arc::new(BeaconConsensus::new(chain_spec)),
            ),
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(pipeline)),
            pending_pipeline_target: None,
            inflight_full_block_requests: Vec::new(),
            inflight_block_range_requests: Vec::new(),
            range_buffered_blocks: BinaryHeap::new(),
            run_pipeline_continuously,
            max_block,
            metrics: EngineSyncMetrics::default(),
        }
    }

    /// Sets the metrics for the active downloads
    fn update_block_download_metrics(&self) {
        self.metrics.active_block_downloads.set(self.inflight_full_block_requests.len() as f64);
        // TODO: full block range metrics
    }

    /// Sets the max block value for testing
    #[cfg(test)]
    pub(crate) fn set_max_block(&mut self, block: BlockNumber) {
        self.max_block = Some(block);
    }

    /// Cancels all download requests that are in progress and buffered blocks.
    /// 清理任何download requests，当前正在进行以及缓存的blocks
    pub(crate) fn clear_block_download_requests(&mut self) {
        self.inflight_full_block_requests.clear();
        self.inflight_block_range_requests.clear();
        self.range_buffered_blocks.clear();
        self.update_block_download_metrics();
    }

    /// Cancels the full block request with the given hash.
    pub(crate) fn cancel_full_block_request(&mut self, hash: H256) {
        self.inflight_full_block_requests.retain(|req| *req.hash() != hash);
        self.update_block_download_metrics();
    }

    /// Returns whether or not the sync controller is set to run the pipeline continuously.
    pub(crate) fn run_pipeline_continuously(&self) -> bool {
        self.run_pipeline_continuously
    }

    /// Returns `true` if a pipeline target is queued and will be triggered on the next `poll`.
    #[allow(unused)]
    pub(crate) fn is_pipeline_sync_pending(&self) -> bool {
        self.pending_pipeline_target.is_some() && self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is idle.
    pub(crate) fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns `true` if the pipeline is active.
    pub(crate) fn is_pipeline_active(&self) -> bool {
        !self.is_pipeline_idle()
    }

    /// Returns true if there's already a request for the given hash.
    pub(crate) fn is_inflight_request(&self, hash: H256) -> bool {
        self.inflight_full_block_requests.iter().any(|req| *req.hash() == hash)
    }

    /// Starts requesting a range of blocks from the network, in reverse from the given hash.
    ///
    /// If the `count` is 1, this will use the `download_full_block` method instead, because it
    /// downloads headers and bodies for the block concurrently.
    pub(crate) fn download_block_range(&mut self, hash: H256, count: u64) {
        if count == 1 {
            self.download_full_block(hash);
        } else {
            trace!(
                target: "consensus::engine",
                ?hash,
                ?count,
                "start downloading full block range."
            );

            let request = self.full_block_client.get_full_block_range(hash, count);
            self.inflight_block_range_requests.push(request);
        }

        // // TODO: need more metrics for block ranges
        // self.update_block_download_metrics();
    }

    /// Starts requesting a full block from the network.
    ///
    /// Returns `true` if the request was started, `false` if there's already a request for the
    /// given hash.
    pub(crate) fn download_full_block(&mut self, hash: H256) -> bool {
        if self.is_inflight_request(hash) {
            return false
        }
        trace!(
            target: "consensus::engine::sync",
            ?hash,
            "Start downloading full block"
        );
        let request = self.full_block_client.get_full_block(hash);
        self.inflight_full_block_requests.push(request);

        self.update_block_download_metrics();

        true
    }

    /// Sets a new target to sync the pipeline to.
    /// 设置一个新的target，pipeline需要同步到
    pub(crate) fn set_pipeline_sync_target(&mut self, target: H256) {
        self.pending_pipeline_target = Some(target);
    }

    /// Check if the engine reached max block as specified by `max_block` parameter.
    ///
    /// Note: this is mainly for debugging purposes.
    pub(crate) fn has_reached_max_block(&self, progress: BlockNumber) -> bool {
        let has_reached_max_block =
            self.max_block.map(|target| progress >= target).unwrap_or_default();
        if has_reached_max_block {
            trace!(
                target: "consensus::engine::sync",
                ?progress,
                max_block = ?self.max_block,
                "Consensus engine reached max block"
            );
        }
        has_reached_max_block
    }

    /// Advances the pipeline state.
    /// 推进pipeline的状态
    ///
    /// This checks for the result in the channel, or returns pending if the pipeline is idle.
    /// 这检查channel的结果，或者返回pending，如果pipeline处于idle
    fn poll_pipeline(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        let res = match self.pipeline_state {
            // 返回poll的结果
            PipelineState::Idle(_) => return Poll::Pending,
            PipelineState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pipeline, result)) => {
                let minimum_block_number = pipeline.minimum_block_number();
                let reached_max_block =
                    self.has_reached_max_block(minimum_block_number.unwrap_or_default());
                self.pipeline_state = PipelineState::Idle(Some(pipeline));
                EngineSyncEvent::PipelineFinished { result, reached_max_block }
            }
            Err(_) => {
                // failed to receive the pipeline
                EngineSyncEvent::PipelineTaskDropped
            }
        };
        Poll::Ready(ev)
    }

    /// This will spawn the pipeline if it is idle and a target is set or if the pipeline is set to
    /// run continuously.
    /// 这会试着生成pipeline，如果它是idle并且一个target被设置，或者pipeline被设置为持续运行
    fn try_spawn_pipeline(&mut self) -> Option<EngineSyncEvent> {
        match &mut self.pipeline_state {
            PipelineState::Idle(pipeline) => {
                let target = self.pending_pipeline_target.take();

                if target.is_none() && !self.run_pipeline_continuously {
                    // nothing to sync
                    // 没有什么需要同步
                    return None
                }

                let (tx, rx) = oneshot::channel();

                let pipeline = pipeline.take().expect("exists");
                self.pipeline_task_spawner.spawn_critical_blocking(
                    "pipeline task",
                    Box::pin(async move {
                        let result = pipeline.run_as_fut(target).await;
                        // 发送result
                        let _ = tx.send(result);
                    }),
                );
                // 设置pipeline state为running
                self.pipeline_state = PipelineState::Running(rx);

                // we also clear any pending full block requests because we expect them to be
                // outdated (included in the range the pipeline is syncing anyway)
                // 我们同时清理任何的pending full block
                // requests，因为我们期望他们outdated（包含pipeline以及syncing away的pipeline）
                self.clear_block_download_requests();

                Some(EngineSyncEvent::PipelineStarted(target))
            }
            PipelineState::Running(_) => None,
        }
    }

    /// Advances the sync process.
    /// 推进sync的进程
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        // try to spawn a pipeline if a target is set
        // 试着生成一个pipeline，如果设置了一个target
        if let Some(event) = self.try_spawn_pipeline() {
            return Poll::Ready(event)
        }

        // make sure we poll the pipeline if it's active, and return any ready pipeline events
        // 确保我们poll the pipeline，如果它是active的话，并且返回任何ready的pipeline events
        if !self.is_pipeline_idle() {
            // advance the pipeline
            // 推动pipeline
            if let Poll::Ready(event) = self.poll_pipeline(cx) {
                return Poll::Ready(event)
            }
        }

        // advance all full block requests
        // 推动所有的full block requests
        for idx in (0..self.inflight_full_block_requests.len()).rev() {
            let mut request = self.inflight_full_block_requests.swap_remove(idx);
            if let Poll::Ready(block) = request.poll_unpin(cx) {
                trace!(target: "consensus::engine", block=?block.num_hash(), "Received single full block, buffering");
                self.range_buffered_blocks.push(Reverse(OrderedSealedBlock(block)));
            } else {
                // still pending
                self.inflight_full_block_requests.push(request);
            }
        }

        // advance all full block range requests
        // 推动所有的full block range requests
        for idx in (0..self.inflight_block_range_requests.len()).rev() {
            let mut request = self.inflight_block_range_requests.swap_remove(idx);
            if let Poll::Ready(blocks) = request.poll_unpin(cx) {
                trace!(target: "consensus::engine", len=?blocks.len(), first=?blocks.first().map(|b| b.num_hash()), last=?blocks.last().map(|b| b.num_hash()), "Received full block range, buffering");
                self.range_buffered_blocks
                    .extend(blocks.into_iter().map(OrderedSealedBlock).map(Reverse));
            } else {
                // still pending
                self.inflight_block_range_requests.push(request);
            }
        }

        // 更新block download metrics
        self.update_block_download_metrics();

        // drain an element of the block buffer if there are any
        // 从block buffer排干一个element，如果有的话
        if let Some(block) = self.range_buffered_blocks.pop() {
            // peek ahead and pop duplicates
            while let Some(peek) = self.range_buffered_blocks.peek_mut() {
                if peek.0 .0.hash() == block.0 .0.hash() {
                    PeekMut::pop(peek);
                } else {
                    break
                }
            }
            return Poll::Ready(EngineSyncEvent::FetchedFullBlock(block.0 .0))
        }

        Poll::Pending
    }
}

/// A wrapper type around [SealedBlock] that implements the [Ord] trait by block number.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedSealedBlock(SealedBlock);

impl PartialOrd for OrderedSealedBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedSealedBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.number.cmp(&other.0.number)
    }
}

/// The event type emitted by the [EngineSyncController].
/// [EngineSyncControoler]发射的event类型
#[derive(Debug)]
pub(crate) enum EngineSyncEvent {
    /// A full block has been downloaded from the network.
    /// 一个完整的block已经从network下载
    FetchedFullBlock(SealedBlock),
    /// Pipeline started syncing
    /// Pipeline开始同步
    ///
    /// This is none if the pipeline is triggered without a specific target.
    /// 这是none，如果pipeline已经触发，没有一个特定的target
    PipelineStarted(Option<H256>),
    /// Pipeline finished
    /// Pipeline结束了
    ///
    /// If this is returned, the pipeline is idle.
    /// 如果这返回了，pipeline处于idle状态
    PipelineFinished {
        /// Final result of the pipeline run.
        /// pipeline运行的最后结果
        result: Result<ControlFlow, PipelineError>,
        /// Whether the pipeline reached the configured `max_block`.
        /// 是否pipeline到达了配置的`max_block`
        ///
        /// Note: this is only relevant in debugging scenarios.
        /// 注意：这只和debugging场景相关
        reached_max_block: bool,
    },
    /// Pipeline task was dropped after it was started, unable to receive it because channel
    /// closed. This would indicate a panicked pipeline task
    /// Pipeline task从它开始之后被丢弃，不能接收，因为channel被关闭，这会表明一个panicked pipeline
    /// task
    PipelineTaskDropped,
}

/// The possible pipeline states within the sync controller.
/// 在sync controller内pipeline states可能的状态
///
/// [PipelineState::Idle] means that the pipeline is currently idle.
/// [PipelineState::Idle]意味着pipeline当前处于idle状态
/// [PipelineState::Running] means that the pipeline is currently running.
/// [PipelineState::Running]意味着pipeline当前处于运行状态
///
/// NOTE: The differentiation between these two states is important, because when the pipeline is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
/// 注意：这两个states之间的不同非常重要，因为pipeline在运行时，它需要db的write
/// lock，这意味着我们不能转发任何message给blockchain tree，这会导致db写，所以这会导致deadlock
enum PipelineState<DB: Database> {
    /// Pipeline is idle.
    /// Pipeline处于idle
    Idle(Option<Pipeline<DB>>),
    /// Pipeline is running and waiting for a response
    /// Pipeline正在running并且等待一个response
    Running(oneshot::Receiver<PipelineWithResult<DB>>),
}

impl<DB: Database> PipelineState<DB> {
    /// Returns `true` if the state matches idle.
    fn is_idle(&self) -> bool {
        matches!(self, PipelineState::Idle(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::poll;
    use reth_db::{
        mdbx::{Env, WriteMap},
        test_utils::create_test_rw_db,
    };
    use reth_interfaces::{p2p::either::EitherDownloader, test_utils::TestFullBlockClient};
    use reth_primitives::{
        constants::ETHEREUM_BLOCK_GAS_LIMIT, stage::StageCheckpoint, BlockBody, ChainSpec,
        ChainSpecBuilder, Header, SealedHeader, MAINNET,
    };
    use reth_provider::{test_utils::TestExecutorFactory, BundleStateWithReceipts};
    use reth_stages::{test_utils::TestStages, ExecOutput, StageError};
    use reth_tasks::TokioTaskExecutor;
    use std::{collections::VecDeque, future::poll_fn, sync::Arc};
    use tokio::sync::watch;

    struct TestPipelineBuilder {
        // pipeline exec的outputs
        pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        // executor的结果
        executor_results: Vec<BundleStateWithReceipts>,
        // 最大的block
        max_block: Option<BlockNumber>,
    }

    impl TestPipelineBuilder {
        /// Create a new [TestPipelineBuilder].
        /// 创建一个新的[TestPipelineBuilder]
        fn new() -> Self {
            Self {
                pipeline_exec_outputs: VecDeque::new(),
                executor_results: Vec::new(),
                max_block: None,
            }
        }

        /// Set the pipeline execution outputs to use for the test consensus engine.
        fn with_pipeline_exec_outputs(
            mut self,
            pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        ) -> Self {
            self.pipeline_exec_outputs = pipeline_exec_outputs;
            self
        }

        /// Set the executor results to use for the test consensus engine.
        #[allow(dead_code)]
        fn with_executor_results(mut self, executor_results: Vec<BundleStateWithReceipts>) -> Self {
            self.executor_results = executor_results;
            self
        }

        /// Sets the max block for the pipeline to run.
        #[allow(dead_code)]
        fn with_max_block(mut self, max_block: BlockNumber) -> Self {
            self.max_block = Some(max_block);
            self
        }

        /// Builds the pipeline.
        /// 构建pipeline
        fn build(self, chain_spec: Arc<ChainSpec>) -> Pipeline<Arc<Env<WriteMap>>> {
            reth_tracing::init_test_tracing();
            let db = create_test_rw_db();

            // 构建executor factory
            let executor_factory = TestExecutorFactory::new(chain_spec.clone());
            executor_factory.extend(self.executor_results);

            // Setup pipeline
            // 设置pipeline
            let (tip_tx, _tip_rx) = watch::channel(H256::default());
            let mut pipeline = Pipeline::builder()
                // 添加stages
                .add_stages(TestStages::new(self.pipeline_exec_outputs, Default::default()))
                .with_tip_sender(tip_tx);

            if let Some(max_block) = self.max_block {
                // 设置max block
                pipeline = pipeline.with_max_block(max_block);
            }

            // 构建pipeline
            pipeline.build(db, chain_spec)
        }
    }

    struct TestSyncControllerBuilder<Client> {
        max_block: Option<BlockNumber>,
        client: Option<Client>,
    }

    impl<Client> TestSyncControllerBuilder<Client> {
        /// Create a new [TestSyncControllerBuilder].
        fn new() -> Self {
            Self { max_block: None, client: None }
        }

        /// Sets the max block for the pipeline to run.
        #[allow(dead_code)]
        fn with_max_block(mut self, max_block: BlockNumber) -> Self {
            self.max_block = Some(max_block);
            self
        }

        /// Sets the client to use for network operations.
        /// 设置client用于网络操作
        fn with_client(mut self, client: Client) -> Self {
            self.client = Some(client);
            self
        }

        /// Builds the sync controller.
        /// 构建sync controller
        fn build<DB>(
            self,
            pipeline: Pipeline<DB>,
            chain_spec: Arc<ChainSpec>,
        ) -> EngineSyncController<DB, EitherDownloader<Client, TestFullBlockClient>>
        where
            DB: Database + 'static,
            Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
        {
            // 配置client
            let client = self
                .client
                .map(EitherDownloader::Left)
                .unwrap_or_else(|| EitherDownloader::Right(TestFullBlockClient::default()));

            EngineSyncController::new(
                pipeline,
                client,
                Box::<TokioTaskExecutor>::default(),
                // run_pipeline_continuously: false here until we want to test this
                // run_pipeline_continuously: 这里为false，直到我们想要测试它
                false,
                self.max_block,
                chain_spec,
            )
        }
    }

    #[tokio::test]
    async fn pipeline_started_after_setting_target() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let client = TestFullBlockClient::default();
        let mut header = SealedHeader::default();
        let body = BlockBody::default();
        // 构建client，插入header和block
        client.insert(header.clone(), body.clone());
        for _ in 0..10 {
            // 插入10个header和block
            header.parent_hash = header.hash_slow();
            header.number += 1;
            header = header.header.seal_slow();
            client.insert(header.clone(), body.clone());
        }

        // force the pipeline to be "done" after 5 blocks
        // 强制pipeline在5个blocks之后完成
        let pipeline = TestPipelineBuilder::new()
            .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(5),
                done: true,
            })]))
            .build(chain_spec.clone());

        // 构建sync controller
        let mut sync_controller = TestSyncControllerBuilder::new()
            .with_client(client.clone())
            .build(pipeline, chain_spec);

        let tip = client.highest_block().expect("there should be blocks here");
        // 设置pipeline sync target
        sync_controller.set_pipeline_sync_target(tip.hash);

        let sync_future = poll_fn(|cx| sync_controller.poll(cx));
        let next_event = poll!(sync_future);

        // can assert that the first event here is PipelineStarted because we set the sync target,
        // and we should get Ready because the pipeline should be spawned immediately
        // 可以assert，第一个event是PipelineStarted，因为我们设置了sync
        // target并且我们应该获得Ready，因为pipeline应该立刻生成
        assert_matches!(next_event, Poll::Ready(EngineSyncEvent::PipelineStarted(Some(target))) => {
            assert_eq!(target, tip.hash);
        });

        // the next event should be the pipeline finishing in a good state
        // 下一个event应该是pipeline结束在一个good state
        let sync_future = poll_fn(|cx| sync_controller.poll(cx));
        let next_ready = sync_future.await;
        assert_matches!(next_ready, EngineSyncEvent::PipelineFinished { result, reached_max_block } => {
            assert_matches!(result, Ok(control_flow) => assert_eq!(control_flow, ControlFlow::Continue { block_number: 5 }));
            // no max block configured
            // 没有配置mac block
            assert!(!reached_max_block);
        });
    }

    #[tokio::test]
    async fn controller_sends_range_request() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let client = TestFullBlockClient::default();
        let mut header = Header {
            base_fee_per_gas: Some(7),
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            ..Default::default()
        }
        .seal_slow();
        let body = BlockBody::default();
        for _ in 0..10 {
            header.parent_hash = header.hash_slow();
            header.number += 1;
            header.timestamp += 1;
            header = header.header.seal_slow();
            client.insert(header.clone(), body.clone());
        }

        // set up a pipeline
        // 设置一个pipeline
        let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());

        let mut sync_controller = TestSyncControllerBuilder::new()
            .with_client(client.clone())
            .build(pipeline, chain_spec);

        let tip = client.highest_block().expect("there should be blocks here");

        // call the download range method
        // 调用download range方法
        sync_controller.download_block_range(tip.hash, tip.number);

        // ensure we have one in flight range request
        // 确保我们有一个in flight range request
        assert_eq!(sync_controller.inflight_block_range_requests.len(), 1);

        // ensure the range request is made correctly
        // 确保range request正确设置
        let first_req = sync_controller.inflight_block_range_requests.first().unwrap();
        assert_eq!(first_req.start_hash(), tip.hash);
        assert_eq!(first_req.count(), tip.number);

        // ensure they are in ascending order
        // 确保它们是升序的
        for num in 1..=10 {
            let sync_future = poll_fn(|cx| sync_controller.poll(cx));
            let next_ready = sync_future.await;
            assert_matches!(next_ready, EngineSyncEvent::FetchedFullBlock(block) => {
                assert_eq!(block.number, num);
            });
        }
    }
}
