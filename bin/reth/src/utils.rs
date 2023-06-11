//! Common CLI utility functions.

use eyre::{Result, WrapErr};
use reth_db::{
    cursor::{DbCursorRO, Walker},
    database::Database,
    table::Table,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{BlockHashOrNumber, HeadersDirection, SealedHeader};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tracing::info;

/// Get a single header from network
/// 从network中获取一个header
pub async fn get_single_header<Client>(
    client: Client,
    id: BlockHashOrNumber,
) -> eyre::Result<SealedHeader>
where
    Client: HeadersClient,
{
    let request = HeadersRequest { direction: HeadersDirection::Rising, limit: 1, start: id };

    let (peer_id, response) =
        // 获取header
        client.get_headers_with_priority(request, Priority::High).await?.split();

    if response.len() != 1 {
        client.report_bad_message(peer_id);
        eyre::bail!("Invalid number of headers received. Expected: 1. Received: {}", response.len())
    }

    let header = response.into_iter().next().unwrap().seal_slow();

    let valid = match id {
        BlockHashOrNumber::Hash(hash) => header.hash() == hash,
        BlockHashOrNumber::Number(number) => header.number == number,
    };

    if !valid {
        client.report_bad_message(peer_id);
        eyre::bail!(
            "Received invalid header. Received: {:?}. Expected: {:?}",
            header.num_hash(),
            id
        );
    }

    Ok(header)
}

/// Wrapper over DB that implements many useful DB queries.
pub struct DbTool<'a, DB: Database> {
    pub(crate) db: &'a DB,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    pub(crate) fn new(db: &'a DB) -> eyre::Result<Self> {
        Ok(Self { db })
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [`HashMap`][std::collections::HashMap].
    pub fn list<T: Table>(
        &mut self,
        start: usize,
        len: usize,
    ) -> Result<BTreeMap<T::Key, T::Value>> {
        let data = self.db.view(|tx| {
            let mut cursor = tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

            // TODO: Upstream this in the DB trait.
            let start_walker = cursor.current().transpose();
            let walker = Walker::new(&mut cursor, start_walker);

            walker.skip(start).take(len).collect::<Vec<_>>()
        })?;

        data.into_iter()
            .collect::<Result<BTreeMap<T::Key, T::Value>, _>>()
            .map_err(|e| eyre::eyre!(e))
    }

    /// Grabs the content of the table for the given key
    pub fn get<T: Table>(&mut self, key: T::Key) -> Result<Option<T::Value>> {
        self.db.view(|tx| tx.get::<T>(key))?.map_err(|e| eyre::eyre!(e))
    }

    /// Drops the database at the given path.
    pub fn drop(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        info!(target: "reth::cli", "Dropping db at {:?}", path);
        std::fs::remove_dir_all(path).wrap_err("Dropping the database failed")?;
        Ok(())
    }

    /// Drops the provided table from the database.
    pub fn drop_table<T: Table>(&mut self) -> Result<()> {
        self.db.update(|tx| tx.clear::<T>())??;
        Ok(())
    }
}

/// Helper to parse a [Duration] from seconds
pub fn parse_duration_from_secs(arg: &str) -> Result<Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(Duration::from_secs(seconds))
}
