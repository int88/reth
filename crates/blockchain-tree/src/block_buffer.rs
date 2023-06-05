use lru::LruCache;
use reth_primitives::{BlockHash, BlockNumHash, BlockNumber, SealedBlockWithSenders};
use std::{
    collections::{btree_map::Entry, hash_map, BTreeMap, HashMap, HashSet},
    num::NonZeroUsize,
};
/// Type that contains blocks by number and hash.
/// 类型包含blocks的number和hash
pub type BufferedBlocks = BTreeMap<BlockNumber, HashMap<BlockHash, SealedBlockWithSenders>>;

/// Contains the Tree of pending blocks that are not executed but buffered
/// It allows us to store unconnected blocks for potential inclusion.
/// 包含未执行但缓冲的pending blocks的树，它允许我们存储未连接的blocks以便可能包含
///
/// It has three main functionality:
/// 它有三个主要功能：
/// * [BlockBuffer::insert_block] for inserting blocks inside the buffer.
/// * [BlockBuffer::insert_block] 用于将blocks插入缓冲区
/// * [BlockBuffer::remove_with_children] for connecting blocks if the parent gets received and
///   inserted.
/// * [BlockBuffer::remove_with_children] 用于连接blocks，如果parent被接收并插入
/// * [BlockBuffer::clean_old_blocks] to clear old blocks that are below finalized line.
/// * [BlockBuffer::clean_old_blocks] 清除finalized line下面的旧blocks
///
/// Note: Buffer is limited by number of blocks that it can contain and eviction of the block
/// is done by last recently used block.
/// 注意：Buffer受它可以包含的blocks数量的限制，并且block的驱逐是通过最近使用的block完成的
#[derive(Debug)]
pub struct BlockBuffer {
    /// Blocks ordered by block number inside the BTreeMap.
    /// 在BTreeMap中按block number排序的blocks
    ///
    /// Note: BTreeMap is used so that we can remove the finalized old blocks
    /// from the buffer
    /// 注意：使用BTreeMap，以便我们可以从buffer中删除finalized的旧blocks
    pub(crate) blocks: BufferedBlocks,
    /// Needed for removal of the blocks. and to connect the potential unconnected block
    /// to the connected one.
    /// 移除blocks所需的，以及将潜在的未连接的block连接到已连接的block
    pub(crate) parent_to_child: HashMap<BlockHash, HashSet<BlockNumHash>>,
    /// Helper map for fetching the block num from the block hash.
    /// Helper map用于从block hash获取block num
    pub(crate) hash_to_num: HashMap<BlockHash, BlockNumber>,
    /// LRU used for tracing oldest inserted blocks that are going to be
    /// first in line for evicting if `max_blocks` limit is hit.
    /// LRU用于追踪最老的插入的blocks，如果达到`max_blocks`限制，它们将首先被驱逐
    ///
    /// Used as counter of amount of blocks inside buffer.
    pub(crate) lru: LruCache<BlockNumHash, ()>,
}

impl BlockBuffer {
    /// Create new buffer with max limit of blocks
    pub fn new(limit: usize) -> Self {
        Self {
            blocks: Default::default(),
            parent_to_child: Default::default(),
            hash_to_num: Default::default(),
            lru: LruCache::new(NonZeroUsize::new(limit).unwrap()),
        }
    }

    /// Insert a correct block inside the buffer.
    /// 插入一个正确的block到buffer中
    pub fn insert_block(&mut self, block: SealedBlockWithSenders) {
        let num_hash = block.num_hash();

        // parent到child的映射
        self.parent_to_child.entry(block.parent_hash).or_default().insert(block.num_hash());
        // hash到num的映射
        self.hash_to_num.insert(block.hash, block.number);
        self.blocks.entry(block.number).or_default().insert(block.hash, block);

        if let Some((evicted_num_hash, _)) =
            self.lru.push(num_hash, ()).filter(|(b, _)| *b != num_hash)
        {
            // evict the block if limit is hit
            // 驱逐block，如果到达了limit
            if let Some(evicted_block) = self.remove_from_blocks(&evicted_num_hash) {
                // evict the block if limit is hit
                self.remove_from_parent(evicted_block.parent_hash, &evicted_num_hash);
            }
        }
    }

