use parking_lot::RwLock;
use reth_primitives::{BlockNumHash, BlockNumber, ChainInfo, SealedHeader};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

/// Tracks the chain info: canonical head, safe block, finalized block.
/// 追踪chain信息：canonical head, safe block以及finalized block
#[derive(Debug, Clone)]
pub(crate) struct ChainInfoTracker {
    inner: Arc<ChainInfoInner>,
}

impl ChainInfoTracker {
    /// Create a new chain info container for the given canonical head.
    /// 用给定的canonical head创建一个新的chain info容器
    pub(crate) fn new(head: SealedHeader) -> Self {
        Self {
            inner: Arc::new(ChainInfoInner {
                last_forkchoice_update: RwLock::new(None),
                canonical_head_number: AtomicU64::new(head.number),
                canonical_head: RwLock::new(head),
                safe_block: RwLock::new(None),
                finalized_block: RwLock::new(None),
            }),
        }
    }

    /// Returns the [ChainInfo] for the canonical head.
    /// 返回canonical head的[ChainInfo]
    pub(crate) fn chain_info(&self) -> ChainInfo {
        let inner = self.inner.canonical_head.read();
        ChainInfo { best_hash: inner.hash(), best_number: inner.number }
    }

    /// Update the timestamp when we received a forkchoice update.
    /// 更新我们接收到forkchoice update的时间戳
    pub(crate) fn on_forkchoice_update_received(&self) {
        self.inner.last_forkchoice_update.write().replace(Instant::now());
    }

    /// Returns the instant when we received the latest forkchoice update.
    /// 返回我们接收到最新的forkchoice update的时间
    #[allow(unused)]
    pub(crate) fn last_forkchoice_update_received_at(&self) -> Option<Instant> {
        *self.inner.last_forkchoice_update.read()
    }

    /// Returns the canonical head of the chain.
    /// 返回chain的canonical head
    #[allow(unused)]
    pub(crate) fn get_canonical_head(&self) -> SealedHeader {
        self.inner.canonical_head.read().clone()
    }

    /// Returns the safe header of the chain.
    /// 返回chain的safe header
    #[allow(unused)]
    pub(crate) fn get_safe_header(&self) -> Option<SealedHeader> {
        self.inner.safe_block.read().clone()
    }

    /// Returns the finalized header of the chain.
    /// 返回chain的finalized header
    #[allow(unused)]
    pub(crate) fn get_finalized_header(&self) -> Option<SealedHeader> {
        self.inner.finalized_block.read().clone()
    }

    /// Returns the canonical head of the chain.
    #[allow(unused)]
    pub(crate) fn get_canonical_num_hash(&self) -> BlockNumHash {
        self.inner.canonical_head.read().num_hash()
    }

    /// Returns the canonical head of the chain.
    pub(crate) fn get_canonical_block_number(&self) -> BlockNumber {
        self.inner.canonical_head_number.load(Ordering::Relaxed)
    }

    /// Returns the safe header of the chain.
    #[allow(unused)]
    pub(crate) fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        let h = self.inner.safe_block.read();
        h.as_ref().map(|h| h.num_hash())
    }

    /// Returns the finalized header of the chain.
    #[allow(unused)]
    pub(crate) fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        let h = self.inner.finalized_block.read();
        h.as_ref().map(|h| h.num_hash())
    }

    /// Sets the canonical head of the chain.
    /// 设置chain的canonical head
    pub(crate) fn set_canonical_head(&self, header: SealedHeader) {
        let number = header.number;
        *self.inner.canonical_head.write() = header;

        // also update the atomic number.
        self.inner.canonical_head_number.store(number, Ordering::Relaxed);
    }

    /// Sets the safe header of the chain.
    /// 设置chain的safe header
    pub(crate) fn set_safe(&self, header: SealedHeader) {
        self.inner.safe_block.write().replace(header);
    }

    /// Sets the finalized header of the chain.
    /// 设置chain的finalized header
    pub(crate) fn set_finalized(&self, header: SealedHeader) {
        self.inner.finalized_block.write().replace(header);
    }
}

/// Container type for all chain info fields
/// 所有chain info字段的容器类型
#[derive(Debug)]
struct ChainInfoInner {
    /// Timestamp when we received the last fork choice update.
    ///
    /// This is mainly used to track if we're connected to a beacon node.
    last_forkchoice_update: RwLock<Option<Instant>>,
    /// Tracks the number of the `canonical_head`.
    canonical_head_number: AtomicU64,
    /// The canonical head of the chain.
    canonical_head: RwLock<SealedHeader>,
    /// The block that the beacon node considers safe.
    /// beacon node认为是安全的block
    safe_block: RwLock<Option<SealedHeader>>,
    /// The block that the beacon node considers finalized.
    /// beacon node认为finalized block
    finalized_block: RwLock<Option<SealedHeader>>,
}
