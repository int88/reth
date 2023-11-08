//! The mode the auto seal miner is operating in.
//! 自动的seal miner在运行

use futures_util::{stream::Fuse, StreamExt};
use reth_primitives::TxHash;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    fmt,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::mpsc::Receiver, time::Interval};
use tokio_stream::{wrappers::ReceiverStream, Stream};

/// Mode of operations for the `Miner`
/// 对于`Miner`的操作模式
#[derive(Debug)]
pub enum MiningMode {
    /// A miner that does nothing
    /// miner什么都不做
    None,
    /// A miner that listens for new transactions that are ready.
    /// 一个miner监听新的txs，当它们ready的时候
    ///
    /// Either one transaction will be mined per block, or any number of transactions will be
    /// allowed
    /// 每个block一个tx，或者任何数目的txs都是允许的
    Auto(ReadyTransactionMiner),
    /// A miner that constructs a new block every `interval` tick
    /// miner构建一个新的block，每`interval`
    FixedBlockTime(FixedBlockTimeMiner),
}

// === impl MiningMode ===

impl MiningMode {
    /// Creates a new instant mining mode that listens for new transactions and tries to build
    /// non-empty blocks as soon as transactions arrive.
    /// 创建一个新的、瞬时的mining mode，监听新的txs并且试着构建非空的blocks，一旦txs到达
    pub fn instant(max_transactions: usize, listener: Receiver<TxHash>) -> Self {
        MiningMode::Auto(ReadyTransactionMiner {
            max_transactions,
            has_pending_txs: None,
            rx: ReceiverStream::new(listener).fuse(),
        })
    }

    /// Creates a new interval miner that builds a block ever `duration`.
    /// 创建一个新的interval miner，每个`duration`构建一个block
    pub fn interval(duration: Duration) -> Self {
        MiningMode::FixedBlockTime(FixedBlockTimeMiner::new(duration))
    }

    /// polls the Pool and returns those transactions that should be put in a block, if any.
    /// 拉取Pool并且返回这些transactions，应该被放在一个block内，如果有的话
    pub(crate) fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        match self {
            MiningMode::None => Poll::Pending,
            MiningMode::Auto(miner) => miner.poll(pool, cx),
            MiningMode::FixedBlockTime(miner) => miner.poll(pool, cx),
        }
    }
}

/// A miner that's supposed to create a new block every `interval`, mining all transactions that are
/// ready at that time.
///
/// The default blocktime is set to 6 seconds
#[derive(Debug)]
pub struct FixedBlockTimeMiner {
    /// The interval this fixed block time miner operates with
    interval: Interval,
}

// === impl FixedBlockTimeMiner ===

impl FixedBlockTimeMiner {
    /// Creates a new instance with an interval of `duration`
    /// 创建一个新的instance，有着`duration`的时间间隔
    pub(crate) fn new(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self { interval: tokio::time::interval_at(start, duration) }
    }

    fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        if self.interval.poll_tick(cx).is_ready() {
            // drain the pool
            // 对Pool进行drain
            return Poll::Ready(pool.best_transactions().collect())
        }
        Poll::Pending
    }
}

impl Default for FixedBlockTimeMiner {
    fn default() -> Self {
        Self::new(Duration::from_secs(6))
    }
}

/// A miner that Listens for new ready transactions
/// 一个miner，监听新的ready txs
pub struct ReadyTransactionMiner {
    /// how many transactions to mine per block
    /// 每个block挖掘多少个txs
    max_transactions: usize,
    /// stores whether there are pending transactions (if known)
    /// 存储是否有pending txs（如果已知的话）
    has_pending_txs: Option<bool>,
    /// Receives hashes of transactions that are ready
    /// 接收已经ready的txs的hashes
    rx: Fuse<ReceiverStream<TxHash>>,
}

// === impl ReadyTransactionMiner ===

impl ReadyTransactionMiner {
    fn poll<Pool>(
        &mut self,
        pool: &Pool,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>
    where
        Pool: TransactionPool,
    {
        // drain the notification stream
        // 排干notification stream
        while let Poll::Ready(Some(_hash)) = Pin::new(&mut self.rx).poll_next(cx) {
            self.has_pending_txs = Some(true);
        }

        if self.has_pending_txs == Some(false) {
            return Poll::Pending
        }

        let transactions = pool.best_transactions().take(self.max_transactions).collect::<Vec<_>>();

        // there are pending transactions if we didn't drain the pool
        // 有pending txs，如果没有排干pool
        self.has_pending_txs = Some(transactions.len() >= self.max_transactions);

        if transactions.is_empty() {
            return Poll::Pending
        }

        Poll::Ready(transactions)
    }
}

impl fmt::Debug for ReadyTransactionMiner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadyTransactionMiner")
            .field("max_transactions", &self.max_transactions)
            .finish_non_exhaustive()
    }
}
