use reth_primitives::{BlockNumber, SealedHeader};

/// Determines the control flow during pipeline execution.
#[derive(Debug, Eq, PartialEq)]
pub enum ControlFlow {
    /// An unwind was requested and must be performed before continuing.
    /// 请求了unwind，必须在继续之前执行
    Unwind {
        /// The block to unwind to.
        target: BlockNumber,
        /// The block that caused the unwind.
        /// 导致unwind的block
        bad_block: SealedHeader,
    },
    /// The pipeline is allowed to continue executing stages.
    /// 允许pipeline继续执行stages
    Continue {
        /// The progress of the last stage
        /// 上一个stage的进度
        progress: BlockNumber,
    },
    /// Pipeline made no progress
    /// Pipeline没有进展
    NoProgress {
        /// The current stage progress.
        /// 当前的stage process
        stage_progress: Option<BlockNumber>,
    },
}

impl ControlFlow {
    /// Whether the pipeline should continue executing stages.
    pub fn should_continue(&self) -> bool {
        matches!(self, ControlFlow::Continue { .. } | ControlFlow::NoProgress { .. })
    }

    /// Returns true if the control flow is unwind.
    pub fn is_unwind(&self) -> bool {
        matches!(self, ControlFlow::Unwind { .. })
    }

    /// Returns the pipeline progress, if the state is not `Unwind`.
    pub fn progress(&self) -> Option<BlockNumber> {
        match self {
            ControlFlow::Unwind { .. } => None,
            ControlFlow::Continue { progress } => Some(*progress),
            ControlFlow::NoProgress { stage_progress } => *stage_progress,
        }
    }
}
