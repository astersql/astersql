// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! TiDB glue matching `br/pkg/gluetidb/glue.go`.
//!
//! TiDB 侧 Glue：在 BR CLI 路径上装配 Domain/Session，并委托 gluetikv 做 Open/进度。
//! Domain 真实能力通过 DomainHooks 注入，便于无完整 TiDB 运行时的单测替换。
//! New() 会写入全局配置位（跳过 dashboard、关闭慢日志、拉长 Copr 超时等）。

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use astersql_br_pkg_glue::{
    CIStr, ClientCLP, Context, Domain, Glue as GlueTrait, GlueClient, Progress, SecurityOption,
    Session, StdIOGlue, Storage,
};
use astersql_br_pkg_gluetikv::Glue as TikvGlue;
use astersql_errors::SharedError;

use crate::infoschema_filter::Filter;

// BR 发出的 SQL 注释标记，便于审计与过滤。
pub const brComment: &str = "/*from(br)*/";
// 临时库名前缀；恢复过程中 BR 相关库需始终加载。
const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_";
// 系统库名（小写比较用）。
const SystemDB: &str = "mysql";

/// 进程级配置位：对齐 Go 全局变量副作用，供测试与启动路径读取。
#[derive(Clone, Debug, Default)]
pub struct GlobalConfigBits {
    pub SkipRegisterToDashboard: bool,
    pub EnableSlowLog: bool,
    // Coprocessor 请求超时（秒）；New 默认 1800。
    pub CoprReqTimeoutSecs: u64,
    pub SchemaLeaseSet: bool,
}

// OnceLock 持有全局位，避免多处静态可变。
fn global_bits() -> &'static Mutex<GlobalConfigBits> {
    static G: OnceLock<Mutex<GlobalConfigBits>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(GlobalConfigBits::default()))
}

/// 克隆当前全局配置位快照。
pub fn GetGlobalConfigBits() -> GlobalConfigBits {
    global_bits().lock().unwrap().clone()
}

/// 测试复位：清空 New() 留下的副作用。
pub fn reset_global_config_bits_for_test() {
    *global_bits().lock().unwrap() = GlobalConfigBits::default();
}

// 小写库名是否为系统库 mysql。
fn IsSystemDB(dbLowerName: &str) -> bool {
    dbLowerName == SystemDB
}

// 原始库名是否为 BR 临时库。
fn IsBRRelatedDB(dbOriginName: &str) -> bool {
    dbOriginName.starts_with(temporaryDBNamePrefix)
}

/// infoschema 加载过滤器：系统库与 BR 临时库必须加载。
pub fn FilterLoadSysDBs(name: &CIStr) -> bool {
    // L 比系统库，O 比临时前缀（大小写敏感前缀与 Go 一致）。
    IsSystemDB(&name.L) || IsBRRelatedDB(&name.O)
}

/// 在系统/临时库之外，再允许一组指定库名（小写集合）。
pub fn FilterLoadSpecifiedDBAndSysDBs(
    extraDBNames: &[String],
) -> impl Fn(&CIStr) -> bool + Send + Sync + 'static {
    let mut dbNameSet = HashSet::new();
    for name in extraDBNames {
        // 统一小写，与 CIStr.L 比较。
        dbNameSet.insert(name.to_lowercase());
    }
    move |name: &CIStr| dbNameSet.contains(&name.L) || IsSystemDB(&name.L) || IsBRRelatedDB(&name.O)
}

/// Domain 生命周期钩子：真实 TiDB 或测试替身均实现此 trait。
pub trait DomainHooks: Send + Sync {
    // store=None 时只查询已有 Domain；Some 时按需创建。
    fn GetOrCreateDomainWithFilter(
        &self,
        store: Option<&dyn Storage>,
        filter: Option<&dyn Filter>,
    ) -> Result<Option<Arc<Domain>>, SharedError>;
    fn StartOwnerManager(&self, store: &dyn Storage) -> Result<(), SharedError>;
    fn StartDomain(&self, dom: &Domain) -> Result<(), SharedError>;
    fn InitMDLVariable(&self, store: &dyn Storage) -> Result<(), SharedError>;
    fn UpdateTableStatsLoop(&self, dom: &Domain) -> Result<(), SharedError>;
    fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>, SharedError>;
    fn CloseDomain(&self, dom: &Domain);
}

