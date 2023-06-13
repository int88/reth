/// Trait definition for [`HeadersClient`]
/// 对于[`HeadersClient`]的trait定义
///
/// [`HeadersClient`]: client::HeadersClient
pub mod client;

/// A downloader that receives and verifies block headers, is generic
/// over the Consensus and the HeadersClient being used.
/// 一个downloader，它接收并验证block headers，它是Consensus和HeadersClient的泛型
///
/// [`Consensus`]: crate::consensus::Consensus
/// [`HeadersClient`]: client::HeadersClient
pub mod downloader;
