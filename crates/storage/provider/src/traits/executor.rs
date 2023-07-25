//! Executor Factory

use crate::{post_state::PostState, StateProvider};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, ChainSpec, U256};

/// Executor factory that would create the EVM with particular state provider.
/// 能够用特定的state provider创建EVM的Executor factory
///
/// It can be used to mock executor.
/// 它可以用于mock executor
pub trait ExecutorFactory: Send + Sync + 'static {
    /// The executor produced by the factory
    /// factory生成的execturo
    type Executor<T: StateProvider>: BlockExecutor<T>;

    /// Executor with [`StateProvider`]
    /// 有着[`StateProvider`]的Executor
    fn with_sp<SP: StateProvider>(&self, sp: SP) -> Self::Executor<SP>;

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec;
}

/// An executor capable of executing a block.
pub trait BlockExecutor<SP: StateProvider> {
    /// Execute a block.
    ///
    /// The number of `senders` should be equal to the number of transactions in the block.
    ///
    /// If no senders are specified, the `execute` function MUST recover the senders for the
    /// provided block's transactions internally. We use this to allow for calculating senders in
    /// parallel in e.g. staged sync, so that execution can happen without paying for sender
    /// recovery costs.
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError>;

    /// Executes the block and checks receipts
    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError>;
}
