use std::path::PathBuf;

use reth_fs_util as fs;

use clap::Parser;
use eyre::Ok;
use secp256k1::{SecretKey, SECP256K1};

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
        let contents = fs::read_to_string(self.file)?;

        let key = contents.as_str().parse::<SecretKey>()?;

        let id = pk2id(key.public_key(SECP256K1));

        println!("{}", id);

        Ok(())
    }
}
