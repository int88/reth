//! Implementation of [`BlockchainTree`]
use crate::{
    canonical_chain::CanonicalChain,
    chain::{BlockChainId, BlockKind},
    AppendableChain, BlockBuffer, BlockIndices, BlockchainTreeConfig, PostStateData, TreeExternals,
};
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{
    blockchain_tree::{
        error::{BlockchainTreeError, InsertBlockError, InsertBlockErrorKind},
        BlockStatus, CanonicalOutcome,
    },
    consensus::{Consensus, ConsensusError},
    executor::BlockExecutionError,
    Error,
};
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, ForkBlock, Hardfork, SealedBlock, SealedBlockWithSenders,
    SealedHeader, U256,
};
use reth_provider::{
    chain::{ChainSplit, SplitAt},
    post_state::PostState,
    BlockNumProvider, CanonStateNotification, CanonStateNotificationSender,
    CanonStateNotifications, Chain, ExecutorFactory, HeaderProvider, Transaction,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tracing::{debug, error, info, instrument, trace};

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Tree of chains and its identifications.
/// chains的tree及其标识
///
/// Mermaid flowchart represent all blocks that can appear in blockchain.
/// Green blocks belong to canonical chain and are saved inside database table, they are our main
/// chain. Pending blocks and sidechains are found in memory inside [`BlockchainTree`].
/// Both pending and sidechains have same mechanisms only difference is when they got committed to
/// database. For pending it is just append operation but for sidechains they need to move current
/// canonical blocks to BlockchainTree flush sidechain to the database to become canonical chain.
/// ```mermaid
/// flowchart BT
/// subgraph canonical chain
/// CanonState:::state
/// block0canon:::canon -->block1canon:::canon -->block2canon:::canon -->block3canon:::canon --> block4canon:::canon --> block5canon:::canon
/// end
/// block5canon --> block6pending1:::pending
/// block5canon --> block6pending2:::pending
/// subgraph sidechain2
/// S2State:::state
/// block3canon --> block4s2:::sidechain --> block5s2:::sidechain
/// end
/// subgraph sidechain1
/// S1State:::state
/// block2canon --> block3s1:::sidechain --> block4s1:::sidechain --> block5s1:::sidechain --> block6s1:::sidechain
/// end
/// classDef state fill:#1882C4
/// classDef canon fill:#8AC926
/// classDef pending fill:#FFCA3A
/// classDef sidechain fill:#FF595E
/// ```
///
///
/// main functions:
/// * [BlockchainTree::insert_block]: Connect block to chain, execute it and if valid insert block
///   inside tree.
/// * [BlockchainTree::insert_block]: 连接block到chain，执行它，如果有效，将block插入到tree中
/// * [BlockchainTree::finalize_block]: Remove chains that join to now finalized block, as chain
///   becomes invalid.
/// * [BlockchainTree::finalize_block]: 移除已经加入到现在finalized block的chain，因为chain变得无效
/// * [BlockchainTree::make_canonical]: Check if we have the hash of block that we want to finalize
///   and commit it to db. If we don't have the block, pipeline syncing should start to fetch the
///   blocks from p2p. Do reorg in tables if canonical chain if needed.
/// * [BlockchainTree::make_canonical]: 检查我们是否有我们想要finalized的block的hash，并将其提交到db
///  如果我们没有这个block，pipeline syncing应该开始从p2p获取block。如果需要，对tables进行reorg
#[derive(Debug)]
pub struct BlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    /// The tracked chains and their current data.
    /// 追踪chains以及它们的状态
    chains: HashMap<BlockChainId, AppendableChain>,
    /// Unconnected block buffer.
    /// 没有连接的block buffer
    buffered_blocks: BlockBuffer,
    /// Static blockchain ID generator
    /// 静态的blockchain ID生成器
    block_chain_id_generator: u64,
    /// Indices to block and their connection to the canonical chain.
    /// 到block的索引及其与canonical chain的连接
    block_indices: BlockIndices,
    /// External components (the database, consensus engine etc.)
    /// 扩展的组件（database，consensus engine等）
    externals: TreeExternals<DB, C, EF>,
    /// Tree configuration
    /// Tree的配置
    config: BlockchainTreeConfig,
    /// Broadcast channel for canon state changes notifications.
    /// 广播通道，用于canon state changes notifications
    canon_state_notification_sender: CanonStateNotificationSender,
}

/// A container that wraps chains and block indices to allow searching for block hashes across all
/// sidechains.
pub struct BlockHashes<'a> {
    /// The current tracked chains.
    pub chains: &'a mut HashMap<BlockChainId, AppendableChain>,
    /// The block indices for all chains.
    pub indices: &'a BlockIndices,
}

impl<DB: Database, C: Consensus, EF: ExecutorFactory> BlockchainTree<DB, C, EF> {
    /// Create a new blockchain tree.
    /// 创建要给新的blockchain tree
    pub fn new(
        externals: TreeExternals<DB, C, EF>,
        canon_state_notification_sender: CanonStateNotificationSender,
        config: BlockchainTreeConfig,
    ) -> Result<Self, Error> {
        let max_reorg_depth = config.max_reorg_depth();

        let last_canonical_hashes = externals
            .db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take((max_reorg_depth + config.num_of_additional_canonical_block_hashes()) as usize)
            .collect::<Result<Vec<(BlockNumber, BlockHash)>, _>>()?;

        // TODO(rakita) save last finalized block inside database but for now just take
        // tip-max_reorg_depth
        // task: https://github.com/paradigmxyz/reth/issues/1712
        let (last_finalized_block_number, _) =
            if last_canonical_hashes.len() > max_reorg_depth as usize {
                last_canonical_hashes[max_reorg_depth as usize]
            } else {
                // it is in reverse order from tip to N
                // 反向顺序从tip到N
                last_canonical_hashes.last().cloned().unwrap_or_default()
            };

        Ok(Self {
            externals,
            // 构建block buffer
            buffered_blocks: BlockBuffer::new(config.max_unconnected_blocks()),
            block_chain_id_generator: 0,
            chains: Default::default(),
            // 构建block indices
            block_indices: BlockIndices::new(
                last_finalized_block_number,
                BTreeMap::from_iter(last_canonical_hashes.into_iter()),
            ),
            config,
            canon_state_notification_sender,
        })
    }

