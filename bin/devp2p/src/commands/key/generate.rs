use std::path::PathBuf;

use clap::Parser;
use eyre::Ok;

/// `devp2p key generate` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// The path of the file to put new generated key.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    file: Option<PathBuf>,
}

impl Command {
    pub fn execute(&self) -> eyre::Result<()> {
        let path = &self.file.clone().unwrap_or_default();
        println!("dev key generate being called, file is {}", path.display());

        let mut file = std::fs::File::create(path)?;

        Ok(())
    }
}
