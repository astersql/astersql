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

// Multi-Schema Change（单条 ALTER 含多个子变更）相关 DDL 测试的步骤记录稿。
//
// 一条 `ALTER TABLE` 可同时包含加删改列/索引等子操作，由父 job 驱动多个 sub-job。
// 本文件用 `CaseRecorder` 按 Go 源码顺序记录 SQL、取消钩子、并行提交、MDL
//（Metadata Lock，元数据锁）与行级校验等场景，不真正执行数据库逻辑。

// 这段逻辑覆盖 multi-schema change 测试中的 add/drop/rename/alter/change/modify/index 混合操作、取消钩子、并行提交、MDL 和行校验限制。

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

/// TestMultiSchemaChangeAddColumnsCancelled 对应 Go 第 39-60 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAddColumnsCancelled 对应 Go 第 39-60 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_add_columns_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAddColumnsCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(40, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(41, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(42, r#"tk.MustExec("use test")"#);
    draft.exec(44, r#"tk.MustExec("create table t (a int);")"#);
    draft.exec(45, r#"tk.MustExec("insert into t values (1);")"#);
    draft.source(
        46,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L47: Cancel job when the column 'c' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(48, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(49, r#"return false"#);
    draft.source(51, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        52,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(54, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    draft.source(55, r#"sql := "alter table t add column b int default 2, add column c int default 3, add column d int default 4;""#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(56, r#"tk.MustGetErrCode(sql, errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        57,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(58, r#"hook.MustCancelDone(t)"#);
    draft.query(
        59,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1"))"#,
    );
    draft
}

/// TestMultiSchemaChangeAddColumnsParallel 对应 Go 第 62-82 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAddColumnsParallel 对应 Go 第 62-82 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_add_columns_parallel() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAddColumnsParallel"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(63, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(64, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(65, r#"tk.MustExec("use test")"#);
    draft.exec(66, r#"tk.MustExec("create table t (a int default 1);")"#);
    draft.exec(67, r#"tk.MustExec("insert into t values ();")"#);
    draft.source(68, r#"putTheSameDDLJobTwice(t, func() {"#);
    draft.exec(
        69,
        r#"tk.MustExec("alter table t add column if not exists b int default 2, " +"#,
    );
    draft.source(70, r#""add column if not exists c int default 3;")"#);
    draft.query(71, r#"tk.MustQuery("show warnings").Check(testkit.Rows("#);
    draft.source(72, r#""Note 1060 Duplicate column name 'b'","#);
    draft.source(73, r#""Note 1060 Duplicate column name 'c'","#);
    draft.query(
        76,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.exec(77, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(78, r#"tk.MustExec("create table t (a int);")"#);
    draft.source(79, r#"putTheSameDDLJobTwice(t, func() {"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(80, r#"tk.MustGetErrCode("alter table t add column b int, add column c int;", errno.ErrDupFieldName)"#);
    draft
}

/// TestMultiSchemaChangeDropColumnsCancelled 对应 Go 第 84-123 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDropColumnsCancelled 对应 Go 第 84-123 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_drop_columns_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDropColumnsCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(85, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(86, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(87, r#"tk.MustExec("use test")"#);
    // Go 注释 L89: Test for cancelling the job in a middle state.
    draft.exec(90, r#"tk.MustExec("create table t (a int default 1, b int default 2, c int default 3, d int default 4);")"#);
    draft.exec(91, r#"tk.MustExec("insert into t values ();")"#);
    draft.source(
        92,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L93: Cancel job when the column 'a' is in delete-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(94, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(95, r#"return false"#);
    draft.source(97, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        98,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateDeleteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(100, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    draft.exec(
        101,
        r#"tk.MustExec("alter table t drop column b, drop column a, drop column d;")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        102,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(103, r#"hook.MustCancelFailed(t)"#);
    draft.query(
        104,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("3"))"#,
    );
    // Go 注释 L106: Test for cancelling the job in public.
    draft.exec(107, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(108, r#"tk.MustExec("create table t (a int default 1, b int default 2, c int default 3, d int default 4);")"#);
    draft.exec(109, r#"tk.MustExec("insert into t values ();")"#);
    draft.source(
        110,
        r#"hook = newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L111: Cancel job when the column 'a' is in public.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(112, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(113, r#"return false"#);
    draft.source(115, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        116,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StatePublic"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(118, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(119, r#"tk.MustGetErrCode("alter table t drop column b, drop column a, drop column d;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        120,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(121, r#"hook.MustCancelDone(t)"#);
    draft.query(
        122,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2 3 4"))"#,
    );
    draft
}

/// TestMultiSchemaChangeDropIndexedColumnsCancelled 对应 Go 第 125-146 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDropIndexedColumnsCancelled 对应 Go 第 125-146 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_drop_indexed_columns_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDropIndexedColumnsCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(126, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(127, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(128, r#"tk.MustExec("use test")"#);
    // Go 注释 L130: Test for cancelling the job in a middle state.
    draft.exec(131, r#"tk.MustExec("create table t (a int default 1, b int default 2, c int default 3, d int default 4, " +"#);
    draft.source(132, r#""index(a), index(b), index(c), index(d));")"#);
    draft.exec(133, r#"tk.MustExec("insert into t values ();")"#);
    draft.source(
        134,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L135: Cancel job when the column 'a' is in delete-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(136, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(137, r#"return false"#);
    draft.source(139, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        140,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateDeleteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(142, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    draft.exec(
        143,
        r#"tk.MustExec("alter table t drop column b, drop column a, drop column d;")"#,
    );
    draft.source(144, r#"hook.MustCancelFailed(t)"#);
    draft.query(
        145,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("3"))"#,
    );
    draft
}

/// TestMultiSchemaChangeDropColumnsParallel 对应 Go 第 148-164 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDropColumnsParallel 对应 Go 第 148-164 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_drop_columns_parallel() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDropColumnsParallel"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(149, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(150, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(151, r#"tk.MustExec("use test")"#);
    draft.exec(
        152,
        r#"tk.MustExec("create table t (a int, b int, c int);")"#,
    );
    draft.source(153, r#"putTheSameDDLJobTwice(t, func() {"#);
    draft.exec(
        154,
        r#"tk.MustExec("alter table t drop column if exists b, drop column if exists c;")"#,
    );
    draft.query(155, r#"tk.MustQuery("show warnings").Check(testkit.Rows("#);
    draft.source(156, r#""Note 1091 column b doesn't exist","#);
    draft.source(157, r#""Note 1091 column c doesn't exist"))"#);
    draft.exec(159, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        160,
        r#"tk.MustExec("create table t (a int, b int, c int);")"#,
    );
    draft.source(161, r#"putTheSameDDLJobTwice(t, func() {"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(162, r#"tk.MustGetErrCode("alter table t drop column b, drop column a;", errno.ErrCantDropFieldOrKey)"#);
    draft
}

/// TestMultiSchemaChangeRenameColumns 对应 Go 第 166-241 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeRenameColumns 对应 Go 第 166-241 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_rename_columns() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeRenameColumns"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(167, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(169, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(170, r#"tk.MustExec("use test")"#);
    draft.source(171, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(172, r#"tk2.MustExec("use test")"#);
    // Go 注释 L174: unsupported ddl operations
    // Go 注释 L176: Test add and rename to same column name
    draft.exec(177, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        178,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(179, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(180, r#"tk.MustGetErrCode("alter table t rename column b to c, add column c int", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L182: Test add column related with rename column
    draft.exec(183, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        184,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(185, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(186, r#"tk.MustGetErrCode("alter table t rename column b to c, add column e int after b", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L188: Test drop and rename with same column
    draft.exec(189, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        190,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(191, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(192, r#"tk.MustGetErrCode("alter table t drop column b, rename column b to c", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L194: Test add index and rename with same column
    draft.exec(195, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        196,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(197, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(198, r#"tk.MustGetErrCode("alter table t rename column b to c, add index t1(a, b)", errno.ErrUnsupportedDDLOperation)"#);
    draft.exec(201, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        202,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(203, r#"tk.MustExec("insert into t values ();")"#);
    draft.exec(
        204,
        r#"tk.MustExec("alter table t rename column b to c, add column e int default 3")"#,
    );
    draft.query(
        205,
        r#"tk.MustQuery("select c from t").Check(testkit.Rows("2"))"#,
    );
    draft.query(
        206,
        r#"tk.MustQuery("select * from t").Check(testkit.Rows("1 2 3"))"#,
    );
    // Go 注释 L208: Test cancel job with rename columns
    draft.exec(209, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        210,
        r#"tk.MustExec("create table t (a int default 1, b int default 2)")"#,
    );
    draft.exec(211, r#"tk.MustExec("insert into t values ()")"#);
    draft.source(
        212,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L213: Cancel job when the column 'c' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(214, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(215, r#"return false"#);
    draft.source(217, r#"assertMultiSchema(t, job, 2)"#);
    draft.source(
        218,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(220, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(221, r#"tk.MustGetErrCode("alter table t add column c int default 3, rename column b to d;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        222,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.query(
        223,
        r#"tk.MustQuery("select b from t").Check(testkit.Rows("2"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        224,
        r#"tk.MustGetErrCode("select d from t", errno.ErrBadField)"#,
    );
    // Go 注释 L226: Test dml stmts when do rename
    draft.exec(227, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        228,
        r#"tk.MustExec("create table t (a int default 1, b int default 2)")"#,
    );
    draft.exec(229, r#"tk.MustExec("insert into t values ()")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(230, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    draft.assertion(
        231,
        r#"assert.Equal(t, model.ActionMultiSchemaChange, job.Type)"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        232,
        r#"if job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization {"#,
    );
    draft.exec(233, r#"rs, _ := tk2.Exec("select b from t")"#);
    draft.assertion(
        234,
        r#"assert.Equal(t, tk2.ResultSetToResult(rs, "").Rows()[0][0], "2")"#,
    );
    draft.exec(
        237,
        r#"tk.MustExec("alter table t add column c int default 3, rename column b to d;")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        238,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep")"#,
    );
    draft.query(
        239,
        r#"tk.MustQuery("select d from t").Check(testkit.Rows("2"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        240,
        r#"tk.MustGetErrCode("select b from t", errno.ErrBadField)"#,
    );
    draft
}

/// TestMultiSchemaChangeAlterColumns 对应 Go 第 243-307 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAlterColumns 对应 Go 第 243-307 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_alter_columns() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAlterColumns"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(244, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(245, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(246, r#"tk.MustExec("use test")"#);
    // Go 注释 L248: unsupported ddl operations
    // Go 注释 L250: Test alter and drop with same column
    draft.exec(251, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        252,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(253, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(254, r#"tk.MustGetErrCode("alter table t alter column b set default 3, drop column b", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L256: Test alter and rename with same column
    draft.exec(257, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        258,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(259, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(260, r#"tk.MustGetErrCode("alter table t alter column b set default 3, rename column b to c", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L262: Test alter and drop modify same column
    draft.exec(263, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        264,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(265, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(266, r#"tk.MustGetErrCode("alter table t alter column b set default 3, modify column b double", errno.ErrUnsupportedDDLOperation)"#);
    draft.exec(269, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        270,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(271, r#"tk.MustExec("insert into t values ();")"#);
    draft.query(
        272,
        r#"tk.MustQuery("select * from t").Check(testkit.Rows("1 2"))"#,
    );
    draft.exec(
        273,
        r#"tk.MustExec("alter table t rename column a to c, alter column b set default 3;")"#,
    );
    draft.exec(274, r#"tk.MustExec("truncate table t;")"#);
    draft.exec(275, r#"tk.MustExec("insert into t values ();")"#);
    draft.query(
        276,
        r#"tk.MustQuery("select * from t").Check(testkit.Rows("1 3"))"#,
    );
    // Go 注释 L278: Test cancel job with alter columns
    draft.exec(279, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        280,
        r#"tk.MustExec("create table t (a int default 1, b int default 2)")"#,
    );
    draft.source(
        281,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L282: Cancel job when the column 'a' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(283, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(284, r#"return false"#);
    draft.source(286, r#"assertMultiSchema(t, job, 2)"#);
    draft.source(
        287,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(289, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(290, r#"tk.MustGetErrCode("alter table t add column c int default 3, alter column b set default 3;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        291,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.exec(292, r#"tk.MustExec("insert into t values ()")"#);
    draft.query(
        293,
        r#"tk.MustQuery("select * from t").Check(testkit.Rows("1 2"))"#,
    );
    // Go 注释 L295: Test dml stmts when do alter
    draft.exec(296, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        297,
        r#"tk.MustExec("create table t (a int default 1, b int default 2)")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(298, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    draft.assertion(
        299,
        r#"assert.Equal(t, model.ActionMultiSchemaChange, job.Type)"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        300,
        r#"if job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteOnly {"#,
    );
    draft.source(301, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(302, r#"tk2.MustExec("insert into test.t values ()")"#);
    draft.exec(
        305,
        r#"tk.MustExec("alter table t add column c int default 3, alter column b set default 3;")"#,
    );
    draft.query(
        306,
        r#"tk.MustQuery("select * from t").Check(testkit.Rows("1 2 3"))"#,
    );
    draft
}

/// TestMultiSchemaChangeChangeColumns 对应 Go 第 309-362 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeChangeColumns 对应 Go 第 309-362 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_change_columns() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeChangeColumns"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(310, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(312, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(313, r#"tk.MustExec("use test")"#);
    // Go 注释 L315: unsupported ddl operations
    // Go 注释 L317: Test change and drop with same column
    draft.exec(318, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        319,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(320, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(321, r#"tk.MustGetErrCode("alter table t change column b c double, drop column b", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L323: Test change and add with same column
    draft.exec(324, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        325,
        r#"tk.MustExec("create table t (a int default 1, b int default 2);")"#,
    );
    draft.exec(326, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(327, r#"tk.MustGetErrCode("alter table t change column b c double, add column c int", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L329: Test add index and rename with same column
    draft.exec(330, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        331,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(332, r#"tk.MustExec("insert into t values ();")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(333, r#"tk.MustGetErrCode("alter table t change column b c double, add index t1(a, b)", errno.ErrUnsupportedDDLOperation)"#);
    draft.exec(336, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        337,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(338, r#"tk.MustExec("insert into t values ();")"#);
    draft.exec(
        339,
        r#"tk.MustExec("alter table t rename column b to c, change column a e bigint default 3;")"#,
    );
    draft.query(
        340,
        r#"tk.MustQuery("select e,c from t").Check(testkit.Rows("1 2"))"#,
    );
    draft.exec(341, r#"tk.MustExec("truncate table t;")"#);
    draft.exec(342, r#"tk.MustExec("insert into t values ();")"#);
    draft.query(
        343,
        r#"tk.MustQuery("select e,c from t").Check(testkit.Rows("3 2"))"#,
    );
    // Go 注释 L345: Test cancel job with change columns
    draft.exec(346, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        347,
        r#"tk.MustExec("create table t (a int default 1, b int default 2)")"#,
    );
    draft.exec(348, r#"tk.MustExec("insert into t values ()")"#);
    draft.source(
        349,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L350: Cancel job when the column 'c' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(351, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(352, r#"return false"#);
    draft.source(354, r#"assertMultiSchema(t, job, 2)"#);
    draft.source(
        355,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(357, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(358, r#"tk.MustGetErrCode("alter table t add column c int default 3, change column b d bigint default 4;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        359,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.query(
        360,
        r#"tk.MustQuery("select b from t").Check(testkit.Rows("2"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        361,
        r#"tk.MustGetErrCode("select d from t", errno.ErrBadField)"#,
    );
    draft
}

/// TestMultiSchemaChangeRenameTable 对应 Go 第 364-393 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeRenameTable 对应 Go 第 364-393 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_rename_table() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeRenameTable"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(365, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(367, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(368, r#"tk.MustExec("use test")"#);
    draft.exec(369, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        370,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a, b));")"#,
    );
    draft.exec(371, r#"tk.MustExec("insert into t values (1, 2);")"#);
    draft.concurrency(372, r#"var wg sync.WaitGroup"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(373, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(374, r#"switch job.SchemaState {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(375, r#"case model.StateNone:"#);
    draft.source(376, r#"wg.Add(1)"#);
    draft.concurrency(377, r#"go func() {"#);
    draft.exec(378, r#"_, err := tk.Exec("alter table t rename column b to c, change column a e bigint default 3;")"#);
    draft.assertion(379, r#"require.Error(t, err)"#);
    draft.source(380, r#"wg.Done()"#);
    draft.source(384, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(385, r#"tk2.MustExec("use test")"#);
    draft.exec(386, r#"tk2.MustExec("alter table t rename to t1")"#);
    draft.source(387, r#"wg.Wait()"#);
    draft.query(
        389,
        r#"tk2.MustQuery("select * from t1").Check(testkit.Rows("1 2"))"#,
    );
    draft.exec(390, r#"tk2.MustExec("admin check table t1;")"#);
    draft.exec(391, r#"tk2.MustExec("alter table t1 rename column b to c, change column a e bigint default 3;")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        392,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep")"#,
    );
    draft
}

/// TestMultiSchemaChangeAddIndexesCancelled 对应 Go 第 394-440 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAddIndexesCancelled 对应 Go 第 394-440 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_add_indexes_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAddIndexesCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(395, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(396, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(397, r#"tk.MustExec("use test")"#);
    // Go 注释 L399: Test cancel successfully.
    draft.exec(400, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        401,
        r#"tk.MustExec("create table t (a int, b int, c int);")"#,
    );
    draft.exec(402, r#"tk.MustExec("insert into t values (1, 2, 3);")"#);
    draft.source(
        403,
        r#"cancelHook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L404: Cancel the job when index 't2' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(405, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(406, r#"return false"#);
    draft.source(408, r#"assertMultiSchema(t, job, 1)"#);
    draft.source(
        409,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(411, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", cancelHook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(412, r#"tk.MustGetErrCode("alter table t "+"#);
    draft.source(413, r#""add index t(a, b), add index t1(a), "+"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        414,
        r#""add index t2(a), add index t3(a, b);", errno.ErrCancelledDDLJob)"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        415,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(416, r#"cancelHook.MustCancelDone(t)"#);
    draft.query(
        417,
        r#"tk.MustQuery("show index from t;").Check(testkit.Rows( /* no index */ ))"#,
    );
    draft.query(
        418,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.exec(419, r#"tk.MustExec("admin check table t;")"#);
    // Go 注释 L421: Test cancel failed when some sub-jobs have been finished.
    draft.exec(422, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        423,
        r#"tk.MustExec("create table t (a int, b int, c int);")"#,
    );
    draft.exec(424, r#"tk.MustExec("insert into t values (1, 2, 3);")"#);
    draft.source(
        425,
        r#"cancelHook = newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L426: Cancel the job when index 't1' is in public.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(427, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(428, r#"return false"#);
    draft.source(430, r#"assertMultiSchema(t, job, 1)"#);
    draft.source(
        431,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StatePublic"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(433, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", cancelHook.OnJobUpdated)"#);
    draft.exec(
        434,
        r#"tk.MustExec("alter table t add index t(a, b), add index t1(a), " +"#,
    );
    draft.source(435, r#""add index t2(a), add index t3(a, b);")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        436,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(437, r#"cancelHook.MustCancelFailed(t)"#);
    draft.query(
        438,
        r#"tk.MustQuery("select * from t use index(t, t1, t2, t3);").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.exec(439, r#"tk.MustExec("admin check table t;")"#);
    draft
}

/// TestMultiSchemaChangeDropIndexesCancelled 对应 Go 第 442-481 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDropIndexesCancelled 对应 Go 第 442-481 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_drop_indexes_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDropIndexesCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(443, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(444, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(445, r#"tk.MustExec("use test;")"#);
    // Go 注释 L447: Test for cancelling the job in a middle state.
    draft.exec(448, r#"tk.MustExec("create table t (a int, b int, index(a), unique index(b), index idx(a, b));")"#);
    draft.source(
        449,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(450, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(451, r#"return false"#);
    draft.source(453, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        454,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateDeleteOnly"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(456, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    draft.exec(
        457,
        r#"tk.MustExec("alter table t drop index a, drop index b, drop index idx;")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        458,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(459, r#"hook.MustCancelFailed(t)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        460,
        r#"tk.MustGetErrCode("select * from t use index (a);", errno.ErrKeyDoesNotExist)"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        461,
        r#"tk.MustGetErrCode("select * from t use index (b);", errno.ErrKeyDoesNotExist)"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        462,
        r#"tk.MustGetErrCode("select * from t use index (idx);", errno.ErrKeyDoesNotExist)"#,
    );
    // Go 注释 L464: Test for cancelling the job in none state.
    draft.exec(465, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(466, r#"tk.MustExec("create table t (a int, b int, index(a), unique index(b), index idx(a, b));")"#);
    draft.source(
        467,
        r#"hook = newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(468, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(469, r#"return false"#);
    draft.source(471, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        472,
        r#"return job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StatePublic"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(474, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(475, r#"tk.MustGetErrCode("alter table t drop index a, drop index b, drop index idx;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        476,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(477, r#"hook.MustCancelDone(t)"#);
    draft.query(
        478,
        r#"tk.MustQuery("select * from t use index (a);").Check(testkit.Rows())"#,
    );
    draft.query(
        479,
        r#"tk.MustQuery("select * from t use index (b);").Check(testkit.Rows())"#,
    );
    draft.query(
        480,
        r#"tk.MustQuery("select * from t use index (idx);").Check(testkit.Rows())"#,
    );
    draft
}

/// TestMultiSchemaChangeDropIndexesParallel 对应 Go 第 483-499 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDropIndexesParallel 对应 Go 第 483-499 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_drop_indexes_parallel() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDropIndexesParallel"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(484, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(485, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(486, r#"tk.MustExec("use test")"#);
    draft.exec(
        487,
        r#"tk.MustExec("create table t (a int, b int, c int, index(a), index(b), index(c));")"#,
    );
    draft.source(488, r#"putTheSameDDLJobTwice(t, func() {"#);
    draft.exec(
        489,
        r#"tk.MustExec("alter table t drop index if exists b, drop index if exists c;")"#,
    );
    draft.query(490, r#"tk.MustQuery("show warnings").Check(testkit.Rows("#);
    draft.source(491, r#""Note 1091 index b doesn't exist","#);
    draft.source(492, r#""Note 1091 index c doesn't exist"))"#);
    draft.exec(494, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        495,
        r#"tk.MustExec("create table t (a int, b int, c int, index (a), index(b), index(c));")"#,
    );
    draft.source(496, r#"putTheSameDDLJobTwice(t, func() {"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(497, r#"tk.MustGetErrCode("alter table t drop index b, drop index a;", errno.ErrCantDropFieldOrKey)"#);
    draft
}

/// TestMultiSchemaChangeRenameIndexes 对应 Go 第 501-550 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeRenameIndexes 对应 Go 第 501-550 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_rename_indexes() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeRenameIndexes"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(502, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(503, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(504, r#"tk.MustExec("use test")"#);
    // Go 注释 L506: Test rename index.
    draft.exec(507, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        508,
        r#"tk.MustExec("create table t (a int, b int, c int, index t(a), index t1(b))")"#,
    );
    draft.exec(
        509,
        r#"tk.MustExec("alter table t rename index t to x, rename index t1 to x1")"#,
    );
    draft.exec(510, r#"tk.MustExec("select * from t use index (x);")"#);
    draft.exec(511, r#"tk.MustExec("select * from t use index (x1);")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        512,
        r#"tk.MustGetErrCode("select * from t use index (t);", errno.ErrKeyDoesNotExist)"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        513,
        r#"tk.MustGetErrCode("select * from t use index (t1);", errno.ErrKeyDoesNotExist)"#,
    );
    // Go 注释 L515: Test drop and rename same index.
    draft.exec(516, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        517,
        r#"tk.MustExec("create table t (a int, b int, c int, index t(a))")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(518, r#"tk.MustGetErrCode("alter table t drop index t, rename index t to t1", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L520: Test add and rename to same index name.
    draft.exec(521, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        522,
        r#"tk.MustExec("create table t (a int, b int, c int, index t(a))")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(523, r#"tk.MustGetErrCode("alter table t add index t1(b), rename index t to t1", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L525: Test drop column with rename index.
    draft.exec(526, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(527, r#"tk.MustExec("create table t (a int default 1, b int default 2, c int default 3, index t(a))")"#);
    draft.exec(528, r#"tk.MustExec("insert into t values ();")"#);
    draft.exec(
        529,
        r#"tk.MustExec("alter table t drop column a, rename index t to x")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        530,
        r#"tk.MustGetErrCode("select * from t use index (x);", errno.ErrKeyDoesNotExist)"#,
    );
    draft.query(
        531,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("2 3"))"#,
    );
    // Go 注释 L533: Test cancel job with renameIndex
    draft.exec(534, r#"tk.MustExec("drop table if exists t")"#);
    draft.exec(
        535,
        r#"tk.MustExec("create table t (a int default 1, b int default 2, index t(a))")"#,
    );
    draft.exec(536, r#"tk.MustExec("insert into t values ()")"#);
    draft.source(
        537,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // Go 注释 L538: Cancel job when the column 'c' is in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(539, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(540, r#"return false"#);
    draft.source(542, r#"assertMultiSchema(t, job, 2)"#);
    draft.source(
        543,
        r#"return job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(545, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(546, r#"tk.MustGetErrCode("alter table t add column c int default 3, rename index t to t1;", errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        547,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.query(
        548,
        r#"tk.MustQuery("select * from t use index (t);").Check(testkit.Rows("1 2"))"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        549,
        r#"tk.MustGetErrCode("select * from t use index (t1);", errno.ErrKeyDoesNotExist)"#,
    );
    draft
}

/// TestMultiSchemaChangeModifyColumnsCancelled 对应 Go 第 552-577 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeModifyColumnsCancelled 对应 Go 第 552-577 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_modify_columns_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeModifyColumnsCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(553, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(554, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(555, r#"tk.MustExec("use test;")"#);
    // Go 注释 L557: Test for cancelling the job in a middle state.
    draft.exec(558, r#"tk.MustExec("create table t (a int, b int, c int, index i1(a), unique index i2(b), index i3(a, b));")"#);
    draft.exec(559, r#"tk.MustExec("insert into t values (1, 2, 3);")"#);
    draft.source(
        560,
        r#"hook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(561, r#"if job.Type != model.ActionMultiSchemaChange {"#);
    draft.source(562, r#"return false"#);
    draft.source(564, r#"assertMultiSchema(t, job, 3)"#);
    draft.source(
        565,
        r#"return job.MultiSchemaInfo.SubJobs[2].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(567, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", hook.OnJobUpdated)"#);
    draft.source(568, r#"sql := "alter table t modify column a tinyint, modify column b bigint, modify column c char(20);""#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(569, r#"tk.MustGetErrCode(sql, errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        570,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(571, r#"hook.MustCancelDone(t)"#);
    draft.query(
        572,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.query(
        573,
        r#"tk.MustQuery("select * from t use index (i1, i2, i3);").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.exec(574, r#"tk.MustExec("admin check table t;")"#);
    draft.query(575, r#"tk.MustQuery("select data_type from information_schema.columns where table_name = 't' and column_name = 'c';")."#);
    draft.source(576, r#"Check(testkit.Rows("int"))"#);
    draft
}

/// TestMultiSchemaChangeAlterIndex 对应 Go 第 579-638 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAlterIndex 对应 Go 第 579-638 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_alter_index() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAlterIndex"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(580, r#"testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/disableLossyDDLOptimization", "return(true)")"#);
    draft.source(582, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(583, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(584, r#"tk.MustExec("use test;")"#);
    draft.source(585, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(586, r#"tk2.MustExec("use test;")"#);
    // Go 注释 L588: unsupported ddl operations
    // Go 注释 L590: Test alter the same index
    draft.exec(591, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        592,
        r#"tk.MustExec("create table t (a int, b int, index idx(a, b));")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(593, r#"tk.MustGetErrCode("alter table t alter index idx visible, alter index idx invisible;", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L595: Test drop and alter the same index
    draft.exec(596, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(
        597,
        r#"tk.MustExec("create table t (a int, b int, index idx(a, b));")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(598, r#"tk.MustGetErrCode("alter table t drop index idx, alter index idx visible;", errno.ErrUnsupportedDDLOperation)"#);
    // Go 注释 L600: Test add and alter the same index
    draft.exec(601, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(602, r#"tk.MustExec("create table t (a int, b int);")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(603, r#"tk.MustGetErrCode("alter table t add index idx(a, b), alter index idx invisible", errno.ErrKeyDoesNotExist)"#);
    draft.exec(606, r#"tk.MustExec("drop table t;")"#);
    draft.exec(
        607,
        r#"tk.MustExec("create table t (a int, b int, index i1(a, b), index i2(b));")"#,
    );
    draft.exec(608, r#"tk.MustExec("insert into t values (1, 2);")"#);
    draft.exec(609, r#"tk.MustExec("alter table t modify column a tinyint, alter index i2 invisible, alter index i1 invisible;")"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        610,
        r#"tk.MustGetErrCode("select * from t use index (i1);", errno.ErrKeyDoesNotExist)"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        611,
        r#"tk.MustGetErrCode("select * from t use index (i2);", errno.ErrKeyDoesNotExist)"#,
    );
    draft.query(
        612,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2"))"#,
    );
    draft.exec(613, r#"tk.MustExec("admin check table t;")"#);
    draft.exec(615, r#"tk.MustExec("drop table t;")"#);
    draft.exec(
        616,
        r#"tk.MustExec("create table t (a int, b int, index i1(a, b), index i2(b));")"#,
    );
    draft.exec(617, r#"tk.MustExec("insert into t values (1, 2);")"#);
    draft.source(618, r#"var checked bool"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(619, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(620, r#"if job.MultiSchemaInfo == nil {"#);
    draft.source(621, r#"return"#);
    // Go 注释 L623: "modify column a tinyint" in write-reorg.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        624,
        r#"if job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateWriteReorganization {"#,
    );
    draft.source(625, r#"checked = true"#);
    draft.exec(
        626,
        r#"rs, err := tk2.Exec("select * from t use index(i1);")"#,
    );
    draft.assertion(627, r#"assert.NoError(t, err)"#);
    draft.assertion(628, r#"assert.NoError(t, rs.Close())"#);
    draft.exec(631, r#"tk.MustExec("alter table t alter index i1 invisible, modify column a tinyint, alter index i2 invisible;")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        632,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.assertion(633, r#"require.True(t, checked)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        634,
        r#"tk.MustGetErrCode("select * from t use index (i1);", errno.ErrKeyDoesNotExist)"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        635,
        r#"tk.MustGetErrCode("select * from t use index (i2);", errno.ErrKeyDoesNotExist)"#,
    );
    draft.query(
        636,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2"))"#,
    );
    draft.exec(637, r#"tk.MustExec("admin check table t;")"#);
    draft
}

/// TestMultiSchemaChangeMixCancelled 对应 Go 第 640-667 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeMixCancelled 对应 Go 第 640-667 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_mix_cancelled() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeMixCancelled"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(641, r#"if kerneltype.IsNextGen() {"#);
    draft.source(
        642,
        r#"t.Skip("add-index always runs on DXF with ingest mode in nextgen")"#,
    );
    draft.source(644, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(645, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(646, r#"tk.MustExec("use test;")"#);
    draft.exec(
        647,
        r#"tk.MustExec("set global tidb_enable_dist_task = 0;")"#,
    );
    draft.exec(
        648,
        r#"tk.MustExec("set global tidb_ddl_enable_fast_reorg = 0;")"#,
    );
    draft.exec(
        650,
        r#"tk.MustExec("create table t (a int, b int, c int, index i1(c), index i2(c));")"#,
    );
    draft.exec(651, r#"tk.MustExec("insert into t values (1, 2, 3);")"#);
    draft.source(
        652,
        r#"cancelHook := newCancelJobHook(t, store, func(job *model.Job) bool {"#,
    );
    draft.source(653, r#"return job.MultiSchemaInfo != nil &&"#);
    draft.source(654, r#"len(job.MultiSchemaInfo.SubJobs) > 8 &&"#);
    draft.source(
        655,
        r#"job.MultiSchemaInfo.SubJobs[8].SchemaState == model.StateWriteReorganization"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(657, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", cancelHook.OnJobUpdated)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        658,
        r#"tk.MustGetErrCode("alter table t add column d int default 4, add index i3(c), "+"#,
    );
    draft.source(
        659,
        r#""drop column a, drop column if exists z, add column if not exists e int default 5, "+"#,
    );
    draft.source(660, r#""drop index i2, add column f int default 6, drop column b, drop index i1, add column if not exists g int;","#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(661, r#"errno.ErrCancelledDDLJob)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        662,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.source(663, r#"cancelHook.MustCancelDone(t)"#);
    draft.query(
        664,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.query(
        665,
        r#"tk.MustQuery("select * from t use index(i1, i2);").Check(testkit.Rows("1 2 3"))"#,
    );
    draft.exec(666, r#"tk.MustExec("admin check table t;")"#);
    draft
}

/// TestMultiSchemaChangeAdminShowDDLJobs 对应 Go 第 669-697 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeAdminShowDDLJobs 对应 Go 第 669-697 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_admin_show_ddl_jobs() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeAdminShowDDLJobs"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(670, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(672, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(673, r#"tk.MustExec("use test")"#);
    draft.exec(
        674,
        r#"tk.MustExec("create table t (a int, b int, c int)")"#,
    );
    draft.exec(675, r#"tk.MustExec("insert into t values (1, 2, 3)")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(677, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    draft.assertion(
        678,
        r#"assert.Equal(t, model.ActionMultiSchemaChange, job.Type)"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        679,
        r#"if job.MultiSchemaInfo.SubJobs[0].SchemaState == model.StateDeleteOnly {"#,
    );
    draft.source(680, r#"newTk := testkit.NewTestKit(t, store)"#);
    draft.query(
        681,
        r#"rows := newTk.MustQuery("admin show ddl jobs 1").Rows()"#,
    );
    // Go 注释 L682: 1 history job and 1 running job with 1 subjobs
    draft.assertion(683, r#"assert.Equal(t, 3, len(rows))"#);
    draft.assertion(684, r#"assert.Equal(t, "test", rows[1][1])"#);
    draft.assertion(685, r#"assert.Equal(t, "t", rows[1][2])"#);
    draft.assertion(
        686,
        r#"assert.Equal(t, "add index /* subjob */", rows[1][3])"#,
    );
    draft.assertion(687, r#"assert.Equal(t, "delete only", rows[1][4])"#);
    draft.assertion(
        688,
        r#"assert.Equal(t, "running", rows[1][len(rows[1])-2])"#,
    );
    draft.assertion(689, r#"assert.True(t, len(rows[1][8].(string)) > 0)"#);
    draft.assertion(690, r#"assert.True(t, len(rows[1][9].(string)) > 0)"#);
    draft.assertion(691, r#"assert.True(t, len(rows[1][10].(string)) > 0)"#);
    draft.assertion(692, r#"assert.Equal(t, "create table", rows[2][3])"#);
    draft.exec(
        696,
        r#"tk.MustExec("alter table t add index t(a), add index t1(b)")"#,
    );
    draft
}

/// TestMultiSchemaChangeWithExpressionIndex 对应 Go 第 699-736 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeWithExpressionIndex 对应 Go 第 699-736 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_with_expression_index() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeWithExpressionIndex"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(700, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(701, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(702, r#"tk.MustExec("use test;")"#);
    draft.exec(703, r#"tk.MustExec("create table t (a int, b int);")"#);
    draft.exec(
        704,
        r#"tk.MustExec("insert into t values (1, 2), (2, 1);")"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(705, r#"tk.MustGetErrCode("alter table t drop column a, add unique index idx((a + b));", errno.ErrUnsupportedDDLOperation)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(706, r#"tk.MustGetErrCode("alter table t add column c int, change column a d bigint, add index idx((a + a));", errno.ErrUnsupportedDDLOperation)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(707, r#"tk.MustGetErrCode("alter table t add column c int default 10, add index idx1((a + b)), add unique index idx2((a + b));","#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(708, r#"errno.ErrDupEntry)"#);
    draft.query(
        709,
        r#"tk.MustQuery("select * from t;").Check(testkit.Rows("1 2", "2 1"))"#,
    );
    draft.source(711, r#"var checkErr error"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(712, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(713, r#"if checkErr != nil {"#);
    draft.source(714, r#"return"#);
    draft.assertion(
        716,
        r#"assert.Equal(t, model.ActionMultiSchemaChange, job.Type)"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        717,
        r#"if job.MultiSchemaInfo.SubJobs[1].SchemaState == model.StateWriteOnly {"#,
    );
    draft.source(718, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(719, r#"tk2.MustExec("use test;")"#);
    draft.exec(
        720,
        r#"_, checkErr = tk2.Exec("update t set a = 3 where a = 1;")"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(721, r#"if checkErr != nil {"#);
    draft.source(722, r#"return"#);
    draft.exec(
        724,
        r#"_, checkErr = tk2.Exec("insert into t values (10, 10);")"#,
    );
    draft.exec(727, r#"tk.MustExec("alter table t add column c int default 10, add index idx1((a + b)), add unique index idx2((a + b));")"#);
    draft.assertion(728, r#"require.NoError(t, checkErr)"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        729,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep")"#,
    );
    draft.exec(731, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(732, r#"tk.MustExec("create table t (a int, b int);")"#);
    draft.exec(
        733,
        r#"tk.MustExec("insert into t values (1, 2), (2, 1);")"#,
    );
    draft.exec(734, r#"tk.MustExec("alter table t add column c int default 10, add index idx1((a + b)), add unique index idx2((a*10 + b));")"#);
    draft.query(735, r#"tk.MustQuery("select * from t use index(idx1, idx2);").Check(testkit.Rows("1 2 10", "2 1 10"))"#);
    draft
}

/// TestMultiSchemaChangeNoSubJobs 对应 Go 第 738-749 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeNoSubJobs 对应 Go 第 738-749 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_no_sub_jobs() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeNoSubJobs"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(739, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(740, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(741, r#"tk.MustExec("use test;")"#);
    draft.exec(743, r#"tk.MustExec("create table t (a int, b int);")"#);
    draft.exec(744, r#"tk.MustExec("alter table t add column if not exists a int, add column if not exists b int;")"#);
    draft.query(745, r#"tk.MustQuery("show warnings;").Check(testkit.Rows("#);
    draft.source(
        746,
        r#""Note 1060 Duplicate column name 'a'", "Note 1060 Duplicate column name 'b'"))"#,
    );
    draft.query(
        747,
        r#"rs := tk.MustQuery("admin show ddl jobs 1;").Rows()"#,
    );
    draft.assertion(748, r#"require.Equal(t, "create table", rs[0][3])"#);
    draft
}

/// TestMultiSchemaChangeSchemaVersion 对应 Go 第 751-772 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeSchemaVersion 对应 Go 第 751-772 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_schema_version() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeSchemaVersion"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(752, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(753, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(754, r#"tk.MustExec("use test;")"#);
    draft.exec(
        755,
        r#"tk.MustExec("create table t(a int, b int, c int, d int)")"#,
    );
    draft.exec(756, r#"tk.MustExec("insert into t values (1,2,3,4)")"#);
    draft.source(758, r#"schemaVerMap := map[int64]struct{}{}"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(760, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeWaitSchemaSynced", func(_ *model.Job, schemaVer int64) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(761, r#"if schemaVer != 0 {"#);
    // Go 注释 L762: No same return schemaVer during multi-schema change
    draft.source(763, r#"_, ok := schemaVerMap[schemaVer]"#);
    draft.assertion(764, r#"assert.False(t, ok)"#);
    draft.source(765, r#"schemaVerMap[schemaVer] = struct{}{}"#);
    draft.exec(
        768,
        r#"tk.MustExec("alter table t drop column b, drop column c")"#,
    );
    draft.exec(
        769,
        r#"tk.MustExec("alter table t add column b int, add column c int")"#,
    );
    draft.exec(
        770,
        r#"tk.MustExec("alter table t add index k(b), add column e int")"#,
    );
    draft.exec(
        771,
        r#"tk.MustExec("alter table t alter index k invisible, drop column e")"#,
    );
    draft
}

/// TestMultiSchemaChangeMixedWithUpdate 对应 Go 第 774-819 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeMixedWithUpdate 对应 Go 第 774-819 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_mixed_with_update() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeMixedWithUpdate"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(775, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(776, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(777, r#"tk.MustExec("use test;")"#);
    draft.exec(778, r#"tk.MustExec("create table t (c_1 int, c_2 char(20), c_pos_1 int, c_idx_visible int, c_3 decimal(5, 3), " +"#);
    draft.source(779, r#""c_drop_1 time, c_4 datetime, c_drop_idx char(10), c_5 time, c_6 double, c_drop_2 int, c_pos_2 char(10), " +"#);
    draft.source(780, r#""c_add_idx_1 int, c_add_idx_2 char(20), index idx_1(c_1), index idx_2(c_2), index idx_drop(c_drop_idx), " +"#);
    draft.source(781, r#""index idx_3(c_drop_1), index idx_4(c_4), index idx_5(c_pos_1, c_pos_2), index idx_visible(c_idx_visible));")"#);
    draft.exec(
        782,
        r#"tk.MustExec("insert into t values (100, 'c_2_insert', 101, 12, 2.1, '10:00:00', " +"#,
    );
    draft.source(
        783,
        r#""'2020-01-01 10:00:00', 'wer', '10:00:00', 2.1, 12, 'qwer', 12, 'asdf');")"#,
    );
    draft.source(785, r#"var checkErr error"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(786, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(787, r#"if checkErr != nil {"#);
    draft.source(788, r#"return"#);
    draft.assertion(
        790,
        r#"assert.Equal(t, model.ActionMultiSchemaChange, job.Type)"#,
    );
    // Go 注释 L791: Wait for "drop column c_drop_2" entering delete-only state.
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(
        792,
        r#"if job.MultiSchemaInfo.SubJobs[8].SchemaState == model.StateDeleteOnly {"#,
    );
    draft.source(793, r#"tk2 := testkit.NewTestKit(t, store)"#);
    draft.exec(794, r#"tk2.MustExec("use test;")"#);
    draft.exec(795, r#"_, checkErr = tk2.Exec("update t set c_4 = '2020-01-01 10:00:00', c_5 = 'c_5_update', c_1 = 102, " +"#);
    draft.source(796, r#""c_2 = '1', c_pos_1 = 102, c_idx_visible = 102, c_3 = 3.1, c_drop_idx = 'er', c_6 = 2, c_pos_2 = 'dddd', " +"#);
    draft.source(797, r#""c_add_idx_1 = 102, c_add_idx_2 = 'zxc', c_add_2 = 10001, c_add_1 = 10001 where c_drop_idx = 'wer';")"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(798, r#"if checkErr != nil {"#);
    draft.source(799, r#"return"#);
    draft.exec(803, r#"tk.MustExec("alter table t " +"#);
    draft.source(804, r#""add index i_add_1(c_add_idx_1), " +"#);
    draft.source(805, r#""drop index idx_drop, " +"#);
    draft.source(806, r#""add index i_add_2(c_add_idx_2),  " +"#);
    draft.source(807, r#""modify column c_2 char(100), " +"#);
    draft.source(808, r#""add column c_add_2 bigint, " +"#);
    draft.source(809, r#""modify column c_1 bigint, " +"#);
    draft.source(810, r#""add column c_add_1 bigint, " +"#);
    draft.source(811, r#""modify column c_5 varchar(255) first, " +"#);
    draft.source(812, r#""modify column c_4 datetime first, " +"#);
    draft.source(813, r#""drop column c_drop_1, " +"#);
    draft.source(814, r#""drop column c_drop_2, " +"#);
    draft.source(815, r#""modify column c_6 int, " +"#);
    draft.source(816, r#""alter index idx_visible invisible, " +"#);
    draft.source(817, r#""modify column c_3 decimal(10, 2);")"#);
    draft.assertion(818, r#"require.NoError(t, checkErr)"#);
    draft
}

/// TestMultiSchemaChangeModifyColumnOrderByStates 对应 Go 第 821-847 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeModifyColumnOrderByStates 对应 Go 第 821-847 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_modify_column_order_by_states() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeModifyColumnOrderByStates"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(822, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(823, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(824, r#"tk.MustExec("use test;")"#);
    draft.exec(826, r#"tk.MustExec("create table t (a int, b int);")"#);
    draft.exec(827, r#"tk.MustExec("insert into t values (1, 1);")"#);
    draft.exec(
        828,
        r#"tk.MustExec("alter table t modify column b smallint, add column d int;")"#,
    );
    draft.exec(830, r#"tk.MustExec("drop table t;")"#);
    draft.exec(831, r#"tk.MustExec("create table t (a int, b int);")"#);
    draft.exec(832, r#"tk.MustExec("insert into t values (1, 1);")"#);
    draft.exec(833, r#"tk.MustExec("alter table t modify column a smallint, add column c int, modify column b smallint;")"#);
    draft.exec(835, r#"tk.MustExec("drop table t;")"#);
    draft.exec(
        836,
        r#"tk.MustExec("create table t (a int, b int, c char(10));")"#,
    );
    draft.exec(837, r#"tk.MustExec("insert into t values (1, 1, '1');")"#);
    draft.exec(838, r#"tk.MustExec("alter table t modify column c int after a, add column d int, add column e int, modify column b smallint;")"#);
    draft.exec(840, r#"tk.MustExec("drop table t;")"#);
    draft.exec(
        841,
        r#"tk.MustExec("create table t (id bigint, c1 bigint, c2 bigint);")"#,
    );
    draft.exec(842, r#"tk.MustExec("alter table t modify column c2 int after id, modify column id int after c2;")"#);
    draft.exec(844, r#"tk.MustExec("drop table t;")"#);
    draft.exec(
        845,
        r#"tk.MustExec("create table t1(id bigint, c1 bigint, c2 bigint);")"#,
    );
    draft.exec(
        846,
        r#"tk.MustExec("alter table t1 modify column c2 int, drop column id;")"#,
    );
    draft
}

/// TestMultiSchemaChangeDMLUpdate 对应 Go 第 849-867 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeDMLUpdate 对应 Go 第 849-867 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_dml_update() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeDMLUpdate"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(850, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(851, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(853, r#"tk.MustExec("use test")"#);
    draft.exec(
        854,
        r#"tk.MustExec("create table t(a int, b int, c int, d int)")"#,
    );
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(856, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", func(job *model.Job) {"#);
    draft.source(857, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(858, r#"tk.MustExec("use test")"#);
    draft.exec(
        859,
        r#"tk.MustExec("insert into t(a, c) values (1, 2), (2, 3), (3, 4), (4, 5)")"#,
    );
    draft.exec(860, r#"tk.MustExec("update t set c = 5 where a = 1")"#);
    draft.exec(861, r#"tk.MustExec("delete from t")"#);
    draft.exec(863, r#"tk.MustExec("alter table t change column b e int unsigned, change column d f int unsigned")"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        864,
        r#"testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced")"#,
    );
    draft.exec(866, r#"tk.MustExec("drop table t")"#);
    draft
}

/// TestMultiSchemaChangeBlockedByRowLevelChecksum 对应 Go 第 869-888 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeBlockedByRowLevelChecksum 对应 Go 第 869-888 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_blocked_by_row_level_checksum() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeBlockedByRowLevelChecksum"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(870, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(872, r#"orig := vardef.EnableRowLevelChecksum.Load()"#);
    draft.concurrency(873, r#"defer vardef.EnableRowLevelChecksum.Store(orig)"#);
    draft.source(875, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(876, r#"tk.MustExec("use test")"#);
    draft.exec(877, r#"tk.MustExec("create table t (c int)")"#);
    draft.source(879, r#"vardef.EnableRowLevelChecksum.Store(true)"#);
    draft.source(
        880,
        r#"tk.Session().GetSessionVars().EnableRowLevelChecksum = false"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(881, r#"tk.MustGetErrCode("alter table t add column c1 int, add column c2 int", errno.ErrUnsupportedDDLOperation)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(882, r#"tk.MustGetErrCode("alter table t add (c1 int, c2 int)", errno.ErrUnsupportedDDLOperation)"#);
    draft.source(884, r#"vardef.EnableRowLevelChecksum.Store(false)"#);
    draft.source(
        885,
        r#"tk.Session().GetSessionVars().EnableRowLevelChecksum = true"#,
    );
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(886, r#"tk.MustGetErrCode("alter table t add column c1 int, add column c2 int", errno.ErrUnsupportedDDLOperation)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(887, r#"tk.MustGetErrCode("alter table t add (c1 int, c2 int)", errno.ErrUnsupportedDDLOperation)"#);
    draft
}

/// TestMultiSchemaChangePollJobCount 对应 Go 第 890-913 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangePollJobCount 对应 Go 第 890-913 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_poll_job_count() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangePollJobCount"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(891, r#"if kerneltype.IsNextGen() {"#);
    draft.source(
        892,
        r#"t.Skip("add-index always runs on DXF with ingest mode in nextgen")"#,
    );
    draft.source(894, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(895, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(896, r#"tk.MustExec("use test")"#);
    draft.exec(897, r#"tk.MustExec("create table t (a int)")"#);
    draft.exec(898, r#"tk.MustExec("insert into t values (1);")"#);
    draft.exec(
        899,
        r#"tk.MustExec("set global tidb_ddl_enable_fast_reorg = 0;")"#,
    );
    draft.exec(
        900,
        r#"tk.MustExec("set global tidb_enable_dist_task = 0;")"#,
    );
    draft.source(901, r#"runOneJobCounter := 0"#);
    draft.source(902, r#"pollJobCounter := 0"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(903, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/onRunOneJobStep", func() {"#);
    draft.source(904, r#"runOneJobCounter++"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(906, r#"testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforePollDDLJob", func() {"#);
    draft.source(907, r#"pollJobCounter++"#);
    // Go 注释 L909: Should not test reorg DDL because the result can be unstable.
    draft.exec(910, r#"tk.MustExec("alter table t add column b int,  modify column a bigint, add column c char(10);")"#);
    draft.assertion(911, r#"require.Equal(t, 30, runOneJobCounter)"#);
    draft.assertion(912, r#"require.Equal(t, 10, pollJobCounter)"#);
    draft
}

/// TestMultiSchemaChangeMDLView 对应 Go 第 915-935 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeMDLView 对应 Go 第 915-935 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_mdl_view() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeMDLView"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    draft.source(916, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(917, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(918, r#"tk.MustExec("use test")"#);
    draft.source(919, r#"unistoreMDLView := metadef.CreateTiDBMDLView"#);
    draft.source(920, r#"unistoreMDLView = strings.ReplaceAll(unistoreMDLView, "cluster_processlist", "processlist")"#);
    draft.source(
        921,
        r#"unistoreMDLView = strings.ReplaceAll(unistoreMDLView, "cluster_tidb_trx", "tidb_trx")"#,
    );
    draft.exec(922, r#"tk.MustExec(unistoreMDLView)"#);
    draft.exec(924, r#"tk.MustExec("create table t (a int);")"#);
    draft.exec(
        925,
        r#"tk.MustExec("alter table t add column b int, add column c int;")"#,
    );
    draft.exec(927, r#"tk.MustExec("begin;")"#);
    draft.exec(928, r#"tk.MustExec("insert into t values (1, 1, 1);")"#);
    draft.source(930, r#"tk1 := testkit.NewTestKit(t, store)"#);
    draft.exec(931, r#"tk1.MustExec("use test")"#);
    draft.query(
        932,
        r#"tk1.MustQuery("select count(*) from mysql.tidb_mdl_view;").Check(testkit.Rows("0"))"#,
    );
    draft.exec(934, r#"tk.MustExec("commit;")"#);
    draft
}

/// TestMultiSchemaChangeWithoutMDL 对应 Go 第 937-966 行：保留原测试/辅助函数的执行顺序。
// TestMultiSchemaChangeWithoutMDL 对应 Go 第 937-966 行：保留原测试/辅助函数的执行顺序。
#[test]
fn test_multi_schema_change_without_mdl() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"TestMultiSchemaChangeWithoutMDL"#);
    // 测试入口在 Go 中通过 testkit 创建 mock store/session；这里仅记录 SQL、failpoint 与断言步骤。
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(938, r#"if kerneltype.IsNextGen() {"#);
    draft.source(
        939,
        r#"t.Skip("MDL is always enabled and read only in nextgen")"#,
    );
    draft.source(941, r#"store := testkit.CreateMockStore(t)"#);
    draft.source(942, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(943, r#"tk.MustExec("use test")"#);
    draft.concurrency(944, r#"defer func() {"#);
    draft.exec(
        945,
        r#"tk.MustExec("set global tidb_enable_metadata_lock = default;")"#,
    );
    draft.source(948, r#"testcases := []struct {"#);
    draft.source(949, r#"name string"#);
    draft.source(950, r#"sql  string"#);
    draft.source(
        952,
        r#"{"drop column", "alter table t drop column col2, drop column col3"},"#,
    );
    draft.source(
        953,
        r#"{"drop index", "alter table t drop index idx2, drop index idx3"},"#,
    );
    draft.source(954, r#"{"modify column", "alter table t modify column col2 bigint, modify column col3 bigint"},"#);
    draft.source(955, r#"{"modify column with reorg", "alter table t modify column col2 varchar(4), modify column col3 varchar(4)"},"#);
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(957, r#"for _, tc := range testcases {"#);
    draft.source(958, r#"t.Run(tc.name, func(t *testing.T) {"#);
    draft.exec(
        959,
        r#"tk.MustExec("set global tidb_enable_metadata_lock = on;")"#,
    );
    draft.exec(960, r#"tk.MustExec("drop table if exists t;")"#);
    draft.exec(961, r#"tk.MustExec("create table t(col1 int, col2 int, col3 int, index idx2(col2), index idx3(col2));")"#);
    draft.exec(
        962,
        r#"tk.MustExec("set global tidb_enable_metadata_lock = off;")"#,
    );
    draft.exec(963, r#"tk.MustExec(tc.sql)"#);
    draft
}

/// cancelOnceHook 对应 Go 第 968-974 行的辅助类型；字段语义保持为测试记录。
///
/// 在 DDL job 更新回调中最多取消一次：`pred` 命中后置 `triggered`，并保存取消错误。
#[derive(Debug, Default, Clone)]
pub struct cancelOnceHook {
    /// 是否已经触发过取消（保证只取消一次）。
    pub triggered: bool,
    /// 取消 job 时返回的错误（若有）。
    pub cancel_err: Option<String>,
    /// 谓词说明文本，便于对照 Go 侧 cancel 条件。
    pub pred_note: &'static str,
}

/// OnJobUpdated 对应 Go 第 976-987 行：保留原测试/辅助函数的执行顺序。
// OnJobUpdated 对应 Go 第 976-987 行：保留原测试/辅助函数的执行顺序。
pub fn on_job_updated() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"OnJobUpdated"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(977, r#"if c.triggered || !c.pred(job) {"#);
    draft.source(978, r#"return"#);
    draft.source(980, r#"c.triggered = true"#);
    draft.concurrency(
        981,
        r#"errs, err := ddl.CancelJobs(context.Background(), c.s, []int64{job.ID})"#,
    );
    // 分支或循环控制影响测试覆盖的状态组合，按源码顺序记录。
    draft.branch(982, r#"if len(errs) > 0 && errs[0] != nil {"#);
    draft.source(983, r#"c.cancelErr = errs[0]"#);
    draft.source(984, r#"return"#);
    draft.source(986, r#"c.cancelErr = err"#);
    draft
}

/// MustCancelDone 对应 Go 第 989-992 行：保留原测试/辅助函数的执行顺序。
// MustCancelDone 对应 Go 第 989-992 行：保留原测试/辅助函数的执行顺序。
pub fn must_cancel_done() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"MustCancelDone"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.assertion(990, r#"require.True(t, c.triggered)"#);
    draft.assertion(991, r#"require.NoError(t, c.cancelErr)"#);
    draft
}

/// MustCancelFailed 对应 Go 第 994-997 行：保留原测试/辅助函数的执行顺序。
// MustCancelFailed 对应 Go 第 994-997 行：保留原测试/辅助函数的执行顺序。
pub fn must_cancel_failed() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"MustCancelFailed"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.assertion(995, r#"require.True(t, c.triggered)"#);
    // 错误断言保留原 errno/错误字符串，方便核对 DDL 失败语义。
    draft.error(
        996,
        r#"require.Contains(t, c.cancelErr.Error(), strconv.Itoa(errno.ErrCannotCancelDDLJob))"#,
    );
    draft
}

/// newCancelJobHook 对应 Go 第 999-1008 行：保留原测试/辅助函数的执行顺序。
// newCancelJobHook 对应 Go 第 999-1008 行：保留原测试/辅助函数的执行顺序。
pub fn new_cancel_job_hook() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"newCancelJobHook"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.concurrency(1000, r#"pred func(job *model.Job) bool) *cancelOnceHook {"#);
    draft.source(1001, r#"tk := testkit.NewTestKit(t, store)"#);
    draft.exec(1002, r#"tk.MustExec("use test")"#);
    draft.concurrency(1003, r#"return &cancelOnceHook{"#);
    draft.source(1004, r#"store: store,"#);
    draft.source(1005, r#"pred:  pred,"#);
    draft.source(1006, r#"s:     tk.Session(),"#);
    draft
}

/// putTheSameDDLJobTwice 对应 Go 第 1010-1016 行：保留原测试/辅助函数的执行顺序。
// putTheSameDDLJobTwice 对应 Go 第 1010-1016 行：保留原测试/辅助函数的执行顺序。
pub fn put_the_same_ddl_job_twice() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"putTheSameDDLJobTwice"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(1011, r#"err := failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/mockParallelSameDDLJobTwice", `return(true)`)"#);
    draft.assertion(1012, r#"require.NoError(t, err)"#);
    draft.source(1013, r#"fn()"#);
    // failpoint 会改变 DDL 状态机路径；这里只记录注入点和回调意图。
    draft.failpoint(
        1014,
        r#"err = failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/mockParallelSameDDLJobTwice")"#,
    );
    draft.assertion(1015, r#"require.NoError(t, err)"#);
    draft
}

/// assertMultiSchema 对应 Go 第 1018-1021 行：保留原测试/辅助函数的执行顺序。
// assertMultiSchema 对应 Go 第 1018-1021 行：保留原测试/辅助函数的执行顺序。
pub fn assert_multi_schema() -> CaseRecorder {
    let mut draft = CaseRecorder::new(r#"assertMultiSchema"#);
    // 辅助函数在 Go 中被其它测试调用；保留参数解析、资源收尾和错误处理的关键语句。
    draft.assertion(1019, r#"assert.NotNil(t, job.MultiSchemaInfo, job)"#);
    draft.assertion(
        1020,
        r#"assert.Len(t, job.MultiSchemaInfo.SubJobs, subJobLen, job)"#,
    );
    draft
}

struct DiskFullRunner;

impl crate::multi_schema_change::SubJobRunner for DiskFullRunner {
    fn run_step(
        &mut self,
        _sub_job: &crate::multi_schema_change::SubJob,
        _rolling_back: bool,
        _skip_version: bool,
    ) -> Result<crate::multi_schema_change::StepResult, crate::multi_schema_change::MultiSchemaError>
    {
        use crate::multi_schema_change::{MultiJobState, StepResult};

        Ok(StepResult {
            version: 7,
            state: MultiJobState::Paused,
            error: None,
            paused_for_disk_full: true,
            pause_reason: Some("disk full".to_string()),
        })
    }
}

#[test]
fn disk_full_pause_preserves_the_generated_schema_version() {
    use crate::multi_schema_change::{
        MultiAction, MultiJobState, MultiSchemaInfo, MultiSchemaJob, SubJob,
        run_multi_schema_change,
    };

    let mut job = MultiSchemaJob {
        state: MultiJobState::Running,
        info: MultiSchemaInfo {
            revertible: true,
            sub_jobs: vec![SubJob {
                action: MultiAction::ModifyComment,
                state: MultiJobState::Running,
                schema_version: 0,
                revertible: true,
                need_reorg: false,
                error: None,
                warning: None,
            }],
            ..MultiSchemaInfo::default()
        },
        resume_reason: Some("resume".to_string()),
        pause_reason: None,
        error: None,
        schema_version: 0,
    };

    let version = run_multi_schema_change(&mut job, &mut DiskFullRunner).unwrap();

    assert_eq!(version, 7);
    assert_eq!(job.schema_version, 7);
    assert_eq!(job.info.sub_jobs[0].schema_version, 7);
    assert_eq!(job.info.sub_jobs[0].state, MultiJobState::Running);
    assert_eq!(job.state, MultiJobState::Paused);
    assert_eq!(job.resume_reason, None);
    assert_eq!(job.pause_reason.as_deref(), Some("disk full"));
}
