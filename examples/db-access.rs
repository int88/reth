use reth_db::open_db_read_only;
use reth_primitives::{Address, ChainSpecBuilder, H256, U256};
use reth_provider::{
    AccountReader, BlockReader, BlockSource, HeaderProvider, ProviderFactory, ReceiptProvider,
    StateProvider, TransactionsProvider,
};
use reth_rpc_types::{Filter, FilteredParams};

use std::path::Path;

// Providers are zero cost abstractions on top of an opened MDBX Transaction
// exposing a familiar API to query the chain's information without requiring knowledge
// of the inner tables.
// Providers是零成本的抽象，在打开的MDBX
// Transaction之上，暴露类似的API用于查询chain的信息而不需要inner tables的信息
//
// These abstractions do not include any caching and the user is responsible for doing that.
// Other parts of the code which include caching are parts of the `EthApi` abstraction.
// 这些抽象不包含任何的缓存并且用户负责，代码的其他部分，包含缓存的，是`EthApi`抽象的部分
fn main() -> eyre::Result<()> {
    // Opens a RO handle to the database file.
    // 打开对于db file的只读handle
    // TODO: Should be able to do `ProviderFactory::new_with_db_path_ro(...)` instead of
    // doing in 2 steps.
    let db = open_db_read_only(Path::new(&std::env::var("RETH_DB_PATH")?), None)?;

    // Instantiate a provider factory for Ethereum mainnet using the provided DB.
    // 初始化一个provider factory，用于Ethereum mainnet，使用提供的DB
    // TODO: Should the DB version include the spec so that you do not need to specify it here?
    let spec = ChainSpecBuilder::mainnet().build();
    let factory = ProviderFactory::new(db, spec.into());

    // This call opens a RO transaction on the database. To write to the DB you'd need to call
    // the `provider_rw` function and look for the `Writer` variants of the traits.
    // 这个调用在DB上打开一个RO
    // tx，为了写DB，需要调用`provider_rw`函数并且寻找traits中的`Writer`类型
    let provider = factory.provider()?;

    // Run basic queryies against the DB
    // 针对DB运行基本的queries
    let block_num = 100;
    header_provider_example(&provider, block_num)?;
    block_provider_example(&provider, block_num)?;
    txs_provider_example(&provider)?;
    receipts_provider_example(&provider)?;

    // Closes the RO transaction opened in the `factory.provider()` call. This is optional and
    // would happen anyway at the end of the function scope.
    // 关闭在`factory.provider()`上打开的RO tx，这是可选的并且会在function接收的时候发生，不管怎样
    drop(provider);

    // Run the example against latest state
    // 针对最新的state运行example
    state_provider_example(factory.latest()?)?;

    // Run it with historical state
    // 针对historical state运行
    state_provider_example(factory.history_by_block_number(block_num)?)?;

    Ok(())
}

/// The `HeaderProvider` allows querying the headers-related tables.
/// `HeaderProvider`允许访问headers相关的tables
fn header_provider_example<T: HeaderProvider>(provider: T, number: u64) -> eyre::Result<()> {
    // Can query the header by number
    // 通过number请求header
    let header = provider.header_by_number(number)?.ok_or(eyre::eyre!("header not found"))?;

    // We can convert a header to a sealed header which contains the hash w/o needing to re-compute
    // it every time.
    // 我们可以转换一个header到sealed header，其中包含hash w/o，需要每次重新计算
    let sealed_header = header.seal_slow();

    // Can also query the header by hash!
    // 同样可以通过hash请求header
    let header_by_hash =
        provider.header(&sealed_header.hash)?.ok_or(eyre::eyre!("header by hash not found"))?;
    assert_eq!(sealed_header.header, header_by_hash);

    // The header's total difficulty is stored in a separate table, so we have a separate call for
    // it. This is not needed for post PoS transition chains.
    // header的td被存储在一个独立的table，这样我们对它有另外的调用，
    // 这对于POS转换之后的chains是不需要的
    let td = provider.header_td_by_number(number)?.ok_or(eyre::eyre!("header td not found"))?;
    assert_ne!(td, U256::ZERO);

    // Can query headers by range as well, already sealed!
    // 可以通过range访问headers，已经sealed
    let headers = provider.sealed_headers_range(100..200)?;
    assert_eq!(headers.len(), 100);

    Ok(())
}

