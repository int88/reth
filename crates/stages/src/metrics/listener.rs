use crate::metrics::SyncMetrics;
use reth_primitives::{
    constants::MGAS_TO_GAS,
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::trace;

/// Alias type for metric producers to use.
/// 让metric producers使用的alias类型
pub type MetricEventsSender = UnboundedSender<MetricEvent>;

/// Collection of metric events.
/// 一系列的metric事件
#[derive(Clone, Copy, Debug)]
pub enum MetricEvent {
    /// Sync reached new height. All stage checkpoints are updated.
    /// Sync到达了新的height，所有的stage checkpoints被更新
    SyncHeight {
        /// Maximum height measured in block number that sync reached.
        /// 衡量sync到达的最大的height
        height: BlockNumber,
    },
    /// Stage reached new checkpoint.
    /// Stage到达的新的checkpoint
    StageCheckpoint {
        /// Stage ID.
        stage_id: StageId,
        /// Stage checkpoint.
        checkpoint: StageCheckpoint,
        /// Maximum known block number reachable by this stage.
        /// 这个stage到达的最高已知的block number
        /// If specified, `entities_total` metric is updated.
        max_block_number: Option<BlockNumber>,
    },
    /// Execution stage processed some amount of gas.
    /// Execution stage消耗了一些gas
    ExecutionStageGas {
        /// Gas processed.
        gas: u64,
    },
}

/// Metrics routine that listens to new metric events on the `events_rx` receiver.
/// Upon receiving new event, related metrics are updated.
#[derive(Debug)]
pub struct MetricsListener {
    events_rx: UnboundedReceiver<MetricEvent>,
    pub(crate) sync_metrics: SyncMetrics,
}

impl MetricsListener {
    /// Creates a new [MetricsListener] with the provided receiver of [MetricEvent].
    pub fn new(events_rx: UnboundedReceiver<MetricEvent>) -> Self {
        Self { events_rx, sync_metrics: SyncMetrics::default() }
    }

    fn handle_event(&mut self, event: MetricEvent) {
        trace!(target: "sync::metrics", ?event, "Metric event received");
        match event {
            MetricEvent::SyncHeight { height } => {
                for stage_id in StageId::ALL {
                    self.handle_event(MetricEvent::StageCheckpoint {
                        stage_id,
                        checkpoint: StageCheckpoint {
                            block_number: height,
                            stage_checkpoint: None,
                        },
                        max_block_number: Some(height),
                    });
                }
            }
            MetricEvent::StageCheckpoint { stage_id, checkpoint, max_block_number } => {
                let stage_metrics = self.sync_metrics.get_stage_metrics(stage_id);

                stage_metrics.checkpoint.set(checkpoint.block_number as f64);

                let (processed, total) = match checkpoint.entities() {
                    Some(entities) => (entities.processed, Some(entities.total)),
                    None => (checkpoint.block_number, max_block_number),
                };

                stage_metrics.entities_processed.set(processed as f64);

                if let Some(total) = total {
                    stage_metrics.entities_total.set(total as f64);
                }
            }
            MetricEvent::ExecutionStageGas { gas } => self
                .sync_metrics
                .execution_stage
                .mgas_processed_total
                .increment(gas as f64 / MGAS_TO_GAS as f64),
        }
    }
}

impl Future for MetricsListener {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Loop until we drain the `events_rx` channel
        loop {
            let Some(event) = ready!(this.events_rx.poll_recv(cx)) else {
                // Channel has closed
                return Poll::Ready(())
            };

            this.handle_event(event);
        }
    }
}
