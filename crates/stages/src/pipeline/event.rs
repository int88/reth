use crate::stage::{ExecOutput, UnwindInput, UnwindOutput};
use reth_primitives::stage::{StageCheckpoint, StageId};

/// An event emitted by a [Pipeline][crate::Pipeline].
/// 由Pipeline发出的一个event
///
/// It is possible for multiple of these events to be emitted over the duration of a pipeline's
/// execution since:
/// 有可能在pipeline的执行期间，多个这样的events被发出
///
/// - Other stages may ask the pipeline to unwind
/// - 其他的stages可能会要求pipeline去unwind
/// - The pipeline will loop indefinitely unless a target block is set
/// - pipeline将会无限循环，除非设置了一个target block
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PipelineEvent {
    /// Emitted when a stage is about to be run.
    /// 当一个stage准备被run的时候，发出
    Running {
        /// 1-indexed ID of the stage that is about to be run out of total stages in the pipeline.
        pipeline_position: usize,
        /// Total number of stages in the pipeline.
        /// pipeline中的stage的总和
        pipeline_total: usize,
        /// The stage that is about to be run.
        /// 准备运行的stage
        stage_id: StageId,
        /// The previous checkpoint of the stage.
        /// stage之前的checkpoint
        checkpoint: Option<StageCheckpoint>,
    },
    /// Emitted when a stage has run a single time.
    /// 当一个stage运行了一次的时候，发出
    Ran {
        /// 1-indexed ID of the stage that was run out of total stages in the pipeline.
        /// 1-索引的stage的ID，这个stage是pipeline中所有stages的总和
        pipeline_position: usize,
        /// Total number of stages in the pipeline.
        /// pipeline中的所有stages的总和
        pipeline_total: usize,
        /// The stage that was run.
        /// 正在运行的stage
        stage_id: StageId,
        /// The result of executing the stage.
        /// 执行stage的结果
        result: ExecOutput,
    },
    /// Emitted when a stage is about to be unwound.
    /// 当一个stage准备被unwind的时候，发出
    Unwinding {
        /// The stage that is about to be unwound.
        stage_id: StageId,
        /// The unwind parameters.
        input: UnwindInput,
    },
    /// Emitted when a stage has been unwound.
    /// 当一个stage已经被unwind的时候，发出
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
    /// 当因为stage的运行条件没有被满足，而stage被跳过的时候，发出
    ///
    /// - The stage might have progressed beyond the point of our target block
    /// - The stage might not need to be unwound since it has not progressed past the unwind target
    /// - The stage requires that the pipeline has reached the tip, but it has not done so yet
    Skipped {
        /// The stage that was skipped.
        stage_id: StageId,
    },
}
