use async_trait::async_trait;
use reth_primitives::{
    BlockHash, BlockNumber, Header, InvalidTransactionError, SealedBlock, SealedHeader, H256, U256,
};
use std::fmt::Debug;

/// Re-export fork choice state
pub use reth_rpc_types::engine::ForkchoiceState;

/// Consensus is a protocol that chooses canonical chain.
/// Consensus是一个协议，用于选择canonical chain
#[async_trait]
#[auto_impl::auto_impl(&, Arc)]
pub trait Consensus: Debug + Send + Sync {
    /// Validate if header is correct and follows consensus specification.
    /// 校验header是否正确，是否遵循共识规范
    ///
    /// This is called on standalone header to check if all hashes are correct.
    /// 这在独立的header上调用，以检查所有的hash是否正确
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError>;

    /// Validate that the header information regarding parent are correct.
    /// 校验header关于parent的信息是否正确
    /// This checks the block number, timestamp, basefee and gas limit increment.
    /// 这检查block number、timestamp、basefee和gas限制增量
    ///
    /// This is called before properties that are not in the header itself (like total difficulty)
    /// have been computed.
    /// 这在计算header本身没有的属性（如总难度）之前调用
    ///
    /// **This should not be called for the genesis block**.
    ///
    /// Note: Validating header against its parent does not include other Consensus validations.
    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError>;

    /// Validate if the header is correct and follows the consensus specification, including
    /// computed properties (like total difficulty).
    /// 校验是否header正确，是否遵循共识规范，包括计算属性（如总难度）
    ///
    /// Some consensus engines may want to do additional checks here.
    /// 有的共识引擎可能想在这里做额外的检查
    ///
    /// Note: validating headers with TD does not include other Consensus validation.
    /// 注意：校验带有TD的headers不包括其他共识校验
    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError>;

    /// Validate a block disregarding world state, i.e. things that can be checked before sender
    /// recovery and execution.
    /// 校验一个block，忽略world state，即在sender recovery和execution之前可以检查的东西
    ///
    /// See the Yellow Paper sections 4.3.2 "Holistic Validity", 4.3.4 "Block Header Validity", and
    /// 11.1 "Ommer Validation".
    ///
    /// **This should not be called for the genesis block**.
    ///
    /// Note: validating blocks does not include other validations of the Consensus
    fn validate_block(&self, block: &SealedBlock) -> Result<(), ConsensusError>;
}

/// Consensus Errors
#[allow(missing_docs)]
#[derive(thiserror::Error, Debug, PartialEq, Eq, Clone)]
pub enum ConsensusError {
    #[error("Block used gas ({gas_used}) is greater than gas limit ({gas_limit}).")]
    HeaderGasUsedExceedsGasLimit { gas_used: u64, gas_limit: u64 },
    #[error("Block ommer hash ({got:?}) is different from expected: ({expected:?})")]
    BodyOmmersHashDiff { got: H256, expected: H256 },
    #[error("Block state root ({got:?}) is different from expected: ({expected:?})")]
    BodyStateRootDiff { got: H256, expected: H256 },
    #[error("Block transaction root ({got:?}) is different from expected ({expected:?})")]
    BodyTransactionRootDiff { got: H256, expected: H256 },
    #[error("Block withdrawals root ({got:?}) is different from expected ({expected:?})")]
    BodyWithdrawalsRootDiff { got: H256, expected: H256 },
    #[error("Block with [hash:{hash:?},number: {number}] is already known.")]
    BlockKnown { hash: BlockHash, number: BlockNumber },
    #[error("Block parent [hash:{hash:?}] is not known.")]
    ParentUnknown { hash: BlockHash },
    #[error(
        "Block number {block_number} is mismatch with parent block number {parent_block_number}"
    )]
    ParentBlockNumberMismatch { parent_block_number: BlockNumber, block_number: BlockNumber },
    #[error(
    "Block timestamp {timestamp} is in the past compared to the parent timestamp {parent_timestamp}."
    )]
    TimestampIsInPast { parent_timestamp: u64, timestamp: u64 },
    #[error("Block timestamp {timestamp} is in the future compared to our clock time {present_timestamp}.")]
    TimestampIsInFuture { timestamp: u64, present_timestamp: u64 },
    #[error("Child gas_limit {child_gas_limit} max increase is {parent_gas_limit}/1024.")]
    GasLimitInvalidIncrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Child gas_limit {child_gas_limit} max decrease is {parent_gas_limit}/1024.")]
    GasLimitInvalidDecrease { parent_gas_limit: u64, child_gas_limit: u64 },
    #[error("Base fee missing.")]
    BaseFeeMissing,
    #[error("Block base fee ({got}) is different than expected: ({expected}).")]
    BaseFeeDiff { expected: u64, got: u64 },
    #[error("Transaction signer recovery error.")]
    TransactionSignerRecoveryError,
    #[error("Extra data {len} exceeds max length: ")]
    ExtraDataExceedsMax { len: usize },
    #[error("Difficulty after merge is not zero")]
    TheMergeDifficultyIsNotZero,
    #[error("Nonce after merge is not zero")]
    TheMergeNonceIsNotZero,
    #[error("Ommer root after merge is not empty")]
    TheMergeOmmerRootIsNotEmpty,
    #[error("Missing withdrawals root")]
    WithdrawalsRootMissing,
    #[error("Unexpected withdrawals root")]
    WithdrawalsRootUnexpected,
    #[error("Withdrawal index #{got} is invalid. Expected: #{expected}.")]
    WithdrawalIndexInvalid { got: u64, expected: u64 },
    #[error("Missing withdrawals")]
    BodyWithdrawalsMissing,
    /// Error for a transaction that violates consensus.
    #[error(transparent)]
    InvalidTransaction(#[from] InvalidTransactionError),
}
