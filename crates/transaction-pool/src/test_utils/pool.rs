//! Test helpers for mocking an entire pool.
//! Test helpers用于mocking一个完整的pool

use crate::{
    error::PoolResult,
    pool::{txpool::TxPool, AddedTransaction},
    test_utils::{
        MockOrdering, MockTransaction, MockTransactionDistribution, MockTransactionFactory,
    },
    TransactionOrdering,
};
use rand::Rng;
use reth_primitives::{Address, U128, U256};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// A wrapped `TxPool` with additional helpers for testing
/// 对于`TxPool`的封装，有额外的helpers用于测试
pub struct MockPool<T: TransactionOrdering = MockOrdering> {
    // The wrapped pool.
    pool: TxPool<T>,
}

impl MockPool {
    /// The total size of all subpools
    /// 对于所有subpools的total size
    fn total_subpool_size(&self) -> usize {
        self.pool.pending().len() + self.pool.base_fee().len() + self.pool.queued().len()
    }

    /// Checks that all pool invariants hold.
    /// 检查所有的pool invariants都维持了
    fn enforce_invariants(&self) {
        assert_eq!(
            self.pool.len(),
            self.total_subpool_size(),
            // tx在AllTransactions和sum(subpools)必须匹配
            "Tx in AllTransactions and sum(subpools) must match"
        );
    }
}

impl Default for MockPool {
    fn default() -> Self {
        Self { pool: TxPool::new(MockOrdering::default(), Default::default()) }
    }
}

impl<T: TransactionOrdering> Deref for MockPool<T> {
    type Target = TxPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl<T: TransactionOrdering> DerefMut for MockPool<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pool
    }
}

/// Simulates transaction execution.
/// 模拟tx的执行
pub struct MockTransactionSimulator<R: Rng> {
    /// The pending base fee
    base_fee: u128,
    /// Generator for transactions
    /// tx的生成器
    tx_generator: MockTransactionDistribution,
    /// represents the on chain balance of a sender.
    balances: HashMap<Address, U256>,
    /// represents the on chain nonce of a sender.
    nonces: HashMap<Address, u64>,
    /// A set of addresses to as senders.
    senders: Vec<Address>,
    /// What scenarios to execute.
    /// 哪个scenarios去执行
    scenarios: Vec<ScenarioType>,
    /// All previous scenarios executed by a sender.
    /// 所有被一个sender执行的previous scenarios
    executed: HashMap<Address, ExecutedScenarios>,
    /// "Validates" generated transactions.
    /// 校验生成的txs
    validator: MockTransactionFactory,
    /// The rng instance used to select senders and scenarios.
    /// rng实例用于选择senders以及scenarios
    rng: R,
}

impl<R: Rng> MockTransactionSimulator<R> {
    /// Returns a new mock instance
    /// 返回一个新的mock实例
    pub fn new(mut rng: R, config: MockSimulatorConfig) -> Self {
        let senders = config.addresses(&mut rng);
        let nonces = senders.iter().copied().map(|a| (a, 0)).collect();
        let balances = senders.iter().copied().map(|a| (a, config.balance)).collect();
        Self {
            base_fee: config.base_fee,
            balances,
            nonces,
            senders,
            scenarios: config.scenarios,
            tx_generator: config.tx_generator,
            executed: Default::default(),
            validator: Default::default(),
            rng,
        }
    }

    /// Returns a random address from the senders set
    /// 从senders set中返回一个随机的地址
    fn rng_address(&mut self) -> Address {
        let idx = self.rng.gen_range(0..self.senders.len());
        self.senders[idx]
    }

    /// Returns a random scenario from the scenario set
    /// 从scenario set返回一个随机的scenario
    fn rng_scenario(&mut self) -> ScenarioType {
        let idx = self.rng.gen_range(0..self.scenarios.len());
        self.scenarios[idx].clone()
    }

    /// Executes the next scenario and applies it to the pool
    /// 执行下一个scenarios并且应用到pool
    pub fn next(&mut self, pool: &mut MockPool) {
        let sender = self.rng_address();
        let scenario = self.rng_scenario();
        let on_chain_nonce = self.nonces[&sender];
        let on_chain_balance = self.balances[&sender];

        match scenario {
            ScenarioType::OnchainNonce => {
                let tx = self
                    .tx_generator
                    .tx(on_chain_nonce, &mut self.rng)
                    .with_gas_price(self.base_fee);
                let valid_tx = self.validator.validated(tx);

                // 添加tx到pool
                let res = pool.add_transaction(valid_tx, on_chain_balance, on_chain_nonce).unwrap();

                // TODO(mattsse): need a way expect based on the current state of the pool and tx
                // settings

                match res {
                    AddedTransaction::Pending(_) => {}
                    AddedTransaction::Parked { .. } => {
                        // 期望的是Pending
                        panic!("expected pending")
                    }
                }

                // TODO(mattsse): check subpools
            }
            ScenarioType::HigherNonce { .. } => {
                unimplemented!()
            }
        }

        // make sure everything is set
        // 确保所有都配置了
        pool.enforce_invariants()
    }
}

/// How to configure a new mock transaction stream
/// 如果配置一个新的mock tx stream
pub struct MockSimulatorConfig {
    /// How many senders to generate.
    /// 生成多少个senders
    pub num_senders: usize,
    // TODO(mattsse): add a way to generate different balances
    pub balance: U256,
    /// Scenarios to test
    pub scenarios: Vec<ScenarioType>,
    /// The start base fee
    /// 开始的base fee
    pub base_fee: u128,
    /// generator for transactions
    /// tx的generator
    pub tx_generator: MockTransactionDistribution,
}

impl MockSimulatorConfig {
    /// Generates a set of random addresses
    /// 生成一系列随机的地址
    pub fn addresses(&self, rng: &mut impl rand::Rng) -> Vec<Address> {
        std::iter::repeat_with(|| Address::random_using(rng)).take(self.num_senders).collect()
    }
}

/// Represents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioType {
    OnchainNonce,
    HigherNonce { skip: u64 },
}

/// The actual scenario, ready to be executed
/// 真实的scenario，准备被执行
///
/// A scenario produces one or more transactions and expects a certain Outcome.
/// 一个scenario生成一个或者多个tx并且期望一个Outcome
///
/// An executed scenario can affect previous executed transactions
/// 一个执行的scenario可以影响之前执行的txs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Scenario {
    /// Send a tx with the same nonce as on chain.
    /// 发送一个tx在chain上，有着同样的nonce
    OnchainNonce { nonce: u64 },
    /// Send a tx with a higher nonce that what the sender has on chain
    HigherNonce { onchain: u64, nonce: u64 },
    Multi {
        // Execute multiple test scenarios
        scenario: Vec<Scenario>,
    },
}

/// Represents an executed scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedScenario {
    /// balance at the time of execution
    balance: U256,
    /// nonce at the time of execution
    nonce: u64,
    /// The executed scenario
    scenario: Scenario,
}

/// All executed scenarios by a sender
/// 一个sender执行的所有的scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedScenarios {
    sender: Address,
    scenarios: Vec<ExecutedScenario>,
}

#[test]
fn test_on_chain_nonce_scenario() {
    let config = MockSimulatorConfig {
        num_senders: 10,
        balance: U256::from(200_000u64),
        scenarios: vec![ScenarioType::OnchainNonce],
        base_fee: 10,
        tx_generator: MockTransactionDistribution::new(30, 10..100),
    };
    let mut simulator = MockTransactionSimulator::new(rand::thread_rng(), config);
    let mut pool = MockPool::default();

    simulator.next(&mut pool);
}
