use crate::{
    providers::state::{historical::HistoricalStateProvider, latest::LatestStateProvider},
    traits::{BlockSource, ReceiptProvider},
    BlockHashProvider, BlockNumProvider, BlockProvider, EvmEnvProvider, HeaderProvider,
    ProviderError, StageCheckpointProvider, StateProviderBox, TransactionsProvider,
    WithdrawalsProvider,
};
use reth_db::{database::Database, models::StoredBlockBodyIndices, tables, transaction::DbTx};
use reth_interfaces::Result;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Block, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, ChainSpec, Header, Receipt,
    SealedBlock, SealedHeader, TransactionMeta, TransactionSigned, TxHash, TxNumber, Withdrawal,
    H256, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
use std::{ops::RangeBounds, sync::Arc};
use tracing::trace;

mod provider;
use provider::{DatabaseProvider, DatabaseProviderRO, DatabaseProviderRW};

/// A common provider that fetches data from a database.
/// 一个公共的provider，从数据库中获取数据
///
/// This provider implements most provider or provider factory traits.
/// 这个provider实现了大多数provider或者provider factory traits
#[derive(Debug)]
pub struct ShareableDatabase<DB> {
    /// Database
    db: DB,
    /// Chain spec
    chain_spec: Arc<ChainSpec>,
}

impl<DB: Database> ShareableDatabase<DB> {
    /// Returns a provider with a created `DbTx` inside, which allows fetching data from the
    /// database using different types of providers. Example: [`HeaderProvider`]
    /// 返回一个provider，其中包含一个创建的`DbTx`，它允许使用不同类型的provider从数据库中获取数据。例如：[`HeaderProvider`]
    /// [`BlockHashProvider`]
    pub fn provider(&self) -> Result<DatabaseProviderRO<'_, DB>> {
        Ok(DatabaseProvider::new(self.db.tx()?, self.chain_spec.clone()))
    }

    /// Returns a provider with a created `DbTxMut` inside, which allows fetching and updating
    /// data from the database using different types of providers. Example: [`HeaderProvider`]
    /// [`BlockHashProvider`]
    pub fn provider_rw(&self) -> Result<DatabaseProviderRW<'_, DB>> {
        Ok(DatabaseProvider::new_rw(self.db.tx_mut()?, self.chain_spec.clone()))
    }
}

impl<DB> ShareableDatabase<DB> {
    /// create new database provider
    pub fn new(db: DB, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, chain_spec }
    }
}

impl<DB: Clone> Clone for ShareableDatabase<DB> {
    fn clone(&self) -> Self {
        Self { db: self.db.clone(), chain_spec: Arc::clone(&self.chain_spec) }
    }
}

impl<DB: Database> ShareableDatabase<DB> {
    /// Storage provider for latest block
    pub fn latest(&self) -> Result<StateProviderBox<'_>> {
        trace!(target: "providers::db", "Returning latest state provider");
        Ok(Box::new(LatestStateProvider::new(self.db.tx()?)))
    }

    /// Storage provider for state at that given block
    pub fn history_by_block_number(
        &self,
        mut block_number: BlockNumber,
    ) -> Result<StateProviderBox<'_>> {
        let provider = self.provider()?;

        if block_number == provider.best_block_number().unwrap_or_default() &&
            block_number == provider.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProvider::new(provider.into_tx())))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        block_number += 1;

        trace!(target: "providers::db", ?block_number, "Returning historical state provider for block number");
        Ok(Box::new(HistoricalStateProvider::new(provider.into_tx(), block_number)))
    }

    /// Storage provider for state at that given block hash
    pub fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<StateProviderBox<'_>> {
        let provider = self.provider()?;

        let mut block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;

        if block_number == provider.best_block_number().unwrap_or_default() &&
            block_number == provider.last_block_number().unwrap_or_default()
        {
            return Ok(Box::new(LatestStateProvider::new(provider.into_tx())))
        }

        // +1 as the changeset that we want is the one that was applied after this block.
        // as the  changeset contains old values.
        block_number += 1;

        trace!(target: "providers::db", ?block_hash, "Returning historical state provider for block hash");
        Ok(Box::new(HistoricalStateProvider::new(provider.into_tx(), block_number)))
    }
}

impl<DB: Database> HeaderProvider for ShareableDatabase<DB> {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        self.provider()?.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        self.provider()?.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        self.provider()?.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        self.provider()?.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        self.provider()?.headers_range(range)
    }

    fn sealed_headers_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        self.provider()?.sealed_headers_range(range)
    }

    fn sealed_header(&self, number: BlockNumber) -> Result<Option<SealedHeader>> {
        self.provider()?.sealed_header(number)
    }
}

