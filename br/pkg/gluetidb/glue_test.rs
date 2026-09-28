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

//! Go-equivalent tests for `br/pkg/gluetidb/glue_test.go`.
//!
//! Real TiDB `session`/`domain`/`testkit`/`model` are unavailable on this
//! platform (no kv/domain/kvproto/grpcio). Boundaries are mocked via
//! [`DomainHooks`] while preserving Go call order, schema-version bumps,
//! batch CreateTables shape, and domain cleanup.
//!
//! 对齐 Go `TestTheSessionIsoation`：在无真实 TiDB 时用 DomainHooks 模拟
//! bootstrap/启动顺序、SchemaMetaVersion 递增、批量建表形状与 Domain 清理。
//! 约束：桩会话通过 pending_* 注入描述符，因 glue 侧 TableInfo/PolicyInfo 为空。
//! 断言覆盖库/策略/表落库、SQL 记录与放置策略绑定，失败即视为调用序偏离 Go。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_glue::{
    BatchCreateTableSession, CIStr, Context, CreateTableOption, DBInfo, Domain, PolicyInfo,
    Session, SessionCtxHandle, Storage, TableInfo,
};
use astersql_errors::SharedError;

// 测试入口依赖 DomainHooks 注入与 New 构造的 Glue。
use crate::{DomainHooks, Filter, New, set_domain_hooks_for_test};

/// Stand-in for `kv.Storage` from `session.CreateStoreAndBootstrap`.
/// 引导存储替身：仅提供 name，满足 CreateSession 入参。
struct BootstrapStore;

impl Storage for BootstrapStore {
    // 固定名称便于日志/调试区分真实 store。
    // 不实现其他 Storage 方法，沿用 trait 默认。
    fn name(&self) -> &str {
        "bootstrap-store"
    }
}

/// Local descriptors mirroring Go `model.*` inputs (glue stand-ins are empty).
/// 本地描述符：因 glue 桩类型无字段，用旁路结构承载 Go model 输入语义。
#[derive(Clone, Debug)]
struct ColumnDesc {
    // 列名，仅用于表描述完整性。
    // Go model.ColumnInfo 在此被压缩为名称。
    name: String,
}

#[derive(Clone, Debug)]
struct TableDesc {
    name: String,
    // 可选放置策略名，断言与 PolicyDesc 交叉引用。
    placement_policy: Option<String>,
    columns: Vec<ColumnDesc>,
}

#[derive(Clone, Debug)]
struct PolicyDesc {
    name: String,
    // followers 数对齐 Go PlacementPolicy 字段。
    // 最终断言校验 followers=4/2。
    followers: u64,
}

/// 隔离测试共享状态：版本号、已建对象与两侧 SQL 轨迹。
struct IsolationState {
    /// Schema meta version — Go `se.GetInfoSchema().SchemaMetaVersion()`.
    /// 初始 100；DDL 成功后必须严格递增。
    schema_meta_version: i64,
    // glue 会话建库结果。
    databases: Vec<String>,
    policies: Vec<PolicyDesc>,
    // 按库名分组的表描述。
    tables: HashMap<String, Vec<TableDesc>>,
    // Session.ExecuteInternal 记录。
    exec_sqls: Vec<String>,
    // TestKit.MustExec 记录，与 exec_sqls 分离以模拟双通道。
    testkit_sqls: Vec<String>,
}

impl IsolationState {
    // 与 Go 用例相近的非零初始 schema 版本。
    fn new() -> Self {
        Self {
            schema_meta_version: 100,
            databases: Vec::new(),
            policies: Vec::new(),
            tables: HashMap::new(),
            exec_sqls: Vec::new(),
            testkit_sqls: Vec::new(),
        }
    }
}

/// Concrete glue session (Go `tidbSession`) shared for type-assert paths.
/// 具体会话：pending_* 在调用前注入，因 trait 入参桩对象无业务字段。
struct IsolationSession {
    state: Arc<Mutex<IsolationState>>,
    // 下一次 CreateDatabaseOnExistError 使用的库名。
    pending_db: Mutex<Option<String>>,
    // 下一次 CreatePlacementPolicy 使用的策略描述。
    pending_policy: Mutex<Option<PolicyDesc>>,
    // 下一次 CreateTables 使用的表描述映射。
    pending_tables: Mutex<Option<HashMap<String, Vec<TableDesc>>>>,
}

impl IsolationSession {
    fn new(state: Arc<Mutex<IsolationState>>) -> Self {
        Self {
            state,
            pending_db: Mutex::new(None),
            pending_policy: Mutex::new(None),
            pending_tables: Mutex::new(None),
        }
    }

