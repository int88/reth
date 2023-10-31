use reth_interfaces::RethResult;
use reth_primitives::{BlockHashOrNumber, Withdrawal};

///  Client trait for fetching [Withdrawal] related data.
/// Client trait用于获取[Withdrawal]相关的数据
#[auto_impl::auto_impl(&, Arc)]
pub trait WithdrawalsProvider: Send + Sync {
    /// Get withdrawals by block id.
    /// 通过block id获取withdrawals
    fn withdrawals_by_block(
        &self,
        id: BlockHashOrNumber,
        timestamp: u64,
    ) -> RethResult<Option<Vec<Withdrawal>>>;

    /// Get latest withdrawal from this block or earlier .
    /// 从这个或者更早的block获取最新的withdrawal
    fn latest_withdrawal(&self) -> RethResult<Option<Withdrawal>>;
}
