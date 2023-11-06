//! Support for building payloads.
//! 支持构建payloads
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.
//! payload builder负责构建paylodas，一旦新的payload被创建，它持续更新

use crate::{
    error::PayloadBuilderError, metrics::PayloadBuilderServiceMetrics, traits::PayloadJobGenerator,
    BuiltPayload, KeepPayloadJobAlive, PayloadBuilderAttributes, PayloadJob,
};
use futures_util::{future::FutureExt, StreamExt};
use reth_rpc_types::engine::PayloadId;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{trace, warn};

/// A communication channel to the [PayloadBuilderService] that can retrieve payloads.
#[derive(Debug, Clone)]
pub struct PayloadStore {
    inner: PayloadBuilderHandle,
}

// === impl PayloadStore ===

impl PayloadStore {
    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// Note: depending on the installed [PayloadJobGenerator], this may or may not terminate the
    /// job, See [PayloadJob::resolve].
    pub async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        self.inner.resolve(id).await
    }

    /// Returns the best payload for the given identifier.
    ///
    /// Note: this merely returns the best payload so far and does not resolve the job.
    pub async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        self.inner.best_payload(id).await
    }

    /// Returns the payload attributes associated with the given identifier.
    ///
    /// Note: this returns the attributes of the payload and does not resolve the job.
    pub async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<PayloadBuilderAttributes, PayloadBuilderError>> {
        self.inner.payload_attributes(id).await
    }
}

impl From<PayloadBuilderHandle> for PayloadStore {
    fn from(inner: PayloadBuilderHandle) -> Self {
        Self { inner }
    }
}

/// A communication channel to the [PayloadBuilderService].
/// 用于和[PayloadBuilderService]交互的channel
///
/// This is the API used to create new payloads and to get the current state of existing ones.
/// 这是API用于创建新的payloads并且获取existing ones的当前状态
#[derive(Debug, Clone)]
pub struct PayloadBuilderHandle {
    /// Sender half of the message channel to the [PayloadBuilderService].
    /// 通往[PayloadBuilderService]的发送部分
    to_service: mpsc::UnboundedSender<PayloadServiceCommand>,
}

// === impl PayloadBuilderHandle ===

impl PayloadBuilderHandle {
    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// Note: depending on the installed [PayloadJobGenerator], this may or may not terminate the
    /// job, See [PayloadJob::resolve].
    pub async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::Resolve(id, tx)).ok()?;
        match rx.await.transpose()? {
            Ok(fut) => Some(fut.await),
            Err(e) => Some(Err(e.into())),
        }
    }

    /// Returns the best payload for the given identifier.
    pub async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::BestPayload(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Returns the payload attributes associated with the given identifier.
    ///
    /// Note: this returns the attributes of the payload and does not resolve the job.
    pub async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<PayloadBuilderAttributes, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::PayloadAttributes(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Sends a message to the service to start building a new payload for the given payload.
    /// 发送一个message到service来开始构建一个新的payload，对于给定的payload
    ///
    /// This is the same as [PayloadBuilderHandle::new_payload] but does not wait for the result and
    /// returns the receiver instead
    /// 这和[PayloadBuilderHandle::new_payload]一样，但是不等待结果，而是返回receiver
    pub fn send_new_payload(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> oneshot::Receiver<Result<PayloadId, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_service.send(PayloadServiceCommand::BuildNewPayload(attr, tx));
        rx
    }

    /// Starts building a new payload for the given payload attributes.
    ///
    /// Returns the identifier of the payload.
    ///
    /// Note: if there's already payload in progress with same identifier, it will be returned.
    pub async fn new_payload(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> Result<PayloadId, PayloadBuilderError> {
        self.send_new_payload(attr).await?
    }
}

/// A service that manages payload building tasks.
/// 一个service用于管理payload的构建task
///
/// This type is an endless future that manages the building of payloads.
/// 这个类型是一个endless future，管理payloads的构建
///
/// It tracks active payloads and their build jobs that run in a worker pool.
/// 它追踪active payloads并且他们的build jobs运行在一个worker pool
///
/// By design, this type relies entirely on the [`PayloadJobGenerator`] to create new payloads and
/// does know nothing about how to build them, it just drives their jobs to completion.
/// 按照设计，这个类型完全依赖[`PayloadJobGenerator`]来创建新的payloads并且不知道如何构建它，
/// 只是驱动它们的jobs完成
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator,
{
    /// The type that knows how to create new payloads.
    /// 这个类型知道如何创建新的payloads
    generator: Gen,
    /// All active payload jobs.
    /// 所有的active payload jobs
    payload_jobs: Vec<(Gen::Job, PayloadId)>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    /// sender部分的拷贝，这样新的 [`PayloadBuilderHandle`]可以按需创建
    _service_tx: mpsc::UnboundedSender<PayloadServiceCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand>,
    /// Metrics for the payload builder service
    metrics: PayloadBuilderServiceMetrics,
}

// === impl PayloadBuilderService ===

impl<Gen> PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator,
{
    /// Creates a new payload builder service.
    /// 生成一个新的payload buidler服务
    pub fn new(generator: Gen) -> (Self, PayloadBuilderHandle) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let service = Self {
            generator,
            payload_jobs: Vec::new(),
            _service_tx: service_tx.clone(),
            command_rx: UnboundedReceiverStream::new(command_rx),
            metrics: Default::default(),
        };
        let handle = PayloadBuilderHandle { to_service: service_tx };
        (service, handle)
    }

    /// Returns true if the given payload is currently being built.
    fn contains_payload(&self, id: PayloadId) -> bool {
        self.payload_jobs.iter().any(|(_, job_id)| *job_id == id)
    }

    /// Returns the best payload for the given identifier that has been built so far.
    /// 返回当前已经构建的，对于给定id的best payload
    fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        self.payload_jobs.iter().find(|(_, job_id)| *job_id == id).map(|(j, _)| j.best_payload())
    }

    /// Returns the payload attributes for the given payload.
    fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<PayloadBuilderAttributes, PayloadBuilderError>> {
        self.payload_jobs
            .iter()
            .find(|(_, job_id)| *job_id == id)
            .map(|(j, _)| j.payload_attributes())
    }

    /// Returns the best payload for the given identifier that has been built so far and terminates
    /// the job if requested.
    /// 返回对于给定的id，当前已经构建的最好的payload，并且终结job，如果需求的话
    fn resolve(&mut self, id: PayloadId) -> Option<PayloadFuture> {
        let job = self.payload_jobs.iter().position(|(_, job_id)| *job_id == id)?;
        let (fut, keep_alive) = self.payload_jobs[job].0.resolve();

        if keep_alive == KeepPayloadJobAlive::No {
            let (_, id) = self.payload_jobs.remove(job);
            trace!(%id, "terminated resolved job");
        }

        Some(Box::pin(fut))
    }
}

