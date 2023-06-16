use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Account, Address};

/// Account provider
#[auto_impl(&, Arc, Box)]
pub trait AccountProvider: Send + Sync {
    /// Get basic account information.
    /// 获取基础的account信息
    ///
    /// Returns `None` if the account doesn't exist.
    /// 返回`None`如果account不存在
    fn basic_account(&self, address: Address) -> Result<Option<Account>>;
}
