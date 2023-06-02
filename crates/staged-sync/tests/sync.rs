use ethers_core::{
    types::{transaction::eip2718::TypedTransaction, Eip1559TransactionRequest, H160, U64},
    utils::Geth,
};
use ethers_providers::Middleware;
use reth_network::{
    test_utils::{unused_tcp_and_udp_port, unused_tcp_udp, NetworkEventStream},
    NetworkConfig, NetworkManager,
};
use reth_network_api::Peers;
use reth_primitives::{ChainSpec, PeerId, SealedHeader};
use reth_provider::test_utils::NoopProvider;
use reth_staged_sync::test_utils::{CliqueGethInstance, CliqueMiddleware};
use secp256k1::SecretKey;
use std::{net::SocketAddr, sync::Arc};

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(not(feature = "geth-tests"), ignore)]
async fn can_peer_with_geth() {
    reth_tracing::init_test_tracing();

    // 初始化geth
    let (clique, chainspec) = init_geth().await;
    let geth_p2p_port = clique.instance.p2p_port().unwrap();

    // === initialize reth networking stack ===
    // 初始化reth的networking stack

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let (reth_p2p, reth_disc) = unused_tcp_udp();
    tracing::info!(
        %reth_p2p,
        %reth_disc,
        "setting up reth networking stack in keepalive test"
    );

    let config = NetworkConfig::<Arc<NoopProvider>>::builder(secret_key)
        .listener_addr(reth_p2p)
        .discovery_addr(reth_disc)
        .chain_spec(chainspec)
        .build(Arc::new(NoopProvider::default()));

    let network = NetworkManager::new(config).await.unwrap();
    let handle = network.handle().clone();

    tokio::task::spawn(network);

    // create networkeventstream to get the next session established event easily
    // 创建networkeventstream以便轻松获取下一个session established事件
    let mut events = NetworkEventStream::new(handle.event_listener());
    let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);

    // get the peer id we should be expecting
    // 获取我们应该期望的peer id
    let geth_peer_id: PeerId = clique.provider.peer_id().await.unwrap();

    // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
    // 添加geth作为
    handle.add_peer(geth_peer_id, geth_socket);

    // wait for the session to be established
    // 等待session建立
    let peer_id = events.peer_added_and_established().await.unwrap();
    assert_eq!(geth_peer_id, peer_id);
}

async fn init_geth() -> (CliqueGethInstance, Arc<ChainSpec>) {
    // first create a signer that we will fund so we can make transactions
    // 首先创建一个signer，我们会fund，这样我们就可以make transactions
    let chain_id = 13337u64;
    let data_dir = tempfile::tempdir().expect("should be able to create temp geth datadir");
    let dir_path = data_dir.path();
    tracing::info!(
        data_dir=?dir_path,
        "initializing geth instance"
    );

    // this creates a funded geth
    // 创建一个funded geth
    let clique_geth = Geth::new()
        .chain_id(chain_id)
        .p2p_port(unused_tcp_and_udp_port())
        .data_dir(dir_path.to_str().unwrap());

    // build the funded geth
    // 构建funded geth
    let mut clique = CliqueGethInstance::new(clique_geth, None).await;
    let geth_p2p_port =
        clique.instance.p2p_port().expect("geth should be configured with a p2p port");
    tracing::info!(
        p2p_port=%geth_p2p_port,
        rpc_port=%clique.instance.port(),
        "configured clique geth instance in keepalive test"
    );

    // don't print logs, but drain the stderr
    // 不要输出日志，但是drain stderr
    clique.prevent_blocking().await;

    // get geth to start producing blocks - use a blank password
    // 让geth开始产生blocks - 使用空密码
    let clique_private_key = clique
        .instance
        .clique_private_key()
        .clone()
        .expect("clique should be configured with a private key");
    clique.provider.enable_mining(clique_private_key, "".into()).await.unwrap();

    // === check that we have the same genesis hash ===
    // 检查我们是否有相同的genesis hash

    // get the chainspec from the genesis we configured for geth
    // 从genesis获取chainspec，我们为geth配置了genesis
    let chainspec: ChainSpec = clique
        .instance
        .genesis()
        .clone()
        .expect("clique should be configured with a genesis")
        .into();
    let remote_genesis = SealedHeader::from(&clique.provider.remote_genesis_block().await.unwrap());

    let local_genesis = chainspec.genesis_header().seal(chainspec.genesis_hash());
    assert_eq!(local_genesis, remote_genesis, "genesis blocks should match, we computed {local_genesis:#?} but geth computed {remote_genesis:#?}");

    // === create many blocks ===
    // 创建许多blocks

    let nonces = 0..1000u64;
    let txs = nonces.map(|nonce| {
        // create a tx that just sends to the zero addr
        TypedTransaction::Eip1559(
            Eip1559TransactionRequest::new().to(H160::zero()).value(1u64).nonce(nonce),
        )
    });
    tracing::info!("generated transactions for blocks");

    // finally send the txs to geth
    // 最后发送txs到geth
    clique.provider.send_requests(txs).await.unwrap();

    let block = clique.provider.get_block_number().await.unwrap();
    assert!(block > U64::zero());

    (clique, Arc::new(chainspec))
}
