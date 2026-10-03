// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// conflict-resolution 步骤的独立回归：覆盖 Go 数据/唯一索引冲突、
// 元数据线格式、并发删除、框架生命周期及资源释放。
// 机械迁移草稿保留为原 Go fixture 的对照资料。

const _GO_CONFLICT_RESOLUTION_TEST_DRAFT: &str = r###"
// 这里描述 conflict-resolution step 如何准备冲突 KV fixture、运行 executor，并确认冲突行被删除。

// writeConflictKVFile 对应 Go 辅助函数：把一组冲突 KV 写入 simplesst writer，
// 并从 writer summary 中取回文件名构造 engineapi.ConflictInfo。
fn write_conflict_kv_file(
    t: &mut testing::T,
    codec: tikv::Codec,
    kv_group: &str,
    obj_store: storeapi::Storage,
    kvs: Vec<simplesst::KVPair>,
) -> engineapi::ConflictInfo {
    t.Helper();
    let ctx = context::Background();
    let mut summary: Option<simplesst::WriterSummary> = None;

    let mut writer = simplesst::NewWriterBuilder()
        .SetTiKVCodec(codec)
        .SetOnCloseFunc(|s: simplesst::WriterSummary| {
            // Go 闭包把 close summary 写回外层变量；这里保留资源收尾路径。
            summary = Some(s);
        })
        .Build(obj_store, "/test", kv_group);

    for kv in kvs.iter() {
        writer
            .WriteRow(ctx.clone(), kv.Key.clone(), kv.Value.clone(), None)
            .expect("Go require.NoError(t, w.WriteRow)");
    }
    writer.Close(ctx).expect("Go require.NoError(t, w.Close)");

    let summary = summary.expect("Go require.Len(t, summary.MultipleFilesStats, 1)");
    assert_eq!(1, summary.MultipleFilesStats.len());
    engineapi::ConflictInfo {
        Count: kvs.len() as u64,
        Files: vec![summary.MultipleFilesStats[0].Filenames[0][0].clone()],
        ..Default::default()
    }
}

// generateConflictKVFiles 对应 Go fixture 生成：为 data KV group 和 unique index KV group
// 分别构造冲突文件；3 组 data 记录重复两次，index KV 保留一份。
fn generate_conflict_kv_files(
    t: &mut testing::T,
    temp_dir: &str,
    tbl: table::Table,
    codec: tikv::Codec,
) -> importinto::KVGroupConflictInfos {
    t.Helper();
    let encode_cfg = encode::EncodingConfig {
        Table: tbl.clone(),
        UseIdentityAutoRowID: true,
        ..Default::default()
    };
    let controller = importer::LoadDataController {
        ASTArgs: importer::ASTArgs {},
        Plan: importer::Plan {},
        Table: tbl.clone(),
        ..Default::default()
    };
    let local_encoder = importer::NewTableKVEncoderForDupResolve(&encode_cfg, &controller)
        .expect("Go require.NoError(t, err)");

    let mut dup_data_kvs: Vec<simplesst::KVPair> = Vec::with_capacity(6);
    let mut dup_index_kvs: Vec<simplesst::KVPair> = Vec::with_capacity(3);
    for i in 0..3 {
        let dup_id = i + 1;
        let row = vec![
            types::NewDatum(dup_id),
            types::NewDatum(dup_id),
            types::NewDatum(dup_id),
        ];
        let dup_pairs = local_encoder
            .Encode(row, dup_id as i64)
            .expect("Go require.NoError(t, err2)");

        for pair in dup_pairs.Pairs.iter() {
            if tablecodec::IsRecordKey(pair.Key.clone()) {
                // Go 把 data KV 追加两次，使同一行的记录 KV 形成冲突。
                let kv = simplesst::KVPair { Key: pair.Key.clone(), Value: pair.Val.clone() };
                dup_data_kvs.push(kv.clone());
                dup_data_kvs.push(kv);
            } else {
                let index_id = tablecodec::DecodeIndexID(pair.Key.clone())
                    .expect("Go require.NoError(t, err)");
                if index_id == 2 {
                    dup_index_kvs.push(simplesst::KVPair {
                        Key: pair.Key.clone(),
                        Value: pair.Val.clone(),
                    });
                }
            }
        }
    }

    let ctx = context::Background();
    let obj_store = dxfhandle::NewObjStore(ctx, temp_dir).expect("Go require.NoError(t, err)");
    importinto::KVGroupConflictInfos {
        ConflictInfos: maplit::hashmap! {
            globalsort::DataKVGroup.to_string() => write_conflict_kv_file(t, codec.clone(), globalsort::DataKVGroup, obj_store.clone(), dup_data_kvs),
            globalsort::IndexID2KVGroup(2) => write_conflict_kv_file(t, codec, "2", obj_store, dup_index_kvs),
        },
    }
}

// conflictedKVHandleContext 对应 Go 测试上下文结构，集中保存 mock store、table、task meta
// 以及预生成的冲突 KV 文件信息，供两个 step executor 测试复用。
struct conflictedKVHandleContext {
    tempDir: String,
    store: tidbkv::Storage,
    logger: zap::Logger,
    tbl: table::Table,
    taskMeta: importinto::TaskMeta,
    tk: testkit::TestKit,
    conflictedKVInfo: importinto::KVGroupConflictInfos,
}

// prepareConflictedKVHandleContext 对应 Go 初始化辅助：创建 mock store、建表插入基础数据，
// 生成冲突 KV 文件，再组装 import-into TaskMeta。真实数据库动作在本 实现中都是占位。
fn prepare_conflicted_kv_handle_context(t: &mut testing::T) -> conflictedKVHandleContext {
    t.Helper();
    let temp_dir = t.TempDir();
    let store = testkit::CreateMockStore(t);
    let mut tk = testkit::NewTestKit(t, store.clone());
    tk.MustExec("use test");
    let domain = session::GetDomain(store.clone()).expect("Go require.NoError(t, err)");
    let ctx = context::Background();
    let logger = zap::Must(zap::NewDevelopment());

    tk.MustExec("create table tc(a bigint primary key clustered, b int, c int, index(b), unique(c))");
    tk.MustExec("insert into tc values (1,1,1), (2,2,2), (3,3,3), (4,4,4), (5,5,5)");
    tk.MustQuery("select * from tc")
        .Sort()
        .Check(testkit::Rows(vec!["1 1 1", "2 2 2", "3 3 3", "4 4 4", "5 5 5"]));
    let tbl = domain
        .InfoSchema()
        .TableByName(ctx, ast::NewCIStr("test"), ast::NewCIStr("tc"))
        .expect("Go require.NoError(t, err)");

    // Go 注释说明这些冲突 KV 并非真实世界状态，只用于制造测试文件。
    let conflicted_kv_info =
        generate_conflict_kv_files(t, &temp_dir, tbl.clone(), store.GetCodec());

    let task_meta = importinto::TaskMeta {
        Plan: importer::Plan {
            CloudStorageURI: temp_dir.clone(),
            TableInfo: tbl.Meta(),
            InImportInto: true,
            Format: importer::DataFormatCSV,
            ..Default::default()
        },
        // Go 只需要一条合法 SQL 来创建 TableImporter。
        Stmt: "import into tc from '/local/file.txt'".to_string(),
        ..Default::default()
    };

    conflictedKVHandleContext {
        tempDir: temp_dir,
        store,
        logger,
        tbl,
        taskMeta: task_meta,
        tk,
        conflictedKVInfo: conflicted_kv_info,
    }
}

