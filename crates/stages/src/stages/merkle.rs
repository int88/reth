use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_codecs::Compact;
use reth_db::{
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::consensus;
use reth_primitives::{
    hex,
    stage::{EntitiesCheckpoint, MerkleCheckpoint, StageCheckpoint, StageId},
    trie::StoredSubNode,
    BlockNumber, SealedHeader, H256,
};
use reth_provider::{
    DatabaseProviderRW, HeaderProvider, ProviderError, StageCheckpointReader, StageCheckpointWriter,
};
use reth_trie::{IntermediateStateRootState, StateRoot, StateRootProgress};
use std::fmt::Debug;
use tracing::*;

/// The merkle hashing stage uses input from
/// [`AccountHashingStage`][crate::stages::AccountHashingStage] and
/// [`StorageHashingStage`][crate::stages::AccountHashingStage] to calculate intermediate hashes
/// and state roots.
/// merkle hashing stage使用来自AccountHashingStage和StorageHashingStage的输入来计算中间哈希和状态根
///
/// This stage should be run with the above two stages, otherwise it is a no-op.
/// 这个stage应该和上面两个stage一起运行，否则它是一个空操作
///
/// This stage is split in two: one for calculating hashes and one for unwinding.
/// 这个stage分为两个部分：一个用于计算哈希，一个用于unwinding
///
/// When run in execution, it's going to be executed AFTER the hashing stages, to generate
/// the state root. When run in unwind mode, it's going to be executed BEFORE the hashing stages,
/// so that it unwinds the intermediate hashes based on the unwound hashed state from the hashing
/// stages. The order of these two variants is important. The unwind variant should be added to the
/// pipeline before the execution variant.
/// 当运行在execution模式下时，它将在hashing stages之后执行，以生成state root。
/// 当运行在unwind模式下时，它将在hashing stages之前执行，以便根据hashing stages的unwound hashed
/// state来unwind中间哈希。 这两个变体的顺序很重要。
/// unwind变体应该在execution变体之前添加到pipeline中
///
/// An example pipeline to only hash state would be:
///
/// - [`MerkleStage::Unwind`]
/// - [`AccountHashingStage`][crate::stages::AccountHashingStage]
/// - [`StorageHashingStage`][crate::stages::StorageHashingStage]
/// - [`MerkleStage::Execution`]
#[derive(Debug, Clone)]
pub enum MerkleStage {
    /// The execution portion of the merkle stage.
    /// merkle stage的execution部分
    Execution {
        /// The threshold (in number of blocks) for switching from incremental trie building
        /// of changes to whole rebuild.
        /// 阈值用于从incremental trie building到整个的rebuild
        clean_threshold: u64,
    },
    /// The unwind portion of the merkle stage.
    /// merkle stage的unwind部分
    Unwind,

    /// Able to execute and unwind. Used for tests
    /// 能够执行和unwind，主要用于测试
    #[cfg(any(test, feature = "test-utils"))]
    #[allow(missing_docs)]
    Both { clean_threshold: u64 },
}

impl MerkleStage {
    /// Stage default for the [MerkleStage::Execution].
    pub fn default_execution() -> Self {
        Self::Execution { clean_threshold: 50_000 }
    }

    /// Stage default for the [MerkleStage::Unwind].
    pub fn default_unwind() -> Self {
        Self::Unwind
    }

    /// Create new instance of [MerkleStage::Execution].
    pub fn new_execution(clean_threshold: u64) -> Self {
        Self::Execution { clean_threshold }
    }

    /// Check that the computed state root matches the root in the expected header.
    /// 检查计算的state root匹配期望的header中的root
    fn validate_state_root(
        &self,
        got: H256,
        expected: SealedHeader,
        target_block: BlockNumber,
    ) -> Result<(), StageError> {
        if got == expected.state_root {
            Ok(())
        } else {
            warn!(target: "sync::stages::merkle", ?target_block, ?got, ?expected, "Failed to verify block state root");
            Err(StageError::Validation {
                block: expected.clone(),
                error: consensus::ConsensusError::BodyStateRootDiff {
                    got,
                    expected: expected.state_root,
                },
            })
        }
    }

    /// Gets the hashing progress
    /// 获取hashing progress
    pub fn get_execution_checkpoint<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, &DB>,
    ) -> Result<Option<MerkleCheckpoint>, StageError> {
        let buf =
            provider.get_stage_checkpoint_progress(StageId::MerkleExecute)?.unwrap_or_default();

        if buf.is_empty() {
            return Ok(None)
        }

        let (checkpoint, _) = MerkleCheckpoint::from_compact(&buf, buf.len());
        Ok(Some(checkpoint))
    }

    /// Saves the hashing progress
    /// 保存hashing progress
    pub fn save_execution_checkpoint<DB: Database>(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        checkpoint: Option<MerkleCheckpoint>,
    ) -> Result<(), StageError> {
        let mut buf = vec![];
        if let Some(checkpoint) = checkpoint {
            debug!(
                target: "sync::stages::merkle::exec",
                last_account_key = ?checkpoint.last_account_key,
                last_walker_key = ?hex::encode(&checkpoint.last_walker_key),
                "Saving inner merkle checkpoint"
            );
            checkpoint.to_compact(&mut buf);
        }
        // 保存merkle checkpoint
        Ok(provider.save_stage_checkpoint_progress(StageId::MerkleExecute, buf)?)
    }
}

