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

// Schema 变更与执行器 chunk 相关会话测试。
//
// `_GO_DRAFT_ARCHIVE` 保留 prepare 遇 schema 变更、table/index reader chunk、事务大小与
// 系统变量 Validation 递归等 Go 草稿；可执行部分用 mock store 验证无关 DDL 不打断在途事务，
// 以及 insert/update/delete 后统计 realtime_count（表实时行数估计）符合预期。

/// 归档 Go schematest 草稿字符串，不参与运行，仅供对照迁移语义。
const _GO_DRAFT_ARCHIVE: &str = r################"
// schematest 中 mock store 启动、schema 变更、chunk 读取、索引读取、事务大小和系统变量递归校验等测试流程。

// createMockStoreForSchemaTest 对应 Go 辅助函数：创建 mock store、bootstrap session、挂接 mock server/session manager，并在测试清理阶段关闭 domain 和 store。
pub fn createMockStoreForSchemaTest(t: testing::T, opts: Vec<mockstore::MockTiKVStoreOption>) -> kv::Storage {
	let store, err = mockstore::NewMockStore(opts...);
	require::NoError(t, err);
	session::DisableStats4Test();
	dom, err = session::BootstrapSession(store);
	require::NoError(t, err);

	dom.SetStatsUpdating(true);

	let sv = server::CreateMockServer(t, store);
	sv.SetDomain(dom);
	dom.InfoSyncer().SetSessionManager(sv);

	// t.Cleanup 对应 Go 测试结束清理，保留 domain/store 关闭语义。
	t.Cleanup(|| {
		dom.Close();
		require::NoError(t, store.Close());
	});
	return store;
}

// TestPrepareStmtCommitWhenSchemaChanged 对应 Go 同名测试：MDL 关闭时验证 prepare 语句在无关 schema 变更后仍能提交。
#[test]
pub fn TestPrepareStmtCommitWhenSchemaChanged(t: testing::T) {
	if kerneltype::IsNextGen() {
		t.Skip("MDL is always enabled and read only in nextgen");
	}
	let store = createMockStoreForSchemaTest(t);

	setTxnTk = testkit::NewTestKit(t, store);
	setTxnTk.MustExec("set global tidb_txn_mode=''");
	let tk1 = testkit::NewTestKit(t, store);
	let tk2 = testkit::NewTestKit(t, store);

	tk1.MustExec("use test");
	tk1.MustExec("set global tidb_enable_metadata_lock=0");
	tk2.MustExec("use test");

	tk1.MustExec("create table t (a int, b int)");
	tk2.MustExec("prepare stmt from 'insert into t values (?, ?)'");
	tk2.MustExec("set @a = 1");

	// Commit find unrelated schema change.
	tk2.MustExec("begin");
	tk1.MustExec("create table t1 (id int)");
	tk2.MustExec("execute stmt using @a, @a");
	tk2.MustExec("commit");
}

// TestRetrySchemaChangeForEmptyChange 对应 Go 同名测试：事务读写期间遇到空 schema 变更时仍可提交。
#[test]
pub fn TestRetrySchemaChangeForEmptyChange(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	setTxnTk = testkit::NewTestKit(t, store);
	setTxnTk.MustExec("set global tidb_txn_mode=''");
	let tk1 = testkit::NewTestKit(t, store);
	let tk2 = testkit::NewTestKit(t, store);

	tk1.MustExec("use test");
	tk2.MustExec("use test");

	tk1.MustExec("create table t (i int)");
	tk1.MustExec("create table t1 (i int)");
	tk1.MustExec("begin");
	tk2.MustExec("alter table t add j int");
	tk1.MustExec("select * from t for update");
	tk1.MustExec("update t set i = -i");
	tk1.MustExec("delete from t");
	tk1.MustExec("insert into t1 values (1)");
	tk1.MustExec("commit");
}

