use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::BlockNumberAddress,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_metrics::{
    metrics::{self, Gauge},
    Metrics,
};
use reth_primitives::{
    constants::MGAS_TO_GAS,
    stage::{StageCheckpoint, StageId},
    Block, BlockNumber, BlockWithSenders, TransactionSigned, U256,
};
use reth_provider::{
    post_state::PostState, BlockExecutor, ExecutorFactory, LatestStateProviderRef, Transaction,
};
use std::time::Instant;
use tracing::*;

/// Execution stage metrics.
#[derive(Metrics)]
#[metrics(scope = "sync.execution")]
pub struct ExecutionStageMetrics {
    /// The total amount of gas processed (in millions)
    mgas_processed_total: Gauge,
}

/// The execution stage executes all transactions and
/// update history indexes.
/// execution stage执行所有的transactions并且更新history indexes
///
/// Input tables:
/// - [tables::CanonicalHeaders] get next block to execute.
/// - [tables::Headers] get for revm environment variables.
/// - [tables::HeaderTD]
/// - [tables::BlockBodyIndices] to get tx number
/// - [tables::Transactions] to execute
///
/// For state access [LatestStateProviderRef] provides us latest state and history state
/// For latest most recent state [LatestStateProviderRef] would need (Used for execution Stage):
/// - [tables::PlainAccountState]
/// - [tables::Bytecodes]
/// - [tables::PlainStorageState]
///
/// Tables updated after state finishes execution:
/// - [tables::PlainAccountState]
/// - [tables::PlainStorageState]
/// - [tables::Bytecodes]
/// - [tables::AccountChangeSet]
/// - [tables::StorageChangeSet]
///
/// For unwinds we are accessing:
/// - [tables::BlockBodyIndices] get tx index to know what needs to be unwinded
/// - [tables::AccountHistory] to remove change set and apply old values to
/// - [tables::PlainAccountState] [tables::StorageHistory] to remove change set and apply old values
/// to [tables::PlainStorageState]
// false positive, we cannot derive it if !DB: Debug.
#[allow(missing_debug_implementations)]
pub struct ExecutionStage<EF: ExecutorFactory> {
    metrics: ExecutionStageMetrics,
    /// The stage's internal executor
    /// stage内部的executor
    executor_factory: EF,
    /// The commit thresholds of the execution stage.
    /// execution的commit阈值
    thresholds: ExecutionStageThresholds,
}

impl<EF: ExecutorFactory> ExecutionStage<EF> {
    /// Create new execution stage with specified config.
    pub fn new(executor_factory: EF, thresholds: ExecutionStageThresholds) -> Self {
        Self { metrics: ExecutionStageMetrics::default(), executor_factory, thresholds }
    }

    /// Create an execution stage with the provided  executor factory.
    ///
    /// The commit threshold will be set to 10_000.
    pub fn new_with_factory(executor_factory: EF) -> Self {
        Self::new(executor_factory, ExecutionStageThresholds::default())
    }

    // TODO: This should be in the block provider trait once we consolidate
    // SharedDatabase/Transaction
    fn read_block_with_senders<DB: Database>(
        tx: &Transaction<'_, DB>,
        block_number: BlockNumber,
    ) -> Result<(BlockWithSenders, U256), StageError> {
        let header = tx.get_header(block_number)?;
        let td = tx.get_td(block_number)?;
        let ommers = tx.get::<tables::BlockOmmers>(block_number)?.unwrap_or_default().ommers;
        let withdrawals = tx.get::<tables::BlockWithdrawals>(block_number)?.map(|v| v.withdrawals);

        // Get the block body
        let body = tx.get::<tables::BlockBodyIndices>(block_number)?.unwrap();
        let tx_range = body.tx_num_range();

        // Get the transactions in the body
        let (transactions, senders) = if tx_range.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let transactions = tx
                .cursor_read::<tables::Transactions>()?
                .walk_range(tx_range.clone())?
                .map(|entry| entry.map(|tx| tx.1))
                .collect::<Result<Vec<_>, _>>()?;

            let senders = tx
                .cursor_read::<tables::TxSenders>()?
                .walk_range(tx_range)?
                .map(|entry| entry.map(|sender| sender.1))
                .collect::<Result<Vec<_>, _>>()?;

            (transactions, senders)
        };

