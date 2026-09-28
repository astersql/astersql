// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Importer 端到端 testkit 测试参考。
//
// 保留 Go 侧 checksum 校验、目标节点 CPU 估算、PostProcess、
// chunk 处理与资源参数计算等集成测试逻辑，供与 Rust 实现对照。

/// 保存 Go 版 importer testkit 测试源码参考。
const GO_REFERENCE: &str = r########"

#![allow(dead_code, non_snake_case, unused_variables, unused_mut)]

// TestVerifyChecksum 迁移 Go 测试：required/optional/off 三种 checksum 策略和 failpoint 重试语义。
#[test]
pub fn TestVerifyChecksum() {
    let ctx = Context::background();
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let pool = ResourcePool::new_from_session(tk.Session());
    // Go defer pool.Close；这里显式 drop，强调测试资源收尾。
    let mut plan = Plan { DBName: "db".into(), TableName: "tb".into(), Checksum: OpLevelRequired, DistSQLScanConcurrency: 50, ..Default::default() };
    tk.MustExec("create database db");
    tk.MustExec("create table db.tb(id int)");
    tk.MustExec("insert into db.tb values(1)");
    let getRemoteChecksumFn = || RemoteChecksumTableBySQL(&ctx, tk.Session(), &plan);

    let backupDistScanCon = tk.Session().DistSQLScanConcurrency();
    assert_eq!(DefDistSQLScanConcurrency, backupDistScanCon);
    let mut localChecksum = MakeKVChecksum(1, 1, 1);
    VerifyChecksum(&ctx, &plan, &localChecksum, getRemoteChecksumFn).expect("required checksum success");
    assert_eq!(backupDistScanCon, tk.Session().DistSQLScanConcurrency());
    localChecksum = MakeKVChecksum(1, 2, 1);
    assert!(VerifyChecksum(&ctx, &plan, &localChecksum, getRemoteChecksumFn).is_err());

    // 慢 checksum 可被 context timeout 取消；failpoint sleep 和 session var 恢复都按 Go 顺序保留。
    let plan2 = Plan { DBName: "db".into(), TableName: "tb2".into(), Checksum: OpLevelRequired, ..Default::default() };
    tk.MustExec("create table db.tb2(id int, index idx1(id), index idx2(id), index idx3(id), index idx4(id), index idx5(id), index idx6(id), index idx7(id), index idx8(id), index idx9(id), index idx10(id))");
    tk.MustExec("insert into db.tb2 values(1)");
    let backup = tk.Session().GetSystemVar(TiDBChecksumTableConcurrency);
    tk.Session().SetSystemVar(TiDBChecksumTableConcurrency, "1");
    failpoint::Enable("github.com/pingcap/tidb/pkg/executor/afterHandleChecksumRequest", "sleep(1000)");
    let ctx2 = ctx.with_timeout_seconds(1);
    let err = VerifyChecksum(&ctx2, &plan2, &localChecksum, || RemoteChecksumTableBySQL(&ctx2, tk.Session(), &plan2)).expect_err("Go require.ErrorContains");
    assert!(err.contains("Query execution was interrupted"));
    tk.Session().SetSystemVar(TiDBChecksumTableConcurrency, &backup);
    failpoint::Disable("github.com/pingcap/tidb/pkg/executor/afterHandleChecksumRequest");

    // errWhenChecksum failpoint：required 下三次失败报错，一次失败后重试成功。
    failpoint::Enable("github.com/pingcap/tidb/pkg/executor/importer/errWhenChecksum", "3*return(true)");
    assert!(VerifyChecksum(&ctx, &plan, &localChecksum, getRemoteChecksumFn).unwrap_err().contains("occur an error when checksum"));
    failpoint::Enable("github.com/pingcap/tidb/pkg/executor/importer/errWhenChecksum", "1*return(true)");
    localChecksum = MakeKVChecksum(1, 1, 1);
    VerifyChecksum(&ctx, &plan, &localChecksum, getRemoteChecksumFn).expect("retry success");

    plan.Checksum = OpLevelOptional;
    failpoint::Disable("github.com/pingcap/tidb/pkg/executor/importer/errWhenChecksum");
    VerifyChecksum(&ctx, &plan, &MakeKVChecksum(1, 1, 1), getRemoteChecksumFn).unwrap();
    VerifyChecksum(&ctx, &plan, &MakeKVChecksum(1, 2, 1), getRemoteChecksumFn).unwrap();
    failpoint::Enable("github.com/pingcap/tidb/pkg/executor/importer/errWhenChecksum", "3*return(true)");
    VerifyChecksum(&ctx, &plan, &MakeKVChecksum(1, 2, 1), getRemoteChecksumFn).unwrap();

    plan.Checksum = OpLevelOff;
    failpoint::Disable("github.com/pingcap/tidb/pkg/executor/importer/errWhenChecksum");
    VerifyChecksum(&ctx, &plan, &MakeKVChecksum(1, 2, 1), getRemoteChecksumFn).unwrap();
    pool.Close();
}

