use crate::{
    trie::{hash_builder::HashBuilderState, StoredSubNode},
    Address, BlockNumber, TxNumber, H256,
};
use bytes::{Buf, BufMut};
use reth_codecs::{derive_arbitrary, main_codec, Compact};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Saves the progress of Merkle stage.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct MerkleCheckpoint {
    /// The target block number.
    pub target_block: BlockNumber,
    /// The last hashed account key processed.
    pub last_account_key: H256,
    /// The last walker key processed.
    pub last_walker_key: Vec<u8>,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
}

impl MerkleCheckpoint {
    /// Creates a new Merkle checkpoint.
    pub fn new(
        target_block: BlockNumber,
        last_account_key: H256,
        last_walker_key: Vec<u8>,
        walker_stack: Vec<StoredSubNode>,
        state: HashBuilderState,
    ) -> Self {
        Self { target_block, last_account_key, last_walker_key, walker_stack, state }
    }
}

impl Compact for MerkleCheckpoint {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut len = 0;

        buf.put_u64(self.target_block);
        len += 8;

        buf.put_slice(self.last_account_key.as_slice());
        len += self.last_account_key.len();

        buf.put_u16(self.last_walker_key.len() as u16);
        buf.put_slice(&self.last_walker_key[..]);
        len += 2 + self.last_walker_key.len();

        buf.put_u16(self.walker_stack.len() as u16);
        len += 2;
        for item in self.walker_stack.into_iter() {
            len += item.to_compact(buf);
        }

        len += self.state.to_compact(buf);
        len
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let target_block = buf.get_u64();

        let last_account_key = H256::from_slice(&buf[..32]);
        buf.advance(32);

        let last_walker_key_len = buf.get_u16() as usize;
        let last_walker_key = Vec::from(&buf[..last_walker_key_len]);
        buf.advance(last_walker_key_len);

        let walker_stack_len = buf.get_u16() as usize;
        let mut walker_stack = Vec::with_capacity(walker_stack_len);
        for _ in 0..walker_stack_len {
            let (item, rest) = StoredSubNode::from_compact(buf, 0);
            walker_stack.push(item);
            buf = rest;
        }

        let (state, buf) = HashBuilderState::from_compact(buf, 0);
        (
            MerkleCheckpoint {
                target_block,
                last_account_key,
                last_walker_key,
                walker_stack,
                state,
            },
            buf,
        )
    }
}

/// Saves the progress of AccountHashing stage.
/// 保存AccountHashing stage的进度
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from
    /// 开始的下一个account
    pub address: Option<Address>,
    /// Start transition id
    /// 开始的transition id
    pub from: u64,
    /// Last transition id
    /// 最后的transition id
    pub to: u64,
}

/// Saves the progress of StorageHashing stage.
/// 保存StorageHashing stage的进度
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct StorageHashingCheckpoint {
    /// The next account to start hashing from
    /// 下一个开始hash的account
    pub address: Option<Address>,
    /// The next storage slot to start hashing from
    /// 下一个开始hash的storage slot
    pub storage: Option<H256>,
    /// Start transition id
    /// 开始的transition id
    pub from: u64,
    /// Last transition id
    /// 最后的transition id
    pub to: u64,
}

/// Saves the progress of abstract stage iterating over or downloading entities.
/// 保存抽象stage的进度，这个stage迭代或者下载entities
#[main_codec]
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct EntitiesCheckpoint {
    /// Number of entities already processed.
    pub processed: u64,
    /// Total entities to be processed.
    pub total: Option<u64>,
}

impl Display for EntitiesCheckpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(total) = self.total {
            write!(f, "{:.1}%", 100.0 * self.processed as f64 / total as f64)
        } else {
            write!(f, "{}", self.processed)
        }
    }
}

/// Saves the progress of a stage.
/// 保存一个stage的进度
#[main_codec]
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct StageCheckpoint {
    /// The maximum block processed by the stage.
    /// 这个stage处理的最大的block数目
    pub block_number: BlockNumber,
    /// Stage-specific checkpoint. None if stage uses only block-based checkpoints.
    /// Stage特定的checkpoint，如果stage只使用基于block的checkpoint，那么为None
    pub stage_checkpoint: Option<StageUnitCheckpoint>,
}

impl StageCheckpoint {
    /// Creates a new [`StageCheckpoint`] with only `block_number` set.
    /// 创建一个新的StageCheckpoint，只设置block_number
    pub fn new(block_number: BlockNumber) -> Self {
        Self { block_number, ..Default::default() }
    }

    /// Returns the account hashing stage checkpoint, if any.
    pub fn account_hashing_stage_checkpoint(&self) -> Option<AccountHashingCheckpoint> {
        match self.stage_checkpoint {
            Some(StageUnitCheckpoint::Account(checkpoint)) => Some(checkpoint),
            _ => None,
        }
    }

    /// Returns the storage hashing stage checkpoint, if any.
    pub fn storage_hashing_stage_checkpoint(&self) -> Option<StorageHashingCheckpoint> {
        match self.stage_checkpoint {
            Some(StageUnitCheckpoint::Storage(checkpoint)) => Some(checkpoint),
            _ => None,
        }
    }

