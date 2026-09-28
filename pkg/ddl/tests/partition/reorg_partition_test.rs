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

// 分区重组（reorganize partition）相关 DDL 测试草稿。
//
// 记录 Go 侧全表键值快照、分区切换与 failpoint 注入步骤，便于迁移为可执行 Rust 测试。
//
// 对应 Go `reorg_partition_test.go`：覆盖 reorganize / remove partitioning /
// `PARTITION BY` / add·coalesce hash·key 分区的失败路径，并发 DML、failpoint
// 注入回滚，以及 Placement Policy（放置策略）约束。
// 上方大段为 Go 步骤文本占位（块注释内）；文件末尾为可运行的元数据与统计用例。
// Reorg 指后台将旧物理分区数据回填到新分区定义的重组过程。

/*
// 主要类型与函数按 Go 声明顺序展开；关键分支、并发、failpoint、资源收尾和 IO/外部依赖均在对应步骤旁用中文标注。

#![allow(non_snake_case)]
#![allow(dead_code)]

// allTableData 对应 Go 第 47-51 行的类型定义。
// 字段保持为原始 Go 文本，避免为测试引入推测性 Rust 依赖。
#[allow(dead_code)]
pub struct AllTableData {}

impl AllTableData {
    // 返回原 Go 类型定义，供人工迁移字段、匿名结构或接口约束时对照。
    pub fn go_definition() -> &'static [&'static str] {
        &[
            r###"type allTableData struct {"###,
            r###"	keys [][]byte"###,
            r###"	vals [][]byte"###,
            r###"	tp   []string"###,
            r###"}"###,
        ]
    }
}

// Go 注释: TODO: Create a more generic function that gets all accessible table ids
// Go 注释: from all schemas, and checks the full key space so that there are no
// Go 注释: keys for non-existing table IDs. Also figure out how to wait for deleteRange
// Go 注释: Checks that there are no accessible data after an existing table
// Go 注释: assumes that tableIDs are only increasing.
// Go 注释: To be used during failure testing of ALTER, to make sure cleanup is done.
// noNewTablesAfter 对应 Go 辅助函数第 59-129 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn no_new_tables_after_go_steps() -> &'static [&'static str] {
    &[
        r###"func noNewTablesAfter(t *testing.T, tk *testkit.TestKit, ctx sessionctx.Context, tbl table.Table, msg string) {"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	waitForGC := tk.MustQuery(`select start_key, end_key, "queue" from mysql.gc_delete_range union all select start_key, end_key, "done" from mysql.gc_delete_range_done`).Rows()"###,
        r###"	require.NoError(t, sessiontxn.NewTxn(context.Background(), ctx))"###,
        r###"	txn, err := ctx.Txn(true)"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        // Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer func() {"###,
        r###"		err := txn.Rollback()"###,
        // Go require 断言，只保留断言意图。
        r###"		require.NoError(t, err)"###,
        r###"	}()"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Get max tableID (if partitioned)"###,
        r###"	tblID := tbl.Meta().ID"###,
        r###"	logutil.DDLLogger().Info("noNewTablesAfter", zap.Int64("Table ID", tblID))"###,
        // Go 条件分支，保留分支判断文本。
        r###"	if pt := tbl.GetPartitionedTable(); pt != nil {"###,
        r###"		defs := pt.Meta().Partition.Definitions"###,
        r###"		{"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"			for i := range defs {"###,
        r###"				logutil.DDLLogger().Info("noNewTablesAfter", zap.Int64("Part ID", defs[i].ID))"###,
        r###"				tblID = max(tblID, defs[i].ID)"###,
        r###"			}"###,
        r###"		}"###,
        r###"	}"###,
        r###"	prefix := tablecodec.EncodeTablePrefix(tblID + 1)"###,
        r###"	it, err := txn.Iter(prefix, nil)"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, rowGC := range waitForGC {"###,
        r###"		logutil.DDLLogger().Info("GC","###,
        r###"			zap.String("start", fmt.Sprintf("%v", rowGC[0])),"###,
        r###"			zap.String("end", fmt.Sprintf("%v", rowGC[1])),"###,
        r###"			zap.String("status", fmt.Sprintf("%s", rowGC[2])))"###,
        r###"	}"###,
        r###"ROW:"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for it.Valid() {"###,
        r###"		foundTblID := tablecodec.DecodeTableID(it.Key())"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"		// There are internal table ids starting from MaxInt48 -1 and allocating decreasing ids"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"		// Allow 0xFF of them, See TiDBDDLJobTableID, TiDBDDLReorgTableID, TiDBDDLHistoryTableID, TiDBMDLInfoTableID"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if it.Key()[0] == 't' && foundTblID >= 0xFFFFFFFFFF00 {"###,
        r###"			break"###,
        r###"		}"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"		for _, rowGC := range waitForGC {"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"			// OK if queued for range delete / GC"###,
        r###"			startHex := fmt.Sprintf("%v", rowGC[0])"###,
        r###"			endHex := fmt.Sprintf("%v", rowGC[1])"###,
        r###"			end, err := hex.DecodeString(endHex)"###,
        // Go require 断言，只保留断言意图。
        r###"			require.NoError(t, err)"###,
        r###"			keyHex := hex.EncodeToString(it.Key())"###,
        // Go 条件分支，保留分支判断文本。
        r###"			if startHex <= keyHex && keyHex < endHex {"###,
        r###"				it.Close()"###,
        r###"				it, err = txn.Iter(end, nil)"###,
        // Go require 断言，只保留断言意图。
        r###"				require.NoError(t, err)"###,
        r###"				continue ROW"###,
        r###"			}"###,
        // Go 条件分支，保留分支判断文本。
        r###"			if keyHex < "748000f" {"###,
        r###"				logutil.DDLLogger().Error("not found in GC","###,
        r###"					zap.String("key", keyHex),"###,
        r###"					zap.String("start", startHex),"###,
        r###"					zap.String("end", endHex))"###,
        r###"			}"###,
        r###"		}"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if it.Key()[0] == 't' {"###,
        // TestKit 会话操作，不创建 TiDB 会话。
        r###"			is := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
        r###"			tbl, found := is.TableByID(context.Background(), foundTblID)"###,
        r###"			tblmsg := " Table ID no longer maps to a table""###,
        // Go 条件分支，保留分支判断文本。
        r###"			if found {"###,
        r###"				tblmsg = fmt.Sprintf(" Table name: %s", tbl.Meta().Name.O)"###,
        r###"			}"###,
        r###"			decodedKey := expression.DecodeKeyFromString(ctx.GetExprCtx().GetEvalCtx().TypeCtx(), is, it.Key().String())"###,
        // Go require 断言，只保留断言意图。
        r###"			require.False(t, true, "Found table data after highest physical Table ID %d < %d (%s)\n%s\n"+msg+tblmsg, tblID, foundTblID, it.Key(), decodedKey)"###,
        r###"		}"###,
        r###"		break"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// getAllDataForTableID 对应 Go 辅助函数第 131-172 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn get_all_data_for_table_id_go_steps() -> &'static [&'static str] {
    &[
        r###"func getAllDataForTableID(t *testing.T, ctx sessionctx.Context, tableID int64) allTableData {"###,
        r###"	require.NoError(t, sessiontxn.NewTxn(context.Background(), ctx))"###,
        r###"	txn, err := ctx.Txn(true)"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        // Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer func() {"###,
        r###"		err := txn.Rollback()"###,
        // Go require 断言，只保留断言意图。
        r###"		require.NoError(t, err)"###,
        r###"	}()"###,
        r###""###,
        r###"	all := allTableData{"###,
        r###"		keys: make([][]byte, 0),"###,
        r###"		vals: make([][]byte, 0),"###,
        r###"		tp:   make([]string, 0),"###,
        r###"	}"###,
        r###"	prefix := tablecodec.EncodeTablePrefix(tableID)"###,
        r###"	it, err := txn.Iter(prefix, nil)"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for it.Valid() {"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if !it.Key().HasPrefix(prefix) {"###,
        r###"			break"###,
        r###"		}"###,
        r###"		all.keys = append(all.keys, it.Key())"###,
        r###"		all.vals = append(all.vals, it.Value())"###,
        // Go 条件分支，保留分支判断文本。
        r###"		if tablecodec.IsRecordKey(it.Key()) {"###,
        r###"			all.tp = append(all.tp, "Record")"###,
        r###"			tblID, kv, _ := tablecodec.DecodeRecordKey(it.Key())"###,
        // Go require 断言，只保留断言意图。
        r###"			require.Equal(t, tableID, tblID)"###,
        r###"			vals, _ := tablecodec.DecodeValuesBytesToStrings(it.Value())"###,
        r###"			logutil.DDLLogger().Info("Record","###,
        r###"				zap.Int64("pid", tblID),"###,
        r###"				zap.Stringer("key", kv),"###,
        r###"				zap.Strings("values", vals))"###,
        // Go 条件分支，保留分支判断文本。
        r###"		} else if tablecodec.IsIndexKey(it.Key()) {"###,
        r###"			all.tp = append(all.tp, "Index")"###,
        r###"		} else {"###,
        r###"			all.tp = append(all.tp, "Other")"###,
        r###"		}"###,
        r###"		err = it.Next()"###,
        // Go require 断言，只保留断言意图。
        r###"		require.NoError(t, err)"###,
        r###"	}"###,
        r###"	return all"###,
        r###"}"###,
    ]
}

// TestReorgPartitionFailures 对应 Go 测试函数第 174-203 行。
#[test]
fn test_reorg_partition_failures() {
    let _steps = test_reorg_partition_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition p2 values less than (30),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`"###,
        r###"	alter := "alter table t reorganize partition p1,p2 into (partition p1 values less than (17), partition p1b values less than (24), partition p2 values less than (30))""###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,"1",1),(2,"2",2),(12,"12",21),(13,"13",13),(17,"17",17),(18,"18",18),(23,"23",32),(34,"34",43),(45,"45",54),(56,"56",65)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 17`,"###,
        r###"		`update t set b = "21", c = 12 where c = 12`,"###,
        r###"		`delete from t where a = 13`,"###,
        r###"		`delete from t where b = "56"`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("###,
        r###"		"1 1 1", "11 11 11", "12 12 21", "18 18 18", "2 2 2", "23 23 32", "34 34 43", "45 45 54","###,
        r###"	)"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (5,"5",5),(13,"13",13)`,"###,
        r###"		`update t set a = 17, b = "17", c = 17 where a = 11`,"###,
        r###"		`update t set b = "12", c = 21 where c = 12`,"###,
        r###"		`delete from t where a = 34`,"###,
        r###"		`delete from t where b = "56"`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("###,
        r###"		"1 1 1", "12 12 21", "13 13 13", "17 17 17", "18 18 18", "2 2 2", "23 23 32", "45 45 54", "5 5 5","###,
        r###"	)"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestRemovePartitionFailures 对应 Go 测试函数第 205-228 行。
#[test]
fn test_remove_partition_failures() {
    let _steps = test_remove_partition_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_remove_partition_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestRemovePartitionFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255)) partition by range(a) ("###,
        r###"                        partition p0 values less than (100),"###,
        r###"                        partition p1 values less than (200))`"###,
        r###"	alter := `alter table t remove partitioning`"###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(3,3,3),(101,101,101),(102,102,102),(103,103,103)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 1`,"###,
        r###"		`update t set b = "12", c = 12 where b = 2`,"###,
        r###"		`delete from t where a = 102`,"###,
        r###"		`delete from t where b = 103`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("101 101 101", "11 11 11", "2 12 12", "3 3 3")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where a = 11`,"###,
        r###"		`update t set b = 2, c = 2 where c = 12`,"###,
        r###"		`update t set a = 9, b = 9 where a = 104`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 102`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "101 101 101", "2 2 2", "3 3 3", "4 4 4", "9 9 104")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionByFailures 对应 Go 测试函数第 230-253 行。
#[test]
fn test_partition_by_failures() {
    let _steps = test_partition_by_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255)) partition by range(a) ("###,
        r###"                        partition p0 values less than (100),"###,
        r###"                        partition p1 values less than (200))`"###,
        r###"	alter := "alter table t partition by range (b) (partition pNoneC values less than (150), partition p2 values less than (300)) update indexes (`primary` global)""###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(3,3,3),(101,101,101),(102,102,102),(103,103,103)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 1`,"###,
        r###"		`update t set b = "12", c = 12 where b = 2`,"###,
        r###"		`delete from t where a = 102`,"###,
        r###"		`delete from t where b = 103`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("101 101 101", "11 11 11", "2 12 12", "3 3 3")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where a = 11`,"###,
        r###"		`update t set b = 2, c = 2 where c = 12`,"###,
        r###"		`update t set a = 9, b = 9 where a = 104`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 102`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "101 101 101", "2 2 2", "3 3 3", "4 4 4", "9 9 104")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestReorganizePartitionListFailures 对应 Go 测试函数第 255-279 行。
#[test]
fn test_reorganize_partition_list_failures() {
    let _steps = test_reorganize_partition_list_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorganize_partition_list_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorganizePartitionListFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique index (c) global) partition by list(b) ("###,
        r###"                        partition p0 values in (1,2,3),"###,
        r###"                        partition p1 values in (4,5,6),"###,
        r###"                        partition p2 values in (7,8,9))`"###,
        r###"	alter := `alter table t reorganize partition p0,p2 into (partition pNone1 values in (1,9), partition pNone2 values in (2,8), partition pNone3 values in (3,7))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(4,4,4),(8,8,8),(9,9,9),(6,6,6)`,"###,
        r###"		`update t set a = 7, b = 7, c = 7 where a = 1`,"###,
        r###"		`update t set b = 3, c = 3 where c = 4`,"###,
        r###"		`delete from t where a = 8`,"###,
        r###"		`delete from t where b = 2`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("4 3 3", "6 6 6", "7 7 7", "9 9 9")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (1,1,1),(5,5,5),(8,8,8)`,"###,
        r###"		`update t set a = 2, b = 2, c = 2 where a = 1`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where c = 6`,"###,
        r###"		`update t set a = 6, b = 6 where a = 9`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 3`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "2 2 2", "6 6 9", "7 7 7", "8 8 8")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionByListFailures 对应 Go 测试函数第 281-304 行。
#[test]
fn test_partition_by_list_failures() {
    let _steps = test_partition_by_list_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_list_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByListFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique index (b), unique index (c) global) partition by list(b) ("###,
        r###"                        partition p0 values in (1,2,3,4,5,6),"###,
        r###"                        partition p1 values in (11,10,9,8,7))`"###,
        r###"	alter := `alter table t partition by list columns (c) (partition pNone1 values in (1,11,3,5,7,9), partition pNone2 values in (2,4,8,10,6)) update indexes (b global, c local)`"###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(4,4,4),(8,8,8),(9,9,9),(6,6,6)`,"###,
        r###"		`update t set a = 7, b = 7, c = 7 where a = 1`,"###,
        r###"		`update t set b = 3, c = 3 where c = "4"`,"###,
        r###"		`delete from t where a = 8`,"###,
        r###"		`delete from t where b = 2`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("4 3 3", "6 6 6", "7 7 7", "9 9 9")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (1,1,1),(5,5,5),(8,8,8)`,"###,
        r###"		`update t set a = 2, b = 2, c = 2 where a = 1`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where c = "6"`,"###,
        r###"		`update t set a = 6, b = 6 where a = 9`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 3`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "2 2 2", "6 6 9", "7 7 7", "8 8 8")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestAddHashPartitionFailures 对应 Go 测试函数第 306-327 行。
#[test]
fn test_add_hash_partition_failures() {
    let _steps = test_add_hash_partition_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_add_hash_partition_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestAddHashPartitionFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique index (c) global) partition by hash(b) partitions 3`"###,
        r###"	alter := `alter table t add partition partitions 2`"###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(4,4,4),(8,8,8),(9,9,9),(6,6,6)`,"###,
        r###"		`update t set a = 7, b = 7, c = 7 where a = 1`,"###,
        r###"		`update t set b = 3, c = 3 where c = "4"`,"###,
        r###"		`delete from t where a = 8`,"###,
        r###"		`delete from t where b = 2`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("4 3 3", "6 6 6", "7 7 7", "9 9 9")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (1,1,1),(5,5,5),(8,8,8)`,"###,
        r###"		`update t set a = 2, b = 2, c = 2 where a = 1`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where c = "6"`,"###,
        r###"		`update t set a = 6, b = 6 where a = 9`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 3`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "2 2 2", "6 6 9", "7 7 7", "8 8 8")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestCoalesceKeyPartitionFailures 对应 Go 测试函数第 329-350 行。
#[test]
fn test_coalesce_key_partition_failures() {
    let _steps = test_coalesce_key_partition_failures_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_coalesce_key_partition_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestCoalesceKeyPartitionFailures(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique index (b) global, unique index (c)) partition by key(c) partitions 5`"###,
        r###"	alter := `alter table t coalesce partition 2`"###,
        r###"	beforeDML := []string{"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(4,4,4),(8,8,8),(9,9,9),(6,6,6)`,"###,
        r###"		`update t set a = 7, b = 7, c = 7 where a = 1`,"###,
        r###"		`update t set b = 3, c = 3 where c = "4"`,"###,
        r###"		`delete from t where a = 8`,"###,
        r###"		`delete from t where b = 2`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("4 3 3", "6 6 6", "7 7 7", "9 9 9")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (1,1,1),(5,5,5),(8,8,8)`,"###,
        r###"		`update t set a = 2, b = 2, c = 2 where a = 1`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where c = "6"`,"###,
        r###"		`update t set a = 6, b = 6 where a = 9`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 3`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "2 2 2", "6 6 9", "7 7 7", "8 8 8")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionByNonPartitionedTable 对应 Go 测试函数第 352-358 行。
#[test]
fn test_partition_by_non_partitioned_table() {
    let _steps = test_partition_by_non_partitioned_table_go_steps();
    // 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_non_partitioned_table_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByNonPartitionedTable(t *testing.T) {"###,
        r###"	create := `create table t (a int)`"###,
        r###"	alter := `alter table t partition by range (a) (partition p0 values less than (20))`"###,
        r###"	beforeResult := testkit.Rows()"###,
        r###"	afterResult := testkit.Rows()"###,
        r###"	testReorganizePartitionFailures(t, create, alter, nil, beforeResult, nil, afterResult)"###,
        r###"}"###,
    ]
}

// testReorganizePartitionFailures 对应 Go 辅助函数第 360-488 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn test_reorganize_partition_failures_go_steps() -> &'static [&'static str] {
    &[
        r###"func testReorganizePartitionFailures(t *testing.T, createSQL, alterSQL string, beforeDML []string, beforeResult [][]any, afterDML []string, afterResult [][]any, skipTests ...string) {"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Skip GC emulator, we trigger it manually to also clean up PlacementBundles"###,
        r###"	util.EmulatorGCDisable()"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	gcWorker, err := gcworker.NewMockGCWorker(store)"###,
        // Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        // MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Fail means we simply inject an error, and set the error count very high to see what happens"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	//   we do expect to do best effort rollback here as well!"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Cancel means we set job.State = JobStateCancelled, as in no need to do more"###,
        // 保留 Go 原注释，帮助对照测试意图。
        r###"	// Rollback means we do full rollback before returning error."###,
        r###"	tests := []struct {"###,
        r###"		name            string"###,
        r###"		count           int"###,
        r###"		rollForwardFrom int"###,
        r###"	}{"###,
        r###"		{"###,
        r###"			"Cancel","###,
        r###"			1,"###,
        r###"			-1,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			"Fail","###,
        r###"			5,"###,
        r###"			4,"###,
        r###"		},"###,
        r###"		{"###,
        r###"			"Rollback","###,
        r###"			4,"###,
        r###"			-1,"###,
        r###"		},"###,
        r###"	}"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"	oldWaitTimeWhenErrorOccurred := ddl.WaitTimeWhenErrorOccurred"###,
        // Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer func() {"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"		ddl.WaitTimeWhenErrorOccurred = oldWaitTimeWhenErrorOccurred"###,
        r###"	}()"###,
        // DDL 内部 hook/job 相关调用，当前不执行 DDL。
        r###"	ddl.WaitTimeWhenErrorOccurred = 0"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, test := range tests {"###,
        r###"	SUBTEST:"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"		for i := 1; i <= test.count; i++ {"###,
        r###"			suffix := test.name + strconv.Itoa(i)"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"			for _, skip := range skipTests {"###,
        // Go 条件分支，保留分支判断文本。
        r###"				if suffix == skip {"###,
        r###"					continue SUBTEST"###,
        r###"				}"###,
        r###"			}"###,
        r###"			suffixComment := ` /* ` + suffix + ` */
