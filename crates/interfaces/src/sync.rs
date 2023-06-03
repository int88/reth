//! Traits used when interacting with the sync status of the network.
//! 用于和network的sync status进行交互的trait
use reth_primitives::Head;

/// A type that provides information about whether the node is currently syncing and the network is
/// currently serving syncing related requests.
/// 一个类型，提供关于node是否正在syncing和network是否正在提供syncing相关请求的信息
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SyncStateProvider: Send + Sync {
    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;
}

/// An updater for updating the [SyncState] and status of the network.
/// 一个updater，用于更新[SyncState]和network的status
///
/// The node is either syncing, or it is idle.
/// node要么在syncing，要么是idle
/// While syncing, the node will download data from the network and process it. The processing
/// consists of several stages, like recovering senders, executing the blocks and indexing.
/// Eventually the node reaches the `Finish` stage and will transition to [`SyncState::Idle`], it
/// which point the node is considered fully synced.
/// 当处于syncing状态时，node会从network下载数据并处理它。处理包括几个阶段，比如恢复senders，执行blocks和索引。
/// 最终node到达`Finish`阶段，并且会转换到[`SyncState::Idle`]，此时node被认为是完全synced的。
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait NetworkSyncUpdater: std::fmt::Debug + Send + Sync + 'static {
    /// Notifies about an [SyncState] update.
    /// 通知关于[SyncState]的更新
    fn update_sync_state(&self, state: SyncState);

    /// Updates the status of the p2p node
    /// 更新p2p node的状态
    fn update_status(&self, head: Head);
}

/// The state the network is currently in when it comes to synchronization.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum SyncState {
    /// Node sync is complete.
    /// Node同步已经完成
    ///
    /// The network just serves requests to keep up of the chain.
    /// network只是提供了chain的keep up请求
    Idle,
    /// Network is syncing
    /// Network正在同步
    Syncing,
}

impl SyncState {
    /// Whether the node is currently syncing.
    ///
    /// Note: this does not include keep-up sync when the state is idle.
    pub fn is_syncing(&self) -> bool {
        !matches!(self, SyncState::Idle)
    }
}

/// A [NetworkSyncUpdater] implementation that does nothing.
/// 一个什么都不做的[NetworkSyncUpdater]实现
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopSyncStateUpdater;

impl SyncStateProvider for NoopSyncStateUpdater {
    fn is_syncing(&self) -> bool {
        false
    }
}

impl NetworkSyncUpdater for NoopSyncStateUpdater {
    fn update_sync_state(&self, _state: SyncState) {}
    fn update_status(&self, _: Head) {}
}
