#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This crate contains a collection of traits and trait implementations for common database
//! operations.
//! 这个crate包含一系列的traits和trait实现，用于常见的数据库操作

/// Various provider traits.
/// 各种provider traits
mod traits;
pub use traits::{
    AccountProvider, BlockExecutor, BlockHashProvider, BlockIdProvider, BlockNumProvider,
    BlockProvider, BlockProviderIdExt, BlockSource, BlockchainTreePendingStateProvider,
    CanonChainTracker, CanonStateNotification, CanonStateNotificationSender,
    CanonStateNotifications, CanonStateSubscriptions, EvmEnvProvider, ExecutorFactory,
    HeaderProvider, PostStateDataProvider, ReceiptProvider, ReceiptProviderIdExt,
    StageCheckpointProvider, StateProvider, StateProviderBox, StateProviderFactory,
    StateRootProvider, TransactionsProvider, WithdrawalsProvider,
};

/// Provider trait implementations.
/// Provider trait的实现
pub mod providers;
pub use providers::{
    HistoricalStateProvider, HistoricalStateProviderRef, LatestStateProvider,
    LatestStateProviderRef, ShareableDatabase,
};

/// Execution result
/// 执行结果
pub mod post_state;
pub use post_state::PostState;

/// Helper types for interacting with the database
/// 用于和database交互的helper types
mod transaction;
pub use transaction::{Transaction, TransactionError};

/// Common database utilities.
/// 公共的database工具
mod utils;
pub use utils::{insert_block, insert_canonical_block};

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers for mocking the Provider.
/// 公共的测试帮助函数，用于mock Provider
pub mod test_utils;

/// Re-export provider error.
pub use reth_interfaces::provider::ProviderError;

pub mod chain;
pub use chain::Chain;
