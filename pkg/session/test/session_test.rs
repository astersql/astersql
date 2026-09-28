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

// Session 包对抗性一致性测试。
//
// 下方注释保留 Go 场景的逐分支映射，文件末尾使用真实 mockstore、TestKit、
// bootstrap runtime 与 session variables 执行对应回归断言。
// 跨模块观察面复用既有生产测试：schema checker 位于 runtime_test/session.rs，
// cached marker 位于 cached_table_runtime_test.rs，hint 生命周期位于
// hint_runtime_test.rs，process/CurTxnStartTS 位于 sessmgr 测试，DDL close/cancel
// 位于 ddl/job_submitter_test.rs，KV assertion 位于 table/tables/assertion_test.rs。
//
// session 包测试中的 schema checker、bootstrap 初始化 SQL、hint、request source、process info、身份匹配和外部时间戳读取等流程。
//
// TestSchemaCheckerSQL 对应 Go 同名测试：schema 版本过期时区分可重试/不可重试 SQL 与分区表路径。
// #[test]
// pub fn TestSchemaCheckerSQL(t: testing::T) {
// 	if kerneltype::IsNextGen() {
// 		t.Skip("MDL is always enabled and read only in nextgen");
// 	}
// 	let store = testkit::CreateMockStoreWithSchemaLease(t, 1*time::Second);
//
// 	setTxnTk = testkit::NewTestKit(t, store);
// 	setTxnTk.MustExec("set global tidb_enable_metadata_lock=0");
// 	setTxnTk.MustExec("set global tidb_txn_mode=''");
// 	let tk = testkit::NewTestKit(t, store);
// 	let tk1 = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk1.MustExec("use test");
//
// create table
// 	tk.MustExec(r#"create table t (id int, c int);"#);
// 	tk.MustExec(r#"create table t1 (id int, c int);"#);
// insert data
// 	tk.MustExec(r#"insert into t values(1, 1);"#);
//
// The schema version is out of date in the first transaction, and the SQL can't be retried.
// 	atomic::StoreUint32(&session::SchemaChangedWithoutRetry, 1);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		atomic::StoreUint32(&session::SchemaChangedWithoutRetry, 0);
// 	});
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table t modify column c bigint;"#);
// 	tk.MustExec(r#"insert into t values(3, 3);"#);
// 	err = tk.ExecToErr(r#"commit;"#);
// 	require::True(t, terror::ErrorEqual(err, domain::ErrInfoSchemaChanged), fmt::Sprintf("err %v", err));
//
// But the transaction related table IDs aren't in the updated table IDs.
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table t add index idx2(c);"#);
// 	tk.MustExec(r#"insert into t1 values(4, 4);"#);
// 	tk.MustExec(r#"commit;"#);
//
// Test for "select for update".
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table t add index idx3(c);"#);
// 	tk.MustQuery(r#"select * from t for update"#);
// 	require::Error(t, tk.ExecToErr(r#"commit;"#));
//
// Repeated tests for partitioned table
// 	tk.MustExec(r#"create table pt (id int, c int) partition by hash (id) partitions 3"#);
// 	tk.MustExec(r#"insert into pt values(1, 1);"#);
// The schema version is out of date in the first transaction, and the SQL can't be retried.
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table pt modify column c bigint;"#);
// 	tk.MustExec(r#"insert into pt values(3, 3);"#);
// 	err = tk.ExecToErr(r#"commit;"#);
// 	require::True(t, terror::ErrorEqual(err, domain::ErrInfoSchemaChanged), fmt::Sprintf("err %v", err));
//
// But the transaction related table IDs aren't in the updated table IDs.
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table pt add index idx2(c);"#);
// 	tk.MustExec(r#"insert into t1 values(4, 4);"#);
// 	tk.MustExec(r#"commit;"#);
//
// Test for "select for update".
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table pt add index idx3(c);"#);
// 	tk.MustQuery(r#"select * from pt for update"#);
// 	require::Error(t, tk.ExecToErr(r#"commit;"#));
//
// Test for "select for update".
// 	tk.MustExec(r#"begin;"#);
// 	tk1.MustExec(r#"alter table pt add index idx4(c);"#);
// 	tk.MustQuery(r#"select * from pt partition (p1) for update"#);
// 	require::Error(t, tk.ExecToErr(r#"commit;"#));
// }
//
// TestLoadSchemaFailed 对应 Go 同名测试：infoschema reload 失败后 server invalid，事务提交和恢复路径按原顺序保留。
// #[test]
// pub fn TestLoadSchemaFailed(t: testing::T) {
// 	let originalRetryTime = domain::SchemaOutOfDateRetryTimes.Load();
// 	let originalRetryInterval = domain::SchemaOutOfDateRetryInterval.Load();
// 	domain::SchemaOutOfDateRetryTimes.Store(3);
// 	domain::SchemaOutOfDateRetryInterval.Store(20 * time::Millisecond);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		domain::SchemaOutOfDateRetryTimes.Store(originalRetryTime);
// 		domain::SchemaOutOfDateRetryInterval.Store(originalRetryInterval);
// 	});
//
// 	let store = testkit::CreateMockStoreWithSchemaLease(t, 1*time::Second);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	let tk1 = testkit::NewTestKit(t, store);
// 	let tk2 = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk1.MustExec("use test");
// 	tk2.MustExec("use test");
//
// 	tk.MustExec("create table t (a int);");
// 	tk.MustExec("create table t1 (a int);");
// 	tk.MustExec("create table t2 (a int);");
//
// 	tk1.MustExec("begin");
// 	tk2.MustExec("begin");
//
// Make sure loading information schema is failed and server is invalid.
// 	testfailpoint::Enable(t, "github.com/pingcap/tidb/pkg/infoschema/issyncer/ErrorMockReloadFailed", r#"return(true)"#);
// 	require::Error(t, domain::GetDomain(tk.Session()).Reload());
//
// 	let lease = domain::GetDomain(tk.Session()).GetSchemaLease();
// 	time::Sleep(lease * 2);
//
// Make sure executing insert statement is failed when server is invalid.
// 	require::Error(t, tk.ExecToErr("insert t values (100);"));
//
// 	tk1.MustExec("insert t1 values (100);");
// 	tk2.MustExec("insert t2 values (100);");
//
// 	require::Error(t, tk1.ExecToErr("commit"));
//
// 	let ver, err = store.CurrentVersion(kv::GlobalTxnScope);
// 	require::NoError(t, err);
// 	require::NotNil(t, ver);
//
// 	require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/infoschema/issyncer/ErrorMockReloadFailed"));
// 	time::Sleep(lease * 2);
//
// 	tk.MustExec("drop table if exists t;");
// 	tk.MustExec("create table t (a int);");
// 	tk.MustExec("insert t values (100);");
// Make sure insert to table t2 transaction executes.
// 	tk2.MustExec("commit");
// }
//
// TestWriteOnMultipleCachedTable 对应 Go 同名测试：多个 cached table 在事务写入后读取结果正确。
// #[test]
// pub fn TestWriteOnMultipleCachedTable(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("drop table if exists ct1, ct2");
// 	tk.MustExec("create table ct1 (id int, c int)");
// 	tk.MustExec("create table ct2 (id int, c int)");
// 	tk.MustExec("alter table ct1 cache");
// 	tk.MustExec("alter table ct2 cache");
// 	tk.MustQuery("select * from ct1").Check(testkit::Rows());
// 	tk.MustQuery("select * from ct2").Check(testkit::Rows());
//
// 	let lastReadFromCache = func(tk *testkit::TestKit) bool {
// 		return tk.Session().GetSessionVars().StmtCtx.ReadFromTableCache;
// 	}
//
// 	let cached = false;
// 	for _ in 0..50 {
// 		tk.MustQuery("select * from ct1");
// 		if lastReadFromCache(tk) {
// 			let cached = true;
// 			break;
// 		}
// 		time::Sleep(100 * time::Millisecond);
// 	}
// 	require::True(t, cached);
//
// 	tk.MustExec("begin");
// 	tk.MustExec("insert into ct1 values (3, 4)");
// 	tk.MustExec("insert into ct2 values (5, 6)");
// 	tk.MustExec("commit");
//
// 	tk.MustQuery("select * from ct1").Check(testkit::Rows("3 4"));
// 	tk.MustQuery("select * from ct2").Check(testkit::Rows("5 6"));
//
// cleanup
// 	tk.MustExec("alter table ct1 nocache");
// 	tk.MustExec("alter table ct2 nocache");
// }
//
// TestFixSetTiDBSnapshotTS 对应 Go 同名测试：设置 tidb_snapshot 后更新 session 变量不应错误切换 infoschema。
// #[test]
// pub fn TestFixSetTiDBSnapshotTS(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	let safePointName = "tikv_gc_safe_point";
// 	let safePointValue = "20160102-15:04:05 -0700";
// 	let safePointComment = "All versions after safe point can be accessed. (DO NOT EDIT)";
// 	let updateSafePoint = fmt::Sprintf(`INSERT INTO mysql::tidb VALUES ('%[1]s', '%[2]s', '%[3]s');
// 	ON DUPLICATE KEY;
// 	UPDATE variable_value = '%[2]s', comment = '%[3]s'`, safePointName, safePointValue, safePointComment);
// 	tk.MustExec(updateSafePoint);
// 	tk.MustExec("create database t123");
// 	time::Sleep(time::Second);
// 	ts = time::Now().Format("2006-1-2 15:04:05");
// 	time::Sleep(time::Second);
// 	tk.MustExec("drop database t123");
// 	tk.MustMatchErrMsg("use t123", ".*Unknown database.*");
// 	tk.MustExec(fmt::Sprintf("set @@tidb_snapshot='%s'", ts));
// 	tk.MustExec("use t123");
// update any session variable and assert whether infoschema is changed
// 	tk.MustExec("SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_AUTO_CREATE_USER';");
// 	tk.MustExec("use t123");
// }
//
// TestPrepareZero 对应 Go 同名测试：prepared timestamp 参数处理 0 与 ZeroDatetimeStr 的兼容性。
// #[test]
// pub fn TestPrepareZero(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("drop table if exists t");
// 	tk.MustExec("create table t(v timestamp)");
// 	tk.MustExec("prepare s1 from 'insert into t (v) values (?)'");
// 	tk.MustExec("set @v1='0'");
// 	require::Error(t, tk.ExecToErr("execute s1 using @v1"));
// 	tk.MustExec("set @v2='" + types::ZeroDatetimeStr + "'");
// 	tk.MustExec("set @orig_sql_mode=@@sql_mode; set @@sql_mode='';");
// 	tk.MustExec("execute s1 using @v2");
// 	tk.MustQuery("select v from t").Check(testkit::Rows("0000-00-00 00:00:00"));
// 	tk.MustExec("set @@sql_mode=@orig_sql_mode;");
// }
//
// TestPrimaryKeyAutoIncrement 对应 Go 同名测试：自增主键、唯一列更新和 bool prepared 参数转换。
// #[test]
// pub fn TestPrimaryKeyAutoIncrement(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("drop table if exists t");
// 	tk.MustExec("create table t (id BIGINT PRIMARY KEY AUTO_INCREMENT NOT NULL, name varchar(255) UNIQUE NOT NULL, status int)");
// 	tk.MustExec("insert t (name) values (?)", "abc");
// 	let id = tk.Session().LastInsertID();
// 	require::NotZero(t, id);
//
// 	let tk1 = testkit::NewTestKit(t, store);
// 	tk1.MustExec("use test");
// 	tk1.MustQuery("select * from t").Check(testkit::Rows(fmt::Sprintf("%d abc <nil>", id)));
//
// 	tk.MustExec("update t set name = 'abc', status = 1 where id = ?", id);
// 	tk1.MustQuery("select * from t").Check(testkit::Rows(fmt::Sprintf("%d abc 1", id)));
//
// Check for pass bool param to tidb prepared statement
// 	tk.MustExec("drop table if exists t");
// 	tk.MustExec("create table t (id tinyint)");
// 	tk.MustExec("insert t values (?)", true);
// 	tk.MustQuery("select * from t").Check(testkit::Rows("1"));
// }
//
// TestParseWithParams 对应 Go 同名测试：restricted SQL ParseWithParams 的转义、语法错误和缺参错误。
// #[test]
// pub fn TestParseWithParams(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	se = tk.Session();
// 	let exec = se.GetRestrictedSQLExecutor();
//
// test compatibility with ExecuteInternal
// 	let _, err = exec.ParseWithParams(context::TODO(), "SELECT 4");
// 	require::NoError(t, err);
//
// test charset attack
// 	let stmt, err = exec.ParseWithParams(context::TODO(), "SELECT * FROM test WHERE name = %? LIMIT 1", "\xbf\x27 OR 1=1 /*");
// 	require::NoError(t, err);
//
// 	var sb strings::Builder;
// 	let ctx = format::NewRestoreCtx(format::RestoreStringDoubleQuotes, &sb);
// 	err = stmt.Restore(ctx);
// 	require::NoError(t, err);
// 	require::Equal(t, "SELECT * FROM test WHERE name=_utf8mb4\"\xbf' OR 1=1 /*\" LIMIT 1", sb.String());
//
// test invalid sql
// 	let _, err = exec.ParseWithParams(context::TODO(), "SELECT");
// 	require::Regexp(t, ".*You have an error in your SQL syntax.*", err);
//
// test invalid arguments to escape
// 	let _, err = exec.ParseWithParams(context::TODO(), "SELECT %?, %?", 3);
// 	require::Regexp(t, "missing arguments.*", err);
//
// test noescape
// 	let stmt, err = exec.ParseWithParams(context::TODO(), "SELECT 3");
// 	require::NoError(t, err);
//
// 	sb.Reset();
// 	let ctx = format::NewRestoreCtx(0, &sb);
// 	err = stmt.Restore(ctx);
// 	require::NoError(t, err);
// 	require::Equal(t, "SELECT 3", sb.String());
// }
//
// TestDoDDLJobQuit 对应 Go 同名测试：DDL 循环中 store close 触发 context canceled。
// #[test]
// pub fn TestDoDDLJobQuit(t: testing::T) {
// This is required since mock tikv does not support paging.
// 	failpoint::Enable("github.com/pingcap/tidb/pkg/store/copr/DisablePaging", r#"return"#);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/store/copr/DisablePaging"));
// 	});
//
// test https://github.com/pingcap/tidb/issues/18714, imitate DM's use environment
// use isolated store, because in below failpoint we will cancel its context
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::MockTiKV));
// 	require::NoError(t, err);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| { require::NoError(t, store.Close()) });
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	defer dom.Close();
// 	se, err = session::CreateSession(store);
// 	require::NoError(t, err);
// 	defer se.Close();
//
// 	testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/storeCloseInLoop", func() {
// 		let _ = dom.DDL().Stop();
// 	});
//
// this DDL call will enter deadloop before this fix
// 	err = dom.DDLExecutor().CreateSchema(se, &ast::CreateDatabaseStmt{Name: ast::NewCIStr("testschema")});
// 	require::Equal(t, "context canceled", err.Error());
// }
//
// TestProcessInfoIssue22068 对应 Go 同名测试：长查询期间 ShowProcess 暴露 SQL 文本且 Plan 为空。
// #[test]
// pub fn TestProcessInfoIssue22068(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t(a int)");
// 	var wg util::WaitGroupWrapper;
// wg.Run 对应 Go goroutine 包装，保留并发等待结构但不实际启动线程。
// 	wg.Run(func() {
// 		tk.MustQuery("select 1 from t where a = (select sleep(5));").Check(testkit::Rows());
// 	});
// 	time::Sleep(2 * time::Second);
// 	let pi = tk.Session().ShowProcess();
// 	require::NotNil(t, pi);
// 	require::Equal(t, "select 1 from t where a = (select sleep(5));", pi.Info);
// 	require::Nil(t, pi.Plan);
// 	wg.Wait();
// }
//
// TestPerStmtTaskID 对应 Go 同名测试：同一事务中不同语句生成不同 TaskID。
// #[test]
// pub fn TestPerStmtTaskID(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table task_id (v int)");
//
// 	tk.MustExec("begin");
// 	tk.MustExec("select * from task_id where v > 10");
// 	let taskID1 = tk.Session().GetSessionVars().StmtCtx.TaskID;
// 	tk.MustExec("select * from task_id where v < 5");
// 	let taskID2 = tk.Session().GetSessionVars().StmtCtx.TaskID;
// 	tk.MustExec("commit");
//
// 	require::NotEqual(t, taskID1, taskID2);
// }
//
// TestStmtHints 对应 Go 同名测试：MEMORY_QUOTA、NO_INDEX_MERGE、STRAIGHT_JOIN、USE_TOJA、USE_CASCADES、READ_CONSISTENT_REPLICA hint 的 session 状态。
// #[test]
// pub fn TestStmtHints(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
//
// Test MEMORY_QUOTA hint
// 	tk.MustExec("select /*+ MEMORY_QUOTA(1 MB) */ 1;");
// 	val = int64(1) * 1024 * 1024;
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
// 	tk.MustExec("select /*+ MEMORY_QUOTA(1 GB) */ 1;");
// 	val = int64(1) * 1024 * 1024 * 1024;
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
// 	tk.MustExec("select /*+ MEMORY_QUOTA(1 GB), MEMORY_QUOTA(1 MB) */ 1;");
// 	val = int64(1) * 1024 * 1024;
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
// 	tk.MustExec("select /*+ MEMORY_QUOTA(0 GB) */ 1;");
// 	val = int64(-1);
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
// 	require::EqualError(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err, "Setting the MEMORY_QUOTA to 0 means no memory limit");
//
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t1(a int);");
// 	tk.MustExec("insert /*+ MEMORY_QUOTA(1 MB) */ into t1 (a) values (1);");
// 	val = int64(1) * 1024 * 1024;
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
//
// 	tk.MustExec("insert /*+ MEMORY_QUOTA(1 MB) */  into t1 select /*+ MEMORY_QUOTA(1 MB) */ * from t1;");
// 	val = int64(1) * 1024 * 1024;
// 	require::True(t, tk.Session().GetSessionVars().MemTracker.CheckBytesLimit(val));
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 2);
// 	require::EqualError(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err, "[planner:3126]Hint MEMORY_QUOTA(r#"1048576"#) is ignored as conflicting/duplicated.");
//
// Test NO_INDEX_MERGE hint
// 	tk.Session().GetSessionVars().SetEnableIndexMerge(true);
// 	tk.MustExec("select /*+ NO_INDEX_MERGE() */ 1;");
// 	require::True(t, tk.Session().GetSessionVars().StmtCtx.NoIndexMergeHint);
// 	tk.MustExec("select /*+ NO_INDEX_MERGE(), NO_INDEX_MERGE() */ 1;");
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::True(t, tk.Session().GetSessionVars().GetEnableIndexMerge());
//
// Test STRAIGHT_JOIN hint
// 	tk.MustExec("select /*+ straight_join() */ 1;");
// 	require::True(t, tk.Session().GetSessionVars().StmtCtx.StraightJoinOrder);
// 	tk.MustExec("select /*+ straight_join(), straight_join() */ 1;");
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
//
// Test USE_TOJA hint
// 	tk.Session().GetSessionVars().SetAllowInSubqToJoinAndAgg(true);
// 	tk.MustExec("select /*+ USE_TOJA(false) */ 1;");
// 	require::False(t, tk.Session().GetSessionVars().GetAllowInSubqToJoinAndAgg());
// 	tk.Session().GetSessionVars().SetAllowInSubqToJoinAndAgg(false);
// 	tk.MustExec("select /*+ USE_TOJA(true) */ 1;");
// 	require::True(t, tk.Session().GetSessionVars().GetAllowInSubqToJoinAndAgg());
// 	tk.MustExec("select /*+ USE_TOJA(false), USE_TOJA(true) */ 1;");
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::True(t, tk.Session().GetSessionVars().GetAllowInSubqToJoinAndAgg());
//
// Test USE_CASCADES hint
// 	tk.Session().GetSessionVars().SetEnableCascadesPlanner(true);
// 	tk.MustExec("select /*+ USE_CASCADES(false) */ 1;");
// 	require::False(t, tk.Session().GetSessionVars().GetEnableCascadesPlanner());
// 	tk.Session().GetSessionVars().SetEnableCascadesPlanner(false);
// 	tk.MustExec("select /*+ USE_CASCADES(true) */ 1;");
// 	require::True(t, tk.Session().GetSessionVars().GetEnableCascadesPlanner());
// 	tk.MustExec("select /*+ USE_CASCADES(false), USE_CASCADES(true) */ 1;");
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::EqualError(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings()[0].Err, "USE_CASCADES() is defined more than once, only the last definition takes effect: USE_CASCADES(true)");
// 	require::True(t, tk.Session().GetSessionVars().GetEnableCascadesPlanner());
//
// Test READ_CONSISTENT_REPLICA hint
// 	tk.Session().GetSessionVars().SetReplicaRead(kv::ReplicaReadLeader);
// 	tk.MustExec("select /*+ READ_CONSISTENT_REPLICA() */ 1;");
// 	require::Equal(t, kv::ReplicaReadFollower, tk.Session().GetSessionVars().GetReplicaRead());
// 	tk.MustExec("select /*+ READ_CONSISTENT_REPLICA(), READ_CONSISTENT_REPLICA() */ 1;");
// 	require::Len(t, tk.Session().GetSessionVars().StmtCtx.GetWarnings(), 1);
// 	require::Equal(t, kv::ReplicaReadFollower, tk.Session().GetSessionVars().GetReplicaRead());
// }
//
// TestRollbackOnCompileError 对应 Go 同名测试：编译期表名错误不污染事务并能在 rename 恢复后继续执行。
// #[test]
// pub fn TestRollbackOnCompileError(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t (a int)");
// 	tk.MustExec("insert t values (1)");
//
// 	let tk2 = testkit::NewTestKit(t, store);
// 	tk2.MustExec("use test");
// 	tk2.MustQuery("select * from t").Check(testkit::Rows("1"));
//
// 	tk.MustExec("rename table t to t2");
// 	let mut meetErr: bool;
// 	for _ in 0..100 {
// 		let _, err = tk2.Exec("insert t values (1)");
// 		if err != nil {
// 			let meetErr = true;
// 			break;
// 		}
// 	}
// 	require::True(t, meetErr);
//
// 	tk.MustExec("rename table t2 to t");
// 	let mut recoverErr: bool;
// 	for _ in 0..100 {
// 		let _, err = tk2.Exec("insert t values (1)");
// 		if err == nil {
// 			recoverErr = true;
// 			break;
// 		}
// 	}
// 	require::True(t, recoverErr);
// }
//
// TestResultField 对应 Go 同名测试：count(*) 字段类型和长度。
// #[test]
// pub fn TestResultField(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t (id int);");
//
// 	tk.MustExec(r#"INSERT INTO t VALUES (1);"#);
// 	tk.MustExec(r#"INSERT INTO t VALUES (2);"#);
// 	r, err = tk.Exec(r#"SELECT count(*) from t;"#);
// 	require::NoError(t, err);
// 	defer r.Close();
// 	let fields = r.Fields();
// 	require::NoError(t, err);
// 	require::Len(t, fields, 1);
// 	let field = fields[0].Column;
// 	require::Equal(t, mysql::TypeLonglong, field.GetType());
// 	require::Equal(t, 21, field.GetFlen());
// }
//
// Testcase for https://github.com/pingcap/tidb/issues/325
//
// TestResultType 对应 Go issue 325 回归：cast(null as char) 的 NULL 和字段类型。
// #[test]
// pub fn TestResultType(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	rs, err = tk.Exec(r#"select cast(null as char(30))"#);
// 	require::NoError(t, err);
// 	req = rs.NewChunk(nil);
// result set 迭代读取 chunk 的错误处理和空 chunk 退出条件按 Go 结构保留。
// 	err = rs.Next(context::Background(), req);
// 	require::NoError(t, err);
// 	require::True(t, req.GetRow(0).IsNull(0));
// 	require::Equal(t, mysql::TypeVarString, rs.Fields()[0].Column.FieldType.GetType());
// }
//
// TestFieldText 对应 Go 同名测试：字段显示文本保留注释、括号和表达式形态。
// #[test]
// pub fn TestFieldText(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t (a int)");
// 	let tests = []struct {
// 		sql   string;
// 		field string;
// 	}{
// 		{"select distinct(a) from t", "a"},
// 		{"select (1)", "1"},
// 		{"select (1+1)", "(1+1)"},
// 		{"select a from t", "a"},
// 		{"select        ((a+1))     from t", "((a+1))"},
// 		{"select 1 /*!32301 +1 */;", "1  +1 "},
// 		{"select /*!32301 1  +1 */;", "1  +1 "},
// 		{"/*!32301 select 1  +1 */;", "1  +1 "},
// 		{"select 1 + /*!32301 1 +1 */;", "1 +  1 +1 "},
// 		{"select 1 /*!32301 + 1, 1 */;", "1  + 1"},
// 		{"select /*!32301 1, 1 +1 */;", "1"},
// 		{"select /*!32301 1 + 1, */ +1;", "1 + 1"},
// 	}
// 	for _, tt = range tests {
// 		result, err = tk.Exec(tt.sql);
// 		require::NoError(t, err);
// 		require::Equal(t, tt.field, result.Fields()[0].ColumnAsName.O);
// 		result.Close();
// 	}
// }
//
// TestMatchIdentity 对应 Go 同名测试：用户 Host 匹配优先级、localhost 和 DNS 解析路径。
// #[test]
// pub fn TestMatchIdentity(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
//
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("CREATE USER r#"useridentity"#@r#"%"#");
// 	tk.MustExec("CREATE USER r#"useridentity"#@r#"localhost"#");
// 	tk.MustExec("CREATE USER r#"useridentity"#@r#"192.168.1.1"#");
// 	tk.MustExec("CREATE USER r#"useridentity"#@r#"example.com"#");
//
// The MySQL matching rule is most specific to least specific.
// So if I log in from 192.168.1.1 I should match that entry always.
// 	let identity, err = tk.Session().MatchIdentity(context::Background(), "useridentity", "192.168.1.1");
// 	require::NoError(t, err);
// 	require::Equal(t, "useridentity", identity.Username);
// 	require::Equal(t, "192.168.1.1", identity.Hostname);
//
// If I log in from localhost, I should match localhost
// 	let identity, err = tk.Session().MatchIdentity(context::Background(), "useridentity", "localhost");
// 	require::NoError(t, err);
// 	require::Equal(t, "useridentity", identity.Username);
// 	require::Equal(t, "localhost", identity.Hostname);
//
// If I log in from 192.168.1.2 I should match wildcard.
// 	let identity, err = tk.Session().MatchIdentity(context::Background(), "useridentity", "192.168.1.2");
// 	require::NoError(t, err);
// 	require::Equal(t, "useridentity", identity.Username);
// 	require::Equal(t, "%", identity.Hostname);
//
// 	let identity, err = tk.Session().MatchIdentity(context::Background(), "useridentity", "127.0.0.1");
// 	require::NoError(t, err);
// 	require::Equal(t, "useridentity", identity.Username);
// 	require::Equal(t, "localhost", identity.Hostname);
//
// This uses the lookup of example.com to get an IP address.
// We then login with that IP address, but expect it to match the example.com
// entry in the privileges table (by reverse lookup).
// DNS 解析是外部 IO，这里只保留 MatchIdentity 对反查路径的依赖。
// 	let ips, err = net::LookupHost("example.com");
// 	require::NoError(t, err);
// 	let identity, err = tk.Session().MatchIdentity(context::Background(), "useridentity", ips[0]);
// 	require::NoError(t, err);
// 	require::Equal(t, "useridentity", identity.Username);
// FIXME: we *should* match example.com instead
// as long as skip-name-resolve is not set (DEFAULT)
// 	require::Equal(t, "%", identity.Hostname);
// }
//
// TestHandleAssertionFailureForPartitionedTable 对应 Go 同名测试：分区表 assertion failpoint 不应输出 table 日志。
// #[test]
// pub fn TestHandleAssertionFailureForPartitionedTable(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	se = tk.Session();
// 	se.SetConnectionID(1);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t (a int, b int, c int, primary key(a, b)) partition by range (a) (partition p0 values less than (10), partition p1 values less than (20))");
// 	failpoint::Enable("github.com/pingcap/tidb/pkg/table/tables/addRecordForceAssertExist", "return");
// 	defer failpoint::Disable("github.com/pingcap/tidb/pkg/table/tables/addRecordForceAssertExist");
//
// 	let ctx, hook = testutil::WithLogHook(context::TODO(), t, "table");
// 	let _, err = tk.ExecWithContext(ctx, "insert into t values (1, 1, 1)");
// 	require::ErrorContains(t, err, "assertion");
// 	hook.CheckLogCount(t, 0);
// }
//
// TestRandomBinary 对应 Go 同名测试：NO_BACKSLASH_ESCAPES 下写入二进制 stats_top_n value。
// #[test]
// pub fn TestRandomBinary(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
//
// 	let ctx = kv::WithInternalSourceType(context::Background(), kv::InternalTxnStatsForegroundPriority);
// 	let allBytes = []Vec<u8>{
// 		{4, 0, 0, 0, 0, 0, 0, 4, '2'},
// 		{4, 0, 0, 0, 0, 0, 0, 4, '.'},
// 		{4, 0, 0, 0, 0, 0, 0, 4, '*'},
// 		{4, 0, 0, 0, 0, 0, 0, 4, '('},
// 		{4, 0, 0, 0, 0, 0, 0, 4, '\''},
// 		{4, 0, 0, 0, 0, 0, 0, 4, '!'},
// 		{4, 0, 0, 0, 0, 0, 0, 4, 29},
// 		{4, 0, 0, 0, 0, 0, 0, 4, 28},
// 		{4, 0, 0, 0, 0, 0, 0, 4, 23},
// 		{4, 0, 0, 0, 0, 0, 0, 4, 16},
// 	}
// 	let sql = "insert into mysql::stats_top_n (table_id, is_index, hist_id, value, count) values ";
// 	let mut val: String;
// 	for i, bytes = range allBytes {
// 		if i == 0 {
// 			val += sqlescape::MustEscapeSQL("(874, 0, 1, %?, 3)", bytes);
// 		} else {
// 			val += sqlescape::MustEscapeSQL(",(874, 0, 1, %?, 3)", bytes);
// 		}
// 	}
// 	sql += val;
// 	tk.MustExec("set sql_mode = 'NO_BACKSLASH_ESCAPES';");
// 	let _, err = tk.Session().ExecuteInternal(ctx, sql);
// 	require::NoError(t, err);
// }
//
// TestSQLModeOp 对应 Go 同名测试：SQL mode 增删位操作。
// #[test]
// pub fn TestSQLModeOp(t: testing::T) {
// 	let s = mysql::ModeNoBackslashEscapes | mysql::ModeOnlyFullGroupBy;
// 	let d = mysql::DelSQLMode(s, mysql::ModeANSIQuotes);
// 	require::Equal(t, s, d);
//
// 	let d = mysql::DelSQLMode(s, mysql::ModeNoBackslashEscapes);
// 	require::Equal(t, mysql::ModeOnlyFullGroupBy, d);
//
// 	let s = mysql::ModeNoBackslashEscapes | mysql::ModeOnlyFullGroupBy;
// 	let a = mysql::SetSQLMode(s, mysql::ModeOnlyFullGroupBy);
// 	require::Equal(t, s, a);
//
// 	let a = mysql::SetSQLMode(s, mysql::ModeAllowInvalidDates);
// 	require::Equal(t, mysql::ModeNoBackslashEscapes|mysql::ModeOnlyFullGroupBy|mysql::ModeAllowInvalidDates, a);
// }
//
// TestRequestSource 对应 Go 同名测试：TiKV 请求 source/origin 在写入和读取 RPC 中正确传递。
// #[test]
// pub fn TestRequestSource(t: testing::T) {
// 	tikvrpc::SetDefaultRequestOrigin(kvrpcpb::RequestOrigin_RequestOriginTiDB);
// t.Cleanup 对应 Go 测试结束清理，保留 domain/store 关闭语义。
// 	t.Cleanup(|| {
// 		tikvrpc::SetDefaultRequestOrigin(kvrpcpb::RequestOrigin_RequestOriginUnknown);
// 	});
//
// 	let store = testkit::CreateMockStore(t, mockstore::WithStoreType(mockstore::MockTiKV));
// 	let tk = testkit::NewTestKit(t, store);
// 	let withCheckInterceptor = func(source string) interceptor::RPCInterceptor {
// RPC interceptor 在 Go 中检查 TiKV request context，保留请求类型分支。
// 		return interceptor::NewRPCInterceptor("kv-request-source-verify", func(next interceptor::RPCInterceptorFunc) interceptor::RPCInterceptorFunc {
// 			return func(target string, req *tikvrpc::Request) (*tikvrpc::Response, error) {
// 				tikvrpc::AttachContext(req, req.Context);
// 				requestSource = "";
// 				requestOrigin = kvrpcpb::RequestOrigin_RequestOriginUnknown;
// 				readType = "";
// 				switch r = req.Req.(type) {
// 				case *kvrpcpb::PrewriteRequest:
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				case *kvrpcpb::CommitRequest:
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				case *coprocessor::Request:
// 					readType = "leader_" // read request will be attached with read type;
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				case *kvrpcpb::GetRequest:
// 					readType = "leader_" // read request will be attached with read type;
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				case *kvrpcpb::BatchGetRequest:
// 					readType = "leader_" // read request will be attached with read type;
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				case *kvrpcpb::PessimisticLockRequest:
// 					requestSource = r.GetContext().GetRequestSource();
// 					requestOrigin = r.GetContext().GetRequestOrigin();
// 				default:
// 					fmt::Printf("unexpected request type %T\n", r);
// 				}
// 				require::Equal(t, readType+source, requestSource);
// 				require::Equal(t, kvrpcpb::RequestOrigin_RequestOriginTiDB, requestOrigin);
// 				return next(target, req);
// 			}
// 		});
// 	}
// 	let ctx = context::Background();
// 	tk.MustExecWithContext(ctx, "use test");
// 	tk.MustExecWithContext(ctx, "create table t(a int primary key, b int)");
// 	tk.MustExecWithContext(ctx, "set @@tidb_request_source_type = 'lightning'");
// 	tk.MustQueryWithContext(ctx, "select @@tidb_request_source_type").Check(testkit::Rows("lightning"));
// 	let insertCtx = interceptor::WithRPCInterceptor(context::Background(), withCheckInterceptor("external_Insert_lightning"));
// 	tk.MustExecWithContext(insertCtx, "insert into t values(1, 1)");
// 	selectCtx = interceptor::WithRPCInterceptor(context::Background(), withCheckInterceptor("external_Select_lightning"));
// 	tk.MustExecWithContext(selectCtx, "select count(*) from t;");
// 	tk.MustQueryWithContext(selectCtx, "select b from t where a = 1;");
// 	tk.MustQueryWithContext(selectCtx, "select b from t where a in (1, 2, 3);");
// }
//
// TestEmptyInitSQLFile 对应 Go 同名测试：不存在 initialize-sql-file 时 bootstrap 失败。
// #[test]
// pub fn TestEmptyInitSQLFile(t: testing::T) {
// A non-existent sql file would stop the bootstrap of the tidb cluster
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// 	config::GetGlobalConfig().InitializeSQLFile = "non-existent.sql";
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, store.Close());
// 		config::GetGlobalConfig().InitializeSQLFile = "";
// 	});
//
// 	dom, err = session::BootstrapSession(store);
// 	require::Nil(t, dom);
// 	require::Error(t, err);
// }
//
// TestInitSystemVariable 对应 Go 同名测试：初始化 SQL 文件设置全局变量后启动效果可见。
// #[test]
// pub fn TestInitSystemVariable(t: testing::T) {
// We create an initialize-sql-file and then bootstrap the server with it.
// The observed behavior should be that tidb_enable_noop_variables is now
// disabled, and the feature works as expected.
// 临时文件用于 initialize SQL fixture，保留创建、写入、关闭和删除顺序。
// 	let initializeSQLFile, err = os::CreateTemp("", "init.sql");
// 	require::NoError(t, err);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		let path = initializeSQLFile.Name();
// 		err = initializeSQLFile.Close();
// 		require::NoError(t, err);
// 		err = os::Remove(path);
// 		require::NoError(t, err);
// 	});
// Implicitly test multi-line init files
// 	let _, err = initializeSQLFile.WriteString(;
// 		"CREATE DATABASE initsqlfiletest;\n" +;
// 			"SET GLOBAL tidb_enable_noop_variables = OFF;\n");
// 	require::NoError(t, err);
//
// Create a mock store
// Set the config parameter for initialize sql file
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// 	config::GetGlobalConfig().InitializeSQLFile = initializeSQLFile.Name();
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, store.Close());
// 		config::GetGlobalConfig().InitializeSQLFile = "";
// 	});
//
// Bootstrap with the InitializeSQLFile config option
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	defer dom.Close();
// 	se = session::CreateSessionAndSetID(t, store);
// 	let ctx = context::Background();
// 	r = session::MustExecToRecodeSet(t, se, r#"SHOW VARIABLES LIKE 'query_cache_type'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 0, req.NumRows()) // not shown in noopvariables mode;
// 	require::NoError(t, r.Close());
//
// 	r = session::MustExecToRecodeSet(t, se, r#"SHOW VARIABLES LIKE 'tidb_enable_noop_variables'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 1, req.NumRows());
// 	row = req.GetRow(0);
// 	require::Equal(t, Vec<u8>("OFF"), row.GetBytes(1));
// 	require::NoError(t, r.Close());
// }
//
// TestInitUsers 对应 Go 同名测试：只执行第一份 initialize SQL 文件并创建 cloud_admin、删除 root。
// #[test]
// pub fn TestInitUsers(t: testing::T) {
// Two sql files are set to 'initialize-sql-file' one after another,
// and only the first one is executed.
// 	var err error;
// 	let sqlFiles = make([]*os::File, 2);
// 	for i, name = range []string{"1.sql", "2.sql"} {
// 临时文件用于 initialize SQL fixture，保留创建、写入、关闭和删除顺序。
// 		sqlFiles[i], err = os::CreateTemp("", name);
// 		require::NoError(t, err);
// 	}
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		for _, sqlFile = range sqlFiles {
// 			let path = sqlFile.Name();
// 			err = sqlFile.Close();
// 			require::NoError(t, err);
// 			err = os::Remove(path);
// 			require::NoError(t, err);
// 		}
// 	});
// 	let _, err = sqlFiles[0].WriteString(`;
// CREATE USER cloud_admin;
// GRANT BACKUP_ADMIN, RESTORE_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT DASHBOARD_CLIENT on *.* TO 'cloud_admin'@'%';
// GRANT SYSTEM_VARIABLES_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT CONNECTION_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_VARIABLES_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_STATUS_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_CONNECTION_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_USER_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_TABLES_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT RESTRICTED_REPLICA_WRITER_ADMIN ON *.* TO 'cloud_admin'@'%';
// GRANT CREATE USER ON *.* TO 'cloud_admin'@'%';
// GRANT RELOAD ON *.* TO 'cloud_admin'@'%';
// GRANT PROCESS ON *.* TO 'cloud_admin'@'%';
// GRANT SELECT, INSERT, UPDATE, DELETE ON mysql::* TO 'cloud_admin'@'%';
// GRANT SELECT ON information_schema.* TO 'cloud_admin'@'%';
// GRANT SELECT ON performance_schema.* TO 'cloud_admin'@'%';
// GRANT SHOW DATABASES on *.* TO 'cloud_admin'@'%';
// GRANT REFERENCES ON *.* TO 'cloud_admin'@'%';
// GRANT SELECT ON *.* TO 'cloud_admin'@'%';
// GRANT INDEX ON *.* TO 'cloud_admin'@'%';
// GRANT INSERT ON *.* TO 'cloud_admin'@'%';
// GRANT UPDATE ON *.* TO 'cloud_admin'@'%';
// GRANT DELETE ON *.* TO 'cloud_admin'@'%';
// GRANT CREATE ON *.* TO 'cloud_admin'@'%';
// GRANT DROP ON *.* TO 'cloud_admin'@'%';
// GRANT ALTER ON *.* TO 'cloud_admin'@'%';
// GRANT CREATE VIEW ON *.* TO 'cloud_admin'@'%';
// GRANT SHUTDOWN, CONFIG ON *.* TO 'cloud_admin'@'%';
// REVOKE SHUTDOWN, CONFIG ON *.* FROM root;
//
// DROP USER root;
// `);
// 	require::NoError(t, err);
// 	let _, err = sqlFiles[1].WriteString("drop user cloud_admin;");
// 	require::NoError(t, err);
//
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// 	config::GetGlobalConfig().InitializeSQLFile = sqlFiles[0].Name();
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, store.Close());
// 		config::GetGlobalConfig().InitializeSQLFile = "";
// 	});
//
// Bootstrap with the first sql file
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	se = session::CreateSessionAndSetID(t, store);
// 	let ctx = context::Background();
// 'cloud_admin' has been created successfully
// 	r = session::MustExecToRecodeSet(t, se, r#"select user from mysql::user where user = 'cloud_admin'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 1, req.NumRows());
// 	row = req.GetRow(0);
// 	require::Equal(t, "cloud_admin", row.GetString(0));
// 	require::NoError(t, r.Close());
// 'root' has been deleted successfully
// 	r = session::MustExecToRecodeSet(t, se, r#"select user from mysql::user where user = 'root'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 0, req.NumRows());
// 	require::NoError(t, r.Close());
// 	dom.Close();
//
// 	session::DisableRunBootstrapSQLFileInTest();
//
// Bootstrap with the second sql file, which would not been executed.
// 	config::GetGlobalConfig().InitializeSQLFile = sqlFiles[1].Name();
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	se = session::CreateSessionAndSetID(t, store);
// 	r = session::MustExecToRecodeSet(t, se, r#"select user from mysql::user where user = 'cloud_admin'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 1, req.NumRows());
// 	row = req.GetRow(0);
// 	require::Equal(t, "cloud_admin", row.GetString(0));
// 	require::NoError(t, r.Close());
// 	dom.Close();
// }
//
// TestBootstrapSQLWithExtension 对应 Go 同名测试：扩展认证插件可在初始化 SQL 创建用户时生效。
// #[test]
// pub fn TestBootstrapSQLWithExtension(t: testing::T) {
// 	let authChecks = []*extension::AuthPlugin{{
// 		Name: "my_auth_plugin",
// 		AuthenticateUser: func(request extension::AuthenticateRequest) error {
// 			return nil;
// 		},
// 		ValidateAuthString: func(pwdHash string) bool {
// 			return pwdHash != "";
// 		},
// 		GenerateAuthString: func(pwd string) (string, bool) {
// 			return pwd, pwd != "";
// 		},
// 		RequiredClientSidePlugin: mysql::AuthNativePassword,
// 	}}
//
// 	require::NoError(t, extension::Register(;
// 		"extension_authentication_plugin",
// 		extension::WithCustomAuthPlugins(authChecks),
// 		extension::WithCustomSysVariables([]*variable::SysVar{
// 			{
// 				Scope:          vardef::ScopeGlobal,
// 				Name:           "extension_authentication_plugin",
// 				Value:          mysql::AuthNativePassword,
// 				Type:           vardef::TypeEnum,
// 				PossibleValues: []string{authChecks[0].Name},
// 			},
// 		}),
// 	));
// 	require::NoError(t, extension::Setup());
// 	let ext, err = extension::GetExtensions();
// 	require::NoError(t, err);
//
// 临时文件用于 initialize SQL fixture，保留创建、写入、关闭和删除顺序。
// 	let sqlFile, err = os::CreateTemp("", "TestBootstrapSQLWithExtension.sql");
// 	require::NoError(t, err);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		let path = sqlFile.Name();
// 		err = sqlFile.Close();
// 		require::NoError(t, err);
// 		err = os::Remove(path);
// 		require::NoError(t, err);
// 	});
// Create a user with the custom auth plugin.
// 	let _, err = sqlFile.WriteString(r#"CREATE USER myuser IDENTIFIED WITH my_auth_plugin BY 'password';"#);
// 	require::NoError(t, err);
//
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// 	config::GetGlobalConfig().InitializeSQLFile = sqlFile.Name();
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, store.Close());
// 		config::GetGlobalConfig().InitializeSQLFile = "";
// 	});
//
// Bootstrap with the sql file
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	se = session::CreateSessionAndSetID(t, store);
// 	se.SetExtensions(ext.NewSessionExtensions());
// 	let ctx = context::Background();
// 'myuser' has been created successfully
// 	r = session::MustExecToRecodeSet(t, se, r#"select user from mysql::user where user = 'myuser'"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 1, req.NumRows());
// 	row = req.GetRow(0);
// 	require::Equal(t, "myuser", row.GetString(0));
// 	require::NoError(t, r.Close());
// 	dom.Close();
// }
//
// TestErrorHappenWhileInit 对应 Go 同名测试：初始化 SQL parser error 会失败，普通执行错误被忽略并保留前序 DDL。
// #[test]
// pub fn TestErrorHappenWhileInit(t: testing::T) {
// 1. parser error in sql file (1.sql) makes the bootstrap panic
// 2. other errors in sql file (2.sql) will be ignored
// 	var err error;
// 	let sqlFiles = make([]*os::File, 2);
// 	for i, name = range []string{"1.sql", "2.sql"} {
// 临时文件用于 initialize SQL fixture，保留创建、写入、关闭和删除顺序。
// 		sqlFiles[i], err = os::CreateTemp("", name);
// 		require::NoError(t, err);
// 	}
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		for _, sqlFile = range sqlFiles {
// 			let path = sqlFile.Name();
// 			err = sqlFile.Close();
// 			require::NoError(t, err);
// 			err = os::Remove(path);
// 			require::NoError(t, err);
// 		}
// 	});
// 	let _, err = sqlFiles[0].WriteString("create table test.t (c in);");
// 	require::NoError(t, err);
// 	let _, err = sqlFiles[1].WriteString(`;
// create table test.t (c int);
// insert into test.t values ("abc"); -- invalid statement;
// `);
// 	require::NoError(t, err);
//
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// 	config::GetGlobalConfig().InitializeSQLFile = sqlFiles[0].Name();
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		config::GetGlobalConfig().InitializeSQLFile = "";
// 	});
//
// Bootstrap with the first sql file
// 	dom, err = session::BootstrapSession(store);
// 	require::Nil(t, dom);
// 	require::Error(t, err);
// 	require::NoError(t, store.Close());
//
// 	session::DisableRunBootstrapSQLFileInTest();
//
// Bootstrap with the second sql file, which would not been executed.
// 	let store, err = mockstore::NewMockStore(mockstore::WithStoreType(mockstore::EmbedUnistore));
// 	require::NoError(t, err);
// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
// 	defer!(|| {
// 		require::NoError(t, store.Close());
// 	});
// 	config::GetGlobalConfig().InitializeSQLFile = sqlFiles[1].Name();
// 	dom, err = session::BootstrapSession(store);
// 	require::NoError(t, err);
// 	se = session::CreateSessionAndSetID(t, store);
// 	let ctx = context::Background();
// 	let _ = session::MustExecToRecodeSet(t, se, r#"use test;"#);
// 	require::NoError(t, err);
// Table t has been created.
// 	r = session::MustExecToRecodeSet(t, se, r#"show tables;"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 1, req.NumRows());
// 	row = req.GetRow(0);
// 	require::Equal(t, "t", row.GetString(0));
// 	require::NoError(t, r.Close());
// But data is failed to inserted since the error
// 	r = session::MustExecToRecodeSet(t, se, r#"select * from test.t"#);
// 	require::NoError(t, err);
// 	req = r.NewChunk(nil);
// record set 迭代读取 chunk 的错误处理按 Go 结构保留。
// 	err = r.Next(ctx, req);
// 	require::NoError(t, err);
// 	require::Equal(t, 0, req.NumRows());
// 	require::NoError(t, r.Close());
// 	dom.Close();
// }
//
// TestIssue60266 对应 Go issue 60266 回归：NO_BACKSLASH_ESCAPES 下生成列 regexp_replace 与反斜线结果。
// #[test]
// pub fn TestIssue60266(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test;");
// 	tk.MustExec("set session sql_mode='NO_BACKSLASH_ESCAPES';");
// 	tk.MustExec(r#"create table t1(id bigint primary key, a text, b text as ((regexp_replace(a, '^[1-9]\d{9,29}$', 'aaaaa'))), c text)"#);
// 	tk.MustExec(r#"insert into t1 (id, a, c) values(1,123456, 'ab\\\\c');"#);
// 	tk.MustExec(r#"insert into t1 (id, a, c) values(2,1234567890123, 'ab\\c');"#);
// 	tk.MustQuery("select * from t1;").Sort().;
// 		Check(testkit::Rows("1 123456 123456 ab\\\\\\\\c", "2 1234567890123 aaaaa ab\\\\c"));
// }
//
// expectTxnStart 对应 Go 辅助函数：后台执行长查询并轮询 process list，确认 CurTxnStartTS 后发送 kill。
// pub fn expectTxnStart(t: testing::T, store: kv::Storage, execute: impl Fn(&testkit::TestKit), txnStart: u64) {
// 	let tk = testkit::NewTestKit(t, store);
//
// 	let wg = sync::WaitGroup::new();
// 	wg.Add(1);
// Go goroutine 在这里作为并发语义保留。
// 	go func() {
// 		defer wg.Done();
//
// 		execute(tk);
// 	});
//
// 	let tk2 = testkit::NewTestKit(t, store);
// 	require::Eventually(t, func() bool {
// 		let sm = tk2.Session().GetSessionManager();
// 		if sm == nil {
// 			return false;
// 		}
//
// 		let pl = sm.ShowProcessList();
// 		for _, pi = range pl {
// 			if pi.ID == tk.Session().GetSessionVars().ConnectionID {
// 				if pi.CurTxnStartTS == txnStart {
// 					return true;
// 				}
//
// 				logutil::BgLogger().Info("ProcessInfo TxnStartTS for current process is not correct",
// 					zap::Uint64("expected", txnStart),
// 					zap::Uint64("actual", pi.CurTxnStartTS));
// 				return false;
// 			}
// 		}
// 		return false;
// 	}, 5*time::Second, 100*time::Millisecond, "ProcessInfo TxnStartTS for current process is not correct");
//
// 	tk.Session().GetSessionVars().SQLKiller.SendKillSignal(sqlkiller::QueryInterrupted);
// 	wg.Wait();
// }
//
// TestProcessInfoForStaleReadAutoCommit 对应 Go 同名测试：外部时间戳读和 as-of timestamp 读在 process info 中记录正确 txn start。
// #[test]
// pub fn TestProcessInfoForStaleReadAutoCommit(t: testing::T) {
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("use test");
// 	tk.MustExec("create table t(a int)");
// 	tk.MustExec("insert into t values (1)");
//
// 	tk.MustExec("begin");
// 	tsStr = tk.MustQuery("select @@tidb_current_ts;").Rows()[0][0];
// 	tk.MustExec("set global tidb_external_ts = @@tidb_current_ts;");
// 	tk.MustExec("commit");
//
// 	ts, err = strconv::Atoi(tsStr.(string));
// 	require::NoError(t, err);
// 	expectTxnStart(t, store, func(tk *testkit::TestKit) {
// 		tk.MustExec("use test");
// 		tk.MustExec("set tidb_enable_external_ts_read = ON;");
// 		err = tk.QueryToErr("select *, sleep(1000) from t");
// 		require::Contains(t, err.Error(), "Query execution was interrupted");
// 	}, uint64(ts));
//
// increase the ts to make it strictly greater than the previous ts, to avoid that
// the `t` is still empty or it cannot find the schema of `t`
// 	time::Sleep(time::Millisecond);
// 	tsTime = oracle::GetTimeFromTS(uint64(ts)).In(tk.Session().GetSessionVars().Location()).Add(time::Millisecond);
// Convert to the ts again to avoid precision issue
// 	ts = int(oracle::GoTimeToTS(tsTime));
// 	expectTxnStart(t, store, func(tk *testkit::TestKit) {
// 		tk.MustExec("use test");
// 		err = tk.QueryToErr(fmt::Sprintf("select *, sleep(1000) from t as of timestamp '%s';",
// 			tsTime));
// 		require::Contains(t, err.Error(), "Query execution was interrupted");
// 	}, uint64(ts));
// }
//
// TestGetDBNames 对应 Go 同名测试：打开 RecordDBLabel 后多种 SQL 操作均保持当前数据库标签。
// #[test]
// pub fn TestGetDBNames(t: testing::T) {
// 	let originCfg = config::GetGlobalConfig();
// 	let newCfg = *originCfg;
// 	newCfg.Status.RecordDBLabel = true;
// 	config::StoreGlobalConfig(&newCfg);
// 	defer config::StoreGlobalConfig(originCfg);
//
// 	let store = testkit::CreateMockStore(t);
// 	let tk = testkit::NewTestKit(t, store);
// 	tk.MustExec("create database DatabaseA;");
// 	tk.MustExec("use DatabaseA;");
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustExec(r#"create table t1(id bigint primary key, a int, b varchar(32), c text)"#);
// 	tk.MustExec(r#"create table t2(id bigint primary key, a int, b varchar(32), c text)"#);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustExec(r#"insert into t1 (id, b, c) values(1, 'ab', 'ab\\\\c');"#);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustQuery("select * from t1 where id = 1").Check(testkit::Rows("1 <nil> ab ab\\\\c"));
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustExec(r#"insert into t1 (id, b, c) values(2, 'xy', 'ab\\c');"#);
// 	tk.MustExec(r#"update t1 set a = 123 where id = 2"#);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustExec(r#"delete from t1 where id = 1;"#);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustQuery("select * from t1;").Check(testkit::Rows("2 123 xy ab\\c"));
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	require::ErrorIs(t, tk.ExecToErr("IMPORT INTO t1(a) FROM select * from t2;"),
// 		plannererrors::ErrWrongValueCountOnRow);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustQuery("show tables");
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// 	tk.MustExec(r#"drop table t1"#);
// 	let dbs = session::GetDBNames(tk.Session().GetSessionVars());
// 	require::Equal(t, dbs[0], "databasea");
// }
// "################;
//
// use astersql_session::session::{Statement, StatementContext, StmtHistory};
//
// #[test]
// fn statement_history_records_retry_context_in_order() {
//     let mut history = StmtHistory::default();
//     history.Add(
//         Statement {
//             sql: "insert into t values (1)".into(),
//         },
//         StatementContext {
//             LastInsertID: 1,
//             AffectedRows: 1,
//             ..Default::default()
//         },
//     );
//     history.Add(
//         Statement {
//             sql: "update t set a=2".into(),
//         },
//         StatementContext {
//             AffectedRows: 1,
//             ..Default::default()
//         },
//     );
//     assert_eq!(history.Count(), 2);
// }
// */
use astersql_parser_mysql::r#const::{
    DelSQLMode, ModeAllowInvalidDates, ModeNoBackslashEscapes, ModeOnlyFullGroupBy, SQLMode,
    SetSQLMode,
};
use astersql_session::GetDBNames;
use astersql_session::bootstrap::{
    BootstrapError, BootstrapRuntime, DatabaseBasicInfo, SqlValue, TableBasicInfo,
    doBootstrapSQLFile,
};
use astersql_sessionctx_variable::session::SessionVars;
use astersql_testkit::{DbValue, NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};
use astersql_util_sqlescape::{EscapeSQL, MustEscapeSQL, SqlArg};

#[derive(Default)]
struct BootstrapSqlRuntime {
    contents: Option<String>,
    parse_error: Option<String>,
    fail_statement: Option<String>,
    executed: Vec<String>,
}

impl BootstrapRuntime for BootstrapSqlRuntime {
    type Error = String;
    type LockGuard = ();

    fn init_mdl_for_bootstrap(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn is_ddl_owner(&mut self) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn upgrade(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn sleep(&mut self, _duration: std::time::Duration) {}

    fn use_system_database(&mut self) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn read_tidb_variable(&mut self, _name: &str) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    fn commit_transaction(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn execute_internal(
        &mut self,
        _sql: &str,
        _args: &[SqlValue],
        _timeout: std::time::Duration,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn query_global_variable(&mut self, _name: &str) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    fn acquire_distributed_lock(&mut self, _key: &str) -> Result<Self::LockGuard, Self::Error> {
        Ok(())
    }

    fn nextgen_schema_version(&mut self) -> Result<i32, Self::Error> {
        Ok(0)
    }

    fn create_system_database(&mut self, _database: DatabaseBasicInfo) -> Result<(), Self::Error> {
        Ok(())
    }

    fn create_and_split_system_table(
        &mut self,
        _database_id: i64,
        _table: TableBasicInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_nextgen_schema_version(&mut self, _version: i32) -> Result<(), Self::Error> {
        Ok(())
    }

    fn classic_kernel(&mut self) -> bool {
        true
    }

    fn insert_builtin_bind_info(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn initialize_sql_file(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.contents.clone())
    }

    fn parse_sql(&mut self, sql: &str) -> Result<Vec<String>, Self::Error> {
        if let Some(error) = self.parse_error.clone() {
            return Err(error);
        }
        Ok(sql
            .split(';')
            .map(str::trim)
            .filter(|statement| !statement.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn execute_statement(&mut self, statement: &str) -> Result<(), Self::Error> {
        self.executed.push(statement.to_owned());
        if self.fail_statement.as_deref() == Some(statement) {
            return Err(format!("failed statement: {statement}"));
        }
        Ok(())
    }

    fn secure_bootstrap_user(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }

    fn global_system_variables(&mut self) -> Result<Vec<(String, String)>, Self::Error> {
        Ok(Vec::new())
    }

    fn current_bootstrap_version(&mut self) -> i64 {
        1
    }

    fn new_collation_enabled_on_first_bootstrap(&mut self) -> bool {
        false
    }

    fn write_system_timezone(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_new_collation_parameter(&mut self, _enabled: bool) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_statement_summary_variables(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_ddl_table_version(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_cluster_id(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn sha1(&mut self, _bytes: &[u8]) -> [u8; 20] {
        [0; 20]
    }

    fn rebuild_partition_maps(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn bootstrap_sql_file_empty_parse_and_statement_errors_match_go() {
    let mut empty = BootstrapSqlRuntime::default();
    doBootstrapSQLFile(&mut empty).unwrap();
    assert!(empty.executed.is_empty());

    let mut parse_failure = BootstrapSqlRuntime {
        contents: Some("create table test.t(c in)".to_owned()),
        parse_error: Some("syntax error".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        doBootstrapSQLFile(&mut parse_failure),
        Err(BootstrapError::External("syntax error".to_owned()))
    );
    assert!(parse_failure.executed.is_empty());

    let mut execution_failure = BootstrapSqlRuntime {
        contents: Some(
            "set global tidb_enable_noop_functions=1;\
             create user myuser identified with my_auth_plugin by 'password';\
             insert invalid;\
             create user cloud_admin"
                .to_owned(),
        ),
        fail_statement: Some("insert invalid".to_owned()),
        ..Default::default()
    };
    doBootstrapSQLFile(&mut execution_failure).unwrap();
    assert_eq!(
        execution_failure.executed,
        [
            "set global tidb_enable_noop_functions=1",
            "create user myuser identified with my_auth_plugin by 'password'",
            "insert invalid",
            "create user cloud_admin",
        ]
    );
}

/// 对齐 Go `TestSQLModeOp`：删除不存在的 bit 不改变原值，设置已有 bit 幂等。
#[test]
fn sql_mode_bit_operations_match_go() {
    let mode = SetSQLMode(ModeNoBackslashEscapes, ModeOnlyFullGroupBy);
    assert_eq!(DelSQLMode(mode, ModeAllowInvalidDates), mode);
    assert_eq!(
        DelSQLMode(mode, ModeNoBackslashEscapes),
        ModeOnlyFullGroupBy
    );
    assert_eq!(SetSQLMode(mode, ModeOnlyFullGroupBy), mode);
    assert_eq!(
        SetSQLMode(mode, ModeAllowInvalidDates),
        SQLMode(mode.0 | ModeAllowInvalidDates.0)
    );
}

#[test]
fn primary_key_auto_increment_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("drop table if exists t", vec![]);
    tk.MustExec(
        "create table t (id bigint primary key auto_increment not null, name varchar(255) unique not null, status int)",
        vec![],
    );
    let inserted = tk
        .Exec("insert t (name) values (?)", vec![DbValue::from("abc")])
        .unwrap();
    assert_ne!(inserted.last_insert_id, 0);
    tk.MustQuery("select * from t", vec![])
        .Check(Rows(&[&format!("{} abc <nil>", inserted.last_insert_id)]));
    tk.MustExec(
        "update t set name = 'abc', status = 1 where id = ?",
        vec![DbValue::from(inserted.last_insert_id)],
    );
    tk.MustQuery("select * from t", vec![])
        .Check(Rows(&[&format!("{} abc 1", inserted.last_insert_id)]));

    tk.MustExec("drop table if exists t", vec![]);
    tk.MustExec("create table t (id tinyint)", vec![]);
    tk.MustExec("insert t values (?)", vec![DbValue::from(true)]);
    tk.MustQuery("select * from t", vec![]).Check(Rows(&["1"]));
}

#[test]
fn rollback_on_compile_error_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    let mut tk2 = NewTestKit(store);
    for session in [&mut tk, &mut tk2] {
        session.MustExec("use test", vec![]);
    }
    tk.MustExec("create table t (a int)", vec![]);
    tk.MustExec("insert t values (1)", vec![]);
    tk2.MustQuery("select * from t", vec![]).Check(Rows(&["1"]));

    tk.MustExec("rename table t to t2", vec![]);
    assert!(
        (0..100).any(|_| tk2.Exec("insert t values (1)", vec![]).is_err()),
        "renamed table must invalidate the stale statement"
    );

    tk.MustExec("rename table t2 to t", vec![]);
    assert!(
        (0..100).any(|_| tk2.Exec("insert t values (1)", vec![]).is_ok()),
        "statement must recover after the table name is restored"
    );
}

#[test]
fn result_type_and_field_text_match_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("create table t (a int)", vec![]);
    let rows = tk.Query("select cast(null as char(30))", vec![]).unwrap();
    assert_eq!(rows.rows, vec![vec![DbValue::Null]]);

    let cases = [
        ("select distinct(a) from t", "a"),
        ("select (1)", "1"),
        ("select (1+1)", "(1+1)"),
        ("select a from t", "a"),
        ("select        ((a+1))     from t", "((a+1))"),
        ("select 1 /*!32301 +1 */;", "1  +1 "),
        ("select /*!32301 1  +1 */;", "1  +1 "),
        ("/*!32301 select 1  +1 */;", "1  +1 "),
        ("select 1 + /*!32301 1 +1 */;", "1 +  1 +1 "),
        ("select 1 /*!32301 + 1, 1 */;", "1  + 1"),
        ("select /*!32301 1, 1 +1 */;", "1"),
        ("select /*!32301 1 + 1, */ +1;", "1 + 1"),
    ];
    for (sql, expected) in cases {
        let (statement_id, fields) = tk.Session().PrepareStmt(sql).unwrap();
        assert!(!fields.is_empty(), "sql={sql:?}");
        assert_eq!(fields[0].column_as_name, expected, "sql={sql:?}");
        tk.Session().DropPreparedStmt(statement_id).unwrap();
    }
}

#[test]
fn issue_60266_no_backslash_escapes_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("set session sql_mode='NO_BACKSLASH_ESCAPES'", vec![]);
    tk.MustExec(
        r#"create table t1(id bigint primary key, a text, b text as ((regexp_replace(a, '^[1-9]\d{9,29}$', 'aaaaa'))), c text)"#,
        vec![],
    );
    tk.MustExec(
        r#"insert into t1 (id, a, c) values(1,123456, 'ab\\\\c')"#,
        vec![],
    );
    tk.MustExec(
        r#"insert into t1 (id, a, c) values(2,1234567890123, 'ab\\c')"#,
        vec![],
    );
    let mut rows = tk.MustQuery("select * from t1", vec![]);
    rows.Sort().Check(Rows(&[
        r#"1 123456 123456 ab\\\\c"#,
        r#"2 1234567890123 aaaaa ab\\c"#,
    ]));
}

#[test]
fn schema_checker_sql_matches_go_normal_and_partition_paths() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    let mut ddl = NewTestKit(store);
    for session in [&mut tk, &mut ddl] {
        session.MustExec("use test", vec![]);
    }
    tk.MustExec("create table t (id int, c int)", vec![]);
    tk.MustExec("create table t1 (id int, c int)", vec![]);
    tk.MustExec("insert into t values(1, 1)", vec![]);

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table t modify column c bigint", vec![]);
    tk.MustExec("insert into t values(3, 3)", vec![]);
    let error = tk.Exec("commit", vec![]).unwrap_err();
    assert!(
        error
            .message()
            .starts_with("[domain:8028]Information schema is changed"),
        "{error}"
    );

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table t add index idx2(c)", vec![]);
    tk.MustExec("insert into t1 values(4, 4)", vec![]);
    tk.MustExec("commit", vec![]);

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table t add index idx3(c)", vec![]);
    tk.MustQuery("select * from t for update", vec![]);
    assert!(tk.Exec("commit", vec![]).is_err());

    tk.MustExec(
        "create table pt (id int, c int) partition by hash (id) partitions 3",
        vec![],
    );
    tk.MustExec("insert into pt values(1, 1)", vec![]);
    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table pt modify column c bigint", vec![]);
    tk.MustExec("insert into pt values(3, 3)", vec![]);
    let error = tk.Exec("commit", vec![]).unwrap_err();
    assert!(
        error
            .message()
            .starts_with("[domain:8028]Information schema is changed"),
        "{error}"
    );

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table pt add index idx2(c)", vec![]);
    tk.MustExec("insert into t1 values(5, 5)", vec![]);
    tk.MustExec("commit", vec![]);

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table pt add index idx3(c)", vec![]);
    tk.MustQuery("select * from pt for update", vec![]);
    assert!(tk.Exec("commit", vec![]).is_err());

    tk.MustExec("begin", vec![]);
    ddl.MustExec("alter table pt add index idx4(c)", vec![]);
    tk.MustQuery("select * from pt partition (p1) for update", vec![]);
    assert!(tk.Exec("commit", vec![]).is_err());
}

#[test]
fn write_on_multiple_cached_tables_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("drop table if exists ct1, ct2", vec![]);
    tk.MustExec("create table ct1 (id int, c int)", vec![]);
    tk.MustExec("create table ct2 (id int, c int)", vec![]);
    tk.MustExec("alter table ct1 cache", vec![]);
    tk.MustExec("alter table ct2 cache", vec![]);
    tk.MustQuery("select * from ct1", vec![]).Check(Rows(&[]));
    tk.MustQuery("select * from ct2", vec![]).Check(Rows(&[]));

    tk.MustExec("begin", vec![]);
    tk.MustExec("insert into ct1 values (3, 4)", vec![]);
    tk.MustExec("insert into ct2 values (5, 6)", vec![]);
    tk.MustExec("commit", vec![]);
    tk.MustQuery("select * from ct1", vec![])
        .Check(Rows(&["3 4"]));
    tk.MustQuery("select * from ct2", vec![])
        .Check(Rows(&["5 6"]));
    tk.MustExec("alter table ct1 nocache", vec![]);
    tk.MustExec("alter table ct2 nocache", vec![]);
}

#[test]
fn prepare_zero_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("drop table if exists t", vec![]);
    tk.MustExec("create table t(v timestamp)", vec![]);
    let prepared = tk.Prepare("insert into t (v) values (?)");
    assert!(prepared.execute(&[DbValue::from("0")]).is_err());
    tk.MustExec("set @orig_sql_mode=@@sql_mode", vec![]);
    tk.MustExec("set @@sql_mode=''", vec![]);
    prepared
        .execute(&[DbValue::from("0000-00-00 00:00:00")])
        .unwrap();
    tk.MustQuery("select v from t", vec![])
        .Check(Rows(&["0000-00-00 00:00:00"]));
    tk.MustExec("set @@sql_mode=@orig_sql_mode", vec![]);
}

#[test]
fn fix_set_tidb_snapshot_ts_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("create database t123", vec![]);
    tk.MustExec("set @schema_ts=@@tidb_current_ts", vec![]);
    tk.MustExec("drop database t123", vec![]);
    assert!(tk.Exec("use t123", vec![]).is_err());
    tk.MustExec("set @@tidb_snapshot=@schema_ts", vec![]);
    tk.MustExec("use t123", vec![]);
    tk.MustExec(
        "set session sql_mode = 'STRICT_TRANS_TABLES,NO_AUTO_CREATE_USER'",
        vec![],
    );
    tk.MustExec("use t123", vec![]);
}

#[test]
fn random_binary_internal_sql_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    let all_bytes = [
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'2'],
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'.'],
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'*'],
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'('],
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'\''],
        vec![4, 0, 0, 0, 0, 0, 0, 4, b'!'],
        vec![4, 0, 0, 0, 0, 0, 0, 4, 29],
        vec![4, 0, 0, 0, 0, 0, 0, 4, 28],
        vec![4, 0, 0, 0, 0, 0, 0, 4, 23],
        vec![4, 0, 0, 0, 0, 0, 0, 4, 16],
    ];
    let values = all_bytes
        .into_iter()
        .enumerate()
        .map(|(index, bytes)| {
            MustEscapeSQL(
                if index == 0 {
                    "(874, 0, 1, %?, 3)"
                } else {
                    ",(874, 0, 1, %?, 3)"
                },
                &[SqlArg::Bytes(Some(bytes))],
            )
        })
        .collect::<String>();
    let sql = format!(
        "insert into mysql.stats_top_n (table_id, is_index, hist_id, value, count) values {values}"
    );
    tk.MustExec("set sql_mode = 'NO_BACKSLASH_ESCAPES'", vec![]);
    let context = astersql_kv::WithInternalSourceType(
        astersql_kv::Context::new(),
        astersql_kv::InternalTxnStatsForegroundPriority,
    );
    tk.Session().ExecuteInternal(&context, &sql, &[]).unwrap();
}

#[test]
fn parse_with_params_escape_contract_matches_go() {
    assert_eq!(EscapeSQL("SELECT 4", &[]).unwrap(), "SELECT 4");
    assert_eq!(EscapeSQL("SELECT 3", &[]).unwrap(), "SELECT 3");
    assert_eq!(
        EscapeSQL(
            "SELECT * FROM test WHERE name = %? LIMIT 1",
            &[SqlArg::String("' OR 1=1 /*".to_owned())],
        )
        .unwrap(),
        "SELECT * FROM test WHERE name = '\\' OR 1=1 /*' LIMIT 1"
    );
    assert_eq!(
        EscapeSQL("SELECT %?, %?", &[SqlArg::Int(3)])
            .unwrap_err()
            .to_string(),
        "missing arguments, need 2-th arg, but only got 1 args"
    );

    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustQuery("SELECT 4", vec![]).Check(Rows(&["4"]));
    assert!(tk.Exec("SELECT", vec![]).is_err());
}

#[test]
fn result_field_metadata_matches_go_query_shape() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", vec![]);
    tk.MustExec("create table t (id int)", vec![]);
    tk.MustExec("insert into t values (1), (2)", vec![]);
    tk.MustQuery("select count(*) from t", vec![])
        .Check(Rows(&["2"]));

    let (statement_id, fields) = tk.Session().PrepareStmt("select count(*) from t").unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].column_as_name, "count(*)");
    tk.Session().DropPreparedStmt(statement_id).unwrap();
}

#[test]
fn match_identity_specificity_matches_go() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut admin = NewTestKit(store.clone());
    admin.MustExec("use test", vec![]);
    admin.MustExec("create user 'useridentity'@'%'", vec![]);
    admin.MustExec("create user 'useridentity'@'localhost'", vec![]);
    admin.MustExec("create user 'useridentity'@'192.168.1.1'", vec![]);
    admin.MustExec("create user 'useridentity'@'example.com'", vec![]);

    for (login_host, matched_host) in [
        ("192.168.1.1", "192.168.1.1"),
        ("localhost", "localhost"),
        ("192.168.1.2", "%"),
        ("127.0.0.1", "localhost"),
    ] {
        let mut login = NewTestKit(store.clone());
        login
            .Session()
            .database()
            .authenticate_user_for_test("useridentity", login_host)
            .unwrap();
        let expected = format!("useridentity@{matched_host}");
        login
            .MustQuery("select current_user()", vec![])
            .Check(Rows(&[expected.as_str()]));
    }
}

