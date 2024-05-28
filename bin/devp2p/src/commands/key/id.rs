use std::path::PathBuf;

use reth_fs_util as fs;

use clap::Parser;
use eyre::Ok;

// Create a node ID from a node key file
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
