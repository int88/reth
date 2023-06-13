use crate::{
    consensus::Consensus,
    p2p::error::{DownloadError, DownloadResult},
};
use futures::Stream;
use reth_primitives::{BlockHashOrNumber, SealedHeader, H256};

/// A downloader capable of fetching and yielding block headers.
/// 一个downloader可以获取和产生区块头
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient][crate::p2p::headers::client::HeadersClient] represents a client capable
/// of fulfilling these requests.
/// 一个downloader代表一个不同的策略来提交下载区块头的请求，而一个HeadersClient代表一个客户端能够满足这些请求
///
/// A [HeaderDownloader] is a [Stream] that returns batches of headers.
pub trait HeaderDownloader: Send + Sync + Stream<Item = Vec<SealedHeader>> + Unpin {
    /// Updates the gap to sync which ranges from local head to the sync target
    /// 更新gap to sync，它的范围是从本地head到sync target
    ///
    /// See also [HeaderDownloader::update_sync_target] and [HeaderDownloader::update_local_head]
    /// 同时见HeaderDownloader::update_sync_target和HeaderDownloader::update_local_head
    fn update_sync_gap(&mut self, head: SealedHeader, target: SyncTarget) {
        self.update_local_head(head);
        self.update_sync_target(target);
    }

    /// Updates the block number of the local database
    /// 更新本地数据库的block number
    fn update_local_head(&mut self, head: SealedHeader);

    /// Updates the target we want to sync to
    /// 更新我们想要同步的目标
    fn update_sync_target(&mut self, target: SyncTarget);

    /// Sets the headers batch size that the Stream should return.
    /// 设置Stream应该返回的headers batch size
    fn set_batch_size(&mut self, limit: usize);
}

/// Specifies the target to sync for [HeaderDownloader::update_sync_target]
/// 指定sync的target
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SyncTarget {
    /// This represents a range missing headers in the form of `(head,..`
    /// 这代表一个range missing headers，形式为`(head,..`
    ///
    /// Sync _inclusively_ to the given block hash.
    ///
    /// This target specifies the upper end of the sync gap `(head...tip]`
    /// 这个target指定了sync gap的上限
    Tip(H256),
    /// This represents a gap missing headers bounded by the given header `h` in the form of
    /// `(head,..h),h+1,h+2...`
    /// 这代表一个gap missing headers，形式为`(head,..h),h+1,h+2...`
    ///
    /// Sync _exclusively_ to the given header's parent which is: `(head..h-1]`
    ///
    /// The benefit of this variant is, that this already provides the block number of the highest
    /// missing block.
    /// 这个variant的好处是，它已经提供了最高缺失块的block number
    Gap(SealedHeader),
    /// This represents a tip by block number
    /// 这代表一个tip，通过block number
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
/// 校验header是否和它的parent相关
///
/// Returns Ok(false) if the
pub fn validate_header_download(
    consensus: &dyn Consensus,
    header: &SealedHeader,
    parent: &SealedHeader,
) -> DownloadResult<()> {
    ensure_parent(header, parent)?;
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

/// Ensures that the given `parent` header is the actual parent of the `header`
pub fn ensure_parent(header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
    if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
        return Err(DownloadError::MismatchedHeaders {
            header_number: header.number,
            parent_number: parent.number,
            header_hash: header.hash(),
            parent_hash: parent.hash(),
        })
    }
    Ok(())
}
