use reth_db::{
    abstraction::table::Encode,
    cursor::{DbCursorRO, DbDupCursorRO},
    open_db_read_only,
    table::{Compress, DupSort, Table},
    tables,
    transaction::DbTx,
};
use reth_primitives::ChainSpecBuilder;
use reth_provider::ProviderFactory;
use std::{io::Write, mem::size_of, path::Path};

// in reth: 33781302 accounts
// in sled: 33781302 accounts
//
// reth size:
//  | PlainAccountState          | 33781302  | 6340         | 664574     | 0              | 2.6 GiB    |
//
// sled size:
//  ❯ du -sh reth
//  785M	reth
//
// todo: check if diff is mostly because data was inserted sorted into sled
fn main() -> eyre::Result<()> {
    // open reth db
    let db_path = std::env::var("RETH_DB_PATH")?;
    let db_path = Path::new(&db_path);
    let db = open_db_read_only(db_path.join("db").as_path(), Default::default())?;
    let spec = ChainSpecBuilder::mainnet().build();
    let factory = ProviderFactory::new(db, spec.into(), db_path.join("static_files"))?;

    // open sled
    let sled = sled::open("reth").expect("could not open sled");

    // open ro tx
    let provider = factory.provider()?.disable_long_read_transaction_safety();
    let tx = provider.into_tx();

    // migrate normal tables
    migrate::<tables::PlainAccountState, _>(&tx, &sled)?;
    migrate::<tables::HashedAccounts, _>(&tx, &sled)?;
    migrate::<tables::TransactionHashNumbers, _>(&tx, &sled)?;
    migrate::<tables::BlockWithdrawals, _>(&tx, &sled)?;
    migrate::<tables::AccountsTrie, _>(&tx, &sled)?;
    migrate::<tables::Bytecodes, _>(&tx, &sled)?;
    migrate::<tables::StoragesHistory, _>(&tx, &sled)?;
    migrate::<tables::Receipts, _>(&tx, &sled)?;
    migrate::<tables::AccountsHistory, _>(&tx, &sled)?;
    migrate::<tables::HeaderNumbers, _>(&tx, &sled)?;
    migrate::<tables::BlockBodyIndices, _>(&tx, &sled)?;
    migrate::<tables::TransactionBlocks, _>(&tx, &sled)?;

    // migrate dup tables
    migrate_dup::<tables::PlainStorageState, _>(&tx, &sled)?;
    migrate_dup::<tables::HashedStorages, _>(&tx, &sled)?;
    migrate_dup::<tables::StoragesTrie, _>(&tx, &sled)?;
    migrate_dup::<tables::StorageChangeSets, _>(&tx, &sled)?;
    migrate_dup::<tables::AccountChangeSets, _>(&tx, &sled)?;

    sled.flush()?;
    println!("flushed");

    Ok(())
}

fn migrate<T, Tx>(tx: &Tx, sled: &sled::Db) -> eyre::Result<()>
where
    T: Table,
    <T as Table>::Key: Default,
    Tx: DbTx,
{
    println!("Migrating table {} ({} entries)", T::NAME, tx.entries::<T>()?);
    let tree = sled.open_tree(T::NAME)?;
    let mut count = 0;

    let mut cursor = tx.cursor_read::<T>()?;
    for item in cursor.walk_range(T::Key::default()..)? {
        let (key, value) = item?;
        tree.insert(key.encode().as_ref(), value.compress().as_ref())?;
        count += 1;
        if count % 10_000 == 0 {
            print!(".");
            std::io::stdout().flush()?;
        }
        if count % 1_000_000 == 0 {
            println!(" {count}");
        }
    }
    println!();

    println!("Inserted {count} items into {}", T::NAME);
    Ok(())
}

fn migrate_dup<T, Tx>(tx: &Tx, sled: &sled::Db) -> eyre::Result<()>
where
    T: DupSort,
    Tx: DbTx,
{
    println!("Migrating dupsort table {} ({} entries)", T::NAME, tx.entries::<T>()?);
    let tree = sled.open_tree(T::NAME)?;
    let sub_key_size = size_of::<T::SubKey>();
    let mut count = 0;

    let mut cursor = tx.cursor_dup_read::<T>()?;
    while let Some((k, _)) = cursor.next_no_dup()? {
        for kv in cursor.walk_dup(Some(k), None)? {
            let (key, value) = kv?;

            // encode the value and key
            let compressed = value.compress();
            let value = compressed.as_ref();
            let key = key.encode();

            // extract the subkey
            let sub_key = &value[0..sub_key_size];

            // set key to `key ++ sub_key`
            let key = [key.as_ref(), sub_key.as_ref()].concat();

            // insert
            tree.insert(key, &value[sub_key_size..])?;

            count += 1;
            if count % 10_000 == 0 {
                print!(".");
                std::io::stdout().flush()?;
            }
            if count % 1_000_000 == 0 {
                println!(" {count}");
            }
        }
    }
    println!();

    println!("Inserted {count} items into {}", T::NAME);
    Ok(())
}
