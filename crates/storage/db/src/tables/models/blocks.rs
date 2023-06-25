//! Block related models and types.
//! Block相关的模型和类型。

use reth_codecs::{main_codec, Compact};
use reth_primitives::{Header, TxNumber, Withdrawal, H256};
use std::ops::Range;

/// Total number of transactions.
/// transactions的数目
pub type NumTransactions = u64;

/// The storage of the block body indices
/// block body indices的存储
///
/// It has the pointer to the transaction Number of the first
/// transaction in the block and the total number of transactions
/// 它有着指向block中第一个transaction的transaction Number的指针和transaction的总数
#[derive(Debug, Default, Eq, PartialEq, Clone)]
#[main_codec]
pub struct StoredBlockBodyIndices {
    /// The number of the first transaction in this block
    /// 这个block中第一个transaction的数目
    ///
    /// Note: If the block is empty, this is the number of the first transaction
    /// in the next non-empty block.
    /// 注意：如果这个block是空的，这个数目是下一个非空block中第一个transaction的数目
    pub first_tx_num: TxNumber,
    /// The total number of transactions in the block
    /// block中transaction的总数
    ///
    /// NOTE: Number of transitions is equal to number of transactions with
    /// additional transition for block change if block has block reward or withdrawal.
    /// 注意：transactions的数目等于transition的数目，如果block有block reward或者withdrawal，会有额外的transition
    pub tx_count: NumTransactions,
}

impl StoredBlockBodyIndices {
    /// Return the range of transaction ids for this block.
    pub fn tx_num_range(&self) -> Range<TxNumber> {
        self.first_tx_num..self.first_tx_num + self.tx_count
    }

    /// Return the index of last transaction in this block unless the block
    /// is empty in which case it refers to the last transaction in a previous
    /// non-empty block
    pub fn last_tx_num(&self) -> TxNumber {
        self.first_tx_num.saturating_add(self.tx_count).saturating_sub(1)
    }

    /// First transaction index.
    ///
    /// Caution: If the block is empty, this is the number of the first transaction
    /// in the next non-empty block.
    pub fn first_tx_num(&self) -> TxNumber {
        self.first_tx_num
    }

    /// Return the index of the next transaction after this block.
    pub fn next_tx_num(&self) -> TxNumber {
        self.first_tx_num + self.tx_count
    }

    /// Return a flag whether the block is empty
    pub fn is_empty(&self) -> bool {
        self.tx_count == 0
    }

    /// Return number of transaction inside block
    ///
    /// NOTE: This is not the same as the number of transitions.
    pub fn tx_count(&self) -> NumTransactions {
        self.tx_count
    }
}

/// The storage representation of a block ommers.
/// 一个block ommers的storage表示
///
/// It is stored as the headers of the block's uncles.
/// tx_amount)`.
#[main_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StoredBlockOmmers {
    /// The block headers of this block's uncles.
    /// block的uncles的block header
    pub ommers: Vec<Header>,
}

/// The storage representation of block withdrawals.
/// 这个storage表示block的withdrawals
#[main_codec]
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StoredBlockWithdrawals {
    /// The block withdrawals.
    pub withdrawals: Vec<Withdrawal>,
}

/// Hash of the block header. Value for [`CanonicalHeaders`][crate::tables::CanonicalHeaders]
pub type HeaderHash = H256;

#[cfg(test)]
mod test {
    use super::*;
    use crate::table::{Compress, Decompress};

    #[test]
    fn test_ommer() {
        let mut ommer = StoredBlockOmmers::default();
        ommer.ommers.push(Header::default());
        ommer.ommers.push(Header::default());
        assert!(
            ommer.clone() == StoredBlockOmmers::decompress::<Vec<_>>(ommer.compress()).unwrap()
        );
    }

    #[test]
    fn block_indices() {
        let first_tx_num = 10;
        let tx_count = 6;
        let block_indices = StoredBlockBodyIndices { first_tx_num, tx_count };

        // 第一个transaction number
        assert_eq!(block_indices.first_tx_num(), first_tx_num);
        // 最后一个transaction number
        assert_eq!(block_indices.last_tx_num(), first_tx_num + tx_count - 1);
        // 下一个transaction number
        assert_eq!(block_indices.next_tx_num(), first_tx_num + tx_count);
        // transaction的数目
        assert_eq!(block_indices.tx_count(), tx_count);
        // transaction的范围
        assert_eq!(block_indices.tx_num_range(), first_tx_num..first_tx_num + tx_count);
    }
}
