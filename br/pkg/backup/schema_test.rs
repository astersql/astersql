// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/backup/schema_test.go`.
//!
//! 对齐 Go `schema_test.go`：BuildBackupRangeAndInitSchema 与 BackupSchemas 主路径。
//! MemMeta 夹具替代真实 TiDB；checksum 通过 inject 控制返回值。
//! 覆盖：不存在表/库、空库、单表备份、全量收集 policy、损坏 stats、系统表改名。
//! 断言进度、finished 标志、checksum 字段非零（或 skip 时行为）。
//!
//! 子场景索引：
//! - 缺表但仍匹配库：应返回 Some(schemas)，Len 可为 0 或仅空库单元。
//! - 缺库：backup_schemas 为 None（与 Go 一致，表示无可备份对象）。
//! - 过滤系统库后的空用户库：仍应有 schemas 容器。
//! - 非全量单表：写出 1 条 schema，policies 为空，checksum 三字段非零。
//! - 全量多表：写出条数=表数，并收集 placement policy。
//! - 无 stats 句柄：StatsIndex 为空，但不影响 checksum/身份字段。
//! - 有 MemStatsHandle：与无句柄路径在 checksum/Table/Db 字节上保持等价（stub 限制）。
//! - 系统表：库名改写为 TemporaryDBName，表名前缀保持。
//!
//! Mock TiDB/SQL is replaced by in-memory `MemMeta` fixtures (darwin-safe stubs).
//! - 缺表命中库：Some(schemas)，验证空库仍进入备份任务构造。
//! - 缺库：None，与 Go 一致，避免对不存在库发起 BackupSchemas。
//! - NoSysFilter：排除四类系统库，只看用户库。
//! - 非全量 + policy 夹具：policies.len()==0。
//! - 全量 + t1/t2：Len==2 且 policies==1。
//! - progress 等于写出 Schema 条数，防漏发/重发。
//! - finished 标志确认 FinishWriteMetas。
//! - Crc64Xor/Kvs/Bytes 非零证明 checksum 路径执行。
//! - broken-stats：无 handle 时 StatsIndex 空，但仍有表/库字节。
//! - 有 MemStatsHandle：身份字段与无 handle 路径等价。
//! - 系统表：TemporaryDBName("mysql")，表名前缀 systable。
//! - skipChecksum=true 专注改名契约，不强制校验和字段。
//! - 并行测试避免依赖单一全局 inject_checksum_response。

// 内存元数据 + 可注入 checksum，避免真实集群依赖。
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

// client 构造 ranges；schema 模块执行写出。
use crate::client::{BuildBackupRangeAndInitSchema, BuildBackupSchemas};
use crate::schema::NewBackupSchemas;
use crate::stubs::filter::{AllFilter, AllowList, Filter};
use crate::stubs::glue::{self, Progress};
use crate::stubs::meta::MemMeta;
use crate::stubs::metautil::{self, AppendOp, MemMetaWriter, MetaPayload, MetaWriter, StatsWriter};
use crate::stubs::model::{
    CIStr, DBInfo, PartitionDefinition, PartitionInfo, PolicyInfo, TableInfo,
};
use crate::stubs::objstore::MemStorage;
use crate::stubs::statistics::MemStatsHandle;
use crate::stubs::{Codec, Error};
use crate::stubs::{
    Context, IdentityCodec, KvClient, Result, Snapshot, Storage, Version, checksum,
};

/// checksum 表级并发默认值，与 Go 测试常量对齐。
const DEF_CHECKSUM_TABLE_CONCURRENCY: u32 = 4;

/// 原子进度计数，用于断言 BackupSchemas 写出次数。
/// simpleProgress mirrors Go atomic progress counter.
#[derive(Default)]
/// 测试进度实现。
pub struct simpleProgress {
    /// Inc 累计值。
    counter: AtomicI64,
}

