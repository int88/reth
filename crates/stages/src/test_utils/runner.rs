use super::TestTransaction;
use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::mdbx::{Env, WriteMap};
use reth_primitives::MAINNET;
use reth_provider::ProviderFactory;
use std::{borrow::Borrow, sync::Arc};
use tokio::sync::oneshot;

#[derive(thiserror::Error, Debug)]
pub(crate) enum TestRunnerError {
    #[error("Database error occurred.")]
    Database(#[from] reth_interfaces::db::DatabaseError),
    #[error("Internal runner error occurred.")]
    Internal(#[from] Box<dyn std::error::Error>),
    #[error("Internal interface error occurred.")]
    Interface(#[from] reth_interfaces::Error),
}

/// A generic test runner for stages.
/// 一个通用的test runner，对于stages
#[async_trait::async_trait]
pub(crate) trait StageTestRunner {
    type S: Stage<Env<WriteMap>> + 'static;

    /// Return a reference to the database.
    /// 返回对于数据库的引用
    fn tx(&self) -> &TestTransaction;

    /// Return an instance of a Stage.
    /// 返回一个Stage的实例
    fn stage(&self) -> Self::S;
}

#[async_trait::async_trait]
pub(crate) trait ExecuteStageTestRunner: StageTestRunner {
    type Seed: Send + Sync;

    /// Seed database for stage execution
    /// 用于stage执行的seed database
    fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError>;

    /// Validate stage execution
    /// 校验stage execution
    fn validate_execution(
        &self,
        input: ExecInput,
        output: Option<ExecOutput>,
    ) -> Result<(), TestRunnerError>;

    /// Run [Stage::execute] and return a receiver for the result.
    /// 运行Stage::execute并且返回一个接收器，用于结果
    fn execute(&self, input: ExecInput) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.tx().inner_raw(), self.stage());
        tokio::spawn(async move {
            let factory = ProviderFactory::new(db.as_ref(), MAINNET.clone());
            let mut provider = factory.provider_rw().unwrap();

            let result = stage.execute(&mut provider, input).await;
            provider.commit().expect("failed to commit");
            tx.send(result).expect("failed to send message")
        });
        rx
    }

    /// Run a hook after [Stage::execute]. Required for Headers & Bodies stages.
    /// 在Stage::execute之后运行一个hook，对于Headers & Bodies stages是必须的
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
    /// 运行[Stage::unwind]并且返回一个接收器，用于结果
    async fn unwind(&self, input: UnwindInput) -> Result<UnwindOutput, StageError> {
        let (tx, rx) = oneshot::channel();
        let (db, mut stage) = (self.tx().inner_raw(), self.stage());
        tokio::spawn(async move {
            let factory = ProviderFactory::new(db.as_ref(), MAINNET.clone());
            let mut provider = factory.provider_rw().unwrap();

            let result = stage.unwind(&mut provider, input).await;
            provider.commit().expect("failed to commit");
            tx.send(result).expect("failed to send result");
        });
        Box::pin(rx).await.unwrap()
    }

    /// Run a hook before [Stage::unwind]. Required for MerkleStage.
    /// 运行一个hook，在Stage::unwind之前，对于MerkleStage是必须的
    fn before_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
        Ok(())
    }
}
