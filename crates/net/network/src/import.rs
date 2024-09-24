//! This module provides an abstraction over block import in the form of the `BlockImport` trait.
//! 这个mod提供了一个抽象，关于block import，以`BlockImport` trait的形式

use std::task::{Context, Poll};

use reth_network_peers::PeerId;

use crate::message::NewBlockMessage;

/// Abstraction over block import.
/// 关于block import的抽象
pub trait BlockImport: std::fmt::Debug + Send + Sync {
    /// Invoked for a received `NewBlock` broadcast message from the peer.
    /// 对于接收到的来自peer的`NewBlock`广播的调用
    ///
    /// > When a `NewBlock` announcement message is received from a peer, the client first verifies
    /// > the basic header validity of the block, checking whether the proof-of-work value is valid.
    /// > 当从peer接收到一个`NewBlock` annoucement message，client首先检验block的basic header
    /// > validity，检查pow的值是否合法
    ///
    /// This is supposed to start verification. The results are then expected to be returned via
    /// [`BlockImport::poll`].
    /// 这应该开始校验，results期望通过[`BlockImport::poll`]返回
    fn on_new_block(&mut self, peer_id: PeerId, incoming_block: NewBlockMessage);

    /// Returns the results of a [`BlockImport::on_new_block`]
    /// 返回[`BlockImport::on_new_block`]的结果
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BlockImportOutcome>;
}

/// Outcome of the [`BlockImport`]'s block handling.
/// [`BlockImport`]的block handling的结果
#[derive(Debug)]
pub struct BlockImportOutcome {
    /// Sender of the `NewBlock` message.
    pub peer: PeerId,
    /// The result after validating the block
    /// 校验block之后的结果
    pub result: Result<BlockValidation, BlockImportError>,
}

/// Represents the successful validation of a received `NewBlock` message.
/// 代表对于接收到的`NeBlock`的成功校验
#[derive(Debug)]
pub enum BlockValidation {
    /// Basic Header validity check, after which the block should be relayed to peers via a
    /// `NewBlock` message
    ValidHeader {
        /// received block
        block: NewBlockMessage,
    },
    /// Successfully imported: state-root matches after execution. The block should be relayed via
    /// `NewBlockHashes`
    ValidBlock {
        /// validated block.
        block: NewBlockMessage,
    },
}

/// Represents the error case of a failed block import
#[derive(Debug, thiserror::Error)]
pub enum BlockImportError {
    /// Consensus error
    #[error(transparent)]
    Consensus(#[from] reth_consensus::ConsensusError),
}

/// An implementation of `BlockImport` used in Proof-of-Stake consensus that does nothing.
///
/// Block propagation over devp2p is invalid in POS: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProofOfStakeBlockImport;

impl BlockImport for ProofOfStakeBlockImport {
    fn on_new_block(&mut self, _peer_id: PeerId, _incoming_block: NewBlockMessage) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<BlockImportOutcome> {
        Poll::Pending
    }
}