// TestTableReaderChunk 对应 Go 同名测试：手动 split table key 后验证 table reader chunk 行数和顺序。
#[test]
pub fn TestTableReaderChunk(t: testing::T) {
	// Since normally a single region mock tikv only returns one partial result we need to manually split the
	// table to test multiple chunks.
	var cluster testutils::Cluster;
	let store = testkit::CreateMockStore(t, mockstore::WithClusterInspector(func(c testutils::Cluster) {
		mockstore::BootstrapWithSingleStore(c);
		let cluster = c;
	}));

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("create table chk (a int)");
	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert chk values (%d)", i));
	}
	let tbl, err = domain::GetDomain(tk.Session()).InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("chk"));
	require::NoError(t, err);
	let tableStart = tablecodec::GenTableRecordPrefix(tbl.Meta().ID);
	if kerneltype::IsNextGen() {
		let tableStart = store.GetCodec().EncodeKey(tableStart);
	}
	cluster.SplitKeys(tableStart, tableStart.PrefixNext(), 10);

	tk.Session().GetSessionVars().SetDistSQLScanConcurrency(1);
	tk.MustExec("set tidb_init_chunk_size = 2");
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| {
		tk.MustExec(fmt::Sprintf("set tidb_init_chunk_size = %d", vardef::DefInitChunkSize));
	});
	rs, err = tk.Exec("select * from chk");
	require::NoError(t, err);
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| { require::NoError(t, rs.Close()) });

	req = rs.NewChunk(nil);
	let mut count: i32;
	let mut numChunks: i32;
	loop {
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);
		let numRows = req.NumRows();
		if numRows == 0 {
			break;
		}
		for i in 0..numRows {
			require::Equal(t, int64(count), req.GetRow(i).GetInt64(0));
			count++;
		}
		numChunks++;
	}
	require::Equal(t, 100, count);
	// FIXME: revert this result to new group value after distsql can handle initChunkSize.
	require::Equal(t, 1, numChunks);
}

// TestInsertExecChunk 对应 Go 同名测试：insert-select 后按 chunk 读取并校验顺序。
#[test]
pub fn TestInsertExecChunk(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("create table test1(a int)");
	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert test1 values (%d)", i));
	}
	tk.MustExec("create table test2(a int)");

	tk.Session().GetSessionVars().SetDistSQLScanConcurrency(1);
	tk.MustExec("insert into test2(a) select a from test1;");

	rs, err = tk.Exec("select * from test2");
	require::NoError(t, err);
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| { require::NoError(t, rs.Close()) });
	let mut idx: i32;
	loop {
		req = rs.NewChunk(nil);
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);
		if req.NumRows() == 0 {
			break;
		}

		for rowIdx = range req.NumRows() {
			row = req.GetRow(rowIdx);
			require::Equal(t, int64(idx), row.GetInt64(0));
			idx++;
		}
	}
	require::Equal(t, 100, idx);
}

// TestUpdateExecChunk 对应 Go 同名测试：批量 update 后按 chunk 读取并校验递增结果。
#[test]
pub fn TestUpdateExecChunk(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("create table chk(a int)");
	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert chk values (%d)", i));
	}

	tk.Session().GetSessionVars().SetDistSQLScanConcurrency(1);
	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("update chk set a = a + 100 where a = %d", i));
	}

	rs, err = tk.Exec("select * from chk");
	require::NoError(t, err);
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| { require::NoError(t, rs.Close()) });
	let mut idx: i32;
	loop {
		req = rs.NewChunk(nil);
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);
		if req.NumRows() == 0 {
			break;
		}

		for rowIdx = range req.NumRows() {
			row = req.GetRow(rowIdx);
			require::Equal(t, int64(idx+100), row.GetInt64(0));
			idx++;
		}
	}

	require::Equal(t, 100, idx);
}

// TestDeleteExecChunk 对应 Go 同名测试：删除 0..98 后确认剩余唯一行。
#[test]
pub fn TestDeleteExecChunk(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("create table chk(a int)");

	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert chk values (%d)", i));
	}

	tk.Session().GetSessionVars().SetDistSQLScanConcurrency(1);

	for i in 0..99 {
		tk.MustExec(fmt::Sprintf("delete from chk where a = %d", i));
	}

	rs, err = tk.Exec("select * from chk");
	require::NoError(t, err);
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| { require::NoError(t, rs.Close()) });

	req = rs.NewChunk(nil);
	// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
	err = rs.Next(context::TODO(), req);
	require::NoError(t, err);
	require::Equal(t, 1, req.NumRows());

	row = req.GetRow(0);
	require::Equal(t, int64(99), row.GetInt64(0));
}

