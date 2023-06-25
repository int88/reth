use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Account, Address, BlockNumber};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{RangeBounds, RangeInclusive},
};

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountReader: Send + Sync {
    /// Get basic account information.
    /// 获取基本的account信息
    ///
    /// Returns `None` if the account doesn't exist.
    /// 返回`None`如果account不存在
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;
}

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountExtReader: Send + Sync {
    /// Iterate over account changesets and return all account address that were changed.
    /// 遍历account changesets并且返回所有被改变的account地址
    fn changed_accounts_with_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<BTreeSet<Address>>;

    /// Get basic account information for multiple accounts. A more efficient version than calling
    /// [`AccountReader::basic_account`] repeatedly.
    /// 获取基本的account信息，对于多个accounts，比调用[`AccountReader::basic_account`]更有效率
    ///
    /// Returns `None` if the account doesn't exist.
    /// 返回`None`如果account不存在
    fn basic_accounts(
        &self,
        _iter: impl IntoIterator<Item = Address>,
    ) -> Result<Vec<(Address, Option<Account>)>>;

    /// Iterate over account changesets and return all account addresses that were changed alongside
    /// each specific set of blocks.
    /// 迭代account changesets并且返回所有被改变的account地址和每个特定的块集合
    ///
    /// NOTE: Get inclusive range of blocks.
    /// 注意：获取包含区块的范围
    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<Address, Vec<BlockNumber>>>;
}

/// Account reader
#[auto_impl(&, Arc, Box)]
pub trait AccountWriter: Send + Sync {
    /// Unwind and clear account hashing
    /// Unwind并且清理account hashing
    fn unwind_account_hashing(&self, range: RangeInclusive<BlockNumber>) -> Result<()>;

    /// Unwind and clear account history indices.
    /// Unwind并且清理account history indices
    ///
    /// Returns number of changesets walked.
    /// 返回遍历的changesets的数量
    fn unwind_account_history_indices(&self, range: RangeInclusive<BlockNumber>) -> Result<usize>;

    /// Insert account change index to database. Used inside AccountHistoryIndex stage
    /// 插入account change index到数据库，使用在AccountHistoryIndex stage中
    fn insert_account_history_index(
        &self,
        account_transitions: BTreeMap<Address, Vec<u64>>,
    ) -> Result<()>;

    /// iterate over accounts and insert them to hashing table
    /// 遍历accounts并且插入到hashing table中
    fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> Result<()>;
}