    // 读取当前 schema 元版本，供前后对比。
    fn SchemaMetaVersion(&self) -> i64 {
        self.state.lock().unwrap().schema_meta_version
    }

    // 注入待建库名（take 语义，一次性）。
    fn set_pending_db(&self, name: &str) {
        *self.pending_db.lock().unwrap() = Some(name.to_string());
    }

    // 注入待建放置策略。
    fn set_pending_policy(&self, policy: PolicyDesc) {
        *self.pending_policy.lock().unwrap() = Some(policy);
    }

    // 注入待批量建表描述。
    fn set_pending_tables(&self, tables: HashMap<String, Vec<TableDesc>>) {
        *self.pending_tables.lock().unwrap() = Some(tables);
    }
}

impl Session for IsolationSession {
    // 委托内部执行，忽略参数绑定。
    fn Execute(&mut self, ctx: Context, sql: &str) -> Result<(), SharedError> {
        self.ExecuteInternal(ctx, sql, &[])
    }

    fn ExecuteInternal(
        &mut self,
        _ctx: Context,
        sql: &str,
        _args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError> {
        // 只记录 SQL 文本，供最终轨迹断言。
        self.state.lock().unwrap().exec_sqls.push(sql.to_string());
        Ok(())
    }

    fn CreateDatabaseOnExistError(
        &mut self,
        _ctx: Context,
        _schema: &DBInfo,
    ) -> Result<(), SharedError> {
        // take pending 库名；缺失时用 unnamed 兜底便于暴露测试错误。
        let name = self
            .pending_db
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "unnamed".into());
        let mut st = self.state.lock().unwrap();
        st.databases.push(name);
        // 建库成功必须 bump schema 版本。
        st.schema_meta_version += 1;
        Ok(())
    }

    // 单表创建本用例不走，保持空成功。
    fn CreateTable(
        &mut self,
        _ctx: Context,
        _dbName: CIStr,
        _table: &TableInfo,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        Ok(())
    }

    fn CreatePlacementPolicy(
        &mut self,
        _ctx: Context,
        _policy: &PolicyInfo,
    ) -> Result<(), SharedError> {
        // 必须事先 set_pending_policy，否则视为测试编排错误。
        let policy = self.pending_policy.lock().unwrap().take().ok_or_else(|| {
            astersql_errors::New("CreatePlacementPolicy: missing pending policy desc")
        })?;
        let mut st = self.state.lock().unwrap();
        st.policies.push(policy);
        // Go: after CreatePlacementPolicy, SchemaMetaVersion must increase.
        // 与 Go 一致：策略创建后 SchemaMetaVersion 递增。
        st.schema_meta_version += 1;
        Ok(())
    }

    // 资源释放空操作。
    // 隔离测试不依赖会话 Close 副作用。
    fn Close(&mut self) {}

    // 全局变量桩：返回空串。
    // GetGlobalSysVar 同样空串，避免无关断言。
    fn GetGlobalVariable(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }

    fn GetGlobalSysVar(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }

    // 擦除后的空 sessionctx。
    // 本用例不向下转型该句柄。
    fn GetSessionCtx(&mut self) -> SessionCtxHandle {
        Arc::new(())
    }

    // 表模式变更本用例不覆盖。
    // 保持 Ok 以免误调用导致失败。
    fn AlterTableMode(
        &mut self,
        _ctx: Context,
        _schemaID: i64,
        _tableID: i64,
        _tableMode: i32,
    ) -> Result<(), SharedError> {
        Ok(())
    }

    // 元数据刷新空成功。
    // 与 AlterTableMode 一样仅占位。
    fn RefreshMeta(
        &mut self,
        _ctx: Context,
        _args: &astersql_br_pkg_glue::RefreshMetaArgs,
    ) -> Result<(), SharedError> {
        Ok(())
    }
}

impl BatchCreateTableSession for IsolationSession {
    fn CreateTables(
        &mut self,
        _ctx: Context,
        tables: HashMap<String, Vec<TableInfo>>,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        // 取出 pending 描述；缺省空 map 会使计数断言失败从而暴露漏注入。
        let descs = self
            .pending_tables
            .lock()
            .unwrap()
            .take()
            .unwrap_or_default();
        // 各库 TableInfo 条数必须与描述符一致。
        for (db, infos) in &tables {
            let expected = descs.get(db).map(|v| v.len()).unwrap_or(0);
            assert_eq!(
                infos.len(),
                expected,
                "CreateTables count mismatch for db {db}"
            );
        }
        let mut st = self.state.lock().unwrap();
        // 落库描述供最终断言放置策略绑定。
        for (db, tdescs) in descs {
            st.tables.insert(db, tdescs);
        }
        // 批量建表同样 bump schema 版本。
        st.schema_meta_version += 1;
        Ok(())
    }
}