impl Progress for simpleProgress {
    /// 每写完一个 schema 递增。
    fn Inc(&self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

impl simpleProgress {
    /// 多场景复用前清零。
    fn reset(&self) {
        self.counter.store(0, Ordering::SeqCst);
    }
    /// 读取进度。
    fn get(&self) -> i64 {
        self.counter.load(Ordering::SeqCst)
    }
}

/// 空快照桩。
struct EmptySnap;
impl Snapshot for EmptySnap {}
/// 空 KV 桩。
struct EmptyKv;
impl KvClient for EmptyKv {}

/// 包装 MemMeta 的 Storage 实现。
struct MemKvStorage {
    /// 共享夹具。
    meta: Arc<MemMeta>,
}

impl Storage for MemKvStorage {
    /// 不读真实数据。
    fn GetSnapshot(&self, _ver: Version) -> Box<dyn Snapshot> {
        Box::new(EmptySnap)
    }
    /// checksum inject 作用在 executor，不依赖此客户端实现细节。
    fn GetClient(&self) -> Box<dyn KvClient> {
        Box::new(EmptyKv)
    }
    /// IdentityCodec。
    fn GetCodec(&self) -> Box<dyn Codec> {
        Box::new(IdentityCodec)
    }
    /// 固定 Version(100)。
    fn CurrentVersion(&self, _scope: &str) -> Result<Version> {
        Ok(Version::New(100))
    }
}

/// 构造带自增标志的样本表。
fn sample_table(id: i64, name: &str) -> TableInfo {
    // Version=1 / HasAutoInc 仅作真实表形状占位。
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Version: 1,
        HasAutoInc: true,
        ..Default::default()
    }
}

/// 仅含 test 库、尚无表的 MemMeta。
fn fixture_meta_with_test_db() -> Arc<MemMeta> {
    // 返回仅含 test 库的空表夹具。
    let meta = Arc::new(MemMeta::default());
    meta.dbs.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("test"),
        PlacementPolicyRef: None,
    });
    meta
}

/// 用真实 BuildBackupSchemas 作为 iterFunc，避免 stand-in 漏表。
/// Rebuild Schemas with a real iterFunc (Rust BuildBackupRangeAndInitSchema uses a stand-in).
/// `size` 必须与过滤后可见表/空库单元数一致，否则 Len 断言会失败。
/// `is_full` 影响 policy 收集，与 ranges 构造侧参数保持同一语义。
fn schemas_from_meta(
    // size 参数应与预期表数一致，供进度条使用。
    // 共享夹具，闭包内再 clone 引用。
    meta: Arc<MemMeta>,
    // 与 BuildBackupRangeAndInitSchema 使用同一过滤语义。
    filter: Arc<dyn Filter>,
    // 备份快照 TS；测试常用 u64::MAX。
    backup_ts: u64,
    // 是否全量备份（决定是否收集 placement policy）。
    is_full: bool,
    // Schemas.Len 期望值，由调用方按场景给定。
    size: isize,
) -> crate::schema::Schemas {
    NewBackupSchemas(
        Arc::new(move |storage, fn_| {
            BuildBackupSchemas(
                storage,
                filter.as_ref(),
                backup_ts,
                is_full,
                meta.as_ref(),
                fn_,
            )
        }),
        size,
    )
}

/// 取出 MemMetaWriter 已缓冲的 schema 列表。
fn get_schemas_from_meta(mw: &MemMetaWriter) -> Vec<crate::stubs::backuppb::Schema> {
    // 克隆缓冲，避免持锁做断言。
    mw.schemas.lock().unwrap().clone()
}

/// Records whether `BackupSchemas` obtains the per-table stats writer from MetaWriter.
#[derive(Default)]
struct CountingMetaWriter {
    inner: MemMetaWriter,
    stats_writers: AtomicI64,
}

impl MetaWriter for CountingMetaWriter {
    fn StartWriteMetasAsync(&self, ctx: &Context, op: AppendOp) {
        self.inner.StartWriteMetasAsync(ctx, op);
    }

    fn Send(&self, data: MetaPayload, op: AppendOp) -> Result<()> {
        self.inner.Send(data, op)
    }

