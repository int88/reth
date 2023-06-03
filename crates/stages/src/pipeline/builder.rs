use crate::{pipeline::BoxedStage, Pipeline, Stage, StageSet};
use reth_db::database::Database;
use reth_primitives::{stage::StageId, BlockNumber, H256};
use tokio::sync::watch;

/// Builds a [`Pipeline`].
#[must_use = "call `build` to construct the pipeline"]
pub struct PipelineBuilder<DB>
where
    DB: Database,
{
    /// All configured stages in the order they will be executed.
    /// 所有配置的stages，按照它们将被执行的顺序
    stages: Vec<BoxedStage<DB>>,
    /// The maximum block number to sync to.
    /// 同步的最大数目的block
    max_block: Option<BlockNumber>,
    /// A receiver for the current chain tip to sync to.
    /// 一个receiver，用于同步到当前的chain tip
    tip_tx: Option<watch::Sender<H256>>,
}

impl<DB> PipelineBuilder<DB>
where
    DB: Database,
{
    /// Add a stage to the pipeline.
    pub fn add_stage<S>(mut self, stage: S) -> Self
    where
        S: Stage<DB> + 'static,
    {
        self.stages.push(Box::new(stage));
        self
    }

    /// Add a set of stages to the pipeline.
    /// 添加一系列的stages到pipeline
    ///
    /// Stages can be grouped into a set by using a [`StageSet`].
    /// Stages可以被分组到一个set中，通过使用一个StageSet
    ///
    /// To customize the stages in the set (reorder, disable, insert a stage) call
    /// [`builder`][StageSet::builder] on the set which will convert it to a
    /// [`StageSetBuilder`][crate::StageSetBuilder].
    pub fn add_stages<Set: StageSet<DB>>(mut self, set: Set) -> Self {
        for stage in set.builder().build() {
            self.stages.push(stage);
        }
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, the pipeline will stop.
    pub fn with_max_block(mut self, block: BlockNumber) -> Self {
        self.max_block = Some(block);
        self
    }

    /// Set the tip sender.
    pub fn with_tip_sender(mut self, tip_tx: watch::Sender<H256>) -> Self {
        self.tip_tx = Some(tip_tx);
        self
    }

    /// Builds the final [`Pipeline`] using the given database.
    ///
    /// Note: it's expected that this is either an [Arc](std::sync::Arc) or an Arc wrapper type.
    pub fn build(self, db: DB) -> Pipeline<DB> {
        let Self { stages, max_block, tip_tx } = self;
        Pipeline {
            db,
            stages,
            max_block,
            tip_tx,
            listeners: Default::default(),
            progress: Default::default(),
            metrics: Default::default(),
        }
    }
}

impl<DB: Database> Default for PipelineBuilder<DB> {
    fn default() -> Self {
        Self { stages: Vec::new(), max_block: None, tip_tx: None }
    }
}

impl<DB: Database> std::fmt::Debug for PipelineBuilder<DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineBuilder")
            .field("stages", &self.stages.iter().map(|stage| stage.id()).collect::<Vec<StageId>>())
            .field("max_block", &self.max_block)
            .finish()
    }
}
