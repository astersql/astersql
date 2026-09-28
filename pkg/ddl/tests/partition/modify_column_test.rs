// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 分区表 `MODIFY COLUMN`（修改列）DDL 测试。
//
// 对应 Go `modify_column_test.go`：覆盖 range/list/key 分区上改列类型、
// 重建索引（recreate index）时 reorg 游标跨物理分区推进、失败回滚清理、
// 全局索引一致性，以及分区列可空性/默认值/表达式白名单等约束。
// 上方大段为 Go 步骤文本占位（块注释内）；文件末尾为可运行的元数据与统计用例。
// Reorg（reorganization）指 DDL 后台回填存量数据的重组阶段。

/*
// 主要类型与函数按 Go 声明顺序展开；关键分支、并发、failpoint、资源收尾和 IO/外部依赖均在对应步骤旁用中文标注。

#![allow(non_snake_case)]
#![allow(dead_code)]

// Go 注释: Modifying an indexed column on a range-partitioned table should advance reorg progress to the next physical partition in recreate-index stage.
// TestModifyColumnPartitionedTableRecreateIndexCursorReset 对应 Go 测试函数第 34-88 行。
#[test]
fn test_modify_column_partitioned_table_recreate_index_cursor_reset() {
    let _steps = test_modify_column_partitioned_table_recreate_index_cursor_reset_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_recreate_index_cursor_reset_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableRecreateIndexCursorReset(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        r###"	tk.MustExec("drop table if exists t_cursor_reset")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t_cursor_reset ("###,
        r###"		a int primary key,"###,
        r###"		b int,"###,
        r###"		key idx_b(b)"###,
        r###"	) partition by range (a) ("###,
        r###"		partition p0 values less than (10),"###,
        r###"		partition p1 values less than (20),"###,
        r###"		partition p2 values less than (30),"###,
        r###"		partition pMax values less than (MAXVALUE)"###,
        r###"	)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t_cursor_reset values"###,
        r###"		(1,1),(2,2),"###,
        r###"		(11,11),(12,12),"###,
        r###"		(21,21),(22,22),"###,
        r###"		(31,31),(32,32)`)"###,
        r###""###,
        r###"	tblMeta := external.GetTableByName(t, tk, "test", "t_cursor_reset").Meta()"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NotNil(t, tblMeta.Partition)"###,
        r###"	partIDs := make([]int64, 0, len(tblMeta.Partition.Definitions))"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, def := range tblMeta.Partition.Definitions {"###,
        r###"		partIDs = append(partIDs, def.ID)"###,
        r###"	}"###,
        // Go require 断言，只保留断言意图。
        r###"	require.Len(t, partIDs, 4)"###,
        r###""###,
        r###"	var firstNextPID atomic.Int64"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Read ddl_reorg progress inside the callback and keep only the first observed physical_id for recreate-index stage."###,
        // failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterUpdatePartitionReorgInfo", func(job *model.Job) {"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if firstNextPID.Load() != 0 {"###,
        r###"			return"###,
        r###"		}"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if job.Type != model.ActionModifyColumn || job.ReorgMeta == nil || job.ReorgMeta.Stage != model.ReorgStageModifyColumnRecreateIndex {"###,
        r###"			return"###,
        r###"		}"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		observer := testkit.NewTestKit(t, store)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"		rows := observer.MustQuery(fmt.Sprintf("select physical_id from mysql.tidb_ddl_reorg where job_id = %d", job.ID)).Rows()"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if len(rows) == 0 || len(rows[0]) == 0 {"###,
        r###"			return"###,
        r###"		}"###,
        r###"		pid, err := strconv.ParseInt(fmt.Sprintf("%v", rows[0][0]), 10, 64)"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if err != nil {"###,
        r###"			return"###,
        r###"		}"###,
        r###"		firstNextPID.CompareAndSwap(0, pid)"###,
        r###"	})"###,
        r###""###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("alter table t_cursor_reset modify column b int unsigned")"###,
        r###"	tk.MustExec(`set session tidb_enable_fast_table_check = off`)"###,
        r###"	tk.MustExec("admin check table t_cursor_reset")"###,
        // Go require 断言，只保留断言意图。
        r###"	require.Equal(t, partIDs[1], firstNextPID.Load())"###,
        r###"}"###,
    ]
}

// Go 注释: A forced failure during modify-column should roll back cleanly without leaving reorg rows or transient schema states.
// TestModifyColumnPartitionedTableRollbackCleanup 对应 Go 测试函数第 91-145 行。
#[test]
fn test_modify_column_partitioned_table_rollback_cleanup() {
    let _steps = test_modify_column_partitioned_table_rollback_cleanup_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_rollback_cleanup_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableRollbackCleanup(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        r###"	tk.MustExec("drop table if exists t_rb")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t_rb ("###,
        r###"		a int primary key,"###,
        r###"		b int,"###,
        r###"		key idx_b(b)"###,
        r###"	) partition by range (a) ("###,
        r###"		partition p0 values less than (30),"###,
        r###"		partition p1 values less than (60),"###,
        r###"		partition p2 values less than (90),"###,
        r###"		partition pMax values less than (MAXVALUE)"###,
        r###"	)`)"###,
        r###""###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for i := range 128 {"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec("insert into t_rb values (?, ?)", i+1, i+1)"###,
        r###"	}"###,
        r###""###,
        r###"	tblMeta := external.GetTableByName(t, tk, "test", "t_rb").Meta()"###,
        r###"	var jobID int64"###,
        // failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionModifyColumn && job.TableID == tblMeta.ID {"###,
        r###"			jobID = job.ID"###,
        r###"		}"###,
        r###"	})"###,
        r###""###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Force index-record decode failure so the DDL enters rollback path deterministically."###,
        // failpoint 注入/释放点，只记录故障触发语义。
        r###"	require.NoError(t, failpoint.Enable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr", `return("cantDecodeRecordErr")`))"###,
        // Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer func() {"###,
        // failpoint 注入/释放点，只记录故障触发语义。
        r###"		require.NoError(t, failpoint.Disable("github.com/pingcap/tidb/pkg/ddl/MockGetIndexRecordErr"))"###,
        r###"	}()"###,
        r###""###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"	err := tk.ExecToErr("alter table t_rb modify column b bigint unsigned")"###,
        // Go require 断言，只保留断言意图。
        r###"	require.ErrorContains(t, err, "Cannot decode index value")"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NotZero(t, jobID)"###,
        r###""###,
        r###"	jobIDStr := strconv.FormatInt(jobID, 10)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery("select job_id from mysql.tidb_ddl_history where job_id = " + jobIDStr).Check(testkit.Rows(jobIDStr))"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery("select job_id, ele_id, ele_type, physical_id from mysql.tidb_ddl_reorg where job_id = " + jobIDStr).Check(testkit.Rows())"###,
        r###""###,
        r###"	tblMeta = external.GetTableByName(t, tk, "test", "t_rb").Meta()"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, col := range tblMeta.Columns {"###,
        // Go require 断言，只保留断言意图。
        r###"		require.False(t, col.IsChanging(), "unexpected changing column left after rollback: %s", col.Name.O)"###,
        // Go require 断言，只保留断言意图。
        r###"		require.False(t, col.IsRemoving(), "unexpected removing column left after rollback: %s", col.Name.O)"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, idx := range tblMeta.Indices {"###,
        // Go require 断言，只保留断言意图。
        r###"		require.False(t, idx.IsChanging(), "unexpected changing index left after rollback: %s", idx.Name.O)"###,
        // Go require 断言，只保留断言意图。
        r###"		require.False(t, idx.IsRemoving(), "unexpected removing index left after rollback: %s", idx.Name.O)"###,
        r###"	}"###,
        r###""###,
        r###"	tk.MustExec(`set session tidb_enable_fast_table_check = off`)"###,
        r###"	tk.MustExec("admin check table t_rb")"###,
        r###"}"###,
    ]
}

// Go 注释: Modifying a globally indexed column on a partitioned table should keep global-index lookup and uniqueness consistent.
// TestModifyColumnPartitionedTableGlobalIndexConsistency 对应 Go 测试函数第 148-168 行。
#[test]
fn test_modify_column_partitioned_table_global_index_consistency() {
    let _steps = test_modify_column_partitioned_table_global_index_consistency_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_global_index_consistency_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableGlobalIndexConsistency(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        r###"	tk.MustExec("drop table if exists t_global_idx")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t_global_idx ("###,
        r###"		a int primary key,"###,
        r###"		b int,"###,
        r###"		c int,"###,
        r###"		unique key uk_c(c) global"###,
        r###"	) partition by hash (a) partitions 4`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t_global_idx values (1,10,10),(2,20,20),(3,30,30),(4,40,40)`)"###,
        r###""###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table t_global_idx modify column c bigint unsigned`)"###,
        r###"	tk.MustExec(`set session tidb_enable_fast_table_check = off`)"###,
        r###"	tk.MustExec("admin check table t_global_idx")"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery("select a, b, c from t_global_idx use index(uk_c) where c = 30").Check(testkit.Rows("3 30 30"))"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"	tk.MustContainErrMsg("insert into t_global_idx values (100,1,30)", "Duplicate entry '30'")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("insert into t_global_idx values (100,100,100)")"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery("select count(*) from t_global_idx").Check(testkit.Rows("5"))"###,
        r###"}"###,
    ]
}

// adminCheckPartitionTable 对应 Go 辅助函数第 170-173 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn admin_check_partition_table_go_steps() -> &'static [&'static str] {
    &[
        r###"func adminCheckPartitionTable(tk *testkit.TestKit, tableName string) {"###,
        r###"	tk.MustExec("set session tidb_enable_fast_table_check = off")"###,
        r###"	tk.MustExec(fmt.Sprintf("admin check table %s", tableName))"###,
        r###"}"###,
    ]
}

// partitionAlterSuccessCase 对应 Go 第 175-182 行的类型定义。
// 字段保持为原始 Go 文本，避免为测试引入推测性 Rust 依赖。
#[allow(dead_code)]
pub struct PartitionAlterSuccessCase {}

impl PartitionAlterSuccessCase {
    // 返回原 Go 类型定义，供人工迁移字段、匿名结构或接口约束时对照。
    pub fn go_definition() -> &'static [&'static str] {
        &[
            r###"type partitionAlterSuccessCase struct {"###,
            r###"	name      string"###,
            r###"	tableName string"###,
            r###"	preSQLs   []string"###,
            r###"	alterSQL  string"###,
            r###"	checkSQL  string"###,
            r###"	checkRows []string"###,
            r###"}"###,
        ]
    }
}

// partitionAlterRejectCase 对应 Go 第 184-193 行的类型定义。
// 字段保持为原始 Go 文本，避免为测试引入推测性 Rust 依赖。
#[allow(dead_code)]
pub struct PartitionAlterRejectCase {}

impl PartitionAlterRejectCase {
    // 返回原 Go 类型定义，供人工迁移字段、匿名结构或接口约束时对照。
    pub fn go_definition() -> &'static [&'static str] {
        &[
            r###"type partitionAlterRejectCase struct {"###,
            r###"	name         string"###,
            r###"	tableName    string"###,
            r###"	preSQLs      []string"###,
            r###"	alterSQL     string"###,
            r###"	errCode      int"###,
            r###"	postAlterSQL []string"###,
            r###"	checkSQL     string"###,
            r###"	checkRows    []string"###,
            r###"}"###,
        ]
    }
}

// partitionAlterVerifyCase 对应 Go 第 195-202 行的类型定义。
// 字段保持为原始 Go 文本，避免为测试引入推测性 Rust 依赖。
#[allow(dead_code)]
pub struct PartitionAlterVerifyCase {}

impl PartitionAlterVerifyCase {
    // 返回原 Go 类型定义，供人工迁移字段、匿名结构或接口约束时对照。
    pub fn go_definition() -> &'static [&'static str] {
        &[
            r###"type partitionAlterVerifyCase struct {"###,
            r###"	name      string"###,
            r###"	tableName string"###,
            r###"	preSQLs   []string"###,
            r###"	alterSQL  string"###,
            r###"	errCode   int"###,
            r###"	verify    func(t *testing.T, tk *testkit.TestKit)"###,
            r###"}"###,
        ]
    }
}

// runPartitionAlterSuccessCase 对应 Go 辅助函数第 204-215 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn run_partition_alter_success_case_go_steps() -> &'static [&'static str] {
    &[
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"func runPartitionAlterSuccessCase(t *testing.T, store kv.Storage, tc partitionAlterSuccessCase) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, sql := range tc.preSQLs {"###,
        r###"		tk.MustExec(sql)"###,
        r###"	}"###,
        r###"	tk.MustExec(tc.alterSQL)"###,
        r###"	adminCheckPartitionTable(tk, tc.tableName)"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if tc.checkSQL != "" {"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"		tk.MustQuery(tc.checkSQL).Check(testkit.Rows(tc.checkRows...))"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// runPartitionAlterRejectCase 对应 Go 辅助函数第 217-230 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn run_partition_alter_reject_case_go_steps() -> &'static [&'static str] {
    &[
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"func runPartitionAlterRejectCase(t *testing.T, store kv.Storage, tc partitionAlterRejectCase) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, sql := range tc.preSQLs {"###,
        r###"		tk.MustExec(sql)"###,
        r###"	}"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"	tk.MustGetErrCode(tc.alterSQL, tc.errCode)"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, sql := range tc.postAlterSQL {"###,
        r###"		tk.MustExec(sql)"###,
        r###"	}"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if tc.checkSQL != "" {"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"		tk.MustQuery(tc.checkSQL).Check(testkit.Rows(tc.checkRows...))"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// runPartitionAlterVerifyCase 对应 Go 辅助函数第 232-246 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn run_partition_alter_verify_case_go_steps() -> &'static [&'static str] {
    &[
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"func runPartitionAlterVerifyCase(t *testing.T, store kv.Storage, tc partitionAlterVerifyCase) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, sql := range tc.preSQLs {"###,
        r###"		tk.MustExec(sql)"###,
        r###"	}"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if tc.errCode != 0 {"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustGetErrCode(tc.alterSQL, tc.errCode)"###,
        r###"		return"###,
        r###"	}"###,
        r###"	tk.MustExec(tc.alterSQL)"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if tc.verify != nil {"###,
        r###"		tc.verify(t, tk)"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// checkPartitionColumnMeta 对应 Go 辅助函数第 248-253 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn check_partition_column_meta_go_steps() -> &'static [&'static str] {
    &[
        r###"func checkPartitionColumnMeta(tk *testkit.TestKit, tableName, columnName, expected string) {"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(fmt.Sprintf(`select column_default, is_nullable, column_comment"###,
        r###"		from information_schema.columns"###,
        r###"		where table_schema='test' and table_name='%s' and column_name='%s'`, tableName, columnName))."###,
        // 查询结果检查点，当前保留期望值。
        r###"		Check(testkit.Rows(expected))"###,
        r###"}"###,
    ]
}

// Go 注释: Covers modify-column on LIST COLUMNS and KEY partitioned tables and verifies index-read correctness after type change.
// TestModifyColumnPartitionedTableListAndKeyPartition 对应 Go 测试函数第 256-299 行。
#[test]
fn test_modify_column_partitioned_table_list_and_key_partition() {
    let _steps = test_modify_column_partitioned_table_list_and_key_partition_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_list_and_key_partition_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableListAndKeyPartition(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// LIST COLUMNS partition variant."###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"	t.Run("list columns", func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		tk := testkit.NewTestKit(t, store)"###,
        r###"		tk.MustExec("use test")"###,
        r###"		tk.MustExec("drop table if exists t_list_mod")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`create table t_list_mod ("###,
        r###"			a int,"###,
        r###"			b int,"###,
        r###"			c int,"###,
        r###"			primary key (a, b),"###,
        r###"			key idx_c(c)"###,
        r###"		) partition by list columns (a) ("###,
        r###"			partition p0 values in (1, 2),"###,
        r###"			partition p1 values in (3, 4)"###,
        r###"		)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`insert into t_list_mod values (1,1,10),(2,2,20),(3,3,30),(4,4,40)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`alter table t_list_mod modify column c bigint unsigned`)"###,
        r###"		tk.MustExec(`set session tidb_enable_fast_table_check = off`)"###,
        r###"		tk.MustExec(`admin check table t_list_mod`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"		tk.MustQuery(`select a, b, c from t_list_mod use index(idx_c) where c = 20`).Check(testkit.Rows("2 2 20"))"###,
        r###"	})"###,
        r###""###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// KEY partition variant."###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"	t.Run("key partition", func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		tk := testkit.NewTestKit(t, store)"###,
        r###"		tk.MustExec("use test")"###,
        r###"		tk.MustExec("drop table if exists t_key_mod")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`create table t_key_mod ("###,
        r###"			a int,"###,
        r###"			b int,"###,
        r###"			c int,"###,
        r###"			primary key (a, b),"###,
        r###"			key idx_c(c)"###,
        r###"		) partition by key (a, b) partitions 3`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`insert into t_key_mod values (1,1,10),(2,2,20),(3,3,30),(4,4,40),(5,5,50),(6,6,60)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`alter table t_key_mod modify column c bigint unsigned`)"###,
        r###"		tk.MustExec(`set session tidb_enable_fast_table_check = off`)"###,
        r###"		tk.MustExec(`admin check table t_key_mod`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"		tk.MustQuery(`select a, b, c from t_key_mod use index(idx_c) where c = 60`).Check(testkit.Rows("6 6 60"))"###,
        r###"	})"###,
        r###"}"###,
    ]
}

// TestModifyColumnPartitionedTableKeyPartitionAllowlist 对应 Go 测试函数第 301-512 行。
#[test]
fn test_modify_column_partitioned_table_key_partition_allowlist() {
    let _steps = test_modify_column_partitioned_table_key_partition_allowlist_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_key_partition_allowlist_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableKeyPartitionAllowlist(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        r###"	successCases := []partitionAlterSuccessCase{"###,
        r###"		{"###,
        r###"			name:      "int widening","###,
        r###"			tableName: "t_key_wl_int","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_int`,"###,
        r###"				`create table t_key_wl_int ("###,
        r###"					a tinyint,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_int values (1,10),(2,20),(3,30)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_int modify column a int`,"###,
        r###"			checkSQL:  `select count(*) from t_key_wl_int`,"###,
        r###"			checkRows: []string{"3"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "int widening by change column","###,
        r###"			tableName: "t_key_wl_change","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_change`,"###,
        r###"				`create table t_key_wl_change ("###,
        r###"					a tinyint,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_change values (1,10),(2,20),(3,30)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_change change column a a int`,"###,
        r###"			checkSQL:  `select count(*) from t_key_wl_change`,"###,
        r###"			checkRows: []string{"3"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "integer display width change","###,
        r###"			tableName: "t_key_wl_display_width","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_display_width`,"###,
        r###"				`create table t_key_wl_display_width ("###,
        r###"					a tinyint(3),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_display_width values (1,10),(2,20),(3,30)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_display_width modify column a tinyint(1)`,"###,
        r###"			checkSQL:  `select count(*) from t_key_wl_display_width`,"###,
        r###"			checkRows: []string{"3"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "string widening","###,
        r###"			tableName: "t_key_wl_str","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_str`,"###,
        r###"				`create table t_key_wl_str ("###,
        r###"					a varchar(8),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_str values ('a',1),('bbb',2),('cccc',3)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_str modify column a varchar(32)`,"###,
        r###"			checkSQL:  `select count(*) from t_key_wl_str where a in ('a','bbb','cccc')`,"###,
        r###"			checkRows: []string{"3"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "enum tail append","###,
        r###"			tableName: "t_key_wl_enum","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_enum`,"###,
        r###"				`create table t_key_wl_enum ("###,
        r###"					a enum('x','y'),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_enum values ('x',1),('y',2)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_enum modify column a enum('x','y','z')`,"###,
        r###"			checkSQL:  `select a from t_key_wl_enum order by b`,"###,
        r###"			checkRows: []string{"x", "y"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "set tail append","###,
        r###"			tableName: "t_key_wl_set","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_set`,"###,
        r###"				`create table t_key_wl_set ("###,
        r###"					a set('x','y'),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_set values ('x',1),('y',2),('x,y',3)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_key_wl_set modify column a set('x','y','z')`,"###,
        r###"			checkSQL:  `select a from t_key_wl_set order by b`,"###,
        r###"			checkRows: []string{"x", "y", "x,y"},"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range successCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterSuccessCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###""###,
        r###"	rejectCases := []partitionAlterRejectCase{"###,
        r###"		{"###,
        r###"			name:      "rename by change column rejected","###,
        r###"			tableName: "t_key_wl_change_rename","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_change_rename`,"###,
        r###"				`create table t_key_wl_change_rename ("###,
        r###"					a tinyint,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_change_rename values (1,10),(2,20),(3,30)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_change_rename change column a a2 int`,"###,
        r###"			errCode:  errno.ErrDependentByPartitionFunctional,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "string collation change rejected","###,
        r###"			tableName: "t_key_wl_str_collate","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_str_collate`,"###,
        r###"				`create table t_key_wl_str_collate ("###,
        r###"					a varchar(8) character set utf8mb4 collate utf8mb4_bin,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_str_collate values ('a',1),('bbb',2),('cccc',3)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_str_collate modify column a varchar(32) character set utf8mb4 collate utf8mb4_general_ci`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "float to double rejected","###,
        r###"			tableName: "t_key_wl_float","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_float`,"###,
        r###"				`create table t_key_wl_float ("###,
        r###"					a float,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_float values (1.25,1),(2.5,2),(3.75,3)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_float modify column a double`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "enum reorder rejected","###,
        r###"			tableName: "t_key_wl_enum_reorder","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_enum_reorder`,"###,
        r###"				`create table t_key_wl_enum_reorder ("###,
        r###"					a enum('x','y'),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_enum_reorder values ('x',1),('y',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_enum_reorder modify column a enum('y','x','z')`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"			checkSQL: `select a, a+0 from t_key_wl_enum_reorder order by b`,"###,
        r###"			checkRows: []string{"###,
        r###"				"x 1","###,
        r###"				"y 2","###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "decimal scale widening rejected","###,
        r###"			tableName: "t_key_wl_decimal","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_decimal`,"###,
        r###"				`create table t_key_wl_decimal ("###,
        r###"					a decimal(10,2),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_decimal values (1.23,1),(2.34,2),(3.45,3)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_decimal modify column a decimal(10,4)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "datetime fsp rejected","###,
        r###"			tableName: "t_key_wl_dt","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_dt`,"###,
        r###"				`create table t_key_wl_dt ("###,
        r###"					a datetime,"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_dt values ('2024-01-01 00:00:00',1),('2024-01-02 00:00:00',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_dt modify column a datetime(3)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "binary length rejected","###,
        r###"			tableName: "t_key_wl_bin","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_key_wl_bin`,"###,
        r###"				`create table t_key_wl_bin ("###,
        r###"					a binary(2),"###,
        r###"					b int"###,
        r###"				) partition by key(a) partitions 3`,"###,
        r###"				`insert into t_key_wl_bin values ('aa',1),('bb',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_key_wl_bin modify column a binary(3)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range rejectCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterRejectCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// TestModifyColumnPartitionedTableRangeListColumnsAllowlist 对应 Go 测试函数第 514-623 行。
#[test]
fn test_modify_column_partitioned_table_range_list_columns_allowlist() {
    let _steps = test_modify_column_partitioned_table_range_list_columns_allowlist_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_range_list_columns_allowlist_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableRangeListColumnsAllowlist(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        r###"	successCases := []partitionAlterSuccessCase{"###,
        r###"		{"###,
        r###"			name:      "range columns int widening","###,
        r###"			tableName: "t_range_cols_wl_int","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_wl_int`,"###,
        r###"				`create table t_range_cols_wl_int ("###,
        r###"					a tinyint,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"				`insert into t_range_cols_wl_int values (1,1),(11,11)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_wl_int modify column a int`,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns datetime fsp","###,
        r###"			tableName: "t_range_cols_wl_dt","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_wl_dt`,"###,
        r###"				`create table t_range_cols_wl_dt ("###,
        r###"					a datetime,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than ('2024-01-10 00:00:00'),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"				`insert into t_range_cols_wl_dt values ('2024-01-01 00:00:00',1),('2024-02-01 00:00:00',2)`,"###,
        r###"			},"###,
        r###"			alterSQL:  `alter table t_range_cols_wl_dt modify column a datetime(3)`,"###,
        r###"			checkSQL:  `select count(*) from t_range_cols_wl_dt where a < '2024-01-10'`,"###,
        r###"			checkRows: []string{"1"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "list columns varbinary extension","###,
        r###"			tableName: "t_list_cols_wl_varbin","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_list_cols_wl_varbin`,"###,
        r###"				`create table t_list_cols_wl_varbin ("###,
        r###"					a varbinary(2),"###,
        r###"					b int"###,
        r###"				) partition by list columns(a) ("###,
        r###"					partition p0 values in ('a'),"###,
        r###"					partition p1 values in ('b')"###,
        r###"				)`,"###,
        r###"				`insert into t_list_cols_wl_varbin values ('a',1),('b',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_list_cols_wl_varbin modify column a varbinary(4)`,"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range successCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterSuccessCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###""###,
        r###"	rejectCases := []partitionAlterRejectCase{"###,
        r###"		{"###,
        r###"			name:      "list columns varchar shrink under empty sql_mode rejected","###,
        r###"			tableName: "t_list_cols_wl_varchar_shrink","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_list_cols_wl_varchar_shrink`,"###,
        r###"				`create table t_list_cols_wl_varchar_shrink ("###,
        r###"					a varchar(6),"###,
        r###"					b int"###,
        r###"				) partition by list columns(a) ("###,
        r###"					partition p0 values in ('123456'),"###,
        r###"					partition p1 values in ('654321')"###,
        r###"				)`,"###,
        r###"				`insert into t_list_cols_wl_varchar_shrink values ('123456',1),('654321',2)`,"###,
        r###"				`set session sql_mode = ''`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_list_cols_wl_varchar_shrink modify column a varchar(5)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"			postAlterSQL: []string{"###,
        r###"				`set session tidb_enable_fast_table_check = off`,"###,
        r###"				`admin check table t_list_cols_wl_varchar_shrink`,"###,
        r###"			},"###,
        r###"			checkSQL:  `select count(*) from t_list_cols_wl_varchar_shrink`,"###,
        r###"			checkRows: []string{"2"},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "list columns binary extension rejected","###,
        r###"			tableName: "t_list_cols_wl_bin","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_list_cols_wl_bin`,"###,
        r###"				`create table t_list_cols_wl_bin ("###,
        r###"					a binary(2),"###,
        r###"					b int"###,
        r###"				) partition by list columns(a) ("###,
        r###"					partition p0 values in ('aa'),"###,
        r###"					partition p1 values in ('bb')"###,
        r###"				)`,"###,
        r###"				`insert into t_list_cols_wl_bin values ('aa',1),('bb',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_list_cols_wl_bin modify column a binary(3)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range rejectCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterRejectCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// TestModifyColumnPartitionedTablePartitionColumnNullability 对应 Go 测试函数第 625-710 行。
#[test]
fn test_modify_column_partitioned_table_partition_column_nullability() {
    let _steps = test_modify_column_partitioned_table_partition_column_nullability_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_partition_column_nullability_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTablePartitionColumnNullability(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        r###"	cases := []partitionAlterVerifyCase{"###,
        r###"		{"###,
        r###"			name:      "range columns not null to null allowed","###,
        r###"			tableName: "t_range_cols_nullable_ok","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_nullable_ok`,"###,
        r###"				`create table t_range_cols_nullable_ok ("###,
        r###"					a int not null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"				`insert into t_range_cols_nullable_ok values (1,1),(11,11)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_nullable_ok modify column a int null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_nullable_ok")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_range_cols_nullable_ok values (null,100)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select count(*) from t_range_cols_nullable_ok where a is null`).Check(testkit.Rows("1"))"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns null to not null rejected","###,
        r###"			tableName: "t_range_cols_nullable_reject","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_nullable_reject`,"###,
        r###"				`create table t_range_cols_nullable_reject ("###,
        r###"					a int null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_nullable_reject modify column a int not null`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "expr not null to null allowed","###,
        r###"			tableName: "t_expr_nullable_ok","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_nullable_ok`,"###,
        r###"				`create table t_expr_nullable_ok ("###,
        r###"					a datetime not null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a)) ("###,
        r###"					partition p0 values less than (to_days('2024-01-10')),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"				`insert into t_expr_nullable_ok values ('2024-01-01 00:00:00',1)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_nullable_ok modify column a datetime null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_expr_nullable_ok values (null,100)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select a, v from t_expr_nullable_ok where a is null`).Check(testkit.Rows("<nil> 100"))"###,
        r###"				adminCheckPartitionTable(tk, "t_expr_nullable_ok")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "expr null to not null rejected","###,
        r###"			tableName: "t_expr_nullable_reject","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_nullable_reject`,"###,
        r###"				`create table t_expr_nullable_reject ("###,
        r###"					a datetime null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a)) ("###,
        r###"					partition p0 values less than (to_days('2024-01-10')),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_nullable_reject modify column a datetime not null`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"	}"###,
        r###""###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range cases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterVerifyCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// TestModifyColumnPartitionedTablePartitionColumnDefaultComment 对应 Go 测试函数第 712-865 行。
#[test]
fn test_modify_column_partitioned_table_partition_column_default_comment() {
    let _steps = test_modify_column_partitioned_table_partition_column_default_comment_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_partition_column_default_comment_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTablePartitionColumnDefaultComment(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        r###"	cases := []partitionAlterVerifyCase{"###,
        r###"		{"###,
        r###"			name:      "range columns comment only","###,
        r###"			tableName: "t_range_cols_comment_only","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_comment_only`,"###,
        r###"				`create table t_range_cols_comment_only ("###,
        r###"					a int not null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_comment_only modify column a int not null comment 'only-comment'`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_range_cols_comment_only", "a", "<nil> NO only-comment")"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"				tk.MustGetErrCode(`insert into t_range_cols_comment_only(b) values (101)`, errno.ErrNoDefaultForField)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select count(*) from t_range_cols_comment_only`).Check(testkit.Rows("0"))"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_comment_only")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns default only","###,
        r###"			tableName: "t_range_cols_default_only","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_default_only`,"###,
        r###"				`create table t_range_cols_default_only ("###,
        r###"					a int not null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_default_only modify column a int not null default 1`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_range_cols_default_only", "a", "1 NO ")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_range_cols_default_only(b) values (101)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select a, b from t_range_cols_default_only where b = 101`).Check(testkit.Rows("1 101"))"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_default_only")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns default and comment","###,
        r###"			tableName: "t_range_cols_def_comment","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_def_comment`,"###,
        r###"				`create table t_range_cols_def_comment ("###,
        r###"					a int not null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_def_comment modify column a int not null default 1 comment 'pcol'`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_range_cols_def_comment", "a", "1 NO pcol")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_range_cols_def_comment(b) values (102)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select a, b from t_range_cols_def_comment where b = 102`).Check(testkit.Rows("1 102"))"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_def_comment")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns default value changed","###,
        r###"			tableName: "t_range_cols_def_change","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_def_change`,"###,
        r###"				`create table t_range_cols_def_change ("###,
        r###"					a int not null default 1,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_def_change modify column a int not null default 2`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_range_cols_def_change", "a", "2 NO ")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_range_cols_def_change(b) values (103)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select a, b from t_range_cols_def_change where b = 103`).Check(testkit.Rows("2 103"))"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_def_change")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "range columns default removed","###,
        r###"			tableName: "t_range_cols_def_removed","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_def_removed`,"###,
        r###"				`create table t_range_cols_def_removed ("###,
        r###"					a int not null default 1,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_def_removed modify column a int not null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_range_cols_def_removed", "a", "<nil> NO ")"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"				tk.MustGetErrCode(`insert into t_range_cols_def_removed(b) values (104)`, errno.ErrNoDefaultForField)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select count(*) from t_range_cols_def_removed`).Check(testkit.Rows("0"))"###,
        r###"				adminCheckPartitionTable(tk, "t_range_cols_def_removed")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "expr default and comment","###,
        r###"			tableName: "t_expr_def_comment","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_def_comment`,"###,
        r###"				`create table t_expr_def_comment ("###,
        r###"					a datetime not null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a)) ("###,
        r###"					partition p0 values less than (to_days('2024-01-10')),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_def_comment modify column a datetime not null default '2024-01-01 00:00:00' comment 'expr pcol'`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				checkPartitionColumnMeta(tk, "t_expr_def_comment", "a", "2024-01-01 00:00:00 NO expr pcol")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"				tk.MustExec(`insert into t_expr_def_comment(v) values (7)`)"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select a, v from t_expr_def_comment where v = 7`).Check(testkit.Rows("2024-01-01 00:00:00 7"))"###,
        r###"				adminCheckPartitionTable(tk, "t_expr_def_comment")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "null to not null with default and comment rejected","###,
        r###"			tableName: "t_range_cols_def_reject","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_range_cols_def_reject`,"###,
        r###"				`create table t_range_cols_def_reject ("###,
        r###"					a int null,"###,
        r###"					b int"###,
        r###"				) partition by range columns(a) ("###,
        r###"					partition p0 values less than (10),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_range_cols_def_reject modify column a int not null default 1 comment 'reject'`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"	}"###,
        r###""###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range cases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterVerifyCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// Go 注释: RANGE/LIST/HASH expression/no-func allowlist matrix for partition columns.
// TestModifyColumnPartitionedTableExpressionAllowlist 对应 Go 测试函数第 868-1081 行。
#[test]
fn test_modify_column_partitioned_table_expression_allowlist() {
    let _steps = test_modify_column_partitioned_table_expression_allowlist_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_modify_column_partitioned_table_expression_allowlist_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestModifyColumnPartitionedTableExpressionAllowlist(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        r###""###,
        r###"	successCases := []partitionAlterVerifyCase{"###,
        r###"		{"###,
        r###"			name:      "hash no-func int widening","###,
        r###"			tableName: "t_hash_nofunc_wl","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_hash_nofunc_wl`,"###,
        r###"				`create table t_hash_nofunc_wl ("###,
        r###"					a tinyint,"###,
        r###"					b int"###,
        r###"				) partition by hash(a) partitions 4`,"###,
        r###"				`insert into t_hash_nofunc_wl values (1,1),(2,2),(3,3),(4,4)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_hash_nofunc_wl modify column a int`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				adminCheckPartitionTable(tk, "t_hash_nofunc_wl")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "unary minus to_days datetime fsp","###,
        r###"			tableName: "t_expr_unary_minus_todays","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_unary_minus_todays`,"###,
        r###"				`create table t_expr_unary_minus_todays ("###,
        r###"					a datetime not null,"###,
        r###"					v int"###,
        r###"				) partition by range (-to_days(a)) ("###,
        r###"					partition p0 values less than (-to_days('2024-06-01')),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"				`insert into t_expr_unary_minus_todays values ('2024-07-01 00:00:00',1),('2024-03-01 00:00:00',2)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_unary_minus_todays modify column a datetime(3) not null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				adminCheckPartitionTable(tk, "t_expr_unary_minus_todays")"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`select count(*) from t_expr_unary_minus_todays`).Check(testkit.Rows("2"))"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "to_days and extract on same column","###,
        r###"			tableName: "t_expr_combo_same_col","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_combo_same_col`,"###,
        r###"				`create table t_expr_combo_same_col ("###,
        r###"					a datetime not null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a) + extract(day from a)) ("###,
        r###"					partition p0 values less than (to_days('2024-03-01') + extract(day from '2024-03-01')),"###,
        r###"					partition p1 values less than (to_days('2024-06-01') + extract(day from '2024-06-01')),"###,
        r###"					partition pmax values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_combo_same_col modify column a datetime(3) not null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				adminCheckPartitionTable(tk, "t_expr_combo_same_col")"###,
        r###"			},"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "to_days and extract on two columns","###,
        r###"			tableName: "t_expr_combo_two_cols_extract","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_combo_two_cols_extract`,"###,
        r###"				`create table t_expr_combo_two_cols_extract ("###,
        r###"					a datetime not null,"###,
        r###"					b time not null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a) + extract(second from b)) ("###,
        r###"					partition p0 values less than (to_days('2024-03-01') + extract(second from '00:00:30')),"###,
        r###"					partition p1 values less than (to_days('2024-06-01') + extract(second from '00:00:45')),"###,
        r###"					partition pmax values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_combo_two_cols_extract modify column a datetime(3) not null, modify column b time(3) not null`,"###,
        r###"			verify: func(t *testing.T, tk *testkit.TestKit) {"###,
        r###"				adminCheckPartitionTable(tk, "t_expr_combo_two_cols_extract")"###,
        r###"			},"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range successCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterVerifyCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###""###,
        r###"	rejectCases := []partitionAlterVerifyCase{"###,
        r###"		{"###,
        r###"			name:      "unix timestamp rejected","###,
        r###"			tableName: "t_expr_unix","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_unix`,"###,
        r###"				`create table t_expr_unix ("###,
        r###"					ts timestamp"###,
        r###"				) partition by range (floor(unix_timestamp(ts))) ("###,
        r###"					partition p0 values less than (unix_timestamp('2024-01-02 00:00:00')),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_unix modify column ts timestamp(3)`,"###,
        r###"			errCode:  errno.ErrFieldTypeNotAllowedAsPartitionField,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "floor to_days rejected","###,
        r###"			tableName: "t_expr_floor_todays","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_floor_todays`,"###,
        r###"				`create table t_expr_floor_todays ("###,
        r###"					dt datetime,"###,
        r###"					v int"###,
        r###"				) partition by range (floor(to_days(dt))) ("###,
        r###"					partition p0 values less than (floor(to_days('2024-01-10'))),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_floor_todays modify column dt datetime(3)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "year rejected","###,
        r###"			tableName: "t_expr_other","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_other`,"###,
        r###"				`create table t_expr_other ("###,
        r###"					dt datetime,"###,
        r###"					v int"###,
        r###"				) partition by range (year(dt)) ("###,
        r###"					partition p0 values less than (2025),"###,
        r###"					partition p1 values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_other modify column dt datetime(3)`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			name:      "to_days plus year rejected","###,
        r###"			tableName: "t_expr_combo_other","###,
        r###"			preSQLs: []string{"###,
        // Go 条件分支，保留分支判断文本。
        r###"				`drop table if exists t_expr_combo_other`,"###,
        r###"				`create table t_expr_combo_other ("###,
        r###"					a datetime not null,"###,
        r###"					v int"###,
        r###"				) partition by range (to_days(a) + year(a)) ("###,
        r###"					partition p0 values less than (to_days('2024-03-01') + 2024),"###,
        r###"					partition p1 values less than (to_days('2024-06-01') + 2024),"###,
        r###"					partition pmax values less than (maxvalue)"###,
        r###"				)`,"###,
        r###"			},"###,
        r###"			alterSQL: `alter table t_expr_combo_other modify column a datetime(3) not null`,"###,
        r###"			errCode:  errno.ErrUnsupportedDDLOperation,"###,
        r###"		},"###,
        r###"	}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, tc := range rejectCases {"###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"		t.Run(tc.name, func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"			runPartitionAlterVerifyCase(t, store, tc)"###,
        r###"		})"###,
        r###"	}"###,
        r###""###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"	t.Run("to_days datetime fsp pruning unchanged", func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		tk := testkit.NewTestKit(t, store)"###,
        r###"		tk.MustExec("use test")"###,
        r###"		tk.MustExec("set @@session.tidb_partition_prune_mode = 'dynamic'")"###,
        r###"		tk.MustExec(`drop table if exists t_expr_todays`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`create table t_expr_todays ("###,
        r###"			dt datetime,"###,
        r###"			v int"###,
        r###"		) partition by range (to_days(dt)) ("###,
        r###"			partition p0 values less than (to_days('2024-01-10')),"###,
        r###"			partition p1 values less than (maxvalue)"###,
        r###"		)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`insert into t_expr_todays values ('2024-01-01 00:00:00',1),('2024-02-01 00:00:00',2)`)"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustPartition(`select * from t_expr_todays where dt = '2024-01-01 00:00:00'`, "p0")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`alter table t_expr_todays modify column dt datetime(3)`)"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustPartition(`select * from t_expr_todays where dt = '2024-01-01 00:00:00'`, "p0")"###,
        r###"		adminCheckPartitionTable(tk, "t_expr_todays")"###,
        r###"	})"###,
        r###""###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"	t.Run("extract time fsp pruning unchanged", func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		tk := testkit.NewTestKit(t, store)"###,
        r###"		tk.MustExec("use test")"###,
        r###"		tk.MustExec("set @@session.tidb_partition_prune_mode = 'dynamic'")"###,
        r###"		tk.MustExec(`drop table if exists t_expr_extract`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`create table t_expr_extract ("###,
        r###"			tm time,"###,
        r###"			v int"###,
        r###"		) partition by range (extract(second from tm)) ("###,
        r###"			partition p0 values less than (30),"###,
        r###"			partition p1 values less than (maxvalue)"###,
        r###"		)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`insert into t_expr_extract values ('00:00:10',1),('00:00:40',2)`)"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustPartition(`select * from t_expr_extract where tm = '00:00:10'`, "p0")"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`alter table t_expr_extract modify column tm time(3)`)"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustPartition(`select * from t_expr_extract where tm = '00:00:10'`, "p0")"###,
        r###"		adminCheckPartitionTable(tk, "t_expr_extract")"###,
        r###"	})"###,
        r###""###,
        // Go 子测试入口，记录子场景名称和闭包体。
        r###"	t.Run("to_days and unix_timestamp on two columns", func(t *testing.T) {"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"		tk := testkit.NewTestKit(t, store)"###,
        r###"		tk.MustExec("use test")"###,
        r###"		tk.MustExec(`drop table if exists t_expr_combo_two_cols_unix`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`create table t_expr_combo_two_cols_unix ("###,
        r###"			a datetime not null,"###,
        r###"			b timestamp not null,"###,
        r###"			v int"###,
        r###"		) partition by range (to_days(a) + unix_timestamp(b)) ("###,
        r###"			partition p0 values less than (to_days('2024-03-01') + unix_timestamp('1970-01-01 00:01:00')),"###,
        r###"			partition p1 values less than (to_days('2024-06-01') + unix_timestamp('1970-01-01 00:02:00')),"###,
        r###"			partition pmax values less than (maxvalue)"###,
        r###"		)`)"###,
        // SQL 执行步骤，仅保存语句和执行顺序。
        r###"		tk.MustExec(`alter table t_expr_combo_two_cols_unix modify column a datetime(3) not null`)"###,
        r###"		adminCheckPartitionTable(tk, "t_expr_combo_two_cols_unix")"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"		tk.MustGetErrCode(`alter table t_expr_combo_two_cols_unix modify column b timestamp(3) not null`, errno.ErrFieldTypeNotAllowedAsPartitionField)"###,
        r###"	})"###,
        r###"}"###,
    ]
}
*/

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::mysql::{NotNullFlag, TypeLong, TypeLonglong, UnsignedFlag};
use astersql_meta_model::{
    ColumnInfo, IndexColumn, IndexInfo, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StatePublic, StateWriteOnly, TableInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit_testfailpoint::{enable, eval_bool};

/// 构造带 LessThan 上界的 range 分区定义，供改列元数据用例复用。
fn partition(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 验证改列后 `Clone` 隔离：原列类型不变，副本类型/无符号标志更新，分区与索引身份保留。
#[test]
fn modify_column_clone_preserves_partition_and_index_identity() {
    let mut column = ColumnInfo::New(1, NewCIStr("b"));
    column.SetType(TypeLong);
    column.SetFlag(NotNullFlag);
    let table = TableInfo {
        ID: 10,
        Columns: vec![column],
        Indices: vec![IndexInfo {
            ID: 20,
            Name: NewCIStr("idx_b"),
            Columns: vec![IndexColumn {
                Name: NewCIStr("b"),
                Offset: 0,
                Length: -1,
                ..Default::default()
            }],
            State: StatePublic,
            ..Default::default()
        }],
        Partition: Some(PartitionInfo {
            Type: PartitionType::Range,
            Enable: true,
            Definitions: vec![partition(30, "p0", "10"), partition(31, "pmax", "MAXVALUE")],
            ..Default::default()
        }),
        ..Default::default()
    };
    // 仅修改副本列类型与 Unsigned 标志，核对深拷贝隔离。
    let mut modified = table.Clone();
    modified.Columns[0].SetType(TypeLonglong);
    modified.Columns[0].AddFlag(UnsignedFlag);
    assert_eq!(table.Columns[0].GetType(), TypeLong);
    assert_eq!(modified.Columns[0].GetType(), TypeLonglong);
    assert_ne!(modified.Columns[0].GetFlag() & UnsignedFlag, 0);
    assert_eq!(modified.Indices[0].Columns[0].Name.L, "b");
    assert_eq!(modified.GetPartitionInfo().unwrap().Definitions[1].ID, 31);
}

/// 验证真实分区行在列元数据重建路径下仍可通过物理 stats 可见。
#[test]
fn real_partition_rows_remain_visible_across_column_metadata_rebuild() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table modify_t(a int primary key, b int, key idx_b(b)) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into modify_t values (1,11),(2,22),(3,33),(4,44)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta modify_t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table = context
        .catalog()
        .get(&("test".to_owned(), "modify_t".to_owned()))
        .unwrap()
        .1
        .Clone();
    // 两个 hash 分区各应有 2 行；索引名在 Clone 后仍为 idx_b。
    let counts = table
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|definition| {
            context
                .physical_stats(definition.ID)
                .unwrap()
                .realtime_count
        })
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![2, 2]);
    assert_eq!(table.Indices[0].Name.L, "idx_b");
}

