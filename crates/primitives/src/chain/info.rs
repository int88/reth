use crate::{BlockNumber, H256};

/// Current status of the blockchain's head.
/// blockchain的head的当前状态。
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// The block hash of the highest fully synced block.
    /// 最高的完全同步的block的hash。
    pub best_hash: H256,
    /// The block number of the highest fully synced block.
    /// 最高的完全同步的block的block number。
    pub best_number: BlockNumber,
}
