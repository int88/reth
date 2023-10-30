use reth_interfaces::RethResult;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};

/// The trait for fetching stage checkpoint related data.
/// 用于获取stage checkpoint相关数据的trait
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointReader: Send + Sync {
    /// Fetch the checkpoint for the given stage.
    /// 获取给定的stage的checkpoint
    fn get_stage_checkpoint(&self, id: StageId) -> RethResult<Option<StageCheckpoint>>;

    /// Get stage checkpoint progress.
    /// 获取stage checkpoint的进程
    fn get_stage_checkpoint_progress(&self, id: StageId) -> RethResult<Option<Vec<u8>>>;
}

/// The trait for updating stage checkpoint related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait StageCheckpointWriter: Send + Sync {
    /// Save stage checkpoint.
    fn save_stage_checkpoint(&self, id: StageId, checkpoint: StageCheckpoint) -> RethResult<()>;

    /// Save stage checkpoint progress.
    fn save_stage_checkpoint_progress(&self, id: StageId, checkpoint: Vec<u8>) -> RethResult<()>;

    /// Update all pipeline sync stage progress.
    fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
        drop_stage_checkpoint: bool,
    ) -> RethResult<()>;
}