    fn FinishWriteMetas(&self, ctx: &Context, op: AppendOp) -> Result<()> {
        self.inner.FinishWriteMetas(ctx, op)
    }

    fn NewStatsWriter(&self) -> StatsWriter {
        self.stats_writers.fetch_add(1, Ordering::SeqCst);
        StatsWriter::default()
    }
}

/// Go calls `metaWriter.NewStatsWriter()` once for every table with a stats handle.
#[test]
fn backup_schemas_uses_meta_writer_stats_factory() {
    let meta = fixture_meta_with_test_db();
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(30, "t3")]);
    let kv = MemKvStorage { meta: meta.clone() };
    let schemas = schemas_from_meta(meta, Arc::new(AllFilter), u64::MAX, false, 1);
    let writer = CountingMetaWriter::default();

    schemas
        .BackupSchemas(
            &Context::Background(),
            &writer,
            None,
            &kv,
            Some(&MemStatsHandle::default()),
            u64::MAX,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            true,
            None,
        )
        .unwrap();

    assert_eq!(writer.stats_writers.load(Ordering::SeqCst), 1);
}

/// 多子场景：缺表仍有 schemas、缺库为 None、空库、单表 checksum、全量 policy。
/// TestBuildBackupRangeAndSchema
#[test]
fn test_build_backup_range_and_schema() {
    // 按 Go 子场景顺序推进：缺表 → 缺库 → 空库 → 单表 → 全量。
    // 初始只有 test 库，无表。
    let meta = fixture_meta_with_test_db();
    let kv = MemKvStorage { meta: meta.clone() };
    // 备份 TS 取最大，表示读最新快照语义。
    let ts = u64::MAX;

    // 匹配到库但无表：仍应返回 Some(schemas)。
    // Table t1 does not exist — empty matched DB still yields schemas.
    // 精确匹配单库单表的过滤器。
    struct TableFilter {
        schema: &'static str,
        table: &'static str,
    }
    impl Filter for TableFilter {
        // 大小写不敏感匹配库名。
        fn MatchSchema(&self, schema: &str) -> bool {
            schema.eq_ignore_ascii_case(self.schema)
        }
        // 库表同时匹配才通过。
        fn MatchTable(&self, schema: &str, table: &str) -> bool {
            schema.eq_ignore_ascii_case(self.schema) && table.eq_ignore_ascii_case(self.table)
        }
    }
    let t1_filter = TableFilter {
        schema: "test",
        table: "t1",
    };
    let (_r, backup_schemas, _) =
        BuildBackupRangeAndInitSchema(&kv, &t1_filter, ts, false, meta.as_ref()).unwrap();
    // 库存在但无目标表：仍返回 schemas 容器。
    assert!(backup_schemas.is_some());

    // 库不存在 → backup_schemas 为 None。
    // Database is not exist.
    let foo_filter = TableFilter {
        schema: "foo",
        table: "t1",
    };
    let (_r, backup_schemas, _) =
        BuildBackupRangeAndInitSchema(&kv, &foo_filter, ts, false, meta.as_ref()).unwrap();
    // 库不存在：None。
    assert!(backup_schemas.is_none());

    // 过滤系统库后匹配到空用户库，仍有 schemas。
    // Empty database / noFilter matching user DBs (test has no tables yet).
    // 排除系统库的过滤器。
    struct NoSysFilter;
    impl Filter for NoSysFilter {
        fn MatchSchema(&self, schema: &str) -> bool {
            !matches!(
                schema.to_ascii_lowercase().as_str(),
                "mysql" | "sys" | "information_schema" | "performance_schema"
            )
        }
        fn MatchTable(&self, schema: &str, _table: &str) -> bool {
            self.MatchSchema(schema)
        }
    }
    let (_r, backup_schemas, _) =
        BuildBackupRangeAndInitSchema(&kv, &NoSysFilter, ts, false, meta.as_ref()).unwrap();
    assert!(backup_schemas.is_some());

    // 插入 t1 与 placement policy，准备非全量/全量两套断言。
    // Create t1 + placement policy (MemMeta fixture).
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(10, "t1")]);
    // 放置策略仅在全量备份时收集。
    meta.policies.lock().unwrap().push(PolicyInfo {
        ID: 1,
        Name: CIStr::new("fivereplicas"),
    });

    let test_filter = AllowList {
        schemas: vec![],
        tables: vec![("test".into(), "t1".into())],
    };
    let (_r, backup_schemas, policies) =
        BuildBackupRangeAndInitSchema(&kv, &test_filter, ts, false, meta.as_ref()).unwrap();
    let backup_schemas = backup_schemas.expect("schemas");
    // 单表选中。
    assert_eq!(backup_schemas.Len(), 1);
    // 非全量不收集 placement policies。
    // not full backup → no policies collected
    // 非全量：policies 为空。
    assert_eq!(policies.len(), 0);

    let update_ch = simpleProgress::default();
    // 开启 checksum，依赖 inject 的响应值。
    // 开启 checksum，依赖 inject。
    let skip_checksum = false;
    let mw = MemMetaWriter::default();
    let ctx = Context::Background();
    // 注入非零 checksum，供后续字段断言。
    checksum::inject_checksum_response(checksum::ChecksumResponse {
        Checksum: 9,
        TotalKvs: 1,
        TotalBytes: 8,
    });
    let schemas = schemas_from_meta(meta.clone(), Arc::new(test_filter), ts, false, 1);
    schemas
        .BackupSchemas(
            &ctx,
            &mw,
            None,
            &kv,
            None,
            ts,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            skip_checksum,
            Some(&update_ch),
        )
        .unwrap();
    // 单表进度为 1，且 meta 写会话已结束。
    assert_eq!(update_ch.get(), 1);
    // FinishWriteMetas 应已调用。
    assert!(*mw.finished.lock().unwrap());

    let schemas_out = get_schemas_from_meta(&mw);
    assert_eq!(schemas_out.len(), 1);
    // checksum 三字段均应非零。
    assert_ne!(schemas_out[0].Crc64Xor, 0);
    assert_ne!(schemas_out[0].TotalKvs, 0);
    assert_ne!(schemas_out[0].TotalBytes, 0);

    // 全量备份应收集到 1 条 policy，两张表都写出。
    // Add t2; full backup collects policy.
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(10, "t1"), sample_table(11, "t2")]);
    let all_test = AllowList {
        schemas: vec!["test".into()],
        tables: vec![],
    };
    let (_r, backup_schemas, policies) =
        BuildBackupRangeAndInitSchema(&kv, &all_test, ts, true, meta.as_ref()).unwrap();
    let backup_schemas = backup_schemas.expect("schemas");
    // 全量两张用户表。
    assert_eq!(backup_schemas.Len(), 2);
    // 全量：收集到 1 条 placement policy。
    assert_eq!(policies.len(), 1);

    update_ch.reset();
    let mw2 = MemMetaWriter::default();
    // 注入非零 checksum，供后续字段断言。
    checksum::inject_checksum_response(checksum::ChecksumResponse {
        Checksum: 1,
        TotalKvs: 1,
        TotalBytes: 1,
    });
    // 多表场景下 inject 行为依赖 stub 实现，此处至少保证写出条数。
    // Second table needs another inject — executor takes once per Build. Inject per-table via default table.ID.
    let schemas2 = schemas_from_meta(meta.clone(), Arc::new(all_test), ts, true, 2);
    schemas2
        .BackupSchemas(
            &ctx,
            &mw2,
            None,
            &kv,
            None,
            ts,
            None,
            2,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            skip_checksum,
            Some(&update_ch),
        )
        .unwrap();
    // 两张表各推进一次进度。
    assert_eq!(update_ch.get(), 2);
    let schemas_out = get_schemas_from_meta(&mw2);
    assert_eq!(schemas_out.len(), 2);
    // 每张表 checksum 字段均非零。
    for s in &schemas_out {
        assert_ne!(s.Crc64Xor, 0, "{s:?}");
        assert_ne!(s.TotalKvs, 0, "{s:?}");
        assert_ne!(s.TotalBytes, 0, "{s:?}");
    }
}