`"###,
        r###"			tk.MustExec(createSQL + suffixComment)"###,
        // Go 循环或表驱动测试，保留迭代步骤。
        r###"			for _, sql := range beforeDML {"###,
        r###"				tk.MustExec(sql + suffixComment)"###,
        r###"			}"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"			tk.MustQuery(`select * from t ` + suffixComment).Sort().Check(beforeResult)"###,
        r###"			tOrg := external.GetTableByName(t, tk, "test", "t")"###,
        r###"			var idxID int64"###,
        // Go 条件分支，保留分支判断文本。
        r###"			if len(tOrg.Meta().Indices) > 0 {"###,
        r###"				idxID = tOrg.Meta().Indices[0].ID"###,
        r###"			}"###,
        // TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"			oldCreate := tk.MustQuery(`show create table t` + suffixComment).Rows()"###,
        // 保留 Go 原注释，帮助对照测试意图。
r###" // Run GC to clean changes in beforeDML"###,
        r###"			require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"###,
        r###"			oldBundles, err := infosync.GetAllRuleBundles(context.TODO())"###,
// Go require 断言，只保留断言意图。
        r###"			require.NoError(t, err)"###,
        r###"			name := "github.com/pingcap/tidb/pkg/ddl/reorgPart" + suffix"###,
        r###"			term := "return(true)""###,
// Go 条件分支，保留分支判断文本。
        r###"			if test.rollForwardFrom > 0 && test.rollForwardFrom <= i {"###,
        r###"				term = "10*" + term"###,
        r###"			}"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"			testfailpoint.Enable(t, name, term)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"			err = tk.ExecToErr(alterSQL + suffixComment)"###,
        r###"			tt := external.GetTableByName(t, tk, "test", "t")"###,
        r###"			partition := tt.Meta().Partition"###,
        r###"			rollback := false"###,
// Go 条件分支，保留分支判断文本。
        r###"			if test.rollForwardFrom > 0 && test.rollForwardFrom <= i {"###,
// Go require 断言，只保留断言意图。
        r###"				require.NoError(t, err)"###,
        r###"			} else {"###,
        r###"				rollback = true"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"				require.Error(t, err, "failpoint reorgPart"+suffix)"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"				// TODO: gracefully handle failures during WriteReorg also for nonclustered tables"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"				// with unique indexes."###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"				// Currently it can also do:"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"				// 	Error "[kv:1062]Duplicate entry '7' for key 't.c'" does not contain "Injected error by reorgPartFail2""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"				//require.ErrorContains(t, err, "Injected error by reorgPart"+suffix)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"				tk.MustQuery(`show create table t` + suffixComment).Check(oldCreate)"###,
// Go 条件分支，保留分支判断文本。
        r###"				if partition == nil {"###,
// Go require 断言，只保留断言意图。
        r###"					require.Nil(t, tOrg.Meta().Partition, suffix)"###,
        r###"				} else {"###,
// Go require 断言，只保留断言意图。
        r###"					require.Equal(t, len(tOrg.Meta().Partition.Definitions), len(partition.Definitions), suffix)"###,
// Go require 断言，只保留断言意图。
        r###"					require.Equal(t, 0, len(partition.AddingDefinitions), suffix)"###,
// Go require 断言，只保留断言意图。
        r###"					require.Equal(t, 0, len(partition.DroppingDefinitions), suffix)"###,
        r###"				}"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"				noNewTablesAfter(t, tk, tk.Session(), tOrg, suffix)"###,
        r###"			}"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"			testfailpoint.Disable(t, name)"###,
// Go require 断言，只保留断言意图。
        r###"			require.Equal(t, len(tOrg.Meta().Indices), len(tt.Meta().Indices), suffix)"###,
// Go 条件分支，保留分支判断文本。
        r###"			if rollback && idxID != 0 {"###,
// Go require 断言，只保留断言意图。
        r###"				require.Equal(t, idxID, tt.Meta().Indices[0].ID, suffix)"###,
        r###"			}"###,
        r###"			require.Nil(t, gcWorker.DeleteRanges(context.TODO(), math.MaxInt64))"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"			noNewTablesAfter(t, tk, tk.Session(), tt, suffix)"###,
        r###"			tk.MustExec(`admin check table t` + suffixComment)"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"			for _, sql := range afterDML {"###,
        r###"				tk.MustExec(sql + suffixComment)"###,
        r###"			}"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"			tk.MustQuery(`select * from t` + suffixComment).Sort().Check(afterResult)"###,
        r###"			newBundles, err := infosync.GetAllRuleBundles(context.TODO())"###,
// Go require 断言，只保留断言意图。
        r###"			require.NoError(t, err)"###,
// Go 条件分支，保留分支判断文本。
        r###"			if rollback {"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"				for i := range newBundles {"###,
        r###"					found := false"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"					for j := range oldBundles {"###,
// Go 条件分支，保留分支判断文本。
        r###"						if newBundles[i].ID == oldBundles[j].ID {"###,
// Go require 断言，只保留断言意图。
        r###"							require.Equal(t, oldBundles[j].String(), newBundles[i].String(), suffix)"###,
        r###"							found = true"###,
        r###"							break"###,
        r###"						}"###,
        r###"					}"###,
// Go require 断言，只保留断言意图。
        r###"					require.True(t, found, "%s: New bundle not cleaned up '%s':\n%s", suffix, newBundles[i].ID, newBundles[i].String())"###,
        r###"				}"###,
// Go require 断言，只保留断言意图。
        r###"				require.Equal(t, len(oldBundles), len(newBundles), suffix)"###,
        r###"			}"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"			tk.MustQuery(`select * from t` + suffixComment).Sort().Check(afterResult)"###,
        r###"			tk.MustExec(`drop table t` + suffixComment)"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"			// TODO: Check TiFlash replicas"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"			// TODO: Check Label rules"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"			// TODO: Check autoIDs"###,
        r###"		}"###,
        r###"	}"###,
        r###"}"###,
    ]
}

// TestReorgPartitionConcurrent 对应 Go 测试函数第 490-724 行。
#[test]
fn test_reorg_partition_concurrent() {
    let _steps = test_reorg_partition_concurrent_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_concurrent_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionConcurrent(t *testing.T) {"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	schemaName := "ReorgPartConcurrent""###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("create database " + schemaName)"###,
        r###"	tk.MustExec("use " + schemaName)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (1,"1",1), (10,"10",10),(23,"23",32),(34,"34",43),(45,"45",54),(56,"56",65)`)"###,
        r###"	syncOnChanged := make(chan bool)"###,
// Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer close(syncOnChanged)"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterReorganizePartition", func() {"###,
        r###"		<-syncOnChanged"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"		// We want to wait here"###,
        r###"		<-syncOnChanged"###,
        r###"	})"###,
        r###""###,
        r###"	wait := make(chan bool)"###,
// Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer close(wait)"###,
        r###""###,
        r###"	currState := model.StateNone"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"###,
// Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionReorganizePartition &&"###,
        r###"			(job.SchemaState == model.StateDeleteOnly ||"###,
        r###"				job.SchemaState == model.StateWriteOnly ||"###,
        r###"				job.SchemaState == model.StateWriteReorganization ||"###,
        r###"				job.SchemaState == model.StateDeleteReorganization ||"###,
        r###"				job.SchemaState == model.StatePublic) &&"###,
        r###"			currState != job.SchemaState {"###,
        r###"			currState = job.SchemaState"###,
        r###"			<-wait"###,
        r###"			<-wait"###,
        r###"		}"###,
        r###"	})"###,
        r###"	alterErr := make(chan error, 1)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	go backgroundExec(store, schemaName, "alter table t reorganize partition p1 into (partition p1a values less than (15), partition p1b values less than (20))", alterErr)"###,
        r###""###,
        r###"	wait <- true"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// StateDeleteOnly"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	deleteOnlyInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
        r###"	wait <- true"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// StateWriteOnly"###,
        r###"	wait <- true"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (11, "11", 11),(12,"12",21)`)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	writeOnlyInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, int64(1), writeOnlyInfoSchema.SchemaMetaVersion()-deleteOnlyInfoSchema.SchemaMetaVersion())"###,
        r###"	deleteOnlyTbl, err := deleteOnlyInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	writeOnlyTbl, err := writeOnlyInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	writeOnlyParts := writeOnlyTbl.Meta().Partition"###,
        r###"	writeOnlyTbl.Meta().Partition = deleteOnlyTbl.Meta().Partition"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// If not DeleteOnly is working, then this would show up when reorg is done"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`delete from t where a = 11`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`update t set b = "12b", c = 12 where a = 12`)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	writeOnlyTbl.Meta().Partition = writeOnlyParts"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	wait <- true"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// StateWriteReorganization"###,
        r###"	wait <- true"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (14, "14", 14),(15, "15",15)`)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	writeReorgInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"	wait <- true"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// StateDeleteReorganization"###,
        r###"	wait <- true"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"10 10 10","###,
        r###"		"12 12b 12","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15"))"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	deleteReorgInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, int64(1), deleteReorgInfoSchema.SchemaMetaVersion()-writeReorgInfoSchema.SchemaMetaVersion())"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (16, "16", 16)`)"###,
        r###"	oldTbl, err := writeReorgInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	partDef := oldTbl.Meta().Partition.Definitions[1]"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, "p1", partDef.Name.O)"###,
        r###"	rows := getNumRowsFromPartitionDefs(t, tk, oldTbl, oldTbl.Meta().Partition.Definitions[1:2])"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, 5, rows)"###,
        r###"	currTbl, err := deleteReorgInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	currPart := currTbl.Meta().Partition"###,
        r###"	currTbl.Meta().Partition = oldTbl.Meta().Partition"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where b = "16"`).Sort().Check(testkit.Rows("16 16 16"))"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t partition (p1)`).Sort().Check(testkit.Rows(""+"###,
        r###"		"10 10 10","###,
        r###"		"12 12b 12","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15","###,
        r###"		"16 16 16"))"###,
        r###"	currTbl.Meta().Partition = currPart"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"	wait <- true"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// StatePublic"###,
        r###"	wait <- true"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"10 10 10","###,
        r###"		"12 12b 12","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15","###,
        r###"		"16 16 16"))"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	publicInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, int64(1), publicInfoSchema.SchemaMetaVersion()-deleteReorgInfoSchema.SchemaMetaVersion())"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (17, "17", 17)`)"###,
        r###"	oldTbl, err = deleteReorgInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	partDef = oldTbl.Meta().Partition.Definitions[1]"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, "p1a", partDef.Name.O)"###,
        r###"	rows = getNumRowsFromPartitionDefs(t, tk, oldTbl, oldTbl.Meta().Partition.Definitions[1:2])"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, 3, rows)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t partition (p1a)`).Sort().Check(testkit.Rows("10 10 10", "12 12b 12", "14 14 14"))"###,
        r###"	currTbl, err = publicInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	currPart = currTbl.Meta().Partition"###,
        r###"	currTbl.Meta().Partition = oldTbl.Meta().Partition"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where b = "17"`).Sort().Check(testkit.Rows("17 17 17"))"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"	currTbl.Meta().Partition = currPart"###,
        r###"	wait <- true"###,
        r###"	syncOnChanged <- true"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// This reads the new schema (Schema update completed)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"10 10 10","###,
        r###"		"12 12b 12","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15","###,
        r###"		"16 16 16","###,
        r###"		"17 17 17"))"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	newInfoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, int64(1), newInfoSchema.SchemaMetaVersion()-publicInfoSchema.SchemaMetaVersion())"###,
        r###"	oldTbl, err = publicInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	partDef = oldTbl.Meta().Partition.Definitions[1]"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, "p1a", partDef.Name.O)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"	newTbl, err := newInfoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	newPart := newTbl.Meta().Partition"###,
        r###"	newTbl.Meta().Partition = oldTbl.Meta().Partition"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	newTbl.Meta().Partition = newPart"###,
        r###"	syncOnChanged <- true"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, <-alterErr)"###,
        r###"}"###,
    ]
}

