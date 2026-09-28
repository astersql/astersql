// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Go-equivalent tests for `br/pkg/metautil/load_test.go`.
//!
//! 覆盖 `LoadBackupTables` 的非分区/分区表文件归属，以及大规模加载的冒烟基准。
//! 夹具写入内存 Storage 的 `backupmeta`，经 `NewMetaReader` 再调用 LoadBackupTables。
//! 行键用本地 `encode_row_key` 模拟 tablecodec，便于按 physical table id 归类 SST。
//! 分区场景验证跨分区文件仍归入对应 physical id，且过滤掉表 id 区间外的噪声文件。
//! 基准用例把 Go Benchmark 收敛为单次迭代单元测试，只断言库表数量而非耗时。

use std::sync::Arc;

use astersql_meta_model::{DBInfo, PartitionDefinition, PartitionInfo, TableInfo};
use astersql_objstore::azblob::MemoryStorage;
use astersql_objstore_storeapi::{Context, Storage};
use astersql_parser_ast::NewCIStr;

use crate::load::LoadBackupTables;
use crate::metafile::{MetaFile as META_FILE_CONST, NewMetaReader, Table};
use crate::stubs::kvproto::brpb::{BackupMeta, CipherInfo, File, Schema};
use crate::stubs::kvproto::encryptionpb::EncryptionMethod;
use crate::stubs::protobuf::Message;

// 下列辅助函数仅服务本文件测试，不导出到 crate 公共 API。

/// tablecodec.EncodeRowKey stand-in (no tablecodec dep on darwin arm64).
///
/// 布局：`t` + 翻转符号位的 8 字节 table id + `_r` + handle，足够 DecodeTableID 解析。
fn encode_row_key(table_id: i64, handle: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(11 + handle.len());
    buf.push(b't');
    // 与 TiDB 键编码一致：对有符号 id 做 XOR 高位，保证字典序。
    let u = (table_id as u64) ^ (1u64 << 63);
    buf.extend_from_slice(&u.to_be_bytes());
    buf.extend_from_slice(b"_r");
    buf.extend_from_slice(handle);
    buf
}

/// 明文密码器，加载路径只校验解密接口可调用而不改密文。
fn plaintext_cipher() -> CipherInfo {
    let mut cipher = CipherInfo::new();
    // Plaintext：Encrypt/Decrypt 短路径，专注归属逻辑。
    cipher.set_cipher_type(EncryptionMethod::Plaintext);
    cipher
}

/// 组装最小 BackupMeta：schemas + files，供写入 backupmeta 对象键。
fn mock_backup_meta(schemas: Vec<Schema>, files: Vec<File>) -> BackupMeta {
    let mut meta = BackupMeta::new();
    // files/schemas 顺序与 Go 测试构造一致，便于对照调试。
    meta.set_files(files);
    meta.set_schemas(schemas);
    meta
}

/// 生成含 database_name/table_name 的统计 JSON，满足 parse_stats_json 最小字段。
fn stats_json(db: &str, table: &str) -> Vec<u8> {
    // 仅填充 parse_stats_json 读取的核心键，其余字段走默认值。
    serde_json::to_vec(&serde_json::json!({
        "database_name": db,
        "table_name": table,
    }))
    .expect("stats json")
}

/// Go `GetTable` dereferences every `table.Info`; a malformed entry must not be
/// silently skipped by the Rust port.
#[test]
#[should_panic(expected = "table info")]
fn get_table_panics_for_missing_table_info_like_go() {
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();
    let db_info = DBInfo {
        Name: NewCIStr("test"),
        ..Default::default()
    };
    let table_info = TableInfo {
        Name: NewCIStr("valid"),
        ..Default::default()
    };
    let mut schema = Schema::new();
    schema.set_db(serde_json::to_vec(&db_info).expect("db"));
    schema.set_table(serde_json::to_vec(&table_info).expect("table"));
    let meta = mock_backup_meta(vec![schema], Vec::new());
    let reader = NewMetaReader(meta, storage, Some(plaintext_cipher()));
    let mut db = LoadBackupTables(&ctx, &reader, false)
        .expect("load")
        .remove("test")
        .expect("database");
    let table = Table {
        DB: db_info.clone(),
        Info: None,
        Crc64Xor: 0,
        TotalKvs: 0,
        TotalBytes: 0,
        FilesOfPhysicals: Default::default(),
        TiFlashReplicas: 0,
        Stats: None,
        StatsFileIndexes: Vec::new(),
        IsMergeOptionAllowed: false,
        PartitionMergeOptionAllowed: Default::default(),
    };
    db.Tables.insert(0, table);

    let _ = db.GetTable("missing");
}

