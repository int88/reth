//! Blockchain tree externals.

use reth_db::database::Database;
use reth_primitives::ChainSpec;
use reth_provider::ShareableDatabase;
use std::sync::Arc;

/// A container for external components.
/// 用于外部组件的容器
///
/// This is a simple container for external components used throughout the blockchain tree
/// implementation:
/// 这是一个简单的容器，用于整个blockchain tree实现中使用的外部组件：
///
/// - A handle to the database
/// - 数据库的handle
/// - A handle to the consensus engine
/// - 共识引擎的handle
/// - The executor factory to execute blocks with
/// - 用于执行blocks的executor factory
/// - The chain spec
/// - 链的spec
#[derive(Debug)]
pub struct TreeExternals<DB, C, EF> {
    /// The database, used to commit the canonical chain, or unwind it.
    pub(crate) db: DB,
    /// The consensus engine.
    pub(crate) consensus: C,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EF,
    /// The chain spec.
    pub(crate) chain_spec: Arc<ChainSpec>,
}

impl<DB, C, EF> TreeExternals<DB, C, EF> {
    /// Create new tree externals.
    pub fn new(db: DB, consensus: C, executor_factory: EF, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, consensus, executor_factory, chain_spec }
    }
}

impl<DB: Database, C, EF> TreeExternals<DB, C, EF> {
    /// Return shareable database helper structure.
    pub fn database(&self) -> ShareableDatabase<&DB> {
        ShareableDatabase::new(&self.db, self.chain_spec.clone())
    }
}