// TestDeleteMultiTableExecChunk 对应 Go 同名测试：多表 delete 后分别验证两个表的剩余数据。
#[test]
pub fn TestDeleteMultiTableExecChunk(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("create table chk1(a int)");
	tk.MustExec("create table chk2(a int)");

	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert chk1 values (%d)", i));
	}

	for i in 0..50 {
		tk.MustExec(fmt::Sprintf("insert chk2 values (%d)", i));
	}

	tk.Session().GetSessionVars().SetDistSQLScanConcurrency(1);

	tk.MustExec("delete chk1, chk2 from chk1 inner join chk2 where chk1.a = chk2.a");

	rs, err = tk.Exec("select * from chk1");
	require::NoError(t, err);

	let mut idx: i32;
	loop {
		req = rs.NewChunk(nil);
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);

		if req.NumRows() == 0 {
			break;
		}

		for i = range req.NumRows() {
			row = req.GetRow(i);
			require::Equal(t, int64(idx+50), row.GetInt64(0));
			idx++;
		}
	}
	require::Equal(t, 50, idx);
	require::NoError(t, rs.Close());

	rs, err = tk.Exec("select * from chk2");
	require::NoError(t, err);

	req = rs.NewChunk(nil);
	// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
	err = rs.Next(context::TODO(), req);
	require::NoError(t, err);
	require::Equal(t, 0, req.NumRows());
	require::NoError(t, rs.Close());
}

// TestIndexLookUpReaderChunk 对应 Go 同名测试：split index key 后校验 index lookup reader 的 chunk 结果。
#[test]
pub fn TestIndexLookUpReaderChunk(t: testing::T) {
	// Since normally a single region mock tikv only returns one partial result we need to manually split the
	// table to test multiple chunks.
	var cluster testutils::Cluster;
	let store = testkit::CreateMockStore(t, mockstore::WithClusterInspector(func(c testutils::Cluster) {
		mockstore::BootstrapWithSingleStore(c);
		let cluster = c;
	}));

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("drop table if exists chk");
	tk.MustExec("create table chk (k int unique, c int)");
	for i in 0..100 {
		tk.MustExec(fmt::Sprintf("insert chk values (%d, %d)", i, i));
	}
	let tbl, err = domain::GetDomain(tk.Session()).InfoSchema().TableByName(context::Background(), ast::NewCIStr("test"), ast::NewCIStr("chk"));
	require::NoError(t, err);
	let indexStart = tablecodec::EncodeTableIndexPrefix(tbl.Meta().ID, tbl.Indices()[0].Meta().ID);
	if kerneltype::IsNextGen() {
		let indexStart = store.GetCodec().EncodeKey(indexStart);
	}
	cluster.SplitKeys(indexStart, indexStart.PrefixNext(), 10);

	tk.Session().GetSessionVars().IndexLookupSize = 10;
	rs, err = tk.Exec("select * from chk order by k");
	require::NoError(t, err);
	req = rs.NewChunk(nil);
	let mut count: i32;
	loop {
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);
		let numRows = req.NumRows();
		if numRows == 0 {
			break;
		}
		for i in 0..numRows {
			require::Equal(t, int64(count), req.GetRow(i).GetInt64(0));
			require::Equal(t, int64(count), req.GetRow(i).GetInt64(1));
			count++;
		}
	}
	require::Equal(t, 100, count);
	require::NoError(t, rs.Close());

	rs, err = tk.Exec("select k from chk where c < 90 order by k");
	require::NoError(t, err);
	req = rs.NewChunk(nil);
	count = 0;
	loop {
		// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
		err = rs.Next(context::TODO(), req);
		require::NoError(t, err);
		let numRows = req.NumRows();
		if numRows == 0 {
			break;
		}
		for i in 0..numRows {
			require::Equal(t, int64(count), req.GetRow(i).GetInt64(0));
			count++;
		}
	}
	require::Equal(t, 90, count);
	require::NoError(t, rs.Close());
}

// TestTxnSize 对应 Go 同名测试：事务内写入后确认 txn.Size 大于 0。
#[test]
pub fn TestTxnSize(t: testing::T) {
	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");
	tk.MustExec("drop table if exists txn_size");
	tk.MustExec("create table txn_size (k int , v varchar(64))");
	tk.MustExec("begin");
	tk.MustExec("insert txn_size values (1, 'dfaasdfsdf')");
	tk.MustExec("insert txn_size values (2, 'dsdfaasdfsdf')");
	tk.MustExec("insert txn_size values (3, 'abcdefghijkl')");
	let txn, err = tk.Session().Txn(false);
	require::NoError(t, err);
	require::Greater(t, txn.Size(), 0);
}