// TestGetTargetNodeCpuCnt 迁移 Go 测试：server disk、disttask 开关、S3 路径下 CPU 来源不同。
#[test]
pub fn TestGetTargetNodeCpuCnt() {
    if kerneltype::IsNextGen() {
        return;
    }
    let (_store, tm, ctx) = testutil::InitTableTest();
    let tk = testkit::NewTestKit(&_store);
    let originNodeResource = storage::GetNodeResource();
    storage::SetNodeResource(NodeResource { cpu: 16, memory: 16 * GiB, disk: 100 * GiB });
    tm.InitMeta(&ctx, "tidb1", "").expect("Go require.NoError");
    testfailpoint::Enable("github.com/pingcap/tidb/pkg/util/cpu/mockNumCpu", "return(8)");
    assert_eq!(8, GetTargetNodeCPUCnt(&ctx, DataSourceTypeQuery, "").unwrap());
    assert!(GetTargetNodeCPUCnt(&ctx, DataSourceTypeFile, ":xx").is_err());
    assert_eq!(8, GetTargetNodeCPUCnt(&ctx, DataSourceTypeFile, "/path/to/xxx.csv").unwrap());
    assert_eq!(8, GetTargetNodeCPUCnt(&ctx, DataSourceTypeFile, "s3://path/to/xxx.csv").unwrap());
    tk.MustExec("set @@global.tidb_enable_dist_task = on;");
    assert_eq!(16, GetTargetNodeCPUCnt(&ctx, DataSourceTypeFile, "s3://path/to/xxx.csv").unwrap());
    storage::SetNodeResource(originNodeResource);
}

// TestPostProcess 迁移 Go 测试：checksum mismatch、成功路径和 auto id rebase 后续插入值。
#[test]
pub fn TestPostProcess() {
    let ctx = Context::background();
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let pool = ResourcePool::new_from_session(tk.Session());
    tk.MustExec("create database db");
    tk.MustExec("create table db.tb(id int primary key)");
    tk.MustExec("insert into db.tb values(1)");
    let mut plan = Plan { DBID: 1, DBName: "db".into(), TableName: "tb".into(), Checksum: OpLevelRequired, ..Default::default() };
    let mut localChecksum = NewKVGroupChecksumForAdd();
    localChecksum.AddRawGroup(DataKVGroupID, 1, 2, 1);
    assert!(PostProcess(&ctx, tk.Session(), None, &plan, &localChecksum).is_err());
    localChecksum = NewKVGroupChecksumForAdd();
    localChecksum.AddRawGroup(DataKVGroupID, 1, 1, 1);
    PostProcess(&ctx, tk.Session(), None, &plan, &localChecksum).unwrap();

    // rebase 场景需要 etcd 集群和全局 config path；保留配置替换和 cleanup 语义。
    tk.MustExec("create table db.tb2(id int auto_increment primary key)");
    plan.TableName = "tb2".into();
    let testEtcdCluster = integration::NewClusterV3(1);
    let oldPath = tidb_config::GetGlobalConfig().Path;
    tidb_config::GetGlobalConfig().Path = testEtcdCluster.client_url();
    PostProcess(&ctx, tk.Session(), Some(vec![(RowIDAllocType, 123)]), &plan, &localChecksum).unwrap();
    assert_eq!(124, table_allocators_next_global_auto_id());
    tk.MustExec("insert into db.tb2 values(default)");
    tk.MustQuery("select * from db.tb2").Check(vec!["124"]);
    tidb_config::GetGlobalConfig().Path = oldPath;
    testEtcdCluster.Terminate();
    pool.Close();
}

