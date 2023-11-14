use super::response::BlockResponse;
use crate::p2p::error::DownloadResult;
use futures::Stream;
use reth_primitives::BlockNumber;
use std::ops::RangeInclusive;

/// Body downloader return type.
/// Body downloader的返回类型
pub type BodyDownloaderResult = DownloadResult<Vec<BlockResponse>>;

/// A downloader capable of fetching and yielding block bodies from block headers.
/// 一个downloader能够抓取并且生成block bodies，从block headers
///
/// A downloader represents a distinct strategy for submitting requests to download block bodies,
/// 一个downloader代表一个独特的策略用于提交请求来下载block bodies
/// while a [BodiesClient][crate::p2p::bodies::client::BodiesClient] represents a client capable of
/// fulfilling these requests.
/// 同时[BodiesClient][crate::p2p::bodies::client::BodiesClient]代表一个client能够填充这些请求
pub trait BodyDownloader: Send + Sync + Stream<Item = BodyDownloaderResult> + Unpin {
    /// Method for setting the download range.
    /// 方法用于设置download range
    fn set_download_range(&mut self, range: RangeInclusive<BlockNumber>) -> DownloadResult<()>;
}
