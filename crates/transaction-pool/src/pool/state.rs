bitflags::bitflags! {
    /// Marker to represents the current state of a transaction in the pool and from which the corresponding sub-pool is derived, depending on what bits are set.
    ///
    /// This mirrors [erigon's ephemeral state field](https://github.com/ledgerwatch/erigon/wiki/Transaction-Pool-Design#ordering-function).
     #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
    pub(crate) struct TxState: u8 {
        /// Set to `1` if all ancestor transactions are pending.
        /// 设置为`1`如果所有的transactions都是pending
        const NO_PARKED_ANCESTORS = 0b100000;
        /// Set to `1` of the transaction is either the next transaction of the sender (on chain nonce == tx.nonce) or all prior transactions are also present in the pool.
        /// 设置为`1`，如果sender的下一个transaction（on chain nonce == tx.nonce）或者所有之前的transactions已经在pool存在
        const NO_NONCE_GAPS = 0b010000;
        /// Bit derived from the sender's balance.
        ///
        /// Set to `1` if the sender's balance can cover the maximum cost for this transaction (`feeCap * gasLimit + value`).
        /// This includes cumulative costs of prior transactions, which ensures that the sender has enough funds for all max cost of prior transactions.
        const ENOUGH_BALANCE = 0b001000;
        /// Bit set to true if the transaction has a lower gas limit than the block's gas limit
        const NOT_TOO_MUCH_GAS = 0b000100;
        /// Covers the Dynamic fee requirement.
        /// 覆盖动态的fee requirement
        ///
        /// Set to 1 if `feeCap` of the transaction meets the requirement of the pending block.
        /// 设置为1，如果tx的`feeCap`满足pending block的情况
        const ENOUGH_FEE_CAP_BLOCK = 0b000010;

        const PENDING_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits()| Self::NO_NONCE_GAPS.bits() | Self::ENOUGH_BALANCE.bits() | Self::NOT_TOO_MUCH_GAS.bits() |  Self::ENOUGH_FEE_CAP_BLOCK.bits();

        const BASE_FEE_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits() | Self::NO_NONCE_GAPS.bits() | Self::ENOUGH_BALANCE.bits() | Self::NOT_TOO_MUCH_GAS.bits();

        const QUEUED_POOL_BITS  = Self::NO_PARKED_ANCESTORS.bits();

    }
}

// === impl TxState ===

impl TxState {
    /// The state of a transaction is considered `pending`, if the transaction has:
    ///   - _No_ parked ancestors
    ///   - enough balance
    ///   - enough fee cap
    /// 一个tx的state被认为是`pending`，如果tx有：
    ///   - 没有parked ancestor
    ///   - 有足够的balance
    ///   - 有足够的fee cap
    #[inline]
    pub(crate) fn is_pending(&self) -> bool {
        *self >= TxState::PENDING_POOL_BITS
    }

    /// Returns `true` if the transaction has a nonce gap.
    /// 返回`true`，如果tx有一个nonce gap
    #[inline]
    pub(crate) fn has_nonce_gap(&self) -> bool {
        !self.intersects(TxState::NO_NONCE_GAPS)
    }
}

/// Identifier for the transaction Sub-pool
/// 对于Sub-pool的transaction的标识符
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum SubPool {
    /// The queued sub-pool contains transactions that are not ready to be included in the next
    /// block because they have missing or queued ancestors.
    /// queued sub-pool包含transactions，没有准备好包含在下一个block，
    /// 因为他们缺失或者有排队的ancestors
    Queued = 0,
    /// The base-fee sub-pool contains transactions that are not ready to be included in the next
    /// block because they don't meet the base fee requirement.
    /// base-fee sub-pool包含transactions没有准备好被加入到下一个block，因为他们没有满足base
    /// fee的要求
    BaseFee,
    /// The pending sub-pool contains transactions that are ready to be included in the next block.
    /// 准备好被包含到下一个block的transactions的sub-pool
    Pending,
}

// === impl SubPool ===

impl SubPool {
    /// Whether this transaction is to be moved to the pending sub-pool.
    /// 这个tx是否被移到pending sub-pool
    #[inline]
    pub fn is_pending(&self) -> bool {
        matches!(self, SubPool::Pending)
    }

    /// Whether this transaction is in the queued pool.
    /// 这个tx是否在queued pool
    #[inline]
    pub fn is_queued(&self) -> bool {
        matches!(self, SubPool::Queued)
    }

    /// Whether this transaction is in the base fee pool.
    #[inline]
    pub fn is_base_fee(&self) -> bool {
        matches!(self, SubPool::BaseFee)
    }

    /// Returns whether this is a promotion depending on the current sub-pool location.
    /// 返回是否是一个promotion，依赖于当前的sub-pool的位置
    #[inline]
    pub fn is_promoted(&self, other: SubPool) -> bool {
        self > &other
    }
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value.is_pending() {
            return SubPool::Pending
        }
        if value < TxState::BASE_FEE_POOL_BITS {
            return SubPool::Queued
        }
        SubPool::BaseFee
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promoted() {
        assert!(SubPool::BaseFee.is_promoted(SubPool::Queued));
        assert!(SubPool::Pending.is_promoted(SubPool::BaseFee));
        assert!(SubPool::Pending.is_promoted(SubPool::Queued));
        assert!(!SubPool::BaseFee.is_promoted(SubPool::Pending));
        assert!(!SubPool::Queued.is_promoted(SubPool::BaseFee));
    }

    #[test]
    fn test_tx_state() {
        let mut state = TxState::default();
        state |= TxState::NO_NONCE_GAPS;
        assert!(state.intersects(TxState::NO_NONCE_GAPS))
    }

    #[test]
    fn test_tx_queued() {
        let state = TxState::default();
        assert_eq!(SubPool::Queued, state.into());

        let state = TxState::NO_PARKED_ANCESTORS |
            TxState::NO_NONCE_GAPS |
            TxState::NOT_TOO_MUCH_GAS |
            TxState::ENOUGH_FEE_CAP_BLOCK;
        // state的状态
        assert_eq!(SubPool::Queued, state.into());
    }

    #[test]
    fn test_tx_pending() {
        let state = TxState::PENDING_POOL_BITS;
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());

        let bits = 0b111110;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());

        let bits = 0b111110;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());
    }
}
