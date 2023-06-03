//! Sync management for the engine implementation.

use futures::FutureExt;
use reth_db::database::Database;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    full_block::{FetchFullBlockFuture, FullBlockClient},
    headers::client::HeadersClient,
};
use reth_primitives::{BlockNumber, SealedBlock, H256};
use reth_stages::{ControlFlow, Pipeline, PipelineError, PipelineWithResult};
use reth_tasks::TaskSpawner;
use std::{
    collections::VecDeque,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::trace;

/// Manages syncing under the control of the engine.
/// 在engine的控制下管理同步
///
/// This type controls the [Pipeline] and supports (single) full block downloads.
/// 这个类型控制Pipeline并支持（单个）完整的区块下载
///
/// Caution: If the pipeline is running, this type will not emit blocks downloaded from the network
/// [EngineSyncEvent::FetchedFullBlock] until the pipeline is idle to prevent commits to the
/// database while the pipeline is still active.
/// 注意：如果pipeline正在运行，这个类型将不会从网络中发出下载的区块，直到pipeline空闲为止，以防止在pipeline仍然活动时对数据库的提交
pub(crate) struct EngineSyncController<DB, Client>
where
    DB: Database,
    Client: HeadersClient + BodiesClient,
{
    /// A downloader that can download full blocks from the network.
    /// 一个downloader，可以从网络中下载完整的区块
    full_block_client: FullBlockClient<Client>,
    /// The type that can spawn the pipeline task.
    /// 可以生成pipeline task的类型
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    /// The current state of the pipeline.
    /// The pipeline is used for large ranges.
    /// pipeline的当前状态。pipeline用于大范围
    pipeline_state: PipelineState<DB>,
    /// Pending target block for the pipeline to sync
    /// pipeline同步的pending target block
    pending_pipeline_target: Option<H256>,
    /// In requests in progress.
    /// 在处理过程中的请求
    inflight_full_block_requests: Vec<FetchFullBlockFuture<Client>>,
    /// Buffered events until the manager is polled and the pipeline is idle.
    /// 缓存的events，直到manager被轮询并且pipeline空闲
    queued_events: VecDeque<EngineSyncEvent>,
    /// If enabled, the pipeline will be triggered continuously, as soon as it becomes idle
    run_pipeline_continuously: bool,
    /// Max block after which the consensus engine would terminate the sync. Used for debugging
    /// purposes.
    /// 最大的block，之后共识引擎将终止同步。用于调试目的
    max_block: Option<BlockNumber>,
}

impl<DB, Client> EngineSyncController<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    /// Create a new instance
    /// 创建一个新的实例
    pub(crate) fn new(
        pipeline: Pipeline<DB>,
        client: Client,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        run_pipeline_continuously: bool,
        max_block: Option<BlockNumber>,
    ) -> Self {
        Self {
            full_block_client: FullBlockClient::new(client),
            pipeline_task_spawner,
            pipeline_state: PipelineState::Idle(Some(pipeline)),
            pending_pipeline_target: None,
            inflight_full_block_requests: Vec::new(),
            queued_events: VecDeque::new(),
            run_pipeline_continuously,
            max_block,
        }
    }

    /// Sets the max block value for testing
    #[cfg(test)]
    pub(crate) fn set_max_block(&mut self, block: BlockNumber) {
        self.max_block = Some(block);
    }

    /// Cancels all full block requests that are in progress.
    pub(crate) fn clear_full_block_requests(&mut self) {
        self.inflight_full_block_requests.clear();
    }

    /// Cancels the full block request with the given hash.
    pub(crate) fn cancel_full_block_request(&mut self, hash: H256) {
        self.inflight_full_block_requests.retain(|req| *req.hash() != hash);
    }

    /// Returns whether or not the sync controller is set to run the pipeline continuously.
    pub(crate) fn run_pipeline_continuously(&self) -> bool {
        self.run_pipeline_continuously
    }

    /// Returns `true` if the pipeline is idle.
    /// 返回`true`，如果pipeline是空闲的
    pub(crate) fn is_pipeline_idle(&self) -> bool {
        self.pipeline_state.is_idle()
    }

    /// Returns true if there's already a request for the given hash.
    pub(crate) fn is_inflight_request(&self, hash: H256) -> bool {
        self.inflight_full_block_requests.iter().any(|req| *req.hash() == hash)
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
            target: "consensus::engine",
            ?hash,
            "start downloading full block."
        );
        let request = self.full_block_client.get_full_block(hash);
        self.inflight_full_block_requests.push(request);
        true
    }

    /// Sets a new target to sync the pipeline to.
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
                target: "consensus::engine",
                ?progress,
                max_block = ?self.max_block,
                "Consensus engine reached max block."
            );
        }
        has_reached_max_block
    }

    /// Advances the pipeline state.
    ///
    /// This checks for the result in the channel, or returns pending if the pipeline is idle.
    fn poll_pipeline(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        let res = match self.pipeline_state {
            PipelineState::Idle(_) => return Poll::Pending,
            PipelineState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pipeline, result)) => {
                let minimum_progress = pipeline.minimum_progress();
                let reached_max_block =
                    self.has_reached_max_block(minimum_progress.unwrap_or_default());
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
    /// 这会在pipeline空闲且设置了target或者pipeline设置为连续运行时，生成pipeline
    fn try_spawn_pipeline(&mut self) -> Option<EngineSyncEvent> {
        match &mut self.pipeline_state {
            PipelineState::Idle(pipeline) => {
                let target = self.pending_pipeline_target.take();

                if target.is_none() && !self.run_pipeline_continuously {
                    // nothing to sync
                    return None
                }

                let (tx, rx) = oneshot::channel();

                let pipeline = pipeline.take().expect("exists");
                self.pipeline_task_spawner.spawn_critical_blocking(
                    "pipeline task",
                    Box::pin(async move {
                        let result = pipeline.run_as_fut(target).await;
                        let _ = tx.send(result);
                    }),
                );
                self.pipeline_state = PipelineState::Running(rx);

                // we also clear any pending full block requests because we expect them to be
                // outdated (included in the range the pipeline is syncing anyway)
                self.clear_full_block_requests();

                Some(EngineSyncEvent::PipelineStarted(target))
            }
            PipelineState::Running(_) => None,
        }
    }

    /// Advances the sync process.
    /// 推进同步进程
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<EngineSyncEvent> {
        // try to spawn a pipeline if a target is set
        // 试着生成一个pipeline，如果设置了target
        if let Some(event) = self.try_spawn_pipeline() {
            return Poll::Ready(event)
        }

        loop {
            // drain buffered events first if pipeline is not running
            if self.is_pipeline_idle() {
                if let Some(event) = self.queued_events.pop_front() {
                    return Poll::Ready(event)
                }
            } else {
                // advance the pipeline
                if let Poll::Ready(event) = self.poll_pipeline(cx) {
                    return Poll::Ready(event)
                }
            }

            // advance all requests
            for idx in (0..self.inflight_full_block_requests.len()).rev() {
                let mut request = self.inflight_full_block_requests.swap_remove(idx);
                if let Poll::Ready(block) = request.poll_unpin(cx) {
                    self.queued_events.push_back(EngineSyncEvent::FetchedFullBlock(block));
                } else {
                    // still pending
                    self.inflight_full_block_requests.push(request);
                }
            }

            if !self.pipeline_state.is_idle() || self.queued_events.is_empty() {
                // can not make any progress
                return Poll::Pending
            }
        }
    }
}

