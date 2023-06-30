//! Tables and data models.
//!
//! # Overview
//!
//! This module defines the tables in reth, as well as some table-related abstractions:
//!
//! - [`codecs`] integrates different codecs into [`Encode`](crate::abstraction::table::Encode) and
//!   [`Decode`](crate::abstraction::table::Decode)
//! - [`models`] defines the values written to tables
//!
//! # Database Tour
//!
//! TODO(onbjerg): Find appropriate format for this...

pub mod codecs;
pub mod models;
mod raw;
pub(crate) mod utils;

pub use raw::{RawDupSort, RawKey, RawTable, RawValue};

/// Declaration of all Database tables.
use crate::{
    table::DupSort,
    tables::{
        codecs::CompactU256,
        models::{
            accounts::{AccountBeforeTx, BlockNumberAddress},
            blocks::{HeaderHash, StoredBlockOmmers},
            storage_sharded_key::StorageShardedKey,
            ShardedKey, StoredBlockBodyIndices, StoredBlockWithdrawals,
        },
    },
};
use reth_primitives::{
    stage::StageCheckpoint,
    trie::{BranchNodeCompact, StorageTrieEntry, StoredNibbles, StoredNibblesSubKey},
    Account, Address, BlockHash, BlockNumber, Bytecode, Header, IntegerList, Receipt, StorageEntry,
    TransactionSignedNoHash, TxHash, TxNumber, H256,
};

/// Enum for the types of tables present in libmdbx.
#[derive(Debug)]
pub enum TableType {
    /// key value table
    Table,
    /// Duplicate key value table
    DupSort,
}

/// Number of tables that should be present inside database.
pub const NUM_TABLES: usize = 25;

/// Default tables that should be present inside database.
pub const TABLES: [(TableType, &str); NUM_TABLES] = [
    (TableType::Table, CanonicalHeaders::const_name()),
    (TableType::Table, HeaderTD::const_name()),
    (TableType::Table, HeaderNumbers::const_name()),
    (TableType::Table, Headers::const_name()),
    (TableType::Table, BlockBodyIndices::const_name()),
    (TableType::Table, BlockOmmers::const_name()),
    (TableType::Table, BlockWithdrawals::const_name()),
    (TableType::Table, TransactionBlock::const_name()),
    (TableType::Table, Transactions::const_name()),
    (TableType::Table, TxHashNumber::const_name()),
    (TableType::Table, Receipts::const_name()),
    (TableType::Table, PlainAccountState::const_name()),
    (TableType::DupSort, PlainStorageState::const_name()),
    (TableType::Table, Bytecodes::const_name()),
    (TableType::Table, AccountHistory::const_name()),
    (TableType::Table, StorageHistory::const_name()),
    (TableType::DupSort, AccountChangeSet::const_name()),
    (TableType::DupSort, StorageChangeSet::const_name()),
    (TableType::Table, HashedAccount::const_name()),
    (TableType::DupSort, HashedStorage::const_name()),
    (TableType::Table, AccountsTrie::const_name()),
    (TableType::DupSort, StoragesTrie::const_name()),
    (TableType::Table, TxSenders::const_name()),
    (TableType::Table, SyncStage::const_name()),
    (TableType::Table, SyncStageProgress::const_name()),
];

#[macro_export]
/// Macro to declare key value table.
/// 用来申明key value table的宏
macro_rules! table {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty | $value:ty) => {
        $(#[$docs])+
        ///
        #[doc = concat!("Takes [`", stringify!($key), "`] as a key and returns [`", stringify!($value), "`]")]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $table_name;

        impl $crate::table::Table for $table_name {
            const NAME: &'static str = $table_name::const_name();
            type Key = $key;
            type Value = $value;
        }

        impl $table_name {
            #[doc=concat!("Return ", stringify!($table_name), " as it is present inside the database.")]
            pub const fn const_name() -> &'static str {
                stringify!($table_name)
            }
        }

        impl std::fmt::Display for $table_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", stringify!($table_name))
            }
        }
    };
}