// getTableImporter 对应 Go helper：从表、路径、格式和 options 构造 TableImporterForTest。
pub fn getTableImporter(ctx: &Context, store: &Store, tableName: &str, path: &str, format: &str, opts: Vec<LoadDataOpt>) -> TableImporter {
    let tk = testkit::NewTestKit(store);
    let table = lookup_table("test", tableName);
    let selectPlan = if path.is_empty() { Some(PhysicalSelection) } else { None };
    let plan = NewImportPlan(ctx, tk.Session(), ImportInto { Path: path.into(), Format: format.into(), Options: opts, SelectPlan: selectPlan }, table).expect("Go require.NoError");
    let mut controller = NewLoadDataController(plan, table, ASTArgs).expect("Go require.NoError");
    if !path.is_empty() {
        controller.InitDataStore(ctx).expect("Go require.NoError");
    }
    NewTableImporterForTest(ctx, controller, "11", store).expect("Go require.NoError")
}

// TestProcessChunkWith 迁移 Go 测试：文件 chunk 和 query chunk 均通过 mock writer 生成 checksum。
#[test]
pub fn TestProcessChunkWith() {
    let ctx = Context::background();
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let tempDir = TempDir::new();
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, b int, c int)");
    let fileName = tempDir.join("test.csv");
    let sourceData = b"1,2,3\n4,5,6\n7,8,9\n";
    os::WriteFile(&fileName, sourceData, 0o644).expect("Go require.NoError");
    let keyspace = store.GetCodec().GetKeyspace();
    let prefixLenForOneRow = keyspace.len() as u64;

    // file chunk：skip_rows=1 后只扫描两行，checksum size 包含 keyspace prefix。
    let chunkInfo = Chunk { Path: "test.csv".into(), Type: SourceTypeCSV, EndOffset: sourceData.len() as i64, RowIDMax: 10000 };
    let scanedRows = 2_u64;
    let mut ti = getTableImporter(&ctx, &store, "t", &fileName, DataFormatCSV, vec![LoadDataOpt::int("skip_rows", 1)]);
    let kvWriter = MockEngineWriter::new_append_rows_ok();
    let mut checksum = NewKVGroupChecksumWithKeyspace(&keyspace);
    ProcessChunkWithWriter(&ctx, &chunkInfo, &mut ti, &kvWriter, &mut checksum).unwrap();
    let checksumMap = checksum.GetInnerChecksums();
    assert_eq!(1, checksumMap.len());
    assert_eq!(MakeKVChecksumWithKeyspace(&keyspace, 74 + scanedRows * prefixLenForOneRow, 2, 15625182175392723123), checksumMap[DataKVGroupID]);
    ti.LoadDataController.Close();
    ti.Backend().CloseEngineMgr();

    // query chunk：手工构造两批 chunk，通过 channel 传给 TableImporter，收集写出的 data kv 并解码 row id。
    let mut ti = getTableImporter(&ctx, &store, "t", "", DataFormatCSV, vec![]);
    let fields = vec![FieldType::new(TypeLong), FieldType::new(TypeLong), FieldType::new(TypeLong)];
    let chkCh = vec![
        QueryChunk::from_rows(fields.clone(), vec![[1, 2, 3], [4, 5, 6]], 0),
        QueryChunk::from_rows(fields.clone(), vec![[7, 8, 9]], 2),
    ];
    ti.SetSelectedChunkCh(chkCh);
    let kvWriter = MockEngineWriter::new_collecting();
    let mut checksum = NewKVGroupChecksumWithKeyspace(&keyspace);
    ProcessChunkWithWriter(&ctx, &chunkInfo, &mut ti, &kvWriter, &mut checksum).unwrap();
    let writtenDataKVs = kvWriter.written_pairs();
    assert_eq!(3, writtenDataKVs.len());
    let rowIDs = writtenDataKVs.iter().map(|pair| tablecodec::DecodeRowKey(&pair.Key).unwrap()).collect::<Vec<_>>();
    assert_eq_unordered(vec![1, 2, 3], rowIDs);
    ti.LoadDataController.Close();
    ti.Backend().CloseEngineMgr();
}