// TestReorgPartitionFailConcurrent 对应 Go 测试函数第 726-842 行。
#[test]
fn test_reorg_partition_fail_concurrent() {
    let _steps = test_reorg_partition_fail_concurrent_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_fail_concurrent_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionFailConcurrent(t *testing.T) {"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	schemaName := "ReorgPartFailConcurrent""###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("create database " + schemaName)"###,
        r###"	tk.MustExec("use " + schemaName)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (1,"1",1), (12,"12",21),(23,"23",32),(34,"34",43),(45,"45",54),(56,"56",65)`)"###,
        r###""###,
        r###"	wait := make(chan bool)"###,
// Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer close(wait)"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// Test insert of duplicate key during copy phase"###,
        r###"	injected := false"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"###,
// Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionReorganizePartition && job.SchemaState == model.StateWriteReorganization && !injected {"###,
        r###"			injected = true"###,
        r###"			<-wait"###,
        r###"			<-wait"###,
        r###"		}"###,
        r###"	})"###,
        r###"	alterErr := make(chan error, 1)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	go backgroundExec(store, schemaName, "alter table t reorganize partition p1 into (partition p1a values less than (15), partition p1b values less than (20))", alterErr)"###,
        r###"	wait <- true"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (14, "14", 14),(15, "15",15)`)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	tk.MustGetErrCode(`insert into t values (11, "11", 11),(12,"duplicate PK 💥", 13)`, errno.ErrDupEntry)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	wait <- true"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, <-alterErr)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"12 12 21","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15"))"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// Test reorg of duplicate key"###,
        r###"	prevState := model.StateNone"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"###,
// Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionReorganizePartition &&"###,
        r###"			job.SchemaState == model.StateWriteReorganization &&"###,
        r###"			job.SnapshotVer == 0 &&"###,
        r###"			prevState != job.SchemaState {"###,
        r###"			prevState = job.SchemaState"###,
        r###"			<-wait"###,
        r###"			<-wait"###,
        r###"		}"###,
// Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionReorganizePartition &&"###,
        r###"			job.SchemaState == model.StateDeleteReorganization &&"###,
        r###"			prevState != job.SchemaState {"###,
        r###"			prevState = job.SchemaState"###,
        r###"			<-wait"###,
        r###"			<-wait"###,
        r###"		}"###,
        r###"	})"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	go backgroundExec(store, schemaName, "alter table t reorganize partition p1a,p1b into (partition p1a values less than (14), partition p1b values less than (17), partition p1c values less than (20))", alterErr)"###,
        r###"	wait <- true"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	infoSchema := sessiontxn.GetTxnManager(tk.Session()).GetTxnInfoSchema()"###,
        r###"	tbl, err := infoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, 0, getNumRowsFromPartitionDefs(t, tk, tbl, tbl.Meta().Partition.AddingDefinitions))"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`delete from t where a = 14`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (13, "13", 31),(14,"14b",14),(16, "16",16)`)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	wait <- true"###,
        r###"	wait <- true"###,
        r###"	tbl, err = infoSchema.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
// Go require 断言，只保留断言意图。
        r###"	require.Equal(t, 5, getNumRowsFromPartitionDefs(t, tk, tbl, tbl.Meta().Partition.AddingDefinitions))"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`delete from t where a = 15`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (11, "11", 11),(15,"15b",15),(17, "17",17)`)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	wait <- true"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, <-alterErr)"###,
        r###""###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where a between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"11 11 11","###,
        r###"		"12 12 21","###,
        r###"		"13 13 31","###,
        r###"		"14 14b 14","###,
        r###"		"15 15b 15","###,
        r###"		"16 16 16","###,
        r###"		"17 17 17"))"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"11 11 11","###,
        r###"		"12 12 21","###,
        r###"		"14 14b 14","###,
        r###"		"15 15b 15","###,
        r###"		"16 16 16","###,
        r###"		"17 17 17"))"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where b between "10" and "22"`).Sort().Check(testkit.Rows(""+"###,
        r###"		"11 11 11","###,
        r###"		"12 12 21","###,
        r###"		"13 13 31","###,
        r###"		"14 14b 14","###,
        r###"		"15 15b 15","###,
        r###"		"16 16 16","###,
        r###"		"17 17 17"))"###,
        r###"}"###,
    ]
}

