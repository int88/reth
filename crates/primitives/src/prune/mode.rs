use crate::BlockNumber;
use reth_codecs::{main_codec, Compact};

/// Prune mode.
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PruneMode {
    /// Prune all blocks.
    /// 修剪所有的blocks
    Full,
    /// Prune blocks before the `head-N` block number. In other words, keep last N + 1 blocks.
    /// 清理block number在`head-N`之前的blocks，换句话说，保留最新的N+1个blocks
    Distance(u64),
    /// Prune blocks before the specified block number. The specified block number is not pruned.
    /// 清理指定block number之前的blocks，指定的block number不被移除
    Before(BlockNumber),
}

#[cfg(test)]
impl Default for PruneMode {
    fn default() -> Self {
        Self::Full
    }
}

#[cfg(test)]
mod tests {
    use crate::prune::PruneMode;
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[test]
    fn prune_mode_deserialize() {
        #[derive(Debug, Deserialize)]
        struct Config {
            a: Option<PruneMode>,
            b: Option<PruneMode>,
            c: Option<PruneMode>,
            d: Option<PruneMode>,
        }

        let toml_str = r#"
        a = "full"
        b = { distance = 10 }
        c = { before = 20 }
    "#;

        assert_matches!(
            toml::from_str(toml_str),
            Ok(Config {
                a: Some(PruneMode::Full),
                b: Some(PruneMode::Distance(10)),
                c: Some(PruneMode::Before(20)),
                d: None
            })
        );
    }
}
