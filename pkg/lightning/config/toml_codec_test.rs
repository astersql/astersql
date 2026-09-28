// Copyright 2026 AsterSQL.

use crate::{ByteSize, new_config};

#[test]
fn load_toml_decodes_all_go_config_fields_owned_by_the_codec() {
    let mut cfg = new_config();
    cfg.load_from_toml(
        br#"
[tidb]
max-allowed-packet = 67108864
build-stats-concurrency = 21
index-serial-scan-concurrency = 22
checksum-table-concurrency = 3
session-vars = { sql_mode = "STRICT_TRANS_TABLES", tidb_distsql_scan_concurrency = "9" }

[mydumper]
source-id = "source-1"
default-file-rules = false

[tikv-importer]
addr = "127.0.0.1:8287"
max-kv-pairs = 1024
send-kv-pairs = 2048
send-kv-size = "32KiB"
region-split-size = "96MiB"
region-split-keys = 960000
range-concurrency = 8
keyspace-name = "tenant-a"
engine-mem-cache-size = "512MiB"
local-writer-mem-cache-size = "256MiB"
store-write-bwlimit = "128MiB"
logical-import-batch-size = "128KiB"
logical-import-batch-rows = 12345
add-index-by-sql = true

[[routes]]
schema-pattern = "source_*"
table-pattern = "orders_*"
target-schema = "warehouse"
target-table = "orders"
"#,
    )
    .unwrap();

    assert_eq!(67_108_864, cfg.tidb.max_allowed_packet);
    assert_eq!(21, cfg.tidb.build_stats_concurrency);
    assert_eq!(22, cfg.tidb.index_serial_scan_concurrency);
    assert_eq!(3, cfg.tidb.checksum_table_concurrency);
    assert_eq!(
        Some(&"STRICT_TRANS_TABLES".to_owned()),
        cfg.tidb.vars.get("sql_mode")
    );
    assert_eq!("source-1", cfg.mydumper.source_id);
    assert!(!cfg.mydumper.default_file_rules);
    assert_eq!("127.0.0.1:8287", cfg.tikv_importer.addr);
    assert_eq!(1024, cfg.tikv_importer.max_kv_pairs);
    assert_eq!(2048, cfg.tikv_importer.send_kv_pairs);
    assert_eq!(ByteSize(32 * 1024), cfg.tikv_importer.send_kv_size);
    assert_eq!(
        ByteSize(96 * 1024 * 1024),
        cfg.tikv_importer.region_split_size
    );
    assert_eq!(960_000, cfg.tikv_importer.region_split_keys);
    assert_eq!(8, cfg.tikv_importer.range_concurrency);
    assert_eq!("tenant-a", cfg.tikv_importer.keyspace_name);
    assert_eq!(
        ByteSize(512 * 1024 * 1024),
        cfg.tikv_importer.engine_mem_cache_size
    );
    assert_eq!(
        ByteSize(256 * 1024 * 1024),
        cfg.tikv_importer.local_writer_mem_cache_size
    );
    assert_eq!(
        ByteSize(128 * 1024 * 1024),
        cfg.tikv_importer.store_write_bw_limit
    );
    assert_eq!(
        ByteSize(128 * 1024),
        cfg.tikv_importer.logical_import_batch_size
    );
    assert_eq!(12_345, cfg.tikv_importer.logical_import_batch_rows);
    assert!(cfg.tikv_importer.add_index_by_sql);
    assert_eq!(1, cfg.routes.len());
    assert_eq!("source_*", cfg.routes[0].schema_pattern);
    assert_eq!("orders_*", cfg.routes[0].table_pattern);
    assert_eq!("warehouse", cfg.routes[0].target_schema);
    assert_eq!("orders", cfg.routes[0].target_table);
}