/// The `TransactionsProvider` allows querying transaction-related information
/// `TransactionsProvider`允许访问tx相关的信息
fn txs_provider_example<T: TransactionsProvider>(provider: T) -> eyre::Result<()> {
    // Try the 5th tx
    // 尝试第五个tx
    let txid = 5;

    // Query a transaction by its primary ordered key in the db
    // 通过primary ordered key从db中访问一个tx
    let tx = provider.transaction_by_id(txid)?.ok_or(eyre::eyre!("transaction not found"))?;

    // Can query the tx by hash
    // 可以通过tx访问
    let tx_by_hash =
        provider.transaction_by_hash(tx.hash)?.ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(tx, tx_by_hash);

    // Can query the tx by hash with info about the block it was included in
    // 可以通过hash以及它包含的block的信息访问tx
    let (tx, meta) =
        provider.transaction_by_hash_with_meta(tx.hash)?.ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(tx.hash, meta.tx_hash);

    // Can reverse lookup the key too
    // 可以反向查找key
    let id = provider.transaction_id(tx.hash)?.ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(id, txid);

    // Can find the block of a transaction given its key
    // 可以找到一个tx的block，给定它的key
    let _block = provider.transaction_block(txid)?;

    // Can query the txs in the range [100, 200)
    // 可以请求txs，在范围[100, 200)
    let _txs_by_tx_range = provider.transactions_by_tx_range(100..200)?;
    // Can query the txs in the _block_ range [100, 200)]
    let _txs_by_block_range = provider.transactions_by_block_range(100..200)?;

    Ok(())
}

/// The `BlockReader` allows querying the headers-related tables.
///  `BlockReader`允许访问block相关的tables
fn block_provider_example<T: BlockReader>(provider: T, number: u64) -> eyre::Result<()> {
    // Can query a block by number
    // 可以通过number请求block
    let block = provider.block(number.into())?.ok_or(eyre::eyre!("block num not found"))?;
    assert_eq!(block.number, number);

    // Can query a block with its senders, this is useful when you'd want to execute a block and do
    // not want to manually recover the senders for each transaction (as each transaction is
    // stored on disk with its v,r,s but not its `from` field.).
    // 可以用senders请求一个block，这很有用当你想要执行一个block并且不想要手动恢复senders，
    // 对于每一个tx（因为每个tx都被存在磁盘中，用它的v,r,s，但是不是它的`from`字段）
    let block = provider.block(number.into())?.ok_or(eyre::eyre!("block num not found"))?;

    // Can seal the block to cache the hash, like the Header above.
    // 可以对block进行封装来缓存hahs，就像上面的header意义
    let sealed_block = block.clone().seal_slow();

    // Can also query the block by hash directly
    let block_by_hash =
        provider.block_by_hash(sealed_block.hash)?.ok_or(eyre::eyre!("block by hash not found"))?;
    assert_eq!(block, block_by_hash);

    // Or by relying in the internal conversion
    // 或者依赖内部的转换
    let block_by_hash2 =
        provider.block(sealed_block.hash.into())?.ok_or(eyre::eyre!("block by hash not found"))?;
    assert_eq!(block, block_by_hash2);

    // Or you can also specify the datasource. For this provider this always return `None`, but
    // the blockchain tree is also able to access pending state not available in the db yet.
    // 或者你也可以指定datasource，对于这个provider，总是返回`None`，但是blockchain也能访问pending
    // state，当前在db不可获得
    let block_by_hash3 = provider
        .find_block_by_hash(sealed_block.hash, BlockSource::Any)?
        .ok_or(eyre::eyre!("block hash not found"))?;
    assert_eq!(block, block_by_hash3);

    // Can query the block's ommers/uncles
    // 可用访问block的ommers/uncles
    let _ommers = provider.ommers(number.into())?;

    // Can query the block's withdrawals (via the `WithdrawalsProvider`)
    // 可用请求block的withdrawals
    let _withdrawals =
        provider.withdrawals_by_block(sealed_block.hash.into(), sealed_block.timestamp)?;

    Ok(())
}

