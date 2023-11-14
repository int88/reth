use reth_db::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::DatabaseGAT,
    models::{AccountBeforeTx, StoredBlockBodyIndices},
    table::{Table, TableRow},
    tables,
    test_utils::{create_test_rw_db, create_test_rw_db_with_path},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
    DatabaseEnv, DatabaseError as DbError,
};
use reth_interfaces::{test_utils::generators::ChangeSet, RethResult};
use reth_primitives::{
    keccak256, Account, Address, BlockNumber, Receipt, SealedBlock, SealedHeader, StorageEntry,
    TxHash, TxNumber, H256, MAINNET, U256,
};
use reth_provider::{DatabaseProviderRO, DatabaseProviderRW, HistoryWriter, ProviderFactory};
use std::{
    borrow::Borrow,
    collections::BTreeMap,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The [TestTransaction] is used as an internal
/// database for testing stage implementation.
///  [TestTransaction]用于作为一个内部的db，用于测试stage的实现
///
/// ```rust,ignore
/// let tx = TestTransaction::default();
/// 执行stage
/// stage.execute(&mut tx.container(), input);
/// ```
#[derive(Debug)]
pub struct TestTransaction {
    /// DB
    pub tx: Arc<DatabaseEnv>,
    pub path: Option<PathBuf>,
    pub factory: ProviderFactory<Arc<DatabaseEnv>>,
}

impl Default for TestTransaction {
    /// Create a new instance of [TestTransaction]
    /// 创建一个新的[TestTransaction]的实例
    fn default() -> Self {
        // 创建rw db
        let tx = create_test_rw_db();
        // 构建factory
        Self { tx: tx.clone(), path: None, factory: ProviderFactory::new(tx, MAINNET.clone()) }
    }
}

impl TestTransaction {
    pub fn new(path: &Path) -> Self {
        let tx = create_test_rw_db_with_path(path);
        Self {
            tx: tx.clone(),
            path: Some(path.to_path_buf()),
            factory: ProviderFactory::new(tx, MAINNET.clone()),
        }
    }

    /// Return a database wrapped in [DatabaseProviderRW].
    pub fn inner_rw(&self) -> DatabaseProviderRW<'_, Arc<DatabaseEnv>> {
        self.factory.provider_rw().expect("failed to create db container")
    }

    /// Return a database wrapped in [DatabaseProviderRO].
    pub fn inner(&self) -> DatabaseProviderRO<'_, Arc<DatabaseEnv>> {
        self.factory.provider().expect("failed to create db container")
    }

    /// Get a pointer to an internal database.
    pub fn inner_raw(&self) -> Arc<DatabaseEnv> {
        self.tx.clone()
    }

    /// Invoke a callback with transaction committing it afterwards
    /// 调用一个callback，提交tx，之后
    pub fn commit<F>(&self, f: F) -> Result<(), DbError>
    where
        F: FnOnce(&<DatabaseEnv as DatabaseGAT<'_>>::TXMut) -> Result<(), DbError>,
    {
        let mut tx = self.inner_rw();
        f(tx.tx_ref())?;
        // 提交tx
        tx.commit().expect("failed to commit");
        Ok(())
    }

    /// Invoke a callback with a read transaction
    pub fn query<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&<DatabaseEnv as DatabaseGAT<'_>>::TX) -> Result<R, DbError>,
    {
        f(self.inner().tx_ref())
    }

    /// Check if the table is empty
    pub fn table_is_empty<T: Table>(&self) -> Result<bool, DbError> {
        self.query(|tx| {
            let last = tx.cursor_read::<T>()?.last()?;
            Ok(last.is_none())
        })
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DbError>
    where
        T::Key: Default + Ord,
    {
        self.query(|tx| {
            tx.cursor_read::<T>()?
                .walk(Some(T::Key::default()))?
                .collect::<Result<Vec<_>, DbError>>()
        })
    }

    /// Map a collection of values and store them in the database.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust,ignore
    /// let tx = TestTransaction::default();
    /// tx.map_put::<Table, _, _>(&items, |item| item)?;
    /// ```
    #[allow(dead_code)]
    pub fn map_put<T, S, F>(&self, values: &[S], mut map: F) -> Result<(), DbError>
    where
        T: Table,
        S: Clone,
        F: FnMut(&S) -> TableRow<T>,
    {
        self.commit(|tx| {
            values.iter().try_for_each(|src| {
                let (k, v) = map(src);
                tx.put::<T>(k, v)
            })
        })
    }

    /// Transform a collection of values using a callback and store
    /// them in the database. The callback additionally accepts the
    /// optional last element that was stored.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust,ignore
    /// let tx = TestTransaction::default();
    /// tx.transform_append::<Table, _, _>(&items, |prev, item| prev.unwrap_or_default() + item)?;
    /// ```
    #[allow(dead_code)]
    pub fn transform_append<T, S, F>(&self, values: &[S], mut transform: F) -> Result<(), DbError>
    where
        T: Table,
        <T as Table>::Value: Clone,
        S: Clone,
        F: FnMut(&Option<<T as Table>::Value>, &S) -> TableRow<T>,
    {
        self.commit(|tx| {
            let mut cursor = tx.cursor_write::<T>()?;
            let mut last = cursor.last()?.map(|(_, v)| v);
            values.iter().try_for_each(|src| {
                let (k, v) = transform(&last, src);
                last = Some(v.clone());
                cursor.append(k, v)
            })
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Key]
    pub fn ensure_no_entry_above<T, F>(&self, num: u64, mut selector: F) -> Result<(), DbError>
    where
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor_read::<T>()?;
            if let Some((key, _)) = cursor.last()? {
                assert!(selector(key) <= num);
            }
            Ok(())
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Value]
    pub fn ensure_no_entry_above_by_value<T, F>(
        &self,
        num: u64,
        mut selector: F,
    ) -> Result<(), DbError>
    where
        T: Table,
        F: FnMut(T::Value) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor_read::<T>()?;
            let mut rev_walker = cursor.walk_back(None)?;
            while let Some((_, value)) = rev_walker.next().transpose()? {
                assert!(selector(value) <= num);
            }
            Ok(())
        })
    }

    /// Inserts a single [SealedHeader] into the corresponding tables of the headers stage.
    /// 插入一个[SealedHeader]到headers stage对应的tables
    fn insert_header<'a, TX: DbTxMut<'a> + DbTx<'a>>(
        tx: &'a TX,
        header: &SealedHeader,
    ) -> Result<(), DbError> {
        // 插入canonical headers
        tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
        // 插入header numbers
        tx.put::<tables::HeaderNumbers>(header.hash(), header.number)?;
        // 插入headers
        tx.put::<tables::Headers>(header.number, header.clone().unseal())
    }

    /// Insert ordered collection of [SealedHeader] into the corresponding tables
    /// that are supposed to be populated by the headers stage.
    /// 插入一系列有序的[SealedHeader]到对应的tagbles，从而能被headers stage填充
    pub fn insert_headers<'a, I>(&self, headers: I) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| headers.into_iter().try_for_each(|header| Self::insert_header(tx, header)))
    }

    /// Inserts total difficulty of headers into the corresponding tables.
    /// 插入headers的td到对应的tables
    ///
    /// Superset functionality of [TestTransaction::insert_headers].
    /// 支持[TestTransaction::insert_headers]功能的超集
    pub(crate) fn insert_headers_with_td<'a, I>(&self, headers: I) -> Result<(), DbError>
    where
        // 一个header的iterator
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| {
            let mut td = U256::ZERO;
            headers.into_iter().try_for_each(|header| {
                //插入header
                Self::insert_header(tx, header)?;
                td += header.difficulty;
                // 写入HeaderTD的table
                tx.put::<tables::HeaderTD>(header.number, td.into())
            })
        })
    }

    /// Insert ordered collection of [SealedBlock] into corresponding tables.
    /// Superset functionality of [TestTransaction::insert_headers].
    ///
    /// Assumes that there's a single transition for each transaction (i.e. no block rewards).
    pub fn insert_blocks<'a, I>(&self, blocks: I, tx_offset: Option<u64>) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedBlock>,
    {
        self.commit(|tx| {
            let mut next_tx_num = tx_offset.unwrap_or_default();

            blocks.into_iter().try_for_each(|block| {
                Self::insert_header(tx, &block.header)?;
                // Insert into body tables.
                let block_body_indices = StoredBlockBodyIndices {
                    first_tx_num: next_tx_num,
                    tx_count: block.body.len() as u64,
                };

                if !block.body.is_empty() {
                    tx.put::<tables::TransactionBlock>(
                        block_body_indices.last_tx_num(),
                        block.number,
                    )?;
                }
                tx.put::<tables::BlockBodyIndices>(block.number, block_body_indices)?;

                block.body.iter().try_for_each(|body_tx| {
                    tx.put::<tables::Transactions>(next_tx_num, body_tx.clone().into())?;
                    next_tx_num += 1;
                    Ok(())
                })
            })
        })
    }

    pub fn insert_tx_hash_numbers<I>(&self, tx_hash_numbers: I) -> Result<(), DbError>
    where
        I: IntoIterator<Item = (TxHash, TxNumber)>,
    {
        self.commit(|tx| {
            tx_hash_numbers.into_iter().try_for_each(|(tx_hash, tx_num)| {
                // Insert into tx hash numbers table.
                tx.put::<tables::TxHashNumber>(tx_hash, tx_num)
            })
        })
    }

    /// Insert collection of ([TxNumber], [Receipt]) into the corresponding table.
    pub fn insert_receipts<I>(&self, receipts: I) -> Result<(), DbError>
    where
        I: IntoIterator<Item = (TxNumber, Receipt)>,
    {
        self.commit(|tx| {
            receipts.into_iter().try_for_each(|(tx_num, receipt)| {
                // Insert into receipts table.
                tx.put::<tables::Receipts>(tx_num, receipt)
            })
        })
    }

    pub fn insert_transaction_senders<I>(&self, transaction_senders: I) -> Result<(), DbError>
    where
        I: IntoIterator<Item = (TxNumber, Address)>,
    {
        self.commit(|tx| {
            transaction_senders.into_iter().try_for_each(|(tx_num, sender)| {
                // Insert into receipts table.
                tx.put::<tables::TxSenders>(tx_num, sender)
            })
        })
    }

    /// Insert collection of ([Address], [Account]) into corresponding tables.
    pub fn insert_accounts_and_storages<I, S>(&self, accounts: I) -> Result<(), DbError>
    where
        I: IntoIterator<Item = (Address, (Account, S))>,
        S: IntoIterator<Item = StorageEntry>,
    {
        self.commit(|tx| {
            accounts.into_iter().try_for_each(|(address, (account, storage))| {
                let hashed_address = keccak256(address);

                // Insert into account tables.
                tx.put::<tables::PlainAccountState>(address, account)?;
                tx.put::<tables::HashedAccount>(hashed_address, account)?;

                // Insert into storage tables.
                storage.into_iter().filter(|e| e.value != U256::ZERO).try_for_each(|entry| {
                    let hashed_entry = StorageEntry { key: keccak256(entry.key), ..entry };

                    let mut cursor = tx.cursor_dup_write::<tables::PlainStorageState>()?;
                    if let Some(e) = cursor
                        .seek_by_key_subkey(address, entry.key)?
                        .filter(|e| e.key == entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(address, entry)?;

                    let mut cursor = tx.cursor_dup_write::<tables::HashedStorage>()?;
                    if let Some(e) = cursor
                        .seek_by_key_subkey(hashed_address, hashed_entry.key)?
                        .filter(|e| e.key == hashed_entry.key)
                    {
                        cursor.delete_current()?;
                    }
                    cursor.upsert(hashed_address, hashed_entry)?;

                    Ok(())
                })
            })
        })
    }

    /// Insert collection of [ChangeSet] into corresponding tables.
    pub fn insert_changesets<I>(
        &self,
        changesets: I,
        block_offset: Option<u64>,
    ) -> Result<(), DbError>
    where
        I: IntoIterator<Item = ChangeSet>,
    {
        let offset = block_offset.unwrap_or_default();
        self.commit(|tx| {
            changesets.into_iter().enumerate().try_for_each(|(block, changeset)| {
                changeset.into_iter().try_for_each(|(address, old_account, old_storage)| {
                    let block = offset + block as u64;
                    // Insert into account changeset.
                    tx.put::<tables::AccountChangeSet>(
                        block,
                        AccountBeforeTx { address, info: Some(old_account) },
                    )?;

                    let block_address = (block, address).into();

                    // Insert into storage changeset.
                    old_storage.into_iter().try_for_each(|entry| {
                        tx.put::<tables::StorageChangeSet>(block_address, entry)
                    })
                })
            })
        })
    }

    pub fn insert_history<I>(&self, changesets: I, block_offset: Option<u64>) -> RethResult<()>
    where
        I: IntoIterator<Item = ChangeSet>,
    {
        let mut accounts = BTreeMap::<Address, Vec<u64>>::new();
        let mut storages = BTreeMap::<(Address, H256), Vec<u64>>::new();

        for (block, changeset) in changesets.into_iter().enumerate() {
            for (address, _, storage_entries) in changeset {
                accounts.entry(address).or_default().push(block as u64);
                for storage_entry in storage_entries {
                    storages.entry((address, storage_entry.key)).or_default().push(block as u64);
                }
            }
        }

        let provider = self.factory.provider_rw()?;
        provider.insert_account_history_index(accounts)?;
        provider.insert_storage_history_index(storages)?;
        provider.commit()?;

        Ok(())
    }
}
