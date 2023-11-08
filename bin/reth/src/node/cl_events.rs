//! Events related to Consensus Layer health.
//! 跟Consensus Layer health相关的Events

use futures::Stream;
use reth_provider::CanonChainTracker;
use std::{
    fmt,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval};

/// Interval of checking Consensus Layer client health.
const CHECK_INTERVAL: Duration = Duration::from_secs(300);
/// Period of not exchanging transition configurations with Consensus Layer client,
/// after which the warning is issued.
const NO_TRANSITION_CONFIG_EXCHANGED_PERIOD: Duration = Duration::from_secs(120);
/// Period of not receiving fork choice updates from Consensus Layer client,
/// after which the warning is issued.
/// 从Consensus Layer client没有接收到fcu的时间段，之后会发出warning
const NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD: Duration = Duration::from_secs(120);

/// A Stream of [ConsensusLayerHealthEvent].
/// [ConsensusLayerHealthEvent]的一个Stream
pub struct ConsensusLayerHealthEvents {
    interval: Interval,
    canon_chain: Box<dyn CanonChainTracker>,
}

impl fmt::Debug for ConsensusLayerHealthEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsensusLayerHealthEvents").field("interval", &self.interval).finish()
    }
}

impl ConsensusLayerHealthEvents {
    /// Creates a new [ConsensusLayerHealthEvents] with the given canonical chain tracker.
    /// 创建一个新的[ConsensusLayerHealthEvents]，用给定的canonical chain tracker
    pub fn new(canon_chain: Box<dyn CanonChainTracker>) -> Self {
        // Skip the first tick to prevent the false `ConsensusLayerHealthEvent::NeverSeen` event.
        // 跳过第一个tick来防止假的`ConsensusLayerHealthEvent::NeverSeen`事件
        let interval = tokio::time::interval_at(Instant::now() + CHECK_INTERVAL, CHECK_INTERVAL);
        Self { interval, canon_chain }
    }
}

impl Stream for ConsensusLayerHealthEvents {
    type Item = ConsensusLayerHealthEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            ready!(this.interval.poll_tick(cx));

            if let Some(fork_choice) = this.canon_chain.last_received_update_timestamp() {
                if fork_choice.elapsed() <= NO_FORKCHOICE_UPDATE_RECEIVED_PERIOD {
                    // We had an FCU, and it's recent. CL is healthy.
                    // 我们有一个FCU，并且是最近的，CL是健康的
                    continue
                } else {
                    // We had an FCU, but it's too old.
                    // 我们有一个FCU，但是太老了
                    return Poll::Ready(Some(
                        ConsensusLayerHealthEvent::HaveNotReceivedUpdatesForAWhile(
                            fork_choice.elapsed(),
                        ),
                    ))
                }
            }

            if let Some(transition_config) =
                this.canon_chain.last_exchanged_transition_configuration_timestamp()
            {
                if transition_config.elapsed() <= NO_TRANSITION_CONFIG_EXCHANGED_PERIOD {
                    // We never had an FCU, but had a transition config exchange, and it's recent.
                    // 我们从没有FCU，但是有一个transition config exchange，这是最近的
                    return Poll::Ready(Some(ConsensusLayerHealthEvent::NeverReceivedUpdates))
                } else {
                    // We never had an FCU, but had a transition config exchange, but it's too old.
                    // 我们从没有FCU，但是有一个transition config exchange，但是太老了
                    return Poll::Ready(Some(ConsensusLayerHealthEvent::HasNotBeenSeenForAWhile(
                        transition_config.elapsed(),
                    )))
                }
            }

            // We never had both FCU and transition config exchange.
            // 我们从没有过FCU和transition config exchange
            return Poll::Ready(Some(ConsensusLayerHealthEvent::NeverSeen))
        }
    }
}

/// Event that is triggered when Consensus Layer health is degraded from the
/// Execution Layer point of view.
/// 被触发的Event，当Consensus Layer health被降级，在Execution Layer看来
#[derive(Clone, Copy, Debug)]
pub enum ConsensusLayerHealthEvent {
    /// Consensus Layer client was never seen.
    /// Consensus Layer Client从未看到过
    NeverSeen,
    /// Consensus Layer client has not been seen for a while.
    /// Consensus Layer client已经有一段时间未见
    HasNotBeenSeenForAWhile(Duration),
    /// Updates from the Consensus Layer client were never received.
    /// 来自Consensu Layer client的更新从未被接收
    NeverReceivedUpdates,
    /// Updates from the Consensus Layer client have not been received for a while.
    /// 来自Consensus Layer client的更新有一段时间未被接收到
    HaveNotReceivedUpdatesForAWhile(Duration),
}