/// The event type emitted by the [EngineSyncController].
/// [EngineSyncController]发出的事件类型
#[derive(Debug)]
pub(crate) enum EngineSyncEvent {
    /// A full block has been downloaded from the network.
    /// 已经从network下载了一个完整的区块
    FetchedFullBlock(SealedBlock),
    /// Pipeline started syncing
    /// Pipeline开始同步
    ///
    /// This is none if the pipeline is triggered without a specific target.
    /// 如果pipeline被触发，没有特定的target，这个是none
    PipelineStarted(Option<H256>),
    /// Pipeline finished
    /// Pipeline结束了
    ///
    /// If this is returned, the pipeline is idle.
    /// 如果返回了，pipeline是空闲的
    PipelineFinished {
        /// Final result of the pipeline run.
        /// Pipeline运行的最终结果
        result: Result<ControlFlow, PipelineError>,
        /// Whether the pipeline reached the configured `max_block`.
        /// pipeline是否dao达到了配置的`max_block`
        ///
        /// Note: this is only relevant in debugging scenarios.
        /// 注意：这只在调试场景中有用
        reached_max_block: bool,
    },
    /// Pipeline task was dropped after it was started, unable to receive it because channel
    /// closed. This would indicate a panicked pipeline task
    /// Pipeline task被丢弃了，因为它已经开始了，无法接收它，因为channel关闭了。这表明了一个恐慌的pipeline task
    PipelineTaskDropped,
}

/// The possible pipeline states within the sync controller.
/// sync controller内可能的pipeline状态
///
/// [PipelineState::Idle] means that the pipeline is currently idle.
/// [PipelineState::Idle]意味着pipeline当前处于空闲状态
/// [PipelineState::Running] means that the pipeline is currently running.
/// [PipelineState::Running]意味着pipeline当前正在运行
///
/// NOTE: The differentiation between these two states is important, because when the pipeline is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
enum PipelineState<DB: Database> {
    /// Pipeline is idle.
    Idle(Option<Pipeline<DB>>),
    /// Pipeline is running and waiting for a response
    Running(oneshot::Receiver<PipelineWithResult<DB>>),
}

impl<DB: Database> PipelineState<DB> {
    /// Returns `true` if the state matches idle.
    fn is_idle(&self) -> bool {
        matches!(self, PipelineState::Idle(_))
    }
}
