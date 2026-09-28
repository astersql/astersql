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

// bindinfo 绑定操作（binding operator）测试模块。
//
// “绑定”（SQL Binding / SQL Plan Binding）是数据库中把某条 SQL 与一份指定的
// 执行计划提示（hint，如 use_index）关联起来的机制：优化器在生成执行计划时，
// 若查询的归一化形态（参数被替换为 ? 的模板）与已有绑定匹配，就按绑定给出的
// 提示选择计划，从而在不改写业务 SQL 的前提下稳定或修正执行计划。
//
// 本文件包含两部分内容：
// 1. `GO_BINDING_OPERATOR_TEST_DRAFT`：一段以原始字符串形式保留的 Go 测试
//    迁移草稿，逐条对应 TiDB 的 binding operator 测试（创建/删除/启停全局与
//    会话级绑定、绑定缓存加载、SQL 归一化与摘要计算、并发缓存重载等），
//    仅作为后续 Rust 化的参照，不参与编译执行。
// 2. 真正可运行的 Rust 单元测试
//    `canonical_cached_binding_prefers_newer_and_honors_delete_tombstone`，
//    验证缓存绑定的挑选逻辑。

/// Go 测试文件 binding_operator_test.go 的机械迁移草稿，整体以原始字符串
/// 常量保存，不参与编译。其中覆盖的测试场景包括：
///
/// - 绑定缓存：`LoadFromStorageToCache` 从系统表 mysql.bind_info（绑定的
///   持久化存储）加载绑定到内存缓存，以及缓存与 `show global bindings` 的
///   一致性。
/// - 并发场景：多个工作线程在缓存被反复重载时持续匹配“热点”绑定，验证
///   匹配不会丢失（对应 Go 中基于 goroutine/WaitGroup/context 的并发协调）。
/// - 绑定解析：各类语句（SELECT、集合运算 UNION/INTERSECT/EXCEPT、
///   UPDATE/DELETE、INSERT/REPLACE INTO SELECT）的绑定创建与删除，
///   以及非法绑定的报错。
/// - 状态管理：`set binding enabled/disabled` 切换绑定状态，及
///   last_plan_binding_update_time 状态变量的更新时间。
/// - SQL 归一化：`NormalizeStmtForBinding` 把 SQL 归一化为参数化模板并计算
///   摘要（digest，SQL 模板的哈希指纹），包括 IN 列表折叠与括号
///   保留/去除的优先级规则。
const GO_BINDING_OPERATOR_TEST_DRAFT: &str = r########################################"
type Error = Box<dyn std::error::Error>;
pub struct TestingT;
pub struct BenchmarkDraft;
macro_rules! defer_draft { ($($tt:tt)*) => {}; }
macro_rules! spawn_go_draft { ($($tt:tt)*) => {}; }

// TestSqlCaseDraft 对应 Go 的 testSQLs 匿名结构体。
pub struct TestSqlCaseDraft {
    pub createSQL: &'static str,
    pub overlaySQL: &'static str,
    pub querySQL: &'static str,
    pub originSQL: &'static str,
    pub bindSQL: &'static str,
    pub dropSQL: &'static str,
    pub memoryUsage: f64,
}

// test_binding_cache 对应 Go 的 TestBindingCache，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_cache() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, index idx(a))");
        tk::MustExec("create global binding for select * from t using select * from t use index(idx);");
        tk::MustExec("create database tmp");
        tk::MustExec("use tmp");
        tk::MustExec("create table t(a int, b int, index idx(a))");
        tk::MustExec("create global binding for select * from t using select * from t use index(idx);");

        require::Nil(t, dom::BindingHandle().LoadFromStorageToCache(false, false));
        require::Nil(t, dom::BindingHandle().LoadFromStorageToCache(false, false));
        let mut res = tk::MustQuery("show global bindings");
        require::Equal(t, 2, len(res::Rows()));

        tk::MustExec("drop global binding for select * from t;");
        require::Nil(t, dom::BindingHandle().LoadFromStorageToCache(false, false));
        require::Equal(t, 1, len(dom::BindingHandle().GetAllBindings()));
}

// TestConcurrentBindingCacheReloadAndMatch mirrors the hot-binding repro:
// keep a small set of digests hot while repeatedly reloading many bindings from storage.
// test_concurrent_binding_cache_reload_and_match 对应 Go 的 TestConcurrentBindingCacheReloadAndMatch，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_concurrent_binding_cache_reload_and_match() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");

        const (;
            bindingCount = 24;
            hotCount     = 3;
            workerCount  = 8;
            testDuration = 20 * time.Second;
        );

        type hotBindingLookup struct {
            noDBDigest string;
            tableNames []*ast.TableName;
        }

        let mut parser4Test = parser::New();
        let mut hotLookups = make([]hotBindingLookup, 0, hotCount);
        let mut for i = 0; i < bindingCount; i++ {
            let mut tableName = fmt::Sprintf("binding_hot_%02d", i);
            tk::MustExec(fmt::Sprintf("create table %s (a int, b int, key idx_a(a), key idx_b(b), key idx_ab(a, b))", tableName));

            let mut originSQL = fmt::Sprintf("select * from %s where a = 1 and b = 1", tableName);
            let mut bindSQL = fmt::Sprintf("select /*+ use_index(%s, idx_a) */ * from %s where a = 1 and b = 1", tableName, tableName);
            tk::MustExec(fmt::Sprintf("create global binding for %s using %s", originSQL, bindSQL));

            if i < hotCount {
                let mut stmt, err = parser4Test::ParseOneStmt(fmt::Sprintf("select * from test.%s where a = 1 and b = 1", tableName), "", "");
                require::NoError(t, err);

                let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
                hotLookups = append(hotLookups, hotBindingLookup{
                    noDBDigest: noDBDigest,
                    tableNames: bindinfo::CollectTableNames(stmt),
                });
            }
        }
        require::Len(t, tk::MustQuery("show global bindings").Rows(), bindingCount);

        let mut bindHandle = dom::BindingHandle();
        require::NoError(t, bindHandle::LoadFromStorageToCache(true, false));

        let mut workers = make([]*testkit.TestKit, 0, workerCount);
        let mut for i = 0; i < workerCount; i++ {
            let mut worker = testkit::NewTestKit(t, store);
            worker::MustExec("use test");
            let mut lookup = hotLookups[i%len(hotLookups)];
            let mut binding, matched = bindHandle::MatchingBinding(worker::Session(), lookup.noDBDigest, lookup.tableNames);
            require::True(t, matched);
            require::NotNil(t, binding);
            workers = append(workers, worker);
        }

        // context 在 Go 中控制取消/超时；这里仅保留调用点和参数。
    let mut ctx, cancel = context::WithTimeout(context::Background(), testDuration);
        // Go defer：资源收尾在原测试结束时执行，先保留调用顺序。
    defer_draft!(cancel());

        let mut (;
            // 并发同步对象保持 Go 语义：用于协调 goroutine 启动、等待或错误传播。
        wg       sync.WaitGroup;
            errOnce  sync.Once;
            firstErr error;
        );
        // 并发同步对象保持 Go 语义：用于协调 goroutine 启动、等待或错误传播。
    let mut start = make(chan struct{});
        let mut setErr = func(err error) {
            if err == None {
                return;
            }
            errOnce::Do(func() {
                firstErr = err;
                cancel();
            });
        }

        wg::Go(func() {
            <-start;
            for ctx::Err() == None {
                setErr(bindHandle::LoadFromStorageToCache(false, false));
            }
        });

        let mut runMatchWorker = func(worker *testkit.TestKit, lookup hotBindingLookup) func() {
            return func() {
                <-start;
                let mut matchCount = 0;
                for ctx::Err() == None {
                    let mut binding, matched = bindHandle::MatchingBinding(worker::Session(), lookup.noDBDigest, lookup.tableNames);
                    if !matched || binding == None {
                        setErr(fmt::Errorf("hot binding lookup missed after %d successful matches", matchCount));
                        return;
                    }
                    matchCount++;
                }
            }
        }

        let mut for i, worker = range workers {
            let mut lookup = hotLookups[i%len(hotLookups)];
            wg::Go(runMatchWorker(worker, lookup));
        }

        close(start);
        wg::Wait();

        require::NoError(t, firstErr);
        require::Len(t, tk::MustQuery("show global bindings").Rows(), bindingCount);
}

// test_binding_last_update_time 对应 Go 的 TestBindingLastUpdateTime，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_last_update_time() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t0;");
        tk::MustExec("create table t0(a int, key(a));");
        tk::MustExec("create global binding for select * from t0 using select * from t0 use index(a);");
        tk::MustExec("admin reload bindings;");

        let mut bindHandle = bindinfo::NewBindingHandle(mockSessionPool {tk::Session()});
        let mut err = bindHandle::LoadFromStorageToCache(true, false);
        require::NoError(t, err);
        let mut stmt, err = parser::New().ParseOneStmt("select * from test . t0", "", "");
        require::NoError(t, err);

        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = bindHandle::MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        let mut updateTime = binding.UpdateTime::String();

        let mut rows1 = tk::MustQuery("show status like 'last_plan_binding_update_time';").Rows();
        let mut updateTime1 = rows1[0][1];
        require::Equal(t, updateTime, updateTime1);

        let mut rows2 = tk::MustQuery("show session status like 'last_plan_binding_update_time';").Rows();
        let mut updateTime2 = rows2[0][1];
        require::Equal(t, updateTime, updateTime2);
        tk::MustQuery(`show global status like 'last_plan_binding_update_time';`).Check(testkit::Rows());
}

