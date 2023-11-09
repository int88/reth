use super::TestTransaction;
use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::DatabaseEnv;
use reth_interfaces::{db::DatabaseError, RethError};
use reth_primitives::MAINNET;
use reth_provider::ProviderFactory;
use std::{borrow::Borrow, sync::Arc};
use tokio::sync::oneshot;

#[derive(thiserror::Error, Debug)]
pub(crate) enum TestRunnerError {
    #[error("Database error occurred.")]
    Database(#[from] DatabaseError),
    #[error("Internal runner error occurred.")]
    Internal(#[from] Box<dyn std::error::Error>),
    #[error("Internal interface error occurred.")]
    Interface(#[from] RethError),
}

/// A generic test runner for stages.
/// 对于stages的，通用的test runner
#[async_trait::async_trait]
pub(crate) trait StageTestRunner {
    type S: Stage<DatabaseEnv> + 'static;

    /// Return a reference to the database.
    /// 返回到db的引用
    fn tx(&self) -> &TestTransaction;

    /// Return an instance of a Stage.
    /// 返回一个Stage的实例
    fn stage(&self) -> Self::S;
}

#[async_trait::async_trait]
pub(crate) trait ExecuteStageTestRunner: StageTestRunner {
    type Seed: Send + Sync;

    /// Seed database for stage execution
    /// 对于stage执行，填充db
    fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError>;

    /// Validate stage execution
    /// 校验stage的执行
    fn validate_execution(
        &self,
        input: ExecInput,
        output: Option<ExecOutput>,
    ) -> Result<(), TestRunnerError>;

    /// Run [Stage::execute] and return a receiver for the result.
    /// 运行[Stage::execute]并且返回一个receiver，用于result
    fn execute(&self, input: ExecInput) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.tx().inner_raw(), self.stage());
        tokio::spawn(async move {
            let factory = ProviderFactory::new(db.as_ref(), MAINNET.clone());
            // 构建provider
            let provider = factory.provider_rw().unwrap();

            let result = stage.execute(&provider, input).await;
            // provider提交
            provider.commit().expect("failed to commit");
            tx.send(result).expect("failed to send message")
        });
        rx
    }

    /// Run a hook after [Stage::execute]. Required for Headers & Bodies stages.
    /// 在[Stage::execute]之后运行一个hook，对于Headers & Bodies stages需要
    async fn after_execution(&self, _seed: Self::Seed) -> Result<(), TestRunnerError> {
        Ok(())
    }
}

#[async_trait::async_trait]
pub(crate) trait UnwindStageTestRunner: StageTestRunner {
    /// Validate the unwind
    /// 校验unwind
    fn validate_unwind(&self, input: UnwindInput) -> Result<(), TestRunnerError>;

    /// Run [Stage::unwind] and return a receiver for the result.
    /// 运行[Stage::unwind]并且返回一个receiver，对于结果
    async fn unwind(&self, input: UnwindInput) -> Result<UnwindOutput, StageError> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.tx().inner_raw(), self.stage());
        tokio::spawn(async move {
            let factory = ProviderFactory::new(db.as_ref(), MAINNET.clone());
            let provider = factory.provider_rw().unwrap();

            // 执行unwind
            let result = stage.unwind(&provider, input).await;
            provider.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        Box::pin(rx).await.unwrap()
    }

    /// Run a hook before [Stage::unwind]. Required for MerkleStage.
    /// 运行一个hook，在[Stage::unwind]之后，对于MerkleStage需要
    fn before_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
        Ok(())
    }
}