// runConflictedKVHandleStep 对应 Go 并行执行辅助：配置 framework info，打开测试 failpoint，
// 然后依次调用 Init 和 RunSubtask。
fn run_conflicted_kv_handle_step(
    t: &mut testing::T,
    subtask: &mut proto::Subtask,
    step_exe: &mut dyn execute::StepExecutor,
) {
    t.Helper();
    // Go 这里分配 8 CPU 和 1GiB 内存，用于模拟并行资源。
    let resource = proto::StepResource {
        CPU: proto::NewAllocatable(8),
        Mem: proto::NewAllocatable(units::GiB),
    };
    execute::SetFrameworkInfo(
        step_exe,
        &proto::Task { TaskBase: proto::TaskBase { ID: 1, ..Default::default() }, ..Default::default() },
        &resource,
        None,
        None,
    );
    testfailpoint::Enable(
        t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/createTableImporterForTest",
        "return(true)",
    );
    let ctx = context::Background();
    step_exe.Init(ctx.clone()).expect("Go require.NoError(t, stepExe.Init)");
    step_exe
        .RunSubtask(ctx, subtask)
        .expect("Go require.NoError(t, stepExe.RunSubtask)");
}

// TestConflictResolutionStepExecutor 对应 Go 测试：执行冲突解析 step 后，
// 断言表中只剩未冲突的两行。
#[test]
fn test_conflict_resolution_step_executor() {
    let mut t = testing::T::new();
    let origin = config::GetGlobalConfig().TempDir.clone();
    defer! {
        // Go defer 恢复全局 TempDir；保留全局配置收尾语义。
        config::GetGlobalConfig().TempDir = origin;
    }
    config::GetGlobalConfig().TempDir = t.TempDir();

    let mut hdl_ctx = prepare_conflicted_kv_handle_context(&mut t);
    let st_meta = importinto::ConflictResolutionStepMeta {
        Infos: hdl_ctx.conflictedKVInfo.clone(),
        ..Default::default()
    };
    let bytes = json::Marshal(&st_meta).expect("Go require.NoError(t, err)");
    let mut st = proto::Subtask {
        SubtaskBase: proto::SubtaskBase {},
        Meta: bytes,
        ..Default::default()
    };
    let mut step_exe = importinto::NewConflictResolutionStepExecutor(
        &proto::TaskBase { RequiredSlots: 1, ..Default::default() },
        hdl_ctx.store.clone(),
        hdl_ctx.taskMeta.clone(),
        hdl_ctx.logger.clone(),
    );
    run_conflicted_kv_handle_step(&mut t, &mut st, &mut step_exe);

    hdl_ctx
        .tk
        .MustQuery("select * from tc")
        .Sort()
        .Check(testkit::Rows(vec!["4 4 4", "5 5 5"]));
}
"###;

use crate::{ConflictResolutionStepMeta, KVGroupConflictInfos};

