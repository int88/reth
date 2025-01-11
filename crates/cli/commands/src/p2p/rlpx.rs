//! RLPx subcommand of P2P Debugging tool.

use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{HelloMessage, UnauthedP2PStream};
use reth_network::config::rng_secret_key;
use reth_network_peers::{pk2id, AnyNode};
use secp256k1::SECP256K1;
use tokio::{
    net::TcpStream,
    time::{timeout, Duration},
};

use crate::p2p;

/// RLPx commands
#[derive(Parser, Debug)]
pub struct Command {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

impl Command {
    // Execute `p2p rlpx` command.
    pub async fn execute(self) -> eyre::Result<()> {
        match self.subcommand {
            Subcommands::Ping { node } => {
                let key = rng_secret_key();
                let node_record = node
                    .node_record()
                    .ok_or_else(|| eyre::eyre!("failed to parse node {}", node))?;
                let outgoing =
                    TcpStream::connect((node_record.address, node_record.tcp_port)).await?;
                let ecies_stream = ECIESStream::connect(outgoing, key, node_record.id).await?;

                let peer_id = pk2id(&key.public_key(SECP256K1));
                let hello = HelloMessage::builder(peer_id).build();
                println!("Before calling handshake");

                let (mut p2p_stream, their_hello) =
                    UnauthedP2PStream::new(ecies_stream).handshake(hello).await?;

                println!("{:#?}", their_hello);

                let mut rx = p2p_stream.subscribe_pong();

                p2p_stream.send_ping();
                // println!("Don't SEND ANYTHING");
                p2p_stream.flush().await?;

                while let Some(_) = p2p_stream.next().await {
                    match timeout(Duration::from_secs(1), &mut rx).await {
                        Ok(Ok(())) => {
                            println!("Successfully ping");
                            break;
                        } // 成功接收到消息
                        Ok(Err(_)) => {
                            println!("Sender dropped");
                        }
                        Err(_) => {
                            println!("Timeout");
                        } // 超时
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// ping node
    Ping {
        /// The node to ping.
        node: AnyNode,
    },
}
