// Copyright 2026 AsterSQL.

//! 与 Go `br/pkg/gluetidb` 公开契约对等：New 副作用、库过滤器、InfoSchemaFilter、
//! Domain 生命周期与 Open 委托 gluetikv。全程用 domain_state_guard 隔离全局状态。

use std::sync::{Arc, Mutex};

use astersql_br_pkg_glue::{CIStr, ClientCLP, Context, Domain, Session, SessionCtxHandle, Storage};
use astersql_errors::SharedError;

use crate::{
    ActionType, DBInfo, DomainHooks, Filter, FilterLoadSpecifiedDBAndSysDBs, FilterLoadSysDBs,
    GetGlobalConfigBits, InfoSchema, New, NewInfoSchemaFilter, SchemaDiff,
    set_domain_hooks_for_test,
};

// 按 schema_id 查库名的 InfoSchema 替身，驱动 SkipLoadDiff 查表分支。
struct MapIS {
    schemas: Mutex<std::collections::HashMap<i64, DBInfo>>,
}

impl InfoSchema for MapIS {
    fn SchemaByID(&self, schema_id: i64) -> Option<DBInfo> {
        self.schemas.lock().unwrap().get(&schema_id).cloned()
    }
}

// 占位 Storage；Open 路径会委托 gluetikv 并用 path 作 name。
struct DummyStore;
impl Storage for DummyStore {
    fn name(&self) -> &str {
        "s"
    }
}

// 记录 CreateSession / CloseDomain 次数与缓存 Domain，验证 closeDomain=true。
struct RecHooks {
    created: Mutex<i32>,
    closed: Mutex<i32>,
    sessions_closed: Arc<Mutex<i32>>,
    domain: Mutex<Option<Arc<Domain>>>,
}

