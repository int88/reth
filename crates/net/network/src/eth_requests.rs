//! Blocks/Headers management for the p2p network.
//! 对于p2p network的Blocks/Headers管理

use crate::{metrics::EthRequestHandlerMetrics, peers::PeersHandle};
use futures::StreamExt;
use reth_eth_wire::{
    BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders, GetNodeData, GetReceipts, NodeData,
    Receipts,
};
use reth_interfaces::p2p::error::RequestResult;
use reth_primitives::{BlockBody, BlockHashOrNumber, Header, HeadersDirection, PeerId};
use reth_provider::{BlockReader, HeaderProvider, ReceiptProvider};
use std::{
    borrow::Borrow,
    future::Future,
    hash::Hash,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::Receiver, oneshot};
use tokio_stream::wrappers::ReceiverStream;

// Limits: <https://github.com/ethereum/go-ethereum/blob/b0d44338bbcefee044f1f635a84487cbbd8f0538/eth/protocols/eth/handler.go#L34-L56>

/// Maximum number of receipts to serve.
///
/// Used to limit lookups.
const MAX_RECEIPTS_SERVE: usize = 1024;

/// Maximum number of block headers to serve.
/// 能服务的最大的block headers的数目
///
/// Used to limit lookups.
const MAX_HEADERS_SERVE: usize = 1024;

/// Maximum number of block headers to serve.
///
/// Used to limit lookups. With 24KB block sizes nowadays, the practical limit will always be
/// SOFT_RESPONSE_LIMIT.
const MAX_BODIES_SERVE: usize = 1024;

/// Estimated size in bytes of an RLP encoded receipt.
const APPROX_RECEIPT_SIZE: usize = 24 * 1024;

/// Estimated size in bytes of an RLP encoded body.
// TODO: check 24kb blocksize assumption
const APPROX_BODY_SIZE: usize = 24 * 1024;

/// Maximum size of replies to data retrievals.
const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// Estimated size in bytes of an RLP encoded header.
const APPROX_HEADER_SIZE: usize = 500;

/// Manages eth related requests on top of the p2p network.
/// 管理eth相关的请求，在p2p network之上
///
/// This can be spawned to another task and is supposed to be run as background service.
/// 这可以生成另一个task并且应该以background service运行
#[derive(Debug)]
#[must_use = "Manager does nothing unless polled."]
pub struct EthRequestHandler<C> {
    /// The client type that can interact with the chain.
    /// 和chain交互的client类型
    client: C,
    /// Used for reporting peers.
    /// 用于汇报peers
    #[allow(unused)]
    // TODO use to report spammers
    peers: PeersHandle,
    /// Incoming request from the [NetworkManager](crate::NetworkManager).
    /// 来自[NetworkManager](crate::NetworkManager)的Incoming request
    incoming_requests: ReceiverStream<IncomingEthRequest>,
    /// Metrics for the eth request handler.
    /// 对于eth request handler的metrics
    metrics: EthRequestHandlerMetrics,
}

// === impl EthRequestHandler ===
impl<C> EthRequestHandler<C> {
    /// Create a new instance
    pub fn new(client: C, peers: PeersHandle, incoming: Receiver<IncomingEthRequest>) -> Self {
        let metrics = Default::default();
        Self { client, peers, incoming_requests: ReceiverStream::new(incoming), metrics }
    }
}

