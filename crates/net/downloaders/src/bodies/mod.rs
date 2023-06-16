/// A naive concurrent downloader.
/// 一个简单的并发downloader
#[allow(clippy::module_inception)]
pub mod bodies;

/// TODO:
pub mod task;

mod queue;
mod request;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
