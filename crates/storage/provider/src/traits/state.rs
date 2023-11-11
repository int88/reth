use super::AccountReader;
use crate::{BlockHashReader, BlockIdReader, BundleStateWithReceipts};
use auto_impl::auto_impl;
use reth_interfaces::{provider::ProviderError, RethResult};
use reth_primitives::{
    Address, BlockHash, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag, Bytecode, Bytes,
    StorageKey, StorageValue, H256, KECCAK_EMPTY, U256,
};

/// Type alias of boxed [StateProvider].
pub type StateProviderBox<'a> = Box<dyn StateProvider + 'a>;

/// An abstraction for a type that provides state data.
/// 对于一个能提供state data的类型的抽象
#[auto_impl(&, Arc, Box)]
pub trait StateProvider: BlockHashReader + AccountReader + StateRootProvider + Send + Sync {
    /// Get storage of given account.
    /// 获取给定的accout的storage
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> RethResult<Option<StorageValue>>;

    /// Get account code by its hash
    /// 通过hash获取account code
    fn bytecode_by_hash(&self, code_hash: H256) -> RethResult<Option<Bytecode>>;

    /// Get account and storage proofs.
    fn proof(
        &self,
        address: Address,
        keys: &[H256],
    ) -> RethResult<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn account_code(&self, addr: Address) -> RethResult<Option<Bytecode>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        let acc = match self.basic_account(addr)? {
            Some(acc) => acc,
            None => return Ok(None),
        };

        if let Some(code_hash) = acc.bytecode_hash {
            if code_hash == KECCAK_EMPTY {
                return Ok(None)
            }
            // Get the code from the code hash
            return self.bytecode_by_hash(code_hash)
        }

        // Return `None` if no code hash is set
        Ok(None)
    }

    /// Get account balance by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_balance(&self, addr: Address) -> RethResult<Option<U256>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.balance)),
            None => Ok(None),
        }
    }

    /// Get account nonce by its address.
    /// 通过地址获取account nonce
    ///
    /// Returns `None` if the account doesn't exist
    /// 返回`None`如果account不存在
    fn account_nonce(&self, addr: Address) -> RethResult<Option<u64>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.nonce)),
            None => Ok(None),
        }
    }
}

/// Light wrapper that returns `StateProvider` implementations that correspond to the given
/// `BlockNumber`, the latest state, or the pending state.
/// Light wrapper，返回`StateProvider`的实现，对应给定的`BlockNumber`，最新的state或者pending state
///
/// This type differentiates states into `historical`, `latest` and `pending`, where the `latest`
/// block determines what is historical or pending: `[historical..latest..pending]`.
/// 这个类型区分`historical`, `latest`以及`pending`，其中`latest` block决定是historical还是pending
///
/// The `latest` state represents the state after the most recent block has been committed to the
/// database, `historical` states are states that have been committed to the database before the
/// `latest` state, and `pending` states are states that have not yet been committed to the
/// database which may or may not become the `latest` state, depending on consensus.
/// `latest` state代表最近的block被提交到数据库之后的state，`historical`
/// state代表已经被提交到db，但是在`latest` state之前的state，而`pending`
/// states是还没有提交到db的states，他们可能或者不可能成为`latest` state，取决于consensus
///
/// Note: the `pending` block is considered the block that extends the canonical chain but one and
/// has the `latest` block as its parent.
/// 注意：`pending` block被认为是扩展了canonical chain，但是有`latest` block作为它的parent
///
/// All states are _inclusive_, meaning they include _all_ all changes made (executed transactions)
/// in their respective blocks. For example [StateProviderFactory::history_by_block_number] for
/// block number `n` will return the state after block `n` was executed (transactions, withdrawals).
/// In other words, all states point to the end of the state's respective block, which is equivalent
/// to state at the beginning of the child block.
/// 所有states都是_inclusive_，这意味着他们包含对应block所有的changes（执行txs），例如，
/// [StateProviderFactory::history_by_block_number]对于block number `n`，会返回block
/// `n`（txs，withdrawals）被执行之后的state，换句话说所有的state指向state对应的block的end
///
/// This affects tracing, or replaying blocks, which will need to be executed on top of the state of
/// the parent block. For example, in order to trace block `n`, the state after block `n - 1` needs
/// to be used, since block `n` was executed on its parent block's state.
pub trait StateProviderFactory: BlockIdReader + Send + Sync {
    /// Storage provider for latest block.
    fn latest(&self) -> RethResult<StateProviderBox<'_>>;

