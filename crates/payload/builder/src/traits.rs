//! Trait abstractions used by the payload crate.
//! 由payload crate使用的trait抽象

use crate::{error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes};
use std::{future::Future, sync::Arc};

/// A type that can build a payload.
///
/// This type is a Future that resolves when the job is done (e.g. timed out) or it failed. It's not
/// supposed to return the best payload built when it resolves instead [PayloadJob::best_payload]
/// should be used for that.
///
/// A PayloadJob must always be prepared to return the best payload built so far to make there's a
/// valid payload to deliver to the CL, so it does not miss a slot, even if the payload is empty.
///
/// Note: A PayloadJob need to be cancel safe because it might be dropped after the CL has requested the payload via `engine_getPayloadV1`, See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1>
pub trait PayloadJob: Future<Output = Result<(), PayloadBuilderError>> + Send + Sync {
    /// Represents the future that resolves the block that's returned to the CL.
    type ResolvePayloadFuture: Future<Output = Result<Arc<BuiltPayload>, PayloadBuilderError>>
        + Send
        + Sync
        + 'static;

    /// Returns the best payload that has been built so far.
    ///
    /// Note: This is never called by the CL.
    fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError>;

    /// Called when the payload is requested by the CL.
    ///
    /// This is invoked on [`engine_getPayloadV2`](https://github.com/ethereum/execution-apis/blob/main/src/engine/shanghai.md#engine_getpayloadv2) and [`engine_getPayloadV1`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#engine_getpayloadv1).
    ///
    /// The timeout for returning the payload to the CL is 1s.
    /// Ideally, future returned by this method must resolve in under 1s. Ideally this is the best
    /// payload built so far or an empty block without transactions if nothing has been built yet.
    ///
    /// According to the spec:
    /// > Client software MAY stop the corresponding build process after serving this call.
    ///
    /// It is at the discretion of the implementer whether the build job should be kept alive or
    /// terminated.
    ///
    /// If this returns [KeepPayloadJobAlive::Yes] then the future the [PayloadJob] will be polled
    /// once more, if this returns [KeepPayloadJobAlive::No] then the [PayloadJob] will be dropped
    /// after this call
    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive);
}

/// Whether the payload job should be kept alive or terminated after the payload was requested by
/// the CL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepPayloadJobAlive {
    /// Keep the job alive.
    Yes,
    /// Terminate the job.
    No,
}

/// A type that knows how to create new jobs for creating payloads.
/// 一个类型知道如何创建新的jobs来创建payloads
pub trait PayloadJobGenerator: Send + Sync {
    /// The type that manages the lifecycle of a payload.
    /// 这个类型管理payload的生命周期
    ///
    /// This type is a Stream that yields better payloads.
    /// 这个类型是一个Stream，它产生更好的payloads
    type Job: PayloadJob;

    /// Creates the initial payload and a new [PayloadJob] that yields better payloads.
    /// 创建初始的payload和一个新的PayloadJob，它产生更好的payloads
    ///
    /// This is called when the CL requests a new payload job via a fork choice update.
    /// 这是当CL通过fork choice update请求一个新的payload job时调用的
    ///
    /// Note: this is expected to build a new (empty) payload without transactions, so it can be
    /// returned directly. when asked for
    /// 注意：这期望构建一个新的（空的）payload，没有transactions，所以它可以直接返回。当被要求时
    fn new_payload_job(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError>;
}