/// 默认空钩子：除 CreateSession 报错外其余成功/空操作。
#[derive(Default)]
struct NopDomainHooks;

impl DomainHooks for NopDomainHooks {
    fn GetOrCreateDomainWithFilter(
        &self,
        _store: Option<&dyn Storage>,
        _filter: Option<&dyn Filter>,
    ) -> Result<Option<Arc<Domain>>, SharedError> {
        // 未注入真实 domain 时返回 None，触发 startDomainAsNeeded。
        Ok(None)
    }
    fn StartOwnerManager(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        Ok(())
    }
    fn StartDomain(&self, _dom: &Domain) -> Result<(), SharedError> {
        Ok(())
    }
    fn InitMDLVariable(&self, _store: &dyn Storage) -> Result<(), SharedError> {
        Ok(())
    }
    fn UpdateTableStatsLoop(&self, _dom: &Domain) -> Result<(), SharedError> {
        Ok(())
    }
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        // 强制配置 hooks，避免静默拿到不可用会话。
        Err(astersql_errors::New(
            "domain hooks: CreateSession not configured",
        ))
    }
    fn CloseDomain(&self, _dom: &Domain) {}
}

fn default_hooks() -> Arc<dyn DomainHooks> {
    static H: OnceLock<Arc<dyn DomainHooks>> = OnceLock::new();
    H.get_or_init(|| Arc::new(NopDomainHooks) as Arc<dyn DomainHooks>)
        .clone()
}

// 测试可覆盖的 hooks；生产路径保持 None。
static OVERRIDE_HOOKS: Mutex<Option<Arc<dyn DomainHooks>>> = Mutex::new(None);

/// 单测注入/清除 DomainHooks。
pub fn set_domain_hooks_for_test(hooks: Option<Arc<dyn DomainHooks>>) {
    *OVERRIDE_HOOKS.lock().unwrap() = hooks;
}

// 优先测试覆盖，否则默认 Nop。
fn active_hooks() -> Arc<dyn DomainHooks> {
    OVERRIDE_HOOKS
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(default_hooks)
}

/// TiDB Glue：组合 StdIO、TikvGlue、Domain 启动互斥与可选 infoschema 过滤。
pub struct Glue {
    pub StdIOGlue: StdIOGlue,
    // Open/进度/版本委托给 gluetikv。
    tikvGlue: TikvGlue,
    // 串行化 startDomainAsNeeded，避免并发双启 Domain。
    startDomainMu: Mutex<()>,
    pub InfoSchemaFilter: Option<Arc<dyn Filter>>,
}

/// Shared handle passed to a one-shot callback. The owning guard keeps the
/// underlying session so it can enforce Go's deferred `se.Close()` even when
/// the callback retains its handle.
struct OneShotSession {
    inner: Arc<Mutex<Option<Box<dyn Session>>>>,
}

impl OneShotSession {
    fn with_inner<T>(&self, f: impl FnOnce(&mut dyn Session) -> T) -> T {
        let mut inner = self.inner.lock().unwrap();
        f(inner
            .as_deref_mut()
            .expect("one-shot session already closed"))
    }
}

impl Session for OneShotSession {
    fn Execute(&mut self, ctx: Context, sql: &str) -> Result<(), SharedError> {
        self.with_inner(|se| se.Execute(ctx, sql))
    }

