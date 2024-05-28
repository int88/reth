use std::path::PathBuf;

use clap::Parser;
use eyre::Ok;

// `devp2p key to-id` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// The path of the file to load key.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    file: PathBuf,
}

impl Command {
    pub fn execute(&self) -> eyre::Result<()> {
        println!("to-id command being called");

        Ok(())
    }
}