    /// Returns a [StateProvider] indexed by the given [BlockId].
    /// 返回一个[StateProvider]，根据给定的BlockId
    ///
    /// Note: if a number or hash is provided this will only look at historical(canonical) state.
    /// 注意：如果一个number或者hash被提供，这只会查看historical(canonical)state
    fn state_by_block_id(&self, block_id: BlockId) -> RethResult<StateProviderBox<'_>> {
        match block_id {
            BlockId::Number(block_number) => self.state_by_block_number_or_tag(block_number),
            BlockId::Hash(block_hash) => self.history_by_block_hash(block_hash.into()),
        }
    }

    /// Returns a [StateProvider] indexed by the given block number or tag.
    ///
    /// Note: if a number is provided this will only look at historical(canonical) state.
    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> RethResult<StateProviderBox<'_>> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash = match self.finalized_block_hash()? {
                    Some(hash) => hash,
                    None => return Err(ProviderError::FinalizedBlockNotFound.into()),
                };
                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = match self.safe_block_hash()? {
                    Some(hash) => hash,
                    None => return Err(ProviderError::SafeBlockNotFound.into()),
                };

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => {
                // Note: The `BlockchainProvider` could also lookup the tree for the given block number, if for example the block number is `latest + 1`, however this should only support canonical state: <https://github.com/paradigmxyz/reth/issues/4515>
                self.history_by_block_number(num)
            }
        }
    }

    /// Returns a historical [StateProvider] indexed by the given historic block number.
    ///
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_number(&self, block: BlockNumber) -> RethResult<StateProviderBox<'_>>;

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> RethResult<StateProviderBox<'_>>;

    /// Returns _any_[StateProvider] with matching block hash.
    ///
    /// This will return a [StateProvider] for either a historical or pending block.
    fn state_by_block_hash(&self, block: BlockHash) -> RethResult<StateProviderBox<'_>>;

    /// Storage provider for pending state.
    ///
    /// Represents the state at the block that extends the canonical chain by one.
    /// If there's no `pending` block, then this is equal to [StateProviderFactory::latest]
    fn pending(&self) -> RethResult<StateProviderBox<'_>>;

    /// Storage provider for pending state for the given block hash.
    ///
    /// Represents the state at the block that extends the canonical chain.
    ///
    /// If the block couldn't be found, returns `None`.
    fn pending_state_by_hash(&self, block_hash: H256) -> RethResult<Option<StateProviderBox<'_>>>;

    /// Return a [StateProvider] that contains post state data provider.
    /// Used to inspect or execute transaction on the pending state.
    fn pending_with_provider(
        &self,
        post_state_data: Box<dyn BundleStateDataProvider>,
    ) -> RethResult<StateProviderBox<'_>>;
}

/// Blockchain trait provider that gives access to the blockchain state that is not yet committed
/// (pending).
pub trait BlockchainTreePendingStateProvider: Send + Sync {
    /// Returns a state provider that includes all state changes of the given (pending) block hash.
    ///
    /// In other words, the state provider will return the state after all transactions of the given
    /// hash have been executed.
    fn pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> RethResult<Box<dyn BundleStateDataProvider>> {
        Ok(self
            .find_pending_state_provider(block_hash)
            .ok_or(ProviderError::StateForHashNotFound(block_hash))?)
    }

    /// Returns state provider if a matching block exists.
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn BundleStateDataProvider>>;
}

/// Post state data needs for execution on it.
/// 需要的Post state，在它之上执行
/// This trait is used to create a state provider over pending state.
/// 这个trait用于创建state provider，在pending state之上
///
/// Pending state contains:
/// Pending state包含：
/// * [`BundleStateWithReceipts`] contains all changed of accounts and storage of pending chain
/// * [`BundleStateWithReceipts`]包含所有的accounts以及pending chain的storage
/// * block hashes of pending chain and canonical blocks.
/// * pending chain和canonical blocks的block hashes
/// * canonical fork, the block on what pending chain was forked from.
/// * canonical fork，block分叉而来的pending chain
#[auto_impl[Box,&]]
pub trait BundleStateDataProvider: Send + Sync {
    /// Return post state
    fn state(&self) -> &BundleStateWithReceipts;
    /// Return block hash by block number of pending or canonical chain.
    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash>;
    /// return canonical fork, the block on what post state was forked from.
    /// 返回canonical fork，block分叉而来的post state
    ///
    /// Needed to create state provider.
    /// 创建state provider的试试需要
    fn canonical_fork(&self) -> BlockNumHash;
}

/// A type that can compute the state root of a given post state.
/// 一个类型可以计算state root，对于给定的post state
#[auto_impl[Box,&, Arc]]
pub trait StateRootProvider: Send + Sync {
    /// Returns the state root of the BundleState on top of the current state.
    /// 在当前的state之上返回BundleState的state root
    fn state_root(&self, post_state: &BundleStateWithReceipts) -> RethResult<H256>;
}
