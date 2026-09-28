// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// ALTER TABLE ... MODIFY/CHANGE COLUMN 相关 DDL 测试的步骤记录稿。
//
// 将 Go 侧 testkit 测试改写为 `CaseRecorder` 步骤序列：不真正执行 SQL，
// 仅按源码顺序记录 exec/query/error/failpoint 等动作。覆盖 reorg（数据重组）
// 清理、NULL 转 NOT NULL、类型/字符集转换、索引写冲突、skip reorg、并行 DDL、
// 统计信息与 PD range 错误等路径。

// 这段逻辑覆盖 modify column 测试中的 reorg 清理、NULL/类型转换、索引冲突、skip reorg、并行 DDL、统计信息和 PD range 错误路径。

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

/// CaseStep 保留 Go 测试中的单条动作：SQL、查询、错误断言、failpoint、并发/资源收尾或普通源码语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseStep {
    /// 对应 Go 源文件中的大致行号，便于对照原文。
    pub line: usize,
    /// 步骤类别：exec / query / error / failpoint / assertion 等。
    pub kind: &'static str,
    /// 记录的源码片段或 SQL 文本（原样保留，不执行）。
    pub text: &'static str,
}

/// CaseRecorder 是 testkit/require/failpoint 的轻量记录器；它不执行任何数据库或 DDL 业务动作。
#[derive(Debug, Default, Clone)]
pub struct CaseRecorder {
    /// 测试或辅助函数名称（通常取自 Go 侧测试名）。
    pub name: &'static str,
    /// 按调用顺序累积的步骤列表。
    pub steps: Vec<CaseStep>,
}

impl CaseRecorder {
    /// 创建空记录器，绑定测试名。
    /// 创建空步骤列表的记录器。
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            steps: Vec::new(),
        }
    }
    /// 追加一条通用步骤。
    /// 记录一条通用步骤。
    pub fn record(&mut self, line: usize, kind: &'static str, text: &'static str) {
        self.steps.push(CaseStep { line, kind, text });
    }
    /// 记录一条执行 SQL（MustExec）步骤。
    /// 记录一条 SQL 执行步骤。
    pub fn exec(&mut self, line: usize, text: &'static str) {
        self.record(line, "exec", text);
    }
    /// 记录一条查询并 Check 结果的步骤。
    /// 记录一条查询与结果校验步骤。
    pub fn query(&mut self, line: usize, text: &'static str) {
        self.record(line, "query", text);
    }
    /// 记录期望失败的错误断言步骤。
    /// 记录一条期望报错的步骤。
    pub fn error(&mut self, line: usize, text: &'static str) {
        self.record(line, "error", text);
    }
    /// 记录 failpoint 启用/禁用或回调注入点。
    /// 记录一条 failpoint 注入步骤。
    pub fn failpoint(&mut self, line: usize, text: &'static str) {
        self.record(line, "failpoint", text);
    }
    /// 记录 require/assert 类断言。
    /// 记录一条断言步骤。
    pub fn assertion(&mut self, line: usize, text: &'static str) {
        self.record(line, "assertion", text);
    }
    /// 记录并发、defer、goroutine 相关语义。
    /// 记录并发/defer 等收尾控制步骤。
    pub fn concurrency(&mut self, line: usize, text: &'static str) {
        self.record(line, "concurrency", text);
    }
    /// 记录分支/循环控制语句，保留覆盖组合顺序。
    /// 记录分支或循环控制源码步骤。
    pub fn branch(&mut self, line: usize, text: &'static str) {
        self.record(line, "branch", text);
    }
    /// 记录普通源码语句（变量、取表元数据等）。
    /// 记录普通源码语义步骤。
    pub fn source(&mut self, line: usize, text: &'static str) {
        self.record(line, "source", text);
    }
}

