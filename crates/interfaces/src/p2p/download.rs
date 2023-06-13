use reth_primitives::PeerId;
use std::fmt::Debug;

/// Generic download client for peer penalization
/// 通用的下载client，用于peer的惩罚
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait DownloadClient: Send + Sync + Debug {
    /// Penalize the peer for responding with a message
    /// that violates validation rules
    /// 惩罚peer，因为它响应了一个违反验证规则的消息
    fn report_bad_message(&self, peer_id: PeerId);

    /// Returns how many peers the network is currently connected to.
    /// 返回当前网络连接的peer的数量
    fn num_connected_peers(&self) -> usize;
}
