use reth_interfaces::Result;
use reth_primitives::stage::{StageCheckpoint, StageId};

/// The trait for fetching stage checkpoint related data.
/// trait用于获取stage checkpoint相关的数据
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointProvider: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    /// 获取给定stage的checkpoint
    fn get_stage_checkpoint(&self, id: StageId) -> Result<Option<StageCheckpoint>>;
}
