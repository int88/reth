use crate::consensus::ConsensusError;
use reth_primitives::SealedHeader;
use thiserror::Error;

/// Header downloader result
/// Header下载的结果
pub type HeadersDownloaderResult<T> = Result<T, HeadersDownloaderError>;

/// Error variants that can happen when sending requests to a session.
/// 当发送requests到一个session可以发生的各种错误
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum HeadersDownloaderError {
    /// The downloaded header cannot be attached to the local head,
    /// but is valid otherwise.
    /// 下载的header不能关联到local head，但是是合法的
    #[error("Valid downloaded header cannot be attached to the local head. Details: {error}.")]
    DetachedHead {
        /// The local head we attempted to attach to.
        /// 我们尝试关联的local head
        local_head: SealedHeader,
        /// The header we attempted to attach.
        /// 我们尝试关联的header
        header: SealedHeader,
        /// The error that occurred when attempting to attach the header.
        /// 当尝试关联到header时产生的error
        error: Box<ConsensusError>,
    },
}