#[macro_export]
/// Macro to declare duplicate key value table.
/// 宏用于声明重复的key value table
macro_rules! dupsort {
    ($(#[$docs:meta])+ ( $table_name:ident ) $key:ty | [$subkey:ty] $value:ty) => {
        table!(
            $(#[$docs])+
            ///
            #[doc = concat!("`DUPSORT` table with subkey being: [`", stringify!($subkey), "`].")]
            ( $table_name ) $key | $value
        );
        impl DupSort for $table_name {
            type SubKey = $subkey;
        }
    };
}

//
//  TABLE DEFINITIONS
//

table!(
    /// Stores the header hashes belonging to the canonical chain.
    ( CanonicalHeaders ) BlockNumber | HeaderHash
);

table!(
    /// Stores the total difficulty from a block header.
    ( HeaderTD ) BlockNumber | CompactU256
);

table!(
    /// Stores the block number corresponding to a header.
    ( HeaderNumbers ) BlockHash | BlockNumber
);

table!(
    /// Stores header bodies.
    ( Headers ) BlockNumber | Header
);

table!(
    /// Stores block indices that contains indexes of transaction and the count of them.
    /// 存储block indices，包含transaction的索引以及它们的数目
    ///
    /// More information about stored indices can be found in the [`StoredBlockBodyIndices`] struct.
    ( BlockBodyIndices ) BlockNumber | StoredBlockBodyIndices
);

table!(
    /// Stores the uncles/ommers of the block.
    ( BlockOmmers ) BlockNumber | StoredBlockOmmers
);

table!(
    /// Stores the block withdrawals.
    ( BlockWithdrawals ) BlockNumber | StoredBlockWithdrawals
);

table!(
    /// (Canonical only) Stores the transaction body for canonical transactions.
    ( Transactions ) TxNumber | TransactionSignedNoHash
);

table!(
    /// Stores the mapping of the transaction hash to the transaction number.
    ( TxHashNumber ) TxHash | TxNumber
);

table!(
    /// Stores the mapping of transaction number to the blocks number.
    ///
    /// The key is the highest transaction ID in the block.
    ( TransactionBlock ) TxNumber | BlockNumber
);

table!(
    /// (Canonical only) Stores transaction receipts.
    ( Receipts ) TxNumber | Receipt
);

table!(
    /// Stores all smart contract bytecodes.
    /// There will be multiple accounts that have same bytecode
    /// So we would need to introduce reference counter.
    /// This will be small optimization on state.
    ( Bytecodes ) H256 | Bytecode
);

table!(
    /// Stores the current state of an [`Account`].
    /// 存储一个[`Account`]的当前状态
    ( PlainAccountState ) Address | Account
);

dupsort!(
    /// Stores the current value of a storage key.
    ( PlainStorageState ) Address | [H256] StorageEntry
);

table!(
    /// Stores pointers to block changeset with changes for each account key.
    /// 存储到block changeset 的指针，每个account key的变化
    ///
    /// Last shard key of the storage will contain `u64::MAX` `BlockNumber`,
    /// storage的最后一个shard会保存`u64::MAX` `BlockNumber`
    /// this would allows us small optimization on db access when change is in plain state.
    /// 这会允许small optimization，对于db的访问，当在plain state发生变更的时候
    ///
    /// Imagine having shards as:
    /// * `Address | 100`
    /// * `Address | u64::MAX`
    ///
    /// What we need to find is number that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// 我们需要找到的是大于N的数字
    /// * For N=50 we would get first shard.
    /// * 对于N=50，我们会找到第一个shard
    /// * for N=150 we would get second shard.
    /// * If max block number is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `AccountPlainState`.
    /// * 如果max block number是200并且我们寻找N=250，我们会
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    /// * 如果没有shard，我们会获取`None` entry或者有着不同storage key的entry
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    ( AccountHistory ) ShardedKey<Address> | BlockNumberList
);

table!(
    /// Stores pointers to block number changeset with changes for each storage key.
    /// 存储指针指向block number changeset，对于每个storage key的变更
    ///
    /// Last shard key of the storage will contain `u64::MAX` `BlockNumber`,
    /// this would allows us small optimization on db access when change is in plain state.
    ///
    /// Imagine having shards as:
    /// * `Address | StorageKey | 100`
    /// * `Address | StorageKey | u64::MAX`
    ///
    /// What we need to find is number that is one greater than N. Db `seek` function allows us to fetch
    /// the shard that equal or more than asked. For example:
    /// * For N=50 we would get first shard.
    /// * for N=150 we would get second shard.
    /// * If max block number is 200 and we ask for N=250 we would fetch last shard and
    ///     know that needed entry is in `StoragePlainState`.
    /// * If there were no shard we would get `None` entry or entry of different storage key.
    ///
    /// Code example can be found in `reth_provider::HistoricalStateProviderRef`
    ( StorageHistory ) StorageShardedKey | BlockNumberList
);

dupsort!(
    /// Stores the state of an account before a certain transaction changed it.
    /// Change on state can be: account is created, selfdestructed, touched while empty
    /// or changed (balance,nonce).
    /// 存储一个account的state，在一个特定的transaction改变它之前。
    /// state的改变可以为：account被创建，自毁，空的时候被touch，或者改变（balance，nonce）
    ( AccountChangeSet ) BlockNumber | [Address] AccountBeforeTx
);

dupsort!(
    /// Stores the state of a storage key before a certain transaction changed it.
    /// If [`StorageEntry::value`] is zero, this means storage was not existing
    /// and needs to be removed.
    /// 存储一个storage的state，在一个特定的transaction改变它之前。如果[`StorageEntry::value`]为0，这意味着storage不存在，需要被移除。
    ( StorageChangeSet ) BlockNumberAddress | [H256] StorageEntry
);

table!(
    /// Stores the current state of an [`Account`] indexed with `keccak256(Address)`
    /// 存储一个Account的当前状态，用`keccak256(Address)`作为索引
    /// This table is in preparation for merkelization and calculation of state root.
    /// 这个table在准备state root的merkelization和calculation
    /// We are saving whole account data as it is needed for partial update when
    /// part of storage is changed. Benefit for merkelization is that hashed addresses are sorted.
    /// 我们保存整个account data，因为它在partial update，当storage部分改变的时候需要，merkelization的优势是
    /// hashed account是有序的
    ( HashedAccount ) H256 | Account
);

dupsort!(
    /// Stores the current storage values indexed with `keccak256(Address)` and
    /// hash of storage key `keccak256(key)`.
    /// 存储当前的storage values，用`keccak256(Address)`以及`keccak256(key)`的storage key的hash当索引
    /// This table is in preparation for merkelization and calculation of state root.
    /// 这个table用于准备merkelization以及state root的计算
    /// Benefit for merklization is that hashed addresses/keys are sorted.
    /// merklization的好处是hashed address/keys是有序的
    ( HashedStorage ) H256 | [H256] StorageEntry
);

table!(
    /// Stores the current state's Merkle Patricia Tree.
    /// 存储当前state的Merkle Patricia Tree
    ( AccountsTrie ) StoredNibbles | BranchNodeCompact
);

dupsort!(
    /// From HashedAddress => NibblesSubKey => Intermediate value
    /// 从HashedAddress到NibblesSubKey到中间值
    ( StoragesTrie ) H256 | [StoredNibblesSubKey] StorageTrieEntry
);

table!(
    /// Stores the transaction sender for each canonical transaction.
    /// It is needed to speed up execution stage and allows fetching signer without doing
    /// transaction signed recovery
    ( TxSenders ) TxNumber | Address
);

table!(
    /// Stores the highest synced block number and stage-specific checkpoint of each stage.
    ( SyncStage ) StageId | StageCheckpoint
);

table!(
    /// Stores arbitrary data to keep track of a stage first-sync progress.
    ( SyncStageProgress ) StageId | Vec<u8>
);

/// Alias Types

/// List with transaction numbers.
/// 一系列的transaction numbers
pub type BlockNumberList = IntegerList;
/// Encoded stage id.
pub type StageId = String;