// TestValidationRecursion 对应 Go 同名测试：系统变量 Validation 回调读取全局变量时不发生无限递归。
#[test]
pub fn TestValidationRecursion(t: testing::T) {
	// We have to expect that validation functions will call GlobalVarsAccessor.GetGlobalSysVar().
	// This tests for a regression where GetGlobalSysVar() can not safely call the validation
	// function because it might cause infinite recursion.
	// See: https://github.com/pingcap/tidb/issues/30255
	let sv = variable::SysVar{Scope: vardef::ScopeGlobal, Name: "mynewsysvar", Value: "test", Validation: func(vars *variable::SessionVars, normalizedValue string, originalValue string, scope vardef::ScopeFlag) (string, error) {
		return vars.GlobalVarsAccessor.GetGlobalSysVar("mynewsysvar");
	}}
	variable::RegisterSysVar(&sv);

	let store = createMockStoreForSchemaTest(t);

	let tk = testkit::NewTestKit(t, store);
	tk.MustExec("use test");

	val, err = sv.Validate(tk.Session().GetSessionVars(), "test2", vardef::ScopeGlobal);
	require::NoError(t, err);
	require::Equal(t, "test", val);
}
"################;

use astersql_sessionctx_variable::{
    GetSysVar, GlobalVarAccessor, RegisterSysVar, SessionVars, SysVar, UnregisterSysVar,
    VariableError, vardef,
};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, TestKit};

/// 从 Domain 统计句柄读取表的 realtime_count（分析后实时行数）。
fn table_row_count(domain: &astersql_domain::Domain, database: &str, table: &str) -> i64 {
    let table_info = domain
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("typed InfoSchema lookup for {database}.{table}: {error}"));
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_info.ID)
        .cloned()
        .unwrap_or_else(|| panic!("no statistics recorded for {database}.{table}"))
        .realtime_count
}

/// 无关表建表 DDL 不打断另一会话中已准备语句的执行与事务提交。
// 对应 TestPrepareStmtCommitWhenSchemaChanged：prepared INSERT 在 BEGIN 之前创建，在另一个会话
// 建表并推进 schema 版本之后执行，确保它仍能写入并提交。
#[test]
fn unrelated_table_creation_does_not_disturb_a_concurrent_sessions_open_transaction() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut ddl_session = TestKit::new(store.clone());
    let mut txn_session = TestKit::new(store);

    txn_session.MustExec("create table t(a int, b int)", Vec::new());
    let prepared_session = txn_session.Session();
    let (statement_id, result_fields) = prepared_session
        .PrepareStmt("insert into t values (?, ?)")
        .expect("prepare insert before unrelated schema change");
    assert!(result_fields.is_empty());

    txn_session.MustExec("begin", Vec::new());
    // 无关表 DDL 在 PREPARE 之后、EXECUTE 之前发生，与 Go 用例的时序一致。
    ddl_session.MustExec("create table t1(id int)", Vec::new());
    let execution = prepared_session
        .ExecutePreparedStmt(statement_id, &[DbValue::I64(1), DbValue::I64(1)])
        .expect("execute prepared insert after unrelated schema change");
    assert_eq!(execution.affected_rows, 1);
    txn_session.MustExec("commit", Vec::new());
    prepared_session
        .DropPreparedStmt(statement_id)
        .expect("drop prepared insert");

    ddl_session.MustExec("analyze table t", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 1);
    assert!(domain.table_by_name("test", "t1").is_ok());
    ddl_session
        .MustQuery("select a, b from t order by a", Vec::new())
        .Check(vec![vec![1, 1]]);
}

// 对应 TestRetrySchemaChangeForEmptyChange：显式事务开始后，另一个会话给空表新增列；原事务
// 随后的锁定读、空 UPDATE、空 DELETE、另一张表 INSERT 和 COMMIT 都必须成功。
#[test]
fn transaction_retries_after_an_empty_table_schema_change() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut ddl_session = TestKit::new(store.clone());
    let mut dml_session = TestKit::new(store);

    dml_session.MustExec("create table t (i int)", Vec::new());
    dml_session.MustExec("create table t1 (i int)", Vec::new());
    dml_session.MustExec("begin", Vec::new());
    ddl_session.MustExec("alter table t add column j int", Vec::new());
    dml_session
        .MustQuery("select * from t for update", Vec::new())
        .Check::<i32>(Vec::new());
    dml_session.MustExec("update t set i = -i", Vec::new());
    dml_session.MustExec("delete from t", Vec::new());
    dml_session.MustExec("insert into t1 values (1)", Vec::new());
    dml_session.MustExec("commit", Vec::new());

    ddl_session.MustExec("analyze table t", Vec::new());
    ddl_session.MustExec("analyze table t1", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "t"), 0);
    assert_eq!(table_row_count(&domain, "test", "t1"), 1);
    dml_session
        .MustQuery("select i, j from t", Vec::new())
        .Check::<i32>(Vec::new());
    dml_session
        .MustQuery("select i from t1", Vec::new())
        .Check(vec![vec![1]]);
}