impl<Gen> Future for PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator + Unpin + 'static,
    <Gen as PayloadJobGenerator>::Job: Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            // we poll all jobs first, so we always have the latest payload that we can report if
            // requests
            // 我们首先轮询所有的jobs，这样我们总是有最新的payload，我们可以汇报，如果请求的话
            // we don't care about the order of the jobs, so we can just swap_remove them
            // 我们不关心jobs的顺序，这样我们可以swap_remove他们
            for idx in (0..this.payload_jobs.len()).rev() {
                let (mut job, id) = this.payload_jobs.swap_remove(idx);

                // drain better payloads from the job
                // 从job排干更好的payloads
                match job.poll_unpin(cx) {
                    Poll::Ready(Ok(_)) => {
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                        trace!(%id, "payload job finished");
                    }
                    Poll::Ready(Err(err)) => {
                        warn!(?err, ?id, "Payload builder job failed; resolving payload");
                        this.metrics.inc_failed_jobs();
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                    }
                    Poll::Pending => {
                        // still pending, put it back
                        this.payload_jobs.push((job, id));
                    }
                }
            }

            // marker for exit condition
            let mut new_job = false;

            // drain all requests
            // 排干所有的请求
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(attr, tx) => {
                        let id = attr.payload_id();
                        let mut res = Ok(id);

                        if this.contains_payload(id) {
                            // payload job已经在进行了
                            warn!(%id, parent = ?attr.parent, "Payload job already in progress, ignoring.");
                        } else {
                            // no job for this payload yet, create one
                            // 对于这个payload，没有job，创建一个
                            match this.generator.new_payload_job(attr) {
                                Ok(job) => {
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push((job, id));
                                }
                                Err(err) => {
                                    this.metrics.inc_failed_jobs();
                                    warn!(?err, %id, "Failed to create payload builder job");
                                    res = Err(err);
                                }
                            }
                        }

                        // return the id of the payload
                        // 返回payload的id
                        let _ = tx.send(res);
                    }
                    PayloadServiceCommand::BestPayload(id, tx) => {
                        let _ = tx.send(this.best_payload(id));
                    }
                    PayloadServiceCommand::PayloadAttributes(id, tx) => {
                        let _ = tx.send(this.payload_attributes(id));
                    }
                    PayloadServiceCommand::Resolve(id, tx) => {
                        let _ = tx.send(this.resolve(id));
                    }
                }
            }

            if !new_job {
                return Poll::Pending
            }
        }
    }
}

type PayloadFuture =
    Pin<Box<dyn Future<Output = Result<Arc<BuiltPayload>, PayloadBuilderError>> + Send>>;

/// Message type for the [PayloadBuilderService].
/// 对于[PayloadBuilderService]的Message类型
enum PayloadServiceCommand {
    /// Start building a new payload.
    /// 开始构建一个新的payload
    BuildNewPayload(
        PayloadBuilderAttributes,
        oneshot::Sender<Result<PayloadId, PayloadBuilderError>>,
    ),
    /// Get the best payload so far
    /// 获取当前最好的payload
    BestPayload(PayloadId, oneshot::Sender<Option<Result<Arc<BuiltPayload>, PayloadBuilderError>>>),
    /// Get the payload attributes for the given payload
    /// 获取给定payload的payload attributes
    PayloadAttributes(
        PayloadId,
        oneshot::Sender<Option<Result<PayloadBuilderAttributes, PayloadBuilderError>>>,
    ),
    /// Resolve the payload and return the payload
    /// 解析payload并且返回payload
    Resolve(PayloadId, oneshot::Sender<Option<PayloadFuture>>),
}

impl fmt::Debug for PayloadServiceCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadServiceCommand::BuildNewPayload(f0, f1) => {
                f.debug_tuple("BuildNewPayload").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::BestPayload(f0, f1) => {
                f.debug_tuple("BestPayload").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::PayloadAttributes(f0, f1) => {
                f.debug_tuple("PayloadAttributes").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::Resolve(f0, _f1) => f.debug_tuple("Resolve").field(&f0).finish(),
        }
    }
}
