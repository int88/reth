#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Puts together all the Reth stages in a unified abstraction.
//! 将所有的Reth stages组合在一个统一的抽象中。
//!
//! # Features
//!
//! - `test-utils`: Various utilities helpful for writing tests
//! - `test-utils`: 各种有用的测试工具
//! - `geth-tests`: Runs tests that require Geth to be installed locally.
//! - `geth-tests`: 运行需要在本地安装Geth的测试。
pub mod utils;

#[cfg(any(test, feature = "test-utils"))]
/// Common helpers for integration testing.
pub mod test_utils;
