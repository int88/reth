//! Listeners for the transaction-pool
//! 对于tx-pool的listener

use crate::{
    pool::events::{FullTransactionEvent, TransactionEvent},
    traits::PropagateKind,
    PoolTransaction, ValidPoolTransaction,
};
use alloy_primitives::{TxHash, B256};
use futures_util::Stream;
use std::{
    collections::{hash_map::Entry, HashMap},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{
    error::TrySendError, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};

/// The size of the event channel used to propagate transaction events.
const TX_POOL_EVENT_CHANNEL_SIZE: usize = 1024;

/// A Stream that receives [`TransactionEvent`] only for the transaction with the given hash.
/// 一个Stream，接收[`TransactionEvent`]，只对于给定hash的tx
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct TransactionEvents {
    hash: TxHash,
    events: UnboundedReceiver<TransactionEvent>,
}

impl TransactionEvents {
    /// The hash for this transaction
    pub const fn hash(&self) -> TxHash {
        self.hash
    }
}

impl Stream for TransactionEvents {
    type Item = TransactionEvent;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

/// A Stream that receives [`FullTransactionEvent`] for _all_ transaction.
/// 一个Stream，接收[`FullTransactionEvent`]，对于所有的txs
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct AllTransactionsEvents<T: PoolTransaction> {
    pub(crate) events: Receiver<FullTransactionEvent<T>>,
}

impl<T: PoolTransaction> AllTransactionsEvents<T> {
    /// Create a new instance of this stream.
    pub const fn new(events: Receiver<FullTransactionEvent<T>>) -> Self {
        Self { events }
    }
}

impl<T: PoolTransaction> Stream for AllTransactionsEvents<T> {
    type Item = FullTransactionEvent<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().events.poll_recv(cx)
    }
}

/// A type that broadcasts [`TransactionEvent`] to installed listeners.
/// 一个类型广播[`TransactionEvent`]到安装的listeners
///
/// This is essentially a multi-producer, multi-consumer channel where each event is broadcast to
/// all active receivers.
/// 这是multi-producer和multi-consumer的channel，每个event被广播到所有的active receivers
#[derive(Debug)]
pub(crate) struct PoolEventBroadcast<T: PoolTransaction> {
    /// All listeners for all transaction events.
    /// 所有Listenrs，对于所有的tx event
    all_events_broadcaster: AllPoolEventsBroadcaster<T>,
    /// All listeners for events for a certain transaction hash.
    /// 所有的listeners，对于特定tx hash的events
    broadcasters_by_hash: HashMap<TxHash, PoolEventBroadcaster>,
}

impl<T: PoolTransaction> Default for PoolEventBroadcast<T> {
    fn default() -> Self {
        Self {
            all_events_broadcaster: AllPoolEventsBroadcaster::default(),
            broadcasters_by_hash: HashMap::default(),
        }
    }
}

impl<T: PoolTransaction> PoolEventBroadcast<T> {
    /// Calls the broadcast callback with the `PoolEventBroadcaster` that belongs to the hash.
    /// 调用广播的callback，用于属于这个hash的`PoolEventBroadcaster`
    fn broadcast_event(
        &mut self,
        hash: &TxHash,
        event: TransactionEvent,
        pool_event: FullTransactionEvent<T>,
    ) {
        // Broadcast to all listeners for the transaction hash.
        // 广播到所有的listeners，对于这个tx hash
        if let Entry::Occupied(mut sink) = self.broadcasters_by_hash.entry(*hash) {
            sink.get_mut().broadcast(event.clone());

            if sink.get().is_empty() || event.is_final() {
                sink.remove();
            }
        }

        // Broadcast to all listeners for all transactions.
        // 广播到所有的Listeners，对于所有的txs
        self.all_events_broadcaster.broadcast(pool_event);
    }

    /// Create a new subscription for the given transaction hash.
    /// 创建一个新的订阅，对于给定的tx hash
    pub(crate) fn subscribe(&mut self, tx_hash: TxHash) -> TransactionEvents {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        match self.broadcasters_by_hash.entry(tx_hash) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().senders.push(tx);
            }
            Entry::Vacant(entry) => {
                // 添加PoolEventBroadcaster
                entry.insert(PoolEventBroadcaster { senders: vec![tx] });
            }
        };
        TransactionEvents { hash: tx_hash, events: rx }
    }

    /// Create a new subscription for all transactions.
    /// 创建一个新的订阅，对于所有的txs
    pub(crate) fn subscribe_all(&mut self) -> AllTransactionsEvents<T> {
        let (tx, rx) = tokio::sync::mpsc::channel(TX_POOL_EVENT_CHANNEL_SIZE);
        self.all_events_broadcaster.senders.push(tx);
        AllTransactionsEvents::new(rx)
    }

    /// Notify listeners about a transaction that was added to the pending queue.
    /// 通知listeners，关于一个tx被加入到Pending queue
    pub(crate) fn pending(&mut self, tx: &TxHash, replaced: Option<Arc<ValidPoolTransaction<T>>>) {
        self.broadcast_event(tx, TransactionEvent::Pending, FullTransactionEvent::Pending(*tx));

        if let Some(replaced) = replaced {
            // notify listeners that this transaction was replaced
            // 通知listeners，这个tx已经被替换了
            self.replaced(replaced, *tx);
        }
    }

    /// Notify listeners about a transaction that was replaced.
    pub(crate) fn replaced(&mut self, tx: Arc<ValidPoolTransaction<T>>, replaced_by: TxHash) {
        let transaction = Arc::clone(&tx);
        self.broadcast_event(
            tx.hash(),
            TransactionEvent::Replaced(replaced_by),
            FullTransactionEvent::Replaced { transaction, replaced_by },
        );
    }

    /// Notify listeners about a transaction that was added to the queued pool.
    pub(crate) fn queued(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Queued, FullTransactionEvent::Queued(*tx));
    }

    /// Notify listeners about a transaction that was propagated.
    pub(crate) fn propagated(&mut self, tx: &TxHash, peers: Vec<PropagateKind>) {
        let peers = Arc::new(peers);
        self.broadcast_event(
            tx,
            TransactionEvent::Propagated(Arc::clone(&peers)),
            FullTransactionEvent::Propagated(peers),
        );
    }

    /// Notify listeners about a transaction that was discarded.
    /// 通知listeners，关于一个tx已经被移除了
    pub(crate) fn discarded(&mut self, tx: &TxHash) {
        self.broadcast_event(tx, TransactionEvent::Discarded, FullTransactionEvent::Discarded(*tx));
    }

    /// Notify listeners that the transaction was mined
    pub(crate) fn mined(&mut self, tx: &TxHash, block_hash: B256) {
        self.broadcast_event(
            tx,
            TransactionEvent::Mined(block_hash),
            FullTransactionEvent::Mined { tx_hash: *tx, block_hash },
        );
    }
}