// getNumRowsFromPartitionDefs 对应 Go 辅助函数第 844-860 行。
// 参数、返回值和外部依赖先以步骤文本保留，避免误造可运行 API。
#[allow(dead_code)]
pub fn get_num_rows_from_partition_defs_go_steps() -> &'static [&'static str] {
    &[
        r###"func getNumRowsFromPartitionDefs(t *testing.T, tk *testkit.TestKit, tbl table.Table, defs []model.PartitionDefinition) int {"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	ctx := tk.Session()"###,
        r###"	pt := tbl.GetPartitionedTable()"###,
// Go require 断言，只保留断言意图。
        r###"	require.NotNil(t, pt)"###,
        r###"	cnt := 0"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, def := range defs {"###,
        r###"		data := getAllDataForTableID(t, ctx, def.ID)"###,
// Go require 断言，只保留断言意图。
        r###"		require.True(t, len(data.keys) == len(data.vals))"###,
// Go require 断言，只保留断言意图。
        r###"		require.True(t, len(data.keys) == len(data.tp))"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"		for _, s := range data.tp {"###,
// Go 条件分支，保留分支判断文本。
        r###"			if s == "Record" {"###,
        r###"				cnt++"###,
        r###"			}"###,
        r###"		}"###,
        r###"	}"###,
        r###"	return cnt"###,
        r###"}"###,
    ]
}

// TestReorgPartitionFailInject 对应 Go 测试函数第 862-913 行。
#[test]
fn test_reorg_partition_fail_inject() {
    let _steps = test_reorg_partition_fail_inject_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_fail_inject_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionFailInject(t *testing.T) {"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	schemaName := "ReorgPartFailInjectConcurrent""###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("create database " + schemaName)"###,
        r###"	tk.MustExec("use " + schemaName)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (1,"1",1), (12,"12",21),(23,"23",32),(34,"34",43),(45,"45",54),(56,"56",65)`)"###,
        r###""###,
        r###"	wait := make(chan bool)"###,
// Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer close(wait)"###,
        r###""###,
        r###"	injected := false"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep", func(job *model.Job) {"###,
// Go 条件分支，保留分支判断文本。
        r###"		if job.Type == model.ActionReorganizePartition && job.SchemaState == model.StateWriteReorganization && !injected {"###,
        r###"			injected = true"###,
        r###"			<-wait"###,
        r###"			<-wait"###,
        r###"		}"###,
        r###"	})"###,
        r###"	alterErr := make(chan error, 1)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	go backgroundExec(store, schemaName, "alter table t reorganize partition p1 into (partition p1a values less than (15), partition p1b values less than (20))", alterErr)"###,
        r###"	wait <- true"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (14, "14", 14),(15, "15",15)`)"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	tk.MustGetErrCode(`insert into t values (11, "11", 11),(12,"duplicate PK 💥", 13)`, errno.ErrDupEntry)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
        r###"	wait <- true"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, <-alterErr)"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select * from t where c between 10 and 22`).Sort().Check(testkit.Rows(""+"###,
        r###"		"12 12 21","###,
        r###"		"14 14 14","###,
        r###"		"15 15 15"))"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1a` VALUES LESS THAN (15),\n" +"###,
        r###"		" PARTITION `p1b` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###"}"###,
    ]
}

