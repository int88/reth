//! CLI definition and entrypoint to executable
//! CLI的定义以及执行的入口

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        LogArgs,
    },
    commands::{
        config_cmd, db, debug_cmd, dump_genesis, import, init_cmd, node, node::NoArgs, p2p,
        recover, stage, test_vectors,
    },
    version::{LONG_VERSION, SHORT_VERSION},
};
use clap::{value_parser, Parser, Subcommand};
use reth_cli_runner::CliRunner;
use reth_db::DatabaseEnv;
use reth_node_builder::{InitState, WithLaunchContext};
use reth_primitives::ChainSpec;
use reth_tracing::FileWorkerGuard;
use std::{ffi::OsString, fmt, future::Future, sync::Arc};

/// Re-export of the `reth_node_core` types specifically in the `cli` module.
///
/// This is re-exported because the types in `reth_node_core::cli` originally existed in
/// `reth::cli` but were moved to the `reth_node_core` crate. This re-export avoids a breaking
/// change.
pub use crate::core::cli::*;

/// The main reth cli interface.
/// main reth的cli接口
///
/// This is the entrypoint to the executable.
/// 这是到executable的entrypoint
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli<Ext: clap::Args + fmt::Debug = NoArgs> {
    /// The command to run
    /// 运行的command
    #[command(subcommand)]
    command: Commands<Ext>,

    /// The chain this node is running.
    /// 这个node运行的chain
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    /// 可能的值是一个内置的chain或者通往特定的chain文件的路径
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

    /// Add a new instance of a node.
    /// 添加一个node的新实例
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// 配置node的端口来避免和默认值的冲突
    /// This is useful for running multiple nodes on the same machine.
    /// 这用于在同一个机器运行多个nodes
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    /// 最大的instances的数目是200，它按一种方式选择，这样让各自的端口不会冲突
    ///
    /// Changes to the following port numbers:
    /// - DISCOVERY_PORT: default + `instance` - 1
    /// - AUTH_PORT: default + `instance` * 100 - 100
    /// - HTTP_RPC_PORT: default - `instance` + 1
    /// - WS_RPC_PORT: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    instance: u16,

    #[command(flatten)]
    logs: LogArgs,
}

impl Cli {
    /// Parsers only the default CLI arguments
    /// Parsers解析默认的CLI参数
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default CLI arguments from the given iterator
    pub fn try_parse_args_from<I, T>(itr: I) -> Result<Self, clap::error::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Cli::try_parse_from(itr)
    }
}

