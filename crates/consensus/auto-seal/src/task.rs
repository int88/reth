use crate::{mode::MiningMode, Storage};
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::{BeaconEngineMessage, ForkchoiceStatus};
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{Block, ChainSpec, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{CanonChainTracker, CanonStateNotificationSender, Chain, StateProviderFactory};
use reth_stages::PipelineEvent;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, warn};

/// A Future that listens for new ready transactions and puts new blocks into storage
/// 一个Future，监听新的ready txs并且将新的blocks放入storage
pub struct MiningTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    /// 配置的chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    /// 用于和state交互的client
    client: Client,
    /// The active miner
    miner: MiningMode,
    /// Single active future that inserts a new block into `storage`
    /// 单个的active future，插入一个新的block到`storage`
    insert_task: Option<BoxFuture<'static, Option<UnboundedReceiverStream<PipelineEvent>>>>,
    /// Shared storage to insert new blocks
    /// 共享的storage用于插入新的blocks
    storage: Storage,
    /// Pool where transactions are stored
    /// 存储txs的pool
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    /// 准备被挖掘的一系列txs的backlog
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    /// 用于通知consumers，关于新的blocks
    canon_state_notification: CanonStateNotificationSender,
    /// The pipeline events to listen on
    /// 监听的pipeline events
    pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
}

// === impl MiningTask ===

impl<Client, Pool: TransactionPool> MiningTask<Client, Pool> {
    /// Creates a new instance of the task
    /// 创建task的一个新实例
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        miner: MiningMode,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        canon_state_notification: CanonStateNotificationSender,
        storage: Storage,
        client: Client,
        pool: Pool,
    ) -> Self {
        Self {
            chain_spec,
            client,
            miner,
            insert_task: None,
            storage,
            pool,
            to_engine,
            canon_state_notification,
            queued: Default::default(),
            pipe_line_events: None,
        }
    }

    /// Sets the pipeline events to listen on.
    /// 设置监听的pipeline events
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool> Future for MiningTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // this drives block production and
        // 这驱动block的生成
        loop {
            // 轮询miner
            if let Poll::Ready(transactions) = this.miner.poll(&this.pool, cx) {
                // miner returned a set of transaction that we feed to the producer
                // miner返回一系列的tx，我们需要提供给producer
                this.queued.push_back(transactions);
            }

            if this.insert_task.is_none() {
                if this.queued.is_empty() {
                    // nothing to insert
                    // 没有什么要插入
                    break
                }

                // ready to queue in new insert task
                // 准备排队，在新的insert task
                let storage = this.storage.clone();
                // 出队
                let transactions = this.queued.pop_front().expect("not empty");

                let to_engine = this.to_engine.clone();
                let client = this.client.clone();
                let chain_spec = Arc::clone(&this.chain_spec);
                let pool = this.pool.clone();
                let events = this.pipe_line_events.take();
                let canon_state_notification = this.canon_state_notification.clone();

                // Create the mining future that creates a block, notifies the engine that drives
                // the pipeline
                // 创建mining future，创建一个block，并且通知engine，驱动pipeline
                this.insert_task = Some(Box::pin(async move {
                    let mut storage = storage.write().await;

                    let (transactions, senders): (Vec<_>, Vec<_>) = transactions
                        .into_iter()
                        .map(|tx| {
                            let recovered = tx.to_recovered_transaction();
                            let signer = recovered.signer();
                            (recovered.into_signed(), signer)
                        })
                        .unzip();

                    // 构建并且执行
                    match storage.build_and_execute(transactions.clone(), &client, chain_spec) {
                        Ok((new_header, bundle_state)) => {
                            // clear all transactions from pool
                            // 从pool情理所有的txs
                            pool.remove_transactions(
                                transactions.iter().map(|tx| tx.hash()).collect(),
                            );

                            let state = ForkchoiceState {
                                head_block_hash: new_header.hash,
                                finalized_block_hash: new_header.hash,
                                safe_block_hash: new_header.hash,
                            };
                            drop(storage);

                            // TODO: make this a future
                            // await the fcu call rx for SYNCING, then wait for a VALID response
                            // 等待fcu call rx用于SYNCING，之后等待一个VALID response
                            loop {
                                // send the new update to the engine, this will trigger the engine
                                // to download and execute the block we just inserted
                                // 发送新的update到engine，
                                // 这会触发engine去下载并且执行我们刚刚插入的block
                                let (tx, rx) = oneshot::channel();
                                let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                                    state,
                                    payload_attrs: None,
                                    tx,
                                });
                                debug!(target: "consensus::auto", ?state, "Sent fork choice update");

                                match rx.await.unwrap() {
                                    Ok(fcu_response) => {
                                        // 等待fcu response
                                        match fcu_response.forkchoice_status() {
                                            ForkchoiceStatus::Valid => break,
                                            ForkchoiceStatus::Invalid => {
                                                error!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned invalid response");
                                                return None
                                            }
                                            ForkchoiceStatus::Syncing => {
                                                // FCU返回SYNCING，等待VALID
                                                debug!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                                                // wait for the next fork choice update
                                                continue
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        error!(target: "consensus::auto", ?err, "Autoseal fork choice update failed");
                                        return None
                                    }
                                }
                            }

                            // seal the block
                            // 封装block
                            let block = Block {
                                header: new_header.clone().unseal(),
                                body: transactions,
                                ommers: vec![],
                                withdrawals: None,
                            };
                            let sealed_block = block.seal_slow();

                            let sealed_block_with_senders =
                                SealedBlockWithSenders::new(sealed_block, senders)
                                    .expect("senders are valid");

                            // update canon chain for rpc
                            // 更新canon chain用于rpc
                            client.set_canonical_head(new_header.clone());
                            client.set_safe(new_header.clone());
                            client.set_finalized(new_header.clone());

                            debug!(target: "consensus::auto", header=?sealed_block_with_senders.hash(), "sending block notification");

                            let chain =
                                Arc::new(Chain::new(vec![sealed_block_with_senders], bundle_state));

                            // send block notification
                            // 发送block notification
                            let _ = canon_state_notification
                                .send(reth_provider::CanonStateNotification::Commit { new: chain });
                        }
                        Err(err) => {
                            warn!(target: "consensus::auto", ?err, "failed to execute block")
                        }
                    }

                    events
                }));
            }

            if let Some(mut fut) = this.insert_task.take() {
                match fut.poll_unpin(cx) {
                    Poll::Ready(events) => {
                        this.pipe_line_events = events;
                    }
                    Poll::Pending => {
                        this.insert_task = Some(fut);
                        break
                    }
                }
            }
        }

        Poll::Pending
    }
}

impl<Client, Pool: TransactionPool> std::fmt::Debug for MiningTask<Client, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiningTask").finish_non_exhaustive()
    }
}
