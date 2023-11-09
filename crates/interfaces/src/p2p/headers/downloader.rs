use super::error::HeadersDownloaderResult;
use crate::{
    consensus::Consensus,
    p2p::error::{DownloadError, DownloadResult},
};
use futures::Stream;
use reth_primitives::{BlockHashOrNumber, SealedHeader, H256};

/// A downloader capable of fetching and yielding block headers.
/// 一个downloader能够抓取以及生成block headers
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient][crate::p2p::headers::client::HeadersClient] represents a client capable
/// of fulfilling these requests.
/// 一个downloader代表一个明确的策略，用于提交请求，来下载block headers，同时
/// [HeadersClient][crate::p2p::headers::client::HeadersClient]
///
/// A [HeaderDownloader] is a [Stream] that returns batches of headers.
/// 一个[HeaderDownloader]是一个[Stream]，返回一系列的headers
pub trait HeaderDownloader:
    Send + Sync + Stream<Item = HeadersDownloaderResult<Vec<SealedHeader>>> + Unpin
{
    /// Updates the gap to sync which ranges from local head to the sync target
    /// 更新gap，从local head到sync target，
    ///
    /// See also [HeaderDownloader::update_sync_target] and [HeaderDownloader::update_local_head]
    fn update_sync_gap(&mut self, head: SealedHeader, target: SyncTarget) {
        self.update_local_head(head);
        self.update_sync_target(target);
    }

    /// Updates the block number of the local database
    /// 更新本地db的block number
    fn update_local_head(&mut self, head: SealedHeader);

    /// Updates the target we want to sync to
    /// 更新我们想要同步的target
    fn update_sync_target(&mut self, target: SyncTarget);

    /// Sets the headers batch size that the Stream should return.
    /// 返回Stream应该返回的headers batch size
    fn set_batch_size(&mut self, limit: usize);
}

/// Specifies the target to sync for [HeaderDownloader::update_sync_target]
/// 指定同步的target，对于[HeaderDownloader::update_sync_target]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SyncTarget {
    /// This represents a range missing headers in the form of `(head,..`
    /// 这代表一个范围的missing headers，以形式`(head,...`
    ///
    /// Sync _inclusively_ to the given block hash.
    /// 同步包括到给定的block hash
    ///
    /// This target specifies the upper end of the sync gap `(head...tip]`
    /// 这个target指定了sync gap的upper end，即`(head...tip]`
    Tip(H256),
    /// This represents a gap missing headers bounded by the given header `h` in the form of
    /// 这代表一个missing header的gap，通过给定header `h`为界
    /// `(head,..h),h+1,h+2...`
    ///
    /// Sync _exclusively_ to the given header's parent which is: `(head..h-1]`
    /// 精确同步到给定header的parent，为`(head...h-1]`
    ///
    /// The benefit of this variant is, that this already provides the block number of the highest
    /// missing block.
    /// 这种类型的好处是，这已经提供了最高缺失的block的准确block number
    Gap(SealedHeader),
    /// This represents a tip by block number
    /// 通过block number代表一个tip
    TipNum(u64),
}

// === impl SyncTarget ===

impl SyncTarget {
    /// Returns the tip to sync to _inclusively_
    ///
    /// This returns the hash if the target is [SyncTarget::Tip] or the `parent_hash` of the given
    /// header in [SyncTarget::Gap]
    pub fn tip(&self) -> BlockHashOrNumber {
        match self {
            SyncTarget::Tip(tip) => (*tip).into(),
            SyncTarget::Gap(gap) => gap.parent_hash.into(),
            SyncTarget::TipNum(num) => (*num).into(),
        }
    }
}

/// Validate whether the header is valid in relation to it's parent
///
/// Returns Ok(false) if the
pub fn validate_header_download(
    consensus: &dyn Consensus,
    header: &SealedHeader,
    parent: &SealedHeader,
) -> DownloadResult<()> {
    // validate header against parent
    consensus
        .validate_header_against_parent(header, parent)
        .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
    // validate header standalone
    consensus
        .validate_header(header)
        .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
    Ok(())
}