        let body = transactions
            .into_iter()
            .map(|tx| {
                TransactionSigned {
                    // TODO: This is the fastest way right now to make everything just work with
                    // a dummy transaction hash.
                    hash: Default::default(),
                    signature: tx.signature,
                    transaction: tx.transaction,
                }
            })
            .collect();
        Ok((Block { header, body, ommers, withdrawals }.with_senders(senders), td))
    }

    /// Execute the stage.
    pub fn execute_inner<DB: Database>(
        &self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let start_block = input.checkpoint().block_number + 1;
        let max_block = input.previous_stage_checkpoint().block_number;

        // Build executor
        let mut executor = self.executor_factory.with_sp(LatestStateProviderRef::new(&**tx));

        // Progress tracking
        let mut stage_progress = start_block;

        // Execute block range
        let mut state = PostState::default();
        for block_number in start_block..=max_block {
            let (block, td) = Self::read_block_with_senders(tx, block_number)?;

            // Configure the executor to use the current state.
            trace!(target: "sync::stages::execution", number = block_number, txs = block.body.len(), "Executing block");

            // Execute the block
            let (block, senders) = block.into_components();
            let block_state = executor
                .execute_and_verify_receipt(&block, td, Some(senders))
                .map_err(|error| StageError::ExecutionError {
                    block: block.header.clone().seal_slow(),
                    error,
                })?;

            // Gas metrics
            self.metrics
                .mgas_processed_total
                .increment(block.header.gas_used as f64 / MGAS_TO_GAS as f64);

            // Merge state changes
            state.extend(block_state);
            stage_progress = block_number;

            // Write history periodically to free up memory
            if self.thresholds.should_write_history(state.changeset_size_hint() as u64) {
                info!(target: "sync::stages::execution", ?block_number, "Writing history.");
                state.write_history_to_db(&**tx)?;
                info!(target: "sync::stages::execution", ?block_number, "Wrote history.");
                // gas_since_history_write = 0;
            }

            // Check if we should commit now
            if self.thresholds.is_end_of_batch(block_number - start_block, state.size_hint() as u64)
            {
                break
            }
        }

        // Write remaining changes
        trace!(target: "sync::stages::execution", accounts = state.accounts().len(), "Writing updated state to database");
        let start = Instant::now();
        state.write_to_db(&**tx)?;
        trace!(target: "sync::stages::execution", took = ?start.elapsed(), "Wrote state");

        let is_final_range = stage_progress == max_block;
        info!(target: "sync::stages::execution", stage_progress, is_final_range, "Stage iteration finished");
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(stage_progress), done: is_final_range })
    }
}

/// The size of the stack used by the executor.
///
/// Ensure the size is aligned to 8 as this is usually more efficient.
///
/// Currently 64 megabytes.
const BIG_STACK_SIZE: usize = 64 * 1024 * 1024;