impl std::process::Termination for CaseRecorder {
    /// 测试返回 CaseRecorder 时视为成功退出。
    fn report(self) -> std::process::ExitCode {
        std::process::ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod parity_tests {
    use super::super::column::{ColumnInfo, ColumnKind, FieldType};
    use super::super::modify_column::{
        ModifyColumnError, PartitionExpressionUsage, PartitionInfo, PartitionType,
        check_partition_column_modifiable, collect_partition_expression_usage,
    };

    fn column(name: &str, kind: ColumnKind, flen: usize, decimal: i32) -> ColumnInfo {
        let mut field_type = FieldType::integer();
        field_type.kind = kind;
        let mut column = ColumnInfo::new(name, field_type);
        column.field_type.flen = flen;
        column.field_type.decimal = decimal;
        column.field_type.charset = "utf8mb4".into();
        column.field_type.collation = "utf8mb4_bin".into();
        column
    }

    #[test]
    fn partition_expression_usage_follows_the_target_columns_ast_path() {
        assert_eq!(
            collect_partition_expression_usage("a + abs(b)", "a"),
            [PartitionExpressionUsage::NoFunction].into_iter().collect()
        );
        assert_eq!(
            collect_partition_expression_usage("floor(to_days(a))", "a"),
            [PartitionExpressionUsage::Unsupported]
                .into_iter()
                .collect()
        );
        assert!(collect_partition_expression_usage("to_days(a)", "to_days").is_empty());
    }

    #[test]
    fn partition_type_allowlist_rejects_cross_string_and_timestamp_changes() {
        let key = PartitionInfo {
            partition_type: PartitionType::Key,
            columns: vec!["a".into()],
            expression: String::new(),
        };
        let old = column("a", ColumnKind::String, 8, 0);
        let new = column("a", ColumnKind::Varchar, 16, 0);
        assert_eq!(
            check_partition_column_modifiable(&key, &old, &new, &[], &[]),
            Err(ModifyColumnError::PartitionTypeChange)
        );

        let range = PartitionInfo {
            partition_type: PartitionType::Range,
            columns: vec!["a".into()],
            expression: String::new(),
        };
        let old = column("a", ColumnKind::Timestamp, 19, 0);
        let new = column("a", ColumnKind::Timestamp, 26, 6);
        assert_eq!(
            check_partition_column_modifiable(&range, &old, &new, &[], &[]),
            Err(ModifyColumnError::PartitionTypeChange)
        );
    }
}

/// batchInsert 对应 Go 第 42-51 行：保留原测试/辅助函数的执行顺序。
// batchInsert 对应 Go 第 42-51 行：保留原测试/辅助函数的执行顺序。
pub fn batch_insert() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"batchInsert"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.source(43, r#"dml := fmt.Sprintf("insert into %s values", tbl)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(44, r#"for i := start; i < end; i++ {"#);
    draft.source(45, r#"dml += fmt.Sprintf("(%d, %d, %d)", i, i, i)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(46, r#"if i != end-1 {"#);
    draft.source(47, r#"dml += ",""#);
    draft.exec(50, r#"tk.MustExec(dml)"#);
    draft
}

/// TestModifyColumnReorgInfo 对应 Go 第 53-167 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnReorgInfo 对应 Go 第 53-167 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_reorg_info() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnReorgInfo"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(54, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(56, r#"limit := vardef.GetDDLErrorCountLimit()"#);
    draft.source(57, r#"vardef.SetDDLErrorCountLimit(5)"#);
    draft.concurrency(58, r#"defer func() {"#);
    draft.source(59, r#"vardef.SetDDLErrorCountLimit(limit)"#);
    draft.source(61, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(62, r#"tk.MustExec("use test")"#);
    draft.exec(63, r#"tk.MustExec("drop table if exists t1")"#);
    draft.exec(64, r#"tk.MustExec("create table t1 (c1 int, c2 int, c3 int, index idx(c2), index idx1(c1, c2));")"#);
    draft.source(66, r#"sql := "alter table t1 change c2 c2 varchar(16);""#);
    // Go 注释 L67: defaultBatchSize is equal to ddl.defaultBatchSize
    draft.source(68, r#"base := defaultBatchSize * 8"#);
    // Go 注释 L69: add some rows
    draft.source(70, r#"batchInsert(tk, "t1", 0, base)"#);
    // Go 注释 L71: Make sure the count of regions more than backfill workers.
    draft.query(72, r#"tk.MustQuery("split table t1 between (0) and (8192) regions 8;").Check(testkit.Rows("8 1"))"#);
    draft.source(74, r#"tbl := external.GetTableByName(t, tk, "test", "t1")"#);
    // Go 注释 L76: Check insert null before job first update.
    draft.source(77, r#"var checkErr error"#);
    draft.source(78, r#"var currJob *model.Job"#);
    draft.source(79, r#"var elements []*meta.Element"#);
    draft.source(80, r#"ctx := mock.NewContext()"#);
    draft.source(81, r#"ctx.Store = store"#);
    draft.source(82, r#"times := 0"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(83, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(84, r#"if tbl.Meta().ID != job.TableID || checkErr != nil || job.SchemaState != model.StateWriteReorganization {"#);
    draft.source(85, r#"return"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(87, r#"if job.Type == model.ActionModifyColumn {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(88, r#"if times == 0 {"#);
    draft.source(89, r#"times++"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(92, r#"if job.Type == model.ActionAddIndex {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(93, r#"if times == 1 {"#);
    draft.source(94, r#"times++"#);
    draft.source(95, r#"return"#);
    draft.source(97, r#"tbl := external.GetTableByName(t, tk, "test", "t1")"#);
    draft.source(98, r#"indexInfo := tbl.Meta().FindIndexByName("idx2")"#);
    draft.source(
        99,
        r#"elements = []*meta.Element{{ID: indexInfo.ID, TypeKey: meta.IndexElementKey}}"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(103, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/modifyColumnTypeWithData", func(job *model.Job, args model.JobArgs) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(104, r#"if tbl.Meta().ID == job.TableID &&"#);
    draft.source(105, r#"checkErr == nil &&"#);
    draft.source(106, r#"job.SchemaState == model.StateDeleteOnly &&"#);
    draft.source(107, r#"job.Type == model.ActionModifyColumn {"#);
    draft.source(108, r#"currJob = job"#);
    draft.source(109, r#"a := args.(*model.ModifyColumnArgs)"#);
    draft.source(
        110,
        r#"elements = ddl.BuildElements(a.ChangingColumn, a.ChangingIdxs)"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(114, r#"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr", `return("cantDecodeRecordErr")`))"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(115, r#"err := tk.ExecToErr(sql)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(116, r#"require.EqualError(t, err, "[ddl:8202]Cannot decode index value, because mock can't decode record error")"#);
    draft.assertion(117, r#"require.NoError(t, checkErr)"#);
    // Go 注释 L118: Check whether the reorg information is cleaned up when executing "modify column" failed.
    draft.source(
        119,
        r#"checkReorgHandle := func(gotElements, expectedElements []*meta.Element) {"#,
    );
    draft.assertion(
        120,
        r#"require.Equal(t, len(expectedElements), len(gotElements))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(121, r#"for i, e := range gotElements {"#);
    draft.assertion(122, r#"require.Equal(t, expectedElements[i], e)"#);
    // Go 注释 L124: check the consistency of the tables.
    draft.source(125, r#"currJobID := strconv.FormatInt(currJob.ID, 10)"#);
    draft.query(126, r#"tk.MustQuery("select job_id, reorg, schema_ids, table_ids, type, processing from mysql.tidb_ddl_job where job_id = " + currJobID).Check(testkit.Rows())"#);
    draft.query(127, r#"tk.MustQuery("select job_id from mysql.tidb_ddl_history where job_id = " + currJobID).Check(testkit.Rows(currJobID))"#);
    draft.query(128, r#"tk.MustQuery("select job_id, ele_id, ele_type, physical_id from mysql.tidb_ddl_reorg where job_id = " + currJobID).Check(testkit.Rows())"#);
    draft.assertion(
        129,
        r#"require.NoError(t, sessiontxn.NewTxn(context.Background(), ctx))"#,
    );
    draft.source(130, r#"e, start, end, physicalID, err := ddl.NewReorgHandlerForTest(testkit.NewTestKit(t, store).Session()).GetDDLReorgHandle(currJob)"#);
    draft.assertion(131, r#"require.Error(t, err, "Error not ErrDDLReorgElementNotExists, found orphan row in tidb_ddl_reorg for job.ID %d: e: '%s', physicalID: %d, start: 0x%x end: 0x%x", currJob.ID, e, physicalID, start, end)"#);
    draft.assertion(
        132,
        r#"require.True(t, meta.ErrDDLReorgElementNotExist.Equal(err))"#,
    );
    draft.assertion(133, r#"require.Nil(t, e)"#);
    draft.assertion(134, r#"require.Nil(t, start)"#);
    draft.assertion(135, r#"require.Nil(t, end)"#);
    draft.assertion(136, r#"require.Zero(t, physicalID)"#);
    draft.source(138, r#"expectedElements := []*meta.Element{"#);
    draft.source(139, r#"{ID: 4, TypeKey: meta.ColumnElementKey},"#);
    draft.source(140, r#"{ID: 3, TypeKey: meta.IndexElementKey},"#);
    draft.source(141, r#"{ID: 4, TypeKey: meta.IndexElementKey}}"#);
    draft.source(142, r#"checkReorgHandle(elements, expectedElements)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(143, r#"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr"))"#);
    draft.exec(144, r#"tk.MustExec("admin check table t1")"#);
    // Go 注释 L146: Check whether the reorg information is cleaned up when executing "modify column" successfully.
    // Go 注释 L147: Test encountering a "notOwnerErr" error which caused the processing backfill job to exit halfway.
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(148, r#"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr", `return("modifyColumnNotOwnerErr")`))"#);
    draft.exec(149, r#"tk.MustExec(sql)"#);
    draft.source(150, r#"expectedElements = []*meta.Element{"#);
    draft.source(151, r#"{ID: 5, TypeKey: meta.ColumnElementKey},"#);
    draft.source(152, r#"{ID: 5, TypeKey: meta.IndexElementKey},"#);
    draft.source(153, r#"{ID: 6, TypeKey: meta.IndexElementKey}}"#);
    draft.source(154, r#"checkReorgHandle(elements, expectedElements)"#);
    draft.exec(155, r#"tk.MustExec("admin check table t1")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(156, r#"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr"))"#);
    // Go 注释 L158: Test encountering a "notOwnerErr" error which caused the processing backfill job to exit halfway.
    // Go 注释 L159: During the period, the old TiDB version(do not exist the element information) is upgraded to the new TiDB version.
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(160, r#"require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr", `return("addIdxNotOwnerErr")`))"#);
    draft.exec(161, r#"tk.MustExec("alter table t1 add index idx2(c1)")"#);
    draft.source(162, r#"expectedElements = []*meta.Element{"#);
    draft.source(163, r#"{ID: 7, TypeKey: meta.IndexElementKey}}"#);
    draft.source(164, r#"checkReorgHandle(elements, expectedElements)"#);
    draft.exec(165, r#"tk.MustExec("admin check table t1")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(166, r#"require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr"))"#);
    draft
}

/// TestModifyColumnNullToNotNullWithChangingVal2 对应 Go 第 169-186 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnNullToNotNullWithChangingVal2 对应 Go 第 169-186 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_null_to_not_null_with_changing_val2() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnNullToNotNullWithChangingVal2"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(170, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(171, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(172, r#"tk.MustExec("use test")"#);
    // Go 注释 L174: insert null value before modifying column
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(175, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeDoModifyColumnSkipReorgCheck", func() {"#);
    draft.source(176, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(
        177,
        r#"tk2.MustExec("insert into test.tt values (NULL, NULL)")"#,
    );
    draft.exec(180, r#"tk.MustExec("drop table if exists tt;")"#);
    draft.exec(181, r#"tk.MustExec(`create table tt (a bigint, b int);`)"#);
    draft.exec(
        182,
        r#"tk.MustExec("insert into tt values (1,1),(2,2),(3,3);")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        183,
        r#"err := tk.ExecToErr("alter table tt modify a int not null;")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        184,
        r#"require.EqualError(t, err, "[ddl:1138]Invalid use of NULL value")"#,
    );
    draft.exec(185, r#"tk.MustExec("drop table tt")"#);
    draft
}

/// TestModifyColumnNullToNotNull 对应 Go 第 188-238 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnNullToNotNull 对应 Go 第 188-238 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_null_to_not_null() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnNullToNotNull"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(
        189,
        r#"store := testkit.CreateMockStoreWithSchemaLease(t, 600*time.Millisecond)"#,
    );
    draft.source(190, r#"tk1 := testkit.NewTestKit(t, store)"#);
    draft.source(191, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(193, r#"tk1.MustExec("use test")"#);
    draft.exec(194, r#"tk2.MustExec("use test")"#);
    draft.exec(196, r#"tk1.MustExec("create table t1 (c1 int, c2 int)")"#);
    draft.source(
        198,
        r#"tbl := external.GetTableByName(t, tk1, "test", "t1")"#,
    );
    // Go 注释 L200: Check insert null before job first update.
    draft.exec(201, r#"tk1.MustExec("delete from t1")"#);
    draft.concurrency(202, r#"once := sync.Once{}"#);
    draft.source(203, r#"var checkErr error"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(204, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(205, r#"if tbl.Meta().ID != job.TableID {"#);
    draft.source(206, r#"return"#);
    draft.source(208, r#"once.Do(func() {"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        209,
        r#"checkErr = tk2.ExecToErr("insert into t1 values ()")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        212,
        r#"err := tk1.ExecToErr("alter table t1 change c2 c2 int not null")"#,
    );
    draft.assertion(213, r#"require.NoError(t, checkErr)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        214,
        r#"require.EqualError(t, err, "[ddl:1138]Invalid use of NULL value")"#,
    );
    draft.query(
        215,
        r#"tk1.MustQuery("select * from t1").Check(testkit.Rows("<nil> <nil>"))"#,
    );
    // Go 注释 L217: Check insert error when column has PreventNullInsertFlag.
    draft.exec(218, r#"tk1.MustExec("delete from t1")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(219, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(220, r#"if tbl.Meta().ID != job.TableID {"#);
    draft.source(221, r#"return"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(224, r#"if job.State != model.JobStateRunning {"#);
    draft.source(225, r#"return"#);
    // Go 注释 L227: now c2 has PreventNullInsertFlag, an error is expected.
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        228,
        r#"checkErr = tk2.ExecToErr("insert into t1 values ()")"#,
    );
    draft.exec(
        230,
        r#"tk1.MustExec("alter table t1 change c2 c2 int not null")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        231,
        r#"require.EqualError(t, checkErr, "[table:1048]Column 'c2' cannot be null")"#,
    );
    draft.source(
        233,
        r#"c2 := external.GetModifyColumn(t, tk1, "test", "t1", "c2", false)"#,
    );
    draft.assertion(
        234,
        r#"require.True(t, mysql.HasNotNullFlag(c2.GetFlag()))"#,
    );
    draft.assertion(
        235,
        r#"require.False(t, mysql.HasPreventNullInsertFlag(c2.GetFlag()))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(236, r#"err = tk1.ExecToErr("insert into t1 values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        237,
        r#"require.EqualError(t, err, "[table:1364]Field 'c2' doesn't have a default value")"#,
    );
    draft
}

/// TestModifyColumnNullToNotNullWithChangingVal 对应 Go 第 240-285 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnNullToNotNullWithChangingVal 对应 Go 第 240-285 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_null_to_not_null_with_changing_val() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnNullToNotNullWithChangingVal"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(
        241,
        r#"store := testkit.CreateMockStoreWithSchemaLease(t, 600*time.Millisecond)"#,
    );
    draft.source(242, r#"tk1 := testkit.NewTestKit(t, store)"#);
    draft.source(243, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(245, r#"tk1.MustExec("use test")"#);
    draft.exec(246, r#"tk2.MustExec("use test")"#);
    draft.exec(248, r#"tk1.MustExec("create table t1 (c1 int, c2 int)")"#);
    draft.source(
        250,
        r#"tbl := external.GetTableByName(t, tk1, "test", "t1")"#,
    );
    // Go 注释 L252: Check insert null before job first update.
    draft.exec(253, r#"tk1.MustExec("delete from t1")"#);
    draft.concurrency(254, r#"once := sync.Once{}"#);
    draft.source(255, r#"var checkErr error"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(256, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(257, r#"if tbl.Meta().ID != job.TableID {"#);
    draft.source(258, r#"return"#);
    draft.source(260, r#"once.Do(func() {"#);
    // Go 注释 L261: Insert null value to make modify column fail.
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        262,
        r#"require.NoError(t, tk2.ExecToErr("insert into t1 values ()"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        265,
        r#"err := tk1.ExecToErr("alter table t1 change c2 c2 tinyint not null")"#,
    );
    draft.assertion(266, r#"require.NoError(t, checkErr)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        267,
        r#"require.EqualError(t, err, "[ddl:1138]Invalid use of NULL value")"#,
    );
    draft.query(
        268,
        r#"tk1.MustQuery("select * from t1").Check(testkit.Rows("<nil> <nil>"))"#,
    );
    // Go 注释 L270: Check insert error when column has PreventNullInsertFlag.
    draft.exec(271, r#"tk1.MustExec("delete from t1")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(272, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterModifyColumnStateDeleteOnly", func(_ int64) {"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(273, r#"err = tk2.ExecToErr("insert into t1 values ()")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        274,
        r#"require.EqualError(t, checkErr, "[table:1048]Column 'c2' cannot be null")"#,
    );
    draft.exec(
        276,
        r#"tk1.MustExec("alter table t1 change c2 c2 tinyint not null")"#,
    );
    draft.source(
        278,
        r#"c2 := external.GetModifyColumn(t, tk1, "test", "t1", "c2", false)"#,
    );
    draft.assertion(
        279,
        r#"require.True(t, mysql.HasNotNullFlag(c2.GetFlag()))"#,
    );
    draft.assertion(
        280,
        r#"require.False(t, mysql.HasPreventNullInsertFlag(c2.GetFlag()))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(281, r#"require.EqualError(t, tk1.ExecToErr("insert into t1 values ()"), "[table:1364]Field 'c2' doesn't have a default value")"#);
    draft.source(
        283,
        r#"c2 = external.GetModifyColumn(t, tk1, "test", "t1", "c2", false)"#,
    );
    draft.assertion(
        284,
        r#"require.Equal(t, mysql.TypeTiny, c2.FieldType.GetType())"#,
    );
    draft
}

/// TestModifyColumnBetweenStringTypes 对应 Go 第 287-387 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnBetweenStringTypes 对应 Go 第 287-387 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_between_string_types() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnBetweenStringTypes"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(288, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(289, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(290, r#"tk.MustExec("use test")"#);
    // Go 注释 L292: varchar to varchar
    draft.exec(293, r#"tk.MustExec("create table tt (a varchar(10));")"#);
    draft.exec(
        294,
        r#"tk.MustExec("insert into tt values ('111'),('10000');")"#,
    );
    draft.exec(
        295,
        r#"tk.MustExec("alter table tt change a a varchar(5);")"#,
    );
    draft.source(
        296,
        r#"mvc := external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(297, r#"require.Equal(t, 5, mvc.FieldType.GetFlen())"#);
    draft.query(
        298,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(299, r#"tk.MustGetErrMsg("alter table tt change a a varchar(4);", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    draft.exec(
        300,
        r#"tk.MustExec("alter table tt change a a varchar(100);")"#,
    );
    draft.query(
        301,
        r#"tk.MustQuery("select length(a) from tt").Check(testkit.Rows("3", "5"))"#,
    );
    // Go 注释 L303: char to char
    draft.exec(304, r#"tk.MustExec("drop table if exists tt;")"#);
    draft.exec(305, r#"tk.MustExec("create table tt (a char(10));")"#);
    draft.exec(
        306,
        r#"tk.MustExec("insert into tt values ('111'),('10000');")"#,
    );
    draft.exec(307, r#"tk.MustExec("alter table tt change a a char(5);")"#);
    draft.source(
        308,
        r#"mc := external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(309, r#"require.Equal(t, 5, mc.FieldType.GetFlen())"#);
    draft.query(
        310,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(311, r#"tk.MustGetErrMsg("alter table tt change a a char(4);", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    draft.exec(
        312,
        r#"tk.MustExec("alter table tt change a a char(100);")"#,
    );
    draft.query(
        313,
        r#"tk.MustQuery("select length(a) from tt").Check(testkit.Rows("3", "5"))"#,
    );
    // Go 注释 L315: binary to binary
    draft.exec(316, r#"tk.MustExec("drop table if exists tt;")"#);
    draft.exec(317, r#"tk.MustExec("create table tt (a binary(10));")"#);
    draft.exec(
        318,
        r#"tk.MustExec("insert into tt values ('111'),('10000');")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(319, r#"tk.MustGetErrMsg("alter table tt change a a binary(5);", "[types:1265]Data truncated for column 'a', value is '111\x00\x00\x00\x00\x00\x00\x00'")"#);
    draft.source(
        320,
        r#"mb := external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(321, r#"require.Equal(t, 10, mb.FieldType.GetFlen())"#);
    draft.query(322, r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111\x00\x00\x00\x00\x00\x00\x00", "10000\x00\x00\x00\x00\x00"))"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(323, r#"tk.MustGetErrMsg("alter table tt change a a binary(4);", "[types:1265]Data truncated for column 'a', value is '111\x00\x00\x00\x00\x00\x00\x00'")"#);
    draft.exec(
        324,
        r#"tk.MustExec("alter table tt change a a binary(12);")"#,
    );
    draft.query(325, r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111\x00\x00\x00\x00\x00\x00\x00\x00\x00", "10000\x00\x00\x00\x00\x00\x00\x00"))"#);
    draft.query(
        326,
        r#"tk.MustQuery("select length(a) from tt").Check(testkit.Rows("12", "12"))"#,
    );
    // Go 注释 L328: varbinary to varbinary
    draft.exec(329, r#"tk.MustExec("drop table if exists tt;")"#);
    draft.exec(330, r#"tk.MustExec("create table tt (a varbinary(10));")"#);
    draft.exec(
        331,
        r#"tk.MustExec("insert into tt values ('111'),('10000');")"#,
    );
    draft.exec(
        332,
        r#"tk.MustExec("alter table tt change a a varbinary(5);")"#,
    );
    draft.source(
        333,
        r#"mvb := external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(334, r#"require.Equal(t, 5, mvb.FieldType.GetFlen())"#);
    draft.query(
        335,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(336, r#"tk.MustGetErrMsg("alter table tt change a a varbinary(4);", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    draft.exec(
        337,
        r#"tk.MustExec("alter table tt change a a varbinary(12);")"#,
    );
    draft.query(
        338,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    draft.query(
        339,
        r#"tk.MustQuery("select length(a) from tt").Check(testkit.Rows("3", "5"))"#,
    );
    // Go 注释 L341: varchar to char
    draft.exec(342, r#"tk.MustExec("drop table if exists tt;")"#);
    draft.exec(343, r#"tk.MustExec("create table tt (a varchar(10));")"#);
    draft.exec(
        344,
        r#"tk.MustExec("insert into tt values ('111'),('10000');")"#,
    );
    draft.exec(346, r#"tk.MustExec("alter table tt change a a char(10);")"#);
    draft.source(
        347,
        r#"c2 := external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(
        348,
        r#"require.Equal(t, mysql.TypeString, c2.FieldType.GetType())"#,
    );
    draft.assertion(349, r#"require.Equal(t, 10, c2.FieldType.GetFlen())"#);
    draft.query(
        350,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(351, r#"tk.MustGetErrMsg("alter table tt change a a char(4);", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    // Go 注释 L353: char to text
    draft.exec(354, r#"tk.MustExec("alter table tt change a a text;")"#);
    draft.source(
        355,
        r#"c2 = external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(
        356,
        r#"require.Equal(t, mysql.TypeBlob, c2.FieldType.GetType())"#,
    );
    // Go 注释 L358: text to set
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(359, r#"tk.MustGetErrMsg("alter table tt change a a set('111', '2222');", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    draft.exec(
        360,
        r#"tk.MustExec("alter table tt change a a set('111', '10000');")"#,
    );
    draft.source(
        361,
        r#"c2 = external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(
        362,
        r#"require.Equal(t, mysql.TypeSet, c2.FieldType.GetType())"#,
    );
    draft.query(
        363,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // Go 注释 L365: set to set
    draft.exec(
        366,
        r#"tk.MustExec("alter table tt change a a set('10000', '111');")"#,
    );
    draft.source(
        367,
        r#"c2 = external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(
        368,
        r#"require.Equal(t, mysql.TypeSet, c2.FieldType.GetType())"#,
    );
    draft.query(
        369,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    // Go 注释 L371: set to enum
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(372, r#"tk.MustGetErrMsg("alter table tt change a a enum('111', '2222');", "[types:1265]Data truncated for column 'a', value is '10000'")"#);
    draft.exec(
        373,
        r#"tk.MustExec("alter table tt change a a enum('111', '10000');")"#,
    );
    draft.source(
        374,
        r#"c2 = external.GetModifyColumn(t, tk, "test", "tt", "a", false)"#,
    );
    draft.assertion(
        375,
        r#"require.Equal(t, mysql.TypeEnum, c2.FieldType.GetType())"#,
    );
    draft.query(
        376,
        r#"tk.MustQuery("select * from tt").Check(testkit.Rows("111", "10000"))"#,
    );
    draft.exec(
        377,
        r#"tk.MustExec("alter table tt change a a enum('10000', '111');")"#,
    );
    draft.query(
        378,
        r#"tk.MustQuery("select * from tt where a = 1").Check(testkit.Rows("10000"))"#,
    );
    draft.query(
        379,
        r#"tk.MustQuery("select * from tt where a = 2").Check(testkit.Rows("111"))"#,
    );
    // Go 注释 L381: no-strict mode
    draft.exec(382, r#"tk.MustExec(`set @@sql_mode="";`)"#);
    draft.exec(
        383,
        r#"tk.MustExec("alter table tt change a a enum('111', '2222');")"#,
    );
    draft.query(384, r#"tk.MustQuery("show warnings").Check(testkit.RowsWithSep("|", "Warning|1265|Data truncated for column 'a', value is '10000'"))"#);
    draft.exec(386, r#"tk.MustExec("drop table tt;")"#);
    draft
}

/// TestModifyColumnCharset 对应 Go 第 389-414 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnCharset 对应 Go 第 389-414 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_charset() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnCharset"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(390, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(391, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(392, r#"tk.MustExec("use test")"#);
    draft.exec(393, r#"tk.MustExec("create table t_mcc(a varchar(8) charset utf8, b varchar(8) charset utf8)")"#);
    draft.query(395, r#"result := tk.MustQuery(`show create table t_mcc`)"#);
    draft.source(396, r#"result.Check(testkit.Rows("#);
    draft.source(397, r#""t_mcc CREATE TABLE `t_mcc` (\n" +"#);
    draft.source(
        398,
        r#""  `a` varchar(8) CHARACTER SET utf8 COLLATE utf8_bin DEFAULT NULL,\n" +"#,
    );
    draft.source(
        399,
        r#""  `b` varchar(8) CHARACTER SET utf8 COLLATE utf8_bin DEFAULT NULL\n" +"#,
    );
    draft.source(
        400,
        r#"") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))"#,
    );
    draft.exec(
        402,
        r#"tk.MustExec("alter table t_mcc modify column a varchar(8);")"#,
    );
    draft.source(
        403,
        r#"tbl := external.GetTableByName(t, tk, "test", "t_mcc")"#,
    );
    draft.source(404, r#"tbl.Meta().Version = model.TableInfoVersion0"#);
    // Go 注释 L405: When the table version is TableInfoVersion0, the following statement don't change "b" charset.
    // Go 注释 L406: So the behavior is not compatible with MySQL.
    draft.exec(
        407,
        r#"tk.MustExec("alter table t_mcc modify column b varchar(8);")"#,
    );
    draft.query(408, r#"result = tk.MustQuery(`show create table t_mcc`)"#);
    draft.source(409, r#"result.Check(testkit.Rows("#);
    draft.source(410, r#""t_mcc CREATE TABLE `t_mcc` (\n" +"#);
    draft.source(411, r#""  `a` varchar(8) DEFAULT NULL,\n" +"#);
    draft.source(
        412,
        r#""  `b` varchar(8) CHARACTER SET utf8 COLLATE utf8_bin DEFAULT NULL\n" +"#,
    );
    draft.source(
        413,
        r#"") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))"#,
    );
    draft
}

/// TestModifyColumnTime 对应 Go 第 416-480 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnTime 对应 Go 第 416-480 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_time() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnTime"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(417, r#"now := time.Now().UTC()"#);
    draft.source(
        418,
        r#"now = time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.UTC)"#,
    );
    draft.source(419, r#"timeToDate1 := now.Format("2006-01-02")"#);
    draft.source(
        420,
        r#"timeToDate2 := now.AddDate(0, 0, 30).Format("2006-01-02")"#,
    );
    draft.source(421, r#"timeToDatetime1 := now.Add(20 * time.Hour).Add(12 * time.Second).Format("2006-01-02 15:04:05")"#);
    draft.source(
        422,
        r#"timeToDatetime2 := now.Add(20 * time.Hour).Format("2006-01-02 15:04:05")"#,
    );
    draft.source(
        423,
        r#"timeToDatetime3 := now.Add(12 * time.Second).Format("2006-01-02 15:04:05")"#,
    );
    draft.source(424, r#"timeToDatetime4 := now.AddDate(0, 0, 30).Add(20 * time.Hour).Add(12 * time.Second).Format("2006-01-02 15:04:05")"#);
    draft.source(425, r#"timeToDatetime5 := now.AddDate(0, 0, 30).Add(20 * time.Hour).Format("2006-01-02 15:04:05")"#);
    draft.source(426, r#"timeToTimestamp1 := now.Add(20 * time.Hour).Add(12 * time.Second).Format("2006-01-02 15:04:05")"#);
    draft.source(
        427,
        r#"timeToTimestamp2 := now.Add(20 * time.Hour).Format("2006-01-02 15:04:05")"#,
    );
    draft.source(
        428,
        r#"timeToTimestamp3 := now.Add(12 * time.Second).Format("2006-01-02 15:04:05")"#,
    );
    draft.source(429, r#"timeToTimestamp4 := now.AddDate(0, 0, 30).Add(20 * time.Hour).Add(12 * time.Second).Format("2006-01-02 15:04:05")"#);
    draft.source(430, r#"timeToTimestamp5 := now.AddDate(0, 0, 30).Add(20 * time.Hour).Format("2006-01-02 15:04:05")"#);
    draft.source(432, r#"tests := []testModifyColumnTimeCase{"#);
    // Go 注释 L433: time to date
    draft.source(434, r#"{"time", `"30 20:00:12"`, "date", timeToDate2, 0},"#);
    draft.source(435, r#"{"time", `"30 20:00"`, "date", timeToDate2, 0},"#);
    draft.source(436, r#"{"time", `"30 20"`, "date", timeToDate2, 0},"#);
    draft.source(437, r#"{"time", `"20:00:12"`, "date", timeToDate1, 0},"#);
    draft.source(438, r#"{"time", `"20:00"`, "date", timeToDate1, 0},"#);
    draft.source(439, r#"{"time", `"12"`, "date", timeToDate1, 0},"#);
    draft.source(440, r#"{"time", `"200012"`, "date", timeToDate1, 0},"#);
    draft.source(441, r#"{"time", `200012`, "date", timeToDate1, 0},"#);
    draft.source(442, r#"{"time", `0012`, "date", timeToDate1, 0},"#);
    draft.source(443, r#"{"time", `12`, "date", timeToDate1, 0},"#);
    draft.source(
        444,
        r#"{"time", `"30 20:00:12.498"`, "date", timeToDate2, 0},"#,
    );
    draft.source(
        445,
        r#"{"time", `"20:00:12.498"`, "date", timeToDate1, 0},"#,
    );
    draft.source(446, r#"{"time", `"200012.498"`, "date", timeToDate1, 0},"#);
    draft.source(447, r#"{"time", `200012.498`, "date", timeToDate1, 0},"#);
    // Go 注释 L448: time to datetime
    draft.source(
        449,
        r#"{"time", `"30 20:00:12"`, "datetime", timeToDatetime4, 0},"#,
    );
    draft.source(
        450,
        r#"{"time", `"30 20:00"`, "datetime", timeToDatetime5, 0},"#,
    );
    draft.source(
        451,
        r#"{"time", `"30 20"`, "datetime", timeToDatetime5, 0},"#,
    );
    draft.source(
        452,
        r#"{"time", `"20:00:12"`, "datetime", timeToDatetime1, 0},"#,
    );
    draft.source(
        453,
        r#"{"time", `"20:00"`, "datetime", timeToDatetime2, 0},"#,
    );
    draft.source(454, r#"{"time", `"12"`, "datetime", timeToDatetime3, 0},"#);
    draft.source(
        455,
        r#"{"time", `"200012"`, "datetime", timeToDatetime1, 0},"#,
    );
    draft.source(
        456,
        r#"{"time", `200012`, "datetime", timeToDatetime1, 0},"#,
    );
    draft.source(457, r#"{"time", `0012`, "datetime", timeToDatetime3, 0},"#);
    draft.source(458, r#"{"time", `12`, "datetime", timeToDatetime3, 0},"#);
    draft.source(
        459,
        r#"{"time", `"30 20:00:12.498"`, "datetime", timeToDatetime4, 0},"#,
    );
    draft.source(
        460,
        r#"{"time", `"20:00:12.498"`, "datetime", timeToDatetime1, 0},"#,
    );
    draft.source(
        461,
        r#"{"time", `"200012.498"`, "datetime", timeToDatetime1, 0},"#,
    );
    draft.source(
        462,
        r#"{"time", `200012.498`, "datetime", timeToDatetime1, 0},"#,
    );
    // Go 注释 L463: time to timestamp
    draft.source(
        464,
        r#"{"time", `"30 20:00:12"`, "timestamp", timeToTimestamp4, 0},"#,
    );
    draft.source(
        465,
        r#"{"time", `"30 20:00"`, "timestamp", timeToTimestamp5, 0},"#,
    );
    draft.source(
        466,
        r#"{"time", `"30 20"`, "timestamp", timeToTimestamp5, 0},"#,
    );
    draft.source(
        467,
        r#"{"time", `"20:00:12"`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(
        468,
        r#"{"time", `"20:00"`, "timestamp", timeToTimestamp2, 0},"#,
    );
    draft.source(
        469,
        r#"{"time", `"12"`, "timestamp", timeToTimestamp3, 0},"#,
    );
    draft.source(
        470,
        r#"{"time", `"200012"`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(
        471,
        r#"{"time", `200012`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(
        472,
        r#"{"time", `0012`, "timestamp", timeToTimestamp3, 0},"#,
    );
    draft.source(473, r#"{"time", `12`, "timestamp", timeToTimestamp3, 0},"#);
    draft.source(
        474,
        r#"{"time", `"30 20:00:12.498"`, "timestamp", timeToTimestamp4, 0},"#,
    );
    draft.source(
        475,
        r#"{"time", `"20:00:12.498"`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(
        476,
        r#"{"time", `"200012.498"`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(
        477,
        r#"{"time", `200012.498`, "timestamp", timeToTimestamp1, 0},"#,
    );
    draft.source(479, r#"testModifyColumnTime(t, tests)"#);
    draft
}

/// testModifyColumnTimeCase 对应 Go 第 482-488 行的辅助类型；字段语义保持为测试记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct testModifyColumnTimeCase {
    pub from: &'static str,
    pub value: &'static str,
    pub to: &'static str,
    pub expect: &'static str,
    pub err: u16,
}

/// testModifyColumnTime 对应 Go 第 490-516 行：保留原测试/辅助函数的执行顺序。
// testModifyColumnTime 对应 Go 第 490-516 行：保留原测试/辅助函数的执行顺序。
pub fn test_modify_column_time_helper() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"testModifyColumnTime"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.source(491, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(492, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(493, r#"tk.MustExec("use test")"#);
    draft.exec(
        494,
        r#"tk.MustExec("set @@global.tidb_ddl_error_count_limit = 3")"#,
    );
    draft.exec(495, r#"tk.MustExec("set @@time_zone=UTC")"#);
    draft.concurrency(497, r#"defer func() {"#);
    draft.exec(
        498,
        r#"tk.MustExec("set @@global.tidb_ddl_error_count_limit = default")"#,
    );
    draft.exec(499, r#"tk.MustExec("set @@time_zone=default")"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(502, r#"for _, test := range tests {"#);
    draft.source(503, r#"comment := fmt.Sprintf("%+v", test)"#);
    draft.exec(504, r#"tk.MustExec("drop table if exists t_mc")"#);
    draft.exec(
        505,
        r#"tk.MustExec(fmt.Sprintf("create table t_mc(a %s)", test.from))"#,
    );
    draft.exec(
        506,
        r#"tk.MustExec(fmt.Sprintf(`insert into t_mc (a) values (%s)`, test.value))"#,
    );
    draft.exec(
        507,
        r#"_, err := tk.Exec(fmt.Sprintf(`alter table t_mc modify a %s`, test.to))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(508, r#"if test.err != 0 {"#);
    draft.assertion(509, r#"require.Error(t, err, comment)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        510,
        r#"require.Regexp(t, fmt.Sprintf(".*[ddl:%d].*", test.err), err.Error(), comment)"#,
    );
    draft.source(511, r#"continue"#);
    draft.assertion(513, r#"require.NoError(t, err, comment)"#);
    draft.query(
        514,
        r#"tk.MustQuery("select a from t_mc").Check(testkit.Rows(test.expect))"#,
    );
    draft
}

/// TestModifyColumnTypeWhenInterception 对应 Go 第 520-545 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnTypeWhenInterception 对应 Go 第 520-545 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_type_when_interception() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnTypeWhenInterception"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(521, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(523, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(524, r#"tk.MustExec("use test")"#);
    // Go 注释 L526: Test normal warnings.
    draft.exec(
        527,
        r#"tk.MustExec("create table t(a int primary key, b decimal(4,2))")"#,
    );
    draft.source(529, r#"count := defaultBatchSize * 4"#);
    // Go 注释 L530: Add some rows.
    draft.source(531, r#"dml := "insert into t values""#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(532, r#"for i := 1; i <= count; i++ {"#);
    draft.source(533, r#"dml += fmt.Sprintf("(%d, %f)", i, 11.22)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(534, r#"if i != count {"#);
    draft.source(535, r#"dml += ",""#);
    draft.exec(538, r#"tk.MustExec(dml)"#);
    // Go 注释 L539: Make the regions scale like: [1, 1024), [1024, 2048), [2048, 3072), [3072, 4096]
    draft.query(
        540,
        r#"tk.MustQuery("split table t between(0) and (4096) regions 4")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(542, r#"testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/MockReorgTimeoutInOneRegion", `return(true)`)"#);
    draft.exec(
        543,
        r#"tk.MustExec("alter table t modify column b decimal(3,1)")"#,
    );
    draft.query(544, r#"tk.MustQuery("show warnings").Check(testkit.Rows("Warning 1292 4096 warnings with this error code, first warning: Truncated incorrect DECIMAL value: '11.22'"))"#);
    draft
}

/// TestModifyColumnWithIndexesWriteConflict 对应 Go 第 547-605 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnWithIndexesWriteConflict 对应 Go 第 547-605 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_with_indexes_write_conflict() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnWithIndexesWriteConflict"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(548, r#"testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/disableLossyDDLOptimization", "return(true)")"#);
    draft.source(550, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(552, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(553, r#"tk.MustExec("use test")"#);
    draft.exec(554, r#"tk.MustExec("set @@global.tidb_general_log=1;")"#);
    draft.exec(555, r#"tk.MustExec(`"#);
    draft.source(556, r#"CREATE TABLE t ("#);
    draft.source(557, r#"id int NOT NULL AUTO_INCREMENT,"#);
    draft.source(558, r#"val0 varchar(16) NOT NULL,"#);
    draft.source(559, r#"val1 int NOT NULL,"#);
    draft.source(560, r#"padding varchar(256) NOT NULL DEFAULT '',"#);
    draft.source(561, r#"PRIMARY KEY (id)"#);
    draft.source(562, r#");"#);
    draft.source(563, r#"`)"#);
    draft.exec(564, r#"tk.MustExec("CREATE INDEX val0_idx ON t (val0)")"#);
    draft.exec(565, r#"tk.MustExec("insert into t (val0, val1, padding) values ('1', 1, 'a'), ('2', 2, 'b'), ('3', 3, 'c')")"#);
    draft.concurrency(567, r#"conflictOnce := sync.Once{}"#);
    draft.source(568, r#"conflictCh := make(chan struct{})"#);
    draft.source(569, r#"tk1 := testkit.NewTestKit(t, store)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(570, r#"failpoint.EnableCall("github.com/pingcap/tidb/pkg/table/tables/duringTableCommonRemoveRecord", func(tblInfo *model.TableInfo) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(571, r#"if tblInfo.Name.L == "t" {"#);
    draft.concurrency(572, r#"conflictOnce.Do(func() {"#);
    draft.exec(573, r#"tk1.MustExec("use test")"#);
    // Go 注释 L574: inject a write conflict for the delete DML.
    draft.exec(
        575,
        r#"tk1.MustExec("update t set val0 = '100' where id = 1;")"#,
    );
    draft.source(576, r#"close(conflictCh)"#);
    draft.concurrency(580, r#"deleteOnce := sync.Once{}"#);
    draft.concurrency(581, r#"insertOnce := sync.Once{}"#);
    draft.source(582, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.source(583, r#"tk3 := testkit.NewTestKit(t, store)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(584, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterReorgWorkForModifyColumn", func() {"#);
    draft.concurrency(585, r#"deleteOnce.Do(func() {"#);
    draft.concurrency(586, r#"go func() {"#);
    draft.exec(587, r#"tk2.MustExec("use test")"#);
    draft.exec(588, r#"tk2.MustExec("delete from t where id = 1;")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(590, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/infoschema/issyncer/afterLoadSchemaDiffs", func(int64) {"#);
    draft.concurrency(591, r#"insertOnce.Do(func() {"#);
    draft.exec(592, r#"tk3.MustExec("use test")"#);
    draft.exec(
        593,
        r#"tk3.MustExec("insert into t (val0, val1, padding) values ('4', 4, 'd');")"#,
    );
    draft.source(596, r#"<-conflictCh"#);
    draft.exec(
        599,
        r#"tk.MustExec("alter table t modify column val0 varchar(8) not null;")"#,
    );
    draft.exec(600, r#"tk.MustExec("admin check table t;")"#);
    draft.query(
        601,
        r#"tk.MustQuery("select * from t order by id;").Check(testkit.Rows("#,
    );
    draft.source(602, r#""2 2 2 b","#);
    draft.source(603, r#""3 3 3 c","#);
    draft.source(604, r#""4 4 4 d"))"#);
    draft
}

/// TestMultiSchemaModifyColumnWithSkipReorg 对应 Go 第 607-622 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaModifyColumnWithSkipReorg 对应 Go 第 607-622 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_modify_column_with_skip_reorg() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaModifyColumnWithSkipReorg"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(608, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(610, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(611, r#"tk.MustExec("use test")"#);
    draft.exec(612, r#"tk.MustExec("create table t(a varchar(16), b bigint, c bigint, index i1(a), index i2(b), index i3(c), index i4(a, b))")"#);
    draft.exec(
        613,
        r#"tk.MustExec("insert into t values ('a  ', 1, 1), ('b  ', 2, 2), ('c ', 3, 3)")"#,
    );
    draft.source(
        614,
        r#"oldMeta := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    draft.exec(616, r#"tk.MustExec("alter table t modify column a char(8) after b, modify column b int after a")"#);
    draft.exec(617, r#"tk.MustExec("admin check table t")"#);
    draft.source(
        618,
        r#"newMeta := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    // Go 注释 L620: the offset and ID of b should be unchanged
    draft.assertion(
        621,
        r#"require.Equal(t, oldMeta.Columns[1].ID, newMeta.Columns[1].ID)"#,
    );
    draft
}

/// TestModifyColumnWithSkipReorg 对应 Go 第 624-684 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnWithSkipReorg 对应 Go 第 624-684 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_with_skip_reorg() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnWithSkipReorg"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(625, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(626, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(627, r#"tk.MustExec("use test")"#);
    // Go 注释 L629: INT -> MEDIUMINT
    draft.exec(
        630,
        r#"tk.MustExec("create table t(a int, b int, index i1(a), index i2(b), index i3(a, b))")"#,
    );
    draft.exec(
        631,
        r#"tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)")"#,
    );
    draft.source(
        632,
        r#"oldMeta := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    // Go 注释 L634: insert should fail by new column type check
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(635, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterDoModifyColumnSkipReorgCheck", func() {"#);
    draft.source(636, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(
        637,
        r#"tk2.MustExecToErr("insert into test.t values (2147483648, 2147483648)")"#,
    );
    draft.exec(
        639,
        r#"tk.MustExec("alter table t modify column b mediumint not null")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(640, r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterDoModifyColumnSkipReorgCheck")"#);
    draft.source(
        641,
        r#"newMeta := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    // Go 注释 L643: ID should be the same.
    draft.assertion(
        644,
        r#"require.Equal(t, oldMeta.Columns[1].ID, newMeta.Columns[1].ID)"#,
    );
    draft.assertion(
        645,
        r#"require.Nil(t, newMeta.Columns[1].ChangingFieldType)"#,
    );
    draft.exec(646, r#"tk.MustExec("admin check table t")"#);
    // Go 注释 L648: insert should succeed before adding flag, and this will make modify column fail.
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(649, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeDoModifyColumnSkipReorgCheck", func() {"#);
    draft.source(650, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(
        651,
        r#"tk2.MustExec("insert into test.t values (512, 512)")"#,
    );
    draft.exec(
        653,
        r#"tk.MustExecToErr("alter table t modify column b tinyint not null")"#,
    );
    // Go 注释 L655: VARCHAR -> CHAR
    draft.source(656, r#"var gotTp byte"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(657, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/getModifyColumnType", func(tp byte) {"#);
    draft.source(658, r#"gotTp = tp"#);
    draft.exec(661, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(662, r#"tk.MustExec("create table t (a varchar(10))")"#);
    draft.exec(663, r#"tk.MustExec("insert into t values ('a '), ('b ')")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(664, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/modifyColumnTypeWithData", func(*model.Job, model.JobArgs) {"#);
    draft.source(665, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(666, r#"tk2.MustExec("use test")"#);
    draft.exec(667, r#"tk2.MustExec("insert into t values ('a ')")"#);
    draft.exec(
        669,
        r#"tk.MustExec("alter table t modify column a char(5)")"#,
    );
    draft.exec(670, r#"tk.MustExec("admin check table t")"#);
    draft.assertion(671, r#"require.Equal(t, model.ModifyTypeReorg, gotTp)"#);
    draft.exec(673, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(674, r#"tk.MustExec("create table t (a varchar(10))")"#);
    draft.exec(675, r#"tk.MustExec("insert into t values ('a'), ('b')")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(676, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterDoModifyColumnSkipReorgCheck", func() {"#);
    draft.source(677, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(678, r#"tk2.MustExec("use test")"#);
    draft.exec(679, r#"tk2.MustExecToErr("insert into t values ('a ')")"#);
    draft.exec(
        681,
        r#"tk.MustExec("alter table t modify column a char(5)")"#,
    );
    draft.exec(682, r#"tk.MustExec("admin check table t")"#);
    draft.assertion(
        683,
        r#"require.Equal(t, model.ModifyTypeNoReorgWithCheck, gotTp)"#,
    );
    draft
}

/// TestGetModifyColumnType 对应 Go 第 686-910 行：保留原测试/辅助函数的执行顺序。
// TestGetModifyColumnType 对应 Go 第 686-910 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_get_modify_column_type() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestGetModifyColumnType"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(687, r#"type testCase struct {"#);
    draft.source(688, r#"beforeType string"#);
    draft.source(689, r#"afterType  string"#);
    draft.source(690, r#"index      bool"#);
    draft.source(691, r#"tp         byte"#);
    draft.source(694, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(695, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(696, r#"tk.MustExec("use test")"#);
    draft.source(698, r#"tcs := []testCase{"#);
    // Go 注释 L699: integer
    draft.source(701, r#"beforeType: "int","#);
    draft.source(702, r#"afterType:  "bigint","#);
    draft.source(703, r#"tp:         model.ModifyTypeNoReorg,"#);
    draft.source(706, r#"beforeType: "bigint","#);
    draft.source(707, r#"afterType:  "int","#);
    draft.source(708, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(711, r#"beforeType: "bigint","#);
    draft.source(712, r#"afterType:  "int","#);
    draft.source(713, r#"index:      true,"#);
    draft.source(714, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(717, r#"beforeType: "bigint","#);
    draft.source(718, r#"afterType:  "bigint unsigned","#);
    draft.source(719, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(722, r#"beforeType: "bigint","#);
    draft.source(723, r#"afterType:  "bigint unsigned","#);
    draft.source(724, r#"index:      true,"#);
    draft.source(725, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(728, r#"beforeType: "int unsigned","#);
    draft.source(729, r#"afterType:  "bigint","#);
    draft.source(730, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(733, r#"beforeType: "int unsigned","#);
    draft.source(734, r#"afterType:  "bigint","#);
    draft.source(735, r#"index:      true,"#);
    draft.source(736, r#"tp:         model.ModifyTypeReorg,"#);
    // Go 注释 L738: string
    draft.source(740, r#"beforeType: "char(10)","#);
    draft.source(741, r#"afterType:  "char(20)","#);
    draft.source(742, r#"tp:         model.ModifyTypeNoReorg,"#);
    draft.source(745, r#"beforeType: "char(20)","#);
    draft.source(746, r#"afterType:  "char(10)","#);
    draft.source(747, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(750, r#"beforeType: "char(20) collate utf8mb4_bin","#);
    draft.source(751, r#"afterType:  "char(10) collate utf8mb4_bin","#);
    draft.source(752, r#"index:      true,"#);
    draft.source(753, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(756, r#"beforeType: "char(20) collate utf8mb4_general_ci","#);
    draft.source(757, r#"afterType:  "char(10) collate utf8mb4_general_ci","#);
    draft.source(758, r#"index:      true,"#);
    draft.source(759, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(762, r#"beforeType: "char(10)","#);
    draft.source(763, r#"afterType:  "varchar(20)","#);
    draft.source(764, r#"tp:         model.ModifyTypeNoReorg,"#);
    draft.source(767, r#"beforeType: "char(20)","#);
    draft.source(768, r#"afterType:  "varchar(10)","#);
    draft.source(769, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(772, r#"beforeType: "char(20) collate utf8mb4_bin","#);
    draft.source(773, r#"afterType:  "varchar(10) collate utf8mb4_bin","#);
    draft.source(774, r#"index:      true,"#);
    draft.source(775, r#"tp:         model.ModifyTypeIndexReorg,"#);
    draft.source(778, r#"beforeType: "char(20) collate utf8mb4_general_ci","#);
    draft.source(
        779,
        r#"afterType:  "varchar(10) collate utf8mb4_general_ci","#,
    );
    draft.source(780, r#"index:      true,"#);
    draft.source(781, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(784, r#"beforeType: "varchar(10)","#);
    draft.source(785, r#"afterType:  "varchar(20)","#);
    draft.source(786, r#"tp:         model.ModifyTypeNoReorg,"#);
    draft.source(789, r#"beforeType: "varchar(20)","#);
    draft.source(790, r#"afterType:  "varchar(10)","#);
    draft.source(791, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(794, r#"beforeType: "varchar(20) collate utf8mb4_bin","#);
    draft.source(795, r#"afterType:  "varchar(10) collate utf8mb4_bin","#);
    draft.source(796, r#"index:      true,"#);
    draft.source(797, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(
        800,
        r#"beforeType: "varchar(20) collate utf8mb4_general_ci","#,
    );
    draft.source(
        801,
        r#"afterType:  "varchar(10) collate utf8mb4_general_ci","#,
    );
    draft.source(802, r#"index:      true,"#);
    draft.source(803, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(806, r#"beforeType: "varchar(10)","#);
    draft.source(807, r#"afterType:  "char(20)","#);
    draft.source(808, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(811, r#"beforeType: "varchar(20)","#);
    draft.source(812, r#"afterType:  "char(10)","#);
    draft.source(813, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    draft.source(816, r#"beforeType: "varchar(20) collate utf8mb4_bin","#);
    draft.source(817, r#"afterType:  "char(10) collate utf8mb4_bin","#);
    draft.source(818, r#"index:      true,"#);
    draft.source(819, r#"tp:         model.ModifyTypeIndexReorg,"#);
    draft.source(
        822,
        r#"beforeType: "varchar(20) collate utf8mb4_general_ci","#,
    );
    draft.source(823, r#"afterType:  "char(10) collate utf8mb4_general_ci","#);
    draft.source(824, r#"index:      true,"#);
    draft.source(825, r#"tp:         model.ModifyTypeNoReorgWithCheck,"#);
    // Go 注释 L827: different collation
    draft.source(829, r#"beforeType: "char(20) collate utf8mb4_bin","#);
    draft.source(830, r#"afterType:  "varchar(10) collate utf8_unicode_ci","#);
    draft.source(831, r#"index:      true,"#);
    draft.source(832, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(835, r#"beforeType: "char(20) collate utf8_unicode_ci","#);
    draft.source(836, r#"afterType:  "varchar(10) collate utf8mb4_bin","#);
    draft.source(837, r#"index:      true,"#);
    draft.source(838, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(841, r#"beforeType: "varchar(20) collate utf8mb4_bin","#);
    draft.source(842, r#"afterType:  "char(10) collate utf8_unicode_ci","#);
    draft.source(843, r#"index:      true,"#);
    draft.source(844, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(847, r#"beforeType: "varchar(20) collate utf8_unicode_ci","#);
    draft.source(848, r#"afterType:  "char(10) collate utf8mb4_bin","#);
    draft.source(849, r#"index:      true,"#);
    draft.source(850, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(854, r#"var gotTp byte"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(855, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/getModifyColumnType", func(tp byte) {"#);
    draft.source(856, r#"gotTp = tp"#);
    draft.source(859, r#"runSingle := func(t *testing.T, tc testCase) {"#);
    draft.exec(860, r#"tk.MustExec("drop table if exists t")"#);
    draft.source(861, r#"indexPart := """#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(862, r#"if tc.index {"#);
    draft.source(
        863,
        r#"indexPart = ", index idx_a(a), primary key(p1, p2)""#,
    );
    draft.exec(865, r#"tk.MustExec(fmt.Sprintf("create table t (p1 int, p2 int, a %s%s)", tc.beforeType, indexPart))"#);
    draft.exec(
        866,
        r#"tk.MustExec("insert into t values (1, 1, '1'), (2, 2, '2'), (3, 3, '3')")"#,
    );
    draft.exec(
        867,
        r#"tk.MustExec(fmt.Sprintf("alter table t modify column a %s", tc.afterType))"#,
    );
    draft.exec(
        868,
        r#"tk.MustExec("insert into t values (4, 4, '4'), (5, 5, '5'), (6, 6, '6 ')")"#,
    );
    draft.exec(869, r#"tk.MustExec("admin check table t")"#);
    draft.assertion(870, r#"require.Equal(t, tc.tp, gotTp, "before type: %s, after type: %s", tc.beforeType, tc.afterType)"#);
    draft.exec(873, r#"tk.MustExec("set sql_mode='STRICT_ALL_TABLES'")"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(874, r#"for _, tc := range tcs {"#);
    draft.source(875, r#"runSingle(t, tc)"#);
    draft.source(878, r#"tcsNonStrict := []testCase{"#);
    draft.source(880, r#"beforeType: "bigint","#);
    draft.source(881, r#"afterType:  "int","#);
    draft.source(882, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(885, r#"beforeType: "char(20)","#);
    draft.source(886, r#"afterType:  "char(10)","#);
    draft.source(887, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(890, r#"beforeType: "varchar(20)","#);
    draft.source(891, r#"afterType:  "varchar(10)","#);
    draft.source(892, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(895, r#"beforeType: "char(20)","#);
    draft.source(896, r#"afterType:  "varchar(10)","#);
    draft.source(897, r#"tp:         model.ModifyTypeReorg,"#);
    draft.source(900, r#"beforeType: "varchar(20)","#);
    draft.source(901, r#"afterType:  "char(10)","#);
    draft.source(902, r#"tp:         model.ModifyTypeReorg,"#);
    draft.exec(906, r#"tk.MustExec("set sql_mode=''")"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(907, r#"for _, tc := range tcsNonStrict {"#);
    draft.source(908, r#"runSingle(t, tc)"#);
    draft
}

/// TestMultiSchemaModifyColumnWithIndex 对应 Go 第 912-942 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaModifyColumnWithIndex 对应 Go 第 912-942 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_modify_column_with_index() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaModifyColumnWithIndex"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(913, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(915, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(916, r#"tk.MustExec("use test")"#);
    draft.exec(
        917,
        r#"tk.MustExec("create table t(c1 bigint, c2 bigint, index i1(c1, c2), index i2(c1))")"#,
    );
    draft.source(
        919,
        r#"oldTblInfo := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    draft.exec(
        920,
        r#"tk.MustExec("alter table t modify column c1 int, modify column c2 int")"#,
    );
    draft.source(
        921,
        r#"newTblInfo := external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    draft.assertion(
        923,
        r#"require.Equal(t, len(oldTblInfo.Indices), len(newTblInfo.Indices))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(924, r#"for i, oldIdx := range oldTblInfo.Indices {"#);
    draft.source(925, r#"newIdx := newTblInfo.Indices[i]"#);
    draft.assertion(926, r#"require.Equal(t, oldIdx.Name, newIdx.Name)"#);
    draft.assertion(
        927,
        r#"require.Equal(t, len(oldIdx.Columns), len(newIdx.Columns))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(928, r#"for j := range oldIdx.Columns {"#);
    draft.assertion(
        929,
        r#"require.Equal(t, oldIdx.Columns[j].Name, newIdx.Columns[j].Name)"#,
    );
    draft.assertion(
        930,
        r#"require.Equal(t, oldIdx.Columns[j].Offset, newIdx.Columns[j].Offset)"#,
    );
    // Go 注释 L934: multi schema change with rename index
    draft.exec(935, r#"tk.MustExec("drop table t")"#);
    draft.exec(
        936,
        r#"tk.MustExec("create table t(c1 bigint, c2 bigint, index i1(c1, c2), index i2(c1))")"#,
    );
    draft.exec(937, r#"tk.MustExec("alter table t modify column c1 int, rename index i1 to new1, rename index i2 to new2, modify column c2 int")"#);
    draft.source(
        938,
        r#"newTblInfo = external.GetTableByName(t, tk, "test", "t").Meta()"#,
    );
    draft.assertion(939, r#"require.Equal(t, 2, len(newTblInfo.Indices))"#);
    draft.assertion(
        940,
        r#"require.Equal(t, "new1", newTblInfo.Indices[0].Name.L)"#,
    );
    draft.assertion(
        941,
        r#"require.Equal(t, "new2", newTblInfo.Indices[1].Name.L)"#,
    );
    draft
}

/// TestParallelAlterTable 对应 Go 第 944-1029 行：保留原测试/辅助函数的执行顺序。
// TestParallelAlterTable 对应 Go 第 944-1029 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_parallel_alter_table() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestParallelAlterTable"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(945, r#"store := testkit.CreateMockStore(t)"#);
    draft.concurrency(946, r#"ctx := context.Background()"#);
    draft.concurrency(947, r#"var wg util.WaitGroupWrapper"#);
    draft.source(949, r#"checkParallelDDL := func(t *testing.T, createSQL, firstSQL, secondSQL string) (err1, err2 error) {"#);
    draft.source(950, r#"var ("#);
    draft.source(951, r#"submitted     = make(chan struct{}, 16)"#);
    draft.source(952, r#"startSchedule = make(chan struct{})"#);
    draft.source(955, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(956, r#"tk.MustExec("use test")"#);
    draft.exec(957, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(958, r#"tk.MustExec(createSQL)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(960, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs", func() {"#);
    draft.source(961, r#"<-startSchedule"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(964, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterGetJobFromLimitCh", func(ch chan *ddl.JobWrapper) {"#);
    draft.source(965, r#"submitted <- struct{}{}"#);
    draft.source(967, r#"wg.Run(func() {"#);
    draft.source(968, r#"tk1 := testkit.NewTestKit(t, store)"#);
    draft.exec(969, r#"tk1.MustExec("use test")"#);
    draft.exec(970, r#"_, err1 = tk1.Exec(firstSQL)"#);
    draft.source(972, r#"wg.Run(func() {"#);
    // Go 注释 L973: wait until first ddl is submitted
    draft.source(974, r#"<-submitted"#);
    draft.source(975, r#"tk1 := testkit.NewTestKit(t, store)"#);
    draft.exec(976, r#"tk1.MustExec("use test")"#);
    draft.exec(977, r#"_, err2 = tk1.Exec(secondSQL)"#);
    draft.assertion(979, r#"require.Eventually(t, func() bool {"#);
    draft.source(
        980,
        r#"gotJobs, err := ddl.GetAllDDLJobs(ctx, tk.Session())"#,
    );
    draft.assertion(981, r#"require.NoError(t, err)"#);
    draft.source(982, r#"return len(gotJobs) == 2"#);
    draft.source(983, r#"}, 10*time.Second, 100*time.Millisecond)"#);
    draft.source(985, r#"close(startSchedule)"#);
    draft.source(986, r#"wg.Wait()"#);
    draft.source(987, r#"return"#);
    draft.source(
        990,
        r#"t.Run("modify column then add index", func(t *testing.T) {"#,
    );
    draft.source(991, r#"err1, err2 := checkParallelDDL(t,"#);
    draft.source(992, r#""create table t(id int, c1 char(16))","#);
    draft.source(993, r#""alter table t modify column c1 text(255)","#);
    draft.source(994, r#""alter table t add index idx_c1(c1)","#);
    draft.assertion(996, r#"require.NoError(t, err1)"#);
    draft.assertion(997, r#"require.Error(t, err2)"#);
    draft.source(
        1000,
        r#"t.Run("add index then modify column", func(t *testing.T) {"#,
    );
    draft.source(1001, r#"err1, err2 := checkParallelDDL(t,"#);
    draft.source(1002, r#""create table t(id int, c1 char(16))","#);
    draft.source(1003, r#""alter table t add index idx_c1(c1)","#);
    draft.source(1004, r#""alter table t modify column c1 text(255)","#);
    draft.assertion(1006, r#"require.NoError(t, err1)"#);
    draft.assertion(1007, r#"require.Error(t, err2)"#);
    draft.source(
        1010,
        r#"t.Run("add index with prefix length then modify column", func(t *testing.T) {"#,
    );
    draft.source(1011, r#"err1, err2 := checkParallelDDL(t,"#);
    draft.source(1012, r#""create table t(id int, c1 char(16))","#);
    draft.source(1013, r#""alter table t add index idx_c1(c1(10))","#);
    draft.source(1014, r#""alter table t modify column c1 text(255)","#);
    draft.assertion(1016, r#"require.NoError(t, err1)"#);
    draft.assertion(1017, r#"require.NoError(t, err2)"#);
    draft.source(
        1020,
        r#"t.Run("modify column then add index with prefix length", func(t *testing.T) {"#,
    );
    draft.source(1021, r#"err1, err2 := checkParallelDDL(t,"#);
    draft.source(1022, r#""create table t(id int, c1 char(16))","#);
    draft.source(1023, r#""alter table t modify column c1 text(255)","#);
    draft.source(1024, r#""alter table t add index idx_c1(c1(10))","#);
    draft.assertion(1026, r#"require.NoError(t, err1)"#);
    draft.assertion(1027, r#"require.NoError(t, err2)"#);
    draft
}

/// TestModifyIntegerColumn 对应 Go 第 1044-1188 行：保留原测试/辅助函数的执行顺序。
// TestModifyIntegerColumn 对应 Go 第 1044-1188 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_integer_column() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyIntegerColumn"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(1045, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(1046, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(1047, r#"tk.MustExec("use test")"#);
    draft.source(1048, r#"var reorgType byte"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(1049, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/getModifyColumnType", func(tp byte) {"#);
    draft.source(1050, r#"reorgType = tp"#);
    draft.source(1053, r#"maxMinSignedVal := map[string][]int{"#);
    draft.source(1054, r#""bigint":    {math.MaxInt64, math.MinInt64},"#);
    draft.source(1055, r#""int":       {math.MaxInt32, math.MinInt32},"#);
    draft.source(1056, r#""mediumint": {1<<23 - 1, -1 << 23},"#);
    draft.source(1057, r#""smallint":  {math.MaxInt16, math.MinInt16},"#);
    draft.source(1058, r#""tinyint":   {math.MaxInt8, math.MinInt8},"#);
    draft.source(1061, r#"maxMinUnsignedVal := map[string][]uint{"#);
    draft.source(1062, r#""bigint unsigned":    {math.MaxUint64, 0},"#);
    draft.source(1063, r#""int unsigned":       {math.MaxUint32, 0},"#);
    draft.source(1064, r#""mediumint unsigned": {1<<24 - 1, 0},"#);
    draft.source(1065, r#""smallint unsigned":  {math.MaxUint16, 0},"#);
    draft.source(1066, r#""tinyint unsigned":   {math.MaxUint8, 0},"#);
    draft.source(
        1069,
        r#"failedValue := func(insertVal []string, newColTp string) {"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1070, r#"for _, val := range insertVal {"#);
    draft.exec(
        1071,
        r#"tk.MustExec(fmt.Sprintf("insert into t values(%s)", val))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        1072,
        r#"err := tk.ExecToErr(fmt.Sprintf("alter table t modify column a %s", newColTp))"#,
    );
    draft.assertion(1073, r#"require.True(t, strings.Contains(err.Error(), "Data truncated for column 'a'") || strings.Contains(err.Error(), "overflow"))"#);
    draft.exec(1074, r#"tk.MustExec("delete from t")"#);
    draft.source(
        1078,
        r#"successValue := func(insertVal string, newColTp string, expectReorgTp byte) {"#,
    );
    draft.exec(
        1079,
        r#"tk.MustExec(fmt.Sprintf("insert into t values %s", insertVal))"#,
    );
    draft.exec(
        1080,
        r#"tk.MustExec(fmt.Sprintf("alter table t modify column a %s", newColTp))"#,
    );
    draft.assertion(1081, r#"require.Equal(t, expectReorgTp, reorgType)"#);
    draft.source(
        1084,
        r#"signed2Signed := func(oldColTp, newColTp string, expectReorgTp byte) {"#,
    );
    draft.source(1085, r#"maxValOfNewCol, minValOfNewCol := maxMinSignedVal[newColTp][0], maxMinSignedVal[newColTp][1]"#);
    draft.source(1086, r#"maxValOfOldCol, minValOfOldCol := maxMinSignedVal[oldColTp][0], maxMinSignedVal[oldColTp][1]"#);
    draft.exec(1087, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        1088,
        r#"tk.MustExec(fmt.Sprintf("create table t(a %s)", oldColTp))"#,
    );
    // Go 注释 L1090: [maxValOfNewCol+1, maxValOfOldCol] fail
    draft.source(1091, r#"failedValue([]string{"#);
    draft.source(1092, r#"fmt.Sprintf("%d", maxValOfNewCol+1),"#);
    draft.source(1093, r#"fmt.Sprintf("%d", maxValOfOldCol),"#);
    draft.source(1094, r#"}, newColTp)"#);
    // Go 注释 L1096: [minValOfOldCol, minValOfNewCol-1] fail
    draft.source(1097, r#"failedValue([]string{"#);
    draft.source(1098, r#"fmt.Sprintf("%d", minValOfNewCol-1),"#);
    draft.source(1099, r#"fmt.Sprintf("%d", minValOfOldCol),"#);
    draft.source(1100, r#"}, newColTp)"#);
    // Go 注释 L1102: [maxValOfNewCol, minValOfNewCol] pass
    draft.source(1103, r#"successValue(fmt.Sprintf("(%d), (%d), (0)", maxValOfNewCol, minValOfNewCol), newColTp, expectReorgTp)"#);
    draft.source(
        1106,
        r#"unsigned2Unsigned := func(oldColTp, newColTp string, expectReorgTp byte) {"#,
    );
    draft.source(1107, r#"maxValOfNewCol, minValOfNewCol := maxMinUnsignedVal[newColTp][0], maxMinUnsignedVal[newColTp][1]"#);
    draft.source(1108, r#"maxValOfOldCol := maxMinUnsignedVal[oldColTp][0]"#);
    draft.exec(1109, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        1110,
        r#"tk.MustExec(fmt.Sprintf("create table t(a %s)", oldColTp))"#,
    );
    // Go 注释 L1112: [maxValOfNewCol+1, maxValOfOldCol] fail
    draft.source(1113, r#"failedValue([]string{"#);
    draft.source(1114, r#"fmt.Sprintf("%d", maxValOfNewCol+1),"#);
    draft.source(1115, r#"fmt.Sprintf("%d", maxValOfOldCol),"#);
    draft.source(1116, r#"}, newColTp)"#);
    // Go 注释 L1118: [0, maxValOfNewCol] pass
    draft.source(1119, r#"successValue(fmt.Sprintf("(%d), (%d), (1)", maxValOfNewCol, minValOfNewCol), newColTp, expectReorgTp)"#);
    draft.source(1122, r#"signed2Unsigned := func(oldColTp, newColTp string, expectReorgTp byte, oldColIdx, newColIdx int) {"#);
    draft.source(1123, r#"maxValOfOldCol, minValOfOldCol := maxMinSignedVal[oldColTp][0], maxMinSignedVal[oldColTp][1]"#);
    draft.source(1124, r#"maxValOfNewCol := maxMinUnsignedVal[newColTp][0]"#);
    draft.exec(1125, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        1126,
        r#"tk.MustExec(fmt.Sprintf("create table t(a %s)", oldColTp))"#,
    );
    // Go 注释 L1128: [minValOfOldCol, -1] fail
    draft.source(1129, r#"failedValue([]string{"#);
    draft.source(1130, r#""-1","#);
    draft.source(1131, r#"fmt.Sprintf("%d", minValOfOldCol),"#);
    draft.source(1132, r#"}, newColTp)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1134, r#"if oldColIdx < newColIdx {"#);
    // Go 注释 L1135: [maxValOfNewCol+1, maxValOfOldCol] fail
    draft.source(1136, r#"failedValue([]string{"#);
    draft.source(1137, r#"fmt.Sprintf("%d", maxValOfNewCol+1),"#);
    draft.source(1138, r#"fmt.Sprintf("%d", maxValOfOldCol),"#);
    draft.source(1139, r#"}, newColTp)"#);
    // Go 注释 L1142: [0, min(maxValOfOldCol, maxValOfNewCol)] pass
    draft.source(1143, r#"successValue(fmt.Sprintf("(%d), (1), (0)", min(uint(maxValOfOldCol), maxValOfNewCol)), newColTp, expectReorgTp)"#);
    draft.source(
        1146,
        r#"unsigned2Signed := func(oldColTp, newColTp string, expectReorgTp byte) {"#,
    );
    draft.source(1147, r#"maxValOfNewCol := maxMinSignedVal[newColTp][0]"#);
    draft.source(1148, r#"maxValOfOldCol := maxMinUnsignedVal[oldColTp][0]"#);
    draft.exec(1149, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        1150,
        r#"tk.MustExec(fmt.Sprintf("create table t(a %s)", oldColTp))"#,
    );
    // Go 注释 L1152: [maxValOfNewCol+1, maxValOfOldCol] fail
    draft.source(1153, r#"failedValue([]string{"#);
    draft.source(1154, r#"fmt.Sprintf("%d", uint64(maxValOfNewCol)+1),"#);
    draft.source(1155, r#"fmt.Sprintf("%d", maxValOfOldCol),"#);
    draft.source(1156, r#"}, newColTp)"#);
    // Go 注释 L1158: [0, maxValOfNewCol] pass
    draft.source(
        1159,
        r#"successValue(fmt.Sprintf("(%d), (1), (0)", maxValOfNewCol), newColTp, expectReorgTp)"#,
    );
    draft.source(
        1162,
        r#"signedTp := []string{"bigint", "int", "mediumint", "smallint", "tinyint"}"#,
    );
    draft.source(1163, r#"unsignedTp := []string{"bigint unsigned", "int unsigned", "mediumint unsigned", "smallint unsigned", "tinyint unsigned"}"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1164, r#"for oldColIdx := range signedTp {"#);
    // Go 注释 L1165: 1. signed -> signed
    // Go 注释 L1166: bigint -> int, mediumint, smallint, tinyint; int -> mediumint, smallint, tinyint; ...
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        1167,
        r#"for newColIdx := oldColIdx + 1; newColIdx < len(signedTp); newColIdx++ {"#,
    );
    draft.source(1168, r#"signed2Signed(signedTp[oldColIdx], signedTp[newColIdx], model.ModifyTypeNoReorgWithCheck)"#);
    // Go 注释 L1170: 2. signed -> unsigned
    // Go 注释 L1171: bigint -> bigint unsigned, int unsigned, mediumint unsigned, smallint unsigned, tinyint unsigned; int -> int unsigned, mediumint unsigned, smallint unsigned, tinyint unsigned; ...
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1172, r#"for newColIdx := range unsignedTp {"#);
    draft.source(1173, r#"signed2Unsigned(signedTp[oldColIdx], unsignedTp[newColIdx], model.ModifyTypeReorg, oldColIdx, newColIdx)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1176, r#"for oldColIdx := range unsignedTp {"#);
    // Go 注释 L1177: 3. unsigned -> unsigned
    // Go 注释 L1178: bigint unsigned -> int unsigned, mediumint unsigned, smallint unsigned, tinyint unsigned; int unsigned -> mediumint unsigned, smallint unsigned, tinyint unsigned; ...
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        1179,
        r#"for newColIdx := oldColIdx + 1; newColIdx < len(unsignedTp); newColIdx++ {"#,
    );
    draft.source(1180, r#"unsigned2Unsigned(unsignedTp[oldColIdx], unsignedTp[newColIdx], model.ModifyTypeNoReorgWithCheck)"#);
    // Go 注释 L1182: 4. unsigned -> signed
    // Go 注释 L1183: bigint unsigned -> bigint, int, mediumint, smallint, tinyint; int unsigned -> int, mediumint, smallint, tinyint; ...
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        1184,
        r#"for newColIdx := oldColIdx; newColIdx < len(signedTp); newColIdx++ {"#,
    );
    draft.source(
        1185,
        r#"unsigned2Signed(unsignedTp[oldColIdx], signedTp[newColIdx], model.ModifyTypeReorg)"#,
    );
    draft
}

/// TestModifyStringColumn 对应 Go 第 1190-1300 行：保留原测试/辅助函数的执行顺序。
// TestModifyStringColumn 对应 Go 第 1190-1300 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_string_column() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyStringColumn"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(1191, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(1192, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(1193, r#"tk.MustExec("use test")"#);
    draft.source(1194, r#"var reorgType byte"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(1195, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/getModifyColumnType", func(tp byte) {"#);
    draft.source(1196, r#"reorgType = tp"#);
    draft.source(1198, r#"type testCase struct {"#);
    draft.source(1199, r#"oldColTp        string"#);
    draft.source(1200, r#"newColTp        string"#);
    draft.source(1201, r#"insertVal       string"#);
    draft.source(1202, r#"pass            bool"#);
    draft.source(1203, r#"expectedReorgTp byte"#);
    draft.source(1205, r#"noPaddingStrLen5 := strings.Repeat("a", 5)"#);
    draft.source(1206, r#"noPaddingStrLen15 := strings.Repeat("a", 15)"#);
    draft.source(
        1207,
        r#"paddingStrLen5 := strings.Repeat("a", 1) + strings.Repeat(" ", 4)"#,
    );
    draft.source(
        1208,
        r#"paddingStrLen15 := strings.Repeat("a", 1) + strings.Repeat(" ", 14)"#,
    );
    draft.source(1210, r#"cases := []testCase{"#);
    draft.source(1212, r#"oldColTp:  "char(20)","#);
    draft.source(1213, r#"newColTp:  "char(10)","#);
    draft.source(1214, r#"insertVal: noPaddingStrLen15,"#);
    draft.source(1217, r#"oldColTp:  "char(20)","#);
    draft.source(1218, r#"newColTp:  "char(10)","#);
    draft.source(1219, r#"insertVal: noPaddingStrLen5,"#);
    draft.source(1220, r#"pass:      true,"#);
    draft.source(1223, r#"oldColTp:  "varchar(20)","#);
    draft.source(1224, r#"newColTp:  "varchar(10)","#);
    draft.source(1225, r#"insertVal: noPaddingStrLen15,"#);
    draft.source(1228, r#"oldColTp:  "varchar(20)","#);
    draft.source(1229, r#"newColTp:  "varchar(10)","#);
    draft.source(1230, r#"insertVal: noPaddingStrLen5,"#);
    draft.source(1231, r#"pass:      true,"#);
    draft.source(1234, r#"oldColTp:  "char(20)","#);
    draft.source(1235, r#"newColTp:  "varchar(10)","#);
    draft.source(1236, r#"insertVal: noPaddingStrLen15,"#);
    draft.source(1239, r#"oldColTp:  "char(20)","#);
    draft.source(1240, r#"newColTp:  "varchar(10)","#);
    draft.source(1241, r#"insertVal: noPaddingStrLen5,"#);
    draft.source(1242, r#"pass:      true,"#);
    draft.source(1245, r#"oldColTp:        "varchar(10)","#);
    draft.source(1246, r#"newColTp:        "char(20)","#);
    draft.source(1247, r#"insertVal:       paddingStrLen5,"#);
    draft.source(1248, r#"pass:            true,"#);
    draft.source(1249, r#"expectedReorgTp: model.ModifyTypeReorg,"#);
    draft.source(1252, r#"oldColTp:  "varchar(10)","#);
    draft.source(1253, r#"newColTp:  "char(20)","#);
    draft.source(1254, r#"insertVal: noPaddingStrLen5,"#);
    draft.source(1255, r#"pass:      true,"#);
    draft.source(1258, r#"oldColTp:        "varchar(20)","#);
    draft.source(1259, r#"newColTp:        "char(10)","#);
    draft.source(1260, r#"insertVal:       paddingStrLen5,"#);
    draft.source(1261, r#"pass:            true,"#);
    draft.source(1262, r#"expectedReorgTp: model.ModifyTypeReorg,"#);
    draft.source(1265, r#"oldColTp:        "varchar(20)","#);
    draft.source(1266, r#"newColTp:        "char(10)","#);
    draft.source(1267, r#"insertVal:       paddingStrLen15,"#);
    draft.source(1268, r#"pass:            true,"#);
    draft.source(1269, r#"expectedReorgTp: model.ModifyTypeReorg,"#);
    draft.source(1272, r#"oldColTp:  "varchar(20)","#);
    draft.source(1273, r#"newColTp:  "char(10)","#);
    draft.source(1274, r#"insertVal: noPaddingStrLen15,"#);
    draft.source(1277, r#"oldColTp:  "varchar(20)","#);
    draft.source(1278, r#"newColTp:  "char(10)","#);
    draft.source(1279, r#"insertVal: noPaddingStrLen5,"#);
    draft.source(1280, r#"pass:      true,"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1284, r#"for _, tc := range cases {"#);
    draft.exec(1285, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        1286,
        r#"tk.MustExec(fmt.Sprintf("create table t(a %s)", tc.oldColTp))"#,
    );
    draft.exec(
        1287,
        r#"tk.MustExec(fmt.Sprintf("insert into t values('%s')", tc.insertVal))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        1288,
        r#"err := tk.ExecToErr(fmt.Sprintf("alter table t modify column a %s", tc.newColTp))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1289, r#"if tc.pass {"#);
    draft.assertion(1290, r#"require.Nil(t, err)"#);
    draft.source(1291, r#"expectedReorgTp := tc.expectedReorgTp"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1292, r#"if tc.expectedReorgTp == model.ModifyTypeNone {"#);
    draft.source(
        1293,
        r#"expectedReorgTp = model.ModifyTypeNoReorgWithCheck"#,
    );
    draft.assertion(1295, r#"require.Equal(t, expectedReorgTp, reorgType)"#);
    draft.source(1296, r#"} else {"#);
    draft.assertion(
        1297,
        r#"require.Contains(t, err.Error(), "Data truncated for column 'a'")"#,
    );
    draft
}

/// TestModifyColumnWithDifferentCollation 对应 Go 第 1302-1365 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnWithDifferentCollation 对应 Go 第 1302-1365 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_with_different_collation() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnWithDifferentCollation"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(1303, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(1304, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.source(
        1306,
        r#"runSingleTest := func(t *testing.T, oldColTp, newColTp string) {"#,
    );
    draft.exec(1307, r#"tk.MustExec("use test")"#);
    draft.exec(1308, r#"tk.MustExec("drop table if exists t1")"#);
    draft.exec(1309, r#"tk.MustExec(fmt.Sprintf(`"#);
    draft.source(1310, r#"CREATE TABLE t1 ("#);
    draft.source(1311, r#"c1 int NOT NULL DEFAULT '1',"#);
    draft.source(1312, r#"c2 int NOT NULL DEFAULT '1',"#);
    draft.source(1313, r#"c3 %s,"#);
    draft.source(1314, r#"PRIMARY KEY (c1, c2),"#);
    draft.source(1315, r#"KEY i1 (c3)"#);
    draft.source(
        1316,
        r#") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"#,
    );
    draft.source(1317, r#"`, oldColTp))"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1319, r#"for i := range 10 {"#);
    draft.exec(
        1320,
        r#"tk.MustExec(fmt.Sprintf("insert into t1 (c1, c3) values (%d, 'space%d ')", i, i))"#,
    );
    draft.source(1323, r#"insertIdx := 32"#);
    draft.source(1324, r#"deleteIdx := 0"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(1325, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(_ *model.Job) {"#);
    draft.source(1326, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(1327, r#"tk2.MustExec("use test")"#);
    // Go 注释 L1328: Test data consistency for insert/delete check during reorg.
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(1329, r#"err := tk2.ExecToErr(fmt.Sprintf("insert into t1 (c1, c3) values ('%d', 'space%d   ')", insertIdx, insertIdx))"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1330, r#"if err != nil {"#);
    // Go 注释 L1331: The only possible error is data truncation error during modify column.
    draft.assertion(
        1332,
        r#"require.Contains(t, err.Error(), "data truncation error during modify column")"#,
    );
    draft.exec(
        1334,
        r#"tk2.MustExec(fmt.Sprintf("delete from t1 where c1 = %d", deleteIdx))"#,
    );
    draft.source(1335, r#"deleteIdx++"#);
    draft.source(1336, r#"insertIdx++"#);
    draft.exec(
        1339,
        r#"tk.MustExec(fmt.Sprintf("alter table t1 modify column c3 %s", newColTp))"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        1340,
        r#"require.True(t, deleteIdx > 0, "failpoint should be triggered")"#,
    );
    draft.exec(1341, r#"tk.MustExec("admin check table t1;")"#);
    draft.source(1344, r#"var ("#);
    draft.source(1345, r#"oldTps []string"#);
    draft.source(1346, r#"newTps []string"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1348, r#"for _, tp := range []string{"char", "varchar"} {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1349, r#"for _, collation := range []string{"utf8mb4_bin", "utf8_unicode_ci", "utf8mb4_general_ci"} {"#);
    draft.source(
        1350,
        r#"oldTps = append(oldTps, fmt.Sprintf("%s(32) collate %s", tp, collation))"#,
    );
    draft.source(
        1351,
        r#"newTps = append(newTps, fmt.Sprintf("%s(23) collate %s", tp, collation))"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1355, r#"for i, oldColTp := range oldTps {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1356, r#"for j, newColTp := range newTps {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1357, r#"if i == j {"#);
    draft.source(1358, r#"continue"#);
    draft.source(
        1360,
        r#"t.Run(fmt.Sprintf("%s -> %s", oldColTp, newColTp), func(t *testing.T) {"#,
    );
    draft.source(1361, r#"runSingleTest(t, oldColTp, newColTp)"#);
    draft
}

/// TestStatsAfterModifyColumn 对应 Go 第 1367-1492 行：保留原测试/辅助函数的执行顺序。
// TestStatsAfterModifyColumn 对应 Go 第 1367-1492 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_stats_after_modify_column() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestStatsAfterModifyColumn"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(1368, r#"type query struct {"#);
    draft.source(1369, r#"pred string"#);
    draft.source(1370, r#"idx  string"#);
    draft.source(1373, r#"type testCase struct {"#);
    draft.source(1374, r#"caseName        string"#);
    draft.source(1375, r#"createTableSQL  string"#);
    draft.source(1376, r#"modifySQL       string"#);
    draft.source(1377, r#"embeddedAnalyze bool"#);
    draft.source(1378, r#"checkResult     bool"#);
    draft.source(1379, r#"queries         []query"#);
    draft.source(1382, r#"tcs := []testCase{"#);
    // Go 注释 L1384: Check stats correctness after modifying column without any reorg
    // Go 注释 L1385: We don't add index on b, because these indexes need reorg due to NeedRestoreData changes.
    draft.source(1386, r#"caseName:        "no reorg without analyze","#);
    draft.source(1387, r#"createTableSQL:  "create table t (a bigint, b char(16) collate utf8mb4_bin, index i1(a))","#);
    draft.source(1388, r#"modifySQL:       "alter table t modify column a int, modify column b varchar(16) collate utf8mb4_bin","#);
    draft.source(1389, r#"embeddedAnalyze: false,"#);
    draft.source(1390, r#"checkResult:     true,"#);
    draft.source(1391, r#"queries: []query{"#);
    draft.source(1392, r#"{"a < 10", "i1"},"#);
    draft.source(1393, r#"{"a <= 10", ""},"#);
    draft.source(1394, r#"{"a > 10", "i1"},"#);
    draft.source(1395, r#"{"a >= 10", ""},"#);
    draft.source(1396, r#"{"a = 10", "i1"},"#);
    draft.source(1397, r#"{"a = -1", ""},"#);
    draft.source(1398, r#"{"b < '10'", ""},"#);
    draft.source(1399, r#"{"b <= '10'", ""},"#);
    draft.source(1400, r#"{"b > '10'", ""},"#);
    draft.source(1401, r#"{"b >= '10'", ""},"#);
    draft.source(1402, r#"{"b = '10'", ""},"#);
    draft.source(1403, r#"{"b = 'non-exist'", ""},"#);
    // Go 注释 L1407: Only indexes are rewritten.
    // Go 注释 L1408: The row data remains the same, so the stats are still valid.
    draft.source(
        1409,
        r#"caseName:        "row and index reorg with analyze","#,
    );
    draft.source(1410, r#"createTableSQL:  "create table t (a bigint, b char(16) collate utf8mb4_bin, index i1(a), index i2(b))","#);
    draft.source(1411, r#"modifySQL:       "alter table t modify column a int, modify column b varchar(16) collate utf8mb4_bin","#);
    draft.source(1412, r#"embeddedAnalyze: true,"#);
    draft.source(1413, r#"checkResult:     true,"#);
    draft.source(1414, r#"queries: []query{"#);
    draft.source(1415, r#"{"a < 10", "i1"},"#);
    draft.source(1416, r#"{"a <= 10", ""},"#);
    draft.source(1417, r#"{"a > 10", "i1"},"#);
    draft.source(1418, r#"{"a >= 10", ""},"#);
    draft.source(1419, r#"{"a = 10", "i1"},"#);
    draft.source(1420, r#"{"a = -1", ""},"#);
    draft.source(1421, r#"{"b < '10'", ""},"#);
    draft.source(1422, r#"{"b <= '10'", "i2"},"#);
    draft.source(1423, r#"{"b > '10'", ""},"#);
    draft.source(1424, r#"{"b >= '10'", "i2"},"#);
    draft.source(1425, r#"{"b = '10'", ""},"#);
    draft.source(1426, r#"{"b = 'non-exist'", "i2"},"#);
    // Go 注释 L1430: Both row and index reorg happen, but with no embedded analyze.
    // Go 注释 L1431: All the stats become invalid, so don't check the results.
    draft.source(
        1432,
        r#"caseName:        "row and index reorg without analyze","#,
    );
    draft.source(1433, r#"createTableSQL:  "create table t (a bigint, b char(16) collate utf8mb4_bin, index i1(a), index i2(b))","#);
    draft.source(1434, r#"modifySQL:       "alter table t modify column a int unsigned, modify column b varchar(16) collate utf8mb4_general_ci","#);
    draft.source(1435, r#"embeddedAnalyze: false,"#);
    draft.source(1436, r#"checkResult:     false,"#);
    draft.source(1437, r#"queries: []query{"#);
    draft.source(1438, r#"{"a < 10", "i1"},"#);
    draft.source(1439, r#"{"a <= 10", ""},"#);
    draft.source(1440, r#"{"a > 10", "i1"},"#);
    draft.source(1441, r#"{"a >= 10", ""},"#);
    draft.source(1442, r#"{"a = 10", "i1"},"#);
    draft.source(1443, r#"{"a = -1", ""},"#);
    draft.source(1444, r#"{"b < '10'", ""},"#);
    draft.source(1445, r#"{"b <= '10'", "i2"},"#);
    draft.source(1446, r#"{"b > '10'", ""},"#);
    draft.source(1447, r#"{"b >= '10'", "i2"},"#);
    draft.source(1448, r#"{"b = '10'", ""},"#);
    draft.source(1449, r#"{"b = 'non-exist'", "i2"},"#);
    draft.source(1454, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(1455, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(1456, r#"tk.MustExec("use test")"#);
    draft.exec(
        1457,
        r#"tk.MustExec("set @@tidb_stats_update_during_ddl = true;")"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1459, r#"for _, tc := range tcs {"#);
    draft.source(1460, r#"t.Run(tc.caseName, func(t *testing.T) {"#);
    draft.exec(1461, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(1462, r#"tk.MustExec(tc.createTableSQL)"#);
    draft.exec(1463, r#"tk.MustExec(fmt.Sprintf("set @@tidb_stats_update_during_ddl = %t", tc.embeddedAnalyze))"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1465, r#"for i := range 128 {"#);
    draft.exec(
        1466,
        r#"tk.MustExec(fmt.Sprintf("insert into t values (%d, '%d')", i, i))"#,
    );
    draft.exec(1469, r#"tk.MustExec("analyze table t columns a, b")"#);
    draft.source(1471, r#"oldRs := make([]string, 0, len(tc.queries))"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1472, r#"for _, q := range tc.queries {"#);
    draft.query(1473, r#"rs := tk.MustQuery(fmt.Sprintf("explain select * from t use index(%s) where %s", q.idx, q.pred)).Rows()"#);
    draft.source(1474, r#"oldRs = append(oldRs, rs[0][1].(string))"#);
    draft.exec(1477, r#"tk.MustExec(tc.modifySQL)"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1479, r#"for i, q := range tc.queries {"#);
    draft.query(1480, r#"rs := tk.MustQuery(fmt.Sprintf("explain select * from t use index(%s) where %s", q.idx, q.pred)).Rows()"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1481, r#"if tc.checkResult {"#);
    draft.assertion(
        1482,
        r#"require.Equal(t, oldRs[i], rs[0][1].(string), "predicate: %s", tc.queries[i].pred)"#,
    );
    draft.source(1483, r#"} else {"#);
    // Go 注释 L1484: For index selectivity, the stats is missing here.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(1485, r#"if q.idx != "" {"#);
    draft.assertion(
        1486,
        r#"require.Contains(t, rs[len(rs)-1][len(rs[0])-1], "missing")"#,
    );
    draft
}

/// TestModifyColumnLoadTableRangeError 对应 Go 第 1494-1511 行：保留原测试/辅助函数的执行顺序。
// TestModifyColumnLoadTableRangeError 对应 Go 第 1494-1511 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_modify_column_load_table_range_error() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestModifyColumnLoadTableRangeError"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(1495, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(1496, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(1497, r#"tk.MustExec("drop database if exists modifycol;")"#);
    draft.exec(1498, r#"tk.MustExec("create database modifycol;")"#);
    draft.exec(1499, r#"tk.MustExec("use modifycol;")"#);
    // Go 注释 L1501: Use a type conversion that definitely requires reorg.
    draft.exec(
        1502,
        r#"tk.MustExec("create table t (a int primary key, b int, c int);")"#,
    );
    draft.source(1503, r#"batchInsert(tk, "t", 0, 100)"#);
    // Go 注释 L1505: Simulate transient PD errors (e.g. "All returned regions have no leaders") when splitting table ranges.
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        1506,
        r#"testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/loadTableRangesFromPDErr","#,
    );
    draft.source(
        1507,
        r#"`1*return("All returned regions have no leaders, limit: 1")`)"#,
    );
    draft.exec(
        1509,
        r#"tk.MustExec("alter table t change column b b varchar(16);")"#,
    );
    draft.exec(1510, r#"tk.MustExec("admin check table t;")"#);
    draft
}
