#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Implementation of the [BlockchainTree]
//! [BlockchainTree]的实现

/// Execution result types.
/// Execution的结果类型
pub use reth_provider::post_state;

pub mod blockchain_tree;
pub use blockchain_tree::{BlockHashes, BlockchainTree};

pub mod block_indices;
pub use block_indices::BlockIndices;

pub mod chain;
pub use chain::AppendableChain;

pub mod config;
pub use config::BlockchainTreeConfig;

pub mod externals;
pub use externals::TreeExternals;

pub mod shareable;
pub use shareable::ShareableBlockchainTree;

pub mod post_state_data;
pub use post_state_data::{PostStateData, PostStateDataRef};

/// Buffer of not executed blocks.
pub mod block_buffer;
mod canonical_chain;

pub use block_buffer::BlockBuffer;