/// 序列化后的元数据须保留 `conflict-infos` 字段名，与 Go 侧 wire 格式兼容。
#[test]
fn conflict_resolution_meta_keeps_group_map_on_wire() {
    let meta = ConflictResolutionStepMeta {
        Infos: KVGroupConflictInfos::default(),
        ..ConflictResolutionStepMeta::default()
    };
    let json = String::from_utf8(meta.Marshal().unwrap()).unwrap();
    assert!(json.contains("\"conflict-infos\""));
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use astersql_dxf_importinto_conflictedkv::{
    ConflictContext, ConflictRowCodec, ConflictSnapshot, ConflictStore, ConflictTransaction,
};
use astersql_ingestor_globalsort::{KvPair as SortKvPair, MemoryStorage, Storage, encode_kvs};
use astersql_kv::tikvstore::ValueEntry;
use astersql_kv::{Handle, IntHandle, Key, Version};
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::KvPair;
use astersql_meta_model::TableInfo;
use astersql_types::datum::{Datum, NewIntDatum};

struct ResolutionCodec;

#[derive(Default)]
struct ResolutionCounter(AtomicI64);
impl astersql_dxf_framework_taskexecutor_execute::Collector for ResolutionCounter {
    fn Accepted(&self, _accepted: i64) {}
    fn Processed(&self, processed: i64, _bytes: i64) {
        self.0.fetch_add(processed, Ordering::SeqCst);
    }
}

impl ConflictRowCodec for ResolutionCodec {
    fn StripKeyspacePrefix(&self, key: &Key) -> Result<Key, String> {
        Ok(key.clone())
    }
    fn DecodeRowKey(&self, key: &Key) -> Result<Box<dyn Handle>, String> {
        let text = std::str::from_utf8(&key.0).map_err(|err| err.to_string())?;
        let id = text
            .strip_prefix("row:1:")
            .ok_or_else(|| "invalid row key".to_owned())?
            .parse::<i64>()
            .map_err(|err| err.to_string())?;
        Ok(Box::new(IntHandle(id)))
    }
    fn DecodeRow(&self, handle: &dyn Handle, _value: &[u8]) -> Result<Vec<Datum>, String> {
        Ok(vec![NewIntDatum(handle.IntValue())])
    }
    fn DecodeTableID(&self, _key: &Key) -> i64 {
        1
    }
    fn DecodeIndexHandle(
        &self,
        _key: &Key,
        _value: &[u8],
        _cols: usize,
    ) -> Result<Box<dyn Handle>, String> {
        Err("not an index group".to_owned())
    }
    fn EncodeRowKey(&self, table_id: i64, handle: &dyn Handle) -> Key {
        Key(format!("row:{table_id}:{}", handle.IntValue()).into_bytes())
    }
    fn EncodeRow(
        &mut self,
        handle: &dyn Handle,
        _row: &[Datum],
        _auto_row_id: i64,
    ) -> Result<Pairs, String> {
        Ok(Pairs {
            Pairs: vec![KvPair {
                key: self.EncodeRowKey(1, handle).0,
                val: vec![1],
            }],
            RowID: Vec::new(),
        })
    }
    fn Close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

struct ResolutionSnapshot(Arc<Mutex<HashMap<Vec<u8>, ValueEntry>>>);
impl ConflictSnapshot for ResolutionSnapshot {
    fn BatchGet(
        &self,
        _context: &ConflictContext,
        keys: &[Key],
    ) -> Result<HashMap<Vec<u8>, ValueEntry>, String> {
        let rows = self.0.lock().unwrap();
        Ok(keys
            .iter()
            .filter_map(|key| {
                rows.get(&key.0)
                    .cloned()
                    .map(|value| (key.0.clone(), value))
            })
            .collect())
    }
}

struct ResolutionTxn {
    rows: Arc<Mutex<HashMap<Vec<u8>, ValueEntry>>>,
    pending: Vec<Key>,
}
impl ConflictTransaction for ResolutionTxn {
    fn Delete(&mut self, key: &Key) -> Result<(), String> {
        self.pending.push(key.clone());
        Ok(())
    }
    fn Commit(self: Box<Self>, _context: &ConflictContext) -> Result<(), String> {
        let mut rows = self.rows.lock().unwrap();
        for key in &self.pending {
            rows.remove(&key.0);
        }
        Ok(())
    }
    fn Rollback(self: Box<Self>) -> Result<(), String> {
        Ok(())
    }
}

struct ResolutionStore(Arc<Mutex<HashMap<Vec<u8>, ValueEntry>>>);
impl ConflictStore for ResolutionStore {
    fn Keyspace(&self) -> Vec<u8> {
        Vec::new()
    }
    fn CurrentVersion(&self) -> Result<Version, String> {
        Ok(astersql_kv::NewVersion(1))
    }
    fn GetSnapshot(&self, _version: Version) -> Box<dyn ConflictSnapshot> {
        Box::new(ResolutionSnapshot(self.0.clone()))
    }
    fn Begin(&self) -> Result<Box<dyn ConflictTransaction>, String> {
        Ok(Box::new(ResolutionTxn {
            rows: self.0.clone(),
            pending: Vec::new(),
        }))
    }
    fn IsRetryableError(&self, _error: &str) -> bool {
        false
    }
}

#[test]
fn conflict_resolution_reads_files_and_deletes_conflicted_rows() {
    let rows = Arc::new(Mutex::new(
        (1..=5)
            .map(|id| {
                (
                    format!("row:1:{id}").into_bytes(),
                    ValueEntry {
                        Value: vec![1],
                        CommitTs: 0,
                    },
                )
            })
            .collect::<HashMap<_, _>>(),
    ));
    let cluster: Arc<dyn ConflictStore> = Arc::new(ResolutionStore(rows.clone()));
    let objects: Arc<dyn Storage> = Arc::new(crate::conflict_resolution::ConflictObjectStorage {
        store: Arc::new(astersql_objstore::memstore::NewMemStorage()),
        context: astersql_objstore::storage::Context::default(),
        access: None,
    });
    let pairs = (1..=3)
        .flat_map(|id| [id, id])
        .map(|id| SortKvPair {
            key: format!("row:1:{id}").into_bytes(),
            value: vec![1],
        })
        .collect::<Vec<_>>();
    objects
        .write("/test/conflicts.data", encode_kvs(&pairs))
        .unwrap();
    let info = astersql_ingestor_engineapi::ConflictInfo {
        Count: pairs.len() as u64,
        Files: vec!["/test/conflicts.data".to_owned()],
        ..Default::default()
    };
    let counter = Arc::new(ResolutionCounter::default());
    crate::conflict_resolution::ResolveConflictGroup(
        &ConflictContext::default(),
        objects,
        cluster,
        Arc::new(TableInfo {
            ID: 1,
            PKIsHandle: true,
            ..Default::default()
        }),
        "data",
        &info,
        vec![Box::new(ResolutionCodec), Box::new(ResolutionCodec)],
        Some(counter.clone()),
        None,
    )
    .unwrap();
    assert_eq!(6, counter.0.load(Ordering::SeqCst));
    let remaining = rows.lock().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(2, remaining.len());
    assert!(remaining.contains(&b"row:1:4".to_vec()));
    assert!(remaining.contains(&b"row:1:5".to_vec()));
}

#[test]
fn conflict_resolution_propagates_reader_error_and_cancellation() {
    let rows = Arc::new(Mutex::new(HashMap::new()));
    let cluster: Arc<dyn ConflictStore> = Arc::new(ResolutionStore(rows));
    let objects: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let info = astersql_ingestor_engineapi::ConflictInfo {
        Files: vec!["/test/missing.data".to_owned()],
        ..Default::default()
    };
    let table = Arc::new(TableInfo {
        ID: 1,
        PKIsHandle: true,
        ..Default::default()
    });
    let error = crate::conflict_resolution::ResolveConflictGroup(
        &ConflictContext::default(),
        objects.clone(),
        cluster.clone(),
        table.clone(),
        "data",
        &info,
        vec![Box::new(ResolutionCodec)],
        None,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("object not found"), "{error}");

    let cancelled = ConflictContext::default();
    cancelled.Cancel();
    let error = crate::conflict_resolution::ResolveConflictGroup(
        &cancelled,
        objects,
        cluster,
        table,
        "data",
        &info,
        vec![Box::new(ResolutionCodec)],
        None,
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
}

#[test]
fn importer_codec_reencodes_a_real_table_row() {
    use astersql_executor_importer::{
        CanonicalImportDatumConverter, NewTableDefinitionFromMeta, NewTableKVEncoderFromMeta,
    };
    use astersql_lightning_backend_encode::{Datum as EncodeDatum, EncodingConfig, SessionOptions};
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, ast};
    use astersql_parser_mysql::r#type::TypeLonglong;
    use astersql_types::StrictContext;

    let mut column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("a"),
        State: StatePublic,
        ..Default::default()
    };
    column.SetType(TypeLonglong);
    let primary = IndexInfo {
        ID: 1,
        Name: ast::NewCIStr("PRIMARY"),
        State: StatePublic,
        Primary: true,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("a"),
            Offset: 0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let meta = TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        PKIsHandle: true,
        Columns: vec![column],
        Indices: vec![primary],
        ..Default::default()
    };
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&meta).unwrap())),
        UseIdentityAutoRowID: true,
        SessionOptions: SessionOptions::default(),
        ..EncodingConfig::default()
    };
    let mut encoder = NewTableKVEncoderFromMeta(
        &config,
        &meta,
        Arc::new(CanonicalImportDatumConverter(StrictContext.Flags())),
    )
    .unwrap();
    let pairs = encoder.Encode(&[EncodeDatum::Int(1)], 1).unwrap();
    let record = pairs
        .Pairs
        .iter()
        .find(|pair| astersql_tablecodec::IsRecordKey(&pair.key))
        .unwrap();
    let mut codec = crate::conflict_resolution::NewImporterConflictCodec(encoder, &meta).unwrap();
    let handle = codec.DecodeRowKey(&Key(record.key.clone())).unwrap();
    let row = codec.DecodeRow(handle.as_ref(), &record.val).unwrap();
    let rebuilt = codec.EncodeRow(handle.as_ref(), &row, 0).unwrap();
    assert!(rebuilt.Pairs.iter().any(|pair| pair.key == record.key));
    assert_eq!(
        "invalid index key",
        codec
            .DecodeIndexHandle(&Key(Vec::new()), &[], 0)
            .err()
            .unwrap(),
    );
    let partition = astersql_kv::NewPartitionHandle(42, Box::new(IntHandle(1)));
    let physical_key = codec.EncodeRowKey(meta.ID, &partition);
    assert_eq!(
        42,
        astersql_tablecodec::DecodeTableID(astersql_tablecodec::kv::Key(physical_key.0))
    );
    codec.Close().unwrap();
}

