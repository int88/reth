use crate::dirs::{DataDirPath, MaybePlatformPath};
use clap::Parser;
use reth_primitives::ChainSpec;
use reth_staged_sync::utils::{
    chainspec::genesis_value_parser,
    init::{init_db, init_genesis},
};
use std::sync::Arc;
use tracing::info;

/// Initializes the database with the genesis block.
/// 用genesis block初始化数据库
#[derive(Debug, Parser)]
pub struct InitCommand {
    /// The path to the data dir for all reth files and subdirectories.
    /// 到data dir的路径，用于所有的reth文件和子目录
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    /// 这个node正在运行的chain
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    /// 可能的值是内置的chain或者chain specification文件的路径
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,
}

impl InitCommand {
    /// Execute the `init` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth init starting");

        // add network name to data dir
        // 添加网络名称到data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path)?);
        info!(target: "reth::cli", "Database opened");

        info!(target: "reth::cli", "Writing genesis block");
        let hash = init_genesis(db, self.chain)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