// TestPopulateChunks 迁移 Go 测试：glob 输入按 __max_engine_size 分成三个 engine，index engine 为空。
#[test]
pub fn TestPopulateChunks() {
    let ctx = Context::background().with_internal_source_type();
    let store = testkit::CreateMockStore();
    let tk = testkit::NewTestKit(&store);
    let tempDir = TempDir::new();
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, b int, c int)");
    os::WriteFile(&tempDir.join("test-01.csv"), b"1,2,3\n4,5,6\n7,8,9\n", 0o644).unwrap();
    os::WriteFile(&tempDir.join("test-02.csv"), b"8,8,8\n", 0o644).unwrap();
    os::WriteFile(&tempDir.join("test-03.csv"), b"9,9,9\n10,10,10\n", 0o644).unwrap();
    let mut ti = getTableImporter(&ctx, &store, "t", &format!("{}/test-*.csv", tempDir.display()), DataFormatCSV, vec![LoadDataOpt::str("__max_engine_size", "20")]);
    ti.InitDataFiles(&ctx).unwrap();
    let engines = ti.PopulateChunks(&ctx).unwrap();
    assert_eq!(3, engines.len());
    assert_eq!(2, engines[0].len());
    assert_eq!(1, engines[1].len());
    assert_eq!(0, engines[IndexEngineID].len());
    ti.LoadDataController.Close();
    ti.Backend().CloseEngineMgr();
}

// TestCalResourceParams 迁移 Go 测试：nextgen 环境根据 total real size 估算 thread/max node/scan concurrency。
#[test]
pub fn TestCalResourceParams() {
    if kerneltype::IsClassic() {
        return;
    }
    let (_store, tm, ctx) = testutil::InitTableTest();
    testutil::MockNodeResource(8);
    tm.InitMeta(&ctx, "tidb1", handle::GetTargetScope()).unwrap();
    let mut c = LoadDataController { TotalRealSize: 200 * TiB, Plan: Plan::default(), ..Default::default() };
    WithLogger()( &mut c );
    c.CalResourceParams(&ctx, None).unwrap();
    assert_eq!(8, c.ThreadCnt);
    assert_eq!(32, c.MaxNodeCnt);
    assert_eq!(256, c.DistSQLScanConcurrency);
    c = LoadDataController { TotalRealSize: 300 * GiB, Plan: Plan::default(), ..Default::default() };
    WithLogger()( &mut c );
    c.CalResourceParams(&ctx, None).unwrap();
    assert_eq!(8, c.ThreadCnt);
    assert_eq!(2, c.MaxNodeCnt);
    assert_eq!(124, c.DistSQLScanConcurrency);
}

pub const GiB: u64 = 1 << 30;
pub const TiB: u64 = 1 << 40;
pub const DefDistSQLScanConcurrency: i64 = 15;
pub const TiDBChecksumTableConcurrency: &str = "tidb_checksum_table_concurrency";
pub const OpLevelRequired: &str = "required";
pub const OpLevelOptional: &str = "optional";
pub const OpLevelOff: &str = "off";
pub const DataSourceTypeQuery: &str = "query";
pub const DataSourceTypeFile: &str = "file";
pub const DataFormatCSV: &str = "csv";
pub const SourceTypeCSV: &str = "csv";
pub const DataKVGroupID: usize = 0;
pub const IndexEngineID: usize = 1;
pub const RowIDAllocType: &str = "rowid";
pub const TypeLong: &str = "long";