#[test]
fn worker_local_importer_codecs_delete_three_real_conflicted_rows() {
    use astersql_executor_importer::{
        CanonicalImportDatumConverter, NewTableDefinitionFromMeta, NewTableKVEncoderFromMeta,
    };
    use astersql_lightning_backend_encode::{Datum as EncodeDatum, EncodingConfig, SessionOptions};
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, ast};
    use astersql_parser_mysql::r#type::TypeLonglong;
    use astersql_types::StrictContext;

    let mut column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("a"),
        State: StatePublic,
        ..Default::default()
    };
    column.SetType(TypeLonglong);
    let primary = IndexInfo {
        ID: 1,
        Name: ast::NewCIStr("PRIMARY"),
        State: StatePublic,
        Primary: true,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("a"),
            Offset: 0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let meta = Arc::new(TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        PKIsHandle: true,
        Columns: vec![column],
        Indices: vec![primary],
        ..Default::default()
    });
    let options = SessionOptions::default();
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&meta).unwrap())),
        UseIdentityAutoRowID: true,
        SessionOptions: options.clone(),
        ..EncodingConfig::default()
    };
    let converter = Arc::new(CanonicalImportDatumConverter(StrictContext.Flags()));
    let mut fixture_encoder = NewTableKVEncoderFromMeta(&config, &meta, converter.clone()).unwrap();
    let mut existing = HashMap::new();
    let mut conflicted = Vec::new();
    for id in 1..=5 {
        let pairs = fixture_encoder.Encode(&[EncodeDatum::Int(id)], id).unwrap();
        let record = pairs
            .Pairs
            .iter()
            .find(|pair| astersql_tablecodec::IsRecordKey(&pair.key))
            .unwrap();
        existing.insert(
            record.key.clone(),
            ValueEntry {
                Value: record.val.clone(),
                CommitTs: 0,
            },
        );
        if id <= 3 {
            let pair = SortKvPair {
                key: record.key.clone(),
                value: record.val.clone(),
            };
            conflicted.extend([pair.clone(), pair]);
        }
    }
    fixture_encoder.Close().unwrap();
    conflicted.sort_by(|a, b| a.key.cmp(&b.key));
    let rows = Arc::new(Mutex::new(existing));
    let cluster: Arc<dyn ConflictStore> = Arc::new(ResolutionStore(rows.clone()));
    let objects: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    objects
        .write("/test/real-conflicts.data", encode_kvs(&conflicted))
        .unwrap();
    let info = astersql_ingestor_engineapi::ConflictInfo {
        Count: conflicted.len() as u64,
        Files: vec!["/test/real-conflicts.data".to_owned()],
        ..Default::default()
    };
    let counter = Arc::new(ResolutionCounter::default());
    crate::conflict_resolution::ResolveConflictGroupFromMeta(
        &ConflictContext::default(),
        objects,
        cluster,
        meta,
        "data",
        &info,
        2,
        options,
        converter,
        Some(counter.clone()),
        None,
    )
    .unwrap();
    assert_eq!(6, counter.0.load(Ordering::SeqCst));
    assert_eq!(2, rows.lock().unwrap().len());
}