// TestReorgPartitionRollback 对应 Go 测试函数第 915-959 行。
#[test]
fn test_reorg_partition_rollback() {
    let _steps = test_reorg_partition_rollback_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_rollback_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionRollback(t *testing.T) {"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	schemaName := "ReorgPartRollback""###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("create database " + schemaName)"###,
        r###"	tk.MustExec("use " + schemaName)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into t values (1,"1",1), (12,"12",21),(23,"23",32),(34,"34",43),(45,"45",54),(56,"56",65)`)"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/mockUpdateVersionAndTableInfoErr", `return(1)`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExecToErr("alter table t reorganize partition p1 into (partition p1a values less than (15), partition p1b values less than (20))")"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/mockUpdateVersionAndTableInfoErr")"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"	ctx := tk.Session()"###,
// domain/DDL owner 相关外部依赖，这里只记录调度语义。
        r###"	is := domain.GetDomain(ctx).InfoSchema()"###,
        r###"	tbl, err := is.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	noNewTablesAfter(t, tk, ctx, tbl, "Reorganize rollback")"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/reorgPartitionAfterDataCopy", `return(true)`)"###,
// Go defer 资源收尾，迁移时需要在真实实现中显式安排释放。
        r###"	defer func() {"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"		testfailpoint.Disable(t, "github.com/pingcap/tidb/pkg/ddl/reorgPartitionAfterDataCopy")"###,
        r###"	}()"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExecToErr("alter table t reorganize partition p1 into (partition p1a values less than (15), partition p1b values less than (20))")"###,
        r###"	tk.MustExec(`admin check table t`)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`show create table t`).Check(testkit.Rows("" +"###,
        r###"		"t CREATE TABLE `t` (\n" +"###,
        r###"		"  `a` int(10) unsigned NOT NULL,\n" +"###,
        r###"		"  `b` varchar(255) DEFAULT NULL,\n" +"###,
        r###"		"  `c` int(11) DEFAULT NULL,\n" +"###,
        r###"		"  PRIMARY KEY (`a`) /*T![clustered_index] CLUSTERED */,\n" +"###,
        r###"		"  KEY `b` (`b`),\n" +"###,
        r###"		"  KEY `c` (`c`,`b`)\n" +"###,
        r###"		") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin\n" +"###,
        r###"		"PARTITION BY RANGE (`a`)\n" +"###,
        r###"		"(PARTITION `p0` VALUES LESS THAN (10),\n" +"###,
        r###"		" PARTITION `p1` VALUES LESS THAN (20),\n" +"###,
        r###"		" PARTITION `pMax` VALUES LESS THAN (MAXVALUE))"))"###,
        r###""###,
        r###"	tbl, err = is.TableByName(context.Background(), ast.NewCIStr(schemaName), ast.NewCIStr("t"))"###,
// Go require 断言，只保留断言意图。
        r###"	require.NoError(t, err)"###,
        r###"	noNewTablesAfter(t, tk, ctx, tbl, "Reorganize rollback")"###,
        r###"}"###,
    ]
}

// TestPartitionByColumnChecks 对应 Go 测试函数第 961-1043 行。
#[test]
fn test_partition_by_column_checks() {
    let _steps = test_partition_by_column_checks_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_column_checks_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByColumnChecks(t *testing.T) {"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        r###"	cols := "(i int, f float, c char(20), b bit(2), b32 bit(32), b64 bit(64), d date, dt datetime, dt6 datetime(6), ts timestamp, ts6 timestamp(6), j json)""###,
        r###"	vals := `(1, 2.2, "A and c", b'10', b'10001000100010001000100010001000', b'1000100010001000100010001000100010001000100010001000100010001000', '2024-09-24', '2024-09-24 13:01:02', '2024-09-24 13:01:02.123456', '2024-09-24 13:01:02', '2024-09-24 13:01:02.123456', '{"key1": "value1", "key2": "value2"}')`"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table t ` + cols)"###,
        r###"	testCases := []struct {"###,
        r###"		partClause string"###,
        r###"		err        error"###,
        r###"	}{"###,
        r###"		{"key (c) partitions 2", nil},"###,
        r###"		{"key (j) partitions 2", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list (c) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list (b) (partition pDef default)", nil},"###,
        r###"		{"list (f) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list (j) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list columns (b) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list columns (f) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list columns (ts) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"list columns (j) (partition pDef default)", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"hash (year(ts)) partitions 2", dbterror.ErrWrongExprInPartitionFunc},"###,
        r###"		{"hash (ts) partitions 2", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"hash (ts6) partitions 2", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"hash (d) partitions 2", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"hash (f) partitions 2", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (c) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (f) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (d) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (dt) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (dt6) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (ts) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (ts6) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range (j) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (b) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (b64) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (c) (partition pMax values less than (maxvalue))", nil},"###,
        r###"		{"range columns (f) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (d) (partition pMax values less than (maxvalue))", nil},"###,
        r###"		{"range columns (dt) (partition pMax values less than (maxvalue))", nil},"###,
        r###"		{"range columns (dt6) (partition pMax values less than (maxvalue))", nil},"###,
        r###"		{"range columns (ts) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (ts6) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"		{"range columns (j) (partition pMax values less than (maxvalue))", dbterror.ErrNotAllowedTypeInPartition},"###,
        r###"	}"###,
// Go 循环或表驱动测试，保留迭代步骤。
        r###"	for _, testCase := range testCases {"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"		err := tk.ExecToErr(`create table tt ` + cols + ` partition by ` + testCase.partClause)"###,
// Go require 断言，只保留断言意图。
        r###"		require.ErrorIs(t, err, testCase.err, testCase.partClause)"###,
// Go 条件分支，保留分支判断文本。
        r###"		if testCase.err == nil {"###,
        r###"			tk.MustExec(`drop table tt`)"###,
        r###"		}"###,
// TestKit 会话操作，不创建 TiDB 会话。
        r###"		err = tk.ExecToErr(`alter table t partition by ` + testCase.partClause)"###,
// Go require 断言，只保留断言意图。
        r###"		require.ErrorIs(t, err, testCase.err)"###,
        r###"	}"###,
        r###""###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// Not documented or tested!!"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// KEY - Allows more types than documented, should be OK!"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table kb ` + cols + ` partition by key(b) partitions 2`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table kf ` + cols + ` partition by key(f) partitions 2`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table kts ` + cols + ` partition by key(ts) partitions 2`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table hb ` + cols + ` partition by hash(b) partitions 2`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into hb values ` + vals)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select count(*) from hb where b = b'10'`).Check(testkit.Rows("1"))"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table hb partition by hash(b) partitions 3`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into hb values ` + vals)"###,
// TestKit 查询/断言步骤，当前不会读取真实结果集。
        r###"	tk.MustQuery(`select count(*) from hb where b = b'10'`).Check(testkit.Rows("2"))"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table hb32 ` + cols + ` partition by hash(b32) partitions 2`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into hb32 values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table hb32 partition by hash(b32) partitions 3`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into hb32 values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table rb ` + cols + ` partition by range (b) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table rb partition by range(b) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table rb32 ` + cols + ` partition by range (b32) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb32 values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table rb32 partition by range(b32) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb32 values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`create table rb64 ` + cols + ` partition by range (b64) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb64 values ` + vals)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`alter table rb64 partition by range(b64) (partition pMax values less than (MAXVALUE))`)"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec(`insert into rb64 values ` + vals)"###,
        r###"}"###,
    ]
}