    fn ExecuteInternal(
        &mut self,
        ctx: Context,
        sql: &str,
        args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.ExecuteInternal(ctx, sql, args))
    }

    fn CreateDatabaseOnExistError(
        &mut self,
        ctx: Context,
        schema: &astersql_br_pkg_glue::DBInfo,
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.CreateDatabaseOnExistError(ctx, schema))
    }

    fn CreateTable(
        &mut self,
        ctx: Context,
        dbName: CIStr,
        table: &astersql_br_pkg_glue::TableInfo,
        cs: Vec<astersql_br_pkg_glue::CreateTableOption>,
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.CreateTable(ctx, dbName, table, cs))
    }

    fn CreatePlacementPolicy(
        &mut self,
        ctx: Context,
        policy: &astersql_br_pkg_glue::PolicyInfo,
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.CreatePlacementPolicy(ctx, policy))
    }

    fn Close(&mut self) {
        self.with_inner(|se| se.Close());
    }

    fn GetGlobalVariable(&mut self, name: &str) -> Result<String, SharedError> {
        self.with_inner(|se| se.GetGlobalVariable(name))
    }

    fn GetGlobalSysVar(&mut self, name: &str) -> Result<String, SharedError> {
        self.with_inner(|se| se.GetGlobalSysVar(name))
    }

    fn GetSessionCtx(&mut self) -> astersql_br_pkg_glue::SessionCtxHandle {
        self.with_inner(|se| se.GetSessionCtx())
    }

    fn AlterTableMode(
        &mut self,
        ctx: Context,
        schemaID: i64,
        tableID: i64,
        tableMode: astersql_br_pkg_glue::TableMode,
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.AlterTableMode(ctx, schemaID, tableID, tableMode))
    }

    fn RefreshMeta(
        &mut self,
        ctx: Context,
        args: &astersql_br_pkg_glue::RefreshMetaArgs,
    ) -> Result<(), SharedError> {
        self.with_inner(|se| se.RefreshMeta(ctx, args))
    }
}

struct OneShotSessionGuard(Arc<Mutex<Option<Box<dyn Session>>>>);

impl Drop for OneShotSessionGuard {
    fn drop(&mut self) {
        if let Some(mut se) = self.0.lock().unwrap().take() {
            se.Close();
        }
    }
}

struct OneShotDomainGuard {
    domain: Option<Arc<Domain>>,
    hooks: Arc<dyn DomainHooks>,
}

impl Drop for OneShotDomainGuard {
    fn drop(&mut self) {
        if let Some(dom) = self.domain.take() {
            self.hooks.CloseDomain(&dom);
        }
    }
}

/// 构造 Glue 并设置 BR 进程级默认配置位。
pub fn New() -> Glue {
    {
        let mut bits = global_bits().lock().unwrap();
        // 与 Go New 副作用一致：租约、跳过 dashboard、关慢日志、Copr 30min。
        bits.SchemaLeaseSet = true;
        bits.SkipRegisterToDashboard = true;
        bits.EnableSlowLog = false;
        bits.CoprReqTimeoutSecs = 1800;
    }
    Glue {
        StdIOGlue,
        tikvGlue: TikvGlue::new(),
        startDomainMu: Mutex::new(()),
        InfoSchemaFilter: None,
    }
}

impl Glue {
    // 经 hooks 取 Domain，并传入当前 InfoSchemaFilter。
    fn getDomainInner(
        &self,
        store: Option<&dyn Storage>,
    ) -> Result<Option<Arc<Domain>>, SharedError> {
        active_hooks().GetOrCreateDomainWithFilter(store, self.InfoSchemaFilter.as_deref())
    }

    // 若尚无 Domain：加锁后 StartOwnerManager → 创建 → StartDomain。
    fn startDomainAsNeeded(&self, store: &dyn Storage) -> Result<(), SharedError> {
        let _guard = self.startDomainMu.lock().unwrap();
        // 双重检查：持锁后再查，防止并发重复启动。
        if self.getDomainInner(None)?.is_some() {
            return Ok(());
        }
        active_hooks().StartOwnerManager(store)?;
        let dom = self
            .getDomainInner(Some(store))?
            .ok_or_else(|| astersql_errors::New("failed to create domain"))?;
        active_hooks().StartDomain(&dom)
    }