/// `Box<dyn Session>` handle over shared [`IsolationSession`] (interior Mutex fields).
/// 对象安全包装：CreateSession 返回 Box<dyn Session>，内部共享具体会话。
struct SessionHandle(Arc<IsolationSession>);

impl Session for SessionHandle {
    // 与 IsolationSession 行为镜像，操作共享 Arc 状态。
    fn Execute(&mut self, ctx: Context, sql: &str) -> Result<(), SharedError> {
        self.ExecuteInternal(ctx, sql, &[])
    }
    fn ExecuteInternal(
        &mut self,
        _ctx: Context,
        sql: &str,
        _args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError> {
        // 写入共享 exec_sqls。
        self.0.state.lock().unwrap().exec_sqls.push(sql.to_string());
        Ok(())
    }
    fn CreateDatabaseOnExistError(
        &mut self,
        _ctx: Context,
        _schema: &DBInfo,
    ) -> Result<(), SharedError> {
        // 从共享 pending_db take 库名。
        let name = self
            .0
            .pending_db
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "unnamed".into());
        let mut st = self.0.state.lock().unwrap();
        st.databases.push(name);
        st.schema_meta_version += 1;
        Ok(())
    }
    // 单表路径空成功。
    fn CreateTable(
        &mut self,
        _ctx: Context,
        _dbName: CIStr,
        _table: &TableInfo,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    fn CreatePlacementPolicy(
        &mut self,
        _ctx: Context,
        _policy: &PolicyInfo,
    ) -> Result<(), SharedError> {
        // 测试主路径经 glue_se 调用此实现。
        let policy = self
            .0
            .pending_policy
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| {
                astersql_errors::New("CreatePlacementPolicy: missing pending policy desc")
            })?;
        let mut st = self.0.state.lock().unwrap();
        st.policies.push(policy);
        // 版本递增供循环内 before/after 断言。
        st.schema_meta_version += 1;
        Ok(())
    }
    fn Close(&mut self) {}
    fn GetGlobalVariable(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }
    fn GetGlobalSysVar(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }
    fn GetSessionCtx(&mut self) -> SessionCtxHandle {
        Arc::new(())
    }
    fn AlterTableMode(
        &mut self,
        _ctx: Context,
        _schemaID: i64,
        _tableID: i64,
        _tableMode: i32,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    fn RefreshMeta(
        &mut self,
        _ctx: Context,
        _args: &astersql_br_pkg_glue::RefreshMetaArgs,
    ) -> Result<(), SharedError> {
        Ok(())
    }
}

/// Domain hooks simulating bootstrap / GetDomain / CreateSession.
/// 模拟 bootstrap 已存在 Domain，以及启动/建会话计数。
struct IsolationHooks {
    // None 表示尚未/已关闭 Domain。
    domain: Mutex<Option<Arc<Domain>>>,
    state: Arc<Mutex<IsolationState>>,
    // 最近一次 CreateSession 的具体会话，供测试取回。
    last_session: Mutex<Option<Arc<IsolationSession>>>,
    // StartOwnerManager 调用次数。
    started_owner: Mutex<i32>,
    // StartDomain 调用次数。
    started_domain: Mutex<i32>,
}

impl IsolationHooks {
    fn new(state: Arc<Mutex<IsolationState>>) -> Self {
        Self {
            // 初始带 bootstrap Domain，对齐 Go CreateStoreAndBootstrap。
            domain: Mutex::new(Some(Arc::new(Domain))),
            state,
            last_session: Mutex::new(None),
            started_owner: Mutex::new(0),
            started_domain: Mutex::new(0),
        }
    }

    // 关闭引导 Domain，迫使 CreateSession 显式重启。
    fn CloseBootstrapDomain(&self) {
        *self.domain.lock().unwrap() = None;
    }

    // 供清理与断言读取当前 Domain 句柄。
    fn GetDomainHandle(&self) -> Option<Arc<Domain>> {
        self.domain.lock().unwrap().clone()
    }
}

impl DomainHooks for IsolationHooks {
    fn GetOrCreateDomainWithFilter(
        &self,
        store: Option<&dyn Storage>,
        _filter: Option<&dyn Filter>,
    ) -> Result<Option<Arc<Domain>>, SharedError> {
        // store=None：只查询；Some：按需创建。
        if store.is_none() {
            return Ok(self.domain.lock().unwrap().clone());
        }
        let mut g = self.domain.lock().unwrap();
        if g.is_none() {
            // 创建新 Domain 实例。
            *g = Some(Arc::new(Domain));
        }
        Ok(g.clone())
    }