/// 统计句柄损坏时备份仍应完成（错误被吞或降级，对齐 Go）。
/// TestBuildBackupRangeAndSchemaWithBrokenStats
#[test]
fn test_build_backup_range_and_schema_with_broken_stats() {
    // 无 stats handle 与有 MemStatsHandle 两条路径对照。
    let meta = fixture_meta_with_test_db();
    meta.tables
        .lock()
        .unwrap()
        .insert(1, vec![sample_table(30, "t3")]);
    let kv = MemKvStorage { meta: meta.clone() };
    let ts = u64::MAX;
    let f = AllowList {
        schemas: vec![],
        tables: vec![("test".into(), "t3".into())],
    };
    let (_r, backup_schemas, _) =
        BuildBackupRangeAndInitSchema(&kv, &f, ts, false, meta.as_ref()).unwrap();
    assert_eq!(backup_schemas.unwrap().Len(), 1);

    let update_ch = simpleProgress::default();
    let mw = MemMetaWriter::default();
    let ctx = Context::Background();
    // Use default executor checksum (table.ID) — avoid global inject races with parallel tests.
    // 避免全局 inject 与并行测试竞态。
    let schemas = schemas_from_meta(
        meta.clone(),
        Arc::new(AllowList {
            schemas: vec![],
            tables: vec![("test".into(), "t3".into())],
        }),
        ts,
        false,
        1,
    );
    schemas
        .BackupSchemas(
            &ctx,
            &mw,
            None,
            &kv,
            None, // broken stats → no stats handle
            // 无统计句柄：StatsIndex 应为空，但 checksum 仍可算出。
            ts,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            false,
            Some(&update_ch),
        )
        .unwrap();
    let schemas_out = get_schemas_from_meta(&mw);
    assert_eq!(schemas_out.len(), 1);
    // 无 handle 时不写统计索引。
    assert!(schemas_out[0].StatsIndex.is_empty());
    // checksum 三字段均应非零。
    assert_ne!(schemas_out[0].Crc64Xor, 0);
    assert_ne!(schemas_out[0].TotalKvs, 0);
    assert_ne!(schemas_out[0].TotalBytes, 0);
    // Table/Db JSON 不应为空。
    assert!(!schemas_out[0].Table.is_empty());
    assert!(!schemas_out[0].Db.is_empty());

    // Recover stats path with MemStatsHandle.
    // 恢复统计路径后，表/库字节与 checksum 应与无统计路径一致。
    update_ch.reset();
    let mw2 = MemMetaWriter::default();
    let handle = MemStatsHandle::default();
    let schemas2 = schemas_from_meta(
        meta.clone(),
        Arc::new(AllowList {
            schemas: vec![],
            tables: vec![("test".into(), "t3".into())],
        }),
        ts,
        false,
        1,
    );
    schemas2
        .BackupSchemas(
            &ctx,
            &mw2,
            None,
            &kv,
            Some(&handle),
            ts,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            false,
            Some(&update_ch),
        )
        .unwrap();
    let schemas2_out = get_schemas_from_meta(&mw2);
    assert_eq!(schemas2_out.len(), 1);
    // 有统计句柄时必须与 Go 一样产生可恢复的统计索引。
    assert_eq!(schemas2_out[0].StatsIndex.len(), 1);
    assert!(
        !schemas2_out[0].StatsIndex[0].InlineData.is_empty()
            || !schemas2_out[0].StatsIndex[0].name.is_empty()
    );
    // Preserve checksum / identity equivalence from Go.
    assert_eq!(schemas_out[0].Crc64Xor, schemas2_out[0].Crc64Xor);
    assert_eq!(schemas_out[0].TotalKvs, schemas2_out[0].TotalKvs);
    assert_eq!(schemas_out[0].TotalBytes, schemas2_out[0].TotalBytes);
    // 有无 stats handle 不应改变表/库编码。
    assert_eq!(schemas_out[0].Table, schemas2_out[0].Table);
    assert_eq!(schemas_out[0].Db, schemas2_out[0].Db);
    // 触达额外导入，防止未使用告警掩盖真实依赖。
    let _ = metautil::LockFile;
    let _ = AllFilter;
}

