//! Prune management for the engine implementation.
//! 对于engine实现的Prune management

use futures::FutureExt;
use reth_db::database::Database;
use reth_primitives::BlockNumber;
use reth_prune::{Pruner, PrunerError, PrunerWithResult};
use reth_tasks::TaskSpawner;
use std::task::{ready, Context, Poll};
use tokio::sync::oneshot;

/// Manages pruning under the control of the engine.
/// 管理pruning，在engine的控制之下
///
/// This type controls the [Pruner].
pub(crate) struct EnginePruneController<DB> {
    /// The current state of the pruner.
    /// pruner的当前状态
    pruner_state: PrunerState<DB>,
    /// The type that can spawn the pruner task.
    /// 可以生成pruner task的类型
    pruner_task_spawner: Box<dyn TaskSpawner>,
}

impl<DB: Database + 'static> EnginePruneController<DB> {
    /// Create a new instance
    /// 创建一个新的实例
    pub(crate) fn new(pruner: Pruner<DB>, pruner_task_spawner: Box<dyn TaskSpawner>) -> Self {
        Self { pruner_state: PrunerState::Idle(Some(pruner)), pruner_task_spawner }
    }

    /// Returns `true` if the pruner is idle.
    /// 返回`true`，如果pruner处于idle
    pub(crate) fn is_pruner_idle(&self) -> bool {
        self.pruner_state.is_idle()
    }

    /// Advances the pruner state.
    /// 推进pruner state
    ///
    /// This checks for the result in the channel, or returns pending if the pruner is idle.
    /// 检查channel中的结果，或者返回pending，如果pruner是idle
    fn poll_pruner(&mut self, cx: &mut Context<'_>) -> Poll<EnginePruneEvent> {
        let res = match self.pruner_state {
            PrunerState::Idle(_) => return Poll::Pending,
            PrunerState::Running(ref mut fut) => {
                ready!(fut.poll_unpin(cx))
            }
        };
        let ev = match res {
            Ok((pruner, result)) => {
                self.pruner_state = PrunerState::Idle(Some(pruner));
                EnginePruneEvent::Finished { result }
            }
            Err(_) => {
                // failed to receive the pruner
                EnginePruneEvent::TaskDropped
            }
        };
        Poll::Ready(ev)
    }

    /// This will try to spawn the pruner if it is idle:
    /// 这会试着生成pruner，如果它处于idle状态
    /// 1. Check if pruning is needed through [Pruner::is_pruning_needed].
    /// 1. 检查是否需要pruning，通过[Pruner::is_pruning_needed]
    /// 2a. If pruning is needed, pass tip block number to the [Pruner::run] and spawn it in a
    /// separate task. Set pruner state to [PrunerState::Running].
    /// 2a. 如果pruning需要，传递tip block
    /// number到[Pruner::run]并且在一个独立的task生成，设置pruner状态为 [PrunerState::Running]
    /// 2b. If pruning is not needed, set pruner state back to [PrunerState::Idle].
    /// 2b. 如果pruning不需要，设置pruner state到[PrunnerState::Idle]
    ///
    /// If pruner is already running, do nothing.
    /// 如果pruner已经在运行了，则什么都不做
    fn try_spawn_pruner(&mut self, tip_block_number: BlockNumber) -> Option<EnginePruneEvent> {
        match &mut self.pruner_state {
            PrunerState::Idle(pruner) => {
                let mut pruner = pruner.take()?;

                // Check tip for pruning
                // 检查tip用于pruning
                if pruner.is_pruning_needed(tip_block_number) {
                    let (tx, rx) = oneshot::channel();
                    self.pruner_task_spawner.spawn_critical_blocking(
                        "pruner task",
                        Box::pin(async move {
                            let result = pruner.run(tip_block_number);
                            let _ = tx.send((pruner, result));
                        }),
                    );
                    self.pruner_state = PrunerState::Running(rx);

                    Some(EnginePruneEvent::Started(tip_block_number))
                } else {
                    self.pruner_state = PrunerState::Idle(Some(pruner));
                    Some(EnginePruneEvent::NotReady)
                }
            }
            PrunerState::Running(_) => None,
        }
    }

    /// Advances the prune process with the tip block number.
    /// 推进prune进程，用tip block number
    pub(crate) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        tip_block_number: BlockNumber,
    ) -> Poll<EnginePruneEvent> {
        // Try to spawn a pruner
        // 试着生成一个pruner
        match self.try_spawn_pruner(tip_block_number) {
            Some(EnginePruneEvent::NotReady) => return Poll::Pending,
            Some(event) => return Poll::Ready(event),
            None => (),
        }

        // Poll pruner and check its status
        // 轮询pruner并且检查它的状态
        self.poll_pruner(cx)
    }
}

/// The event type emitted by the [EnginePruneController].
#[derive(Debug)]
pub(crate) enum EnginePruneEvent {
    /// Pruner is not ready
    NotReady,
    /// Pruner started with tip block number
    Started(BlockNumber),
    /// Pruner finished
    ///
    /// If this is returned, the pruner is idle.
    Finished {
        /// Final result of the pruner run.
        result: Result<(), PrunerError>,
    },
    /// Pruner task was dropped after it was started, unable to receive it because channel
    /// closed. This would indicate a panicked pruner task
    TaskDropped,
}

/// The possible pruner states within the sync controller.
///
/// [PrunerState::Idle] means that the pruner is currently idle.
/// [PrunerState::Running] means that the pruner is currently running.
///
/// NOTE: The differentiation between these two states is important, because when the pruner is
/// running, it acquires the write lock over the database. This means that we cannot forward to the
/// blockchain tree any messages that would result in database writes, since it would result in a
/// deadlock.
enum PrunerState<DB> {
    /// Pruner is idle.
    Idle(Option<Pruner<DB>>),
    /// Pruner is running and waiting for a response
    Running(oneshot::Receiver<PrunerWithResult<DB>>),
}

impl<DB> PrunerState<DB> {
    /// Returns `true` if the state matches idle.
    fn is_idle(&self) -> bool {
        matches!(self, PrunerState::Idle(_))
    }
}
