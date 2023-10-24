use crate::stage::{ExecOutput, UnwindInput, UnwindOutput};
use reth_primitives::stage::{StageCheckpoint, StageId};

/// An event emitted by a [Pipeline][crate::Pipeline].
/// 由[Pipeline][crate::Pipeline]发出的一个event
///
/// It is possible for multiple of these events to be emitted over the duration of a pipeline's
/// execution since:
/// 可能在一个pipeline的执行过程中有多个events发射出来
///
/// - Other stages may ask the pipeline to unwind
/// - 其他stages可能让pipeline unwind
/// - The pipeline will loop indefinitely unless a target block is set
/// - pipeline会无限执行，除非设置了一个target block
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PipelineEvent {
    /// Emitted when a stage is about to be run.
    /// 当一个stage准备开始运行的时候被发射
    Running {
        /// 1-indexed ID of the stage that is about to be run out of total stages in the pipeline.
        pipeline_position: usize,
        /// Total number of stages in the pipeline.
        /// pipeline中stages的数目
        pipeline_total: usize,
        /// The stage that is about to be run.
        /// 准备运行的stage
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        /// stage之前的checkpoint
        checkpoint: Option<StageCheckpoint>,
    },
    /// Emitted when a stage has run a single time.
    /// 当一个stage运行一次之后发出
    Ran {
        /// 1-indexed ID of the stage that was run out of total stages in the pipeline.
        pipeline_position: usize,
        /// Total number of stages in the pipeline.
        pipeline_total: usize,
        /// The stage that was run.
        stage_id: StageId,
        /// The result of executing the stage.
        result: ExecOutput,
    },
    /// Emitted when a stage is about to be unwound.
    Unwinding {
        /// The stage that is about to be unwound.
        stage_id: StageId,
        /// The unwind parameters.
        input: UnwindInput,
    },
    /// Emitted when a stage has been unwound.
    Unwound {
        /// The stage that was unwound.
        stage_id: StageId,
        /// The result of unwinding the stage.
        result: UnwindOutput,
    },
    /// Emitted when a stage encounters an error either during execution or unwinding.
    Error {
        /// The stage that encountered an error.
        stage_id: StageId,
    },
    /// Emitted when a stage was skipped due to it's run conditions not being met:
    /// 当一个stage被跳过的时候被发射，可能是他的运行条件没被满足
    ///
    /// - The stage might have progressed beyond the point of our target block
    /// - stage可能可能已经处理超过了我们的target block
    /// - The stage might not need to be unwound since it has not progressed past the unwind target
    /// - stage可能不需要unwind，因为它还没超过unwind target
    /// - The stage requires that the pipeline has reached the tip, but it has not done so yet
    /// - stage需要pipeline运行到tip，但是他还没有完成
    Skipped {
        /// The stage that was skipped.
        stage_id: StageId,
    },
}