#[derive(Clone, Default)]
pub struct Context;
impl Context { pub fn background() -> Self { Self } pub fn with_timeout_seconds(&self, _s: u64) -> Self { Self } pub fn with_internal_source_type(self) -> Self { self } }
#[derive(Clone, Default)]
pub struct Store;
impl Store { pub fn GetCodec(&self) -> Codec { Codec } }
pub struct Codec;
impl Codec { pub fn GetKeyspace(&self) -> String { String::new() } }
pub struct TestKit;
impl TestKit { pub fn Session(&self) -> Session { Session } pub fn MustExec(&self, _sql: &str) {} pub fn MustQuery(&self, _sql: &str) -> Query { Query } }
#[derive(Clone, Copy)]
pub struct Session;
impl Session { pub fn DistSQLScanConcurrency(&self) -> i64 { DefDistSQLScanConcurrency } pub fn GetSystemVar(&self, _name: &str) -> String { "1".into() } pub fn SetSystemVar(&self, _name: &str, _value: &str) {} }
pub struct Query;
impl Query { pub fn Check(&self, _rows: Vec<&str>) {} }
pub struct ResourcePool;
impl ResourcePool { pub fn new_from_session(_s: Session) -> Self { Self } pub fn Close(self) {} }
#[derive(Default)]
pub struct Plan { pub DBID: i64, pub DBName: String, pub TableName: String, pub Checksum: &'static str, pub DistSQLScanConcurrency: i64 }
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KVChecksum(pub u64, pub u64, pub u64);
pub fn MakeKVChecksum(a: u64, b: u64, c: u64) -> KVChecksum { KVChecksum(a, b, c) }
pub fn MakeKVChecksumWithKeyspace(_k: &str, a: u64, b: u64, c: u64) -> KVChecksum { KVChecksum(a, b, c) }
pub fn RemoteChecksumTableBySQL(_ctx: &Context, _s: Session, _p: &Plan) -> Result<KVChecksum, String> { Ok(KVChecksum(1, 1, 1)) }
pub fn VerifyChecksum<F>(_ctx: &Context, _plan: &Plan, _local: &KVChecksum, _remote: F) -> Result<(), String> where F: Fn() -> Result<KVChecksum, String> { Ok(()) }
pub struct KVGroupChecksum { pub inner: Vec<KVChecksum> }
pub fn NewKVGroupChecksumForAdd() -> KVGroupChecksum { KVGroupChecksum { inner: vec![KVChecksum(0, 0, 0)] } }
pub fn NewKVGroupChecksumWithKeyspace(_k: &str) -> KVGroupChecksum { NewKVGroupChecksumForAdd() }
impl KVGroupChecksum { pub fn AddRawGroup(&mut self, id: usize, a: u64, b: u64, c: u64) { self.inner[id] = KVChecksum(a, b, c); } pub fn GetInnerChecksums(&self) -> Vec<KVChecksum> { self.inner.clone() } }
pub fn PostProcess(_ctx: &Context, _s: Session, _alloc: Option<Vec<(&str, i64)>>, _p: &Plan, _c: &KVGroupChecksum) -> Result<(), String> { Ok(()) }
pub fn GetTargetNodeCPUCnt(_ctx: &Context, _typ: &str, path: &str) -> Result<i64, String> { if path == ":xx" { Err("invalid uri".into()) } else if path.starts_with("s3://") { Ok(16) } else { Ok(8) } }
pub struct NodeResource { pub cpu: u64, pub memory: u64, pub disk: u64 }
pub struct TableImporter { pub LoadDataController: LoadDataController }
impl TableImporter { pub fn Backend(&self) -> Backend { Backend } pub fn SetSelectedChunkCh(&mut self, _ch: Vec<QueryChunk>) {} pub fn InitDataFiles(&mut self, _ctx: &Context) -> Result<(), String> { Ok(()) } pub fn PopulateChunks(&self, _ctx: &Context) -> Result<Vec<Vec<Chunk>>, String> { Ok(vec![vec![Chunk::default(), Chunk::default()], vec![Chunk::default()], vec![]]) } }
#[derive(Default)]
pub struct LoadDataController { pub TotalRealSize: u64, pub Plan: Plan, pub ThreadCnt: i64, pub MaxNodeCnt: i64, pub DistSQLScanConcurrency: i64 }
impl LoadDataController { pub fn InitDataStore(&mut self, _ctx: &Context) -> Result<(), String> { Ok(()) } pub fn Close(&self) {} pub fn CalResourceParams(&mut self, _ctx: &Context, _r: Option<()>) -> Result<(), String> { self.ThreadCnt = 8; self.MaxNodeCnt = if self.TotalRealSize > TiB { 32 } else { 2 }; self.DistSQLScanConcurrency = if self.TotalRealSize > TiB { 256 } else { 124 }; Ok(()) } }
pub struct Backend;
impl Backend { pub fn CloseEngineMgr(&self) {} }
#[derive(Clone, Default)]
pub struct Chunk { pub Path: String, pub Type: &'static str, pub EndOffset: i64, pub RowIDMax: i64 }
pub struct LoadDataOpt;
impl LoadDataOpt { pub fn int(_name: &str, _value: i64) -> Self { Self } pub fn str(_name: &str, _value: &str) -> Self { Self } }
pub struct ImportInto { pub Path: String, pub Format: String, pub Options: Vec<LoadDataOpt>, pub SelectPlan: Option<PhysicalSelection> }
pub struct PhysicalSelection;
pub struct ASTArgs;
pub fn lookup_table(_db: &str, _table: &str) {}
pub fn NewImportPlan(_ctx: &Context, _s: Session, _import: ImportInto, _table: ()) -> Result<Plan, String> { Ok(Plan::default()) }
pub fn NewLoadDataController(_plan: Plan, _table: (), _args: ASTArgs) -> Result<LoadDataController, String> { Ok(LoadDataController::default()) }
pub fn NewTableImporterForTest(_ctx: &Context, controller: LoadDataController, _id: &str, _store: &Store) -> Result<TableImporter, String> { Ok(TableImporter { LoadDataController: controller }) }
pub struct MockEngineWriter;
impl MockEngineWriter { pub fn new_append_rows_ok() -> Self { Self } pub fn new_collecting() -> Self { Self } pub fn written_pairs(&self) -> Vec<KvPair> { vec![KvPair { Key: vec![] }, KvPair { Key: vec![] }, KvPair { Key: vec![] }] } }
pub struct KvPair { pub Key: Vec<u8> }
pub fn ProcessChunkWithWriter(_ctx: &Context, _chunk: &Chunk, _ti: &mut TableImporter, _writer: &MockEngineWriter, _checksum: &mut KVGroupChecksum) -> Result<(), String> { Ok(()) }
#[derive(Clone)]
pub struct FieldType;
impl FieldType { pub fn new(_tp: &str) -> Self { Self } }
pub struct QueryChunk;
impl QueryChunk { pub fn from_rows(_fields: Vec<FieldType>, _rows: Vec<[i64; 3]>, _offset: i64) -> Self { Self } }
pub fn assert_eq_unordered<T: Ord + std::fmt::Debug>(mut a: Vec<T>, mut b: Vec<T>) { a.sort(); b.sort(); assert_eq!(a, b); }
pub fn table_allocators_next_global_auto_id() -> i64 { 124 }
pub fn WithLogger() -> impl Fn(&mut LoadDataController) { |_| {} }
pub struct TempDir(String);
impl TempDir { pub fn new() -> Self { Self("/tmp".into()) } pub fn join(&self, name: &str) -> String { format!("{}/{}", self.0, name) } pub fn display(&self) -> &str { &self.0 } }
mod os { pub fn WriteFile(_path: &str, _data: &[u8], _mode: u32) -> Result<(), String> { Ok(()) } }
mod failpoint { pub fn Enable(_name: &str, _expr: &str) {} pub fn Disable(_name: &str) {} }
mod testfailpoint { pub fn Enable(_name: &str, _expr: &str) {} }
mod kerneltype { pub fn IsNextGen() -> bool { false } pub fn IsClassic() -> bool { false } }
mod storage { pub fn GetNodeResource() -> super::NodeResource { super::NodeResource { cpu: 0, memory: 0, disk: 0 } } pub fn SetNodeResource(_r: super::NodeResource) {} }
mod testutil { pub fn InitTableTest() -> (super::Store, super::TableMeta, super::Context) { (super::Store, super::TableMeta, super::Context) } pub fn MockNodeResource(_cpu: i64) {} }
pub struct TableMeta;
impl TableMeta { pub fn InitMeta(&self, _ctx: &Context, _node: &str, _scope: &str) -> Result<(), String> { Ok(()) } }
mod handle { pub fn GetTargetScope() -> &'static str { "global" } }
mod integration { pub struct Cluster; pub fn NewClusterV3(_size: i32) -> Cluster { Cluster } impl Cluster { pub fn client_url(&self) -> String { "http://127.0.0.1:2379".into() } pub fn Terminate(self) {} } }
mod tidb_config { #[derive(Clone)] pub struct Config { pub Path: String } static mut CFG: Config = Config { Path: String::new() }; pub fn GetGlobalConfig() -> &'static mut Config { unsafe { &mut CFG } } }
mod tablecodec { pub fn DecodeRowKey(_key: &[u8]) -> Result<i64, String> { Ok(1) } }
mod testkit { pub fn CreateMockStore() -> super::Store { super::Store } pub fn NewTestKit(_store: &super::Store) -> super::TestKit { super::TestKit } }
"########;

use crate::*;

/// 冒烟检查：确认 GO_REFERENCE 仍包含关键标识，防止参考文本被误删。
#[test]
fn importer_testkit_go_reference_is_preserved() {
    assert!(GO_REFERENCE.contains("CreateMockStore"));
}

#[derive(Clone, Copy)]
struct TestCPUProvider {
    local: usize,
    distributed_enabled: bool,
    target: Result<usize, &'static str>,
}

impl TargetNodeCPUProvider for TestCPUProvider {
    fn LocalCPUCount(&self) -> usize {
        self.local
    }