impl<Ext: clap::Args + fmt::Debug> Cli<Ext> {
    /// Execute the configured cli command.
    /// 执行配置的cli命令
    ///
    /// This accepts a closure that is used to launch the node via the
    /// [NodeCommand](node::NodeCommand).
    /// 这接收一个closure，用于启动node，通过[NodeCommand](node::NodeCommand)
    ///
    ///
    /// # Example
    ///
    /// ```no_run
    /// // 不运行
    /// use reth::cli::Cli;
    /// use reth_node_ethereum::EthereumNode;
    ///
    /// Cli::parse_args()
    ///     .run(|builder, _| async move {
    ///         let handle = builder.launch_node(EthereumNode::default()).await?;
    ///
    ///         handle.wait_for_node_exit().await
    ///     })
    ///     .unwrap();
    /// ```
    ///
    /// # Example
    ///
    /// Parse additional CLI arguments for the node command and use it to configure the node.
    /// 解析额外的CLI命令，对于node command并且用于配置Node
    ///
    /// ```no_run
    /// use clap::Parser;
    /// use reth::cli::Cli;
    ///
    /// #[derive(Debug, Parser)]
    /// pub struct MyArgs {
    ///     pub enable: bool,
    /// }
    ///
    /// Cli::parse()
    ///     .run(|builder, my_args: MyArgs| async move {
    ///         // launch the node
    ///         // 启动node
    ///
    ///         Ok(())
    ///     })
    ///     .unwrap();
    /// ````
    pub fn run<L, Fut>(mut self, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<Arc<DatabaseEnv>, InitState>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        // add network name to logs dir
        // 添加network name到logs dir
        self.logs.log_file_directory =
            self.logs.log_file_directory.join(self.chain.chain.to_string());

        let _guard = self.init_tracing()?;

        let runner = CliRunner::default();
        match self.command {
            Commands::Node(command) => {
                runner.run_command_until_exit(|ctx| command.execute(ctx, launcher))
            }
            Commands::Init(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Import(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::DumpGenesis(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Db(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Stage(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
            Commands::P2P(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Debug(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
            Commands::Recover(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
        }
    }

    /// Initializes tracing with the configured options.
    /// 用配置的options初始化tracing
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    /// 如果使能了logging，这个函数返回一个guard，必须保持alive来确保所有的logs被刷到磁盘
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
        let guard = self.logs.init_tracing()?;
        Ok(guard)
    }
}

/// Commands to be executed
/// 执行的命令
#[derive(Debug, Subcommand)]
pub enum Commands<Ext: clap::Args + fmt::Debug = NoArgs> {
    /// Start the node
    /// 启动node
    #[command(name = "node")]
    Node(node::NodeCommand<Ext>),
    /// Initialize the database from a genesis file.
    /// 从一个genesis file初始化一个db
    #[command(name = "init")]
    Init(init_cmd::InitCommand),
    /// This syncs RLP encoded blocks from a file.
    /// 从一个文件同步RLP encoded blocks
    #[command(name = "import")]
    Import(import::ImportCommand),
    /// Dumps genesis block JSON configuration to stdout.
    DumpGenesis(dump_genesis::DumpGenesisCommand),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(stage::Command),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command),
    /// Generate Test Vectors
    #[command(name = "test-vectors")]
    TestVectors(test_vectors::Command),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config_cmd::Command),
    /// Various debug routines
    #[command(name = "debug")]
    Debug(debug_cmd::Command),
    /// Scripts for node recovery
    #[command(name = "recover")]
    Recover(recover::Command),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ColorMode;
    use clap::CommandFactory;

    #[test]
    fn parse_color_mode() {
        let reth = Cli::try_parse_args_from(["reth", "node", "--color", "always"]).unwrap();
        assert_eq!(reth.logs.color, ColorMode::Always);
    }

    /// Tests that the help message is parsed correctly. This ensures that clap args are configured
    /// correctly and no conflicts are introduced via attributes that would result in a panic at
    /// runtime
    /// 测试help message被正确解析，这确保clap
    /// args被正确配置并且没有引入冲突，通过attributes，因为这会导致运行时panic
    #[test]
    fn test_parse_help_all_subcommands() {
        let reth = Cli::<NoArgs>::command();
        for sub_command in reth.get_subcommands() {
            // 获取子命令的名字
            let err = Cli::try_parse_args_from(["reth", sub_command.get_name(), "--help"])
                .err()
                .unwrap_or_else(|| {
                    panic!("Failed to parse help message {}", sub_command.get_name())
                });

            // --help is treated as error, but
            // > Not a true "error" as it means --help or similar was used. The help message will be sent to stdout.
            // --help被作为error，但是不是一个真的"error"，因为这意味着 -- help或者乐死的被使用，help message会被发送给stdout
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        }
    }

    /// Tests that the log directory is parsed correctly. It's always tied to the specific chain's
    /// name
    /// 测试log directory被正确解析，它总是关联到特定的chain的名字
    #[test]
    fn parse_logs_path() {
        let mut reth = Cli::try_parse_args_from(["reth", "node"]).unwrap();
        reth.logs.log_file_directory =
            reth.logs.log_file_directory.join(reth.chain.chain.to_string());
        let log_dir = reth.logs.log_file_directory;
        let end = format!("reth/logs/{}", SUPPORTED_CHAINS[0]);
        // log目录是否匹配
        assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");

        let mut iter = SUPPORTED_CHAINS.iter();
        iter.next();
        for chain in iter {
            // 使用不同的chains
            let mut reth = Cli::try_parse_args_from(["reth", "node", "--chain", chain]).unwrap();
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(reth.chain.chain.to_string());
            let log_dir = reth.logs.log_file_directory;
            let end = format!("reth/logs/{chain}");
            assert!(log_dir.as_ref().ends_with(end), "{log_dir:?}");
        }
    }

    #[test]
    fn parse_env_filter_directives() {
        let temp_dir = tempfile::tempdir().unwrap();

        // 设置环境变量，配置log
        std::env::set_var("RUST_LOG", "info,evm=debug");
        let reth = Cli::try_parse_args_from([
            "reth",
            "init",
            "--datadir",
            temp_dir.path().to_str().unwrap(),
            "--log.file.filter",
            "debug,net=trace",
        ])
        .unwrap();
        assert!(reth.run(|_, _| async move { Ok(()) }).is_ok());
    }
}
