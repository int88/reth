//! Wrapper around BlockchainTree that allows for it to be shared.
//! 对于允许共享的BlockchainTree的包装
use super::BlockchainTree;
use parking_lot::RwLock;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{
        error::InsertBlockError, BlockStatus, BlockchainTreeEngine, BlockchainTreeViewer,
        CanonicalOutcome,
    },
    consensus::Consensus,
    Error,
};
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, SealedBlock, SealedBlockWithSenders, SealedHeader,
};
use reth_provider::{
    BlockchainTreePendingStateProvider, CanonStateSubscriptions, ExecutorFactory,
    PostStateDataProvider,
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};
use tracing::trace;

/// Shareable blockchain tree that is behind tokio::RwLock
/// 共享的blockchain tree，在tokio::RwLock后面
#[derive(Clone)]
pub struct ShareableBlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// BlockchainTree
    /// 一个BlockchainTree
    pub tree: Arc<RwLock<BlockchainTree<DB, C, EF>>>,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> ShareableBlockchainTree<DB, C, EF> {
    /// Create a new sharable database.
    pub fn new(tree: BlockchainTree<DB, C, EF>) -> Self {
        Self { tree: Arc::new(RwLock::new(tree)) }
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeEngine
    for ShareableBlockchainTree<DB, C, EF>
{
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        self.tree.write().buffer_block(block)
    }

    fn insert_block(&self, block: SealedBlockWithSenders) -> Result<BlockStatus, InsertBlockError> {
        trace!(target: "blockchain_tree", ?block, "Inserting block");
        self.tree.write().insert_block(block)
    }

    fn finalize_block(&self, finalized_block: BlockNumber) {
        trace!(target: "blockchain_tree", ?finalized_block, "Finalizing block");
        self.tree.write().finalize_block(finalized_block)
    }

    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<(), Error> {
        trace!(target: "blockchain_tree", ?last_finalized_block, "Restoring canonical hashes for last finalized block");
        self.tree.write().restore_canonical_hashes(last_finalized_block)
    }

    fn make_canonical(&self, block_hash: &BlockHash) -> Result<CanonicalOutcome, Error> {
        trace!(target: "blockchain_tree", ?block_hash, "Making block canonical");
        self.tree.write().make_canonical(block_hash)
    }

    fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error> {
        trace!(target: "blockchain_tree", ?unwind_to, "Unwinding to block number");
        self.tree.write().unwind(unwind_to)
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreeViewer
    for ShareableBlockchainTree<DB, C, EF>
{
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>> {
        trace!(target: "blockchain_tree", "Returning all blocks in blockchain tree");
        self.tree.read().block_indices().block_number_to_block_hashes().clone()
    }

    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader> {
        trace!(target: "blockchain_tree", ?hash, "Returning header by hash");
        self.tree.read().block_by_hash(hash).map(|b| b.header.clone())
    }

    fn block_by_hash(&self, block_hash: BlockHash) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", ?block_hash, "Returning block by hash");
        self.tree.read().block_by_hash(block_hash).cloned()
    }

    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash> {
        trace!(target: "blockchain_tree", "Returning canonical blocks in tree");
        self.tree.read().block_indices().canonical_chain().inner().clone()
    }

    fn find_canonical_ancestor(&self, mut parent: BlockHash) -> Option<BlockHash> {
        let tree = self.tree.read();

        // walk up the tree and check if the parent is in the sidechain
        while let Some(block) = tree.block_by_hash(parent) {
            parent = block.parent_hash;
        }

        if tree.block_indices().is_block_hash_canonical(&parent) {
            return Some(parent)
        }

        None
    }

    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders> {
        trace!(target: "blockchain_tree", ?hash, "Returning lowest buffered ancestor");
        self.tree.read().lowest_buffered_ancestor(&hash).cloned()
    }

    fn canonical_tip(&self) -> BlockNumHash {
        trace!(target: "blockchain_tree", "Returning canonical tip");
        self.tree.read().block_indices().canonical_tip()
    }

    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>) {
        trace!(target: "blockchain_tree", "Returning all pending blocks");
        self.tree.read().block_indices().pending_blocks()
    }

    fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().block_indices().pending_block_num_hash()
    }

    fn pending_block(&self) -> Option<SealedBlock> {
        trace!(target: "blockchain_tree", "Returning first pending block");
        self.tree.read().pending_block().cloned()
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTreePendingStateProvider
    for ShareableBlockchainTree<DB, C, EF>
{
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn PostStateDataProvider>> {
        trace!(target: "blockchain_tree", ?block_hash, "Finding pending state provider");
        let provider = self.tree.read().post_state_data(block_hash)?;
        Some(Box::new(provider))
    }
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> CanonStateSubscriptions
    for ShareableBlockchainTree<DB, C, EF>
{
    fn subscribe_to_canonical_state(&self) -> reth_provider::CanonStateNotifications {
        trace!(target: "blockchain_tree", "Registered subscriber for canonical state");
        self.tree.read().subscribe_canon_state()
    }
}
