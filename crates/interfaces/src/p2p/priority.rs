/// BlockHeader and BodyHeader DownloadRequest priority
/// BlockHeader以及BodyHeader的DownloadRequest优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Queued from the back for download requests.
    /// 从后面排队等待下载请求
    #[default]
    Normal,

    /// Queued from the front for download requests.
    High,
}

impl Priority {
    /// Returns `true` if this is [Priority::High]
    pub fn is_high(&self) -> bool {
        matches!(self, Priority::High)
    }

    /// Returns `true` if this is [Priority::Normal]
    pub fn is_normal(&self) -> bool {
        matches!(self, Priority::Normal)
    }
}
