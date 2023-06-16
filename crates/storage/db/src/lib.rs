//! reth's database abstraction layer with concrete implementations.
//! reth的数据库抽象层，包含具体的实现。
//!
//! The database abstraction assumes that the underlying store is a KV store subdivided into tables.
//! database抽象层假设底层的存储是一个KV存储，分成了多个表。
//!
//! One or more changes are tied to a transaction that is atomically committed to the data store at
//! the same time. Strong consistency in what data is written and when is important for reth, so it
//! is not possible to write data to the database outside of using a transaction.
//! 一个或者多个改变被绑定到一个transaction，这个transaction被原子性的提交到数据存储中。强一致性是reth的重要特性，
//! 所以在transaction之外不可能写数据到数据库中。
//!
//! Good starting points for this crate are:
//!
//! - [`Database`] for the main database abstraction
//! - [`DbTx`] (RO) and [`DbTxMut`] (RW) for the transaction abstractions.
//! - [`DbCursorRO`] (RO) and [`DbCursorRW`] (RW) for the cursor abstractions (see below).
//!
//! # Cursors and Walkers
//!
//! The abstraction also defines a couple of helpful abstractions for iterating and writing data:
//! abstraction同时定义了一些有用的抽象，用于迭代和写数据：
//!
//! - **Cursors** ([`DbCursorRO`] / [`DbCursorRW`]) for iterating data in a table. Cursors are
//!   assumed to resolve data in a sorted manner when iterating from start to finish, and it is safe
//!   to assume that they are efficient at doing so.
//! - **Cursors** ([`DbCursorRO`] / [`DbCursorRW`])用于迭代一个表中的数据。当从头到尾迭代时，cursors
//!  被假设以排序的方式解析数据，可以安全的假设它们在这方面是高效的。
//! - **Walkers** ([`Walker`] / [`RangeWalker`] / [`ReverseWalker`]) use cursors to walk the entries
//!   in a table, either fully from a specific point, or over a range.
//! - **Walkers** ([`Walker`] / [`RangeWalker`] / [`ReverseWalker`])使用cursors来遍历一个表中的条目，
//! 从一个特定的点开始，或者在一个范围内。
//!
//! Dup tables (see below) also have corresponding cursors and walkers (e.g. [`DbDupCursorRO`]).
//! These **should** be preferred when working with dup tables, as they provide additional methods
//! that are optimized for dup tables.
//! Dup tables（见下文）也有相应的cursors和walkers（例如[`DbDupCursorRO`]）。当使用dup tables时，应该优先使用它们，
//! 因为它们提供了额外的方法，这些方法针对dup tables进行了优化。
//!
//! # Tables
//!
//! reth has two types of tables: simple KV stores (one key, one value) and dup tables (one key,
//! many values). Dup tables can be efficient for certain types of data.
//! reth有两种类型的tables：简单的KV存储（一个key，一个value）和dup tables（一个key，多个values）。dup tables
//! 对于某些类型的数据来说是高效的。
//!
//! Keys are de/serialized using the [`Encode`] and [`Decode`] traits, and values are de/serialized
//! ("compressed") using the [`Compress`] and [`Decompress`] traits.
//!
//! Tables implement the [`Table`] trait.
//!
//! # Overview
//!
//! An overview of the current data model of reth can be found in the [`tables`] module.
//!
//! [`Database`]: crate::abstraction::database::Database
//! [`DbTx`]: crate::abstraction::transaction::DbTx
//! [`DbTxMut`]: crate::abstraction::transaction::DbTxMut
//! [`DbCursorRO`]: crate::abstraction::cursor::DbCursorRO
//! [`DbCursorRW`]: crate::abstraction::cursor::DbCursorRW
//! [`Walker`]: crate::abstraction::cursor::Walker
//! [`RangeWalker`]: crate::abstraction::cursor::RangeWalker
//! [`ReverseWalker`]: crate::abstraction::cursor::ReverseWalker
//! [`DbDupCursorRO`]: crate::abstraction::cursor::DbDupCursorRO
//! [`Encode`]: crate::abstraction::table::Encode
//! [`Decode`]: crate::abstraction::table::Decode
//! [`Compress`]: crate::abstraction::table::Compress
//! [`Decompress`]: crate::abstraction::table::Decompress
//! [`Table`]: crate::abstraction::table::Table

#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

/// Traits defining the database abstractions, such as cursors and transactions.
pub mod abstraction;

mod implementation;
pub mod tables;
mod utils;

#[cfg(feature = "mdbx")]
/// Bindings for [MDBX](https://libmdbx.dqdkfa.ru/).
pub mod mdbx {
    pub use crate::implementation::mdbx::*;
    pub use reth_libmdbx::*;
}

pub use abstraction::*;
pub use reth_interfaces::db::DatabaseError;
pub use tables::*;