// test_binding_last_update_time_with_invalid_bind 对应 Go 的 TestBindingLastUpdateTimeWithInvalidBind，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_last_update_time_with_invalid_bind() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);

        let mut rows0 = tk::MustQuery("show status like 'last_plan_binding_update_time';").Rows();
        let mut updateTime0 = rows0[0][1];
        require::Equal(t, updateTime0, "0000-00-00 00:00:00");

        tk::MustExec("insert into mysql.bind_info (original_sql, bind_sql, default_db, status, create_time, update_time, charset, collation, source, sql_digest, plan_digest) values('select * from `test` . `t`', 'invalid_binding', 'test', 'enabled', '2000-01-01 09:00:00', '2000-01-01 09:00:00', '', '','" +;
            bindinfo.SourceManual + "', '', '')");
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("admin reload bindings;");

        let mut rows2 = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows2, 0);
}

// test_bind_parse 对应 Go 的 TestBindParse，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_bind_parse() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("create table t(i int)");
        tk::MustExec("create index index_t on t(i)");

        let mut originSQL = "select * from `test` . `t`";
        let mut bindSQL = "select * from `test` . `t` use index(index_t)";
        let mut defaultDb = "test";
        let mut status = bindinfo.StatusEnabled;
        let mut charset = "utf8mb4";
        let mut collation = "utf8mb4_bin";
        let mut source = bindinfo.SourceManual;
        let mut _, digest = parser::NormalizeDigestForBinding(originSQL);
        let mut mockDigest = digest::String();
        let mut sql = fmt::Sprintf(`INSERT INTO mysql.bind_info(original_sql,bind_sql,default_db,status,create_time,update_time,charset,collation,source, sql_digest, plan_digest) VALUES ('%s', '%s', '%s', '%s', NOW(), NOW(),'%s', '%s', '%s', '%s', '%s')`,
            originSQL, bindSQL, defaultDb, status, charset, collation, source, mockDigest, mockDigest);
        tk::MustExec(sql);
        let mut bindHandle = bindinfo::NewBindingHandle(mockSessionPool {tk::Session()});
        let mut err = bindHandle::LoadFromStorageToCache(true, false);
        require::NoError(t, err);
        require::Equal(t, 1, len(bindHandle::GetAllBindings()));

        let mut stmt, err = parser::New().ParseOneStmt("select * from test . t", "", "");
        require::NoError(t, err);
        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = bindHandle::MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t`", binding.OriginalSQL);
        require::Equal(t, "select * from `test` . `t` use index(index_t)", binding.BindSQL);
        require::Equal(t, "test", binding.Db);
        require::Equal(t, bindinfo.StatusEnabled, binding.Status);
        require::Equal(t, "utf8mb4", binding.Charset);
        require::Equal(t, "utf8mb4_bin", binding.Collation);
        require::NotNil(t, binding.CreateTime);
        require::NotNil(t, binding.UpdateTime);

        let mut dur, err = binding.UpdateTime::GoTime(time.Local);
        require::NoError(t, err);
        require::GreaterOrEqual(t, i64::from(time::Since(dur)), i64::from(0));

        // Test fields with quotes or slashes.
        sql = `CREATE GLOBAL BINDING FOR  select * from t where i BETWEEN "a" and "b" USING select * from t use index(index_t) where i BETWEEN "a\nb\rc\td\0e" and 'x'`;
        tk::MustExec(sql);
        tk::MustExec(`DROP global binding for select * from t use index(idx) where i BETWEEN "a\nb\rc\td\0e" and "x"`);

        // Test SetOprStmt.
        tk::MustExec(`create binding for select * from t union all select * from t using select * from t use index(index_t) union all select * from t use index()`);
        tk::MustExec(`drop binding for select * from t union all select * from t using select * from t use index(index_t) union all select * from t use index()`);
        tk::MustExec(`create binding for select * from t INTERSECT select * from t using select * from t use index(index_t) INTERSECT select * from t use index()`);
        tk::MustExec(`drop binding for select * from t INTERSECT select * from t using select * from t use index(index_t) INTERSECT select * from t use index()`);
        tk::MustExec(`create binding for select * from t EXCEPT select * from t using select * from t use index(index_t) EXCEPT select * from t use index()`);
        tk::MustExec(`drop binding for select * from t EXCEPT select * from t using select * from t use index(index_t) EXCEPT select * from t use index()`);
        tk::MustExec(`create binding for (select * from t) union all (select * from t) using (select * from t use index(index_t)) union all (select * from t use index())`);
        tk::MustExec(`drop binding for (select * from t) union all (select * from t) using (select * from t use index(index_t)) union all (select * from t use index())`);

        // Test Update / Delete.
        tk::MustExec("create table t1(a int, b int, c int, key(b), key(c))");
        tk::MustExec("create table t2(a int, b int, c int, key(b), key(c))");
        tk::MustExec("create binding for delete from t1 where b = 1 and c > 1 using delete /*+ use_index(t1, c) */ from t1 where b = 1 and c > 1");
        tk::MustExec("drop binding for delete from t1 where b = 1 and c > 1 using delete /*+ use_index(t1, c) */ from t1 where b = 1 and c > 1");
        tk::MustExec("create binding for delete t1, t2 from t1 inner join t2 on t1.b = t2.b where t1.c = 1 using delete /*+ hash_join(t1, t2), use_index(t1, c) */ t1, t2 from t1 inner join t2 on t1.b = t2.b where t1.c = 1");
        tk::MustExec("drop binding for delete t1, t2 from t1 inner join t2 on t1.b = t2.b where t1.c = 1 using delete /*+ hash_join(t1, t2), use_index(t1, c) */ t1, t2 from t1 inner join t2 on t1.b = t2.b where t1.c = 1");
        tk::MustExec("create binding for update t1 set a = 1 where b = 1 and c > 1 using update /*+ use_index(t1, c) */ t1 set a = 1 where b = 1 and c > 1");
        tk::MustExec("drop binding for update t1 set a = 1 where b = 1 and c > 1 using update /*+ use_index(t1, c) */ t1 set a = 1 where b = 1 and c > 1");
        tk::MustExec("create binding for update t1, t2 set t1.a = 1 where t1.b = t2.b using update /*+ inl_join(t1) */ t1, t2 set t1.a = 1 where t1.b = t2.b");
        tk::MustExec("drop binding for update t1, t2 set t1.a = 1 where t1.b = t2.b using update /*+ inl_join(t1) */ t1, t2 set t1.a = 1 where t1.b = t2.b");
        // Test Insert / Replace.
        tk::MustExec("create binding for insert into t1 select * from t2 where t2.b = 1 and t2.c > 1 using insert into t1 select /*+ use_index(t2,c) */ * from t2 where t2.b = 1 and t2.c > 1");
        tk::MustExec("drop binding for insert into t1 select * from t2 where t2.b = 1 and t2.c > 1 using insert into t1 select /*+ use_index(t2,c) */ * from t2 where t2.b = 1 and t2.c > 1");
        tk::MustExec("create binding for replace into t1 select * from t2 where t2.b = 1 and t2.c > 1 using replace into t1 select /*+ use_index(t2,c) */ * from t2 where t2.b = 1 and t2.c > 1");
        tk::MustExec("drop binding for replace into t1 select * from t2 where t2.b = 1 and t2.c > 1 using replace into t1 select /*+ use_index(t2,c) */ * from t2 where t2.b = 1 and t2.c > 1");
        err = tk::ExecToErr("create binding for insert into t1 values(1,1,1) using insert into t1 values(1,1,1)");
        require::Equal(t, "create binding only supports INSERT / REPLACE INTO SELECT", err::Error());
        err = tk::ExecToErr("create binding for replace into t1 values(1,1,1) using replace into t1 values(1,1,1)");
        require::Equal(t, "create binding only supports INSERT / REPLACE INTO SELECT", err::Error());

        // Test errors.
        tk::MustExec(`drop table if exists t1`);
        tk::MustExec("create table t1(i int, s varchar(20))");
        _, err = tk::Exec("create global binding for select * from t using select * from t1 use index for join(index_t)");
        require::NotNil(t, err, "err %v", err);
}

// test_set_binding_status 对应 Go 的 TestSetBindingStatus，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_set_binding_status() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, index idx_a(a))");
        tk::MustQuery("show global bindings").Check(testkit::Rows());
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ USE_INDEX(t, idx_a) */ * from t where a > 10");
        let mut rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);
        tk::MustExec("select * from t where a > 10");
        tk::MustQuery("select @@last_plan_from_binding").Check(testkit::Rows("1"));

        tk::MustExec("set binding disabled for select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusDisabled, rows[0][3]);
        tk::MustExec("select * from t where a > 10");
        tk::MustQuery("select @@last_plan_from_binding").Check(testkit::Rows("0"));

        tk::MustExec("set binding enabled for select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);

        tk::MustExec("set binding disabled for select * from t where a > 10");
        tk::MustExec("create global binding for select * from t where a > 10 using select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);
        tk::MustExec("select * from t where a > 10");
        tk::MustQuery("select @@last_plan_from_binding").Check(testkit::Rows("1"));

        tk::MustExec("set binding disabled for select * from t where a > 10 using select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusDisabled, rows[0][3]);
        tk::MustExec("select * from t where a > 10");
        tk::MustQuery("select @@last_plan_from_binding").Check(testkit::Rows("0"));

        tk::MustExec("set binding enabled for select * from t where a > 10 using select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);

        tk::MustExec("set binding disabled for select * from t where a > 10 using select * from t where a > 10");
        tk::MustExec("drop global binding for select * from t where a > 10 using select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 0);
}

// test_set_binding_status_without_binding_in_cache 对应 Go 的 TestSetBindingStatusWithoutBindingInCache，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_set_binding_status_without_binding_in_cache() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, index idx_a(a))");
        utilCleanBindingEnv(tk);
        tk::MustQuery("show global bindings").Check(testkit::Rows());

        // Simulate creating bindings on other machines
        let mut _, sqlDigest = parser::NormalizeDigestForBinding("select * from `test` . `t` where `a` > ?");
        tk::MustExec("insert into mysql.bind_info (original_sql, bind_sql, default_db, status, create_time, update_time, charset, collation, source, sql_digest, plan_digest) values('select * from `test` . `t` where `a` > ?', 'SELECT /*+ USE_INDEX(`t` `idx_a`)*/ * FROM `test`.`t` WHERE `a` > 10', 'test', 'enabled', '2000-01-02 09:00:00', '2000-01-02 09:00:00', '', '','" +;
            bindinfo.SourceManual + "', '" + sqlDigest::String() + "', '')");
        tk::MustExec("set binding disabled for select * from t where a > 10");
        tk::MustExec("admin reload bindings");
        let mut rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusDisabled, rows[0][3]);

        // clear the mysql.bind_info
        utilCleanBindingEnv(tk);

        // Simulate creating bindings on other machines
        tk::MustExec("insert into mysql.bind_info (original_sql, bind_sql, default_db, status, create_time, update_time, charset, collation, source, sql_digest, plan_digest) values('select * from `test` . `t` where `a` > ?', 'SELECT * FROM `test`.`t` WHERE `a` > 10', 'test', 'disabled', '2000-01-02 09:00:00', '2000-01-02 09:00:00', '', '','" +;
            bindinfo.SourceManual + "', '" + sqlDigest::String() + "', '')");
        tk::MustExec("set binding enabled for select * from t where a > 10");
        tk::MustExec("admin reload bindings");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);

        utilCleanBindingEnv(tk);
}

// testSQLs 对应 Go 的同名表驱动测试数据，字段和用例顺序保持不变。
pub static TEST_SQLS: &[TestSqlCaseDraft] = &[
    // 字段定义已提升为 TestSqlCaseDraft；下面只保留 Go 用例数据。
    {
    createSQL:   "binding for select * from t where i>100 using select * from t use index(index_t) where i>100",
    overlaySQL:  "binding for select * from t where i>99 using select * from t use index(index_t) where i>99",
    querySQL:    "select * from t where i          >      30.0",
    originSQL:   "select * from `test` . `t` where `i` > ?",
    bindSQL:     "SELECT * FROM `test`.`t` USE INDEX (`index_t`) WHERE `i` > 99",
    dropSQL:     "binding for select * from t where i>100",
    memoryUsage: 167.0,
    },
    {
    createSQL:   "binding for select * from t union all select * from t using select * from t use index(index_t) union all select * from t use index()",
    overlaySQL:  "",
    querySQL:    "select * from t union all         select * from t",
    originSQL:   "select * from `test` . `t` union all select * from `test` . `t`",
    bindSQL:     "SELECT * FROM `test`.`t` USE INDEX (`index_t`) UNION ALL SELECT * FROM `test`.`t` USE INDEX ()",
    dropSQL:     "binding for select * from t union all select * from t",
    memoryUsage: 237.0,
    },
    {
    createSQL:   "binding for (select * from t) union all (select * from t) using (select * from t use index(index_t)) union all (select * from t use index())",
    overlaySQL:  "",
    querySQL:    "(select * from t) union all         (select * from t)",
    originSQL:   "( select * from `test` . `t` ) union all ( select * from `test` . `t` )",
    bindSQL:     "(SELECT * FROM `test`.`t` USE INDEX (`index_t`)) UNION ALL (SELECT * FROM `test`.`t` USE INDEX ())",
    dropSQL:     "binding for (select * from t) union all (select * from t)",
    memoryUsage: 249.0,
    },
    {
    createSQL:   "binding for select * from t intersect select * from t using select * from t use index(index_t) intersect select * from t use index()",
    overlaySQL:  "",
    querySQL:    "select * from t intersect         select * from t",
    originSQL:   "select * from `test` . `t` intersect select * from `test` . `t`",
    bindSQL:     "SELECT * FROM `test`.`t` USE INDEX (`index_t`) INTERSECT SELECT * FROM `test`.`t` USE INDEX ()",
    dropSQL:     "binding for select * from t intersect select * from t",
    memoryUsage: 237.0,
    },
    {
    createSQL:   "binding for select * from t except select * from t using select * from t use index(index_t) except select * from t use index()",
    overlaySQL:  "",
    querySQL:    "select * from t except         select * from t",
    originSQL:   "select * from `test` . `t` except select * from `test` . `t`",
    bindSQL:     "SELECT * FROM `test`.`t` USE INDEX (`index_t`) EXCEPT SELECT * FROM `test`.`t` USE INDEX ()",
    dropSQL:     "binding for select * from t except select * from t",
    memoryUsage: 231.0,
    },
    {
    createSQL:   "binding for select * from t using select /*+ use_index(t,index_t)*/ * from t",
    overlaySQL:  "",
    querySQL:    "select * from t ",
    originSQL:   "select * from `test` . `t`",
    bindSQL:     "SELECT /*+ use_index(`t` `index_t`)*/ * FROM `test`.`t`",
    dropSQL:     "binding for select * from t",
    memoryUsage: 166.0,
    },
    {
    createSQL:   "binding for delete from t where i = 1 using delete /*+ use_index(t,index_t) */ from t where i = 1",
    overlaySQL:  "",
    querySQL:    "delete    from t where   i = 2",
    originSQL:   "delete from `test` . `t` where `i` = ?",
    bindSQL:     "DELETE /*+ use_index(`t` `index_t`)*/ FROM `test`.`t` WHERE `i` = 1",
    dropSQL:     "binding for delete from t where i = 1",
    memoryUsage: 190.0,
    },
    {
    createSQL:   "binding for delete t, t1 from t inner join t1 on t.s = t1.s where t.i = 1 using delete /*+ use_index(t,index_t), hash_join(t,t1) */ t, t1 from t inner join t1 on t.s = t1.s where t.i = 1",
    overlaySQL:  "",
    querySQL:    "delete t,   t1 from t inner join t1 on t.s = t1.s  where   t.i = 2",
    originSQL:   "delete `test` . `t` , `test` . `t1` from `test` . `t` join `test` . `t1` on `t` . `s` = `t1` . `s` where `t` . `i` = ?",
    bindSQL:     "DELETE /*+ use_index(`t` `index_t`) hash_join(`t`, `t1`)*/ `test`.`t`,`test`.`t1` FROM `test`.`t` JOIN `test`.`t1` ON `t`.`s` = `t1`.`s` WHERE `t`.`i` = 1",
    dropSQL:     "binding for delete t, t1 from t inner join t1 on t.s = t1.s where t.i = 1",
    memoryUsage: 402.0,
    },
    {
    createSQL:   "binding for update t set s = 'a' where i = 1 using update /*+ use_index(t,index_t) */ t set s = 'a' where i = 1",
    overlaySQL:  "",
    querySQL:    "update   t  set s='b' where i=2",
    originSQL:   "update `test` . `t` set `s` = ? where `i` = ?",
    bindSQL:     "UPDATE /*+ use_index(`t` `index_t`)*/ `test`.`t` SET `s`='a' WHERE `i` = 1",
    dropSQL:     "binding for update t set s = 'a' where i = 1",
    memoryUsage: 204.0,
    },
    {
    createSQL:   "binding for update t, t1 set t.s = 'a' where t.i = t1.i using update /*+ inl_join(t1) */ t, t1 set t.s = 'a' where t.i = t1.i",
    overlaySQL:  "",
    querySQL:    "update   t  , t1 set t.s='b' where t.i=t1.i",
    originSQL:   "update ( `test` . `t` ) join `test` . `t1` set `t` . `s` = ? where `t` . `i` = `t1` . `i`",
    bindSQL:     "UPDATE /*+ inl_join(`t1`)*/ (`test`.`t`) JOIN `test`.`t1` SET `t`.`s`='a' WHERE `t`.`i` = `t1`.`i`",
    dropSQL:     "binding for update t, t1 set t.s = 'a' where t.i = t1.i",
    memoryUsage: 262.0,
    },
    {
    createSQL:   "binding for insert into t1 select * from t where t.i = 1 using insert into t1 select /*+ use_index(t,index_t) */ * from t where t.i = 1",
    overlaySQL:  "",
    querySQL:    "insert  into   t1 select * from t where t.i  = 2",
    originSQL:   "insert into `test` . `t1` select * from `test` . `t` where `t` . `i` = ?",
    bindSQL:     "INSERT INTO `test`.`t1` SELECT /*+ use_index(`t` `index_t`)*/ * FROM `test`.`t` WHERE `t`.`i` = 1",
    dropSQL:     "binding for insert into t1 select * from t where t.i = 1",
    memoryUsage: 254.0,
    },
    {
    createSQL:   "binding for replace into t1 select * from t where t.i = 1 using replace into t1 select /*+ use_index(t,index_t) */ * from t where t.i = 1",
    overlaySQL:  "",
    querySQL:    "replace  into   t1 select * from t where t.i  = 2",
    originSQL:   "replace into `test` . `t1` select * from `test` . `t` where `t` . `i` = ?",
    bindSQL:     "REPLACE INTO `test`.`t1` SELECT /*+ use_index(`t` `index_t`)*/ * FROM `test`.`t` WHERE `t`.`i` = 1",
    dropSQL:     "binding for replace into t1 select * from t where t.i = 1",
    memoryUsage: 256.0,
    },
];

// test_load_binding_time_lag 对应 Go 的 TestLoadBindingTimeLag，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_load_binding_time_lag() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec(`create table t (a int)`);
        tk::MustExec(`create global binding using select * from t`);
        let mut numBindings = len(tk::MustQuery(`show global bindings`).Rows());
        require::Equal(t, 1, numBindings);

        tk::Session().SetValue(bindinfo.TestTimeLagInLoadingBinding, 5*time.Second);
        tk::MustExec(`create global binding using select * from t where a < 1`);
        numBindings = len(tk::MustQuery(`show global bindings`).Rows());
        require::Equal(t, 2, numBindings);

        tk::Session().SetValue(bindinfo.TestTimeLagInLoadingBinding, 15*time.Second);
        tk::MustExec(`create global binding using select * from t where a > 1`);
        numBindings = len(tk::MustQuery(`show global bindings`).Rows());
        require::Equal(t, 2, numBindings) // can't see the latest one since the time lag tolerance is only 10s;
}

// test_global_binding 对应 Go 的 TestGlobalBinding，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_global_binding() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);

        let mut for _, testSQL = range testSQLs {
            utilCleanBindingEnv(tk);
            tk::MustExec("use test");
            tk::MustExec("drop table if exists t");
            tk::MustExec("drop table if exists t1");
            tk::MustExec("create table t(i int, s varchar(20))");
            tk::MustExec("create table t1(i int, s varchar(20))");
            tk::MustExec("create index index_t on t(i,s)");

            let mut _, err = tk::Exec("create global " + testSQL.createSQL);
            require::NoErrorf(t, err, "testSQL: %+v", testSQL);

            if testSQL.overlaySQL != "" {
                _, err = tk::Exec("create global " + testSQL.overlaySQL);
                require::NoErrorf(t, err, "testSQL: %+v", testSQL);
            }

            let mut stmt, _, _ = utilNormalizeWithDefaultDB(t, testSQL.querySQL);

            let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
            require::Truef(t, matched, "testSQL %+v", testSQL);
            require::Equalf(t, testSQL.originSQL, binding.OriginalSQL, "testSQL %+v", testSQL);
            require::Equalf(t, testSQL.bindSQL, binding.BindSQL, "testSQL %+v", testSQL);
            require::Equalf(t, "test", binding.Db, "testSQL %+v", testSQL);
            require::Equalf(t, bindinfo.StatusEnabled, binding.Status, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.Charset, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.Collation, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.CreateTime, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.UpdateTime, "testSQL %+v", testSQL);

            let mut rs, err = tk::Exec("show global bindings");
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            let mut chk = rs::NewChunk(None);
            // context 在 Go 中控制取消/超时；这里仅保留调用点和参数。
        err = rs::Next(context::TODO(), chk);
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            require::Equalf(t, 1, chk::NumRows(), "testSQL %+v", testSQL);
            let mut row = chk::GetRow(0);
            require::Equalf(t, testSQL.originSQL, row::GetString(0), "testSQL %+v", testSQL);
            require::Equalf(t, testSQL.bindSQL, row::GetString(1), "testSQL %+v", testSQL);
            require::Equalf(t, "test", row::GetString(2), "testSQL %+v", testSQL);
            require::Equalf(t, bindinfo.StatusEnabled, row::GetString(3), "testSQL %+v", testSQL);
            require::NotNilf(t, row::GetTime(4), "testSQL %+v", testSQL);
            require::NotNilf(t, row::GetTime(5), "testSQL %+v", testSQL);
            require::NotNilf(t, row::GetString(6), "testSQL %+v", testSQL);
            require::NotNilf(t, row::GetString(7), "testSQL %+v", testSQL);

            let mut bindHandle = bindinfo::NewBindingHandle(mockSessionPool {tk::Session()});
            err = bindHandle::LoadFromStorageToCache(true, false);
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            require::Equalf(t, 1, len(bindHandle::GetAllBindings()), "testSQL %+v", testSQL);

            _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
            require::Truef(t, matched, "testSQL %+v", testSQL);
            require::Equalf(t, testSQL.originSQL, binding.OriginalSQL, "testSQL %+v", testSQL);
            require::Equalf(t, testSQL.bindSQL, binding.BindSQL, "testSQL %+v", testSQL);
            require::Equalf(t, "test", binding.Db, "testSQL %+v", testSQL);
            require::Equalf(t, bindinfo.StatusEnabled, binding.Status, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.Charset, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.Collation, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.CreateTime, "testSQL %+v", testSQL);
            require::NotNilf(t, binding.UpdateTime, "testSQL %+v", testSQL);

            _, err = tk::Exec("drop global " + testSQL.dropSQL);
            require::Equalf(t, ui64::from(1), tk::Session().AffectedRows(), "testSQL %+v", testSQL);
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            _, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
            require::Falsef(t, matched, "testSQL %+v", testSQL) // dropped;
            bindHandle = bindinfo::NewBindingHandle(mockSessionPool {tk::Session()});
            err = bindHandle::LoadFromStorageToCache(true, false);
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            require::Equalf(t, 0, len(bindHandle::GetAllBindings()), "testSQL %+v", testSQL);

            _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            _, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
            require::Falsef(t, matched, "testSQL %+v", testSQL) // dropped;

            rs, err = tk::Exec("show global bindings");
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            chk = rs::NewChunk(None);
            // context 在 Go 中控制取消/超时；这里仅保留调用点和参数。
        err = rs::Next(context::TODO(), chk);
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
            require::Equalf(t, 0, chk::NumRows(), "testSQL %+v", testSQL);

            _, err = tk::Exec("delete from mysql.bind_info where source != 'builtin'");
            require::NoErrorf(t, err, "testSQL %+v", testSQL);
        }
}

// test_outdated_info_schema 对应 Go 的 TestOutdatedInfoSchema，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_outdated_info_schema() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, index idx(a))");
        tk::MustExec("create global binding for select * from t using select * from t use index(idx)");
        require::Nil(t, dom::BindingHandle().LoadFromStorageToCache(false, false));
        utilCleanBindingEnv(tk);
        tk::MustExec("create global binding for select * from t using select * from t use index(idx)");
}

// test_reload_bindings 对应 Go 的 TestReloadBindings，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_reload_bindings() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, index idx(a))");
        tk::MustExec("create global binding for select * from t using select * from t use index(idx)");
        let mut rows = tk::MustQuery("show global bindings").Rows();
        require::Equal(t, 1, len(rows));
        rows = tk::MustQuery("select * from mysql.bind_info where source != 'builtin'").Rows();
        require::Equal(t, 1, len(rows));
        tk::MustExec(`drop global binding for select * from t`);
        rows = tk::MustQuery("show global bindings").Rows();
        require::Equal(t, 0, len(rows));
}

// test_set_var_fix_control_with_binding 对应 Go 的 TestSetVarFixControlWithBinding，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_set_var_fix_control_with_binding() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");

        tk::MustExec(`create table t(id int, a varchar(100), b int, c int, index idx_ab(a, b))`);
        tk::MustQuery(`explain format='brief' select * from t where c = 10 and (a = 'xx' or (a = 'kk' and b = 1))`).Check(;
            testkit::Rows(;
                `IndexLookUp 1.00 root  `,
                `├─IndexRangeScan(Build) 10.10 cop[tikv] table:t, index:idx_ab(a, b) range:["kk" 1,"kk" 1], ["xx","xx"], keep order:false, stats:pseudo`,
                `└─Selection(Probe) 1.00 cop[tikv]  eq(test.t.c, 10)`,
                `  └─TableRowIDScan 10.10 cop[tikv] table:t keep order:false, stats:pseudo`));

        tk::MustExec(`create global binding using select /*+ set_var(tidb_opt_fix_control='44389:ON') */ * from t where c = 10 and (a = 'xx' or (a = 'kk' and b = 1))`);
        tk::MustQuery(`show warnings`).Check(testkit::Rows()) // no warning;

        // the fix control can take effect
        tk::MustQuery(`explain format='brief' select * from t where c = 10 and (a = 'xx' or (a = 'kk' and b = 1))`).Check(;
            testkit::Rows(`IndexLookUp 1.00 root  `,
                `├─IndexRangeScan(Build) 10.10 cop[tikv] table:t, index:idx_ab(a, b) range:["kk" 1,"kk" 1], ["xx","xx"], keep order:false, stats:pseudo`,
                `└─Selection(Probe) 1.00 cop[tikv]  eq(test.t.c, 10)`,
                `  └─TableRowIDScan 10.10 cop[tikv] table:t keep order:false, stats:pseudo`));
        tk::MustQuery(`select @@last_plan_from_binding`).Check(testkit::Rows("1"));
}

// test_remove_duplicated_pseudo_binding 对应 Go 的 TestRemoveDuplicatedPseudoBinding，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_remove_duplicated_pseudo_binding() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");

        let mut checkPseudoBinding = func(num int) {
            tk::MustQuery(fmt::Sprintf("select count(1) from mysql.bind_info where original_sql='%s'",
                bindinfo.BuiltinPseudoSQL4BindLock)).Check(testkit::Rows(fmt::Sprintf("%d", num)));
        }
        let mut insertPseudoBinding = func() {
            tk::MustExec(fmt::Sprintf(`INSERT INTO mysql.bind_info(original_sql, bind_sql, default_db, status, create_time, update_time, charset, collation, source);
                VALUES ('%v', '%v', "mysql", '%v', "2000-01-01 00:00:00", "2000-01-01 00:00:00", "", "", '%v')`,
                bindinfo.BuiltinPseudoSQL4BindLock, bindinfo.BuiltinPseudoSQL4BindLock, bindinfo.StatusBuiltin, bindinfo.StatusBuiltin));
        }
        let mut removeDuplicated = func() {
            tk::MustExec(bindinfo.StmtRemoveDuplicatedPseudoBinding);
        }

        checkPseudoBinding(1);
        insertPseudoBinding();
        checkPseudoBinding(2);
        removeDuplicated();
        checkPseudoBinding(1);

        insertPseudoBinding();
        insertPseudoBinding();
        insertPseudoBinding();
        checkPseudoBinding(4);
        removeDuplicated();
        checkPseudoBinding(1);
        removeDuplicated();
        checkPseudoBinding(1);
}

// mockSessionPool 对应 Go 的同名结构体，字段保留测试辅助数据的承载关系。
pub struct mockSessionPool {
    pub se: sessionapi::Session,
}

// Get 对应 Go 的 mockSessionPool.Get 方法，保留测试断言语义。
impl mockSessionPool {
    pub fn Get(&self) {
        return p.se, None;
    }
}

// Put 对应 Go 的 mockSessionPool.Put 方法，保留测试断言语义。
impl mockSessionPool {
    pub fn Put(&self, pools.Resource: ) {
    }
}

// Destroy 对应 Go 的 mockSessionPool.Destroy 方法，保留测试断言语义。
impl mockSessionPool {
    pub fn Destroy(&self, pools.Resource: ) {
    }
}

// Close 对应 Go 的 mockSessionPool.Close 方法，保留测试断言语义。
impl mockSessionPool {
    pub fn Close(&self) {
    }
}

// test_show_binding_digest_field 对应 Go 的 TestShowBindingDigestField，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_show_binding_digest_field() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("drop table if exists t1, t2");
        tk::MustExec("create table t1(id int, key(id))");
        tk::MustExec("create table t2(id int, key(id))");
        tk::MustExec("create binding for select * from t1, t2 where t1.id = t2.id using select /*+ merge_join(t1, t2)*/ * from t1, t2 where t1.id = t2.id");
        let mut result = tk::MustQuery("show bindings;");
        let mut rows = result::Rows()[0];
        require::Equal(t, len(rows), 11);
        require::Equal(t, rows[9], "ac1ceb4eb5c01f7c03e29b7d0d6ab567e563f4c93164184cde218f20d07fd77c");
        tk::MustExec("drop binding for select * from t1, t2 where t1.id = t2.id");
        result = tk::MustQuery("show bindings;");
        require::Equal(t, len(result::Rows()), 0);

        tk::MustExec("create global binding for select * from t1, t2 where t1.id = t2.id using select /*+ merge_join(t1, t2)*/ * from t1, t2 where t1.id = t2.id");
        result = tk::MustQuery("show global bindings;");
        rows = result::Rows()[0];
        require::Equal(t, len(rows), 11);
        require::Equal(t, rows[9], "ac1ceb4eb5c01f7c03e29b7d0d6ab567e563f4c93164184cde218f20d07fd77c");
        tk::MustExec("drop global binding for select * from t1, t2 where t1.id = t2.id");
        result = tk::MustQuery("show global bindings;");
        require::Equal(t, len(result::Rows()), 0);
}

// test_issue63032 对应 Go 的 TestIssue63032，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_issue63032() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);

        tk::MustExec("use test");
        tk::MustExec("create table t (a int, b int, index idx_a(a))");

        // Enable pessimistic-auto-commit globally.
        let mut origVal = config::GetGlobalConfig().PessimisticTxn.PessimisticAutoCommit::Load();
        config::GetGlobalConfig().PessimisticTxn.PessimisticAutoCommit::Store(true);
        // Go defer：资源收尾在原测试结束时执行，先保留调用顺序。
    defer_draft!(config::GetGlobalConfig().PessimisticTxn.PessimisticAutoCommit::Store(origVal));

        // Ensure the session is in auto-commit mode (default) and pessimistic txn mode (default).
        tk::MustQuery("select @@autocommit").Check(testkit::Rows("1"));
        tk::MustQuery("select @@tidb_txn_mode").Check(testkit::Rows("pessimistic"));

        // CREATE BINDING should not panic with "context provider not set".
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(t, idx_a) */ * from t where a > 10");
        let mut rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 1);
        require::Equal(t, bindinfo.StatusEnabled, rows[0][3]);

        // Also test session binding.
        tk::MustExec("create binding for select * from t where a > 10 using select /*+ use_index(t, idx_a) */ * from t where a > 10");

        // DROP BINDING should also work.
        tk::MustExec("drop global binding for select * from t where a > 10");
        rows = tk::MustQuery("show global bindings").Rows();
        require::Len(t, rows, 0);
}

// test_issue64558 对应 Go 的 TestIssue64558，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_issue64558() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec(`use test`);
        tk::MustExec(`create table t (a int)`);
        tk::MustExec(`create global binding using select * from t`);
        let mut sqlDigest = tk::MustQuery(`show global bindings`).Rows()[0][9].(string);
        tk::MustExec(fmt::Sprintf(`set binding disabled for sql digest '%s'`, sqlDigest));
        tk::MustQuery(`show warnings`).Check(testkit::Rows()) // no warning;
}

// test_optimize_only_once 对应 Go 的 TestOptimizeOnlyOnce，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_optimize_only_once() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store = testkit::CreateMockStore(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, index idxa(a))");
        tk::MustExec("create global binding for select * from t using select * from t use index(idxa)");
        // failpoint 是外部测试注入点，不实际启用，只保留检查语义。
    require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/planner/checkOptimizeCountOne", "return(\"select * from t\")"));
        // Go defer：资源收尾在原测试结束时执行，先保留调用顺序。
    defer_draft!(func() {);
            // failpoint 是外部测试注入点，不实际启用，只保留检查语义。
        require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/planner/checkOptimizeCountOne"));
        }();
        tk::MustQuery("select * from t").Check(testkit::Rows());
}

// test_normalize_stmt_for_binding 对应 Go 的 TestNormalizeStmtForBinding，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_normalize_stmt_for_binding() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut tests = []struct {
            sql        string;
            normalized string;
            digest     string;
        }{
            {"select 1 from b where (x,y) in ((1, 3), ('3', 1))", "select ? from `b` where row ( `x` , `y` ) in ( ... )", "ab6c607d118c24030807f8d1c7c846ec23e3b752fd88ed763bb8e26fbfa56a83"},
            {"select 1 from b where (x,y) in ((1, 3), ('3', 1), (2, 3))", "select ? from `b` where row ( `x` , `y` ) in ( ... )", "ab6c607d118c24030807f8d1c7c846ec23e3b752fd88ed763bb8e26fbfa56a83"},
            {"select 1 from b where (x,y) in ((1, 3), ('3', 1), (2, 3),('x', 'y'))", "select ? from `b` where row ( `x` , `y` ) in ( ... )", "ab6c607d118c24030807f8d1c7c846ec23e3b752fd88ed763bb8e26fbfa56a83"},
            {"select 1 from b where (x,y) in ((1, 3), ('3', 1), (2, 3),('x', 'y'),('x', 'y'))", "select ? from `b` where row ( `x` , `y` ) in ( ... )", "ab6c607d118c24030807f8d1c7c846ec23e3b752fd88ed763bb8e26fbfa56a83"},
            {"select 1 from b where (x) in ((1), ('3'), (2),('x'),('x'))", "select ? from `b` where `x` in ( ... )", "695a1f42dbae3cae4a5d475d7a9d67955aed6d9790c1ffa20106d80f79b33dc0"},
            {"select 1 from b where (x) in ((1), ('3'), (2),('x'))", "select ? from `b` where `x` in ( ... )", "695a1f42dbae3cae4a5d475d7a9d67955aed6d9790c1ffa20106d80f79b33dc0"},
        }
        let mut for _, test = range tests {
            let mut stmt, _, _ = utilNormalizeWithDefaultDB(t, test.sql);
            let mut n, digest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            require::Equalf(t, test.normalized, n, "sql: %s", test.sql);
            require::Equalf(t, test.digest, digest, "sql: %s", test.sql);
        }

        let mut parenthesesTests = []struct {
            sql        string;
            normalized string;
        }{
            // The inner addition is redundant inside a function argument and can be
            // restored without changing the expression boundary.
            {
                "select 1 from b where abs((x + 1)) = 2",
                "select ? from `b` where `abs` ( `x` + ? ) = ?",
            },
            // Addition has lower precedence than multiplication, so the left child
            // parentheses are semantic.
            {
                "select 1 from b where (x + 1) * y = 2",
                "select ? from `b` where ( `x` + ? ) * `y` = ?",
            },
            // The same precedence rule applies to the right child of multiplication.
            {
                "select 1 from b where x * (y + z) = 2",
                "select ? from `b` where `x` * ( `y` + `z` ) = ?",
            },
            // OR has lower precedence than AND and must stay grouped as the right child.
            {
                "select 1 from b where x = 1 and (y = 1 or y = 2)",
                "select ? from `b` where `x` = ? and ( `y` = ? or `y` = ? )",
            },
            // IN, LIKE, REGEXP, IS NULL, IS TRUE, BETWEEN, subquery comparison,
            // MEMBER OF, BINARY, and COLLATE are not BinaryOperationExpr parents,
            // but their operands still need precedence context so a lower-precedence
            // OR group keeps its parentheses.
            {
                "select 1 from b where (x or y) in (1, 2)",
                "select ? from `b` where ( `x` or `y` ) in ( ... )",
            },
            {
                "select 1 from b where (x or y) like 'x%'",
                "select ? from `b` where ( `x` or `y` ) like ?",
            },
            {
                "select 1 from b where (x or y) regexp 'x'",
                "select ? from `b` where ( `x` or `y` ) regexp ?",
            },
            {
                "select 1 from b where (x or y) is null",
                "select ? from `b` where ( `x` or `y` ) is ?",
            },
            {
                "select 1 from b where (x or y) is true",
                "select ? from `b` where ( `x` or `y` ) is true",
            },
            {
                "select 1 from b where (x or y) between 1 and 2",
                "select ? from `b` where ( `x` or `y` ) between ? and ?",
            },
            {
                "select 1 from b where x between (y or z) and 1",
                "select ? from `b` where `x` between ( `y` or `z` ) and ?",
            },
            // Operator-like expressions that are not BinaryOperationExpr still need
            // to keep their outer parentheses under a tighter arithmetic parent.
            // Otherwise `(x LIKE 'a') + 1` would be restored as `x LIKE 'a' + 1`,
            // which MySQL parses as `x LIKE ('a' + 1)`.
            {
                "select 1 from b where (x like 'test') + 1",
                "select ? from `b` where ( `x` like ? ) + ?",
            },
            {
                "select 1 from b where x like ('test' or 'fallback')",
                "select ? from `b` where `x` like ( ? or ? )",
            },
            {
                "select 1 from b where (x regexp 'x') + 1",
                "select ? from `b` where ( `x` regexp ? ) + ?",
            },
            {
                "select 1 from b where (x in (1, 2)) + 1",
                "select ? from `b` where ( `x` in ( ... ) ) + ?",
            },
            {
                "select 1 from b where (x is null) + 1",
                "select ? from `b` where ( `x` is ? ) + ?",
            },
            {
                "select 1 from b where (x is true) + 1",
                "select ? from `b` where ( `x` is true ) + ?",
            },
            {
                "select 1 from b where (x between y and z) + 1",
                "select ? from `b` where ( `x` between `y` and `z` ) + ?",
            },
            {
                "select 1 from b where (x or y) = any (select z from b)",
                "select ? from `b` where ( `x` or `y` ) = any ( select `z` from `b` )",
            },
            {
                "select 1 from b where (x = any (select z from b)) + 1",
                "select ? from `b` where ( `x` = any ( select `z` from `b` ) ) + ?",
            },
            {
                "select 1 from b where exists (select 1 from b where (x or y) in (1, 2))",
                "select ? from `b` where exists ( select ? from `b` where ( `x` or `y` ) in ( ... ) )",
            },
            {
                "select 1 from b where (x or y) member of ('[true]')",
                "select ? from `b` where ( `x` or `y` ) member of ( ? )",
            },
            {
                "select 1 from b where 1 member of ((x or y))",
                "select ? from `b` where ? member of ( ( `x` or `y` ) )",
            },
            {
                "select 1 from b where (1 member of ('[true]')) + 1",
                "select ? from `b` where ( ? member of ( ? ) ) + ?",
            },
            {
                "select 1 from b where binary (x or y)",
                "select ? from `b` where binary ( `x` or `y` )",
            },
            {
                "select 1 from b where (x or y) collate utf8mb4_bin",
                "select ? from `b` where ( `x` or `y` ) collate `utf8mb4_bin`",
            },
            // Unary NOT keeps its operand parenthesized so the restore output does not
            // expose a different parse boundary.
            {
                "select 1 from b where not (x = 1)",
                "select ? from `b` where not ( `x` = ? )",
            },
            {
                "select 1 from b where (not x) = 1",
                "select ? from `b` where ( not `x` ) = ?",
            },
            // Bitwise XOR binds tighter than addition, so `(x + y) ^ z` must preserve
            // the addition group.
            {
                "select 1 from b where (x + y) ^ z = 1",
                "select ? from `b` where ( `x` + `y` ) ^ `z` = ?",
            },
            // Bitwise XOR also binds tighter than multiplication in MySQL.
            {
                "select 1 from b where (x * y) ^ z = 1",
                "select ? from `b` where ( `x` * `y` ) ^ `z` = ?",
            },
            // The XOR child binds tighter than addition, so the right-child
            // parentheses are redundant here.
            {
                "select 1 from b where x + (y ^ z) = 1",
                "select ? from `b` where `x` + `y` ^ `z` = ?",
            },
            // BETWEEN binds weaker than comparison operators, so the parentheses
            // around BETWEEN are semantic in this shape.
            {
                "select 1 from b where (x between y and z) = 1",
                "select ? from `b` where ( `x` between `y` and `z` ) = ?",
            },
            // Arithmetic operators are not safe to reassociate in binding
            // normalization because finite-precision SQL evaluation can differ by
            // grouping, for example with floating-point values.
            {
                "select 1 from b where x + (y + z) = 1",
                "select ? from `b` where `x` + ( `y` + `z` ) = ?",
            },
            {
                "select 1 from b where x * (y * z) = 1",
                "select ? from `b` where `x` * ( `y` * `z` ) = ?",
            },
            // The outer parentheses are required because subtraction binds weaker
            // than multiplication. The left addition can be restored without its own
            // parentheses because the surrounding subtraction is left-associative,
            // while the right addition must keep its grouping.
            {
                "select 1 from b where x * ((y + z) - (u + v)) = 1",
                "select ? from `b` where `x` * ( `y` + `z` - ( `u` + `v` ) ) = ?",
            },
            // Function-call arguments can drop the redundant outer parentheses, but
            // the right side of subtraction must still keep its addition group.
            {
                "select 1 from b where abs(((y + z) - (u + v))) = 1",
                "select ? from `b` where `abs` ( `y` + `z` - ( `u` + `v` ) ) = ?",
            },
        }
        let mut for _, test = range parenthesesTests {
            let mut stmt, _, _ = utilNormalizeWithDefaultDB(t, test.sql);
            let mut n, _ = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            require::Equalf(t, test.normalized, n, "sql: %s", test.sql);
        }

        let mut issue67363SQLs = []string{
            "select pid from t where id=1 and (ptype=1 or ptype=2) order by pid limit 10",
            "select pid from t where id=1 and ((ptype=1) or ptype=2) order by pid limit 10",
            "select pid from t where id=1 and (ptype=1 or (ptype=2)) order by pid limit 10",
            "select pid from t where id=1 and ((ptype=1) or (ptype=2)) order by pid limit 10",
            "select pid from t where (id=1) and (ptype=1 or ptype=2) order by pid limit 10",
            "select pid from t where (id=1) and ((ptype=1) or ptype=2) order by pid limit 10",
            "select pid from t where (id=1) and (ptype=1 or (ptype=2)) order by pid limit 10",
            "select pid from t where (id=1) and ((ptype=1) or (ptype=2)) order by pid limit 10",
            "select pid from t where (id=1 and (ptype=1 or ptype=2)) order by pid limit 10",
            "select pid from t where (id=1 and ((ptype=1) or ptype=2)) order by pid limit 10",
            "select pid from t where (id=1 and (ptype=1 or (ptype=2))) order by pid limit 10",
            "select pid from t where (id=1 and ((ptype=1) or (ptype=2))) order by pid limit 10",
            "select pid from t where ((id=1) and (ptype=1 or ptype=2)) order by pid limit 10",
            "select pid from t where ((id=1) and ((ptype=1) or ptype=2)) order by pid limit 10",
            "select pid from t where ((id=1) and (ptype=1 or (ptype=2))) order by pid limit 10",
            "select pid from t where ((id=1) and ((ptype=1) or (ptype=2))) order by pid limit 10",
        }
        let mut normalized,: digest string;
        let mut for i, sql = range issue67363SQLs {
            let mut stmt, _, _ = utilNormalizeWithDefaultDB(t, sql);
            let mut n, d = bindinfo::NormalizeStmtForBinding(stmt, "", true);
            if i == 0 {
                normalized, digest = n, d;
                continue;
            }
            require::Equalf(t, normalized, n, "sql: %s", sql);
            require::Equalf(t, digest, d, "sql: %s", sql);
        }
}

// benchmark_normalize_stmt_for_binding 对应 Go benchmark BenchmarkNormalizeStmtForBinding，保留循环与分配统计意图。
pub fn benchmark_normalize_stmt_for_binding(b: &mut BenchmarkDraft) {
        let mut testParser = parser::New();
        let mut benchmarks = []struct {
            name string;
            sql  string;
        }{
            {
                name: "simple",
                sql:  "select * from t where a = 1 and b = 1",
            },
            {
                name: "redundant_parentheses",
                sql:  "select * from t where ((a = 1) and ((b = 1) or (b = 2))) and abs((c + 1)) = 3",
            },
            {
                name: "precedence_sensitive",
                sql:  "select * from t where ((a + b) * c = 1) and ((a between b and c) = 1) and ((x * y) ^ z = 1)",
            },
        }

        let mut for _, bm = range benchmarks {
            let mut stmt, err = testParser::ParseOneStmt(bm.sql, "", "");
            require::NoError(b, err);
            b::Run(bm.name, func(b *testing.B) {
                b::ReportAllocs();
                let mut normalized: &str;
                for b::Loop() {
                    normalized, _ = bindinfo::NormalizeStmtForBinding(stmt, "test", true);
                }
                runtime::KeepAlive(normalized);
            });
        }
}

// test_hints_set_id 对应 Go 的 TestHintsSetID，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_hints_set_id() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, index idx_a(a))");
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(test.t, idx_a) */ * from t where a > 10");
        // Verify the added Binding contains ID with restored query block.
        let mut stmt, err = parser::New().ParseOneStmt("select * from t where a > ?", "", "");
        require::NoError(t, err);
        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
        require::Equal(t, "use_index(@`sel_1` `test`.`t` `idx_a`)", binding.ID);

        utilCleanBindingEnv(tk);
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(t, idx_a) */ * from t where a > 10");
        _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
        require::Equal(t, "use_index(@`sel_1` `test`.`t` `idx_a`)", binding.ID);

        utilCleanBindingEnv(tk);
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(@sel_1 t, idx_a) */ * from t where a > 10");
        _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
        require::Equal(t, "use_index(@`sel_1` `test`.`t` `idx_a`)", binding.ID);

        utilCleanBindingEnv(tk);
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(@qb1 t, idx_a) qb_name(qb1) */ * from t where a > 10");
        _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
        require::Equal(t, "use_index(@`sel_1` `test`.`t` `idx_a`)", binding.ID);

        utilCleanBindingEnv(tk);
        tk::MustExec("create global binding for select * from t where a > 10 using select /*+ use_index(T, IDX_A) */ * from t where a > 10");
        _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
        require::Equal(t, "use_index(@`sel_1` `test`.`t` `idx_a`)", binding.ID);

        utilCleanBindingEnv(tk);
        err = tk::ExecToErr("create global binding for select * from t using select /*+ non_exist_hi32::from() */ * from t");
        require::True(t, terror::ErrorEqual(err, parser.ErrParse));
        tk::MustExec("create global binding for select * from t where a > 10 using select * from t where a > 10");
        _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `a` > ?", binding.OriginalSQL);
}

// test_error_bind 对应 Go 的 TestErrorBind，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_error_bind() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustContainErrMsg("create global binding for select * xxx", "You have an error in your SQL syntax");
        tk::MustExec("drop table if exists t");
        tk::MustExec("drop table if exists t1");
        tk::MustExec("create table t(i int, s varchar(20))");
        tk::MustExec("create table t1(i int, s varchar(20))");
        tk::MustExec("create index index_t on t(i,s)");

        let mut _, err = tk::Exec("create global binding for select * from t where i>100 using select * from t use index(index_t) where i>100");
        require::NoError(t, err, "err %v", err);

        let mut stmt, err = parser::New().ParseOneStmt("select * from test . t where i > ?", "", "");
        require::NoError(t, err);
        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select * from `test` . `t` where `i` > ?", binding.OriginalSQL);
        require::Equal(t, "SELECT * FROM `test`.`t` USE INDEX (`index_t`) WHERE `i` > 100", binding.BindSQL);
        require::Equal(t, "test", binding.Db);
        require::Equal(t, bindinfo.StatusEnabled, binding.Status);
        require::NotNil(t, binding.Charset);
        require::NotNil(t, binding.Collation);
        require::NotNil(t, binding.CreateTime);
        require::NotNil(t, binding.UpdateTime);

        tk::MustExec("drop index index_t on t");
        require::Equal(t, 1, len(tk::MustQuery(`show global bindings`).Rows()));
        tk::MustQuery("select * from t where i > 10");
}

// test_best_plan_in_baselines 对应 Go 的 TestBestPlanInBaselines，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_best_plan_in_baselines() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, INDEX ia (a), INDEX ib (b));");
        tk::MustExec("insert into t value(1, 1);");

        // before binding
        tk::MustQuery("select a, b from t where a = 3 limit 1, 100");
        require::Equal(t, "t:ia", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where a = 3 limit 1, 100", "ia(a)");

        tk::MustQuery("select a, b from t where b = 3 limit 1, 100");
        require::Equal(t, "t:ib", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where b = 3 limit 1, 100", "ib(b)");

        tk::MustExec(`create global binding for select a, b from t where a = 1 limit 0, 1 using select /*+ use_index(@sel_1 test.t ia) */ a, b from t where a = 1 limit 0, 1`);
        tk::MustExec(`create global binding for select a, b from t where b = 1 limit 0, 1 using select /*+ use_index(@sel_1 test.t ib) */ a, b from t where b = 1 limit 0, 1`);

        let mut stmt, _, _ = utilNormalizeWithDefaultDB(t, "select a, b from t where a = 1 limit 0, 1");

        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select `a` , `b` from `test` . `t` where `a` = ? limit ...", binding.OriginalSQL);
        require::Equal(t, "SELECT /*+ use_index(@`sel_1` `test`.`t` `ia`)*/ `a`,`b` FROM `test`.`t` WHERE `a` = 1 LIMIT 0,1", binding.BindSQL);
        require::Equal(t, "test", binding.Db);
        require::Equal(t, bindinfo.StatusEnabled, binding.Status);

        tk::MustQuery("select a, b from t where a = 3 limit 1, 10");
        require::Equal(t, "t:ia", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where a = 3 limit 1, 100", "ia(a)");

        tk::MustQuery("select a, b from t where b = 3 limit 1, 100");
        require::Equal(t, "t:ib", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where b = 3 limit 1, 100", "ib(b)");
}

// TestBindingSymbolList tests sql with "?, ?, ?, ?", fixes #13871
// test_binding_symbol_list 对应 Go 的 TestBindingSymbolList，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_symbol_list() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, INDEX ia (a), INDEX ib (b));");
        tk::MustExec("insert into t value(1, 1);");

        // before binding
        tk::MustQuery("select a, b from t where a = 3 limit 1, 100");
        require::Equal(t, "t:ia", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where a = 3 limit 1, 100", "ia(a)");

        tk::MustExec(`create global binding for select a, b from t where a = 1 limit 0, 1 using select a, b from t use index (ib) where a = 1 limit 0, 1`);

        // after binding
        tk::MustQuery("select a, b from t where a = 3 limit 1, 100");
        require::Equal(t, "t:ib", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex("select a, b from t where a = 3 limit 1, 100", "ib(b)");

        // Normalize
        let mut stmt, err = parser::New().ParseOneStmt("select a, b from test . t where a = 1 limit 0, 1", "", "");
        require::NoError(t, err);

        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select `a` , `b` from `test` . `t` where `a` = ? limit ...", binding.OriginalSQL);
        require::Equal(t, "SELECT `a`,`b` FROM `test`.`t` USE INDEX (`ib`) WHERE `a` = 1 LIMIT 0,1", binding.BindSQL);
        require::Equal(t, "test", binding.Db);
        require::Equal(t, bindinfo.StatusEnabled, binding.Status);
        require::NotNil(t, binding.Charset);
        require::NotNil(t, binding.Collation);
        require::NotNil(t, binding.CreateTime);
        require::NotNil(t, binding.UpdateTime);
}

// test_binding_query_in_list 对应 Go 的 TestBindingQueryInList，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_query_in_list() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`create table t (a int)`);

        let mut inList = []string{"(1)", "(1, 2)", "(1, 2, 3)"}
        let mut for _, bindingInList = range inList {
            tk::MustExec(`create global binding using select * from t where a in ` + bindingInList);
            require::NoErrorf(t, dom::BindingHandle().LoadFromStorageToCache(true, false), "bindingInList: %+v", bindingInList);
            require::Equalf(t, 1, len(tk::MustQuery(`show global bindings`).Rows()), "bindingInList: %+v", bindingInList);

            let mut for _, queryInList = range inList {
                tk::MustQuery(`select * from t where a in ` + queryInList);
                tk::MustQuery(`select @@last_plan_from_binding`).Check(testkit::Rows("1"));
            }

            tk::MustExec(`drop global binding for select * from t where a in ` + bindingInList);
            require::NoErrorf(t, dom::BindingHandle().LoadFromStorageToCache(true, false), "bindingInList: %+v", bindingInList);
            require::Equalf(t, 0, len(tk::MustQuery(`show global bindings`).Rows()), "bindingInList: %+v", bindingInList);
        }
}

// test_issue64070 对应 Go 的 TestIssue64070，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_issue64070() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, _ = testkit::CreateMockStoreAndDomain(t);
        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec(`set tidb_opt_enable_fuzzy_binding=true`);
        tk::MustExec(`create table tttt (a int)`);
        tk::MustExec(`create global binding using select * from test.tttt`);
        let mut sqlDigest = tk::MustQuery(`select sql_digest from mysql.bind_info where bind_sql like "%tttt%"`).Rows()[0][0].(string);
        tk::MustExec(fmt::Sprintf(`SET BINDING DISABLED FOR SQL DIGEST '%v'`, sqlDigest)) // disable this binding;
        tk::MustExec(`create global binding using select * from *.tttt`);
        tk::MustQuery(`select bind_sql, status from mysql.bind_info where source != "builtin" order by bind_sql`).Check(testkit::Rows(;
            "SELECT * FROM `*`.`tttt` enabled", // enabled cross-db binding v.s. disabled normal binding;
            "SELECT * FROM `test`.`tttt` disabled"));
        tk::MustQuery(`select * from tttt`);
        tk::MustQuery(`select @@last_plan_from_binding`).Check(testkit::Rows("1")) // use the cross-db binding;
}

// TestBindingInListWithSingleLiteral tests sql with "IN (Lit)", fixes #44298
// test_binding_in_list_with_single_literal 对应 Go 的 TestBindingInListWithSingleLiteral，保留测试步骤、SQL 和断言顺序。
#[test]
pub fn test_binding_in_list_with_single_literal() {
    // Go 的 *testing.T、testkit 和 require 语义在 实现中以调用形状保留；本文件不实际连接数据库或执行测试。
        let mut store, dom = testkit::CreateMockStoreAndDomain(t);

        let mut tk = testkit::NewTestKit(t, store);
        tk::MustExec("use test");
        tk::MustExec("drop table if exists t");
        tk::MustExec("create table t(a int, b int, INDEX ia (a), INDEX ib (b));");
        tk::MustExec("insert into t value(1, 1);");

        // GIVEN
        let mut sqlcmd = "select a, b from t where a in (1)";
        let mut bindingStmt = `create global binding for select a, b from t where a in (1, 2, 3) using select a, b from t use index (ib) where a in (1, 2, 3)`;

        // before binding
        tk::MustQuery(sqlcmd);
        require::Equal(t, "t:ia", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex(sqlcmd, "ia(a)");

        tk::MustExec(bindingStmt);

        // after binding
        tk::MustQuery(sqlcmd);
        require::Equal(t, "t:ib", tk::Session().GetSessionVars().StmtCtx.IndexNames[0]);
        tk::MustUseIndex(sqlcmd, "ib(b)");

        tk::MustQuery("select @@last_plan_from_binding").Check(testkit::Rows("1"));

        // Normalize
        let mut stmt, err = parser::New().ParseOneStmt("select a, b from test . t where a in (1)", "", "");
        require::NoError(t, err);

        let mut _, noDBDigest = bindinfo::NormalizeStmtForBinding(stmt, "", true);
        let mut binding, matched = dom::BindingHandle().MatchingBinding(tk::Session(), noDBDigest, bindinfo::CollectTableNames(stmt));
        require::True(t, matched);
        require::Equal(t, "select `a` , `b` from `test` . `t` where `a` in ( ... )", binding.OriginalSQL);
        require::Equal(t, "SELECT `a`,`b` FROM `test`.`t` USE INDEX (`ib`) WHERE `a` IN (1,2,3)", binding.BindSQL);
        require::Equal(t, "test", binding.Db);
        require::Equal(t, bindinfo.StatusEnabled, binding.Status);
        require::NotNil(t, binding.Charset);
        require::NotNil(t, binding.Collation);
        require::NotNil(t, binding.CreateTime);
        require::NotNil(t, binding.UpdateTime);
}

// utilCleanBindingEnv 对应 Go 的同名辅助函数，保留入参和返回值语义。
pub fn utilCleanBindingEnv(tk: testkit::TestKit) {
        tk::MustExec("update mysql.bind_info set status='deleted' where source != 'builtin'");
        tk::MustExec(`admin reload bindings`);
        tk::MustExec("delete from mysql.bind_info where source != 'builtin'");
        tk::MustExec(`admin reload bindings`);
}

// utilNormalizeWithDefaultDB 对应 Go 的同名辅助函数，保留入参和返回值语义。
pub fn utilNormalizeWithDefaultDB(t: TestingT, sql: &str) -> (stmt ast::StmtNode, normalized, digest string) {
        let mut testParser = parser::New();
        let mut stmt, err = testParser::ParseOneStmt(sql, "", "");
        require::NoError(t, err);
        let mut normalized, digestResult = parser::NormalizeDigestForBinding(bindinfo::RestoreDBForBinding(stmt, "test"));
        return stmt, normalized, digestResult::String();
}
"########################################;

/// 验证 `pickCachedBinding` 在合并缓存绑定时的两条规则：
///
/// 1. 同一 SQL 摘要（SQLDigest）存在多个版本时，优先选择
///    `UpdateTime` 更新的绑定（新版本覆盖旧版本）；
/// 2. 若最新版本的状态为“已删除”（deleted，即墓碑标记，用于在多节点间
///    传播删除事件而非立即物理删除），则该绑定视为不存在，返回 `None`。
#[test]
fn canonical_cached_binding_prefers_newer_and_honors_delete_tombstone() {
    use std::sync::Arc;

    // 构造两个同摘要、不同更新时间的启用状态绑定：old(10) 与 newer(20)。
    let old = Arc::new(crate::Binding {
        Status: crate::StatusEnabled.to_owned(),
        UpdateTime: crate::BindingTime(10),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    let newer = Arc::new(crate::Binding {
        Status: crate::StatusEnabled.to_owned(),
        UpdateTime: crate::BindingTime(20),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    // 合并后应选中更新时间更大的 newer（UpdateTime=20）。
    assert_eq!(
        crate::pickCachedBinding(Some(old), [Arc::clone(&newer)])
            .unwrap()
            .UpdateTime,
        crate::BindingTime(20)
    );
    // 最新版本被标记为删除（UpdateTime=30 的墓碑）时，整个绑定应被判定为不存在。
    let deleted = Arc::new(crate::Binding {
        Status: crate::StatusDeleted.to_owned(),
        UpdateTime: crate::BindingTime(30),
        SQLDigest: "digest".to_owned(),
        ..crate::Binding::default()
    });
    assert!(crate::pickCachedBinding(Some(newer), [deleted]).is_none());
}

mod operator_parity {
    use std::sync::{Arc, Mutex};

    use crate::{
        Binding, BindingCache, BindingCacheUpdater, BindingOperator, BindingRow, BindingSqlContext,
        BindingStore, BindingTime, BindingValidator, Result, SqlValue, StatusDisabled,
        StatusEnabled, TableName, bindingOperator,
    };

    #[derive(Default)]
    struct StoreState {
        replaced: Vec<Arc<Binding>>,
        deleted: Vec<(Vec<String>, BindingTime)>,
        statuses: Vec<(String, String, BindingTime)>,
        gc_cutoffs: Vec<BindingTime>,
    }

    #[derive(Default)]
    struct RecordingStore(Mutex<StoreState>);

    impl BindingStore for RecordingStore {
        fn read_bindings_since(&self, _since: BindingTime) -> Result<Vec<Arc<Binding>>> {
            Ok(Vec::new())
        }

        fn replace_bindings(&self, bindings: &[Arc<Binding>]) -> Result<()> {
            self.0.lock().unwrap().replaced = bindings.to_vec();
            Ok(())
        }

        fn mark_deleted(&self, sql_digests: &[String], at: BindingTime) -> Result<u64> {
            self.0
                .lock()
                .unwrap()
                .deleted
                .push((sql_digests.to_vec(), at));
            Ok(sql_digests.len() as u64)
        }

        fn set_status(&self, sql_digest: &str, status: &str, at: BindingTime) -> Result<bool> {
            self.0
                .lock()
                .unwrap()
                .statuses
                .push((sql_digest.to_owned(), status.to_owned(), at));
            Ok(false)
        }

        fn gc_deleted_before(&self, cutoff: BindingTime) -> Result<u64> {
            self.0.lock().unwrap().gc_cutoffs.push(cutoff);
            Ok(0)
        }

        fn save_usage(
            &self,
            _sql_digest: &str,
            _plan_digest: &str,
            _used_at: BindingTime,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CacheState {
        reloads: usize,
        direct_sets: usize,
        direct_removes: usize,
    }

    #[derive(Default)]
    struct RecordingCache(Mutex<CacheState>);

    impl BindingCache for RecordingCache {
        fn MatchingBinding(
            &self,
            _current_db: &str,
            _no_db_digest: &str,
            _table_names: &[TableName],
        ) -> (Option<Arc<Binding>>, bool) {
            (None, false)
        }

        fn GetBinding(&self, _sql_digest: &str) -> Option<Arc<Binding>> {
            None
        }

        fn GetAllBindings(&self) -> Vec<Arc<Binding>> {
            Vec::new()
        }

        fn SetBinding(&self, _sql_digest: String, _binding: Arc<Binding>) -> Result<()> {
            self.0.lock().unwrap().direct_sets += 1;
            Ok(())
        }

        fn RemoveBinding(&self, _sql_digest: &str) {
            self.0.lock().unwrap().direct_removes += 1;
        }

        fn SetMemCapacity(&self, _capacity: i64) {}
        fn GetMemUsage(&self) -> i64 {
            0
        }
        fn GetMemCapacity(&self) -> i64 {
            0
        }
        fn Size(&self) -> usize {
            0
        }
        fn Close(&self) {}
    }

    impl BindingCacheUpdater for RecordingCache {
        fn LoadFromStorageToCache(&self, _full_load: bool, _from_remote: bool) -> Result<()> {
            self.0.lock().unwrap().reloads += 1;
            Ok(())
        }

        fn UpdateBindingUsageInfoToStorage(&self) -> Result<()> {
            Ok(())
        }
        fn LastUpdateTime(&self) -> BindingTime {
            BindingTime::default()
        }
    }

    struct Validator;

    impl BindingValidator for Validator {
        fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct SqlContext(Mutex<Vec<String>>);

    impl BindingValidator for SqlContext {
        fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
            Ok(())
        }
    }

    impl BindingSqlContext for SqlContext {
        fn execute(&self, sql: &str, _args: &[SqlValue]) -> Result<Vec<BindingRow>> {
            self.0.lock().unwrap().push(sql.to_owned());
            Ok(Vec::new())
        }

        fn plan_digest(&self, _schema: &str, _binding_sql: &str) -> Result<String> {
            Ok(String::new())
        }
    }

    fn operator() -> (bindingOperator, Arc<RecordingStore>, Arc<RecordingCache>) {
        let store = Arc::new(RecordingStore::default());
        let cache = Arc::new(RecordingCache::default());
        (
            bindingOperator {
                store: store.clone(),
                cache: cache.clone(),
                lease: std::time::Duration::from_secs(3),
            },
            store,
            cache,
        )
    }

    #[test]
    fn create_matches_go_metadata_and_reloads_cache() {
        let (operator, store, cache) = operator();
        operator
            .CreateBinding(
                &Validator,
                vec![Binding {
                    BindSQL: "select * from t".to_owned(),
                    Db: "TeSt".to_owned(),
                    CreateTime: BindingTime(1),
                    ..Binding::default()
                }],
            )
            .unwrap();

        let state = store.0.lock().unwrap();
        assert_eq!(state.replaced.len(), 1);
        assert!(state.replaced[0].CreateTime > BindingTime(1));
        assert_eq!(state.replaced[0].Db, "test");
        drop(state);
        let cache = cache.0.lock().unwrap();
        assert_eq!(cache.reloads, 1);
        assert_eq!(cache.direct_sets, 0);
    }

    #[test]
    fn empty_drop_is_rejected_like_go() {
        let (operator, _, _) = operator();
        assert_eq!(
            operator.DropBinding(&[]).unwrap_err().to_string(),
            "sql digest is empty"
        );
    }

    #[test]
    fn drop_and_unchanged_status_reload_instead_of_mutating_cache_directly() {
        let (operator, _, cache) = operator();
        assert_eq!(operator.DropBinding(&["digest".to_owned()]).unwrap(), 1);
        assert!(!operator.SetBindingStatus(StatusDisabled, "digest").unwrap());
        let cache = cache.0.lock().unwrap();
        assert_eq!(cache.reloads, 2);
        assert_eq!(cache.direct_removes, 0);
    }

    #[test]
    fn unsupported_status_matches_no_rows_like_go() {
        let (operator, store, cache) = operator();
        for invalid in ["", "deleted", "using", "arbitrary"] {
            assert!(!operator.SetBindingStatus(invalid, "digest").unwrap());
        }
        assert!(!operator.SetBindingStatus(StatusEnabled, "digest").unwrap());
        assert_eq!(store.0.lock().unwrap().statuses.len(), 1);
        assert_eq!(cache.0.lock().unwrap().reloads, 5);
    }

    #[test]
    fn gc_uses_ten_leases_and_saturates_large_durations() {
        let (mut operator, store, _) = operator();
        operator.lease = std::time::Duration::MAX;
        operator.GCBinding().unwrap();
        assert!(store.0.lock().unwrap().gc_cutoffs[0] < BindingTime::now());
    }

    #[test]
    fn lock_executes_the_go_lock_statement_without_arguments() {
        let context = SqlContext::default();
        crate::lockBindInfoTable(&context).unwrap();
        assert_eq!(
            context.0.lock().unwrap().as_slice(),
            [crate::LockBindInfoSQL]
        );
    }
}
