use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::SealedBlock;
use std::sync::Arc;

/// Events emitted by [crate::BeaconConsensusEngine].
#[derive(Clone, Debug)]
pub enum BeaconConsensusEngineEvent {
    /// The fork choice state was updated.
    /// fork choice state被更新
    ForkchoiceUpdated(ForkchoiceState),
    /// A block was added to the canonical chain.
    /// 一个block被添加到canonical chain
    CanonicalBlockAdded(Arc<SealedBlock>),
    /// A block was added to the fork chain.
    /// 一个block被添加到fork chain
    ForkBlockAdded(Arc<SealedBlock>),
}