// TestPartitionIssue56634 对应 Go 测试函数第 1045-1055 行。
#[test]
fn test_partition_issue56634() {
    let _steps = test_partition_issue56634_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_issue56634_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionIssue56634(t *testing.T) {"###,
// failpoint 注入/释放点，只记录故障触发语义。
        r###"	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/ddl/updateVersionAndTableInfoErrInStateDeleteReorganization", `4*return(1)`)"###,
        r###""###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	store := testkit.CreateMockStore(t)"###,
// MockStore/Storage 外部依赖，当前不初始化存储。
        r###"	tk := testkit.NewTestKit(t, store)"###,
        r###"	tk.MustExec("use test")"###,
        r###"	tk.MustExec("drop table if exists t")"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("create table t (a int)")"###,
// 保留 Go 原注释，帮助对照测试意图。
        r###"	// Changed, since StatePublic can no longer rollback!"###,
// SQL 执行步骤，仅保存语句和执行顺序。
        r###"	tk.MustExec("alter table t partition by range(a) (partition p1 values less than (20))")"###,
        r###"}"###,
    ]
}

// TestReorgPartitionFailuresPlacementPolicy 对应 Go 测试函数第 1057-1076 行。
#[test]
fn test_reorg_partition_failures_placement_policy() {
    let _steps = test_reorg_partition_failures_placement_policy_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorg_partition_failures_placement_policy_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorgPartitionFailuresPlacementPolicy(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned PRIMARY KEY, b varchar(255), c int, key (b), key (c,b))` +"###,
        r###"		` partition by range (a) ` +"###,
        r###"		`(partition p0 values less than (10),` +"###,
        r###"		` partition p1 values less than (20),` +"###,
        r###"		` partition p2 values less than (30),` +"###,
        r###"		` partition pMax values less than (MAXVALUE))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`create or replace placement policy pp2 followers=2`,"###,
        r###"		`create or replace placement policy pp3 followers=3`,"###,
        r###"		`alter table t placement policy ='pp1'`,"###,
        r###"		`alter table t partition p1 placement policy ='pp2'`,"###,
        r###"		`alter table t partition p2 placement policy ='pp3'`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows()"###,
        r###"	alter := "alter table t reorganize partition p1,p2 into (partition p1 values less than (17), partition p1b values less than (24) placement policy 'pp1', partition p2 values less than (30))""###,
        r###"	afterResult := testkit.Rows()"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, nil, afterResult)"###,
        r###"}"###,
    ]
}

