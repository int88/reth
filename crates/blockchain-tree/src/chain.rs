//! A chain in a [`BlockchainTree`][super::BlockchainTree].
//!
//! A [`Chain`] contains the state of accounts for the chain after execution of its constituent
//! blocks, as well as a list of the blocks the chain is composed of.
use super::externals::TreeExternals;
use crate::BundleStateDataRef;
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::error::{BlockchainTreeError, InsertBlockError},
    consensus::{Consensus, ConsensusError},
    RethResult,
};
use reth_primitives::{
    BlockHash, BlockNumber, ForkBlock, SealedBlockWithSenders, SealedHeader, U256,
};
use reth_provider::{
    providers::BundleStateProvider, BundleStateDataProvider, BundleStateWithReceipts, Chain,
    ExecutorFactory, StateRootProvider,
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

/// The ID of a sidechain internally in a [`BlockchainTree`][super::BlockchainTree].
/// 在一个[`BlockchainTree`][super::BlockchainTree]内部的sidechain的ID
pub(crate) type BlockChainId = u64;

/// A chain if the blockchain tree, that has functionality to execute blocks and append them to the
/// it self.
/// 一个chain，如果blockchain tree，有功能执行blocks并且扩展它们到自己
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppendableChain {
    chain: Chain,
}

impl Deref for AppendableChain {
    type Target = Chain;

    fn deref(&self) -> &Self::Target {
        &self.chain
    }
}

impl DerefMut for AppendableChain {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.chain
    }
}

impl AppendableChain {
    /// Create a new appendable chain from a given chain.
    /// 从给定的chain创建一个新的appendable chain
    pub fn new(chain: Chain) -> Self {
        Self { chain }
    }

    /// Get the chain.
    pub fn into_inner(self) -> Chain {
        self.chain
    }

    /// Create a new chain that forks off the canonical.
    /// 创建一个新的chain，分叉canonical
    ///
    /// This will also verify the state root of the block extending the canonical chain.
    /// 这也会校验扩展canonical chain的state root
    pub fn new_canonical_head_fork<DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<Self, InsertBlockError>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let state = BundleStateWithReceipts::default();
        let empty = BTreeMap::new();

        let state_provider = BundleStateDataRef {
            state: &state,
            sidechain_block_hashes: &empty,
            canonical_block_hashes,
            canonical_fork,
        };

        // 检验并且执行canonical head descendant
        let bundle_state = Self::validate_and_execute_canonical_head_descendant(
            block.clone(),
            parent_header,
            state_provider,
            externals,
        )
        .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?;