    fn StartOwnerManager(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        // 计数供断言 CreateSession 触发了启动链。
        *self.started_owner.lock().unwrap() += 1;
        Ok(())
    }

    fn StartDomain(&self, _dom: &Domain) -> Result<(), SharedError> {
        *self.started_domain.lock().unwrap() += 1;
        Ok(())
    }

    // MDL 初始化空成功。
    fn InitMDLVariable(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        Ok(())
    }

    // 统计循环空成功。
    fn UpdateTableStatsLoop(&self, _dom: &Domain) -> Result<(), SharedError> {
        Ok(())
    }

    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        // 记住具体会话，测试用其 pending_* 注入。
        let se = Arc::new(IsolationSession::new(Arc::clone(&self.state)));
        *self.last_session.lock().unwrap() = Some(Arc::clone(&se));
        Ok(Box::new(SessionHandle(se)))
    }

    fn CloseDomain(&self, _dom: &Domain) {
        // 清理后 GetDomainHandle 为 None。
        *self.domain.lock().unwrap() = None;
    }
}

/// Stand-in for `testkit.NewTestKit(t, store)`.
/// 模拟 testkit：要求 test_db 已由 glue 会话创建。
struct TestKit {
    state: Arc<Mutex<IsolationState>>,
}

impl TestKit {
    fn MustExec(&self, sql: &str) {
        let mut st = self.state.lock().unwrap();
        // 与 Go 用例依赖顺序一致：先 glue 建库再 testkit 执行。
        assert!(
            st.databases.iter().any(|d| d == "test_db"),
            "testkit MustExec requires test_db created by glue session"
        );
        st.testkit_sqls.push(sql.to_string());
        // create table 语句额外 bump 版本。
        if sql.to_lowercase().starts_with("create table") {
            st.schema_meta_version += 1;
        }
    }
}

/// Go `t.Cleanup` — close domain recreated by glue.
/// RAII 清理：测试结束关闭 glue 重建的 Domain，避免泄漏到其他用例。
struct DomainCleanup {
    hooks: Arc<IsolationHooks>,
}

impl Drop for DomainCleanup {
    fn drop(&mut self) {
        // 若仍存活则 CloseDomain。
        if let Some(dom) = self.hooks.GetDomainHandle() {
            self.hooks.CloseDomain(&dom);
        }
    }
}