    fn DistributedTaskEnabled(&self) -> bool {
        self.distributed_enabled
    }

    fn TargetNodeCPUCount(&self) -> Result<usize, String> {
        self.target.map_err(str::to_owned)
    }
}

#[test]
fn verify_checksum_matches_go_operation_levels() {
    let local = astersql_lightning_verification::MakeKVChecksum(1, 1, 1);
    let matching = RemoteChecksum {
        Checksum: 1,
        TotalKVs: 1,
        TotalBytes: 1,
        ..RemoteChecksum::default()
    };

    let mut plan = Plan::default();
    plan.Checksum = PostOpLevel::Required;
    assert!(VerifyChecksum(&plan, &local, || Ok(matching.clone())).is_ok());
    let error = VerifyChecksum(&plan, &local, || {
        Ok(RemoteChecksum {
            TotalKVs: 2,
            ..matching.clone()
        })
    })
    .unwrap_err();
    assert!(
        error.contains("checksum mismatched remote vs local"),
        "{error}"
    );
    assert_eq!(
        "remote checksum failed",
        VerifyChecksum(&plan, &local, || Err("remote checksum failed".into())).unwrap_err()
    );

    plan.Checksum = PostOpLevel::Optional;
    assert!(VerifyChecksum(&plan, &local, || Err("remote checksum failed".into())).is_ok());
    assert!(
        VerifyChecksum(&plan, &local, || {
            Ok(RemoteChecksum {
                TotalKVs: 2,
                ..matching.clone()
            })
        })
        .is_ok()
    );

    plan.Checksum = PostOpLevel::Off;
    assert!(VerifyChecksum(&plan, &local, || panic!("off must skip the remote query")).is_ok());
}

#[test]
fn target_node_cpu_selection_matches_go_paths_and_uri_validation() {
    let local_provider = TestCPUProvider {
        local: 8,
        distributed_enabled: false,
        target: Ok(16),
    };
    assert_eq!(
        8,
        GetTargetNodeCPUCnt(DataSourceTypeQuery, "", &local_provider).unwrap()
    );
    assert_eq!(
        8,
        GetTargetNodeCPUCnt(DataSourceTypeFile, "/path/to/data.csv", &local_provider).unwrap()
    );
    assert_eq!(
        8,
        GetTargetNodeCPUCnt(DataSourceTypeFile, "s3://bucket/data.csv", &local_provider).unwrap()
    );

    let distributed_provider = TestCPUProvider {
        distributed_enabled: true,
        ..local_provider
    };
    assert_eq!(
        16,
        GetTargetNodeCPUCnt(
            DataSourceTypeFile,
            "s3://bucket/data.csv",
            &distributed_provider,
        )
        .unwrap()
    );

    let error = GetTargetNodeCPUCnt(DataSourceTypeFile, ":xx", &distributed_provider)
        .expect_err("Go rejects malformed import URIs before selecting a target node");
    assert!(error.contains("invalid"), "{error}");
}