// 空操作 Session：DomainHooks::CreateSession 只需返回可用句柄。
struct NopSession {
    closed: Arc<Mutex<i32>>,
}
impl Session for NopSession {
    // 以下方法一律成功返回，仅满足 DomainHooks 创建会话需求。
    fn Execute(&mut self, _ctx: Context, _sql: &str) -> Result<(), SharedError> {
        Ok(())
    }
    fn ExecuteInternal(
        &mut self,
        _ctx: Context,
        _sql: &str,
        _args: &[Box<dyn std::any::Any + Send>],
    ) -> Result<(), SharedError> {
        Ok(())
    }
    // DDL 接口空成功：本用例不验证真实建库建表。
    fn CreateDatabaseOnExistError(
        &mut self,
        _ctx: Context,
        _schema: &astersql_br_pkg_glue::DBInfo,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    fn CreateTable(
        &mut self,
        _ctx: Context,
        _dbName: CIStr,
        _table: &astersql_br_pkg_glue::TableInfo,
        _cs: Vec<astersql_br_pkg_glue::CreateTableOption>,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    fn CreatePlacementPolicy(
        &mut self,
        _ctx: Context,
        _policy: &astersql_br_pkg_glue::PolicyInfo,
    ) -> Result<(), SharedError> {
        Ok(())
    }
    // Close 无状态。
    fn Close(&mut self) {
        *self.closed.lock().unwrap() += 1;
    }
    // 变量查询返回空串即可。
    fn GetGlobalVariable(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }
    fn GetGlobalSysVar(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }
    // 占位 SessionCtx，避免 Option 缺口。
    fn GetSessionCtx(&mut self) -> SessionCtxHandle {
        Arc::new(())
    }
    // 表模式/元数据刷新同样空成功。
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

impl DomainHooks for RecHooks {
    fn GetOrCreateDomainWithFilter(
        &self,
        store: Option<&dyn Storage>,
        _filter: Option<&dyn Filter>,
    ) -> Result<Option<Arc<Domain>>, SharedError> {
        // store 为 None 时只读缓存，不新建（对齐 Go 查询路径）。
        if store.is_none() {
            return Ok(self.domain.lock().unwrap().clone());
        }
        let mut g = self.domain.lock().unwrap();
        // 懒创建单例 Domain，供后续 CloseDomain 清空。
        if g.is_none() {
            *g = Some(Arc::new(Domain));
        }
        Ok(g.clone())
    }
    // Owner/Domain 启动钩子：测试中无需真实 PD 交互。
    fn StartOwnerManager(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        Ok(())
    }
    fn StartDomain(&self, _dom: &Domain) -> Result<(), SharedError> {
        Ok(())
    }
    // MDL / 统计循环钩子置空成功。
    fn InitMDLVariable(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        Ok(())
    }
    fn UpdateTableStatsLoop(&self, _dom: &Domain) -> Result<(), SharedError> {
        Ok(())
    }
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        // 计数供断言 UseOneShotSession 至少创建一次会话。
        *self.created.lock().unwrap() += 1;
        Ok(Box::new(NopSession {
            closed: Arc::clone(&self.sessions_closed),
        }))
    }
    fn CloseDomain(&self, _dom: &Domain) {
        // closeDomain=true 时必须调用；同时清空缓存避免泄漏。
        *self.closed.lock().unwrap() += 1;
        *self.domain.lock().unwrap() = None;
    }
}

/// 对照 Go gluetidb：配置副作用、过滤器、SkipLoad*、Domain 关闭与 Open。
#[test]
fn go_rust_public_contract_matches() {
    // 串行化并复位 Domain 钩子/配置位，避免并行污染。
    let _domain_state_guard = crate::test_support::domain_state_guard();

    // 正常：New() 写入全局配置位（跳过 dashboard、关慢日志、Copr 超时、SchemaLease）。
    // normal: New() config side-effects
    let g = New();
    let bits = GetGlobalConfigBits();
    // 跳过注册 Dashboard，减少 BR 侧噪音。
    assert!(bits.SkipRegisterToDashboard);
    // 关闭慢日志，避免备份 SQL 刷屏。
    assert!(!bits.EnableSlowLog);
    // Go 将 CoprReqTimeout 设为 1800s，避免长备份被默认超时打断。
    assert_eq!(bits.CoprReqTimeoutSecs, 1800);
    // SchemaLease 设为生产值，防止 PD TSO 轻微滞后卡住。
    assert!(bits.SchemaLeaseSet);
    // TiDB Glue 拥有其打开的 storage。
    assert!(g.OwnsStorage());
    // 客户端类型为 ClientCLP。
    assert_eq!(g.GetClient(), ClientCLP);
    // 版本串以 BR\n 开头，与 gluetikv 拼接 build Info 一致。
    assert!(g.GetVersion().starts_with("BR\n"));

    // 边界：系统库/BR 临时库应加载；普通 test 库不应；指定库大小写不敏感。
    // boundary: FilterLoadSysDBs / specified + BR temp
    // mysql 系统库必须加载。
    assert!(FilterLoadSysDBs(&CIStr {
        O: "mysql".into(),
        L: "mysql".into()
    }));
    // 普通业务库 test 默认不加载。
    assert!(!FilterLoadSysDBs(&CIStr {
        O: "test".into(),
        L: "test".into()
    }));
    // BR 临时库前缀按 metadef.IsBRRelatedDB 识别。
    assert!(FilterLoadSysDBs(&CIStr {
        O: "__TiDB_BR_Temporary_orders".into(),
        L: "__tidb_br_temporary_orders".into()
    }));
    let f = FilterLoadSpecifiedDBAndSysDBs(&["App".into()]);
    // 指定库按 L（小写）匹配。
    assert!(f(&CIStr {
        O: "App".into(),
        L: "app".into()
    }));
    // 系统库仍一并加载。
    assert!(f(&CIStr {
        O: "mysql".into(),
        L: "mysql".into()
    }));
    // 未指定且非系统库应过滤。
    assert!(!f(&CIStr {
        O: "other".into(),
        L: "other".into()
    }));

    // InfoSchemaFilter：无 allow 回调则不建 filter；SkipLoadDiff/Schema 规则对齐 Go。
    // filter SkipLoadDiff / SkipLoadSchema
    assert!(NewInfoSchemaFilter(None).is_none());
    let allow =
        Box::new(|name: &CIStr| name.L == "allowdb") as Box<dyn Fn(&CIStr) -> bool + Send + Sync>;
    let filter = NewInfoSchemaFilter(Some(allow)).unwrap();
    // ActionCreateSchema 始终不跳过（需加载新建 schema）。
    assert!(!filter.SkipLoadDiff(
        &SchemaDiff {
            Type: ActionType::ActionCreateSchema,
            SchemaID: 9,
            ..Default::default()
        },
        None
    ));
    // SchemaID=0 且无 InfoSchema 时不跳过。
    assert!(!filter.SkipLoadDiff(
        &SchemaDiff {
            Type: ActionType::Other,
            SchemaID: 0,
            ..Default::default()
        },
        None
    ));
    // 非 0 SchemaID、无 InfoSchema → 跳过（无法解析库名）。
    assert!(filter.SkipLoadDiff(
        &SchemaDiff {
            Type: ActionType::Other,
            SchemaID: 1,
            ..Default::default()
        },
        None
    ));
    // 有 InfoSchema 且库名在 allow 列表 → 不跳过。
    let is = MapIS {
        schemas: Mutex::new(std::collections::HashMap::from([(
            1,
            DBInfo {
                Name: CIStr {
                    O: "AllowDB".into(),
                    L: "allowdb".into(),
                },
            },
        )])),
    };
    assert!(!filter.SkipLoadDiff(
        &SchemaDiff {
            Type: ActionType::Other,
            SchemaID: 1,
            ..Default::default()
        },
        Some(&is as &dyn InfoSchema)
    ));
    // SkipLoadSchema：None 不跳过；非 allow 库名跳过。
    assert!(!filter.SkipLoadSchema(None));
    assert!(filter.SkipLoadSchema(Some(&DBInfo {
        Name: CIStr {
            O: "x".into(),
            L: "x".into()
        }
    })));

    // 资源：GetDomain 启动 + UseOneShotSession(closeDomain=true) 必须 CloseDomain。
    // resource: domain start + UseOneShotSession closeDomain
    let hooks = Arc::new(RecHooks {
        created: Mutex::new(0),
        closed: Mutex::new(0),
        sessions_closed: Arc::new(Mutex::new(0)),
        domain: Mutex::new(None),
    });
    // 注入可观测钩子后再 New/GetDomain。
    set_domain_hooks_for_test(Some(hooks.clone()));
    let g = New();
    let store = DummyStore;
    // GetDomain 应创建/缓存 Domain。
    let _dom = g.GetDomain(&store).unwrap();
    // closeDomain=true：回调结束后必须关闭 Domain。
    g.UseOneShotSession(&store, true, &mut |_se| Ok(()))
        .unwrap();
    // 至少创建过一次 session。
    assert!(*hooks.created.lock().unwrap() >= 1);
    // CloseDomain 恰好一次。
    assert_eq!(*hooks.closed.lock().unwrap(), 1);
    // Go 使用 defer se.Close()：回调成功后会话必须恰好关闭一次。
    assert_eq!(*hooks.sessions_closed.lock().unwrap(), 1);
    // 回调报错时 Go 的 defer 仍关闭会话，但 closeDomain=false 不关闭 Domain。
    let callback_error = g.UseOneShotSession(&store, false, &mut |_se| {
        Err(astersql_errors::New("callback failed"))
    });
    assert!(callback_error.is_err());
    assert_eq!(*hooks.sessions_closed.lock().unwrap(), 2);
    assert_eq!(*hooks.closed.lock().unwrap(), 1);

    // Open 委托 gluetikv 默认替身，name 等于 path。
    // Open delegates to gluetikv (no panic)
    let opened = g
        .Open("tikv://x", astersql_br_pkg_glue::SecurityOption::default())
        .unwrap();
    // 默认 Open 用 path 作为 Storage.name。
    assert_eq!(opened.name(), "tikv://x");
}

/// Go 在编译期断言 Glue 实现 glue.Glue；Rust 也必须满足同一公开接线。
#[test]
fn glue_implements_public_glue_trait() {
    fn assert_glue<T: astersql_br_pkg_glue::Glue>() {}
    assert_glue::<crate::Glue>();
}