#[test]
fn unique_index_conflict_group_deletes_three_real_rows() {
    use astersql_executor_importer::{
        CanonicalImportDatumConverter, NewTableDefinitionFromMeta, NewTableKVEncoderFromMeta,
    };
    use astersql_lightning_backend_encode::{Datum as EncodeDatum, EncodingConfig, SessionOptions};
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, ast};
    use astersql_parser_mysql::r#type::TypeLonglong;
    use astersql_types::StrictContext;

    let column = |id, name: &str, offset| {
        let mut column = ColumnInfo {
            ID: id,
            Name: ast::NewCIStr(name),
            Offset: offset,
            State: StatePublic,
            ..Default::default()
        };
        column.SetType(TypeLonglong);
        column
    };
    let index = |id, name: &str, column: &str, offset| IndexInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        State: StatePublic,
        Primary: id == 1,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr(column),
            Offset: offset,
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = Arc::new(TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        PKIsHandle: true,
        Columns: vec![column(1, "a", 0), column(2, "b", 1)],
        Indices: vec![index(1, "PRIMARY", "a", 0), index(2, "u", "b", 1)],
        ..Default::default()
    });
    let options = SessionOptions::default();
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&table).unwrap())),
        UseIdentityAutoRowID: true,
        SessionOptions: options.clone(),
        ..EncodingConfig::default()
    };
    let converter = Arc::new(CanonicalImportDatumConverter(StrictContext.Flags()));
    let mut fixture = NewTableKVEncoderFromMeta(&config, &table, converter.clone()).unwrap();
    let mut existing = HashMap::new();
    let mut index_conflicts = Vec::new();
    for id in 1..=5 {
        let pairs = fixture
            .Encode(&[EncodeDatum::Int(id), EncodeDatum::Int(id)], id)
            .unwrap();
        for pair in &pairs.Pairs {
            existing.insert(
                pair.key.clone(),
                ValueEntry {
                    Value: pair.val.clone(),
                    CommitTs: 0,
                },
            );
            if id <= 3 && !astersql_tablecodec::IsRecordKey(&pair.key) {
                let index_id = astersql_tablecodec::DecodeIndexID(astersql_tablecodec::kv::Key(
                    pair.key.clone(),
                ))
                .unwrap();
                if index_id == 2 {
                    index_conflicts.push(SortKvPair {
                        key: pair.key.clone(),
                        value: pair.val.clone(),
                    });
                }
            }
        }
    }
    fixture.Close().unwrap();
    assert_eq!(3, index_conflicts.len());
    index_conflicts.sort_by(|a, b| a.key.cmp(&b.key));
    let rows = Arc::new(Mutex::new(existing));
    let cluster: Arc<dyn ConflictStore> = Arc::new(ResolutionStore(rows.clone()));
    let objects: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    objects
        .write("/test/index-conflicts.data", encode_kvs(&index_conflicts))
        .unwrap();
    let info = astersql_ingestor_engineapi::ConflictInfo {
        Count: 3,
        Files: vec!["/test/index-conflicts.data".to_owned()],
    };
    // Exercise the same non-empty SST/snapshot rows through the collector first.
    let shared = Arc::new(AtomicI64::new(0));
    let global = Arc::new(astersql_dxf_importinto_conflictedkv::NewBoundedKeySet(
        shared.clone(),
        1 << 20,
    ));
    let directory = std::env::temp_dir().join(format!(
        "astersql-conflicts-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Arc::new(astersql_objstore::local::NewLocalStorage(&directory).unwrap());
    let make_codec = || {
        let config = EncodingConfig {
            Table: Some(Arc::new(NewTableDefinitionFromMeta(&table).unwrap())),
            UseIdentityAutoRowID: true,
            SessionOptions: options.clone(),
            ..Default::default()
        };
        let encoder = NewTableKVEncoderFromMeta(&config, &table, converter.clone())?;
        let codec = crate::conflict_resolution::NewImporterConflictCodecWithOptions(
            encoder, &table, &options,
        )
        .map_err(|error| error.to_string())?;
        Ok(Box::new(codec) as Box<dyn ConflictRowCodec>)
    };
    let collected_progress = Arc::new(ResolutionCounter::default());
    let (collected, local) = crate::collect_conflicts::CollectConflictGroup(
        &ConflictContext::default(),
        objects.clone(),
        output.clone(),
        cluster.clone(),
        table.clone(),
        "2",
        &info,
        2,
        &make_codec,
        "conflicted-rows",
        global.clone(),
        shared.clone(),
        1 << 20,
        Arc::new(AtomicI64::new(0)),
        Some(collected_progress.clone()),
        None,
    )
    .unwrap();
    assert_eq!(3, collected.RowCount);
    assert_eq!(3, local.Len());
    assert_eq!(9, collected.Checksum.SumKVS());
    let content: Vec<_> = collected
        .Filenames
        .iter()
        .map(|name| std::fs::read(directory.join(name)).unwrap())
        .collect();
    assert_eq!(
        3,
        content
            .iter()
            .map(|bytes| bytes.iter().filter(|&&byte| byte == b'\n').count())
            .sum::<usize>()
    );
    assert_eq!(3, collected_progress.0.load(Ordering::SeqCst));
    let (repeated, _) = crate::collect_conflicts::CollectConflictGroup(
        &ConflictContext::default(),
        objects.clone(),
        output.clone(),
        cluster.clone(),
        table.clone(),
        "2",
        &info,
        2,
        &make_codec,
        "conflicted-rows",
        global,
        shared,
        1 << 20,
        Arc::new(AtomicI64::new(0)),
        Some(collected_progress.clone()),
        None,
    )
    .unwrap();
    assert_eq!(3, repeated.RowCount);
    assert_eq!(6, collected_progress.0.load(Ordering::SeqCst));
    assert!(
        repeated
            .Filenames
            .iter()
            .all(|name| !collected.Filenames.contains(name))
    );
    std::fs::remove_dir_all(directory).unwrap();
    let progress = Arc::new(ResolutionCounter::default());
    crate::conflict_resolution::ResolveConflictGroupFromMeta(
        &ConflictContext::default(),
        objects,
        cluster,
        table,
        "2",
        &info,
        2,
        options,
        converter,
        Some(progress.clone()),
        None,
    )
    .unwrap();
    assert_eq!(3, progress.0.load(Ordering::SeqCst));
    assert_eq!(
        2,
        rows.lock()
            .unwrap()
            .keys()
            .filter(|key| astersql_tablecodec::IsRecordKey(key))
            .count()
    );
}

#[test]
fn resolution_meta_loads_inline_and_external_conflict_infos() {
    let mut inline = ConflictResolutionStepMeta::default();
    inline.Infos.ConflictInfos.insert(
        "data".to_owned(),
        astersql_ingestor_engineapi::ConflictInfo {
            Count: 2,
            Files: vec!["/test/conflicts.data".to_owned()],
        },
    );
    let objects: Arc<dyn Storage> = Arc::new(MemoryStorage::default());
    let inline_bytes = inline.Marshal().unwrap();
    let decoded =
        crate::conflict_resolution::ReadConflictResolutionMeta(&inline_bytes, objects.as_ref())
            .unwrap();
    assert_eq!(2, decoded.Infos.ConflictInfos["data"].Count);

    objects.write("/test/meta.json", inline_bytes).unwrap();
    let mut external = ConflictResolutionStepMeta::default();
    external.BaseExternalMeta.ExternalPath = "/test/meta.json".to_owned();
    let decoded = crate::conflict_resolution::ReadConflictResolutionMeta(
        &external.Marshal().unwrap(),
        objects.as_ref(),
    )
    .unwrap();
    assert_eq!(
        vec!["/test/conflicts.data"],
        decoded.Infos.ConflictInfos["data"].Files
    );
}

#[test]
fn conflict_resolution_step_dispatch_checks_task_step_and_meta() {
    use astersql_dxf_framework_proto::{
        step,
        task::{Task, TaskBase},
    };

    struct UnusedRuntime;
    impl crate::conflict_resolution::ConflictResolutionRuntime for UnusedRuntime {
        fn BuildImporter(
            &self,
            _: i64,
            _: &crate::proto::TaskMeta,
        ) -> Result<Box<dyn crate::conflict_resolution::ConflictResolutionImporter>, String>
        {
            Err("unused".to_owned())
        }
        fn OpenObjectStore(
            &self,
            _: &astersql_dxf_framework_taskexecutor_execute::Context,
            _: &str,
        ) -> Result<astersql_objstore::storage::StorageRef, String> {
            Err("unused".to_owned())
        }
        fn ClusterStore(&self) -> Arc<dyn ConflictStore> {
            Arc::new(ResolutionStore(Arc::new(Mutex::new(HashMap::new()))))
        }
    }

    fn task(step_number: step::Step, meta: Vec<u8>) -> Task {
        use std::time::SystemTime;
        Task {
            TaskBase: TaskBase {
                ID: 42,
                Key: "import".to_owned(),
                Type: "import-into",
                State: "running",
                Step: step_number,
                Priority: 1,
                RequiredSlots: 2,
                TargetScope: String::new(),
                CreateTime: SystemTime::UNIX_EPOCH,
                MaxNodeCount: 1,
                ExtraParams: Default::default(),
                Keyspace: String::new(),
            },
            SchedulerID: String::new(),
            StartTime: SystemTime::UNIX_EPOCH,
            StateUpdateTime: SystemTime::UNIX_EPOCH,
            Meta: meta,
            Error: None,
            ModifyParam: astersql_dxf_framework_proto::modify::ModifyParam {
                PrevState: "running",
                Modifications: Vec::new(),
            },
        }
    }
    let valid_meta = crate::proto::TaskMeta::default().Marshal().unwrap();
    let valid_task = task(step::ImportStepConflictResolution, valid_meta.clone());
    let runtime = Arc::new(UnusedRuntime);
    assert!(
        crate::task_executor::GetConflictResolutionStepExecutor(&valid_task, runtime.clone())
            .is_ok()
    );
    let wrong_step = task(step::StepInit, valid_meta);
    assert!(
        crate::task_executor::GetConflictResolutionStepExecutor(&wrong_step, runtime.clone())
            .is_err()
    );
    let invalid_meta = task(step::ImportStepConflictResolution, b"{".to_vec());
    assert!(
        crate::task_executor::GetConflictResolutionStepExecutor(&invalid_meta, runtime).is_err()
    );
}

#[test]
fn conflict_resolution_task_meta_preserves_table_columns_and_indexes() {
    use astersql_meta_model::{ColumnInfo, IndexInfo, TableInfo, ast};
    let table = Arc::new(TableInfo {
        ID: 9,
        Name: ast::NewCIStr("target"),
        Columns: vec![ColumnInfo {
            ID: 1,
            Name: ast::NewCIStr("a"),
            ..Default::default()
        }],
        Indices: vec![IndexInfo {
            ID: 2,
            Name: ast::NewCIStr("u"),
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut meta = crate::proto::TaskMeta::default();
    meta.Plan.TableInfo = Some(table);
    meta.Plan.SQLMode = astersql_parser_mysql::r#const::SQLMode(1024);
    meta.Plan
        .ImportantSysVars
        .insert("time_zone".to_owned(), "+08:00".to_owned());
    meta.Plan.DiskQuota = astersql_executor_importer::ByteSize(1234);
    meta.Plan.InImportInto = true;
    let decoded = crate::proto::TaskMeta::Unmarshal(&meta.Marshal().unwrap()).unwrap();
    assert_eq!(1024, decoded.Plan.SQLMode.0);
    assert_eq!("+08:00", decoded.Plan.ImportantSysVars["time_zone"]);
    assert_eq!(1234, decoded.Plan.DiskQuota.0);
    assert!(decoded.Plan.InImportInto);
    let table = decoded.Plan.TableInfo.unwrap();
    assert_eq!(1, table.Columns.len());
    assert_eq!(2, table.Indices[0].ID);

    let go_wire = serde_json::json!({
        "JobID": 7,
        "Plan": {
            "TableInfo": table.as_ref(),
            "CloudStorageURI": "memstore://conflicts",
            "DBName": "test",
            "SQLMode": 2048,
            "ImportantSysVars": {"time_zone":"UTC"},
            "OnDupKey": "capture",
            "DataSourceType": "file"
        },
        "Stmt": "IMPORT INTO target FROM 'file.csv'",
        "Summary": {"resolve-conflicts-summary":{"input-rows":3}},
        "ChunkMap": {"1":[{"Path":"file.csv","FileSize":42,"Type":4,"Compression":0}]}
    });
    let decoded =
        crate::proto::TaskMeta::Unmarshal(&serde_json::to_vec(&go_wire).unwrap()).unwrap();
    assert_eq!(7, decoded.JobID);
    assert_eq!("memstore://conflicts", decoded.Plan.CloudStorageURI);
    assert_eq!(2048, decoded.Plan.SQLMode.0);
    assert_eq!("UTC", decoded.Plan.ImportantSysVars["time_zone"]);
    assert_eq!(
        astersql_executor_importer::OnDupKeyModeCapture,
        decoded.Plan.OnDupKey
    );
    assert_eq!(3, decoded.Summary.ResolveConflictsSummary.RowCnt);
    assert_eq!(42, decoded.ChunkMap[&1][0].FileSize);
    assert_eq!(1, decoded.Plan.TableInfo.unwrap().Columns.len());
}

#[test]
fn framework_conflict_resolution_lifecycle_deletes_rows_and_closes_resources() {
    use astersql_dxf_framework_proto::{
        step,
        subtask::{NewAllocatable, NewSubtask, StepResource},
        task::{Task, TaskBase},
    };
    use astersql_dxf_framework_taskexecutor_execute as execute;
    use astersql_executor_importer::{
        CanonicalImportDatumConverter, ImportDatumConverter, NewTableDefinitionFromMeta,
        NewTableKVEncoderFromMeta, Plan,
    };
    use astersql_lightning_backend_encode::{Datum as EncodeDatum, EncodingConfig, SessionOptions};
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, ast};
    use astersql_parser_mysql::r#type::TypeLonglong;
    use astersql_types::StrictContext;
    use std::sync::atomic::AtomicBool;
    use std::time::SystemTime;

    let mut column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("a"),
        State: StatePublic,
        ..Default::default()
    };
    column.SetType(TypeLonglong);
    let mut index_column = ColumnInfo {
        ID: 2,
        Name: ast::NewCIStr("b"),
        Offset: 1,
        State: StatePublic,
        ..Default::default()
    };
    index_column.SetType(TypeLonglong);
    let primary = IndexInfo {
        ID: 1,
        Name: ast::NewCIStr("PRIMARY"),
        State: StatePublic,
        Primary: true,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("a"),
            Offset: 0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let unique = IndexInfo {
        ID: 2,
        Name: ast::NewCIStr("u"),
        State: StatePublic,
        Unique: true,
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("b"),
            Offset: 1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = Arc::new(TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        PKIsHandle: true,
        Columns: vec![column, index_column],
        Indices: vec![primary, unique],
        ..Default::default()
    });
    let converter: Arc<dyn ImportDatumConverter> =
        Arc::new(CanonicalImportDatumConverter(StrictContext.Flags()));
    let config = EncodingConfig {
        Table: Some(Arc::new(NewTableDefinitionFromMeta(&table).unwrap())),
        UseIdentityAutoRowID: true,
        SessionOptions: SessionOptions::default(),
        ..EncodingConfig::default()
    };
    let mut fixture = NewTableKVEncoderFromMeta(&config, &table, converter.clone()).unwrap();
    let mut existing = HashMap::new();
    let mut conflicts = Vec::new();
    let mut index_conflicts = Vec::new();
    for id in 1..=5 {
        let pairs = fixture
            .Encode(&[EncodeDatum::Int(id), EncodeDatum::Int(id)], id)
            .unwrap();
        for pair in &pairs.Pairs {
            existing.insert(
                pair.key.clone(),
                ValueEntry {
                    Value: pair.val.clone(),
                    CommitTs: 0,
                },
            );
            if id <= 3 && !astersql_tablecodec::IsRecordKey(&pair.key) {
                if astersql_tablecodec::DecodeIndexID(astersql_tablecodec::kv::Key(
                    pair.key.clone(),
                ))
                .unwrap()
                    == 2
                {
                    index_conflicts.push(SortKvPair {
                        key: pair.key.clone(),
                        value: pair.val.clone(),
                    });
                }
            }
        }
        let record = pairs
            .Pairs
            .iter()
            .find(|pair| astersql_tablecodec::IsRecordKey(&pair.key))
            .unwrap();
        if id <= 3 {
            let pair = SortKvPair {
                key: record.key.clone(),
                value: record.val.clone(),
            };
            conflicts.extend([pair.clone(), pair]);
        }
    }
    fixture.Close().unwrap();
    assert_eq!(3, index_conflicts.len());
    conflicts.sort_by(|a, b| a.key.cmp(&b.key));
    index_conflicts.sort_by(|a, b| a.key.cmp(&b.key));
    let original_rows = existing.clone();
    let rows = Arc::new(Mutex::new(existing));
    let raw_store: astersql_objstore::storage::StorageRef =
        Arc::new(astersql_objstore::memstore::NewMemStorage());
    let storage_context = astersql_objstore::storage::Context::default();
    raw_store
        .WriteFile(
            &storage_context,
            "/test/conflicts.data",
            &encode_kvs(&conflicts),
        )
        .unwrap();

    raw_store
        .WriteFile(
            &storage_context,
            "/test/index-conflicts.data",
            &encode_kvs(&index_conflicts),
        )
        .unwrap();

    struct FixtureImporter {
        plan: Plan,
        table: Arc<TableInfo>,
        converter: Arc<dyn ImportDatumConverter>,
        closed: Arc<AtomicBool>,
    }
    impl crate::conflict_resolution::ConflictResolutionImporter for FixtureImporter {
        fn Plan(&self) -> &Plan {
            &self.plan
        }
        fn TableInfo(&self) -> Arc<TableInfo> {
            self.table.clone()
        }
        fn DatumConverter(&self) -> Arc<dyn ImportDatumConverter> {
            self.converter.clone()
        }
        fn Close(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }
    struct FixtureRuntime {
        table: Arc<TableInfo>,
        converter: Arc<dyn ImportDatumConverter>,
        store: astersql_objstore::storage::StorageRef,
        cluster: Arc<dyn ConflictStore>,
        closed: Arc<AtomicBool>,
    }
    impl crate::conflict_resolution::ConflictResolutionRuntime for FixtureRuntime {
        fn BuildImporter(
            &self,
            _: i64,
            _: &crate::proto::TaskMeta,
        ) -> Result<Box<dyn crate::conflict_resolution::ConflictResolutionImporter>, String>
        {
            Ok(Box::new(FixtureImporter {
                plan: Plan {
                    CloudStorageURI: "memstore://fixture".to_owned(),
                    ..Default::default()
                },
                table: self.table.clone(),
                converter: self.converter.clone(),
                closed: self.closed.clone(),
            }))
        }
        fn OpenObjectStore(
            &self,
            _: &execute::Context,
            _: &str,
        ) -> Result<astersql_objstore::storage::StorageRef, String> {
            Ok(self.store.clone())
        }
        fn ClusterStore(&self) -> Arc<dyn ConflictStore> {
            self.cluster.clone()
        }
    }
    let closed = Arc::new(AtomicBool::new(false));
    let runtime = Arc::new(FixtureRuntime {
        table: table.clone(),
        converter,
        store: raw_store.clone(),
        cluster: Arc::new(ResolutionStore(rows.clone())),
        closed: closed.clone(),
    });
    let mut task_meta = crate::proto::TaskMeta::default();
    task_meta.Plan.TableInfo = Some(table);
    let task = Task {
        TaskBase: TaskBase {
            ID: 42,
            Key: "import".to_owned(),
            Type: "import-into",
            State: "running",
            Step: step::ImportStepConflictResolution,
            Priority: 1,
            RequiredSlots: 2,
            TargetScope: String::new(),
            CreateTime: SystemTime::UNIX_EPOCH,
            MaxNodeCount: 1,
            ExtraParams: Default::default(),
            Keyspace: String::new(),
        },
        SchedulerID: String::new(),
        StartTime: SystemTime::UNIX_EPOCH,
        StateUpdateTime: SystemTime::UNIX_EPOCH,
        Meta: task_meta.Marshal().unwrap(),
        Error: None,
        ModifyParam: astersql_dxf_framework_proto::modify::ModifyParam {
            PrevState: "running",
            Modifications: Vec::new(),
        },
    };
    let mut executor =
        crate::task_executor::GetConflictResolutionStepExecutor(&task, runtime).unwrap();
    execute::SetFrameworkInfo(
        Some(executor.as_mut()),
        &task,
        Arc::new(StepResource {
            CPU: NewAllocatable(2),
            Mem: NewAllocatable(1024),
        }),
        None,
        None,
    );
    let mut subtask_meta = ConflictResolutionStepMeta::default();
    subtask_meta.Infos.ConflictInfos.insert(
        "data".to_owned(),
        astersql_ingestor_engineapi::ConflictInfo {
            Count: conflicts.len() as u64,
            Files: vec!["/test/conflicts.data".to_owned()],
        },
    );
    subtask_meta.Infos.ConflictInfos.insert(
        "2".to_owned(),
        astersql_ingestor_engineapi::ConflictInfo {
            Count: index_conflicts.len() as u64,
            Files: vec!["/test/index-conflicts.data".to_owned()],
        },
    );
    let mut subtask = NewSubtask(
        step::ImportStepConflictResolution,
        42,
        "import-into",
        String::new(),
        2,
        subtask_meta.Marshal().unwrap(),
        1,
    );
    let context = execute::Context::new();
    executor.Init(context.clone()).unwrap();
    executor.RunSubtask(context.clone(), &mut subtask).unwrap();
    assert_eq!(
        2,
        rows.lock()
            .unwrap()
            .keys()
            .filter(|key| astersql_tablecodec::IsRecordKey(key))
            .count()
    );
    let summary = executor.RealtimeSummary().unwrap();
    assert_eq!(9, summary.Processed.load(Ordering::SeqCst));
    assert_eq!(1, summary.Progresses.len());
    executor.Cleanup(context).unwrap();
    assert!(closed.load(Ordering::SeqCst));
    assert!(
        !raw_store
            .FileExists(&storage_context, "/test/conflicts.data")
            .unwrap()
    );

    let node_rows = Arc::new(Mutex::new(original_rows));
    let node_store: astersql_objstore::storage::StorageRef =
        Arc::new(astersql_objstore::memstore::NewMemStorage());
    node_store
        .WriteFile(
            &storage_context,
            "/test/conflicts.data",
            &encode_kvs(&conflicts),
        )
        .unwrap();
    node_store
        .WriteFile(
            &storage_context,
            "/test/index-conflicts.data",
            &encode_kvs(&index_conflicts),
        )
        .unwrap();
    let node_closed = Arc::new(AtomicBool::new(false));
    let node_runtime = Arc::new(FixtureRuntime {
        table: task_meta.Plan.TableInfo.as_ref().unwrap().clone(),
        converter: Arc::new(CanonicalImportDatumConverter(StrictContext.Flags())),
        store: node_store.clone(),
        cluster: Arc::new(ResolutionStore(node_rows.clone())),
        closed: node_closed.clone(),
    });
    let node_task = astersql_dxf_framework_taskexecutor::Task {
        TaskBase: astersql_dxf_framework_taskexecutor::TaskBase {
            ID: 42,
            Type: "ImportInto".to_owned(),
            Step: step::ImportStepConflictResolution as i64,
            RequiredSlots: 2,
            ..Default::default()
        },
        Meta: task.Meta.clone(),
    };
    struct OtherSteps(Arc<AtomicI64>);
    impl astersql_dxf_framework_taskexecutor::Extension for OtherSteps {
        fn IsIdempotent(&self, _: &astersql_dxf_framework_taskexecutor::Subtask) -> bool {
            true
        }
        fn GetStepExecutor(
            &self,
            _: &astersql_dxf_framework_taskexecutor::Task,
        ) -> astersql_dxf_framework_taskexecutor::Result<
            Arc<dyn astersql_dxf_framework_taskexecutor::StepExecutor>,
        > {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(
                astersql_dxf_framework_taskexecutor::BaseStepExecutor,
            ))
        }
        fn IsRetryableError(&self, _: &astersql_dxf_framework_taskexecutor::ExecutorError) -> bool {
            false
        }
    }
    let other_calls = Arc::new(AtomicI64::new(0));
    let extension = crate::task_executor::ImportConflictExtension {
        Runtime: node_runtime,
        OtherSteps: Arc::new(OtherSteps(other_calls.clone())),
    };
    let node_step =
        astersql_dxf_framework_taskexecutor::Extension::GetStepExecutor(&extension, &node_task)
            .unwrap();
    assert_eq!(0, other_calls.load(Ordering::SeqCst));
    let mut node_subtask = astersql_dxf_framework_taskexecutor::Subtask {
        SubtaskBase: astersql_dxf_framework_taskexecutor::SubtaskBase {
            TaskID: 42,
            Step: step::ImportStepConflictResolution as i64,
            ..Default::default()
        },
        Meta: subtask.Meta.clone(),
    };
    let node_context = astersql_dxf_framework_taskexecutor::Context::Background();
    astersql_dxf_framework_taskexecutor::StepExecutor::Init(node_step.as_ref(), &node_context)
        .unwrap();
    astersql_dxf_framework_taskexecutor::StepExecutor::RunSubtask(
        node_step.as_ref(),
        &node_context,
        &mut node_subtask,
    )
    .unwrap();
    assert_eq!(
        2,
        node_rows
            .lock()
            .unwrap()
            .keys()
            .filter(|key| astersql_tablecodec::IsRecordKey(key))
            .count()
    );
    assert_eq!(
        9,
        astersql_dxf_framework_taskexecutor::StepExecutor::RealtimeSummary(node_step.as_ref())
            .unwrap()
            .RowCount
    );
    astersql_dxf_framework_taskexecutor::StepExecutor::Cleanup(node_step.as_ref(), &node_context)
        .unwrap();
    assert!(node_closed.load(Ordering::SeqCst));
    assert!(
        !node_store
            .FileExists(&storage_context, "/test/conflicts.data")
            .unwrap()
    );
    let mut non_conflict_task = node_task;
    non_conflict_task.TaskBase.Step = step::ImportStepImport as i64;
    astersql_dxf_framework_taskexecutor::Extension::GetStepExecutor(&extension, &non_conflict_task)
        .unwrap();
    assert_eq!(1, other_calls.load(Ordering::SeqCst));
}

#[test]
fn importer_preserves_partition_and_common_handle_identity() {
    use astersql_tablecodec::{self as tc, kv};
    let partition = kv::NewPartitionHandle(42, Box::new(kv::IntHandle(1)));
    let converted = crate::conflict_resolution::importerHandleForTest(&partition).unwrap();
    assert_eq!(
        42,
        converted
            .as_any()
            .downcast_ref::<astersql_kv::PartitionHandle>()
            .unwrap()
            .PartitionID
    );
    let make = |a: &str, b: &str| {
        let mut encoded = Vec::new();
        for value in [a, b] {
            encoded.push(1); // TiDB bytes datum flag.
            encoded = tc::codec::EncodeBytes(encoded, value.as_bytes());
        }
        kv::NewCommonHandle(encoded).unwrap()
    };
    let a = make("x, y", "z");
    let b = make("x", "y, z");
    assert_eq!(a.String(), b.String());
    let a = crate::conflict_resolution::importerHandleForTest(&a).unwrap();
    let b = crate::conflict_resolution::importerHandleForTest(&b).unwrap();
    assert_ne!(a.Encoded(), b.Encoded());
    let mut set =
        astersql_dxf_importinto_conflictedkv::NewBoundedKeySet(Arc::new(AtomicI64::new(0)), 4096);
    let ka = Key(tc::EncodeRowKey(1, &a.Encoded()).0);
    let kb = Key(tc::EncodeRowKey(1, &b.Encoded()).0);
    set.Add(&ka);
    assert!(!set.Contains(&kb));
    set.Add(&kb);
    assert_eq!(2, set.Len());
}