    /// Returns the entities stage checkpoint, if any.
    pub fn entities_stage_checkpoint(&self) -> Option<EntitiesCheckpoint> {
        match self.stage_checkpoint {
            Some(StageUnitCheckpoint::Entities(checkpoint)) => Some(checkpoint),
            _ => None,
        }
    }

    /// Sets the stage checkpoint to account hashing.
    pub fn with_account_hashing_stage_checkpoint(
        mut self,
        checkpoint: AccountHashingCheckpoint,
    ) -> Self {
        self.stage_checkpoint = Some(StageUnitCheckpoint::Account(checkpoint));
        self
    }

    /// Sets the stage checkpoint to storage hashing.
    pub fn with_storage_hashing_stage_checkpoint(
        mut self,
        checkpoint: StorageHashingCheckpoint,
    ) -> Self {
        self.stage_checkpoint = Some(StageUnitCheckpoint::Storage(checkpoint));
        self
    }

    /// Sets the stage checkpoint to entities.
    pub fn with_entities_stage_checkpoint(mut self, checkpoint: EntitiesCheckpoint) -> Self {
        self.stage_checkpoint = Some(StageUnitCheckpoint::Entities(checkpoint));
        self
    }
}

impl Display for StageCheckpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.stage_checkpoint {
            Some(StageUnitCheckpoint::Entities(stage_checkpoint)) => stage_checkpoint.fmt(f),
            _ => write!(f, "{}", self.block_number),
        }
    }
}

// TODO(alexey): add a merkle checkpoint. Currently it's hard because [`MerkleCheckpoint`]
//  is not a Copy type.
/// Stage-specific checkpoint metrics.
#[derive_arbitrary(compact)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub enum StageUnitCheckpoint {
    /// Saves the progress of transaction-indexed stages.
    /// 保存transaction-indexed stages的进度
    Transaction(TxNumber),
    /// Saves the progress of AccountHashing stage.
    /// 保存AccountHashing stage的进度
    Account(AccountHashingCheckpoint),
    /// Saves the progress of StorageHashing stage.
    /// 保存StorageHashing stage的进度
    Storage(StorageHashingCheckpoint),
    /// Saves the progress of abstract stage iterating over or downloading entities.
    /// 保存抽象stage的进度，这个stage迭代或者下载entities
    Entities(EntitiesCheckpoint),
}

impl Compact for StageUnitCheckpoint {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            StageUnitCheckpoint::Transaction(data) => {
                buf.put_u8(0);
                1 + data.to_compact(buf)
            }
            StageUnitCheckpoint::Account(data) => {
                buf.put_u8(1);
                1 + data.to_compact(buf)
            }
            StageUnitCheckpoint::Storage(data) => {
                buf.put_u8(2);
                1 + data.to_compact(buf)
            }
            StageUnitCheckpoint::Entities(data) => {
                buf.put_u8(3);
                1 + data.to_compact(buf)
            }
        }
    }

    fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        match buf[0] {
            0 => {
                let (data, buf) = TxNumber::from_compact(&buf[1..], buf.len() - 1);
                (Self::Transaction(data), buf)
            }
            1 => {
                let (data, buf) = AccountHashingCheckpoint::from_compact(&buf[1..], buf.len() - 1);
                (Self::Account(data), buf)
            }
            2 => {
                let (data, buf) = StorageHashingCheckpoint::from_compact(&buf[1..], buf.len() - 1);
                (Self::Storage(data), buf)
            }
            3 => {
                let (data, buf) = EntitiesCheckpoint::from_compact(&buf[1..], buf.len() - 1);
                (Self::Entities(data), buf)
            }
            _ => unreachable!("Junk data in database: unknown StageUnitCheckpoint variant"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn merkle_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoint = MerkleCheckpoint {
            target_block: rng.gen(),
            last_account_key: H256::from_low_u64_be(rng.gen()),
            last_walker_key: H256::from_low_u64_be(rng.gen()).to_vec(),
            walker_stack: Vec::from([StoredSubNode {
                key: H256::from_low_u64_be(rng.gen()).to_vec(),
                nibble: Some(rng.gen()),
                node: None,
            }]),
            state: HashBuilderState::default(),
        };

        let mut buf = Vec::new();
        let encoded = checkpoint.clone().to_compact(&mut buf);
        let (decoded, _) = MerkleCheckpoint::from_compact(&buf, encoded);
        assert_eq!(decoded, checkpoint);
    }

    #[test]
    fn stage_unit_checkpoint_roundtrip() {
        let mut rng = rand::thread_rng();
        let checkpoints = vec![
            StageUnitCheckpoint::Transaction(rng.gen()),
            StageUnitCheckpoint::Account(AccountHashingCheckpoint {
                address: Some(Address::from_low_u64_be(rng.gen())),
                from: rng.gen(),
                to: rng.gen(),
            }),
            StageUnitCheckpoint::Storage(StorageHashingCheckpoint {
                address: Some(Address::from_low_u64_be(rng.gen())),
                storage: Some(H256::from_low_u64_be(rng.gen())),
                from: rng.gen(),
                to: rng.gen(),
            }),
        ];

        for checkpoint in checkpoints {
            let mut buf = Vec::new();
            let encoded = checkpoint.to_compact(&mut buf);
            let (decoded, _) = StageUnitCheckpoint::from_compact(&buf, encoded);
            assert_eq!(decoded, checkpoint);
        }
    }
}