#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for MerkleStage {
    /// Return the id of the stage
    fn id(&self) -> StageId {
        match self {
            MerkleStage::Execution { .. } => StageId::MerkleExecute,
            MerkleStage::Unwind => StageId::MerkleUnwind,
            #[cfg(any(test, feature = "test-utils"))]
            MerkleStage::Both { .. } => StageId::Other("MerkleBoth"),
        }
    }

    /// Execute the stage.
    async fn execute(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let threshold = match self {
            // stage处于的阶段？
            MerkleStage::Unwind => {
                info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
                return Ok(ExecOutput::done(StageCheckpoint::new(input.target())))
            }
            MerkleStage::Execution { clean_threshold } => *clean_threshold,
            #[cfg(any(test, feature = "test-utils"))]
            MerkleStage::Both { clean_threshold } => *clean_threshold,
        };

        let range = input.next_block_range();
        let (from_block, to_block) = range.clone().into_inner();
        let current_block = input.target();

        let block = provider
            .header_by_number(current_block)?
            .ok_or_else(|| ProviderError::HeaderNotFound(current_block.into()))?;
        let block_root = block.state_root;

        let mut checkpoint = self.get_execution_checkpoint(provider)?;

        let (trie_root, entities_checkpoint) = if range.is_empty() {
            (block_root, input.checkpoint().entities_stage_checkpoint().unwrap_or_default())
        } else if to_block - from_block > threshold || from_block == 1 {
            // if there are more blocks than threshold it is faster to rebuild the trie
            // 如果有超过threshold的blocks，重构trie是更快的
            let mut entities_checkpoint = if let Some(checkpoint) =
                checkpoint.as_ref().filter(|c| c.target_block == to_block)
            {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block,
                    target = ?to_block,
                    last_account_key = ?checkpoint.last_account_key,
                    last_walker_key = ?hex::encode(&checkpoint.last_walker_key),
                    "Continuing inner merkle checkpoint"
                );

                input.checkpoint().entities_stage_checkpoint()
            } else {
                debug!(
                    target: "sync::stages::merkle::exec",
                    current = ?current_block,
                    target = ?to_block,
                    previous_checkpoint = ?checkpoint,
                    "Rebuilding trie"
                );
                // Reset the checkpoint and clear trie tables
                // 重置checkpoint并且清理trie tables
                checkpoint = None;
                self.save_execution_checkpoint(provider, None)?;
                // 清理Accounts Trie和Storages Trie
                provider.tx_ref().clear::<tables::AccountsTrie>()?;
                provider.tx_ref().clear::<tables::StoragesTrie>()?;

                None
            }
            .unwrap_or(EntitiesCheckpoint {
                processed: 0,
                total: (provider.tx_ref().entries::<tables::HashedAccount>()? +
                    provider.tx_ref().entries::<tables::HashedStorage>()?)
                    as u64,
            });

            let tx = provider.tx_ref();
            let progress = StateRoot::new(tx)
                .with_intermediate_state(checkpoint.map(IntermediateStateRootState::from))
                .root_with_progress()
                .map_err(|e| StageError::Fatal(Box::new(e)))?;
            match progress {
                StateRootProgress::Progress(state, hashed_entries_walked, updates) => {
                    updates.flush(tx)?;

                    let checkpoint = MerkleCheckpoint::new(
                        to_block,
                        state.last_account_key,
                        state.last_walker_key.hex_data.to_vec(),
                        state.walker_stack.into_iter().map(StoredSubNode::from).collect(),
                        state.hash_builder.into(),
                    );
                    self.save_execution_checkpoint(provider, Some(checkpoint))?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    return Ok(ExecOutput {
                        checkpoint: input
                            .checkpoint()
                            .with_entities_stage_checkpoint(entities_checkpoint),
                        done: false,
                    })
                }
                StateRootProgress::Complete(root, hashed_entries_walked, updates) => {
                    updates.flush(tx)?;

                    entities_checkpoint.processed += hashed_entries_walked as u64;

                    (root, entities_checkpoint)
                }
            }
        } else {
            debug!(target: "sync::stages::merkle::exec", current = ?current_block, target = ?to_block, "Updating trie");
            let (root, updates) =
                StateRoot::incremental_root_with_updates(provider.tx_ref(), range)
                    .map_err(|e| StageError::Fatal(Box::new(e)))?;
            updates.flush(provider.tx_ref())?;

            let total_hashed_entries = (provider.tx_ref().entries::<tables::HashedAccount>()? +
                provider.tx_ref().entries::<tables::HashedStorage>()?)
                as u64;

            let entities_checkpoint = EntitiesCheckpoint {
                // This is fine because `range` doesn't have an upper bound, so in this `else`
                // branch we're just hashing all remaining accounts and storage slots we have in the
                // database.
                processed: total_hashed_entries,
                total: total_hashed_entries,
            };

            (root, entities_checkpoint)
        };

        // Reset the checkpoint
        // 重置checkpoint
        self.save_execution_checkpoint(provider, None)?;

        // 校验state root
        self.validate_state_root(trie_root, block.seal_slow(), to_block)?;

        Ok(ExecOutput {
            checkpoint: StageCheckpoint::new(to_block)
                .with_entities_stage_checkpoint(entities_checkpoint),
            done: true,
        })
    }

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        provider: &mut DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        let tx = provider.tx_ref();
        let range = input.unwind_block_range();
        if matches!(self, MerkleStage::Execution { .. }) {
            info!(target: "sync::stages::merkle::unwind", "Stage is always skipped");
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }

        let mut entities_checkpoint =
            input.checkpoint.entities_stage_checkpoint().unwrap_or(EntitiesCheckpoint {
                processed: 0,
                total: (tx.entries::<tables::HashedAccount>()? +
                    tx.entries::<tables::HashedStorage>()?) as u64,
            });

        if input.unwind_to == 0 {
            // 如果unwind到0，则直接清空table
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;

            entities_checkpoint.processed = 0;

            return Ok(UnwindOutput {
                checkpoint: StageCheckpoint::new(input.unwind_to)
                    .with_entities_stage_checkpoint(entities_checkpoint),
            })
        }

        // Unwind trie only if there are transitions
        // 只在有transitions的时候unwind trie
        if !range.is_empty() {
            let (block_root, updates) = StateRoot::incremental_root_with_updates(tx, range)
                .map_err(|e| StageError::Fatal(Box::new(e)))?;

            // Validate the calulated state root
            // 校验计算的state root
            let target = provider
                .header_by_number(input.unwind_to)?
                .ok_or_else(|| ProviderError::HeaderNotFound(input.unwind_to.into()))?;
            self.validate_state_root(block_root, target.seal_slow(), input.unwind_to)?;

            // Validation passed, apply unwind changes to the database.
            // 校验通过，应用unwind changes到database
            updates.flush(provider.tx_ref())?;

            // TODO(alexey): update entities checkpoint
        } else {
            info!(target: "sync::stages::merkle::unwind", "Nothing to unwind");
        }

        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestTransaction, UnwindStageTestRunner,
    };
    use assert_matches::assert_matches;
    use reth_db::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_interfaces::test_utils::generators::{
        random_block, random_block_range, random_contract_account_range, random_transition_range,
    };
    use reth_primitives::{
        keccak256, stage::StageUnitCheckpoint, SealedBlock, StorageEntry, H256, U256,
    };
    use reth_trie::test_utils::{state_root, state_root_prehashed};
    use std::collections::BTreeMap;

    stage_test_suite_ext!(MerkleTestRunner, merkle);

    /// Execute from genesis so as to merkelize whole state
    /// 从genesis开始执行，为了merkelize整个state
    #[tokio::test]
    async fn execute_clean_merkle() {
        let (previous_stage, stage_progress) = (500, 0);

        // Set up the runner
        // 设置runner
        let mut runner = MerkleTestRunner::default();
        // set low threshold so we hash the whole storage
        // 设置low threshold，因此我们对整个storage进行hash
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        // 为execution提供输入
        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        // 对成功的结果进行断言
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == (
                    // 获取HashAccount和HashStorage中的条目数
                    runner.tx.table::<tables::HashedAccount>().unwrap().len() +
                    runner.tx.table::<tables::HashedStorage>().unwrap().len()
                ) as u64
        );

        // Validate the stage execution
        // 校验stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    /// Update small trie
    /// 更新small trie
    #[tokio::test]
    async fn execute_small_merkle() {
        let (previous_stage, stage_progress) = (2, 1);

        // Set up the runner
        // 设置runner
        let mut runner = MerkleTestRunner::default();
        let input = ExecInput {
            target: Some(previous_stage),
            checkpoint: Some(StageCheckpoint::new(stage_progress)),
        };

        runner.seed_execution(input).expect("failed to seed execution");

        let rx = runner.execute(input);

        // Assert the successful result
        // 校验成功的结果
        let result = rx.await.unwrap();
        assert_matches!(
            result,
            Ok(ExecOutput {
                checkpoint: StageCheckpoint {
                    block_number,
                    stage_checkpoint: Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint {
                        processed,
                        total
                    }))
                },
                done: true
            }) if block_number == previous_stage && processed == total &&
                total == (
                    runner.tx.table::<tables::HashedAccount>().unwrap().len() +
                    runner.tx.table::<tables::HashedStorage>().unwrap().len()
                ) as u64
        );

        // Validate the stage execution
        // 校验stage execution
        assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
    }

    struct MerkleTestRunner {
        tx: TestTransaction,
        clean_threshold: u64,
    }

    impl Default for MerkleTestRunner {
        fn default() -> Self {
            Self { tx: TestTransaction::default(), clean_threshold: 10000 }
        }
    }

    impl StageTestRunner for MerkleTestRunner {
        type S = MerkleStage;

        fn tx(&self) -> &TestTransaction {
            &self.tx
        }

        fn stage(&self) -> Self::S {
            Self::S::Both { clean_threshold: self.clean_threshold }
        }
    }

    #[async_trait::async_trait]
    impl ExecuteStageTestRunner for MerkleTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let stage_progress = input.checkpoint().block_number;
            let start = stage_progress + 1;
            let end = input.target();

            let num_of_accounts = 31;
            // 随机生成contract account range
            let accounts = random_contract_account_range(&mut (0..num_of_accounts))
                .into_iter()
                .collect::<BTreeMap<_, _>>();

            // 插入accounts和storages
            self.tx.insert_accounts_and_storages(
                accounts.iter().map(|(addr, acc)| (*addr, (*acc, std::iter::empty()))),
            )?;

            let SealedBlock { header, body, ommers, withdrawals } =
                // 随机的block
                random_block(stage_progress, None, Some(0), None);
            let mut header = header.unseal();

            // 构建state root
            header.state_root = state_root(
                accounts
                    .clone()
                    .into_iter()
                    .map(|(address, account)| (address, (account, std::iter::empty()))),
            );
            let sealed_head = SealedBlock { header: header.seal_slow(), body, ommers, withdrawals };

            let head_hash = sealed_head.hash();
            let mut blocks = vec![sealed_head];
            // 扩展blocks
            blocks.extend(random_block_range(start..=end, head_hash, 0..3));
            // 插入blocks
            self.tx.insert_blocks(blocks.iter(), None)?;

            let (transitions, final_state) = random_transition_range(
                blocks.iter(),
                accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
                0..3,
                0..256,
            );
            // add block changeset from block 1.
            // 添加block 1以来的changeset
            self.tx.insert_transitions(transitions, Some(start))?;
            self.tx.insert_accounts_and_storages(final_state)?;

            // Calculate state root
            // 计算state root
            let root = self.tx.query(|tx| {
                let mut accounts = BTreeMap::default();
                let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
                let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;
                for entry in accounts_cursor.walk_range(..)? {
                    let (key, account) = entry?;
                    let mut storage_entries = Vec::new();
                    let mut entry = storage_cursor.seek_exact(key)?;
                    while let Some((_, storage)) = entry {
                        storage_entries.push(storage);
                        entry = storage_cursor.next_dup()?;
                    }
                    let storage = storage_entries
                        .into_iter()
                        .filter(|v| v.value != U256::ZERO)
                        .map(|v| (v.key, v.value))
                        .collect::<Vec<_>>();
                    // 插入account的address和（account storage）
                    accounts.insert(key, (account, storage));
                }

                Ok(state_root_prehashed(accounts.into_iter()))
            })?;

            let last_block_number = end;
            self.tx.commit(|tx| {
                let mut last_header = tx.get::<tables::Headers>(last_block_number)?.unwrap();
                // 设置last header的state root
                last_header.state_root = root;
                // 重新放入header table
                tx.put::<tables::Headers>(last_block_number, last_header)
            })?;

            Ok(blocks)
        }

        fn validate_execution(
            &self,
            _input: ExecInput,
            _output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            // The execution is validated within the stage
            // execution在stage范围内执行
            Ok(())
        }
    }

    impl UnwindStageTestRunner for MerkleTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            // The unwind is validated within the stage
            // unwind在stage范围内被校验
            Ok(())
        }

        fn before_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError> {
            let target_block = input.unwind_to + 1;

            self.tx
                .commit(|tx| {
                    // storage changsets的cursor
                    let mut storage_changesets_cursor =
                        tx.cursor_dup_read::<tables::StorageChangeSet>().unwrap();
                    let mut storage_cursor =
                        tx.cursor_dup_write::<tables::HashedStorage>().unwrap();

                    let mut tree: BTreeMap<H256, BTreeMap<H256, U256>> = BTreeMap::new();

                    let mut rev_changeset_walker =
                        storage_changesets_cursor.walk_back(None).unwrap();
                    while let Some((tid_address, entry)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if tid_address.block_number() < target_block {
                            break
                        }

                        // 插入tree中
                        tree.entry(keccak256(tid_address.address()))
                            .or_default()
                            .insert(keccak256(entry.key), entry.value);
                    }
                    for (hashed_address, storage) in tree.into_iter() {
                        for (hashed_slot, value) in storage.into_iter() {
                            let storage_entry = storage_cursor
                                .seek_by_key_subkey(hashed_address, hashed_slot)
                                .unwrap();
                            if storage_entry.map(|v| v.key == hashed_slot).unwrap_or_default() {
                                storage_cursor.delete_current().unwrap();
                            }

                            if value != U256::ZERO {
                                // 构建storage entry
                                let storage_entry = StorageEntry { key: hashed_slot, value };
                                storage_cursor.upsert(hashed_address, storage_entry).unwrap();
                            }
                        }
                    }

                    // 构建account changeset cursor
                    let mut changeset_cursor =
                        tx.cursor_dup_write::<tables::AccountChangeSet>().unwrap();
                    let mut rev_changeset_walker = changeset_cursor.walk_back(None).unwrap();

                    while let Some((block_number, account_before_tx)) =
                        rev_changeset_walker.next().transpose().unwrap()
                    {
                        if block_number < target_block {
                            break
                        }

                        if let Some(acc) = account_before_tx.info {
                            // 添加hashed account
                            tx.put::<tables::HashedAccount>(
                                keccak256(account_before_tx.address),
                                acc,
                            )
                            .unwrap();
                        } else {
                            // 删除hashed account
                            tx.delete::<tables::HashedAccount>(
                                keccak256(account_before_tx.address),
                                None,
                            )
                            .unwrap();
                        }
                    }
                    Ok(())
                })
                .unwrap();
            Ok(())
        }
    }
}
