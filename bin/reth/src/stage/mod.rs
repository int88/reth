//! `reth stage` command
use clap::{Parser, Subcommand};

pub mod drop;
pub mod dump;
pub mod run;
pub mod unwind;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    #[clap(subcommand)]
    command: Subcommands,
}

/// `reth stage` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    /// Run a single stage.
    /// 运行单个stage
    ///
    /// Note that this won't use the Pipeline and as a result runs stages
    /// assuming that all the data can be held in memory. It is not recommended
    /// to run a stage for really large block ranges if your computer does not have
    /// a lot of memory to store all the data.
    /// 注意这不会使用pipeline并且作为结果，运行stages假设所有的数据都在内存中，不建议对于一个
    /// 大的block range运行stage，如果你的计算机没有足够的内存存储数据
    Run(run::Command),
    /// Drop a stage's tables from the database.
    /// 从db中丢弃一个stage的table
    Drop(drop::Command),
    /// Dumps a stage from a range into a new database.
    /// 对一个stage进行dump到一个新的数据库
    Dump(dump::Command),
    /// Unwinds a certain block range, deleting it from the database.
    /// 对特定的block range进行unwinds，从数据库中删除
    Unwind(unwind::Command),
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(self) -> eyre::Result<()> {
        match self.command {
            Subcommands::Run(command) => command.execute().await,
            Subcommands::Drop(command) => command.execute().await,
            Subcommands::Dump(command) => command.execute().await,
            Subcommands::Unwind(command) => command.execute().await,
        }
    }
}