    /// Check if then block is known to blockchain tree or database and return its status.
    /// 检查是否block已知于blockchain tree或database，并返回其状态
    ///
    /// Function will check:
    /// * if block is inside database and return [BlockStatus::Valid] if it is.
    /// * 如果block是在database中，如果是，则返回BlockStatus::Valid
    /// * if block is inside buffer and return [BlockStatus::Disconnected] if it is.
    /// * 如果block是在buffer中，如果是，则返回BlockStatus::Disconnected
    /// * if block is part of the side chain and return [BlockStatus::Accepted] if it is.
    /// * 如果block是side chain的一部分，如果是，则返回BlockStatus::Accepted
    /// * if block is part of the canonical chain that tree knows, return [BlockStatus::Valid], if
    ///   it is.
    /// * 如果block是tree知道的canonical chain的一部分，如果是，则返回BlockStatus::Valid
    ///
    /// Returns an error if
    /// 返回一个errorr，如果
    ///    - an error occurred while reading from the database.
    ///    - 一个error发生在从database中读取时
    ///    - the block is already finalized
    ///    - block已经finalized
    pub(crate) fn is_block_known(
        &self,
        block: BlockNumHash,
    ) -> Result<Option<BlockStatus>, InsertBlockErrorKind> {
        let last_finalized_block = self.block_indices.last_finalized_block();
        // check db if block is finalized.
        // 检查db，如果block已经finalized
        if block.number <= last_finalized_block {
            // check if block is canonical
            // 检查block是否是canonical
            if self.is_block_hash_canonical(&block.hash)? {
                return Ok(Some(BlockStatus::Valid))
            }

            // check if block is inside database
            // 见擦汗block是否在database中
            if self.externals.database().provider()?.block_number(block.hash)?.is_some() {
                return Ok(Some(BlockStatus::Valid))
            }

            return Err(BlockchainTreeError::PendingBlockIsFinalized {
                last_finalized: last_finalized_block,
            }
            .into())
        }

        // check if block is part of canonical chain
        // 检查block是否是canonical chain的一部分
        if self.is_block_hash_canonical(&block.hash)? {
            return Ok(Some(BlockStatus::Valid))
        }

        // is block inside chain
        // block是否在chain中
        if let Some(status) = self.is_block_inside_chain(&block) {
            return Ok(Some(status))
        }

        // check if block is disconnected
        // 检查block是否是disconnected
        if let Some(block) = self.buffered_blocks.block(block) {
            return Ok(Some(BlockStatus::Disconnected { missing_parent: block.parent_num_hash() }))
        }

        Ok(None)
    }

    /// Expose internal indices of the BlockchainTree.
    #[inline]
    pub fn block_indices(&self) -> &BlockIndices {
        &self.block_indices
    }

    #[inline]
    fn canonical_chain(&self) -> &CanonicalChain {
        self.block_indices.canonical_chain()
    }

    /// Returns the block with matching hash from any side-chain.
    ///
    /// Caution: This will not return blocks from the canonical chain.
    pub fn block_by_hash(&self, block_hash: BlockHash) -> Option<&SealedBlock> {
        let id = self.block_indices.get_blocks_chain_id(&block_hash)?;
        let chain = self.chains.get(&id)?;
        chain.block(block_hash)
    }

    /// Returns true if the block is included in a side-chain.
    /// 返回true，如果block包含在side-chain中
    fn is_block_hash_inside_chain(&self, block_hash: BlockHash) -> bool {
        self.block_by_hash(block_hash).is_some()
    }

    /// Returns the block that's considered the `Pending` block, if it exists.
    pub fn pending_block(&self) -> Option<&SealedBlock> {
        let b = self.block_indices.pending_block_num_hash()?;
        self.block_by_hash(b.hash)
    }

    /// Return items needed to execute on the pending state.
    /// 返回items用于在pending state上执行
    /// This includes:
    ///     * `BlockHash` of canonical block that chain connects to. Needed for creating database
    ///       provider for the rest of the state.
    ///     * chains连接的canonical block的`BlockHash`，需要为state的其余部分创建database provider
    ///     * `PostState` changes that happened at the asked `block_hash`
    ///     * 在`block_hash`发生的`PostState`变化
    ///     * `BTreeMap<BlockNumber,BlockHash>` list of past pending and canonical hashes, That are
    ///       needed for evm `BLOCKHASH` opcode.
    ///     * 过去pending和canonical hashes的`BTreeMap<BlockNumber,BlockHash>`列表，这些是evm `BLOCKHASH` opcode所需的
    /// Return none if block is not known.
    pub fn post_state_data(&self, block_hash: BlockHash) -> Option<PostStateData> {
        trace!(target: "blockchain_tree", ?block_hash, "Searching for post state data");
        // if it is part of the chain
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block_hash) {
            trace!(target: "blockchain_tree", ?block_hash, "Constructing post state data based on non-canonical chain");
            // get block state
            let chain = self.chains.get(&chain_id).expect("Chain should be present");
            let block_number = chain.block_number(block_hash)?;
            let state = chain.state_at_block(block_number)?;

            // get parent hashes
            let mut parent_block_hashed = self.all_chain_hashes(chain_id);
            let first_pending_block_number =
                *parent_block_hashed.first_key_value().expect("There is at least one block hash").0;
            let canonical_chain = self
                .canonical_chain()
                .iter()
                .filter(|&(key, _)| key < first_pending_block_number)
                .collect::<Vec<_>>();
            parent_block_hashed.extend(canonical_chain.into_iter());

            // get canonical fork.
            let canonical_fork = self.canonical_fork(chain_id)?;
            return Some(PostStateData { state, parent_block_hashed, canonical_fork })
        }

        // check if there is canonical block
        if let Some(canonical_number) = self.canonical_chain().canonical_number(block_hash) {
            trace!(target: "blockchain_tree", ?block_hash, "Constructing post state data based on canonical chain");
            return Some(PostStateData {
                canonical_fork: ForkBlock { number: canonical_number, hash: block_hash },
                state: PostState::new(),
                parent_block_hashed: self.canonical_chain().inner().clone(),
            })
        }

