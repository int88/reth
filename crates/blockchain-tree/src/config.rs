//! Blockchain tree configuration

/// The configuration for the blockchain tree.
/// blockchain tree的配置
#[derive(Clone, Copy, Debug)]
pub struct BlockchainTreeConfig {
    /// Number of blocks after the last finalized block that we are storing.
    /// 在上一次finalized block之后，我们存储的blocks的数目
    ///
    /// It should be more than the finalization window for the canonical chain.
    /// 对于canonical chain，这应该超过finalization window
    max_blocks_in_chain: u64,
    /// The number of blocks that can be re-orged (finalization windows)
    /// 可以被reorg的blocks的数目（finalization）
    max_reorg_depth: u64,
    /// The number of unconnected blocks that we are buffering
    /// 我们正在缓存的unconnected blocks的数目
    max_unconnected_blocks: usize,
    /// Number of additional block hashes to save in blockchain tree. For `BLOCKHASH` EVM opcode we
    /// need last 256 block hashes.
    /// 额外的保存在blockchain tree中的block hashes，对于`BLOCKHASH` EVM
    /// opcode，我们需要最新的256个block hashes
    ///
    /// The total number of block hashes retained in-memory will be
    /// `max(additional_canonical_block_hashes, max_reorg_depth)`, and for Ethereum that would
    /// be 256. It covers both number of blocks required for reorg, and number of blocks
    /// required for `BLOCKHASH` EVM opcode.
    /// 同时覆盖了需要reorg的blocks的数目以及`BLOCKHASH` EVM opcode需要的blocks的数目
    num_of_additional_canonical_block_hashes: u64,
}

impl Default for BlockchainTreeConfig {
    fn default() -> Self {
        // The defaults for Ethereum mainnet
        Self {
            // Gasper allows reorgs of any length from 1 to 64.
            max_reorg_depth: 64,
            // This default is just an assumption. Has to be greater than the `max_reorg_depth`.
            max_blocks_in_chain: 65,
            // EVM requires that last 256 block hashes are available.
            num_of_additional_canonical_block_hashes: 256,
            // max unconnected blocks.
            max_unconnected_blocks: 200,
        }
    }
}

impl BlockchainTreeConfig {
    /// Create tree configuration.
    /// 创建tree的配置
    pub fn new(
        max_reorg_depth: u64,
        max_blocks_in_chain: u64,
        num_of_additional_canonical_block_hashes: u64,
        max_unconnected_blocks: usize,
    ) -> Self {
        if max_reorg_depth > max_blocks_in_chain {
            // sidechain的大小应该大于finalization window
            panic!("Side chain size should be more than finalization window");
        }
        Self {
            max_blocks_in_chain,
            max_reorg_depth,
            num_of_additional_canonical_block_hashes,
            max_unconnected_blocks,
        }
    }

    /// Return the maximum reorg depth.
    /// 返回最大的reorg depth
    pub fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    /// Return the maximum number of blocks in one chain.
    pub fn max_blocks_in_chain(&self) -> u64 {
        self.max_blocks_in_chain
    }

    /// Return number of additional canonical block hashes that we need to retain
    /// in order to have enough information for EVM execution.
    pub fn num_of_additional_canonical_block_hashes(&self) -> u64 {
        self.num_of_additional_canonical_block_hashes
    }

    /// Return total number of canonical hashes that we need to retain in order to have enough
    /// information for reorg and EVM execution.
    ///
    /// It is calculated as the maximum of `max_reorg_depth` (which is the number of blocks required
    /// for the deepest reorg possible according to the consensus protocol) and
    /// `num_of_additional_canonical_block_hashes` (which is the number of block hashes needed to
    /// satisfy the `BLOCKHASH` opcode in the EVM. See [`crate::BundleStateDataRef`] and
    /// [`crate::AppendableChain::new_canonical_head_fork`] where it's used).
    pub fn num_of_canonical_hashes(&self) -> u64 {
        self.max_reorg_depth.max(self.num_of_additional_canonical_block_hashes)
    }

    /// Return max number of unconnected blocks that we are buffering
    /// 返回我们正在缓冲的最大的unconnected blocks的数目
    pub fn max_unconnected_blocks(&self) -> usize {
        self.max_unconnected_blocks
    }
}