        Ok(Self { chain: Chain::new(vec![block], bundle_state) })
    }

    /// Create a new chain that forks off of the canonical chain.
    /// 创建一个新的chain，分叉canonicla chain
    pub fn new_canonical_fork<DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_header: &SealedHeader,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<Self, InsertBlockError>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let state = BundleStateWithReceipts::default();
        let empty = BTreeMap::new();

        let state_provider = BundleStateDataRef {
            state: &state,
            sidechain_block_hashes: &empty,
            canonical_block_hashes,
            canonical_fork,
        };

        let bundle_state = Self::validate_and_execute_sidechain(
            block.clone(),
            parent_header,
            state_provider,
            externals,
        )
        .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?;

        Ok(Self { chain: Chain::new(vec![block], bundle_state) })
    }

    /// Create a new chain that forks off of an existing sidechain.
    ///
    /// This differs from [AppendableChain::new_canonical_fork] in that this starts a new fork.
    pub(crate) fn new_chain_fork<DB, C, EF>(
        &self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        canonical_fork: ForkBlock,
        externals: &TreeExternals<DB, C, EF>,
    ) -> Result<Self, InsertBlockError>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let parent_number = block.number - 1;
        let parent = self.blocks().get(&parent_number).ok_or_else(|| {
            InsertBlockError::tree_error(
                BlockchainTreeError::BlockNumberNotFoundInChain { block_number: parent_number },
                block.block.clone(),
            )
        })?;

        let mut state = self.state.clone();

        // Revert state to the state after execution of the parent block
        state.revert_to(parent.number);

        // Revert changesets to get the state of the parent that we need to apply the change.
        let post_state_data = BundleStateDataRef {
            state: &state,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };
        let block_state =
            Self::validate_and_execute_sidechain(block.clone(), parent, post_state_data, externals)
                .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?;
        state.extend(block_state);

        let chain =
            Self { chain: Chain { state, blocks: BTreeMap::from([(block.number, block)]) } };

        // If all is okay, return new chain back. Present chain is not modified.
        Ok(chain)
    }

    /// Validate and execute the given block that _extends the canonical chain_, validating its
    /// state root after execution.
    /// 校验并且执行给定的block，扩展canonical chain，校验它的state，在执行之后
    fn validate_and_execute<BSDP, DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        post_state_data_provider: BSDP,
        externals: &TreeExternals<DB, C, EF>,
        block_kind: BlockKind,
    ) -> RethResult<BundleStateWithReceipts>
    where
        BSDP: BundleStateDataProvider,
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        // some checks are done before blocks comes here.
        // 在blocks到这里来之前的一些检查
        externals.consensus.validate_header_against_parent(&block, parent_block)?;

        let (block, senders) = block.into_components();
        let block = block.unseal();

        // get the state provider.
        // 获取state provider
        let db = externals.database();
        let canonical_fork = post_state_data_provider.canonical_fork();
        let state_provider = db.history_by_block_number(canonical_fork.number)?;

        let provider = BundleStateProvider::new(state_provider, post_state_data_provider);

        // 获取executor
        let mut executor = externals.executor_factory.with_state(&provider);
        executor.execute_and_verify_receipt(&block, U256::MAX, Some(senders))?;
        let bundle_state = executor.take_output_state();

        // check state root if the block extends the canonical chain.
        // 检查state root，是否block扩展了canonical chain
        if block_kind.extends_canonical_head() {
            // check state root
            // 检查state root
            let state_root = provider.state_root(&bundle_state)?;
            if block.state_root != state_root {
                return Err(ConsensusError::BodyStateRootDiff {
                    got: state_root,
                    expected: block.state_root,
                }
                .into())
            }
        }

        Ok(bundle_state)
    }

    /// Validate and execute the given block that _extends the canonical chain_, validating its
    /// state root after execution.
    /// 校验并且执行给定的block，扩展canonical chain，校验它的state root，在执行之后
    fn validate_and_execute_canonical_head_descendant<BSDP, DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        post_state_data_provider: BSDP,
        externals: &TreeExternals<DB, C, EF>,
    ) -> RethResult<BundleStateWithReceipts>
    where
        BSDP: BundleStateDataProvider,
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        Self::validate_and_execute(
            block,
            parent_block,
            post_state_data_provider,
            externals,
            BlockKind::ExtendsCanonicalHead,
        )
    }

    /// Validate and execute the given sidechain block, skipping state root validation.
    /// 校验并且执行给定的sidechain block，跳过state root的校验
    fn validate_and_execute_sidechain<BSDP, DB, C, EF>(
        block: SealedBlockWithSenders,
        parent_block: &SealedHeader,
        post_state_data_provider: BSDP,
        externals: &TreeExternals<DB, C, EF>,
    ) -> RethResult<BundleStateWithReceipts>
    where
        BSDP: BundleStateDataProvider,
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        Self::validate_and_execute(
            block,
            parent_block,
            post_state_data_provider,
            externals,
            BlockKind::ForksHistoricalBlock,
        )
    }

    /// Validate and execute the given block, and append it to this chain.
    /// 校验并且执行给定的block，扩展到这个chain
    ///
    /// This expects that the block's ancestors can be traced back to the `canonical_fork` (the
    /// first parent block of the `block`'s chain that is in the canonical chain).
    /// 这期望block的ancestor可以追溯到`canonical_fork`（这个`block`的第一个parent
    /// block的chain在canoncial chain）
    ///
    /// In other words, expects a gap less (side-) chain:  [`canonical_fork..block`] in order to be
    /// able to __execute__ the block.
    ///
    /// CAUTION: This will only perform state root check if it's possible: if the `canonical_fork`
    /// is the canonical head, or: state root check can't be performed if the given canonical is
    /// __not__ the canonical head.
    #[track_caller]
    pub(crate) fn append_block<DB, C, EF>(
        &mut self,
        block: SealedBlockWithSenders,
        side_chain_block_hashes: BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &BTreeMap<BlockNumber, BlockHash>,
        externals: &TreeExternals<DB, C, EF>,
        canonical_fork: ForkBlock,
        block_kind: BlockKind,
    ) -> Result<(), InsertBlockError>
    where
        DB: Database,
        C: Consensus,
        EF: ExecutorFactory,
    {
        let (_, parent_block) = self.blocks.last_key_value().expect("Chain has at least one block");

        let post_state_data = BundleStateDataRef {
            state: &self.state,
            sidechain_block_hashes: &side_chain_block_hashes,
            canonical_block_hashes,
            canonical_fork,
        };

        let block_state = Self::validate_and_execute(
            block.clone(),
            parent_block,
            post_state_data,
            externals,
            block_kind,
        )
        .map_err(|err| InsertBlockError::new(block.block.clone(), err.into()))?;
        // extend the state.
        // 扩展state
        self.state.extend(block_state);
        self.blocks.insert(block.number, block);
        Ok(())
    }
}

/// Represents what kind of block is being executed and validated.
/// 代表执行和校验的block的类型
///
/// This is required because the state root check can only be performed if the targeted block can be
/// traced back to the canonical __head__.
/// 这是需要的，因为state root check可以被执行，只有targeted block可以在canonical __head__被trace到
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockKind {
    /// The `block` is a descendant of the canonical head:
    /// 这个block是canonical head的后代
    ///
    ///    [`head..(block.parent)*,block`]
    ExtendsCanonicalHead,
    /// The block can be traced back to an ancestor of the canonical head: a historical block, but
    /// this chain does __not__ include the canonical head.
    /// 这个block可以追溯到canonical head的一个ancestor：一个historical
    /// block，但是这个chain不包含canonical head
    ForksHistoricalBlock,
}

impl BlockKind {
    /// Returns `true` if the block is a descendant of the canonical head.
    /// 返回`true`，如果这个block是canonical head的后代
    #[inline]
    pub(crate) fn extends_canonical_head(&self) -> bool {
        matches!(self, BlockKind::ExtendsCanonicalHead)
    }
}