impl<C> EthRequestHandler<C>
where
    // 包含了BlockReader，HeaderReader以及ReceiptProvider
    C: BlockReader + HeaderProvider + ReceiptProvider,
{
    /// Returns the list of requested headers
    /// 返回一系列请求的headers
    fn get_headers_response(&self, request: GetBlockHeaders) -> Vec<Header> {
        let GetBlockHeaders { start_block, limit, skip, direction } = request;

        let mut headers = Vec::new();

        let mut block: BlockHashOrNumber = match start_block {
            BlockHashOrNumber::Hash(start) => start.into(),
            BlockHashOrNumber::Number(num) => {
                let Some(hash) = self.client.block_hash(num).unwrap_or_default() else {
                    return headers
                };
                hash.into()
            }
        };

        let skip = skip as u64;
        let mut total_bytes = APPROX_HEADER_SIZE;

        for _ in 0..limit {
            if let Some(header) = self.client.header_by_hash_or_number(block).unwrap_or_default() {
                match direction {
                    HeadersDirection::Rising => {
                        if let Some(next) = (header.number + 1).checked_add(skip) {
                            block = next.into()
                        } else {
                            break
                        }
                    }
                    HeadersDirection::Falling => {
                        if skip > 0 {
                            // prevent under flows for block.number == 0 and `block.number - skip <
                            // 0`
                            if let Some(next) =
                                header.number.checked_sub(1).and_then(|num| num.checked_sub(skip))
                            {
                                block = next.into()
                            } else {
                                break
                            }
                        } else {
                            block = header.parent_hash.into()
                        }
                    }
                }

                headers.push(header);

                if headers.len() >= MAX_HEADERS_SERVE {
                    break
                }

                total_bytes += APPROX_HEADER_SIZE;

                if total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }
            } else {
                break
            }
        }

        headers
    }

    fn on_headers_request(
        &mut self,
        _peer_id: PeerId,
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    ) {
        self.metrics.received_headers_requests.increment(1);
        let headers = self.get_headers_response(request);
        let _ = response.send(Ok(BlockHeaders(headers)));
    }

    fn on_bodies_request(
        &mut self,
        _peer_id: PeerId,
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies>>,
    ) {
        self.metrics.received_bodies_requests.increment(1);
        let mut bodies = Vec::new();

        let mut total_bytes = APPROX_BODY_SIZE;

        for hash in request.0 {
            if let Some(block) = self.client.block_by_hash(hash).unwrap_or_default() {
                let body = BlockBody {
                    transactions: block.body,
                    ommers: block.ommers,
                    withdrawals: block.withdrawals,
                };

                bodies.push(body);

                total_bytes += APPROX_BODY_SIZE;

                if total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }

                if bodies.len() >= MAX_BODIES_SERVE {
                    break
                }
            } else {
                break
            }
        }

        let _ = response.send(Ok(BlockBodies(bodies)));
    }

    fn on_receipts_request(
        &mut self,
        _peer_id: PeerId,
        request: GetReceipts,
        response: oneshot::Sender<RequestResult<Receipts>>,
    ) {
        let mut receipts = Vec::new();

        let mut total_bytes = APPROX_RECEIPT_SIZE;

        for hash in request.0 {
            if let Some(receipts_by_block) =
                self.client.receipts_by_block(BlockHashOrNumber::Hash(hash)).unwrap_or_default()
            {
                receipts.push(
                    receipts_by_block
                        .into_iter()
                        .map(|receipt| receipt.with_bloom())
                        .collect::<Vec<_>>(),
                );

                total_bytes += APPROX_RECEIPT_SIZE;

                if total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }

                if receipts.len() >= MAX_RECEIPTS_SERVE {
                    break
                }
            } else {
                break
            }
        }

        let _ = response.send(Ok(Receipts(receipts)));
    }
}

/// An endless future.
/// 一个endless的future
///
/// This should be spawned or used as part of `tokio::select!`.
impl<C> Future for EthRequestHandler<C>
where
    C: BlockReader + HeaderProvider + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match this.incoming_requests.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Ready(Some(incoming)) => match incoming {
                    IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                        // 获取了header request
                        this.on_headers_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetBlockBodies { peer_id, request, response } => {
                        // 获取了body request
                        this.on_bodies_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetNodeData { .. } => {}
                    IncomingEthRequest::GetReceipts { peer_id, request, response } => {
                        // 获取receipts requests
                        this.on_receipts_request(peer_id, request, response)
                    }
                },
            }
        }
    }
}

/// Represents a handled [`GetBlockHeaders`] requests
///
/// This is the key type for spam detection cache. The counter is ignored during `PartialEq` and
/// `Hash`.
#[derive(Debug, PartialEq, Hash)]
#[allow(unused)]
struct RespondedGetBlockHeaders {
    req: (PeerId, GetBlockHeaders),
}

impl Borrow<(PeerId, GetBlockHeaders)> for RespondedGetBlockHeaders {
    fn borrow(&self) -> &(PeerId, GetBlockHeaders) {
        &self.req
    }
}

/// All `eth` request related to blocks delegated by the network.
/// 所有的`eth`请求，和block相关
#[derive(Debug)]
#[allow(missing_docs)]
pub enum IncomingEthRequest {
    /// Request Block headers from the peer.
    /// 从peer请求Block headers
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        peer_id: PeerId,
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    },
    /// Request Block headers from the peer.
    /// 从peer请求Block bodies
    ///
    /// The response should be sent through the channel.
    GetBlockBodies {
        peer_id: PeerId,
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies>>,
    },
    /// Request Node Data from the peer.
    /// 从peer请求Node Data
    ///
    /// The response should be sent through the channel.
    GetNodeData {
        peer_id: PeerId,
        request: GetNodeData,
        response: oneshot::Sender<RequestResult<NodeData>>,
    },
    /// Request Receipts from the peer.
    /// 从peer请求Receipts
    ///
    /// The response should be sent through the channel.
    /// response应该通过channel发送
    GetReceipts {
        peer_id: PeerId,
        request: GetReceipts,
        response: oneshot::Sender<RequestResult<Receipts>>,
    },
}