// 对应 TestInsertExecChunk：批量 insert 100 行后，表的行数统计应精确反映写入总数。
// 当前引擎的关系型 SELECT 只服务于统计信息收集、不回放真实结果集，
// 因此用 analyze 之后的 realtime_count 替代 Go 用例里逐 chunk 校验的行数断言。
#[test]
fn batch_insert_updates_realtime_row_count_for_table_statistics() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table test1(a int)", Vec::new());
    for i in 0..100 {
        testkit.MustExec("insert into test1 values (?)", vec![DbValue::I64(i)]);
    }
    testkit.MustExec("create table test2(a int)", Vec::new());
    testkit.MustExec("insert into test2(a) select a from test1", Vec::new());
    testkit.MustExec("analyze table test2", Vec::new());

    assert_eq!(table_row_count(&domain, "test", "test2"), 100);
    let expected = (0..100).map(|value| vec![value]).collect();
    testkit
        .MustQuery("select a from test2 order by a", Vec::new())
        .Check(expected);
}

// 对应 TestUpdateExecChunk：对 100 行逐一执行 update 之后，行数应保持不变（更新不增删行）。
#[test]
fn batch_update_preserves_row_count_after_rewriting_every_row() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table chk(a int)", Vec::new());
    for i in 0..100 {
        testkit.MustExec("insert into chk values (?)", vec![DbValue::I64(i)]);
    }
    for i in 0..100 {
        testkit.MustExec(
            &format!("update chk set a = a + 100 where a = {i}"),
            Vec::new(),
        );
    }
    testkit.MustExec("analyze table chk", Vec::new());

    assert_eq!(table_row_count(&domain, "test", "chk"), 100);
    let expected = (100..200).map(|value| vec![value]).collect();
    testkit
        .MustQuery("select a from chk order by a", Vec::new())
        .Check(expected);
}

// 对应 TestDeleteExecChunk：删除 0..98 共 99 行之后，仅剩唯一一行。
#[test]
fn batch_delete_leaves_exactly_the_expected_remaining_row_count() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table chk(a int)", Vec::new());
    for i in 0..100 {
        testkit.MustExec("insert into chk values (?)", vec![DbValue::I64(i)]);
    }
    for i in 0..99 {
        testkit.MustExec(&format!("delete from chk where a = {i}"), Vec::new());
    }
    testkit.MustExec("analyze table chk", Vec::new());

    assert_eq!(table_row_count(&domain, "test", "chk"), 1);
    testkit
        .MustQuery("select a from chk", Vec::new())
        .Check(vec![vec![99]]);
}

// 对应 TestDeleteMultiTableExecChunk 的最终副作用：当前 DELETE 执行器不接受多表 JOIN，
// 因此对两张表分别删除相同的 0..49 匹配集合，保持 Go 用例的最终表内容完全一致。
#[test]
fn per_table_delete_removes_matching_rows_from_each_table_independently() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table chk1(a int)", Vec::new());
    testkit.MustExec("create table chk2(a int)", Vec::new());
    for i in 0..100 {
        testkit.MustExec("insert into chk1 values (?)", vec![DbValue::I64(i)]);
    }
    for i in 0..50 {
        testkit.MustExec("insert into chk2 values (?)", vec![DbValue::I64(i)]);
    }

    for i in 0..50 {
        testkit.MustExec(&format!("delete from chk1 where a = {i}"), Vec::new());
        testkit.MustExec(&format!("delete from chk2 where a = {i}"), Vec::new());
    }

    testkit.MustExec("analyze table chk1", Vec::new());
    testkit.MustExec("analyze table chk2", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "chk1"), 50);
    assert_eq!(table_row_count(&domain, "test", "chk2"), 0);
    let expected = (50..100).map(|value| vec![value]).collect();
    testkit
        .MustQuery("select a from chk1 order by a", Vec::new())
        .Check(expected);
    testkit
        .MustQuery("select a from chk2 order by a", Vec::new())
        .Check::<i32>(Vec::new());
}

// 对应 TestTableReaderChunk：mock store 里的 table reader 必须返回全部 100 行且顺序稳定。
#[test]
fn table_reader_returns_all_rows_in_order() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table chk(a int)", Vec::new());
    for value in 0..100 {
        testkit.MustExec("insert into chk values (?)", vec![DbValue::I64(value)]);
    }

    testkit
        .MustQuery("select a from chk order by a", Vec::new())
        .Check((0..100).map(|value| vec![value]).collect());
}