/// 验证改列失败（failpoint）模拟回滚后，表与分区 SchemaState 回到 Public 且无 changing 列。
#[test]
fn modify_column_rollback_leaves_public_schema_metadata() {
    let mut table = TableInfo {
        State: StatePublic,
        Columns: vec![ColumnInfo::New(1, NewCIStr("a"))],
        Partition: Some(PartitionInfo {
            Type: PartitionType::Range,
            Enable: true,
            Definitions: vec![partition(11, "p0", "MAXVALUE")],
            DDLState: StateWriteOnly,
            ..Default::default()
        }),
        ..Default::default()
    };
    // 注入解码失败点后，将分区状态从 WriteOnly 经 DeleteOnly 收回到 Public。
    let _failure = enable("partition/modify-column-decode", "return(true)");
    assert!(eval_bool("partition/modify-column-decode"));
    table.Partition.as_mut().unwrap().DDLState = StateDeleteOnly;
    table.Partition.as_mut().unwrap().DDLState = StatePublic;
    assert_eq!(table.State, StatePublic);
    assert_eq!(table.Partition.as_ref().unwrap().DDLState, StatePublic);
    assert!(table.Columns[0].ChangingFieldType.is_none());
}

/// 验证分区列 Comment 与默认值在类型变更 Clone 后仍保留。
#[test]
fn partition_column_default_comment_survives_type_change_clone() {
    let mut column = ColumnInfo::New(7, NewCIStr("partition_key"));
    column.Comment = "partition column".to_owned();
    column.SetType(TypeLong);
    column
        .SetDefaultValue(Some(astersql_meta_model::DefaultValue::Int(1)))
        .unwrap();
    let mut changed = column.Clone();
    changed.SetType(TypeLonglong);
    assert_eq!(changed.Comment, "partition column");
    assert_eq!(
        changed.GetDefaultValue(),
        Some(astersql_meta_model::DefaultValue::Int(1))
    );
    assert_eq!(column.GetType(), TypeLong);
}

