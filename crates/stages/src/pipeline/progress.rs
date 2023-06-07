use super::ctrl::ControlFlow;
use crate::util::opt;
use reth_primitives::BlockNumber;

#[derive(Debug, Default)]
pub(crate) struct PipelineProgress {
    /// The progress of the current stage
    /// 当前stage的进度
    pub(crate) progress: Option<BlockNumber>,
    /// The maximum progress achieved by any stage during the execution of the pipeline.
    /// 在pipeline执行期间，任何stage达到的最大进度
    pub(crate) maximum_progress: Option<BlockNumber>,
    /// The minimum progress achieved by any stage during the execution of the pipeline.
    /// 在pipeline执行期间，任何stage达到的最小进度
    pub(crate) minimum_progress: Option<BlockNumber>,
}

impl PipelineProgress {
    pub(crate) fn update(&mut self, progress: BlockNumber) {
        self.progress = Some(progress);
        self.minimum_progress = opt::min(self.minimum_progress, progress);
        self.maximum_progress = opt::max(self.maximum_progress, progress);
    }

    /// Get next control flow step
    /// 获取下一个control flow step
    pub(crate) fn next_ctrl(&self) -> ControlFlow {
        match self.progress {
            Some(progress) => ControlFlow::Continue { progress },
            None => ControlFlow::NoProgress { stage_progress: None },
        }
    }
}
