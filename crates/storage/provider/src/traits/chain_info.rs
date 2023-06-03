use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::SealedHeader;
use std::time::Instant;

/// A type that can track updates related to fork choice updates.
/// 一个类型，追踪与fork choice updates相关的更新
pub trait CanonChainTracker: Send + Sync {
    /// Notify the tracker about a received fork choice update.
    fn on_forkchoice_update_received(&self, update: &ForkchoiceState);

    /// Returns the last time a fork choice update was received from the CL
    /// ([CanonChainTracker::on_forkchoice_update_received])
    fn last_received_update_timestamp(&self) -> Option<Instant>;

    /// Sets the canonical head of the chain.
    fn set_canonical_head(&self, header: SealedHeader);

    /// Sets the safe block of the chain.
    fn set_safe(&self, header: SealedHeader);

    /// Sets the finalized block of the chain.
    fn set_finalized(&self, header: SealedHeader);
}
