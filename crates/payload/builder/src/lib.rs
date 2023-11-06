//! This crate defines abstractions to create and update payloads (blocks):
//! - [`PayloadJobGenerator`]: a type that knows how to create new jobs for creating payloads based
//!   on [`PayloadAttributes`](reth_rpc_types::engine::PayloadAttributes).
//! - [`PayloadJob`]: a type that yields (better) payloads over time.
//! 这个crate定义了抽象来创建以及更新payloads（blocks）
//! - [`PayloadJobGenerator`]：是一个类型知道如何创建新的jobs用于创建paylods，
//!   基于[`PayloadAttributes`](reth_rpc_types::engine::PayloadAttributes)
//! - [`PayloadJob`]:是一个类型每次生成payloads
//!
//! This crate comes with the generic [`PayloadBuilderService`] responsible for managing payload
//! jobs.
//! 这个crate负责通用的[`PayloadBuilderService`]负责管理payload jobs
//!
//! ## Node integration
//!
//! In a standard node the [`PayloadBuilderService`] sits downstream of the engine API, or rather
//! the component that handles requests from the consensus layer like `engine_forkchoiceUpdatedV1`.
//! 在标准node，[`PayloadBuilderService`]是engine API的downstream，或者是组件负责处理来自consensus
//! layer的请求，例如`engine_forkchoiceUpdatedV1`
//!
//! Payload building is enabled if the forkchoice update request contains payload attributes.
//! Payload building被使能，如果forkchoice update请求包含payload attributes
//!
//! See also [the engine API docs](https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_forkchoiceupdatedv2)
//! If the forkchoice update request is `VALID` and contains payload attributes the
//! [`PayloadBuilderService`] will create a new [`PayloadJob`] via the given [`PayloadJobGenerator`]
//! and start polling it until the payload is requested by the CL and the payload job is resolved
//! (see [`PayloadJob::resolve`]).
//! 如果forkchoide update请求是`VALID`并且包含payload特性，
//! [`PayloadBuilderService`]会创建一个新的[`PayloadJob`]通过给定的[`PayloadJobGenerator`]并且开始轮询，直到payload被CL请求并且paylod job被解析
//!
//! ## Example
//!
//! A simple example of a [`PayloadJobGenerator`] that creates empty blocks:
//!
//! ```
//! use std::future::Future;
//! use std::pin::Pin;
//! use std::sync::Arc;
//! use std::task::{Context, Poll};
//! use reth_payload_builder::{BuiltPayload, KeepPayloadJobAlive, PayloadBuilderAttributes, PayloadJob, PayloadJobGenerator};
//! use reth_payload_builder::error::PayloadBuilderError;
//! use reth_primitives::{Block, Header, U256};
//!
//! /// The generator type that creates new jobs that builds empty blocks.
//! pub struct EmptyBlockPayloadJobGenerator;
//!
//! impl PayloadJobGenerator for EmptyBlockPayloadJobGenerator {
//!     type Job = EmptyBlockPayloadJob;
//!
//! /// This is invoked when the node receives payload attributes from the beacon node via `engine_forkchoiceUpdatedV1`
//! /// 当node从beacon node接收到payload attributes时被调用，通过`engine_forkchoiceUpdatedV1`
//! fn new_payload_job(&self, attr: PayloadBuilderAttributes) -> Result<Self::Job, PayloadBuilderError> {
//!         Ok(EmptyBlockPayloadJob{ attributes: attr,})
//!     }
//!
//! }
//!
//! /// A [PayloadJob] that builds empty blocks.
//! /// 一个[PayloadJob]构建空的blocks
//! pub struct EmptyBlockPayloadJob {
//!   attributes: PayloadBuilderAttributes,
//! }
//!
//! impl PayloadJob for EmptyBlockPayloadJob {
//!    type ResolvePayloadFuture = futures_util::future::Ready<Result<Arc<BuiltPayload>, PayloadBuilderError>>;
//!
//! fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError> {
//!     // NOTE: some fields are omitted here for brevity
//!     let payload = Block {
//!         header: Header {
//!             parent_hash: self.attributes.parent,
//!             timestamp: self.attributes.timestamp,
//!             beneficiary: self.attributes.suggested_fee_recipient,
//!             ..Default::default()
//!         },
//!         ..Default::default()
//!     };
//!     let payload = BuiltPayload::new(self.attributes.id, payload.seal_slow(), U256::ZERO);
//!     Ok(Arc::new(payload))
//! }
//!
//! fn payload_attributes(&self) -> Result<PayloadBuilderAttributes, PayloadBuilderError> {
//!     Ok(self.attributes.clone())
//! }
//!
//! fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
//!        let payload = self.best_payload();
//!        (futures_util::future::ready(payload), KeepPayloadJobAlive::No)
//!     }
//! }
//!
//! /// A [PayloadJob] is a a future that's being polled by the `PayloadBuilderService`
//! impl Future for EmptyBlockPayloadJob {
//!  type Output = Result<(), PayloadBuilderError>;
//!
//! fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//!         Poll::Pending
//!     }
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

pub mod database;
pub mod error;
mod metrics;
mod payload;
mod service;
mod traits;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

pub use payload::{BuiltPayload, PayloadBuilderAttributes};
pub use reth_rpc_types::engine::PayloadId;
pub use service::{PayloadBuilderHandle, PayloadBuilderService, PayloadStore};
pub use traits::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};