/// All Sender half(s) of the event channels for all transactions.
/// 所有的Sender部分，对于所有txs的event channels
///
/// This mimics [`tokio::sync::broadcast`] but uses separate channels.
#[derive(Debug)]
struct AllPoolEventsBroadcaster<T: PoolTransaction> {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<Sender<FullTransactionEvent<T>>>,
}

impl<T: PoolTransaction> Default for AllPoolEventsBroadcaster<T> {
    fn default() -> Self {
        Self { senders: Vec::new() }
    }
}

impl<T: PoolTransaction> AllPoolEventsBroadcaster<T> {
    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, event: FullTransactionEvent<T>) {
        self.senders.retain(|sender| match sender.try_send(event.clone()) {
            Ok(_) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        })
    }
}

/// All Sender half(s) of the event channels for a specific transaction.
/// event channels的Sender部分，对于一个特定的tx
///
/// This mimics [`tokio::sync::broadcast`] but uses separate channels and is unbounded.
/// 这模仿tokio的broadcast，但是使用单独的channels并且是unbounded
#[derive(Default, Debug)]
struct PoolEventBroadcaster {
    /// Corresponding sender half(s) for event listener channel
    senders: Vec<UnboundedSender<TransactionEvent>>,
}

impl PoolEventBroadcaster {
    /// Returns `true` if there are no more listeners remaining.
    fn is_empty(&self) -> bool {
        self.senders.is_empty()
    }

    // Broadcast an event to all listeners. Dropped listeners are silently evicted.
    fn broadcast(&mut self, event: TransactionEvent) {
        self.senders.retain(|sender| sender.send(event.clone()).is_ok())
    }
}
