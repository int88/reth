//! Blockchain tree externals.

use reth_db::database::Database;
use reth_primitives::ChainSpec;
use reth_provider::ProviderFactory;
use std::sync::Arc;

/// A container for external components.
/// 用于外部的components的容器
///
/// This is a simple container for external components used throughout the blockchain tree
/// implementation:
/// 这是一个简单的container用于访问外部组件，在blockchain tree的实现中：
///
/// - A handle to the database
/// - 对于db的一个handle
/// - A handle to the consensus engine
/// - 对于consensus engine的一个handle
/// - The executor factory to execute blocks with
/// - 用于执行blocks的executor factory
/// - The chain spec
/// - chain spec
#[derive(Debug)]
pub struct TreeExternals<DB, C, EF> {
    /// The database, used to commit the canonical chain, or unwind it.
    /// db，用于提交canonical chain或者unwind
    pub(crate) db: DB,
    /// The consensus engine.
    pub(crate) consensus: C,
    /// The executor factory to execute blocks with.
    /// 用于执行blocks的executor factory
    pub(crate) executor_factory: EF,
    /// The chain spec.
    /// chain spec
    pub(crate) chain_spec: Arc<ChainSpec>,
}

impl<DB, C, EF> TreeExternals<DB, C, EF> {
    /// Create new tree externals.
    /// 创建新的tree externals
    pub fn new(db: DB, consensus: C, executor_factory: EF, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, consensus, executor_factory, chain_spec }
    }
}

impl<DB: Database, C, EF> TreeExternals<DB, C, EF> {
    /// Return shareable database helper structure.
    /// 返回共享的db helper结构
    pub fn database(&self) -> ProviderFactory<&DB> {
        ProviderFactory::new(&self.db, self.chain_spec.clone())
    }
}