/// The `ReceiptProvider` allows querying the receipts tables.
/// `ReceiptProvider`允许访问receipts tables
fn receipts_provider_example<T: ReceiptProvider + TransactionsProvider + HeaderProvider>(
    provider: T,
) -> eyre::Result<()> {
    let txid = 5;
    let header_num = 100;

    // Query a receipt by txid
    // 通过txid访问receipt
    let receipt = provider.receipt(txid)?.ok_or(eyre::eyre!("tx receipt not found"))?;

    // Can query receipt by txhash too
    // 也可以通过txhash访问
    let tx = provider.transaction_by_id(txid)?.unwrap();
    let receipt_by_hash =
        provider.receipt_by_hash(tx.hash)?.ok_or(eyre::eyre!("tx receipt by hash not found"))?;
    assert_eq!(receipt, receipt_by_hash);

    // Can query all the receipts in a block
    // 可以访问一个block中的所有receipts
    let _receipts = provider
        .receipts_by_block(100.into())?
        .ok_or(eyre::eyre!("no receipts found for block"))?;

    // Can check if a address/topic filter is present in a header, if it is we query the block and
    // receipts and do something with the data
    // 可以检查是否一个address/topic
    // filter存在于header中，如果是的话，我们请求block以及receipts并且对data做一些操作
    // 1. get the bloom from the header
    // 1. 从header获取bloom
    let header = provider.header_by_number(header_num)?.unwrap();
    let bloom = header.logs_bloom;

    // 2. Construct the address/topics filters
    // 2. 构建address/topics filters
    // For a hypothetical address, we'll want to filter down for a specific indexed topic (e.g.
    // `from`).
    let addr = Address::random();
    let topic = H256::random();

    // TODO: Make it clearer how to choose between topic0 (event name) and the other 3 indexed
    // topics. This API is a bit clunky and not obvious to use at the moemnt.
    let filter = Filter::new().address(addr).topic0(topic);
    let filter_params = FilteredParams::new(Some(filter));
    let address_filter = FilteredParams::address_filter(&addr.into());
    let topics_filter = FilteredParams::topics_filter(&[topic.into()]);

    // 3. If the address & topics filters match do something. We use the outer check against the
    // bloom filter stored in the header to avoid having to query the receipts table when there
    // is no instance of any event that matches the filter in the header.
    if FilteredParams::matches_address(bloom, &address_filter) &&
        FilteredParams::matches_topics(bloom, &topics_filter)
    {
        let receipts = provider.receipt(header_num)?.ok_or(eyre::eyre!("receipt not found"))?;
        for log in &receipts.logs {
            if filter_params.filter_address(log) && filter_params.filter_topics(log) {
                // Do something with the log e.g. decode it.
                // 对log做一些事情，例如，解码它
                println!("Matching log found! {log:?}")
            }
        }
    }

    Ok(())
}

fn state_provider_example<T: StateProvider + AccountReader>(provider: T) -> eyre::Result<()> {
    let address = Address::random();
    let storage_key = H256::random();

    // Can get account / storage state with simple point queries
    // 可用获取account / storage state，用简单的point queries
    let _account = provider.basic_account(address)?;
    let _code = provider.account_code(address)?;
    let _storage = provider.storage(address, storage_key)?;
    // TODO: unimplemented.
    // let _proof = provider.proof(address, &[])?;

    Ok(())
}