    /// Removes the given block from the buffer and also all the children of the block.
    /// 从buffer中移除给定的block并且也移除block的所有children
    ///
    /// This is used to get all the blocks that are dependent on the block that is included.
    /// 这用于获取所有依赖于包含的block的block
    ///
    /// Note: that order of returned blocks is important and the blocks with lower block number
    /// in the chain will come first so that they can be executed in the correct order.
    pub fn remove_with_children(&mut self, parent: BlockNumHash) -> Vec<SealedBlockWithSenders> {
        // remove parent block if present
        let mut taken = Vec::new();
        if let Some(block) = self.remove_from_blocks(&parent) {
            taken.push(block);
        }

        taken.extend(self.remove_children(vec![parent]).into_iter());
        taken
    }

    /// Clean up the old blocks from the buffer as blocks before finalization are not needed
    /// anymore. We can discard them from the buffer.
    pub fn clean_old_blocks(&mut self, finalized_number: BlockNumber) {
        let mut remove_parent_children = Vec::new();

        // discard all blocks that are before the finalized number.
        while let Some(entry) = self.blocks.first_entry() {
            if *entry.key() > finalized_number {
                break
            }
            let blocks = entry.remove();
            remove_parent_children.extend(
                blocks.into_iter().map(|(hash, block)| BlockNumHash::new(block.number, hash)),
            );
        }
        // remove from lru
        for block in remove_parent_children.iter() {
            self.lru.pop(block);
        }

        self.remove_children(remove_parent_children);
    }

    /// Return reference to buffered blocks
    pub fn blocks(&self) -> &BufferedBlocks {
        &self.blocks
    }

    /// Return reference to the asked block.
    pub fn block(&self, block: BlockNumHash) -> Option<&SealedBlockWithSenders> {
        self.blocks.get(&block.number)?.get(&block.hash)
    }

    /// Return reference to the asked block by hash.
    /// 返回reference到指定的block
    pub fn block_by_hash(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        let num = self.hash_to_num.get(hash)?;
        self.blocks.get(num)?.get(hash)
    }

    /// Return a reference to the lowest ancestor of the given block in the buffer.
    /// 返回buffer中给定block的最低祖先的reference
    pub fn lowest_ancestor(&self, hash: &BlockHash) -> Option<&SealedBlockWithSenders> {
        let mut current_block = self.block_by_hash(hash)?;
        while let Some(block) = self
            .blocks
            .get(&(current_block.number - 1))
            .and_then(|blocks| blocks.get(&current_block.parent_hash))
        {
            current_block = block;
        }
        Some(current_block)
    }

    /// Return number of blocks inside buffer.
    pub fn len(&self) -> usize {
        self.lru.len()
    }

