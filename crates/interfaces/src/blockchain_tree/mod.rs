use crate::{blockchain_tree::error::InsertBlockError, Error};
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, SealedBlock, SealedBlockWithSenders, SealedHeader,
};
use std::collections::{BTreeMap, HashSet};

pub mod error;

/// * [BlockchainTreeEngine::insert_block]: Connect block to chain, execute it and if valid insert
///   block inside tree.
/// * [BlockchainTreeEngine::insert_block]: 连接block到chain，执行它，如果有效则将block插入到tree中
/// * [BlockchainTreeEngine::finalize_block]: Remove chains that join to now finalized block, as
///   chain becomes invalid.
/// * [BlockchainTreeEngine::finalize_block]: 删除连接到现在finalized block的链，因为chain变得无效
/// * [BlockchainTreeEngine::make_canonical]: Check if we have the hash of block that we want to
///   finalize and commit it to db. If we don't have the block, syncing should start to fetch the
///   blocks from p2p. Do reorg in tables if canonical chain if needed.
/// * [BlockchainTreeEngine::make_canonical]: 检查我们是否有要finalize的block的hash，并将其提交到db中。
/// 如果我们没有block，同步应该开始从p2p获取block。如果需要，对tables进行reorg。
pub trait BlockchainTreeEngine: BlockchainTreeViewer + Send + Sync {
    /// Recover senders and call [`BlockchainTreeEngine::insert_block`].
    /// 恢复senders并调用BlockchainTreeEngine::insert_block
    ///
    /// This will recover all senders of the transactions in the block first, and then try to insert
    /// the block.
    /// 这会首先恢复block中所有交易的senders，然后尝试插入block
    fn insert_block_without_senders(
        &self,
        block: SealedBlock,
    ) -> Result<BlockStatus, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Recover senders and call [`BlockchainTreeEngine::buffer_block`].
    /// 恢复senders并调用BlockchainTreeEngine::buffer_block
    ///
    /// This will recover all senders of the transactions in the block first, and then try to buffer
    /// the block.
    /// 这会首先恢复block中所有交易的senders，然后尝试buffer block
    fn buffer_block_without_senders(&self, block: SealedBlock) -> Result<(), InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.buffer_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Buffer block with senders
    fn buffer_block(&self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError>;

    /// Insert block with senders
    fn insert_block(&self, block: SealedBlockWithSenders) -> Result<BlockStatus, InsertBlockError>;

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    /// Finalize blocks直到包括finalized_block，并将它们从tree中删除
    fn finalize_block(&self, finalized_block: BlockNumber);

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree.
    ///
    /// `N` is the `max_reorg_depth` plus the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTreeEngine::finalize_block`]).
    fn restore_canonical_hashes(&self, last_finalized_block: BlockNumber) -> Result<(), Error>;

    /// Make a block and its parent chain part of the canonical chain by committing it to the
    /// database.
    /// 将一个block及其parent chain作为canonical chain的一部分，通过将其提交到数据库中
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// re-orged.
    /// 这会解开数据库，如果有必要的话，即如果canonical chain的部分已经被re-orged
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    /// 返回Ok，如果blocks是canonical的，或者如果blocks已经是canonical的
    fn make_canonical(&self, block_hash: &BlockHash) -> Result<CanonicalOutcome, Error>;

    /// Unwind tables and put it inside state
    fn unwind(&self, unwind_to: BlockNumber) -> Result<(), Error>;
}

/// All possible outcomes of a canonicalization attempt of [BlockchainTreeEngine::make_canonical].
/// 所有的可能结果，尝试将blockchain tree的block变成canonical
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalOutcome {
    /// The block is already canonical.
    /// block已经是canonical的
    AlreadyCanonical {
        /// The corresponding [SealedHeader] that is already canonical.
        /// 对应的SealedHeader已经是canonical的
        header: SealedHeader,
    },
    /// Committed the block to the database.
    /// 提交block到数据库
    Committed {
        /// The new corresponding canonical head
        /// 新的canonical
        head: SealedHeader,
    },
}

impl CanonicalOutcome {
    /// Returns the header of the block that was made canonical.
    pub fn header(&self) -> &SealedHeader {
        match self {
            CanonicalOutcome::AlreadyCanonical { header } => header,
            CanonicalOutcome::Committed { head } => head,
        }
    }

