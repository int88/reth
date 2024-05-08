use clap::{Parser, Subcommand};

/// `devp2p key` command
#[derive(Debug, Parser)]
pub struct KeyCommand {
    #[command(subcommand)]
    command: Subcommands,
}

/// `devp2p key` subcommands
#[derive(Subcommand, Debug)]
pub enum Subcommands {
    Generate,
}

impl KeyCommand {
    pub fn execute(&self) -> eyre::Result<()> {
        println!("key command execute");

        match self.command {
            Subcommands::Generate => {
                println!("key generate subcommand execute");
            }
        }
        Ok(())
    }
}