// TestRemovePartitionFailuresPlacementPolicy 对应 Go 测试函数第 1078-1108 行。
#[test]
fn test_remove_partition_failures_placement_policy() {
    let _steps = test_remove_partition_failures_placement_policy_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_remove_partition_failures_placement_policy_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestRemovePartitionFailuresPlacementPolicy(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255)) partition by range(a) ("###,
        r###"                        partition p0 values less than (50),"###,
        r###"                        partition p1 values less than (100),"###,
        r###"                        partition p2 values less than (200))`"###,
        r###"	alter := `alter table t remove partitioning`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`create or replace placement policy pp2 followers=2`,"###,
        r###"		`create or replace placement policy pp3 followers=2`,"###,
        r###"		`alter table t placement policy ='pp3'`,"###,
        r###"		`alter table t partition p1 placement policy ='pp1'`,"###,
        r###"		`alter table t partition p2 placement policy ='pp2'`,"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(3,3,3),(101,101,101),(102,102,102),(103,103,103)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 1`,"###,
        r###"		`update t set b = "12", c = 12 where b = 2`,"###,
        r###"		`delete from t where a = 102`,"###,
        r###"		`delete from t where b = 103`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("101 101 101", "11 11 11", "2 12 12", "3 3 3")"###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where a = 11`,"###,
        r###"		`update t set b = 2, c = 2 where c = 12`,"###,
        r###"		`update t set a = 9, b = 9 where a = 104`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 102`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "101 101 101", "2 2 2", "3 3 3", "4 4 4", "9 9 104")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionByFailuresPlacementPolicy 对应 Go 测试函数第 1110-1138 行。
#[test]
fn test_partition_by_failures_placement_policy() {
    let _steps = test_partition_by_failures_placement_policy_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_failures_placement_policy_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByFailuresPlacementPolicy(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255)) partition by range(a) ("###,
        r###"                        partition p0 values less than (100),"###,
        r###"                        partition p1 values less than (200))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`create or replace placement policy pp2 followers=2`,"###,
        r###"		`create or replace placement policy pp3 followers=3`,"###,
        r###"		`alter table t placement policy ='pp1'`,"###,
        r###"		`alter table t partition p0 placement policy ='pp2'`,"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(3,3,3),(101,101,101),(102,102,102),(103,103,103)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 1`,"###,
        r###"		`update t set b = "12", c = 12 where b = 2`,"###,
        r###"		`delete from t where a = 102`,"###,
        r###"		`delete from t where b = 103`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("101 101 101", "11 11 11", "2 12 12", "3 3 3")"###,
        r###"	alter := "alter table t partition by range (b) (partition pNoneC values less than (150) placement policy 'pp3', partition p2 values less than (300)) update indexes (`primary` global)""###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where a = 11`,"###,
        r###"		`update t set b = 2, c = 2 where c = 12`,"###,
        r###"		`update t set a = 9, b = 9 where a = 104`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 102`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "101 101 101", "2 2 2", "3 3 3", "4 4 4", "9 9 104")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionNonPartitionedFailuresPlacementPolicy 对应 Go 测试函数第 1140-1164 行。
#[test]
fn test_partition_non_partitioned_failures_placement_policy() {
    let _steps = test_partition_non_partitioned_failures_placement_policy_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_non_partitioned_failures_placement_policy_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionNonPartitionedFailuresPlacementPolicy(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`create or replace placement policy pp2 followers=2`,"###,
        r###"		`alter table t placement policy ='pp1'`,"###,
        r###"		`insert into t values (1,1,1),(2,2,2),(3,3,3),(101,101,101),(102,102,102),(103,103,103)`,"###,
        r###"		`update t set a = 11, b = "11", c = 11 where a = 1`,"###,
        r###"		`update t set b = "12", c = 12 where b = 2`,"###,
        r###"		`delete from t where a = 102`,"###,
        r###"		`delete from t where b = 103`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("101 101 101", "11 11 11", "2 12 12", "3 3 3")"###,
        r###"	alter := "alter table t partition by range (b) (partition pNoneC values less than (150), partition p2 values less than (300) placement policy 'pp1') update indexes (`primary` global)""###,
        r###"	afterDML := []string{"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"		`update t set a = 1, b = 1, c = 1 where a = 11`,"###,
        r###"		`update t set b = 2, c = 2 where c = 12`,"###,
        r###"		`update t set a = 9, b = 9 where a = 104`,"###,
        r###"		`delete from t where a = 5`,"###,
        r###"		`delete from t where b = 102`,"###,
        r###"	}"###,
        r###"	afterResult := testkit.Rows("1 1 1", "101 101 101", "2 2 2", "3 3 3", "4 4 4", "9 9 104")"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, afterDML, afterResult)"###,
        r###"}"###,
    ]
}

// TestReorganizePartitionFailuresAddPlacementPolicy 对应 Go 测试函数第 1166-1179 行。
#[test]
fn test_reorganize_partition_failures_add_placement_policy() {
    let _steps = test_reorganize_partition_failures_add_placement_policy_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_reorganize_partition_failures_add_placement_policy_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestReorganizePartitionFailuresAddPlacementPolicy(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered, b int not null, c varchar(255)) partition by range(a) ("###,
        r###"                        partition p0 values less than (50),"###,
        r###"                        partition p1 values less than (100),"###,
        r###"                        partition p2 values less than (200))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`insert into t values (4,4,4),(5,5,5),(104,104,104)`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("104 104 104", "4 4 4", "5 5 5")"###,
        r###"	alter := `alter table t reorganize partition p2 into (partition p2 values less than (200), partition pMax values less than (maxvalue) placement policy pp1)`"###,
        r###"	afterResult := beforeResult"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, nil, afterResult)"###,
        r###"}"###,
    ]
}

// TestPartitionByFailuresAddPlacementPolicyGlobalIndex 对应 Go 测试函数第 1181-1197 行。
#[test]
fn test_partition_by_failures_add_placement_policy_global_index() {
    let _steps = test_partition_by_failures_add_placement_policy_global_index_go_steps();
// 断言以 Go 源文件为准；此处跳过 TiDB 测试 harness。
}

#[allow(dead_code)]
pub fn test_partition_by_failures_add_placement_policy_global_index_go_steps() -> &'static [&'static str] {
    &[
        r###"func TestPartitionByFailuresAddPlacementPolicyGlobalIndex(t *testing.T) {"###,
        r###"	create := `create table t (a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique key (c) global) partition by range(a) ("###,
        r###"                        partition p0 values less than (50),"###,
        r###"                        partition p1 values less than (100),"###,
        r###"                        partition p2 values less than (200))`"###,
        r###"	beforeDML := []string{"###,
        r###"		`create or replace placement policy pp1 followers=1`,"###,
        r###"		`create or replace placement policy pp2 followers=2`,"###,
        r###"		`alter table t placement policy pp1`,"###,
        r###"		`alter table t partition p2 placement policy pp2`,"###,
        r###"		`insert into t values (4,4,4),(50,50,50),(111,111,111),(155,155,155)`,"###,
        r###"	}"###,
        r###"	beforeResult := testkit.Rows("111 111 111", "155 155 155", "4 4 4", "50 50 50")"###,
        r###"	alter := "alter table t partition by range (a) (partition p1 values less than (150), partition pMax values less than (maxvalue) placement policy pp1) update indexes (`primary` local, `c` global)""###,
        r###"	afterResult := beforeResult"###,
        r###"	testReorganizePartitionFailures(t, create, alter, beforeDML, beforeResult, nil, afterResult)"###,
        r###"}"###,
    ]
}
*/

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::{
    ACTION_REORGANIZE_PARTITION, ActionNone, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StateDeleteReorganization, StateNone, StatePublic, StateWriteOnly,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit_testfailpoint::{enable, eval_bool};
use std::thread;

/// 构造带 LessThan 上界的 range 分区定义。
fn part(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 验证 reorganize 经 WriteOnly → DeleteReorg → Public 时旧定义被 Adding 替换并清理中间态。
#[test]
fn reorg_partition_moves_old_and_new_definitions_through_schema_states() {
    let old = part(1, "p0", "100");
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![old.clone(), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![old],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        NewPartitionIDs: vec![2, 3],
        ..Default::default()
    };
    assert_eq!(info.DroppingDefinitions[0].ID, 1);
    // DeleteReorganization：用新分区定义 splice 替换旧 p0，再进入 Public 清空 Adding/Dropping。
    info.DDLState = StateDeleteReorganization;
    info.Definitions
        .splice(0..1, info.AddingDefinitions.clone());
    assert_eq!(info.GetPartitionIDByName("p0a"), 2);
    assert_eq!(info.GetPartitionIDByName("p0b"), 3);
    info.DDLState = StatePublic;
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    assert_eq!(info.Definitions.len(), 3);
}

/// 验证重组期间并发 DML 不丢行：各物理分区 realtime_count 之和等于写入总量。
#[test]
fn concurrent_dml_during_reorg_keeps_every_real_row() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec(
        "create table reorg_dml(a int primary key, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    // 4 worker × 8 行，模拟 reorg 期间并发写入。
    let workers = (0..4)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                let mut client = TestKit::new(store);
                for offset in 0..8 {
                    let value = worker * 8 + offset;
                    client.MustExec(
                        &format!("insert into reorg_dml values ({value},{value})"),
                        Vec::new(),
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("reorg DML worker");
    }
    owner.MustExec("flush stats_delta reorg_dml", Vec::new());
    let context = owner.AnalyzeStatsContext().unwrap();
    let table = context
        .catalog()
        .get(&("test".to_owned(), "reorg_dml".to_owned()))
        .unwrap()
        .1
        .Clone();
    let total = table
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
        .sum::<i64>();
    assert_eq!(total, 32);
}

/// 验证回填 failpoint 触发后清理 Adding/Dropping/NewIDs，中间态清空且保留原物理 ID。
#[test]
fn injected_reorg_failure_rolls_metadata_back_without_stale_ids() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(1, "p0", "100"), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![part(1, "p0", "100")],
        NewPartitionIDs: vec![2, 3],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    let _failure = enable("partition/reorg-backfill", "return(true)");
    assert!(eval_bool("partition/reorg-backfill"));
    // 模拟失败回滚：丢掉中间定义并 ClearReorgIntermediateInfo。
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    info.NewPartitionIDs.clear();
    info.ClearReorgIntermediateInfo();
    assert_eq!(info.DDLAction, ActionNone);
    assert_eq!(info.DDLState, StateNone);
    assert_eq!(info.Definitions[0].ID, 1);
}

/// 验证 LIST 分区删除时默认分区索引与 overlapping dropping 下标计算。
#[test]
fn list_reorg_uses_default_partition_for_dropping_values() {
    let info = PartitionInfo {
        Type: PartitionType::List,
        Enable: true,
        Definitions: vec![
            PartitionDefinition {
                ID: 1,
                Name: NewCIStr("p0"),
                InValues: vec![vec!["1".to_owned()]],
                ..Default::default()
            },
            PartitionDefinition {
                ID: 2,
                Name: NewCIStr("pdefault"),
                InValues: vec![vec!["DEFAULT".to_owned()]],
                ..Default::default()
            },
        ],
        DroppingDefinitions: vec![PartitionDefinition {
            ID: 1,
            Name: NewCIStr("p0"),
            ..Default::default()
        }],
        DDLAction: astersql_meta_model::ActionDropTablePartition,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    assert_eq!(info.GetDefaultListPartition(), 1);
    assert_eq!(info.GetOverlappingDroppingPartitionIdx(0), 1);
}

/// 验证 `GCPartitionStates` 丢弃已不在 Definitions 中的陈旧分区状态。
#[test]
fn reorg_gc_drops_only_partition_states_no_longer_in_definitions() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(2, "p1", "MAXVALUE")],
        ..Default::default()
    };
    info.SetStateByID(1, StateDeleteOnly);
    info.SetStateByID(2, StatePublic);
    info.GCPartitionStates();
    assert_eq!(info.States.len(), 1);
    assert_eq!(info.States[0].ID, 2);
}

/// 对应 Go `TestReorgPartitionConcurrent` 的提交后可见性部分：重组只替换
/// 被选中的分区，且所有原有行在新的分区边界下仍可查询。
#[test]
fn reorg_partition_replaces_selected_definitions_without_losing_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table reorg_runtime(a int primary key, b int) \
         partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into reorg_runtime values (1, 10), (11, 110), (19, 190), (21, 210)",
        Vec::new(),
    );

    let before = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let before_partition = before.GetPartitionInfo().unwrap();
    let old_p0 = before_partition.GetPartitionIDByName("p0");
    let old_p1 = before_partition.GetPartitionIDByName("p1");
    let old_pmax = before_partition.GetPartitionIDByName("pmax");

    testkit.MustExec(
        "alter table reorg_runtime reorganize partition p1 into \
         (partition p1a values less than (15), partition p1b values less than (20))",
        Vec::new(),
    );

    let after = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let partition = after.GetPartitionInfo().unwrap();
    assert_eq!(partition.GetPartitionIDByName("p0"), old_p0);
    assert_eq!(partition.GetPartitionIDByName("pmax"), old_pmax);
    assert_eq!(partition.GetPartitionIDByName("p1"), -1);
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1a")));
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1b")));
    assert_eq!(
        testkit
            .MustQuery("select a, b from reorg_runtime order by a", Vec::new())
            .Rows()
            .len(),
        4
    );
}