#[async_trait::async_trait]
impl<EF: ExecutorFactory, DB: Database> Stage<DB> for ExecutionStage<EF> {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        StageId::Execution
    }

    /// Execute the stage
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // For Ethereum transactions that reaches the max call depth (1024) revm can use more stack
        // space than what is allocated by default.
        //
        // To make sure we do not panic in this case, spawn a thread with a big stack allocated.
        //
        // A fix in revm is pending to give more insight into the stack size, which we can use later
        // to optimize revm or move data to the heap.
        //
        // See https://github.com/bluealloy/revm/issues/305
        std::thread::scope(|scope| {
            let handle = std::thread::Builder::new()
                .stack_size(BIG_STACK_SIZE)
                .spawn_scoped(scope, || {
                    // execute and store output to results
                    self.execute_inner(tx, input)
                })
                .expect("Expects that thread name is not null");
            handle.join().expect("Expects for thread to not panic")
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Acquire changeset cursors
        let mut account_changeset = tx.cursor_dup_write::<tables::AccountChangeSet>()?;
        let mut storage_changeset = tx.cursor_dup_write::<tables::StorageChangeSet>()?;

        let (range, unwind_to, is_final_range) =
            input.unwind_block_range_with_threshold(self.thresholds.max_blocks.unwrap_or(u64::MAX));

        if range.is_empty() {
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }

        // get all batches for account change
        // Check if walk and walk_dup would do the same thing
        let account_changeset_batch =
            account_changeset.walk_range(range.clone())?.collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainState
        for (_, changeset) in account_changeset_batch.into_iter().rev() {
            if let Some(account_info) = changeset.info {
                tx.put::<tables::PlainAccountState>(changeset.address, account_info)?;
            } else {
                tx.delete::<tables::PlainAccountState>(changeset.address, None)?;
            }
        }

        // get all batches for storage change
        let storage_changeset_batch = storage_changeset
            .walk_range(BlockNumberAddress::range(range.clone()))?
            .collect::<Result<Vec<_>, _>>()?;

        // revert all changes to PlainStorage
        let mut plain_storage_cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;

        for (key, storage) in storage_changeset_batch.into_iter().rev() {
            let address = key.address();
            if let Some(v) = plain_storage_cursor.seek_by_key_subkey(address, storage.key)? {
                if v.key == storage.key {
                    plain_storage_cursor.delete_current()?;
                }
            }
            if storage.value != U256::ZERO {
                plain_storage_cursor.upsert(address, storage)?;
            }
        }

        // Discard unwinded changesets
        let mut rev_acc_changeset_walker = account_changeset.walk_back(None)?;
        while let Some((block_num, _)) = rev_acc_changeset_walker.next().transpose()? {
            if block_num <= unwind_to {
                break
            }
            // delete all changesets
            rev_acc_changeset_walker.delete_current()?;
        }

        let mut rev_storage_changeset_walker = storage_changeset.walk_back(None)?;
        while let Some((key, _)) = rev_storage_changeset_walker.next().transpose()? {
            if key.block_number() < *range.start() {
                break
            }
            // delete all changesets
            rev_storage_changeset_walker.delete_current()?;
        }

        // Look up the start index for the transaction range
        let first_tx_num = tx.block_body_indices(*range.start())?.first_tx_num();

        // Unwind all receipts for transactions in the block range
        tx.unwind_table_by_num::<tables::Receipts>(first_tx_num)?;
        // `unwind_table_by_num` doesn't unwind the provided key, so we need to unwind it manually
        tx.delete::<tables::Receipts>(first_tx_num, None)?;

        info!(target: "sync::stages::execution", to_block = input.unwind_to, unwind_progress = unwind_to, is_final_range, "Unwind iteration finished");
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(unwind_to) })
    }
}

/// The thresholds at which the execution stage writes state changes to the database.
///
/// If either of the thresholds (`max_blocks` and `max_changes`) are hit, then the execution stage
/// commits all pending changes to the database.
///
/// A third threshold, `max_changesets`, can be set to periodically write changesets to the
/// current database transaction, which frees up memory.
#[derive(Debug)]
pub struct ExecutionStageThresholds {
    /// The maximum number of blocks to process before the execution stage commits.
    pub max_blocks: Option<u64>,
    /// The maximum amount of state changes to keep in memory before the execution stage commits.
    pub max_changes: Option<u64>,
    /// The maximum amount of changesets to keep in memory before they are written to the pending
    /// database transaction.
    ///
    /// If this is lower than `max_changes`, then history is periodically flushed to the database
    /// transaction, which frees up memory.
    pub max_changesets: Option<u64>,
}

