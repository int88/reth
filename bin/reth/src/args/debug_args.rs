//! clap [Args](clap::Args) for debugging purposes

use clap::Args;
use reth_primitives::{TxHash, H256};

/// Parameters for debugging purposes
/// 用于调试的参数
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Rpc")]
pub struct DebugArgs {
    /// Prompt the downloader to download blocks one at a time.
    /// 对于每个块，提示下载器下载一个块。
    ///
    /// NOTE: This is for testing purposes only.
    /// 注意：这仅用于测试。
    #[arg(long = "debug.continuous", help_heading = "Debug", conflicts_with = "tip")]
    pub continuous: bool,

    /// Flag indicating whether the node should be terminated after the pipeline sync.
    #[arg(long = "debug.terminate", help_heading = "Debug")]
    pub terminate: bool,

    /// Set the chain tip manually for testing purposes.
    /// 手动设置链的tip，用于测试
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug", conflicts_with = "continuous")]
    pub tip: Option<H256>,

    /// Runs the sync only up to the specified block.
    /// 运行sync，只到指定的块。
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    pub max_block: Option<u64>,

    /// Print opcode level traces directly to console during execution.
    #[arg(long = "debug.print-inspector", help_heading = "Debug")]
    pub print_inspector: bool,

    /// Hook on a specific block during execution.
    #[arg(
        long = "debug.hook-block",
        help_heading = "Debug",
        conflicts_with = "hook_transaction",
        conflicts_with = "hook_all"
    )]
    pub hook_block: Option<u64>,

    /// Hook on a specific transaction during execution.
    #[arg(
        long = "debug.hook-transaction",
        help_heading = "Debug",
        conflicts_with = "hook_block",
        conflicts_with = "hook_all"
    )]
    pub hook_transaction: Option<TxHash>,

    /// Hook on every transaction in a block.
    #[arg(
        long = "debug.hook-all",
        help_heading = "Debug",
        conflicts_with = "hook_block",
        conflicts_with = "hook_transaction"
    )]
    pub hook_all: bool,
}
