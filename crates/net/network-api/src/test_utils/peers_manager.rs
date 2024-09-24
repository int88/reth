//! Interaction with `reth_network::PeersManager`, for integration testing. Otherwise
//! `reth_network::NetworkManager` manages `reth_network::PeersManager`.

use std::net::SocketAddr;

use derive_more::Constructor;
use reth_network_peers::{NodeRecord, PeerId};
use reth_network_types::{Peer, ReputationChangeKind};
use tokio::sync::{mpsc, oneshot};

/// Provides an API for managing the peers of the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait PeersHandleProvider {
    /// Returns the [`PeersHandle`] that can be cloned and shared.
    ///
    /// The [`PeersHandle`] can be used to interact with the network's peer set.
    fn peers_handle(&self) -> &PeersHandle;
}

/// A communication channel to the `PeersManager` to apply manual changes to the peer set.
/// 一个到`PeerManager`的communication channel，来应用changes到peer set
#[derive(Clone, Debug, Constructor)]
pub struct PeersHandle {
    /// Sender half of command channel back to the `PeersManager`
    /// command channel的Sender部分，发送命令到`PeersManager`
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
}

// === impl PeersHandle ===

impl PeersHandle {
    fn send(&self, cmd: PeerCommand) {
        let _ = self.manager_tx.send(cmd);
    }

    /// Adds a peer to the set.
    pub fn add_peer(&self, peer_id: PeerId, addr: SocketAddr) {
        self.send(PeerCommand::Add(peer_id, addr));
    }

    /// Removes a peer from the set.
    pub fn remove_peer(&self, peer_id: PeerId) {
        self.send(PeerCommand::Remove(peer_id));
    }

    /// Send a reputation change for the given peer.
    pub fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        self.send(PeerCommand::ReputationChange(peer_id, kind));
    }

    /// Returns a peer by its [`PeerId`], or `None` if the peer is not in the peer set.
    pub async fn peer_by_id(&self, peer_id: PeerId) -> Option<Peer> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeer(peer_id, tx));

        rx.await.unwrap_or(None)
    }

    /// Returns all peers in the peerset.
    pub async fn all_peers(&self) -> Vec<NodeRecord> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeers(tx));

        rx.await.unwrap_or_default()
    }
}

/// Commands the `PeersManager` listens for.
/// `PeerManager`监听的commands
#[derive(Debug)]
pub enum PeerCommand {
    /// Command for manually add
    /// 手动添加的命令
    Add(PeerId, SocketAddr),
    /// Remove a peer from the set
    /// 从集合中移除一个peer
    ///
    /// If currently connected this will disconnect the session
    /// 如果当前是已经连接的，会断开session
    Remove(PeerId),
    /// Apply a reputation change to the given peer.
    /// 对给定的peer应用reputation change
    ReputationChange(PeerId, ReputationChangeKind),
    /// Get information about a peer
    /// 获取关于一个peer的信息
    GetPeer(PeerId, oneshot::Sender<Option<Peer>>),
    /// Get node information on all peers
    /// 获取关于所有peers的node信息
    GetPeers(oneshot::Sender<Vec<NodeRecord>>),
}