    /// 确保 Domain 已启动；首次创建时初始化 MDL 与表统计循环。
    pub fn GetDomain(&self, store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        // 记录启动前是否已有 Domain，决定是否跑一次性初始化。
        let existDom = self.getDomainInner(None)?;
        self.startDomainAsNeeded(store)?;
        let dom = self
            .getDomainInner(Some(store))?
            .ok_or_else(|| astersql_errors::New("domain missing after start"))?;
        if existDom.is_none() {
            // 仅首次：InitMDL + UpdateTableStatsLoop，对齐 Go。
            active_hooks().InitMDLVariable(store)?;
            active_hooks().UpdateTableStatsLoop(&dom)?;
        }
        Ok(dom)
    }

    /// 先确保 Domain，再经 hooks 创建会话。
    pub fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        self.startDomainAsNeeded(store)?;
        active_hooks().CreateSession(store)
    }

    /// 存储打开委托 tikvGlue（PD 路径）。
    pub fn Open(
        &self,
        path: &str,
        option: SecurityOption,
    ) -> Result<Box<dyn Storage>, SharedError> {
        self.tikvGlue.Open(path, option)
    }

    /// CLI Glue 拥有 Open 返回的存储。
    pub fn OwnsStorage(&self) -> bool {
        true
    }

    /// 进度条委托 tikvGlue。
    pub fn StartProgress(
        &self,
        ctx: Context,
        cmdName: &str,
        total: i64,
        redirectLog: bool,
    ) -> Box<dyn Progress> {
        self.tikvGlue
            .StartProgress(ctx, cmdName, total, redirectLog)
    }

    pub fn Record(&self, name: &str, value: u64) {
        self.tikvGlue.Record(name, value);
    }

    pub fn GetVersion(&self) -> String {
        self.tikvGlue.GetVersion()
    }

    /// 一次性会话：执行回调后可按需 CloseDomain。
    pub fn UseOneShotSession(
        &self,
        store: &dyn Storage,
        closeDomain: bool,
        fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        let se = self.CreateSession(store)?;
        let shared_session = Arc::new(Mutex::new(Some(se)));
        let _session_guard = OneShotSessionGuard(Arc::clone(&shared_session));
        let dom = self.getDomainInner(Some(store))?;
        // 会话前初始化 MDL，与 Go UseOneShotSession 顺序一致。
        active_hooks().InitMDLVariable(store)?;
        let _domain_guard = OneShotDomainGuard {
            domain: closeDomain.then_some(dom).flatten(),
            hooks: active_hooks(),
        };
        let err = fn_(Box::new(OneShotSession {
            inner: Arc::clone(&shared_session),
        }));
        err
    }

    /// BR CLI 入口恒为 ClientCLP。
    pub fn GetClient(&self) -> GlueClient {
        ClientCLP
    }
}

impl GlueTrait for Glue {
    fn GetDomain(&self, store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        Glue::GetDomain(self, store)
    }

    fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        Glue::CreateSession(self, store)
    }

    fn Open(&self, path: &str, option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
        Glue::Open(self, path, option)
    }

    fn OwnsStorage(&self) -> bool {
        Glue::OwnsStorage(self)
    }

    fn StartProgress(
        &self,
        ctx: Context,
        cmdName: &str,
        total: i64,
        redirectLog: bool,
    ) -> Box<dyn Progress> {
        Glue::StartProgress(self, ctx, cmdName, total, redirectLog)
    }

    fn Record(&self, name: &str, value: u64) {
        Glue::Record(self, name, value)
    }

    fn GetVersion(&self) -> String {
        Glue::GetVersion(self)
    }

    fn UseOneShotSession(
        &self,
        store: &dyn Storage,
        closeDomain: bool,
        fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        Glue::UseOneShotSession(self, store, closeDomain, fn_)
    }

    fn GetClient(&self) -> GlueClient {
        Glue::GetClient(self)
    }
}
