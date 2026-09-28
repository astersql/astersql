// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// DDL 集成测试的 Go→Rust 迁移记录骨架。
//
// 以 `Step`/`RecordedCase` 保留原 Go 测试的可观察语义顺序（SQL、failpoint、断言等），
// 覆盖 backfill（回填）、partial index（部分索引）、AffectColumn 偏移维护，以及 next-gen
// 下 JobVersion / Global Index V1 支持等场景，并在测试运行时校验这些步骤与 Go 源文件保持同序。

#![allow(dead_code, non_snake_case)]

// Step 对应 Go 测试中的一条可观察语义。
/// Go 测试中一条可观察语义步骤的枚举标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    // 保留 Go 源注释，尤其是问题编号、边界条件和英文说明。
    Comment(&'static str),
    // Exec step from source test.
    Sql(&'static str),
    // 对应 MustQuery/Rows/Check，保留查询和结果检查位置。
    Query(&'static str),
    // 对应 MustGetErrCode/MustGetErrMsg/DBError/ErrorContains。
    ExpectedError(&'static str),
    // 对应 failpoint/testfailpoint 注入；闭包副作用在相邻步骤中保留。
    Failpoint(&'static str),
    // 对应 require/assert 断言，保留 Go 断言条件和参数形状。
    Assertion(&'static str),
    // 对应 defer/Cleanup/Close/Cancel/Stop 等资源收尾。
    Cleanup(&'static str),
    // 对应 goroutine、WaitGroup、Mutex、Once、atomic 等并发或异步行为。
    Concurrency(&'static str),
    // 对应 if/for/switch/t.Run 等控制流，保留分支和循环边界。
    ControlFlow(&'static str),
    // 对应结构体、测试用例表、map/slice fixture 和 mock 对象。
    Fixture(&'static str),
    // 其余 Go 语句暂以源码形状保留，避免凭空推测依赖 API。
    Source(&'static str),
}

/// 一条已记录的 Go 测试用例：名称与步骤序列。
struct RecordedCase {
    /// 原 Go 测试/方法名。
    go_name: &'static str,
    /// 按执行顺序排列的语义步骤。
    steps: &'static [Step],
}

const GO_SOURCE: &str = include_str!("integration_test.go");

impl Step {
    fn source(self) -> &'static str {
        match self {
            Self::Comment(source)
            | Self::Sql(source)
            | Self::Query(source)
            | Self::ExpectedError(source)
            | Self::Failpoint(source)
            | Self::Assertion(source)
            | Self::Cleanup(source)
            | Self::Concurrency(source)
            | Self::ControlFlow(source)
            | Self::Fixture(source)
            | Self::Source(source) => source,
        }
    }
}

/// 校验记录用例的每一步均存在于对应 Go 函数中，且顺序完全一致。
fn record_case(case: RecordedCase) -> RecordedCase {
    let first_step = case
        .steps
        .first()
        .unwrap_or_else(|| panic!("Go case {} has no recorded steps", case.go_name))
        .source();
    let function_offset = GO_SOURCE
        .find(first_step)
        .unwrap_or_else(|| panic!("missing Go function {}", case.go_name));
    let mut remaining = &GO_SOURCE[function_offset..];
    for step in case.steps {
        let source = step.source();
        let offset = remaining.find(source).unwrap_or_else(|| {
            panic!(
                "Go step is missing or out of order in {}: {source}",
                case.go_name
            )
        });
        remaining = &remaining[offset + source.len()..];
    }
    case
}

#[test]
fn mock_etcd_backend_contract_matches_go_source() {
    let mut remaining = GO_SOURCE;
    for step in TOP_LEVEL_ITEMS {
        let source = step.source();
        let offset = remaining
            .find(source)
            .unwrap_or_else(|| panic!("missing top-level Go declaration: {source}"));
        remaining = &remaining[offset + source.len()..];
    }
    etcd_addrs();
    get_pd_addrs();
    tls_config();
    start_gc_worker();
}

// TOP_LEVEL_ITEMS 保留 Go 文件中测试函数之外的类型、常量或辅助声明。
// 这些声明通常是 mock、fixture 或包级变量；不推测真实 Rust 类型。
const TOP_LEVEL_ITEMS: &[Step] = &[
    Step::Fixture(r#"type mockEtcdBackend struct {"#),
    Step::Source(r#"	kv.Storage"#),
    Step::Fixture(r#"	pdAddrs []string"#),
    Step::ControlFlow(r#"}"#),
];

/// mock etcd 后端返回 PD/etcd 地址列表。
fn etcd_addrs() {
    // EtcdAddrs 对应 Go 的 `func (mebd *mockEtcdBackend) EtcdAddrs() ([]string, error) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"EtcdAddrs"#,
        steps: &[
            Step::Fixture(r#"func (mebd *mockEtcdBackend) EtcdAddrs() ([]string, error) {"#),
            Step::ControlFlow(r#"	return mebd.pdAddrs, nil"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// mock etcd 后端返回 PD（Placement Driver，集群调度组件）地址。
fn get_pd_addrs() {
    // GetPDAddrs 对应 Go 的 `func (mebd *mockEtcdBackend) GetPDAddrs() ([]string, error) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"GetPDAddrs"#,
        steps: &[
            Step::Fixture(r#"func (mebd *mockEtcdBackend) GetPDAddrs() ([]string, error) {"#),
            Step::ControlFlow(r#"	return mebd.pdAddrs, nil"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// mock etcd 后端 TLS 配置（测试中返回 nil）。
fn tls_config() {
    // TLSConfig 对应 Go 的 `func (mebd *mockEtcdBackend) TLSConfig() *tls.Config { return nil }`。
    let _case = record_case(RecordedCase {
        go_name: r#"TLSConfig"#,
        steps: &[Step::Source(
            r#"func (mebd *mockEtcdBackend) TLSConfig() *tls.Config { return nil }"#,
        )],
    });
}

/// mock 启动 GC Worker（测试中空实现）。
fn start_gc_worker() {
    // StartGCWorker 对应 Go 的 `func (mebd *mockEtcdBackend) StartGCWorker() error { return nil }`。
    let _case = record_case(RecordedCase {
        go_name: r#"StartGCWorker"#,
        steps: &[Step::Source(
            r#"func (mebd *mockEtcdBackend) StartGCWorker() error { return nil }"#,
        )],
    });
}

/// 验证各类 DDL 是否进入 reorg（重组/回填）阶段：改列类型、加索引、主键等。
#[test]
fn test_ddl_statements_back_fill() {
    // TestDDLStatementsBackFill 对应 Go 的 `func TestDDLStatementsBackFill(t *testing.T) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"TestDDLStatementsBackFill"#,
        steps: &[
            Step::Source(r#"func TestDDLStatementsBackFill(t *testing.T) {"#),
            Step::Source(r#"	store := testkit.CreateMockStore(t)"#),
            Step::Source(r#"	tk := testkit.NewTestKit(t, store)"#),
            Step::Sql(r#"	tk.MustExec("use test;")"#),
            Step::Source(r#"	needReorg := false"#),
            Step::Failpoint(
                r#"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {"#,
            ),
            Step::ControlFlow(r#"		if job.SchemaState == model.StateWriteReorganization {"#),
            Step::Source(r#"			needReorg = true"#),
            Step::ControlFlow(r#"		}"#),
            Step::ControlFlow(r#"	})"#),
            Step::Sql(r#"	tk.MustExec("create table t (a int, b char(65));")"#),
            Step::Sql(r#"	tk.MustExec("insert into t values (1, '123');")"#),
            Step::Fixture(r#"	testCases := []struct {"#),
            Step::Source(r#"		ddlSQL            string"#),
            Step::Source(r#"		expectedNeedReorg bool"#),
            Step::Source(r#"	}{"#),
            Step::Source(r#"		{"alter table t modify column a bigint;", false},"#),
            Step::Source(r#"		{"alter table t modify column b char(255);", false},"#),
            Step::Source(r#"		{"alter table t modify column a varchar(100);", true},"#),
            Step::Source(r#"		{"create table t1 (a int, b int);", false},"#),
            Step::Source(r#"		{"alter table t1 add index idx_a(a);", true},"#),
            Step::Source(r#"		{"alter table t1 add primary key(b) nonclustered;", true},"#),
            Step::Source(r#"		{"alter table t1 drop primary key;", false},"#),
            Step::ControlFlow(r#"	}"#),
            Step::ControlFlow(r#"	for _, tc := range testCases {"#),
            Step::Source(r#"		needReorg = false"#),
            Step::Sql(r#"		tk.MustExec(tc.ddlSQL)"#),
            Step::Assertion(r#"		require.Equal(t, tc.expectedNeedReorg, needReorg, tc)"#),
            Step::ControlFlow(r#"	}"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// 部分索引（partial index，带 WHERE 谓词）的建表/改表类型校验与限制。
#[test]
fn test_partial_index() {
    // TestPartialIndex 对应 Go 的 `func TestPartialIndex(t *testing.T) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"TestPartialIndex"#,
        steps: &[
            Step::Source(r#"func TestPartialIndex(t *testing.T) {"#),
            Step::Source(r#"	store := testkit.CreateMockStore(t)"#),
            Step::Source(r#"	tk := testkit.NewTestKit(t, store)"#),
            Step::Sql(r#"	tk.MustExec("use test;")"#),
            Step::Comment(r#"	// test validate column exists in create table"#),
            Step::Sql(r#"	tk.MustExec("create table t (a int, b int, key(b) where a = 1);")"#),
            Step::ExpectedError(
                r#"	tk.MustGetDBError("create table t1 (a int, b int, key(b) where c = 1);","#,
            ),
            Step::Source(r#"		dbterror.ErrUnsupportedAddPartialIndex)"#),
            Step::Sql(r#"	tk.MustExec("drop table t;")"#),
            Step::Comment(r#"	// test primary key is not allowed in partial index"#),
            Step::Sql(r#"	tk.MustExec("create table t (a int, b int, key(b) where a = 1);")"#),
            Step::ExpectedError(
                r#"	tk.MustGetDBError("create table t2 (a int, b int, primary key(b) where a = 1);","#,
            ),
            Step::Source(r#"		dbterror.ErrUnsupportedAddPartialIndex)"#),
            Step::Sql(r#"	tk.MustExec("drop table t;")"#),
            Step::Fixture(
                r#"	checkColumnTypes := func(columnTypes []string, literals []string, shouldAllowed bool) {"#,
            ),
            Step::ControlFlow(r#"		for _, columnType := range columnTypes {"#),
            Step::ControlFlow(r#"			for _, literal := range literals {"#),
            Step::Sql(r#"				tk.MustExec("drop table if exists t;")"#),
            Step::Source(
                r#"				sql := fmt.Sprintf("create table t (a %s, b int, key(b) where a = %s);", columnType, literal)"#,
            ),
            Step::ControlFlow(r#"				if shouldAllowed {"#),
            Step::Sql(r#"					tk.MustExec(sql)"#),
            Step::Sql(r#"					tk.MustExec("drop table t;")"#),
            Step::ControlFlow(r#"				} else {"#),
            Step::ExpectedError(
                r#"					tk.MustGetDBError(sql, dbterror.ErrUnsupportedAddPartialIndex)"#,
            ),
            Step::ControlFlow(r#"				}"#),
            Step::ControlFlow(r#"			}"#),
            Step::ControlFlow(r#"		}"#),
            Step::ControlFlow(r#"	}"#),
            Step::Comment(r#"	// test create table type validation"#),
            Step::Fixture(r#"	differentTypeLiterals := [][]string{"#),
            Step::Source(r#"		{"1", "true", "1998"}, // int"#),
            Step::Source(r#"		{"'1'"},               // string with default collate"#),
            Step::Source(r#"		{"1.0"},               // float"#),
            Step::Source(r#"		{"b'101010'", "0x1234567890abcdef", "0b10"}, // binary literal"#),
            Step::Source(r#"		{"null"}, // null"#),
            Step::ControlFlow(r#"	}"#),
            Step::Fixture(r#"	differentColumnTypes := [][]string{"#),
            Step::Source(r#"		{"int", "bigint", "tinyint", "smallint", "year"},"#),
            Step::Source(
                r#"		{"char(25)", "varchar(123)", "text", "char(25) collate utf8mb4_general_ci", "char(25) collate utf8mb4_bin"},"#,
            ),
            Step::Source(r#"		{"float", "double"},"#),
            Step::Source(
                r#"		{"binary(25) collate binary", "varbinary(123)", "blob", "char(25) collate binary"},"#,
            ),
            Step::Source(r#"		{},"#),
            Step::ControlFlow(r#"	}"#),
            Step::ControlFlow(r#"	for i, columnTypes := range differentColumnTypes {"#),
            Step::ControlFlow(r#"		for j, literals := range differentTypeLiterals {"#),
            Step::Source(r#"			checkColumnTypes(columnTypes, literals, i == j)"#),
            Step::ControlFlow(r#"		}"#),
            Step::ControlFlow(r#"	}"#),
            Step::Comment(
                r#"	// test comparing between time column and string constant is allowed."#,
            ),
            Step::Fixture(
                r#"	timeColumnTypes := []string{"timestamp", "datetime", "date", "time"}"#,
            ),
            Step::Fixture(
                r#"	allowedLiterals := []string{"'2025-07-28 12:34:56'", "'2025-07-28'", "'12:34:56'"}"#,
            ),
            Step::Fixture(r#"	notAllowedLiterals := []string{"1", "1.0", "true", "null"}"#),
            Step::Source(r#"	checkColumnTypes(timeColumnTypes, allowedLiterals, true)"#),
            Step::Source(r#"	checkColumnTypes(timeColumnTypes, notAllowedLiterals, false)"#),
            Step::Comment(
                r#"	// test comparing between enum/set column and int/string constant is allowed."#,
            ),
            Step::Fixture(
                r#"	enumSetColumnTypes := []string{"enum('a', 'b', 'c')", "set('a', 'b', 'c')"}"#,
            ),
            Step::Fixture(r#"	allowedLiterals = []string{"1", "'1'", "'a'"}"#),
            Step::Fixture(r#"	notAllowedLiterals = []string{"1.0", "null"}"#),
            Step::Source(r#"	checkColumnTypes(enumSetColumnTypes, allowedLiterals, true)"#),
            Step::Source(r#"	checkColumnTypes(enumSetColumnTypes, notAllowedLiterals, false)"#),
            Step::Comment(r#"	// test alter table type validation"#),
            Step::ControlFlow(r#"	for i, literals := range differentTypeLiterals {"#),
            Step::ControlFlow(r#"		for _, literal := range literals {"#),
            Step::ControlFlow(r#"			for j, columnTypes := range differentColumnTypes {"#),
            Step::Sql(r#"				tk.MustExec("drop table if exists t;")"#),
            Step::ControlFlow(r#"				for _, columnType := range columnTypes {"#),
            Step::Source(
                r#"					sql := fmt.Sprintf("create table t (a %s, b int, key idx_b(b) where a = %s);", columnType, literal)"#,
            ),
            Step::ControlFlow(r#"					if i == j {"#),
            Step::Sql(r#"						tk.MustExec(sql)"#),
            Step::Sql(r#"						tk.MustExec("drop table t;")"#),
            Step::ControlFlow(r#"					} else {"#),
            Step::ExpectedError(
                r#"						tk.MustGetDBError(sql, dbterror.ErrUnsupportedAddPartialIndex)"#,
            ),
            Step::ControlFlow(r#"					}"#),
            Step::ControlFlow(r#"				}"#),
            Step::ControlFlow(r#"			}"#),
            Step::ControlFlow(r#"		}"#),
            Step::ControlFlow(r#"	}"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// 删表前开启 CheckTableBeforeDrop，并覆盖 FastCheckTable 会话变量开关。
#[test]
fn test_drop_table_admin_check_table_fast_check_table() {
    // TestDropTableAdminCheckTableFastCheckTable 对应 Go 的 `func TestDropTableAdminCheckTableFastCheckTable(t *testing.T) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"TestDropTableAdminCheckTableFastCheckTable"#,
        steps: &[
            Step::Source(r#"func TestDropTableAdminCheckTableFastCheckTable(t *testing.T) {"#),
            Step::Source(r#"	store := testkit.CreateMockStore(t)"#),
            Step::Source(r#"	tk := testkit.NewTestKit(t, store)"#),
            Step::Sql(r#"	tk.MustExec("use test;")"#),
            Step::Sql(r#"	tk.MustExec("drop table if exists t;")"#),
            Step::Sql(r#"	tk.MustExec("create table t (a int, b int, key(b) where a = 1);")"#),
            Step::Source(r#"	dom := domain.GetDomain(tk.Session())"#),
            Step::Assertion(r#"	require.NotNil(t, dom)"#),
            Step::Source(r#"	pool := dom.SysSessionPool()"#),
            Step::Source(r#"	seOn, err := pool.Get()"#),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Source(r#"	seOff, err := pool.Get()"#),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Source(r#"	seOffCtx := seOff.(sessionctx.Context)"#),
            Step::Assertion(
                r#"	require.NoError(t, seOffCtx.GetSessionVars().SetSystemVar(vardef.TiDBFastCheckTable, vardef.Off))"#,
            ),
            Step::Source(r#"	pool.Put(seOn)"#),
            Step::Source(r#"	pool.Put(seOff)"#),
            Step::Source(r#"	oldCheckTableBeforeDrop := config.CheckTableBeforeDrop"#),
            Step::Source(r#"	config.CheckTableBeforeDrop = true"#),
            Step::Cleanup(r#"	defer func() {"#),
            Step::Source(r#"		config.CheckTableBeforeDrop = oldCheckTableBeforeDrop"#),
            Step::Source(r#"	}()"#),
            Step::Sql(r#"	tk.MustExec("drop table t;")"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// 增删列后维护索引 AffectColumn 的列偏移（Offset），保证部分索引依赖列定位正确。
#[test]
fn test_maintain_affect_columns() {
    // TestMaintainAffectColumns 对应 Go 的 `func TestMaintainAffectColumns(t *testing.T) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"TestMaintainAffectColumns"#,
        steps: &[
            Step::Source(r#"func TestMaintainAffectColumns(t *testing.T) {"#),
            Step::Source(r#"	store, dom := testkit.CreateMockStoreAndDomain(t)"#),
            Step::Source(r#"	tk := testkit.NewTestKit(t, store)"#),
            Step::Sql(r#"	tk.MustExec("use test;")"#),
            Step::Sql(r#"	tk.MustExec("create table t (col2 int, key(col2) where col2 > 0);")"#),
            Step::Comment(r#"	// Now, the offset of col2 is 0"#),
            Step::Source(
                r#"	tbl, err := dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))"#,
            ),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Assertion(
                r#"	require.Equal(t, 0, tbl.Meta().Indices[0].AffectColumn[0].Offset)"#,
            ),
            Step::Sql(r#"	tk.MustExec("alter table t add column col1 int first;")"#),
            Step::Comment(r#"	// Now, the offset of col2 should be 1"#),
            Step::Source(
                r#"	tbl, err = dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))"#,
            ),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Assertion(
                r#"	require.Equal(t, 1, tbl.Meta().Indices[0].AffectColumn[0].Offset)"#,
            ),
            Step::Sql(r#"	tk.MustExec("alter table t add column col3 int after col1;")"#),
            Step::Comment(r#"	// Now, the offset of col2 should be 2"#),
            Step::Source(
                r#"	tbl, err = dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))"#,
            ),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Assertion(
                r#"	require.Equal(t, 2, tbl.Meta().Indices[0].AffectColumn[0].Offset)"#,
            ),
            Step::Sql(r#"	tk.MustExec("alter table t drop column col1;")"#),
            Step::Comment(r#"	// Now, the offset of col2 should be 1"#),
            Step::Source(
                r#"	tbl, err = dom.InfoSchema().TableByName(context.Background(), ast.NewCIStr("test"), ast.NewCIStr("t"))"#,
            ),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Assertion(
                r#"	require.Equal(t, 1, tbl.Meta().Indices[0].AffectColumn[0].Offset)"#,
            ),
            Step::ControlFlow(r#"}"#),
        ],
    });
}

/// next-gen 集群下确认 JobVersion=2 且始终支持 Global Index V1；需隔离 ingest 全局环境。
#[test]
fn test_job_version_and_global_index_v1_support_for_next_gen() {
    // TestJobVersionAndGlobalIndexV1SupportForNextGen 对应 Go 的 `func TestJobVersionAndGlobalIndexV1SupportForNextGen(t *testing.T) {`。
    let _case = record_case(RecordedCase {
        go_name: r#"TestJobVersionAndGlobalIndexV1SupportForNextGen"#,
        steps: &[
            Step::Source(r#"func TestJobVersionAndGlobalIndexV1SupportForNextGen(t *testing.T) {"#),
            Step::ControlFlow(r#"	if !kerneltype.IsNextGen() {"#),
            Step::Source(r#"		t.Skip("nextgen only")"#),
            Step::ControlFlow(r#"	}"#),
            Step::Source(r#"	integration.BeforeTestExternal(t)"#),
            Step::Comment(
                r#"	// This test temporarily sets `global config.Store=TiKV` to initialize DDL in a"#,
            ),
            Step::Comment(
                r#"	// next-gen-like mode. It must not leak ingest global env state to other UTs"#,
            ),
            Step::Comment(
                r#"	// in the same test binary (for example, tests that run with the default"#,
            ),
            Step::Comment(r#"	// unistore config)."#),
            Step::Source(r#"	origLitInitialized := ingest.LitInitialized"#),
            Step::Source(r#"	origLitMemRoot := ingest.LitMemRoot"#),
            Step::Source(r#"	origLitDiskRoot := ingest.LitDiskRoot"#),
            Step::Cleanup(r#"	t.Cleanup(func() {"#),
            Step::Source(r#"		ingest.LitInitialized = origLitInitialized"#),
            Step::Source(r#"		ingest.LitMemRoot = origLitMemRoot"#),
            Step::Source(r#"		ingest.LitDiskRoot = origLitDiskRoot"#),
            Step::ControlFlow(r#"	})"#),
            Step::Source(r#"	originJobVer := model.GetJobVerInUse()"#),
            Step::Source(r#"	originGlobalIdxV1 := model.GetGlobalIndexV1Supported()"#),
            Step::Cleanup(r#"	t.Cleanup(func() {"#),
            Step::Source(r#"		model.SetJobVerInUse(originJobVer)"#),
            Step::Source(r#"		model.SetGlobalIndexV1Supported(originGlobalIdxV1)"#),
            Step::ControlFlow(r#"	})"#),
            Step::Assertion(r#"	require.Equal(t, model.JobVersion2, model.GetJobVerInUse())"#),
            Step::Assertion(r#"	require.True(t, model.GetGlobalIndexV1Supported())"#),
            Step::Fixture(r#"	serverInfos := map[string]*serverinfo.ServerInfo{"#),
            Step::Source(r#"		"node0": {"#),
            Step::Source(r#"			StaticInfo: serverinfo.StaticInfo{"#),
            Step::Source(
                r#"				VersionInfo: serverinfo.VersionInfo{Version: "8.0.11-TiDB-CLOUD.202510.1"},"#,
            ),
            Step::ControlFlow(r#"			},"#),
            Step::ControlFlow(r#"		},"#),
            Step::ControlFlow(r#"	}"#),
            Step::Source(r#"	bytes, err := json.Marshal(serverInfos)"#),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Failpoint(r#"	testfailpoint.Enable(t,"#),
            Step::Source(
                r#"		"github.com/pingcap/tidb/pkg/domain/serverinfo/mockGetAllServerInfo","#,
            ),
            Step::Source(r#"		fmt.Sprintf("return(`%s`)", string(bytes)),"#),
            Step::Source(r#"	)"#),
            Step::Source(
                r#"	cluster := integration.NewClusterV3(t, &integration.ClusterConfig{Size: 1})"#,
            ),
            Step::Cleanup(r#"	defer cluster.Terminate(t)"#),
            Step::Source(
                r#"	store, dom := testkit.CreateMockStoreAndDomainWithSchemaLease(t, testLease)"#,
            ),
            Step::Source(r#"	mockStore := &mockEtcdBackend{"#),
            Step::Source(r#"		Storage: store,"#),
            Step::Fixture(r#"		pdAddrs: []string{cluster.Members[0].GRPCURL()},"#),
            Step::ControlFlow(r#"	}"#),
            Step::Source(r#"	storeTypeBak := config.GetGlobalConfig().Store"#),
            Step::Source(r#"	config.GetGlobalConfig().Store = config.StoreTypeTiKV"#),
            Step::Cleanup(r#"	t.Cleanup(func() {"#),
            Step::Source(r#"		config.GetGlobalConfig().Store = storeTypeBak"#),
            Step::Source(r#"		ddl.CloseOwnerManager(mockStore)"#),
            Step::ControlFlow(r#"	})"#),
            Step::Assertion(
                r#"	require.NoError(t, ddl.StartOwnerManager(context.Background(), mockStore))"#,
            ),
            Step::Source(r#"	newDDL, _ := ddl.NewDDL(context.Background(),"#),
            Step::Source(r#"		ddl.WithStore(mockStore),"#),
            Step::Source(r#"		ddl.WithInfoCache(dom.InfoCache()),"#),
            Step::Source(r#"		ddl.WithLease(testLease),"#),
            Step::Source(r#"		ddl.WithSchemaLoader(dom),"#),
            Step::Source(r#"		ddl.WithEtcdClient(cluster.RandClient()),"#),
            Step::Source(r#"	)"#),
            Step::Source(
                r#"	err = newDDL.Start(ddl.Normal, pools.NewResourcePool(func() (pools.Resource, error) {"#,
            ),
            Step::Source(r#"		session := testkit.NewTestKit(t, mockStore).Session()"#),
            Step::Source(r#"		session.GetSessionVars().CommonGlobalLoaded = true"#),
            Step::ControlFlow(r#"		return session, nil"#),
            Step::Source(r#"	}, 1, 1, time.Second))"#),
            Step::Assertion(r#"	require.NoError(t, err)"#),
            Step::Assertion(r#"	require.NoError(t, newDDL.Stop())"#),
            Step::Comment(
                r#"	// The only meaningful assert in this test. It makes sure that the JobVersion is 2"#,
            ),
            Step::Comment(
                r#"	// and the global index v1 is always supported for next-gen cluster."#,
            ),
            Step::Assertion(r#"	require.Equal(t, model.JobVersion2, model.GetJobVerInUse())"#),
            Step::Assertion(r#"	require.True(t, model.GetGlobalIndexV1Supported())"#),
            Step::ControlFlow(r#"}"#),
        ],
    });
}
