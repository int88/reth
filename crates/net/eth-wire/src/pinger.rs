use crate::errors::PingerError;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval, Sleep};
use tokio_stream::Stream;

/// The pinger is a state machine that is created with a maximum number of pongs that can be
/// missed.
/// pinger是一个状态机，被创建有着最大可以缺失的pongs
#[derive(Debug)]
pub(crate) struct Pinger {
    /// The timer used for the next ping.
    /// 用于下一次Ping的时间
    ping_interval: Interval,
    /// The timer used for the next ping.
    /// ping的超时时间
    timeout_timer: Pin<Box<Sleep>>,
    /// The timeout duration for each ping.
    timeout: Duration,
    /// Keeps track of the state
    state: PingState,
}

// === impl Pinger ===

impl Pinger {
    /// Creates a new [`Pinger`] with the given ping interval duration,
    /// and timeout duration.
    /// 创建一个新的[`Pinger`]用给定的ping interval duration，以及超时的duration
    pub(crate) fn new(ping_interval: Duration, timeout_duration: Duration) -> Self {
        let now = Instant::now();
        let timeout_timer = tokio::time::sleep(timeout_duration);
        Self {
            // 初始状态设置为Ready
            state: PingState::Ready,
            ping_interval: tokio::time::interval_at(now + ping_interval, ping_interval),
            timeout_timer: Box::pin(timeout_timer),
            timeout: timeout_duration,
        }
    }

    /// Mark a pong as received, and transition the pinger to the `Ready` state if it was in the
    /// `WaitingForPong` state. Unsets the sleep timer.
    /// 将pong标记为接收，并且转换pinger为`Ready`状态，如果它处于`WaitingForPong`的状态，
    /// 重新设置sleep timer
    pub(crate) fn on_pong(&mut self) -> Result<(), PingerError> {
        match self.state {
            PingState::Ready => Err(PingerError::UnexpectedPong),
            PingState::WaitingForPong => {
                // 状态重新置为Ready
                self.state = PingState::Ready;
                self.ping_interval.reset();
                Ok(())
            }
            PingState::TimedOut => {
                // if we receive a pong after timeout then we also reset the state, since the
                // connection was kept alive after timeout
                self.state = PingState::Ready;
                self.ping_interval.reset();
                Ok(())
            }
        }
    }

    /// Returns the current state of the pinger.
    pub(crate) const fn state(&self) -> PingState {
        self.state
    }

    /// Polls the state of the pinger and returns whether a new ping needs to be sent or if a
    /// previous ping timed out.
    /// 轮询pinger的状态并且返回是否一个新的ping需要被发送，或者之前的一个ping已经超时了
    pub(crate) fn poll_ping(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<PingerEvent, PingerError>> {
        match self.state() {
            PingState::Ready => {
                if self.ping_interval.poll_tick(cx).is_ready() {
                    // ping_interval ready之后才运行
                    self.timeout_timer.as_mut().reset(Instant::now() + self.timeout);
                    self.state = PingState::WaitingForPong;
                    return Poll::Ready(Ok(PingerEvent::Ping))
                }
            }
            PingState::WaitingForPong => {
                if self.timeout_timer.is_elapsed() {
                    // 设置状态为超时
                    self.state = PingState::TimedOut;
                    return Poll::Ready(Ok(PingerEvent::Timeout))
                }
            }
            PingState::TimedOut => {
                // we treat continuous calls while in timeout as pending, since the connection is
                // not yet terminated
                // 在超时状态下，我们将其视为Pending，因为连接尚未终止
                return Poll::Pending
            }
        };
        Poll::Pending
    }
}

impl Stream for Pinger {
    type Item = Result<PingerEvent, PingerError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_ping(cx).map(Some)
    }
}

/// This represents the possible states of the pinger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PingState {
    /// There are no pings in flight, or all pings have been responded to, and we are ready to send
    /// a ping at a later point.
    /// 没有pings in flight，或者所有的pings都已经被回复，并且我们准备好后面发送一个ping
    Ready,
    /// We have sent a ping and are waiting for a pong, but the peer has missed n pongs.
    /// 我们已经发送了一个ping并且等待一个pong，但是peer已经丢失了n个pongs
    WaitingForPong,
    /// The peer has failed to respond to a ping.
    /// peer回复pong失败
    TimedOut,
}

/// The element type produced by a [`Pinger`], representing either a new
/// [`Ping`](super::P2PMessage::Ping)
/// 代表一个[`Pinger`]产生的元素类型，代表一个新的[`Ping`](super::P2PMessage::Ping)要发送
/// message to send, or an indication that the peer should be timed out.
/// 或者表明peer应该超时
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PingerEvent {
    /// A new [`Ping`](super::P2PMessage::Ping) message should be sent.
    /// 一个新的[`Ping`](super::P2PMessage::Ping)消息应该被发送
    Ping,

    /// The peer should be timed out.
    /// peer应该超时
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_ping_timeout() {
        let interval = Duration::from_millis(300);
        // we should wait for the interval to elapse and receive a pong before the timeout elapses
        // 我们应该等待interval过去，并在超时之前收到pong
        let mut pinger = Pinger::new(interval, Duration::from_millis(20));
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
        pinger.on_pong().unwrap();
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);

        tokio::time::sleep(interval).await;
        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Timeout);
        pinger.on_pong().unwrap();

        assert_eq!(pinger.next().await.unwrap().unwrap(), PingerEvent::Ping);
    }
}