impl<DB: Database> BlockHashProvider for ShareableDatabase<DB> {
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.provider()?.block_hash(number)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        self.provider()?.canonical_hashes_range(start, end)
    }
}

impl<DB: Database> BlockNumProvider for ShareableDatabase<DB> {
    fn chain_info(&self) -> Result<ChainInfo> {
        self.provider()?.chain_info()
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        self.provider()?.best_block_number()
    }

    fn last_block_number(&self) -> Result<BlockNumber> {
        self.provider()?.last_block_number()
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.provider()?.block_number(hash)
    }
}

impl<DB: Database> BlockProvider for ShareableDatabase<DB> {
    fn find_block_by_hash(&self, hash: H256, source: BlockSource) -> Result<Option<Block>> {
        self.provider()?.find_block_by_hash(hash, source)
    }

    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>> {
        self.provider()?.block(id)
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        self.provider()?.pending_block()
    }

    fn ommers(&self, id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
        self.provider()?.ommers(id)
    }

    fn block_body_indices(&self, num: u64) -> Result<Option<StoredBlockBodyIndices>> {
        self.provider()?.block_body_indices(num)
    }
}

impl<DB: Database> TransactionsProvider for ShareableDatabase<DB> {
    fn transaction_id(&self, tx_hash: TxHash) -> Result<Option<TxNumber>> {
        self.provider()?.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        self.provider()?.transaction_by_id(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        self.provider()?.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        self.provider()?.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> Result<Option<BlockNumber>> {
        self.provider()?.transaction_block(id)
    }

    fn transactions_by_block(
        &self,
        id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        self.provider()?.transactions_by_block_range(range)
    }
}

impl<DB: Database> ReceiptProvider for ShareableDatabase<DB> {
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        self.provider()?.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        self.provider()?.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        self.provider()?.receipts_by_block(block)
    }
}

impl<DB: Database> WithdrawalsProvider for ShareableDatabase<DB> {
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> Result<Option<Vec<Withdrawal>>> {
        self.provider()?.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
        self.provider()?.latest_withdrawal()
    }
}

impl<DB: Database> StageCheckpointProvider for ShareableDatabase<DB> {
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>> {
        self.provider()?.get_stage_checkpoint(id)
    }
}

impl<DB: Database> EvmEnvProvider for ShareableDatabase<DB> {
    fn fill_env_at(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        at: BlockHashOrNumber,
    ) -> Result<()> {
        self.provider()?.fill_env_at(cfg, block_env, at)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        self.provider()?.fill_env_with_header(cfg, block_env, header)
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockHashOrNumber) -> Result<()> {
        self.provider()?.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        self.provider()?.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockHashOrNumber) -> Result<()> {
        self.provider()?.fill_cfg_env_at(cfg, at)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        self.provider()?.fill_cfg_env_with_header(cfg, header)
    }
}

/// Get checkpoint for the given stage.
/// 获取给定stage的checkpoint
#[inline]
pub fn get_stage_checkpoint<'a, TX>(
    tx: &TX,
    id: StageId,
) -> std::result::Result<Option<StageCheckpoint>, reth_interfaces::db::DatabaseError>
where
    TX: DbTx<'a> + Send + Sync,
{
    tx.get::<tables::SyncStage>(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::ShareableDatabase;
    use crate::{BlockHashProvider, BlockNumProvider};
    use reth_db::mdbx::{test_utils::create_test_db, EnvKind, WriteMap};
    use reth_primitives::{ChainSpecBuilder, H256};
    use std::sync::Arc;

    #[test]
    fn common_history_provider() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let provider = ShareableDatabase::new(db, Arc::new(chain_spec));
        let _ = provider.latest();
    }

    #[test]
    fn default_chain_info() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let db = ShareableDatabase::new(db, Arc::new(chain_spec));
        let provider = db.provider().unwrap();

        let chain_info = provider.chain_info().expect("should be ok");
        assert_eq!(chain_info.best_number, 0);
        assert_eq!(chain_info.best_hash, H256::zero());
    }

    #[test]
    fn provider_flow() {
        let chain_spec = ChainSpecBuilder::mainnet().build();
        let db = create_test_db::<WriteMap>(EnvKind::RW);
        let db = ShareableDatabase::new(db, Arc::new(chain_spec));
        let provider = db.provider().unwrap();
        provider.block_hash(0).unwrap();
        let provider_rw = db.provider_rw().unwrap();
        provider_rw.block_hash(0).unwrap();
        provider.block_hash(0).unwrap();
    }
}