    /// Consumes the outcome and returns the header of the block that was made canonical.
    pub fn into_header(self) -> SealedHeader {
        match self {
            CanonicalOutcome::AlreadyCanonical { header } => header,
            CanonicalOutcome::Committed { head } => head,
        }
    }
}

/// From Engine API spec, block inclusion can be valid, accepted or invalid.
/// Invalid case is already covered by error, but we need to make distinction
/// between if it is valid (extends canonical chain) or just accepted (is side chain).
/// If we don't know the block parent we are returning Disconnected status
/// as we can't make a claim if block is valid or not.
/// 基于Engine API spec, block inclusion可以是valid, accepted或者invalid
/// invalid case已经被error覆盖了，但是我们需要区分它是valid（扩展canonical chain）还是accepted（是side chain）
/// 如果我们不知道block parent，我们返回Disconnected status，因为我们不能断言block是valid还是invalid
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// If block validation is valid and block extends canonical chain.
    /// In BlockchainTree sense it forks on canonical tip.
    /// 如果block validation是valid，block扩展canonical chain
    /// 在BlockchainTree中，它在canonical tip上分叉
    Valid,
    /// If the block is valid, but it does not extend canonical chain
    /// (It is side chain) or hasn't been fully validated but ancestors of a payload are known.
    /// 如果block是valid的，但是它不扩展canonical chain（它是side chain）或者没有完全验证，但是payload的ancestors是已知的
    Accepted,
    /// If blocks is not connected to canonical chain.
    /// 如果block没有连接到canonical chain
    Disconnected {
        /// The lowest parent block that is not connected to the canonical chain.
        /// 最低的parent block，没有连接到canonical chain
        missing_parent: BlockNumHash,
    },
}

/// Allows read only functionality on the blockchain tree.
/// 允许对于blockchain tree的只读功能
///
/// Tree contains all blocks that are not canonical that can potentially be included
/// Tree包含所有的blocks，这些blocks不是canonical，但是可能被包含
/// as canonical chain. For better explanation we can group blocks into four groups:
/// 作为canonical chain。为了更好的解释，我们可以将blocks分为四组：
/// * Canonical chain blocks
/// * 权威的chain blocks
/// * Side chain blocks. Side chain are block that forks from canonical chain but not its tip.
/// * Side chain blocks，side chain是从canonical chain分叉出来的block，但不是它的tip
/// * Pending blocks that extend the canonical chain but are not yet included.
/// * Pending blocks，扩展canonical chain，但是还没有被包含
/// * Future pending blocks that extend the pending blocks.
/// * 未来的pending blocks，扩展pending blocks
pub trait BlockchainTreeViewer: Send + Sync {
    /// Returns both pending and side-chain block numbers and their hashes.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    fn blocks(&self) -> BTreeMap<BlockNumber, HashSet<BlockHash>>;

    /// Returns the header with matching hash from the tree, if it exists.
    ///
    /// Caution: This will not return headers from the canonical chain.
    fn header_by_hash(&self, hash: BlockHash) -> Option<SealedHeader>;

    /// Returns the block with matching hash from the tree, if it exists.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    fn block_by_hash(&self, hash: BlockHash) -> Option<SealedBlock>;

    /// Returns true if the tree contains the block with matching hash.
    fn contains(&self, hash: BlockHash) -> bool {
        self.block_by_hash(hash).is_some()
    }

    /// Canonical block number and hashes best known by the tree.
    fn canonical_blocks(&self) -> BTreeMap<BlockNumber, BlockHash>;

    /// Given the parent hash of a block, this tries to find the last ancestor that is part of the
    /// canonical chain.
    ///
    /// In other words, this will walk up the (side) chain starting with the given hash and return
    /// the first block that's canonical.
    ///
    /// Note: this could be the given `parent_hash` if it's already canonical.
    fn find_canonical_ancestor(&self, parent_hash: BlockHash) -> Option<BlockHash>;

    /// Given the hash of a block, this checks the buffered blocks for the lowest ancestor in the
    /// buffer.
    ///
    /// If there is a buffered block with the given hash, this returns the block itself.
    fn lowest_buffered_ancestor(&self, hash: BlockHash) -> Option<SealedBlockWithSenders>;

    /// Return BlockchainTree best known canonical chain tip (BlockHash, BlockNumber)
    fn canonical_tip(&self) -> BlockNumHash;

    /// Return block hashes that extends the canonical chain tip by one.
    /// This is used to fetch what is considered the pending blocks, blocks that
    /// has best chance to become canonical.
    /// 返回block hashes，扩展canonical chain tip by one
    /// 这用于获取被认为是pending blocks的blocks，这些blocks有最好的机会成为canonical
    fn pending_blocks(&self) -> (BlockNumber, Vec<BlockHash>);

    /// Return block number and hash that extends the canonical chain tip by one.
    ///
    /// If there is no such block, this returns `None`.
    fn pending_block_num_hash(&self) -> Option<BlockNumHash>;

    /// Returns the pending block if there is one.
    fn pending_block(&self) -> Option<SealedBlock> {
        self.block_by_hash(self.pending_block_num_hash()?.hash)
    }

    /// Returns the pending block if there is one.
    fn pending_header(&self) -> Option<SealedHeader> {
        self.header_by_hash(self.pending_block_num_hash()?.hash)
    }
}