impl Default for ExecutionStageThresholds {
    fn default() -> Self {
        Self {
            max_blocks: Some(500_000),
            max_changes: Some(5_000_000),
            max_changesets: Some(1_000_000),
        }
    }
}

impl ExecutionStageThresholds {
    /// Check if the batch thresholds have been hit.
    #[inline]
    pub fn is_end_of_batch(&self, blocks_processed: u64, changes_processed: u64) -> bool {
        blocks_processed >= self.max_blocks.unwrap_or(u64::MAX) ||
            changes_processed >= self.max_changes.unwrap_or(u64::MAX)
    }

    /// Check if the history write threshold has been hit.
    #[inline]
    pub fn should_write_history(&self, history_changes: u64) -> bool {
        history_changes >= self.max_changesets.unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestTransaction, PREV_STAGE_ID};
    use reth_db::{
        mdbx::{test_utils::create_test_db, EnvKind, WriteMap},
        models::AccountBeforeTx,
    };
    use reth_primitives::{
        hex_literal::hex, keccak256, Account, Bytecode, ChainSpecBuilder, SealedBlock,
        StorageEntry, H160, H256, U256,
    };
    use reth_provider::insert_canonical_block;
    use reth_revm::Factory;
    use reth_rlp::Decodable;
    use std::{
        ops::{Deref, DerefMut},
        sync::Arc,
    };