/// 系统表备份时库名应被改写为临时名。
/// TestBackupSchemasForSystemTable
#[test]
fn test_backup_schemas_for_system_table() {
    // mysql 系统库下多表：写出时库名应被临时化。
    let meta = Arc::new(MemMeta::default());
    meta.dbs.lock().unwrap().push(DBInfo {
        ID: 1,
        Name: CIStr::new("mysql"),
        PlacementPolicyRef: None,
    });
    // 规模对齐 Go 用例，确保改名逻辑覆盖批量。
    let system_tables_count = 32;
    let table_prefix = "systable";
    let mut tables = Vec::new();
    for i in 1..=system_tables_count {
        tables.push(sample_table(100 + i as i64, &format!("{table_prefix}{i}")));
    }
    meta.tables.lock().unwrap().insert(1, tables);

    let kv = MemKvStorage { meta: meta.clone() };
    let ts = u64::MAX;
    // Filter mysql.systable* — AllowList with schema mysql and tables matching prefix via MatchTable.
    // 仅匹配 mysql.systable* 前缀表。
    // Use AllFilter on schema mysql only via custom filter.
    // 系统库过滤器。
    struct SysFilter;
    impl Filter for SysFilter {
        fn MatchSchema(&self, schema: &str) -> bool {
            schema.eq_ignore_ascii_case("mysql")
        }
        fn MatchTable(&self, schema: &str, table: &str) -> bool {
            schema.eq_ignore_ascii_case("mysql") && table.starts_with("systable")
        }
    }
    let f = SysFilter;
    let (_r, backup_schemas, _) =
        BuildBackupRangeAndInitSchema(&kv, &f, ts, false, meta.as_ref()).unwrap();
    // 选表数量应等于插入的系统表数。
    assert_eq!(backup_schemas.unwrap().Len(), system_tables_count as isize);

    let ctx = Context::Background();
    let update_ch = simpleProgress::default();
    let mw = MemMetaWriter::default();
    let schemas = schemas_from_meta(
        meta.clone(),
        Arc::new(SysFilter),
        ts,
        false,
        system_tables_count as isize,
    );
    schemas
        .BackupSchemas(
            &ctx,
            &mw,
            None,
            &kv,
            None,
            ts,
            None,
            1,
            DEF_CHECKSUM_TABLE_CONCURRENCY,
            true, // skip checksum
            // 系统表路径跳过 checksum，焦点在库名改写。
            Some(&update_ch),
        )
        .unwrap();
    let schemas_out = get_schemas_from_meta(&mw);
    // 写出条数等于系统表数。
    assert_eq!(schemas_out.len(), system_tables_count);
    // 每条 Db JSON 反序列化后 Name 应为 TemporaryDBName(mysql)。
    for schema in &schemas_out {
        let db: DBInfo = serde_json::from_slice(&schema.Db).unwrap();
        let table: TableInfo = serde_json::from_slice(&schema.Table).unwrap();
        // 临时库名对齐 Go utils.TemporaryDBName。
        assert_eq!(crate::stubs::utils::TemporaryDBName("mysql"), db.Name);
        // 表名前缀保持 systable。
        assert!(table.Name.O.starts_with(table_prefix));
    }
    // 触达额外 stub 类型，稳定导入集合。
    let _ = MemStorage::new("mem:///unused".into());
    let _ = glue::AtomicProgress::default();
    let _ = Error::new("x");
    let _ = PartitionInfo {
        Definitions: vec![PartitionDefinition {
            ID: 1,
            Name: CIStr::new("p0"),
        }],
    };
}