    /// Return if buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.lru.is_empty()
    }

    /// Remove from the hash to num map.
    fn remove_from_hash_to_num(&mut self, hash: &BlockHash) {
        self.hash_to_num.remove(hash);
    }

    /// Remove from parent child connection. Dont touch childrens.
    /// 从parent child连接中移除，不要触摸childrens
    fn remove_from_parent(&mut self, parent: BlockHash, block: &BlockNumHash) {
        self.remove_from_hash_to_num(&parent);

        // remove from parent to child connection, but only for this block parent.
        if let hash_map::Entry::Occupied(mut entry) = self.parent_to_child.entry(parent) {
            entry.get_mut().remove(block);
            // if set is empty remove block entry.
            if entry.get().is_empty() {
                entry.remove();
            }
        };
    }

    /// Remove block from `self.blocks`, This will also remove block from `self.lru`.
    /// 从`self.blocks`移除block，这也将从`self.lru`中移除block
    ///
    /// Note: This function will not remove block from the `self.parent_to_child` connection.
    /// 注意：这个函数不会从`self.parent_to_child`连接中移除block
    fn remove_from_blocks(&mut self, block: &BlockNumHash) -> Option<SealedBlockWithSenders> {
        self.remove_from_hash_to_num(&block.hash);

        if let Entry::Occupied(mut entry) = self.blocks.entry(block.number) {
            let ret = entry.get_mut().remove(&block.hash);
            // if set is empty remove block entry.
            if entry.get().is_empty() {
                entry.remove();
            }
            self.lru.pop(block);
            return ret
        };
        None
    }

    /// Remove all children and their descendants for the given blocks and return them.
    fn remove_children(&mut self, parent_blocks: Vec<BlockNumHash>) -> Vec<SealedBlockWithSenders> {
        // remove all parent child connection and all the child children blocks that are connected
        // to the discarded parent blocks.
        let mut remove_parent_children = parent_blocks;
        let mut removed_blocks = Vec::new();
        while let Some(parent_num_hash) = remove_parent_children.pop() {
            // get this child blocks children and add them to the remove list.
            if let Some(parent_childrens) = self.parent_to_child.remove(&parent_num_hash.hash) {
                // remove child from buffer
                for child in parent_childrens.iter() {
                    if let Some(block) = self.remove_from_blocks(child) {
                        removed_blocks.push(block);
                    }
                }
                remove_parent_children.extend(parent_childrens.into_iter());
            }
        }
        removed_blocks
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use reth_interfaces::test_utils::generators::random_block;
    use reth_primitives::{BlockHash, BlockNumHash, SealedBlockWithSenders};

    use crate::BlockBuffer;

    fn create_block(number: u64, parent: BlockHash) -> SealedBlockWithSenders {
        let block = random_block(number, Some(parent), None, None);
        block.seal_with_senders().unwrap()
    }

    #[test]
    fn simple_insertion() {
        // 随机创建blocks
        let block1 = create_block(10, BlockHash::random());
        let mut buffer = BlockBuffer::new(3);

        // buffer中插入blocks
        buffer.insert_block(block1.clone());
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.block(block1.num_hash()), Some(&block1));
        assert_eq!(buffer.block_by_hash(&block1.hash), Some(&block1));
    }

    #[test]
    fn take_all_chain_of_childrens() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(12, block2.hash);
        let block4 = create_block(14, BlockHash::random());

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.block_by_hash(&block4.hash), Some(&block4));
        assert_eq!(buffer.block_by_hash(&block2.hash), Some(&block2));
        assert_eq!(buffer.block_by_hash(&main_parent.hash), None);

        assert_eq!(buffer.lowest_ancestor(&block4.hash), Some(&block4));
        assert_eq!(buffer.lowest_ancestor(&block3.hash), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block1.hash), Some(&block1));
        assert_eq!(buffer.remove_with_children(main_parent), vec![block1, block2, block3]);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn take_all_multi_level_childrens() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(11, block1.hash);
        let block4 = create_block(12, block2.hash);

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_eq!(buffer.len(), 4);
        assert_eq!(
            buffer
                .remove_with_children(main_parent)
                .into_iter()
                .map(|b| (b.hash, b))
                .collect::<HashMap<_, _>>(),
            HashMap::from([
                (block1.hash, block1),
                (block2.hash, block2),
                (block3.hash, block3),
                (block4.hash, block4)
            ])
        );
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn take_self_with_childs() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(11, block1.hash);
        let block4 = create_block(12, block2.hash);

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());
        buffer.insert_block(block4.clone());

        assert_eq!(buffer.len(), 4);
        assert_eq!(
            buffer
                .remove_with_children(block1.num_hash())
                .into_iter()
                .map(|b| (b.hash, b))
                .collect::<HashMap<_, _>>(),
            HashMap::from([
                (block1.hash, block1),
                (block2.hash, block2),
                (block3.hash, block3),
                (block4.hash, block4)
            ])
        );
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn clean_chain_of_childres() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(12, block2.hash);
        let block4 = create_block(14, BlockHash::random());

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        assert_eq!(buffer.len(), 4);
        buffer.clean_old_blocks(block1.number);
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn clean_all_multi_level_childrens() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(11, block1.hash);
        let block4 = create_block(12, block2.hash);

        let mut buffer = BlockBuffer::new(5);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        assert_eq!(buffer.len(), 4);
        buffer.clean_old_blocks(block1.number);
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn clean_multi_chains() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block1a = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block2a = create_block(11, block1.hash);
        let random_block1 = create_block(10, BlockHash::random());
        let random_block2 = create_block(11, BlockHash::random());
        let random_block3 = create_block(12, BlockHash::random());

        let mut buffer = BlockBuffer::new(10);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block1a.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block2a.clone());
        buffer.insert_block(random_block1.clone());
        buffer.insert_block(random_block2.clone());
        buffer.insert_block(random_block3.clone());

        // check that random blocks are their own ancestor, and that chains have proper ancestors
        // 检查random blocks有它们自己的ancestor并且chains有正确的ancestor
        assert_eq!(buffer.lowest_ancestor(&random_block1.hash), Some(&random_block1));
        assert_eq!(buffer.lowest_ancestor(&random_block2.hash), Some(&random_block2));
        assert_eq!(buffer.lowest_ancestor(&random_block3.hash), Some(&random_block3));

        // descendants have ancestors
        assert_eq!(buffer.lowest_ancestor(&block2a.hash), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block2.hash), Some(&block1));

        // roots are themselves
        assert_eq!(buffer.lowest_ancestor(&block1a.hash), Some(&block1a));
        assert_eq!(buffer.lowest_ancestor(&block1.hash), Some(&block1));

        assert_eq!(buffer.len(), 7);
        buffer.clean_old_blocks(10);
        assert_eq!(buffer.len(), 2);
    }

    fn assert_block_existance(buffer: &BlockBuffer, block: &SealedBlockWithSenders) {
        assert!(buffer.blocks.get(&block.number).and_then(|t| t.get(&block.hash)).is_none());
        assert!(buffer
            .parent_to_child
            .get(&block.parent_hash)
            .and_then(|p| p.get(&block.num_hash()))
            .is_none());
        assert!(buffer.hash_to_num.get(&block.hash).is_none());
    }

    #[test]
    fn evict_with_gap() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(12, block2.hash);
        let block4 = create_block(13, BlockHash::random());

        let mut buffer = BlockBuffer::new(3);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2.clone());
        buffer.insert_block(block3.clone());

        // pre-eviction block1 is the root
        assert_eq!(buffer.lowest_ancestor(&block3.hash), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block2.hash), Some(&block1));
        assert_eq!(buffer.lowest_ancestor(&block1.hash), Some(&block1));

        buffer.insert_block(block4.clone());

        assert_eq!(buffer.lowest_ancestor(&block4.hash), Some(&block4));

        // block1 gets evicted
        // block1被驱逐
        assert_block_existance(&buffer, &block1);

        // check lowest ancestor results post eviction
        // 检查lowest ancestor导致post eviction
        assert_eq!(buffer.lowest_ancestor(&block3.hash), Some(&block2));
        assert_eq!(buffer.lowest_ancestor(&block2.hash), Some(&block2));
        assert_eq!(buffer.lowest_ancestor(&block1.hash), None);

        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn simple_eviction() {
        let main_parent = BlockNumHash::new(9, BlockHash::random());
        let block1 = create_block(10, main_parent.hash);
        let block2 = create_block(11, block1.hash);
        let block3 = create_block(12, block2.hash);
        let block4 = create_block(13, BlockHash::random());

        let mut buffer = BlockBuffer::new(3);

        buffer.insert_block(block1.clone());
        buffer.insert_block(block2);
        buffer.insert_block(block3);
        buffer.insert_block(block4);

        // block3 gets evicted
        assert_block_existance(&buffer, &block1);

        assert_eq!(buffer.len(), 3);
    }
}
