/// Guarantees max transactions for one sender, compatible with geth/erigon
/// 每个sender保证的最大的txs，和geth/erigon兼容
pub const TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER: usize = 16;

/// The default maximum allowed number of transactions in the given subpool.
/// 在给定的subpool中默认允许最大的txs数目
pub const TXPOOL_SUBPOOL_MAX_TXS_DEFAULT: usize = 10_000;

/// The default maximum allowed size of the given subpool.
/// 给定的subpool允许的最大的size
pub const TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT: usize = 20;

/// Default price bump (in %) for the transaction pool underpriced check.
pub const DEFAULT_PRICE_BUMP: u128 = 10;

/// Replace blob price bump (in %) for the transaction pool underpriced check.
pub const REPLACE_BLOB_PRICE_BUMP: u128 = 100;

/// Configuration options for the Transaction pool.
/// Transaction pool的配置项选项
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Max number of transaction in the pending sub-pool
    /// pending sub-pool最大的tx数目
    pub pending_limit: SubPoolLimit,
    /// Max number of transaction in the basefee sub-pool
    /// basefee sub-pool最大的tx数目
    pub basefee_limit: SubPoolLimit,
    /// Max number of transaction in the queued sub-pool
    /// queued sub-pool最大的tx数目
    pub queued_limit: SubPoolLimit,
    /// Max number of executable transaction slots guaranteed per account
    /// 赋值给每个account的最大的tx slots
    pub max_account_slots: usize,
    /// Price bump (in %) for the transaction pool underpriced check.
    pub price_bump: u128,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            pending_limit: Default::default(),
            basefee_limit: Default::default(),
            queued_limit: Default::default(),
            max_account_slots: TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
            price_bump: PriceBumpConfig::default().default_price_bump,
        }
    }
}

/// Size limits for a sub-pool.
/// 一个sub-pool的Size limits
#[derive(Debug, Clone)]
pub struct SubPoolLimit {
    /// Maximum amount of transaction in the pool.
    /// pool中最大的tx的数目
    pub max_txs: usize,
    /// Maximum combined size (in bytes) of transactions in the pool.
    /// pool中txs最大的combined size
    pub max_size: usize,
}

impl SubPoolLimit {
    /// Returns whether the size or amount constraint is violated.
    #[inline]
    pub fn is_exceeded(&self, txs: usize, size: usize) -> bool {
        self.max_txs < txs || self.max_size < size
    }
}

impl Default for SubPoolLimit {
    fn default() -> Self {
        // either 10k transactions or 20MB
        // 10k个txs或者20MB
        Self {
            max_txs: TXPOOL_SUBPOOL_MAX_TXS_DEFAULT,
            max_size: TXPOOL_SUBPOOL_MAX_SIZE_MB_DEFAULT * 1024 * 1024,
        }
    }
}

/// Price bump config (in %) for the transaction pool underpriced check.
#[derive(Debug, Clone)]
pub struct PriceBumpConfig {
    /// Default price bump (in %) for the transaction pool underpriced check.
    pub default_price_bump: u128,
    /// Replace blob price bump (in %) for the transaction pool underpriced check.
    pub replace_blob_tx_price_bump: u128,
}

impl Default for PriceBumpConfig {
    fn default() -> Self {
        Self {
            default_price_bump: DEFAULT_PRICE_BUMP,
            replace_blob_tx_price_bump: REPLACE_BLOB_PRICE_BUMP,
        }
    }
}