    fn stage() -> ExecutionStage<Factory> {
        let factory =
            Factory::new(Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build()));
        ExecutionStage::new(
            factory,
            ExecutionStageThresholds {
                max_blocks: Some(100),
                max_changes: None,
                max_changesets: None,
            },
        )
    }

    #[tokio::test]
    async fn sanity_execution_of_block() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework
        let state_db = create_test_db::<WriteMap>(EnvKind::RW);
        let mut tx = Transaction::new(state_db.as_ref()).unwrap();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(1))),
            /// The progress of this stage the last time it was executed.
            checkpoint: None,
        };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None).unwrap();
        tx.commit().unwrap();

        // insert pre state
        // 插入pre state
        let db_tx = tx.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        db_tx
            .put::<tables::PlainAccountState>(
                acc1,
                Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) },
            )
            .unwrap();
        db_tx
            .put::<tables::PlainAccountState>(
                acc2,
                Account { nonce: 0, balance, bytecode_hash: None },
            )
            .unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        tx.commit().unwrap();

        let mut execution_stage = stage();
        let output = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();
        assert_eq!(output, ExecOutput { checkpoint: StageCheckpoint::new(1), done: true });
        let tx = tx.deref_mut();
        // check post state
        let account1 = H160(hex!("1000000000000000000000000000000000000000"));
        let account1_info =
            Account { balance: U256::ZERO, nonce: 0x00, bytecode_hash: Some(code_hash) };
        let account2 = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        let account2_info = Account {
            balance: U256::from(0x1bc16d674ece94bau128),
            nonce: 0x00,
            bytecode_hash: None,
        };
        let account3 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let account3_info = Account {
            balance: U256::from(0x3635c9adc5de996b46u128),
            nonce: 0x01,
            bytecode_hash: None,
        };

        // assert accounts
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account1),
            Ok(Some(account1_info)),
            "Post changed of a account"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account2),
            Ok(Some(account2_info)),
            "Post changed of a account"
        );
        assert_eq!(
            tx.get::<tables::PlainAccountState>(account3),
            Ok(Some(account3_info)),
            "Post changed of a account"
        );
        // assert storage
        // Get on dupsort would return only first value. This is good enough for this test.
        assert_eq!(
            tx.get::<tables::PlainStorageState>(account1),
            Ok(Some(StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(2) })),
            "Post changed of a account"
        );
    }

    #[tokio::test]
    async fn sanity_execute_unwind() {
        // TODO cleanup the setup after https://github.com/paradigmxyz/reth/issues/332
        // is merged as it has similar framework

        let state_db = create_test_db::<WriteMap>(EnvKind::RW);
        let mut tx = Transaction::new(state_db.as_ref()).unwrap();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(1))),
            /// The progress of this stage the last time it was executed.
            /// 这个stage的progress，上次执行的时候
            checkpoint: None,
        };
        let mut genesis_rlp = hex!("f901faf901f5a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa045571b40ae66ca7480791bbb2887286e4e4c4b1b298b191c889d6959023a32eda056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000808502540be400808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f90262f901f9a075c371ba45999d87f4542326910a11af515897aebce5265d3f6acd1f1161f82fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa098f2dcd87c8ae4083e7017a05456c14eea4b1db2032126e27b3b1563d57d7cc0a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba03f4e5c2ec5b2170b711d97ee755c160457bb58d8daa338e835ec02ae6860bbabb901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8798203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None).unwrap();
        tx.commit().unwrap();

        // variables
        let code = hex!("5a465a905090036002900360015500");
        let balance = U256::from(0x3635c9adc5dea00000u128);
        let code_hash = keccak256(code);
        // pre state
        let db_tx = tx.deref_mut();
        let acc1 = H160(hex!("1000000000000000000000000000000000000000"));
        let acc1_info = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) };
        let acc2 = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let acc2_info = Account { nonce: 0, balance, bytecode_hash: None };

        db_tx.put::<tables::PlainAccountState>(acc1, acc1_info).unwrap();
        db_tx.put::<tables::PlainAccountState>(acc2, acc2_info).unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        tx.commit().unwrap();

        // execute
        let mut execution_stage = stage();
        let _ = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();

        let mut stage = stage();
        let o = stage
            .unwind(
                &mut tx,
                UnwindInput { checkpoint: StageCheckpoint::new(1), unwind_to: 0, bad_block: None },
            )
            .await
            .unwrap();

        assert_eq!(o, UnwindOutput { checkpoint: StageCheckpoint::new(0) });

        // assert unwind stage
        let db_tx = tx.deref();
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(acc1),
            Ok(Some(acc1_info)),
            "Pre changed of a account"
        );
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(acc2),
            Ok(Some(acc2_info)),
            "Post changed of a account"
        );

        let miner_acc = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));
        assert_eq!(
            db_tx.get::<tables::PlainAccountState>(miner_acc),
            Ok(None),
            "Third account should be unwound"
        );

        assert_eq!(db_tx.get::<tables::Receipts>(0), Ok(None), "First receipt should be unwound");
    }

    #[tokio::test]
    async fn test_selfdestruct() {
        let test_tx = TestTransaction::default();
        let mut tx = test_tx.inner();
        let input = ExecInput {
            previous_stage: Some((PREV_STAGE_ID, StageCheckpoint::new(1))),
            /// The progress of this stage the last time it was executed.
            checkpoint: None,
        };
        let mut genesis_rlp = hex!("f901f8f901f3a00000000000000000000000000000000000000000000000000000000000000000a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0c9ceb8372c88cb461724d8d3d87e8b933f6fc5f679d4841800e662f4428ffd0da056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000080830f4240808000a00000000000000000000000000000000000000000000000000000000000000000880000000000000000c0c0").as_slice();
        let genesis = SealedBlock::decode(&mut genesis_rlp).unwrap();
        let mut block_rlp = hex!("f9025ff901f7a0c86e8cc0310ae7c531c758678ddbfd16fc51c8cef8cec650b032de9869e8b94fa01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa050554882fbbda2c2fd93fdc466db9946ea262a67f7a76cc169e714f105ab583da00967f09ef1dfed20c0eacfaa94d5cd4002eda3242ac47eae68972d07b106d192a0e3c8b47fbfc94667ef4cceb17e5cc21e3b1eebd442cebb27f07562b33836290db90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008302000001830f42408238108203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f862f860800a83061a8094095e7baea6a6c7c4c2dfeb977efac326af552d8780801ba072ed817487b84ba367d15d2f039b5fc5f087d0a8882fbdf73e8cb49357e1ce30a0403d800545b8fc544f92ce8124e2255f8c3c6af93f28243a120585d4c4c6a2a3c0").as_slice();
        let block = SealedBlock::decode(&mut block_rlp).unwrap();
        insert_canonical_block(tx.deref_mut(), genesis, None).unwrap();
        insert_canonical_block(tx.deref_mut(), block.clone(), None).unwrap();
        tx.commit().unwrap();

        // variables
        let caller_address = H160(hex!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b"));
        let destroyed_address = H160(hex!("095e7baea6a6c7c4c2dfeb977efac326af552d87"));
        let beneficiary_address = H160(hex!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba"));

        let code = hex!("73095e7baea6a6c7c4c2dfeb977efac326af552d8731ff00");
        let balance = U256::from(0x0de0b6b3a7640000u64);
        let code_hash = keccak256(code);

        // pre state
        let db_tx = tx.deref_mut();
        let caller_info = Account { nonce: 0, balance, bytecode_hash: None };
        let destroyed_info =
            Account { nonce: 0, balance: U256::ZERO, bytecode_hash: Some(code_hash) };

        // set account
        db_tx.put::<tables::PlainAccountState>(caller_address, caller_info).unwrap();
        db_tx.put::<tables::PlainAccountState>(destroyed_address, destroyed_info).unwrap();
        db_tx.put::<tables::Bytecodes>(code_hash, Bytecode::new_raw(code.to_vec().into())).unwrap();
        // set storage to check when account gets destroyed.
        db_tx
            .put::<tables::PlainStorageState>(
                destroyed_address,
                StorageEntry { key: H256::zero(), value: U256::ZERO },
            )
            .unwrap();
        db_tx
            .put::<tables::PlainStorageState>(
                destroyed_address,
                StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1u64) },
            )
            .unwrap();

        tx.commit().unwrap();

        // execute
        let mut execution_stage = stage();
        let _ = execution_stage.execute(&mut tx, input).await.unwrap();
        tx.commit().unwrap();

        // assert unwind stage
        assert_eq!(
            tx.deref().get::<tables::PlainAccountState>(destroyed_address),
            Ok(None),
            "Account was destroyed"
        );

        assert_eq!(
            tx.deref().get::<tables::PlainStorageState>(destroyed_address),
            Ok(None),
            "There is storage for destroyed account"
        );
        // drops tx so that it returns write privilege to test_tx
        drop(tx);
        let plain_accounts = test_tx.table::<tables::PlainAccountState>().unwrap();
        let plain_storage = test_tx.table::<tables::PlainStorageState>().unwrap();

        assert_eq!(
            plain_accounts,
            vec![
                (
                    beneficiary_address,
                    Account {
                        nonce: 0,
                        balance: U256::from(0x1bc16d674eca30a0u64),
                        bytecode_hash: None
                    }
                ),
                (
                    caller_address,
                    Account {
                        nonce: 1,
                        balance: U256::from(0xde0b6b3a761cf60u64),
                        bytecode_hash: None
                    }
                )
            ]
        );
        assert!(plain_storage.is_empty());

        let account_changesets = test_tx.table::<tables::AccountChangeSet>().unwrap();
        let storage_changesets = test_tx.table::<tables::StorageChangeSet>().unwrap();

        assert_eq!(
            account_changesets,
            vec![
                (
                    block.number,
                    AccountBeforeTx { address: destroyed_address, info: Some(destroyed_info) },
                ),
                (block.number, AccountBeforeTx { address: beneficiary_address, info: None }),
                (
                    block.number,
                    AccountBeforeTx { address: caller_address, info: Some(caller_info) }
                ),
            ]
        );

        assert_eq!(
            storage_changesets,
            vec![
                (
                    (block.number, destroyed_address).into(),
                    StorageEntry { key: H256::zero(), value: U256::ZERO }
                ),
                (
                    (block.number, destroyed_address).into(),
                    StorageEntry { key: H256::from_low_u64_be(1), value: U256::from(1u64) }
                )
            ]
        );
    }
}
