use reth_primitives::{BlockNumber, PrunePart};
use std::{collections::BTreeMap, time::Duration};

/// An event emitted by a [Pruner][crate::Pruner].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PrunerEvent {
    /// Emitted when pruner finished running.
    /// 当pruner结束运行的时候被发射
    Finished {
        tip_block_number: BlockNumber,
        elapsed: Duration,
        done: bool,
        parts_done: BTreeMap<PrunePart, bool>,
    },
}