#[test]
fn load_backup_tables_returns_context_cancellation() {
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();
    ctx.cancel();
    let reader = NewMetaReader(
        mock_backup_meta(Vec::new(), Vec::new()),
        storage,
        Some(plaintext_cipher()),
    );

    let err = match LoadBackupTables(&ctx, &reader, false) {
        Ok(_) => panic!("cancelled load must fail"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("context canceled"));
}

/// TestLoadBackupMeta — Go `TestLoadBackupMeta`.
///
/// 单表两文件：仅 start_key 落在本表 id 区间内的 SST 应归入 FilesOfPhysicals。
#[test]
fn test_load_backup_meta() {
    // 内存 Storage：无外部依赖，可重复写入 backupmeta。
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();

    let tbl_name = NewCIStr("t1");
    let db_name = NewCIStr("test");
    // 固定表 id=123，便于手工计算行键归属。
    let tbl_id: i64 = 123;
    // 最小 TableInfo/DBInfo；Deprecated.Tables 满足序列化形态。
    let mock_tbl = TableInfo {
        ID: tbl_id,
        Name: tbl_name.clone(),
        ..Default::default()
    };
    let mut mock_db = DBInfo {
        ID: 1,
        Name: db_name.clone(),
        ..Default::default()
    };
    mock_db.Deprecated.Tables = vec![std::sync::Arc::new(mock_tbl.clone())];

    let db_bytes = serde_json::to_vec(&mock_db).expect("marshal db");
    let tbl_bytes = serde_json::to_vec(&mock_tbl).expect("marshal table");
    let stats_bytes = stats_json(&db_name.O, &tbl_name.O);

    let mut schema = Schema::new();
    schema.set_db(db_bytes);
    schema.set_table(tbl_bytes);
    schema.set_stats(stats_bytes);

    // Schema 三件套：db/table/stats 字节均需可反序列化。
    // file1：start 在本表 → 应收录；file2：start 在更小 id → 应过滤。
    let mut file1 = File::new();
    file1.set_name("1.sst".to_string());
    file1.set_start_key(encode_row_key(tbl_id, b"a"));
    file1.set_end_key(encode_row_key(tbl_id + 1, b"a"));

    let mut file2 = File::new();
    file2.set_name("2.sst".to_string());
    file2.set_start_key(encode_row_key(tbl_id - 1, b"a"));
    file2.set_end_key(encode_row_key(tbl_id, b"a"));

    let meta = mock_backup_meta(vec![schema], vec![file1, file2]);
    let data = meta.write_to_bytes().expect("marshal backupmeta");
    // 写入默认 MetaFile 对象键，NewMetaReader 依赖该约定。
    storage
        .WriteFile(&ctx, META_FILE_CONST, &data)
        .expect("write backupmeta");

    // Reader 持有 BackupMeta 克隆与 Storage；cipher 走明文。
    let reader = NewMetaReader(meta, storage, Some(plaintext_cipher()));
    // loadStats=true：解析 stats JSON，覆盖完整加载路径。
    let dbs = LoadBackupTables(&ctx, &reader, true).expect("load tables");
    let tbl = dbs
        .get(&db_name.O)
        .expect("db")
        .GetTable(&tbl_name.O)
        .expect("table");
    // 仅一个 physical id，且只含 1.sst。
    assert_eq!(tbl.FilesOfPhysicals.len(), 1);
    assert_eq!(tbl.FilesOfPhysicals.get(&tbl_id).map(|v| v.len()), Some(1));
    assert_eq!(
        tbl.FilesOfPhysicals.get(&tbl_id).unwrap()[0].get_name(),
        "1.sst"
    );
}

/// TestLoadBackupMetaPartionTable — Go `TestLoadBackupMetaPartionTable`.
///
/// 分区表：文件按分区 physical id 归桶；跨分区与表外文件按 Go 规则取舍。
#[test]
fn test_load_backup_meta_partition_table() {
    // 分区夹具与单表测共用内存 Storage 模式。
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();

    let tbl_name = NewCIStr("t1");
    let db_name = NewCIStr("test");
    let tbl_id: i64 = 123;
    let part_id1: i64 = 124;
    // 分区 id 紧随表 id，模拟真实分配习惯。
    let part_id2: i64 = 125;
    // 两个分区定义；Load 时应对每个 definition.ID 查 file_map。
    let mock_tbl = TableInfo {
        ID: tbl_id,
        Name: tbl_name.clone(),
        Partition: Some(PartitionInfo {
            Definitions: vec![
                PartitionDefinition {
                    ID: part_id1,
                    ..Default::default()
                },
                PartitionDefinition {
                    ID: part_id2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut mock_db = DBInfo {
        ID: 1,
        Name: db_name.clone(),
        ..Default::default()
    };
    mock_db.Deprecated.Tables = vec![std::sync::Arc::new(mock_tbl.clone())];

    let mut schema = Schema::new();
    schema.set_db(serde_json::to_vec(&mock_db).expect("db"));
    schema.set_table(serde_json::to_vec(&mock_tbl).expect("table"));
    schema.set_stats(stats_json(&db_name.O, &tbl_name.O));

    // 分区表同样写入 stats，避免 SkipStats 分支掩盖问题。
    // 四文件：1/2/3 应保留，4 落在表 id 之前应丢弃（与 Go 期望 count==3）。
    let mut files = Vec::new();
    for (name, start_id, end_id, start_h, end_h) in [
        (
            "1.sst",
            part_id1,
            part_id1,
            b"a".as_slice(),
            b"b".as_slice(),
        ),
        (
            "2.sst",
            part_id1,
            part_id2,
            b"b".as_slice(),
            b"a".as_slice(),
        ),
        (
            "3.sst",
            part_id2,
            part_id2 + 1,
            b"a".as_slice(),
            b"b".as_slice(),
        ),
        (
            "4.sst",
            tbl_id - 1,
            tbl_id,
            b"a".as_slice(),
            b"a".as_slice(),
        ),
    ] {
        let mut f = File::new();
        f.set_name(name.to_string());
        f.set_start_key(encode_row_key(start_id, start_h));
        f.set_end_key(encode_row_key(end_id, end_h));
        files.push(f);
    }

    let meta = mock_backup_meta(vec![schema], files);
    let data = meta.write_to_bytes().expect("marshal");
    storage
        .WriteFile(&ctx, META_FILE_CONST, &data)
        .expect("write");

    // 分区加载同样开启 stats，与 Go 测试默认一致。
    let reader = NewMetaReader(meta, storage, Some(plaintext_cipher()));
    let dbs = LoadBackupTables(&ctx, &reader, true).expect("load");
    let tbl = dbs
        .get(&db_name.O)
        .expect("db")
        .GetTable(&tbl_name.O)
        .expect("table");
    // 两个分区 physical id；合计 3 个文件条目。
    assert_eq!(tbl.FilesOfPhysicals.len(), 2);
    let count: usize = tbl.FilesOfPhysicals.values().map(|v| v.len()).sum();
    assert_eq!(count, 3);
    // 闭包扫描所有 physical 桶，忽略文件落在哪个分区。
    let contains = |name: &str| {
        tbl.FilesOfPhysicals
            .values()
            .flatten()
            .any(|f| f.get_name() == name)
    };
    assert!(contains("1.sst"));
    assert!(contains("2.sst"));
    assert!(contains("3.sst"));
    // 4.sst 不应出现：start_key 解码出的 physical id 不属于分区集合。
}

/// 为基准构造单表及其连续 handle 区间的 SST 列表。
fn build_table_and_files(name: &str, table_id: i64, file_count: usize) -> (TableInfo, Vec<File>) {
    let mock_tbl = TableInfo {
        ID: table_id,
        Name: NewCIStr(name),
        ..Default::default()
    };
    let mut mock_files = Vec::with_capacity(file_count);
    for i in 0..file_count {
        let mut f = File::new();
        // 文件名编码 table_id 与序号，便于人工排查归属。
        f.set_name(format!("{table_id}-{i}.sst"));
        f.set_start_key(encode_row_key(table_id, format!("{i:09}").as_bytes()));
        f.set_end_key(encode_row_key(table_id, format!("{:09}", i + 1).as_bytes()));
        // handle 用 9 位零填充，保证字典序与序号一致。
        mock_files.push(f);
    }
    (mock_tbl, mock_files)
}

/// 批量生成多表 BackupMeta；每张表共享同一逻辑库名，形成单库多表负载。
fn build_benchmark_backupmeta(
    db_name: &str,
    table_count: usize,
    file_count_per_table: usize,
) -> BackupMeta {
    // 先攒齐全部 SST，再按表生成 Schema，保持 BackupMeta 字段顺序可读。
    let mut mock_files = Vec::new();
    let mut mock_schemas = Vec::new();
    // 表 id 从 1 递增，与 encode_row_key 一一对应。
    for i in 1..=table_count {
        let (mock_tbl, files) =
            build_table_and_files(&format!("mock{i}"), i as i64, file_count_per_table);
        mock_files.extend(files);
        let mut mock_db = DBInfo {
            ID: 1,
            Name: NewCIStr(db_name),
            ..Default::default()
        };
        mock_db.Deprecated.Tables = vec![std::sync::Arc::new(mock_tbl.clone())];
        let mut schema = Schema::new();
        schema.set_db(serde_json::to_vec(&mock_db).expect("db"));
        schema.set_table(serde_json::to_vec(&mock_tbl).expect("table"));
        mock_schemas.push(schema);
    }
    // 最终 schemas/files 长度分别为 table_count 与 table_count*file_count。
    mock_backup_meta(mock_schemas, mock_files)
}

/// 跑一轮加载并断言：单库、表数量等于输入 table_count。
fn run_load_backup_meta_bench(table_count: usize, file_count_per_table: usize) {
    // 每次基准独立 Storage，避免跨用例残留对象键。
    let storage: Arc<dyn Storage + Send + Sync> = Arc::new(MemoryStorage::default());
    let ctx = Context::default();
    let meta = build_benchmark_backupmeta("bench", table_count, file_count_per_table);
    let data = meta.write_to_bytes().expect("marshal");
    storage
        .WriteFile(&ctx, META_FILE_CONST, &data)
        .expect("write");
    let reader = NewMetaReader(meta, storage, Some(plaintext_cipher()));
    // loadStats=true：走完整 schema+stats 解析路径，贴近 Go 基准场景。
    let dbs = LoadBackupTables(&ctx, &reader, true).expect("load");
    assert_eq!(dbs.len(), 1);
    assert!(dbs.contains_key("bench"));
    assert_eq!(dbs["bench"].Tables.len(), table_count);
    // 不检查单表文件数：基准关注吞吐与归桶收敛，非细粒度归属。
}

/// BenchmarkLoadBackupMeta64 — Go benchmark body as a unit test (one iteration).
///
/// 64 表 × 64 文件：中等规模正确性冒烟。
#[test]
fn benchmark_load_backup_meta_64() {
    // Go: BenchmarkLoadBackupMeta64 — 单次迭代即可暴露崩溃/死锁。
    run_load_backup_meta_bench(64, 64);
}

/// BenchmarkLoadBackupMeta1024 — Go benchmark body as a unit test (one iteration).
///
/// 1024 表：验证批量解析与归桶在较大输入下仍收敛。
#[test]
fn benchmark_load_backup_meta_1024() {
    // Go: BenchmarkLoadBackupMeta1024 — 千表级压力冒烟。
    run_load_backup_meta_bench(1024, 64);
}

/// BenchmarkLoadBackupMeta10240 — Go benchmark body as a unit test (one iteration).
///
/// 10240 表：对齐 Go 最大档基准规模，仅做功能断言。
#[test]
fn benchmark_load_backup_meta_10240() {
    // Go: BenchmarkLoadBackupMeta10240 — 最大档；CI 可能较慢但应通过。
    run_load_backup_meta_bench(10240, 64);
}
