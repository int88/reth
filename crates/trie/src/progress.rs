use crate::{trie_cursor::CursorSubNode, updates::TrieUpdates};
use reth_primitives::{
    stage::MerkleCheckpoint,
    trie::{hash_builder::HashBuilder, Nibbles},
    H256,
};

/// The progress of the state root computation.
#[derive(Debug)]
pub enum StateRootProgress {
    /// The complete state root computation with updates and computed root.
    Complete(H256, usize, TrieUpdates),
    /// The intermediate progress of state root computation.
    /// Contains the walker stack, the hash builder and the trie updates.
    Progress(Box<IntermediateStateRootState>, usize, TrieUpdates),
}

/// The intermediate state of the state root computation.
/// state root计算过程的中间状态
#[derive(Debug)]
pub struct IntermediateStateRootState {
    /// Previously constructed hash builder.
    /// 之前构建的hash builder
    pub hash_builder: HashBuilder,
    /// Previously recorded walker stack.
    /// 之前记录的walker stack
    pub walker_stack: Vec<CursorSubNode>,
    /// The last hashed account key processed.
    /// 最新处理的hashed account key
    pub last_account_key: H256,
    /// The last walker key processed.
    /// 最新处理的last walker
    pub last_walker_key: Nibbles,
}

impl From<MerkleCheckpoint> for IntermediateStateRootState {
    fn from(value: MerkleCheckpoint) -> Self {
        Self {
            hash_builder: HashBuilder::from(value.state),
            walker_stack: value.walker_stack.into_iter().map(CursorSubNode::from).collect(),
            last_account_key: value.last_account_key,
            last_walker_key: Nibbles::from_hex(value.last_walker_key),
        }
    }
}
