use reth_metrics::{
    metrics::{self, Counter},
    Metrics,
};

/// Beacon consensus engine metrics.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct Metrics {
    /// The number of times the pipeline was run.
    /// pipeline运行的次数
    pub(crate) pipeline_runs: Counter,
    /// The total count of forkchoice updated messages received.
    /// 接收到的forkchoice updated messages的总数
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of new payload messages received.
    /// 接收到的new payload messages的总数
    pub(crate) new_payload_messages: Counter,
}
