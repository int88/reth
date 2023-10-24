use crate::engine::forkchoice::ForkchoiceStatus;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{SealedBlock, SealedHeader};
use std::{sync::Arc, time::Duration};

/// Events emitted by [crate::BeaconConsensusEngine].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated.
    /// fork choice state被更新
    ForkchoiceUpdated(ForkchoiceState, ForkchoiceStatus),
    /// A block was added to the canonical chain.
    /// 一个block被添加到canonical chain
    CanonicalBlockAdded(Arc<SealedBlock>),
    /// A canonical chain was committed.
    /// canonical chain被提交
    CanonicalChainCommitted(SealedHeader, Duration),
    /// A block was added to the fork chain.
    /// 一个block被添加到fork chain
    ForkBlockAdded(Arc<SealedBlock>),
}
