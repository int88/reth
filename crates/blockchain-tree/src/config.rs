//! Blockchain tree configuration

/// The configuration for the blockchain tree.
/// blockchain tree的配置
#[derive(Clone, Debug)]
pub struct BlockchainTreeConfig {
    /// Number of blocks after the last finalized block that we are storing.
    /// 在最后一个finalized block之后，我们存储的block数量
    ///
    /// It should be more than the finalization window for the canonical chain.
    /// 它应该比canonical chain的finalization window更大
    max_blocks_in_chain: u64,
    /// The number of blocks that can be re-orged (finalization windows)
    /// 应该被re-orged的block的数量（finalization windows）
    max_reorg_depth: u64,
    /// The number of unconnected blocks that we are buffering
    /// 我们缓存的unconnected blocks的数量
    max_unconnected_blocks: usize,
    /// For EVM's "BLOCKHASH" opcode we require last 256 block hashes. So we need to specify
    /// at least `additional_canonical_block_hashes`+`max_reorg_depth`, for eth that would be
    /// 256+64.
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
    pub fn new(
        max_reorg_depth: u64,
        max_blocks_in_chain: u64,
        num_of_additional_canonical_block_hashes: u64,
        max_unconnected_blocks: usize,
    ) -> Self {
        if max_reorg_depth > max_blocks_in_chain {
            panic!("Side chain size should be more then finalization window");
        }
        Self {
            max_blocks_in_chain,
            max_reorg_depth,
            num_of_additional_canonical_block_hashes,
            max_unconnected_blocks,
        }
    }

    /// Return the maximum reorg depth.
    /// 返回做大的reorg depth
    pub fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    /// Return the maximum number of blocks in one chain.
    /// 返回一个chain中的最大block数量
    pub fn max_blocks_in_chain(&self) -> u64 {
        self.max_blocks_in_chain
    }

    /// Return number of additional canonical block hashes that we need to retain
    /// in order to have enough information for EVM execution.
    pub fn num_of_additional_canonical_block_hashes(&self) -> u64 {
        self.num_of_additional_canonical_block_hashes
    }

    /// Return max number of unconnected blocks that we are buffering
    pub fn max_unconnected_blocks(&self) -> usize {
        self.max_unconnected_blocks
    }
}
