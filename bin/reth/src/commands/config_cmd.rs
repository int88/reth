//! CLI command to show configs.

use std::path::PathBuf;

use clap::Parser;
use eyre::{bail, WrapErr};
use reth_config::Config;

/// `reth config` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    /// 使用的配置文件的路径
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// Show the default config
    #[arg(long, verbatim_doc_comment, conflicts_with = "config")]
    default: bool,
}

impl Command {
    /// Execute `config` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let config = if self.default {
            Config::default()
        } else {
            let path = self.config.clone().unwrap_or_default();
            // confy will create the file if it doesn't exist; we don't want this
            // confy会创建文件，如果他不存在，我们不想要这样
            if !path.exists() {
                bail!("Config file does not exist: {}", path.display());
            }
            confy::load_path::<Config>(&path)
                .wrap_err_with(|| format!("Could not load config file: {}", path.display()))?
        };
        println!("{}", toml::to_string_pretty(&config)?);
        Ok(())
    }
}