        None
    }

    /// Try inserting a validated [Self::validate_block] block inside the tree.
    /// 试着插入一个合法的[Self::validate_block] block到tree中
    ///
    /// If blocks does not have parent [`BlockStatus::Disconnected`] would be returned, in which
    /// case it is buffered for future inclusion.
    /// 如果blocks没有parent，将返回[`BlockStatus::Disconnected`]，在这种情况下，它将被缓冲以供将来使用
    #[instrument(skip_all, fields(block = ?block.num_hash()), target = "blockchain_tree", ret)]
    fn try_insert_validated_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, InsertBlockError> {
        debug_assert!(self.validate_block(&block).is_ok(), "Block must be validated");

        let parent = block.parent_num_hash();

        // check if block parent can be found in Tree
        // 检查是否block parent能从Tree中找到
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&parent.hash) {
            // found parent in side tree, try to insert there
            // 在side tree中找到parent，试着插入它
            return self.try_insert_block_into_side_chain(block, chain_id)
        }

        // if not found, check if the parent can be found inside canonical chain.
        // 如果没有找到，检查parent是否能在canonical chain中找到
        if self
            .is_block_hash_canonical(&parent.hash)
            .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?
        {
            // 试着扩展canonical chain
            return self.try_append_canonical_chain(block)
        }

        // this is another check to ensure that if the block points to a canonical block its block
        // is valid
        // 检查block是否指向一个canonical block，它的block是否有效
        if let Some(canonical_parent_number) =
            self.block_indices.canonical_number(block.parent_hash)
        {
            // we found the parent block in canonical chain
            // 在canonical chain中找到parent
            if canonical_parent_number != parent.number {
                return Err(InsertBlockError::consensus_error(
                    ConsensusError::ParentBlockNumberMismatch {
                        parent_block_number: canonical_parent_number,
                        block_number: block.number,
                    },
                    block.block,
                ))
            }
        }

        // if there is a parent inside the buffer, validate against it.
        // 如果parent在buffer中找打，对它进行验证
        if let Some(buffered_parent) = self.buffered_blocks.block(parent) {
            self.externals
                .consensus
                .validate_header_against_parent(&block, buffered_parent)
                .map_err(|err| InsertBlockError::consensus_error(err, block.block.clone()))?;
        }

        // insert block inside unconnected block buffer. Delaying its execution.
        // 插入block到unconnected block buffer中，延迟执行
        self.buffered_blocks.insert_block(block.clone());

        // find the lowest ancestor of the block in the buffer to return as the missing parent
        // this shouldn't return None because that only happens if the block was evicted, which
        // shouldn't happen right after insertion
        // 在buffer中找到block的最低祖先，作为缺失的parent，这不应该返回None，因为只有在block被驱逐后才会发生，而这不应该发生在插入之后
        let lowest_ancestor =
            self.buffered_blocks.lowest_ancestor(&block.hash).ok_or_else(|| {
                InsertBlockError::tree_error(
                    BlockchainTreeError::BlockBufferingFailed { block_hash: block.hash },
                    block.block,
                )
            })?;

        Ok(BlockStatus::Disconnected { missing_parent: lowest_ancestor.parent_num_hash() })
    }

    /// This tries to append the given block to the canonical chain.
    /// 试着扩展给定的block到canonical chain中
    ///
    /// WARNING: this expects that the block is part of the canonical chain: The block's parent is
    /// part of the canonical chain (e.g. the block's parent is the latest canonical hash). See also
    /// [Self::is_block_hash_canonical].
    /// WARNING: 这期望block是canonical chain的一部分：block的parent是canonical chain的一部分（例如，block的parent是最新的canonical hash）
    #[instrument(skip_all, target = "blockchain_tree")]
    fn try_append_canonical_chain(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, InsertBlockError> {
        let parent = block.parent_num_hash();
        let block_num_hash = block.num_hash();
        debug!(target: "blockchain_tree", head = ?block_num_hash.hash, ?parent, "Appending block to canonical chain");
        // create new chain that points to that block
        //return self.fork_canonical_chain(block.clone());
        // TODO save pending block to database
        // https://github.com/paradigmxyz/reth/issues/1713

        let db = self.externals.database();
        let provider =
            db.provider().map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?;

        // Validate that the block is post merge
        // 校验block是post merge
        let parent_td = provider
            .header_td(&block.parent_hash)
            .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?
            .ok_or_else(|| {
                InsertBlockError::tree_error(
                    BlockchainTreeError::CanonicalChain { block_hash: block.parent_hash },
                    block.block.clone(),
                )
            })?;

        // Pass the parent total difficulty to short-circuit unnecessary calculations.
        // 传入parent total difficulty，以避免不必要的计算
        if !self.externals.chain_spec.fork(Hardfork::Paris).active_at_ttd(parent_td, U256::ZERO) {
            return Err(InsertBlockError::execution_error(
                BlockExecutionError::BlockPreMerge { hash: block.hash },
                block.block,
            ))
        }

        let parent_header = provider
            .header(&block.parent_hash)
            .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?
            .ok_or_else(|| {
                InsertBlockError::tree_error(
                    BlockchainTreeError::CanonicalChain { block_hash: block.parent_hash },
                    block.block.clone(),
                )
            })?
            .seal(block.parent_hash);

        let canonical_chain = self.canonical_chain();

        let (block_status, chain) = if block.parent_hash == canonical_chain.tip().hash {
            // 如果parent是canonical chain的tip，那么block是canonical chain的一部分
            let chain = AppendableChain::new_canonical_head_fork(
                block,
                &parent_header,
                canonical_chain.inner(),
                parent,
                &self.externals,
            )?;
            (BlockStatus::Valid, chain)
        } else {
            // 否则，构建一个新的canonical fork
            let chain = AppendableChain::new_canonical_fork(
                block,
                &parent_header,
                canonical_chain.inner(),
                parent,
                &self.externals,
            )?;
            (BlockStatus::Accepted, chain)
        };

        // let go of `db` immutable borrow
        drop(provider);

        // 插入chain
        self.insert_chain(chain);
        // 试着连接缓存的blocks
        self.try_connect_buffered_blocks(block_num_hash);
        Ok(block_status)
    }

    /// Try inserting a block into the given side chain.
    /// 试着插入一个block到给定的side chain中
    ///
    /// WARNING: This expects a valid side chain id, see [BlockIndices::get_blocks_chain_id]
    /// WARNING: 这期望一个合法的side chain id，参见[BlockIndices::get_blocks_chain_id]
    #[instrument(skip_all, target = "blockchain_tree")]
    fn try_insert_block_into_side_chain(
        &mut self,
        block: SealedBlockWithSenders,
        chain_id: BlockChainId,
    ) -> Result<BlockStatus, InsertBlockError> {
        debug!(target: "blockchain_tree", "Inserting block into side chain");
        let block_num_hash = block.num_hash();
        // Create a new sidechain by forking the given chain, or append the block if the parent
        // block is the top of the given chain.
        // 通过fork给定的chain创建一个新的sidechain，或者如果parent block是给定chain的顶部，则附加block
        let block_hashes = self.all_chain_hashes(chain_id);

        // get canonical fork.
        // 获取canonical fork
        let canonical_fork = match self.canonical_fork(chain_id) {
            None => {
                return Err(InsertBlockError::tree_error(
                    BlockchainTreeError::BlockSideChainIdConsistency { chain_id },
                    block.block,
                ))
            }
            Some(fork) => fork,
        };

        // get chain that block needs to join to.
        let parent_chain = match self.chains.get_mut(&chain_id) {
            Some(parent_chain) => parent_chain,
            None => {
                return Err(InsertBlockError::tree_error(
                    BlockchainTreeError::BlockSideChainIdConsistency { chain_id },
                    block.block,
                ))
            }
        };

        let chain_tip = parent_chain.tip().hash();
        let canonical_chain = self.block_indices.canonical_chain();

        // append the block if it is continuing the side chain.
        let status = if chain_tip == block.parent_hash {
            // check if the chain extends the currently tracked canonical head
            let block_kind = if canonical_fork.hash == canonical_chain.tip().hash {
                BlockKind::ExtendsCanonicalHead
            } else {
                BlockKind::ForksHistoricalBlock
            };

            debug!(target: "blockchain_tree", "Appending block to side chain");
            let block_hash = block.hash();
            let block_number = block.number;
            parent_chain.append_block(
                block,
                block_hashes,
                canonical_chain.inner(),
                &self.externals,
                canonical_fork,
                block_kind,
            )?;

            self.block_indices.insert_non_fork_block(block_number, block_hash, chain_id);

            if block_kind.extends_canonical_head() {
                // if the block can be traced back to the canonical head, we were able to fully
                // validate it
                Ok(BlockStatus::Valid)
            } else {
                Ok(BlockStatus::Accepted)
            }
        } else {
            debug!(target: "blockchain_tree", ?canonical_fork, "Starting new fork from side chain");
            // the block starts a new fork
            let chain = parent_chain.new_chain_fork(
                block,
                block_hashes,
                canonical_chain.inner(),
                canonical_fork,
                &self.externals,
            )?;
            self.insert_chain(chain);
            Ok(BlockStatus::Accepted)
        };

        // After we inserted the block, we try to connect any buffered blocks
        // 在我们插入block之后，我们试着连接任何buffered blocks
        self.try_connect_buffered_blocks(block_num_hash);

        status
    }

    /// Get all block hashes from a sidechain that are not part of the canonical chain.
    /// 从一个sidechain中获取所有block hashes，这些block hashes不是canonical chain的一部分
    ///
    /// This is a one time operation per block.
    /// 对于每个block这是一次性操作
    ///
    /// # Note
    ///
    /// This is not cached in order to save memory.
    /// 这没有缓存，为了节省内存
    fn all_chain_hashes(&self, chain_id: BlockChainId) -> BTreeMap<BlockNumber, BlockHash> {
        // find chain and iterate over it,
        let mut chain_id = chain_id;
        let mut hashes = BTreeMap::new();
        loop {
            let Some(chain) = self.chains.get(&chain_id) else { return hashes };
            hashes.extend(chain.blocks().values().map(|b| (b.number, b.hash())));

            let fork_block = chain.fork_block_hash();
            if let Some(next_chain_id) = self.block_indices.get_blocks_chain_id(&fork_block) {
                chain_id = next_chain_id;
            } else {
                // if there is no fork block that point to other chains, break the loop.
                // it means that this fork joins to canonical block.
                break
            }
        }
        hashes
    }

    /// Get the block at which the given chain forked from the current canonical chain.
    ///
    /// This is used to figure out what kind of state provider the executor should use to execute
    /// the block.
    ///
    /// Returns `None` if the chain is not known.
    fn canonical_fork(&self, chain_id: BlockChainId) -> Option<ForkBlock> {
        let mut chain_id = chain_id;
        let mut fork;
        loop {
            // chain fork block
            fork = self.chains.get(&chain_id)?.fork_block();
            // get fork block chain
            if let Some(fork_chain_id) = self.block_indices.get_blocks_chain_id(&fork.hash) {
                chain_id = fork_chain_id;
                continue
            }
            break
        }
        (self.block_indices.canonical_hash(&fork.number) == Some(fork.hash)).then_some(fork)
    }

    /// Insert a chain into the tree.
    /// 插入一个chain到tree中
    ///
    /// Inserts a chain into the tree and builds the block indices.
    /// 插入一个chain到tree并且构建block indices
    fn insert_chain(&mut self, chain: AppendableChain) -> Option<BlockChainId> {
        if chain.is_empty() {
            return None
        }
        // chain id就是直接从block chain id gerator中获取，之后再加1
        let chain_id = self.block_chain_id_generator;
        self.block_chain_id_generator += 1;

        // 在block indices插入chain
        self.block_indices.insert_chain(chain_id, &chain);
        // add chain_id -> chain index
        // 添加chain_id到chain index
        self.chains.insert(chain_id, chain);
        Some(chain_id)
    }

    /// Checks the block buffer for the given block.
    pub fn get_buffered_block(&mut self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        self.buffered_blocks.block_by_hash(hash)
    }

    /// Gets the lowest ancestor for the given block in the block buffer.
    pub fn lowest_buffered_ancestor(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        self.buffered_blocks.lowest_ancestor(hash)
    }

    /// Insert a new block in the tree.
    ///
    /// # Note
    ///
    /// This recovers transaction signers (unlike [`BlockchainTree::insert_block`]).
    pub fn insert_block_without_senders(
        &mut self,
        block: SealedBlock,
    ) -> Result<BlockStatus, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    /// Insert block for future execution.
    ///
    /// Returns an error if the block is invalid.
    pub fn buffer_block(&mut self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        // validate block consensus rules
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block.block))
        }

        self.buffered_blocks.insert_block(block);
        Ok(())
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    /// 校验block是否正确，并且满足所有关于header和block body的共识规则
    fn validate_block(&self, block: &SealedBlockWithSenders) -> Result<(), ConsensusError> {
        if let Err(e) =
            self.externals.consensus.validate_header_with_total_difficulty(block, U256::MAX)
        {
            info!(
                "Failed to validate header for TD related check with error: {e:?}, block:{:?}",
                block
            );
            return Err(e)
        }

        // 校验header
        if let Err(e) = self.externals.consensus.validate_header(block) {
            info!("Failed to validate header with error: {e:?}, block:{:?}", block);
            return Err(e)
        }

        // 校验block
        if let Err(e) = self.externals.consensus.validate_block(block) {
            info!("Failed to validate blocks with error: {e:?}, block:{:?}", block);
            return Err(e)
        }

        Ok(())
    }

    /// Check if block is found inside chain and if the chain extends the canonical chain.
    /// 检查是否block在chain中找到并且如果chain扩展了canonical chain
    ///
    /// if it does extend the canonical chain, return `BlockStatus::Valid`
    /// 如果它扩展了canonical chain，返回`BlockStatus::Valid`
    /// if it does not extend the canonical chain, return `BlockStatus::Accepted`
    /// 如果它没有扩展canonical chain，返回`BlockStatus::Accepted`
    #[track_caller]
    fn is_block_inside_chain(&self, block: &BlockNumHash) -> Option<BlockStatus> {
        // check if block known and is already in the tree
        // 检查block是否已知并且已经在tree中
        if let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block.hash) {
            // find the canonical fork of this chain
            // 找到这个chain的canonical fork
            let canonical_fork = self.canonical_fork(chain_id).expect("Chain id is valid");
            // if the block's chain extends canonical chain
            // 如果block的chain扩展了canonical chain
            return if canonical_fork == self.block_indices.canonical_tip() {
                // 返回Valid
                Some(BlockStatus::Valid)
            } else {
                Some(BlockStatus::Accepted)
            }
        }
        None
    }

    /// Insert a block (with senders recovered) in the tree.
    /// 插入一个block到tree中
    ///
    /// Returns the [BlockStatus] on success:
    /// 返回BlockStatus
    ///
    /// - The block is already part of a sidechain in the tree, or
    /// - block已经是tree中的sidechain的一部分，或者
    /// - The block is already part of the canonical chain, or
    /// - block已经是canonical chain的一部分，或者
    /// - The parent is part of a sidechain in the tree, and we can fork at this block, or
    /// - parent已经是tree中的sidechain的一部分，我们可以在这个block上fork，或者
    /// - The parent is part of the canonical chain, and we can fork at this block
    /// - parent已经是canonical chain的一部分，我们可以在这个block上fork
    ///
    /// Otherwise, and error is returned, indicating that neither the block nor its parent is part
    /// of the chain or any sidechains.
    /// 否则，返回一个错误，表示block或者它的parent都不是chain或者任何sidechain的一部分
    ///
    /// This means that if the block becomes canonical, we need to fetch the missing blocks over
    /// P2P.
    /// 这意味着如果block变成canonical，我们需要通过P2P获取丢失的blocks
    ///
    /// # Note
    ///
    /// If the senders have not already been recovered, call
    /// [`BlockchainTree::insert_block_without_senders`] instead.
    pub fn insert_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<BlockStatus, InsertBlockError> {
        // check if we already have this block
        // 检查我们是否已经有这个block
        match self.is_block_known(block.num_hash()) {
            Ok(Some(status)) => return Ok(status),
            Err(err) => return Err(InsertBlockError::new(block.block, err)),
            _ => {}
        }

        // validate block consensus rules
        // 校验block的共识规则
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block.block))
        }

        self.try_insert_validated_block(block)
    }

    /// Finalize blocks up until and including `finalized_block`, and remove them from the tree.
    /// Finalize blocks直到包括finalized_block，并且从tree中移除它们
    pub fn finalize_block(&mut self, finalized_block: BlockNumber) {
        // remove blocks
        // 移除blocks
        let mut remove_chains = self.block_indices.finalize_canonical_blocks(
            finalized_block,
            self.config.num_of_additional_canonical_block_hashes(),
        );
        // remove chains of removed blocks
        // 移除chains of removed blocks
        while let Some(chain_id) = remove_chains.pop_first() {
            if let Some(chain) = self.chains.remove(&chain_id) {
                remove_chains.extend(self.block_indices.remove_chain(&chain));
            }
        }
        // clean block buffer.
        // 清理block buffer
        self.buffered_blocks.clean_old_blocks(finalized_block);
    }

    /// Reads the last `N` canonical hashes from the database and updates the block indices of the
    /// tree.
    ///
    /// `N` is the `max_reorg_depth` plus the number of block hashes needed to satisfy the
    /// `BLOCKHASH` opcode in the EVM.
    ///
    /// # Note
    ///
    /// This finalizes `last_finalized_block` prior to reading the canonical hashes (using
    /// [`BlockchainTree::finalize_block`]).
    pub fn restore_canonical_hashes(
        &mut self,
        last_finalized_block: BlockNumber,
    ) -> Result<(), Error> {
        self.finalize_block(last_finalized_block);

        let num_of_canonical_hashes =
            self.config.max_reorg_depth() + self.config.num_of_additional_canonical_block_hashes();

        let last_canonical_hashes = self
            .externals
            .db
            .tx()?
            .cursor_read::<tables::CanonicalHeaders>()?
            .walk_back(None)?
            .take(num_of_canonical_hashes as usize)
            .collect::<Result<BTreeMap<BlockNumber, BlockHash>, _>>()?;

        let (mut remove_chains, _) =
            self.block_indices.update_block_hashes(last_canonical_hashes.clone());

        // remove all chains that got discarded
        while let Some(chain_id) = remove_chains.first() {
            if let Some(chain) = self.chains.remove(chain_id) {
                remove_chains.extend(self.block_indices.remove_chain(&chain));
            }
        }

        // check unconnected block buffer for the childs of new added blocks,
        for added_block in last_canonical_hashes.into_iter() {
            self.try_connect_buffered_blocks(added_block.into())
        }

        // check unconnected block buffer for childs of the chains.
        let mut all_chain_blocks = Vec::new();
        for (_, chain) in self.chains.iter() {
            for (&number, blocks) in chain.blocks.iter() {
                all_chain_blocks.push(BlockNumHash { number, hash: blocks.hash })
            }
        }
        for block in all_chain_blocks.into_iter() {
            self.try_connect_buffered_blocks(block)
        }

        Ok(())
    }

    /// Connect unconnected (buffered) blocks if the new block closes a gap.
    /// 连接所有未连接的blocks，如果新的block关闭了一个gap
    ///
    /// This will try to insert all children of the new block, extending the chain.
    /// 这会试着插入所有新block的children，扩展chain
    ///
    /// If all children are valid, then this essentially moves appends all children blocks to the
    /// new block's chain.
    /// 如果所有的children都是有效的，那么这本质上就是将所有的children block附加到新block的chain上
    fn try_connect_buffered_blocks(&mut self, new_block: BlockNumHash) {
        trace!(target: "blockchain_tree", ?new_block, "try_connect_buffered_blocks");

        let include_blocks = self.buffered_blocks.remove_with_children(new_block);
        // insert block children
        // 插入block的children
        for block in include_blocks.into_iter() {
            // dont fail on error, just ignore the block.
            // 不要在遇到error的时候失败，只是忽略这个block
            let _ = self.try_insert_validated_block(block).map_err(|err| {
                debug!(
                    target: "blockchain_tree", ?err,
                    "Failed to insert buffered block",
                );
                err
            });
        }
    }

    /// Split a sidechain at the given point, and return the canonical part of it.
    /// 在给定的点分割一个sidechain，并且返回它的canonical部分
    ///
    /// The pending part of the chain is reinserted into the tree with the same `chain_id`.
    /// chain的pending部分被重新插入到tree中，使用相同的`chain_id`
    fn split_chain(
        &mut self,
        chain_id: BlockChainId,
        chain: AppendableChain,
        split_at: SplitAt,
    ) -> Chain {
        let chain = chain.into_inner();
        match chain.split(split_at) {
            ChainSplit::Split { canonical, pending } => {
                // rest of split chain is inserted back with same chain_id.
                self.block_indices.insert_chain(chain_id, &pending);
                self.chains.insert(chain_id, AppendableChain::new(pending));
                canonical
            }
            ChainSplit::NoSplitCanonical(canonical) => canonical,
            ChainSplit::NoSplitPending(_) => {
                unreachable!("Should not happen as block indices guarantee structure of blocks")
            }
        }
    }

    /// Attempts to find the header for the given block hash if it is canonical.
    /// 试着找到给定block hash的header，如果它是canonical的
    ///
    /// Returns `Ok(None)` if the block hash is not canonical (block hash does not exist, or is
    /// included in a sidechain).
    /// 返回Ok(None)，如果block hash不是canonical的（block hash不存在，或者包含在sidechain中）
    pub fn find_canonical_header(&self, hash: &BlockHash) -> Result<Option<SealedHeader>, Error> {
        // if the indices show that the block hash is not canonical, it's either in a sidechain or
        // canonical, but in the db. If it is in a sidechain, it is not canonical. If it is not in
        // the db, then it is not canonical.
        // 如果indices显示block hash不是canonical的，它要么在sidechain中，要么在db中，
        // 如果它在sidechain中，它不是canonical的，如果它不在db中，那么它不是canonical的

        let mut header = None;
        if let Some(num) = self.block_indices.get_canonical_block_number(hash) {
            header = self.externals.database().provider()?.header_by_number(num)?;
        }

        if header.is_none() && self.is_block_hash_inside_chain(*hash) {
            return Ok(None)
        }

        if header.is_none() {
            header = self.externals.database().provider()?.header(hash)?
        }

        Ok(header.map(|header| header.seal(*hash)))
    }

    /// Determines whether or not a block is canonical, checking the db if necessary.
    /// 确定block是否是canonical的，如果有必要的话，检查db
    pub fn is_block_hash_canonical(&self, hash: &BlockHash) -> Result<bool, Error> {
        self.find_canonical_header(hash).map(|header| header.is_some())
    }

    /// Make a block and its parent(s) part of the canonical chain and commit them to the database
    /// 让一个block和它的parent成为canonical chain的一部分，并且将它们提交到数据库中
    ///
    /// # Note
    ///
    /// This unwinds the database if necessary, i.e. if parts of the canonical chain have been
    /// re-orged.
    /// 这会回滚数据库，如果有必要的话，例如，如果canonical chain的一部分已经被re-orged
    ///
    /// # Returns
    ///
    /// Returns `Ok` if the blocks were canonicalized, or if the blocks were already canonical.
    /// 返回Ok，如果blocks已经是canonical的，或者如果blocks已经是canonical的
    #[track_caller]
    #[instrument(skip(self), target = "blockchain_tree")]
    pub fn make_canonical(&mut self, block_hash: &BlockHash) -> Result<CanonicalOutcome, Error> {
        let old_block_indices = self.block_indices.clone();
        let old_buffered_blocks = self.buffered_blocks.parent_to_child.clone();

        // If block is already canonical don't return error.
        // 如果block已经是canonical的，不要返回error
        if let Some(header) = self.find_canonical_header(block_hash)? {
            info!(target: "blockchain_tree", ?block_hash, "Block is already canonical");
            let td = self
                .externals
                .database()
                .provider()?
                .header_td(block_hash)?
                .ok_or(BlockExecutionError::MissingTotalDifficulty { hash: *block_hash })?;
            if !self.externals.chain_spec.fork(Hardfork::Paris).active_at_ttd(td, U256::ZERO) {
                return Err(BlockExecutionError::BlockPreMerge { hash: *block_hash }.into())
            }
            return Ok(CanonicalOutcome::AlreadyCanonical { header })
        }

        let Some(chain_id) = self.block_indices.get_blocks_chain_id(block_hash) else {
            info!(target: "blockchain_tree", ?block_hash,  "Block hash not found in block indices");
            // TODO: better error
            return Err(BlockExecutionError::BlockHashNotFoundInChain { block_hash: *block_hash }.into())
        };
        let chain = self.chains.remove(&chain_id).expect("To be present");

        // we are splitting chain at the block hash that we want to make canonical
        // 我们正在split chain，block hash是我们想要make canonical的
        let canonical = self.split_chain(chain_id, chain, SplitAt::Hash(*block_hash));

        let mut block_fork = canonical.fork_block();
        let mut block_fork_number = canonical.fork_block_number();
        let mut chains_to_promote = vec![canonical];

        // loop while fork blocks are found in Tree.
        // 循环，当在Tree中找到fork blocks时
        while let Some(chain_id) = self.block_indices.get_blocks_chain_id(&block_fork.hash) {
            let chain = self.chains.remove(&chain_id).expect("To fork to be present");
            block_fork = chain.fork_block();
            let canonical = self.split_chain(chain_id, chain, SplitAt::Number(block_fork_number));
            block_fork_number = canonical.fork_block_number();
            chains_to_promote.push(canonical);
        }

        let old_tip = self.block_indices.canonical_tip();
        // Merge all chain into one chain.
        // 将所有的chain合并成一个chain
        let mut new_canon_chain = chains_to_promote.pop().expect("There is at least one block");
        for chain in chains_to_promote.into_iter().rev() {
            new_canon_chain.append_chain(chain).expect("We have just build the chain.");
        }

        // update canonical index
        // 更新canonical index
        self.block_indices.canonicalize_blocks(new_canon_chain.blocks());

        // event about new canonical chain.
        // 关于新的canonical chain的事件
        let chain_notification;
        info!(
            target: "blockchain_tree",
            "Committing new canonical chain: {:?}",
            new_canon_chain.blocks().iter().map(|(_, b)| b.num_hash()).collect::<Vec<_>>()
        );
        // if joins to the tip;
        // 如果加入到tip
        if new_canon_chain.fork_block_hash() == old_tip.hash {
            chain_notification =
                CanonStateNotification::Commit { new: Arc::new(new_canon_chain.clone()) };
            // append to database
            self.commit_canonical(new_canon_chain)?;
        } else {
            // it forks to canonical block that is not the tip.

            let canon_fork: BlockNumHash = new_canon_chain.fork_block();
            // sanity check
            if self.block_indices.canonical_hash(&canon_fork.number) != Some(canon_fork.hash) {
                unreachable!("all chains should point to canonical chain.");
            }

            let old_canon_chain = self.revert_canonical(canon_fork.number);

            let old_canon_chain = match old_canon_chain {
                val @ Err(_) => {
                    error!(
                        target: "blockchain_tree",
                        "Reverting canonical chain failed with error: {:?}\n\
                            Old BlockIndices are:{:?}\n\
                            New BlockIndices are: {:?}\n\
                            Old BufferedBlocks are:{:?}",
                        val, old_block_indices, self.block_indices, old_buffered_blocks
                    );
                    val?
                }
                Ok(val) => val,
            };

            // commit new canonical chain.
            // 提交新的canonical chain
            self.commit_canonical(new_canon_chain.clone())?;

            if let Some(old_canon_chain) = old_canon_chain {
                // state action
                chain_notification = CanonStateNotification::Reorg {
                    old: Arc::new(old_canon_chain.clone()),
                    new: Arc::new(new_canon_chain.clone()),
                };
                // insert old canon chain
                self.insert_chain(AppendableChain::new(old_canon_chain));
            } else {
                // error here to confirm that we are reverting nothing from db.
                error!(target: "blockchain_tree", "Reverting nothing from db on block: #{:?}", block_hash);

                chain_notification =
                    CanonStateNotification::Commit { new: Arc::new(new_canon_chain) };
            }
        }

        //
        let head = chain_notification.tip().header.clone();

        // send notification about new canonical chain.
        // 发送关于新的canonical chain的通知
        let _ = self.canon_state_notification_sender.send(chain_notification);

        Ok(CanonicalOutcome::Committed { head })
    }

    /// Subscribe to new blocks events.
    /// 订阅新的block events
    ///
    /// Note: Only canonical blocks are send.
    /// 注意：只有canonical blocks被发送
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications {
        self.canon_state_notification_sender.subscribe()
    }

    /// Canonicalize the given chain and commit it to the database.
    /// 将给定的chain进行canonical化，并且提交到数据库中
    fn commit_canonical(&mut self, chain: Chain) -> Result<(), Error> {
        let mut tx = Transaction::new(&self.externals.db)?;

        let (blocks, state) = chain.into_inner();

        tx.append_blocks_with_post_state(blocks.into_blocks().collect(), state)
            .map_err(|e| BlockExecutionError::CanonicalCommit { inner: e.to_string() })?;

        tx.commit()?;

        Ok(())
    }

    /// Unwind tables and put it inside state
    /// 回退tables并且输入到state中
    pub fn unwind(&mut self, unwind_to: BlockNumber) -> Result<(), Error> {
        // nothing to be done if unwind_to is higher then the tip
        // 什么都不做，如果unwind_to比tip高
        if self.block_indices.canonical_tip().number <= unwind_to {
            return Ok(())
        }
        // revert `N` blocks from current canonical chain and put them inside BlockchanTree
        // 从当前的canonical chain中回退N个blocks，并且将它们放到BlockchanTree中
        let old_canon_chain = self.revert_canonical(unwind_to)?;

        // check if there is block in chain
        // 检查是否有block在chain中
        if let Some(old_canon_chain) = old_canon_chain {
            self.block_indices.unwind_canonical_chain(unwind_to);
            // insert old canonical chain to BlockchainTree.
            // 将old canonical chain插入到BlockchainTree中
            self.insert_chain(AppendableChain::new(old_canon_chain));
        }

        Ok(())
    }

    /// Revert canonical blocks from the database and return them.
    /// 从数据库回退canonical blocks，并且返回它们
    ///
    /// The block, `revert_until`, is non-inclusive, i.e. `revert_until` stays in the database.
    /// block，revert_until，是不包含的，例如，revert_until保留在数据库中
    fn revert_canonical(&mut self, revert_until: BlockNumber) -> Result<Option<Chain>, Error> {
        // read data that is needed for new sidechain

        let mut tx = Transaction::new(&self.externals.db)?;

        let tip = tx.tip_number()?;
        let revert_range = (revert_until + 1)..=tip;
        info!(target: "blockchain_tree", "Revert canonical chain range: {:?}", revert_range);
        // read block and execution result from database. and remove traces of block from tables.
        let blocks_and_execution = tx
            .take_block_and_execution_range(self.externals.chain_spec.as_ref(), revert_range)
            .map_err(|e| BlockExecutionError::CanonicalRevert { inner: e.to_string() })?;

        tx.commit()?;

        if blocks_and_execution.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Chain::new(blocks_and_execution)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_buffer::BufferedBlocks;
    use assert_matches::assert_matches;
    use linked_hash_set::LinkedHashSet;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, Env, WriteMap},
        transaction::DbTxMut,
    };
    use reth_interfaces::test_utils::TestConsensus;
    use reth_primitives::{
        proofs::EMPTY_ROOT, stage::StageCheckpoint, ChainSpecBuilder, H256, MAINNET,
    };
    use reth_provider::{
        insert_block,
        post_state::PostState,
        test_utils::{blocks::BlockChainTestData, TestExecutorFactory},
    };
    use std::{collections::HashSet, sync::Arc};
    use tracing_test::traced_test;

    fn setup_externals(
        exec_res: Vec<PostState>,
    ) -> TreeExternals<Arc<Env<WriteMap>>, Arc<TestConsensus>, TestExecutorFactory> {
        // 创建一个db
        let db = create_test_rw_db();
        // 创建consensus 和 chain_spec
        let consensus = Arc::new(TestConsensus::default());
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );
        let executor_factory = TestExecutorFactory::new(chain_spec.clone());
        executor_factory.extend(exec_res);

        TreeExternals::new(db, consensus, executor_factory, chain_spec)
    }

    fn setup_genesis<DB: Database>(db: DB, mut genesis: SealedBlock) {
        // insert genesis to db.

        genesis.header.header.number = 10;
        genesis.header.header.state_root = EMPTY_ROOT;
        let tx_mut = db.tx_mut().unwrap();

        insert_block(&tx_mut, genesis, None).unwrap();

        // insert first 10 blocks
        for i in 0..10 {
            tx_mut.put::<tables::CanonicalHeaders>(i, H256([100 + i as u8; 32])).unwrap();
        }
        tx_mut.put::<tables::SyncStage>("Finish".to_string(), StageCheckpoint::new(10)).unwrap();
        tx_mut.commit().unwrap();
    }

    /// Test data structure that will check tree internals
    #[derive(Default, Debug)]
    struct TreeTester {
        /// Number of chains
        chain_num: Option<usize>,
        /// Check block to chain index
        block_to_chain: Option<HashMap<BlockHash, BlockChainId>>,
        /// Check fork to child index
        fork_to_child: Option<HashMap<BlockHash, HashSet<BlockHash>>>,
        /// Pending blocks
        pending_blocks: Option<(BlockNumber, HashSet<BlockHash>)>,
        /// Buffered blocks
        buffered_blocks: Option<BufferedBlocks>,
    }

    impl TreeTester {
        fn with_chain_num(mut self, chain_num: usize) -> Self {
            self.chain_num = Some(chain_num);
            self
        }
        fn with_block_to_chain(mut self, block_to_chain: HashMap<BlockHash, BlockChainId>) -> Self {
            self.block_to_chain = Some(block_to_chain);
            self
        }
        fn with_fork_to_child(
            mut self,
            fork_to_child: HashMap<BlockHash, HashSet<BlockHash>>,
        ) -> Self {
            self.fork_to_child = Some(fork_to_child);
            self
        }

        fn with_buffered_blocks(mut self, buffered_blocks: BufferedBlocks) -> Self {
            self.buffered_blocks = Some(buffered_blocks);
            self
        }

        fn with_pending_blocks(
            mut self,
            pending_blocks: (BlockNumber, HashSet<BlockHash>),
        ) -> Self {
            self.pending_blocks = Some(pending_blocks);
            self
        }

        fn assert<DB: Database, C: Consensus, EF: ExecutorFactory>(
            self,
            tree: &BlockchainTree<DB, C, EF>,
        ) {
            if let Some(chain_num) = self.chain_num {
                assert_eq!(tree.chains.len(), chain_num);
            }
            if let Some(block_to_chain) = self.block_to_chain {
                assert_eq!(*tree.block_indices.blocks_to_chain(), block_to_chain);
            }
            if let Some(fork_to_child) = self.fork_to_child {
                let mut x: HashMap<BlockHash, LinkedHashSet<BlockHash>> = HashMap::new();
                for (key, hash_set) in fork_to_child.into_iter() {
                    x.insert(key, hash_set.into_iter().collect());
                }
                assert_eq!(*tree.block_indices.fork_to_child(), x);
            }
            if let Some(pending_blocks) = self.pending_blocks {
                let (num, hashes) = tree.block_indices.pending_blocks();
                let hashes = hashes.into_iter().collect::<HashSet<_>>();
                assert_eq!((num, hashes), pending_blocks);
            }
            if let Some(buffered_blocks) = self.buffered_blocks {
                assert_eq!(*tree.buffered_blocks.blocks(), buffered_blocks);
            }
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn sanity_path() {
        let data = BlockChainTestData::default_with_numbers(11, 12);
        let (block1, exec1) = data.blocks[0].clone();
        let (block2, exec2) = data.blocks[1].clone();
        let genesis = data.genesis;

        // test pops execution results from vector, so order is from last to first.
        // 测试从vector中弹出execution results，所以顺序是从后往前的
        let externals = setup_externals(vec![exec2.clone(), exec1.clone(), exec2, exec1]);

        // last finalized block would be number 9.
        // 最后的finalized block应该是number 9
        setup_genesis(externals.db.clone(), genesis);

        // make tree
        // 构建tree
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let (sender, mut canon_notif) = tokio::sync::broadcast::channel(10);
        // 构建block chain tree
        let mut tree =
            BlockchainTree::new(externals, sender, config).expect("failed to create tree");

        // genesis block 10 is already canonical
        // genesis block 10已经是canonical的了
        assert!(tree.make_canonical(&H256::zero()).is_ok());

        // make sure is_block_hash_canonical returns true for genesis block
        // 对于genesis block，确保is_block_hash_canonical返回true
        assert!(tree.is_block_hash_canonical(&H256::zero()).unwrap());

        // make genesis block 10 as finalized
        // 确保genesis block 10是finalized的
        tree.finalize_block(10);

        // block 2 parent is not known, block2 is buffered.
        // block2的parent是未知的，block2是buffered的
        assert_eq!(
            // 插入block2
            tree.insert_block(block2.clone()).unwrap(),
            BlockStatus::Disconnected { missing_parent: block2.parent_num_hash() }
        );

        // Buffered block: [block2]
        // Trie state:
        // |
        // g1 (canonical blocks)
        // |

        TreeTester::default()
            .with_buffered_blocks(BTreeMap::from([(
                block2.number,
                HashMap::from([(block2.hash(), block2.clone())]),
            )]))
            .assert(&tree);

        assert_eq!(
            tree.is_block_known(block2.num_hash()).unwrap(),
            Some(BlockStatus::Disconnected { missing_parent: block2.parent_num_hash() })
        );

        // check if random block is known
        let old_block = BlockNumHash::new(1, H256([32; 32]));
        let err = BlockchainTreeError::PendingBlockIsFinalized { last_finalized: 10 };

        assert_eq!(tree.is_block_known(old_block).unwrap_err().as_tree_error(), Some(err));

        // insert block1 and buffered block2 is inserted
        // 插入block1，buffered block2被插入
        assert_eq!(tree.insert_block(block1.clone()).unwrap(), BlockStatus::Valid);

        // Buffered blocks: []
        // Trie state:
        //      b2 (pending block)
        //      |
        //      |
        //      b1 (pending block)
        //    /
        //  /
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block1.hash, 0), (block2.hash, 0)]))
            .with_fork_to_child(HashMap::from([(block1.parent_hash, HashSet::from([block1.hash]))]))
            .with_pending_blocks((block1.number, HashSet::from([block1.hash])))
            .assert(&tree);

        // already inserted block will return true.
        // 已经插入的block会返回true
        assert_eq!(tree.insert_block(block1.clone()).unwrap(), BlockStatus::Valid);

        // block two is already inserted.
        // block2已经被插入了
        assert_eq!(tree.insert_block(block2.clone()).unwrap(), BlockStatus::Valid);

        // make block1 canonical
        // 将block1变成canonical的
        assert!(tree.make_canonical(&block1.hash()).is_ok());
        // check notification
        // 检查notification
        assert_matches!(canon_notif.try_recv(), Ok(CanonStateNotification::Commit{ new}) if *new.blocks() == BTreeMap::from([(block1.number,block1.clone())]));

        // make block2 canonicals
        // 让block2变成canonical的
        assert!(tree.make_canonical(&block2.hash()).is_ok());
        // check notification.
        // 检查notification
        assert_matches!(canon_notif.try_recv(), Ok(CanonStateNotification::Commit{ new}) if *new.blocks() == BTreeMap::from([(block2.number,block2.clone())]));

        // Trie state:
        // b2 (canonical block)
        // |
        // |
        // b1 (canonical block)
        // |
        // |
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::from([]))
            .with_fork_to_child(HashMap::from([]))
            .assert(&tree);

        /**** INSERT SIDE BLOCKS *** */
        // 插入side blocks

        let mut block1a = block1.clone();
        let block1a_hash = H256([0x33; 32]);
        block1a.hash = block1a_hash;
        let mut block2a = block2.clone();
        let block2a_hash = H256([0x34; 32]);
        block2a.hash = block2a_hash;

        // reinsert two blocks that point to canonical chain
        // 重新插入两个指向canonical chain的blocks
        assert_eq!(tree.insert_block(block1a.clone()).unwrap(), BlockStatus::Accepted);

        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1)]))
            .with_fork_to_child(HashMap::from([(
                block1.parent_hash,
                HashSet::from([block1a_hash]),
            )]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        assert_eq!(tree.insert_block(block2a.clone()).unwrap(), BlockStatus::Accepted);
        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1), (block2a_hash, 2)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // make b2a canonical
        // 将b2a变为canonical的
        assert!(tree.make_canonical(&block2a_hash).is_ok());
        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block2.number,block2.clone())])
                && *new.blocks() == BTreeMap::from([(block2a.number,block2a.clone())]));

        // Trie state:
        // b2a   b2 (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([(block1a_hash, 1), (block2.hash, 3)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2.hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        assert!(tree.make_canonical(&block1a_hash).is_ok());
        // Trie state:
        //       b2a   b2 (side chain)
        //       |   /
        //       | /
        // b1a  b1 (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([
                (block1.hash, 4),
                (block2a_hash, 4),
                (block2.hash, 3),
            ]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1.hash])),
                (block1.hash(), HashSet::from([block2.hash])),
            ]))
            .with_pending_blocks((block1a.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2a.number,block2a.clone())])
                && *new.blocks() == BTreeMap::from([(block1a.number,block1a.clone())]));

        // check that b2 and b1 are not canonical
        assert!(!tree.is_block_hash_canonical(&block2.hash).unwrap());
        assert!(!tree.is_block_hash_canonical(&block1.hash).unwrap());

        // ensure that b1a is canonical
        assert!(tree.is_block_hash_canonical(&block1a.hash).unwrap());

        // make b2 canonical
        assert!(tree.make_canonical(&block2.hash()).is_ok());

        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1  b1a (side chain)
        // |  /
        // |/
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([(block1a_hash, 5), (block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([
                (block1.parent_hash, HashSet::from([block1a_hash])),
                (block1.hash(), HashSet::from([block2a_hash])),
            ]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Reorg{ old, new})
            if *old.blocks() == BTreeMap::from([(block1a.number,block1a.clone())])
                && *new.blocks() == BTreeMap::from([(block1.number,block1.clone()),(block2.number,block2.clone())]));

        // check that b2 is now canonical
        assert!(tree.is_block_hash_canonical(&block2.hash).unwrap());

        // finalize b1 that would make b1a removed from tree
        tree.finalize_block(11);
        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1 (canon)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .assert(&tree);

        // unwind canonical
        assert_eq!(tree.unwind(block1.number), Ok(()));
        // Trie state:
        //    b2   b2a (pending block)
        //   /    /
        //  /   /
        // /  /
        // b1 (canonical block)
        // |
        // |
        // g1 (canonical blocks)
        // |
        TreeTester::default()
            .with_chain_num(2)
            .with_block_to_chain(HashMap::from([(block2a_hash, 4), (block2.hash, 6)]))
            .with_fork_to_child(HashMap::from([(
                block1.hash(),
                HashSet::from([block2a_hash, block2.hash]),
            )]))
            .with_pending_blocks((block2.number, HashSet::from([block2.hash, block2a.hash])))
            .assert(&tree);

        // commit b2a
        assert!(tree.make_canonical(&block2.hash).is_ok());

        // Trie state:
        // b2   b2a (side chain)
        // |   /
        // | /
        // b1 (finalized)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(1)
            .with_block_to_chain(HashMap::from([(block2a_hash, 4)]))
            .with_fork_to_child(HashMap::from([(block1.hash(), HashSet::from([block2a_hash]))]))
            .with_pending_blocks((block2.number + 1, HashSet::new()))
            .assert(&tree);

        // check notification.
        assert_matches!(canon_notif.try_recv(),
            Ok(CanonStateNotification::Commit{ new})
            if *new.blocks() == BTreeMap::from([(block2.number,block2.clone())]));

        // insert unconnected block2b
        let mut block2b = block2a.clone();
        block2b.hash = H256([0x99; 32]);
        block2b.parent_hash = H256([0x88; 32]);

        assert_eq!(
            tree.insert_block(block2b.clone()).unwrap(),
            BlockStatus::Disconnected { missing_parent: block2b.parent_num_hash() }
        );

        TreeTester::default()
            .with_buffered_blocks(BTreeMap::from([(
                block2b.number,
                HashMap::from([(block2b.hash(), block2b.clone())]),
            )]))
            .assert(&tree);

        // update canonical block to b2, this would make b2a be removed
        assert_eq!(tree.restore_canonical_hashes(12), Ok(()));

        assert_eq!(tree.is_block_known(block2.num_hash()).unwrap(), Some(BlockStatus::Valid));

        // Trie state:
        // b2 (finalized)
        // |
        // b1 (finalized)
        // |
        // g1 (10)
        // |
        TreeTester::default()
            .with_chain_num(0)
            .with_block_to_chain(HashMap::from([]))
            .with_fork_to_child(HashMap::from([]))
            .with_pending_blocks((block2.number + 1, HashSet::from([])))
            .with_buffered_blocks(BTreeMap::from([]))
            .assert(&tree);
    }
}