#[test]
fn internal_request_source_is_required_and_preserved() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let tk = NewTestKit(store);
    let error = tk
        .Session()
        .QueryInternal(&astersql_kv::Context::new(), "select 1", &[])
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "ExecuteInternal requires an internal request source"
            .replace("ExecuteInternal", "QueryInternal")
    );

    let context = astersql_kv::WithInternalSourceType(
        astersql_kv::Context::new(),
        astersql_kv::InternalTxnStatsForegroundPriority,
    );
    assert_eq!(
        tk.Session()
            .QueryInternal(&context, "select 1", &[])
            .unwrap()
            .string_rows(),
        vec![vec!["1".to_owned()]]
    );
}

#[test]
fn get_db_names_matches_go_config_and_statement_fallbacks() {
    let original = astersql_config::get_global_config();
    let mut disabled = original.as_ref().clone();
    disabled.status.record_db_label = false;
    astersql_config::store_global_config(disabled);

    let variables = SessionVars::default();
    variables.SetCurrentDB("DatabaseA");
    let nil_when_disabled = GetDBNames(None);
    let session_when_disabled = GetDBNames(Some(&variables));

    let mut enabled = original.as_ref().clone();
    enabled.status.record_db_label = true;
    astersql_config::store_global_config(enabled);
    let nil_when_enabled = GetDBNames(None);
    let current_database_fallback = GetDBNames(Some(&variables));
    astersql_config::store_global_config(original.as_ref().clone());

    assert_eq!(nil_when_disabled, [""]);
    assert_eq!(session_when_disabled, [""]);
    assert_eq!(nil_when_enabled, [""]);
    assert_eq!(current_database_fallback, ["databasea"]);
}
