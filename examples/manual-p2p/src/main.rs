//! Low level example of connecting to and communicating with a peer.
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p manual-p2p
//! ```

use std::time::Duration;

use futures::StreamExt;
use once_cell::sync::Lazy;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    EthMessage, EthStream, HelloMessage, P2PStream, Status, UnauthedEthStream, UnauthedP2PStream,
};
use reth_network::config::rng_secret_key;
use reth_primitives::{
    mainnet_nodes, pk2id, Chain, Hardfork, Head, NodeRecord, MAINNET, MAINNET_GENESIS_HASH,
};
use secp256k1::{SecretKey, SECP256K1};
use tokio::net::TcpStream;

type AuthedP2PStream = P2PStream<ECIESStream<TcpStream>>;
type AuthedEthStream = EthStream<P2PStream<ECIESStream<TcpStream>>>;

pub static MAINNET_BOOT_NODES: Lazy<Vec<NodeRecord>> = Lazy::new(mainnet_nodes);

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Setup configs related to this 'node' by creating a new random
    // 设置和这个'node'关联的configs，通过创建一个随机数
    let our_key = rng_secret_key();
    println!("KEY is {}", our_key.display_secret());
    let our_enr = NodeRecord::from_secret_key(DEFAULT_DISCOVERY_ADDRESS, &our_key);

    // Setup discovery v4 protocol to find peers to talk to
    // 设置discovery v4协议，来找到我们能交互的peers
    let mut discv4_cfg = Discv4ConfigBuilder::default();
    discv4_cfg.add_boot_nodes(MAINNET_BOOT_NODES.clone()).lookup_interval(Duration::from_secs(1));

    // Start discovery protocol
    // 开始discovery协议
    let discv4 = Discv4::spawn(our_enr.udp_addr(), our_enr, our_key, discv4_cfg.build()).await?;
    let mut discv4_stream = discv4.update_stream().await?;

    while let Some(update) = discv4_stream.next().await {
        tokio::spawn(async move {
            if let DiscoveryUpdate::Added(peer) = update {
                // Boot nodes hard at work, lets not disturb them
                // Boot nodes在努力工作，不要打扰他们
                if MAINNET_BOOT_NODES.contains(&peer) {
                    return
                }

                // p2p握手
                let (p2p_stream, their_hello) = match handshake_p2p(peer, our_key).await {
                    Ok(s) => s,
                    Err(e) => {
                        println!("Failed P2P handshake with peer {}, {}", peer.address, e);
                        return
                    }
                };

                // eth握手
                let (eth_stream, their_status) = match handshake_eth(p2p_stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        println!("Failed ETH handshake with peer {}, {}", peer.address, e);
                        return
                    }
                };

                println!(
                    "Successfully connected to a peer at {}:{} ({}) using eth-wire version eth/{}",
                    peer.address, peer.tcp_port, their_hello.client_version, their_status.version
                );

                snoop(peer, eth_stream).await;
            }
        });
    }

    Ok(())
}

// Perform a P2P handshake with a peer
// 和peer执行p2p握手
async fn handshake_p2p(
    peer: NodeRecord,
    key: SecretKey,
) -> eyre::Result<(AuthedP2PStream, HelloMessage)> {
    // 建立outgoing连接
    let outgoing = TcpStream::connect((peer.address, peer.tcp_port)).await?;
    // 构建ECIES stream
    let ecies_stream = ECIESStream::connect(outgoing, key, peer.id).await?;

    // 后去peer id
    let our_peer_id = pk2id(&key.public_key(SECP256K1));
    // 构建hello message
    let our_hello = HelloMessage::builder(our_peer_id).build();

    Ok(UnauthedP2PStream::new(ecies_stream).handshake(our_hello).await?)
}

// Perform a ETH Wire handshake with a peer
// 和peer执行一个ETH Wire握手
async fn handshake_eth(p2p_stream: AuthedP2PStream) -> eyre::Result<(AuthedEthStream, Status)> {
    let fork_filter = MAINNET.fork_filter(Head {
        timestamp: MAINNET.fork(Hardfork::Shanghai).as_timestamp().unwrap(),
        ..Default::default()
    });

    // 构建status
    let status = Status::builder()
        .chain(Chain::mainnet())
        .genesis(MAINNET_GENESIS_HASH)
        .forkid(MAINNET.hardfork_fork_id(Hardfork::Shanghai).unwrap())
        .build();

    let status = Status { version: p2p_stream.shared_capabilities().eth()?.version(), ..status };
    // 构建stream
    let eth_unauthed = UnauthedEthStream::new(p2p_stream);
    Ok(eth_unauthed.handshake(status, fork_filter).await?)
}

// Snoop by greedily capturing all broadcasts that the peer emits
// Snoop贪婪地抓取peer发出的所有广播
// note: this node cannot handle request so will be disconnected by peer when challenged
// 注意：这个node不会处理请求，因此当被挑战时，会被断开连接
async fn snoop(peer: NodeRecord, mut eth_stream: AuthedEthStream) {
    while let Some(Ok(update)) = eth_stream.next().await {
        match update {
            EthMessage::NewPooledTransactionHashes66(txs) => {
                println!("Got {} new tx hashes from peer {}", txs.0.len(), peer.address);
            }
            EthMessage::NewBlock(block) => {
                println!("Got new block data {:?} from peer {}", block, peer.address);
            }
            EthMessage::NewPooledTransactionHashes68(txs) => {
                println!("Got {} new tx hashes from peer {}", txs.hashes.len(), peer.address);
            }
            EthMessage::NewBlockHashes(block_hashes) => {
                println!(
                    "Got {} new block hashes from peer {}",
                    block_hashes.0.len(),
                    peer.address
                );
            }
            EthMessage::GetNodeData(_) => {
                println!("Unable to serve GetNodeData request to peer {}", peer.address);
            }
            EthMessage::GetReceipts(_) => {
                println!("Unable to serve GetReceipts request to peer {}", peer.address);
            }
            EthMessage::GetBlockHeaders(_) => {
                println!("Unable to serve GetBlockHeaders request to peer {}", peer.address);
            }
            EthMessage::GetBlockBodies(_) => {
                println!("Unable to serve GetBlockBodies request to peer {}", peer.address);
            }
            EthMessage::GetPooledTransactions(_) => {
                println!("Unable to serve GetPooledTransactions request to peer {}", peer.address);
            }
            _ => {}
        }
    }
}
