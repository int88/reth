use super::response::BlockResponse;
use crate::p2p::error::DownloadResult;
use futures::Stream;
use reth_primitives::BlockNumber;
use std::ops::RangeInclusive;

/// Body downloader return type.
/// Body downloader返回类型
pub type BodyDownloaderResult = DownloadResult<Vec<BlockResponse>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
/// downloader可以从block headers获取和产生block bodies
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// while a [BodiesClient][crate::p2p::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
/// 一个downloader代表一个不同的策略来提交下载block bodies的请求，而一个BodiesClient代表一个客户端能够满足这些请求
pub trait BodyDownloader: Send + Sync + Stream<Item = BodyDownloaderResult> + Unpin {
    /// Method for setting the download range.
    /// 用于设置下载range的方法
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