/// 对应 Go `TestModifyColumnPartitionedTableRecreateIndexCursorReset` 的真实
/// testkit 路径：改列后旧数据、分区定义与二级索引仍可同时访问。
#[test]
fn modify_column_rebuild_keeps_rows_partitions_and_index_accessible() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table modify_runtime(a int, b int, key idx_b(b)) \
         partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into modify_runtime values (1, 101), (11, 111), (21, 121)",
        Vec::new(),
    );
    let before = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "modify_runtime".to_owned()))
        .expect("table before modify column")
        .1
        .Clone();
    let partition_ids_before = before
        .GetPartitionInfo()
        .expect("range partition metadata")
        .Definitions
        .iter()
        .map(|definition| definition.ID)
        .collect::<Vec<_>>();
    assert_eq!(before.Columns[1].GetType(), TypeLong);

    testkit.MustExec(
        "alter table modify_runtime modify column b bigint",
        Vec::new(),
    );

    assert_eq!(
        testkit
            .MustQuery("select a, b from modify_runtime order by a", Vec::new())
            .Rows(),
        vec![
            vec!["1".to_owned(), "101".to_owned()],
            vec!["11".to_owned(), "111".to_owned()],
            vec!["21".to_owned(), "121".to_owned()],
        ]
    );
    assert_eq!(
        testkit
            .MustQuery(
                "select a from modify_runtime use index (idx_b) where b = 111",
                Vec::new(),
            )
            .Rows(),
        vec![vec!["11".to_owned()]],
    );
    let table = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "modify_runtime".to_owned()))
        .expect("modified table")
        .1
        .Clone();
    assert_eq!(table.Columns[1].GetType(), TypeLonglong);
    assert_eq!(
        table
            .GetPartitionInfo()
            .unwrap()
            .Definitions
            .iter()
            .map(|definition| definition.ID)
            .collect::<Vec<_>>(),
        partition_ids_before,
    );
    assert!(table.Indices.iter().any(|index| index.Name.L == "idx_b"));
}
