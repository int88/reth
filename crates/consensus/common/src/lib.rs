#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Commonly used consensus methods.
//! 由常用的共识方法组成。

/// Collection of consensus validation methods.
/// 一系列共识验证方法。
pub mod validation;

/// Various calculation methods (e.g. block rewards)
/// 各种计算方法（例如区块奖励）
pub mod calc;