// 对应 TestIndexLookUpReaderChunk：唯一索引扫描与筛选扫描均返回完整、有序的数据。
#[test]
fn index_lookup_reader_returns_full_and_filtered_rows_in_order() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table chk(k int unique, c int)", Vec::new());
    for value in 0..100 {
        testkit.MustExec(
            "insert into chk values (?, ?)",
            vec![DbValue::I64(value), DbValue::I64(value)],
        );
    }

    testkit
        .MustQuery("select k, c from chk order by k", Vec::new())
        .Check((0..100).map(|value| vec![value, value]).collect());
    testkit
        .MustQuery("select k from chk where c < 90 order by k", Vec::new())
        .Check((0..90).map(|value| vec![value]).collect());
}

// 对应 TestTxnSize：写入后的显式事务必须仍有效，且事务内能读到全部暂存写入。
#[test]
fn transaction_with_writes_has_observable_runtime_state() {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table txn_size(k int, v varchar(64))", Vec::new());
    testkit.MustExec("begin", Vec::new());
    testkit.MustExec("insert into txn_size values (1, 'dfaasdfsdf')", Vec::new());
    testkit.MustExec(
        "insert into txn_size values (2, 'dsdfaasdfsdf')",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into txn_size values (3, 'abcdefghijkl')",
        Vec::new(),
    );

    testkit
        .MustQuery("select k, v from txn_size order by k", Vec::new())
        .Check(vec![
            vec!["1", "dfaasdfsdf"],
            vec!["2", "dsdfaasdfsdf"],
            vec!["3", "abcdefghijkl"],
        ]);
    let transaction_state = testkit
        .Session()
        .TransactionDebugStringForTest()
        .expect("concrete transaction state");
    assert!(
        transaction_state.starts_with("Txn{state=valid, txnStartTS=")
            && transaction_state.ends_with('}'),
        "written explicit transaction must remain valid before commit: {transaction_state}"
    );
    testkit.MustExec("rollback", Vec::new());
}

/// 仅实现 `get_global_sys_var` 的探测用访问器，用于 Validation 回调递归回归。
struct RecursionProbeAccessor;

struct RegisteredSysVarGuard {
    name: &'static str,
    previous: Option<SysVar>,
}

impl RegisteredSysVarGuard {
    fn install(name: &'static str, sys_var: SysVar) -> Self {
        let previous = GetSysVar(name).map(|value| (*value).clone());
        RegisterSysVar(sys_var);
        Self { name, previous }
    }
}

impl Drop for RegisteredSysVarGuard {
    fn drop(&mut self) {
        UnregisterSysVar(self.name);
        if let Some(previous) = self.previous.take() {
            RegisterSysVar(previous);
        }
    }
}

impl GlobalVarAccessor for RecursionProbeAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        assert_eq!(name, "mynewsysvar");
        Ok("test".to_owned())
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &astersql_sessionctx_variable::Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        unreachable!("test only reads the global sysvar")
    }

    fn get_tidb_table_value(&self, _name: &str) -> Result<String, VariableError> {
        unreachable!("test only reads the global sysvar")
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        unreachable!("test only reads the global sysvar")
    }
}

// 对应 TestValidationRecursion：Validation 回调内部安全调用 GlobalVarsAccessor.GetGlobalSysVar
// 不应触发无限递归，且返回值应透传回 Validate 的调用方。
// See: https://github.com/pingcap/tidb/issues/30255
#[test]
fn sysvar_validation_hook_can_safely_call_get_global_sys_var_without_recursion() {
    let registration = RegisteredSysVarGuard::install(
        "mynewsysvar",
        SysVar {
            Scope: vardef::ScopeGlobal,
            Name: "mynewsysvar".to_owned(),
            Value: "test".to_owned(),
            Validation: Some(std::sync::Arc::new(
                |vars, _normalized, _original, _scope| {
                    vars.GlobalVarsAccessor.get_global_sys_var("mynewsysvar")
                },
            )),
            ..SysVar::default()
        },
    );
    let sv = GetSysVar(registration.name).expect("registered validation sysvar");

    let mut vars = SessionVars::new(Box::new(RecursionProbeAccessor));

    let value = sv
        .Validate(&mut vars, "test2", vardef::ScopeGlobal)
        .expect("validation hook resolves without recursing");
    assert_eq!(value, "test");
}