/// `TestTheSessionIsoation` — Go `TestTheSessionIsoation`.
/// 会话隔离主用例：校验启动链、DDL 版本递增、批量建表与双通道 SQL。
#[test]
fn test_the_session_isoation() {
    // 串行化 domain 全局 hooks，避免与其他测试互相污染。
    let _domain_state_guard = crate::test_support::domain_state_guard();

    let state = Arc::new(Mutex::new(IsolationState::new()));
    let hooks = Arc::new(IsolationHooks::new(Arc::clone(&state)));
    // 注入测试 hooks，替换默认 Nop。
    set_domain_hooks_for_test(Some(hooks.clone()));

    let store = BootstrapStore;
    let ctx = Context::new();

    // Go: close bootstrap domain so CreateSession must start it explicitly.
    // 关闭引导 Domain，迫使后续 CreateSession 走 StartOwner/StartDomain。
    assert!(hooks.GetDomainHandle().is_some());
    hooks.CloseBootstrapDomain();
    assert!(hooks.GetDomainHandle().is_none());

    let g = New();
    // CreateSession 应触发 domain 启动并返回可用会话。
    let mut glue_se = g.CreateSession(&store).expect("CreateSession");
    assert!(
        *hooks.started_owner.lock().unwrap() >= 1,
        "StartOwnerManager called"
    );
    assert!(
        *hooks.started_domain.lock().unwrap() >= 1,
        "StartDomain called"
    );
    // Domain 应已被重建。
    assert!(hooks.GetDomainHandle().is_some());

    // 取回具体会话以注入 pending 描述符。
    let concrete = hooks
        .last_session
        .lock()
        .unwrap()
        .clone()
        .expect("concrete tidbSession");

    // 作用域结束时关闭 Domain。
    let _cleanup = DomainCleanup {
        hooks: hooks.clone(),
    };

    // Go: CreateDatabaseOnExistError(ctx, &model.DBInfo{Name: "test_db"})
    // 注入库名后经 trait 调用建库。
    concrete.set_pending_db("test_db");
    glue_se
        .CreateDatabaseOnExistError(ctx.clone(), &DBInfo::default())
        .expect("CreateDatabaseOnExistError");

    let tk = TestKit {
        state: Arc::clone(&state),
    };
    // testkit 通道：use + create table。
    tk.MustExec("use test_db");
    tk.MustExec("create table t(id int)");

    // glue 会话通道：切换到 test 库。
    glue_se
        .ExecuteInternal(ctx.clone(), "use test;", &[])
        .expect("ExecuteInternal use test");

    // 三张表：无策略 / threereplication / fivereplication。
    let infos = vec![
        TableDesc {
            name: "tables_1".into(),
            placement_policy: None,
            columns: vec![ColumnDesc { name: "foo".into() }],
        },
        TableDesc {
            name: "tables_2".into(),
            placement_policy: Some("threereplication".into()),
            columns: vec![ColumnDesc { name: "foo".into() }],
        },
        TableDesc {
            name: "tables_3".into(),
            placement_policy: Some("fivereplication".into()),
            columns: vec![ColumnDesc { name: "foo".into() }],
        },
    ];

    // 先建两条放置策略，顺序与 Go 用例一致。
    let polices = vec![
        PolicyDesc {
            name: "fivereplication".into(),
            followers: 4,
        },
        PolicyDesc {
            name: "threereplication".into(),
            followers: 2,
        },
    ];

    for pinfo in polices {
        // 每次创建前后比较 SchemaMetaVersion。
        let before = concrete.SchemaMetaVersion();
        concrete.set_pending_policy(pinfo);
        glue_se
            .CreatePlacementPolicy(ctx.clone(), &PolicyInfo::default())
            .expect("CreatePlacementPolicy");
        let after = concrete.SchemaMetaVersion();
        assert!(
            after > before,
            "schema meta version must grow after CreatePlacementPolicy: before={before} after={after}"
        );
    }

    // 准备批量建表：描述挂在 test 库下。
    let mut tables_desc = HashMap::new();
    tables_desc.insert("test".to_string(), infos.clone());
    concrete.set_pending_tables(tables_desc);

    // glue 侧 TableInfo 桩列表，条数必须为 3。
    let mut glue_tables: HashMap<String, Vec<TableInfo>> = HashMap::new();
    glue_tables.insert(
        "test".to_string(),
        vec![
            TableInfo::default(),
            TableInfo::default(),
            TableInfo::default(),
        ],
    );

    // Go: glueSe.(glue.BatchCreateTableSession).CreateTables(...)
    // Type-assert path: operate on the concrete IsolationSession via shared state.
    // Rust 无 Go 类型断言：用独立 IsolationSession 模拟 BatchCreateTableSession。
    {
        let mut batch = IsolationSession::new(Arc::clone(&state));
        let mut pending = HashMap::new();
        pending.insert("test".to_string(), infos);
        batch.set_pending_tables(pending);
        // Clear concrete pending (already transferred conceptually).
        // 清空 concrete 上残留 pending，避免双写。
        let _ = concrete.pending_tables.lock().unwrap().take();
        BatchCreateTableSession::CreateTables(&mut batch, ctx, glue_tables, vec![])
            .expect("CreateTables");
    }

    // 最终状态：库、SQL 轨迹、策略与表绑定均对齐 Go。
    let st = state.lock().unwrap();
    assert_eq!(st.databases, vec!["test_db".to_string()]);
    assert_eq!(st.exec_sqls, vec!["use test;".to_string()]);
    assert_eq!(
        st.testkit_sqls,
        vec![
            "use test_db".to_string(),
            "create table t(id int)".to_string()
        ]
    );
    // 策略顺序与 followers 精确匹配。
    assert_eq!(st.policies.len(), 2);
    assert_eq!(st.policies[0].name, "fivereplication");
    assert_eq!(st.policies[0].followers, 4);
    assert_eq!(st.policies[1].name, "threereplication");
    assert_eq!(st.policies[1].followers, 2);
    let test_tables = st.tables.get("test").expect("tables under test db");
    assert_eq!(test_tables.len(), 3);
    assert_eq!(test_tables[0].name, "tables_1");
    assert_eq!(test_tables[1].name, "tables_2");
    // tables_2 绑定 threereplication。
    assert_eq!(
        test_tables[1].placement_policy.as_deref(),
        Some("threereplication")
    );
    assert_eq!(test_tables[2].name, "tables_3");
    // tables_3 绑定 fivereplication。
    assert_eq!(
        test_tables[2].placement_policy.as_deref(),
        Some("fivereplication")
    );
}
