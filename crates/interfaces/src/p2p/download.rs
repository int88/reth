use reth_primitives::PeerId;
use std::fmt::Debug;

/// Generic download client for peer penalization
/// 通用的download client用于Peer处罚
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    /// 处罚peer，对于回复一个message，违反了validation rules
    fn report_bad_message(&self, peer_id: PeerId);

    /// Returns how many peers the network is currently connected to.
    fn num_connected_peers(&self) -> usize;
}
