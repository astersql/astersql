// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! 纯 TiKV 侧 Glue，对齐 Go `br/pkg/gluetikv/glue.go`。
//! 为避开 arm64/grpc 重建，PD/TiKV Open 与 summary 采集改为可注入替身
//!（策略同 `br/pkg/glue`）；公开方法签名与 Go 保持一致，无 TiDB Domain/SQL。
//! TiKV-only Glue matching `br/pkg/gluetikv/glue.go`.
//!
//! Slimmed for arm64/grpc: PD/TiKV open and summary collection are injectable
//! stand-ins (same strategy as `br/pkg/glue`). Call shapes match Go.

use std::any::Any;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use astersql_br_pkg_glue::{
    CIStr, ClientCLP, ConsoleGlue, Context, CreateTableOption, DBInfo, Domain, Glue as GlueTrait,
    GlueClient, PolicyInfo, Progress, RefreshMetaArgs, SecurityOption, Session, SessionCtxHandle,
    StdIOGlue, Storage, TableInfo, TableMode,
};
use astersql_br_pkg_version_build::Info as BuildInfo;
use astersql_config::{get_global_config, store_global_config};
use astersql_errors::{New, SharedError};

/// 只访问 TiKV、不依赖 TiDB SQL 层的 `glue.Glue` 实现。
/// Glue is an implementation of glue.Glue that accesses only TiKV without TiDB.
#[derive(Default, Clone, Copy)]
pub struct Glue {
    pub StdIOGlue: StdIOGlue,
}

impl Glue {
    /// 构造默认 TiKV Glue（控制台 IO 走 StdIOGlue）。
    pub fn new() -> Self {
        Self { StdIOGlue }
    }
}

/// Open 边界的可注入函数类型，便于单测替换真实 PD/TiKV 打开逻辑。
/// OpenFn mirrors the PD/TiKV open boundary for tests.
pub type OpenFn =
    Arc<dyn Fn(&str, SecurityOption) -> Result<Box<dyn Storage>, SharedError> + Send + Sync>;

// 进程级 Open 钩子槽；OnceLock 保证懒初始化一次。
static OPEN_HOOK: OnceLock<Mutex<Option<OpenFn>>> = OnceLock::new();

// 取得钩子互斥槽；首次访问时创建空 Option。
fn open_hook_slot() -> &'static Mutex<Option<OpenFn>> {
    OPEN_HOOK.get_or_init(|| Mutex::new(None))
}

/// 安装测试用 opener；`None` 恢复默认替身 Open。
/// Install a test opener (None restores default stand-in open).
pub fn set_open_hook_for_test(hook: Option<OpenFn>) {
    *open_hook_slot().lock().expect("open hook lock") = hook;
}

/// 记录 [`Glue::Record`] 的 `(name, unit_count, value)`，对应
/// `summary.CollectSuccessUnit(name, 1, val)`。
/// Recorded `(name, unit_count, value)` from [`Glue::Record`], mirroring
/// `summary.CollectSuccessUnit(name, 1, val)`.
static RECORDS: OnceLock<Mutex<Vec<(String, i32, u64)>>> = OnceLock::new();

// Record 缓冲槽；供 take_records_for_test 与 Record 共用。
fn records_slot() -> &'static Mutex<Vec<(String, i32, u64)>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 测试辅助：取出并清空已记录的 CollectSuccessUnit 等价调用。
/// Test helper: drain recorded CollectSuccessUnit-equivalent calls.
pub fn take_records_for_test() -> Vec<(String, i32, u64)> {
    std::mem::take(&mut *records_slot().lock().expect("records lock"))
}

// 以 path 字符串作为 name 的轻量 Storage，供默认 Open 返回。
struct NamedStorage {
    name: String,
}

impl Storage for NamedStorage {
    fn name(&self) -> &str {
        &self.name
    }
}

fn default_open(path: &str, _option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
    validate_tikv_path(path)?;
    // 在 TiKVDriver/grpc arm64 健康前使用 path 命名的替身 Storage。
    // Stand-in until TiKVDriver/grpc arm64 rebuild is healthy.
    Ok(Box::new(NamedStorage {
        name: path.to_string(),
    }))
}

/// Mirror the synchronous validation performed by `TiKVDriver::Open` before it
/// reaches the external PD/TiKV boundary.
fn validate_tikv_path(path: &str) -> Result<(), SharedError> {
    let rest = path
        .strip_prefix("tikv://")
        .ok_or_else(|| New("invalid TiKV path: path must start with tikv://"))?;
    let (authority_and_path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let has_pd_address = authority_and_path
        .split('/')
        .next()
        .unwrap_or_default()
        .split(',')
        .any(|address| !address.is_empty());
    if !has_pd_address {
        return Err(New("invalid TiKV path: PD address is empty"));
    }
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "disableGC" && value.parse::<bool>().is_err() {
            return Err(New(format!(
                "invalid TiKV path: invalid disableGC value {value}"
            )));
        }
    }
    Ok(())
}

// 本地 Progress：满足 Inc/Close 契约，不拉 utils/grpc 依赖。
struct CounterProgress {
    current: AtomicI64,
}

impl Progress for CounterProgress {
    // 步进 1，等价于 Go progress.Inc。
    fn Inc(&self) {
        self.IncBy(1);
    }
    // 按 cnt 累加；Relaxed 足够单测读计数。
    fn IncBy(&self, cnt: i64) {
        self.current.fetch_add(cnt, Ordering::Relaxed);
    }
    fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::Relaxed)
    }
    // Close 无状态清理；与 Go utils.StartProgress 的可关闭句柄对齐。
    fn Close(&self) {}
}

/// CreateSession 在 Go 返回 `(nil, nil)` 时的 Session 替身——调用方法即报错。
/// Session stand-in for Go's `(nil, nil)` CreateSession — methods error if used.
struct NilSession;

impl Session for NilSession {
    // 下列方法均报 session nil，防止误用 SQL 路径。
    fn Execute(&mut self, _ctx: Context, _sql: &str) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    fn ExecuteInternal(
        &mut self,
        _ctx: Context,
        _sql: &str,
        _args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    // DDL/变量类接口同样拒绝：TiKV Glue 不提供 session。
    fn CreateDatabaseOnExistError(
        &mut self,
        _ctx: Context,
        _schema: &DBInfo,
    ) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    fn CreateTable(
        &mut self,
        _ctx: Context,
        _dbName: CIStr,
        _table: &TableInfo,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    fn CreatePlacementPolicy(
        &mut self,
        _ctx: Context,
        _policy: &PolicyInfo,
    ) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    // Close 对 nil session 为空操作。
    fn Close(&mut self) {}
    fn GetGlobalVariable(&mut self, _name: &str) -> Result<String, SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    fn GetGlobalSysVar(&mut self, _name: &str) -> Result<String, SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    // 仍返回占位句柄，避免调用方因 Option 缺失而无法编译。
    fn GetSessionCtx(&mut self) -> SessionCtxHandle {
        Arc::new(()) as SessionCtxHandle
    }
    fn AlterTableMode(
        &mut self,
        _ctx: Context,
        _schemaID: i64,
        _tableID: i64,
        _tableMode: TableMode,
    ) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
    fn RefreshMeta(&mut self, _ctx: Context, _args: &RefreshMetaArgs) -> Result<(), SharedError> {
        Err(New("gluetikv: session is nil"))
    }
}

impl GlueTrait for Glue {
    // GetDomain：纯 TiKV 模式无 TiDB domain（Go: nil, nil）。
    // GetDomain implements glue.Glue — TiKV-only mode has no TiDB domain (Go: nil, nil).
    fn GetDomain(&self, _store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        Ok(Arc::new(Domain))
    }

    // CreateSession：无 SQL session，返回 NilSession（Go: nil, nil）。
    // CreateSession implements glue.Glue — no SQL session (Go: nil, nil).
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        Ok(Box::new(NilSession))
    }

    // Open：若带 CA 则写入全局 TLS，再走钩子或默认替身。
    // Open implements glue.Glue.
    fn Open(&self, path: &str, option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
        // 与 Go 一致：仅当 CAPath 非空时同步 cluster SSL 三件套到全局配置。
        if !option.CAPath.is_empty() {
            let mut conf = (*get_global_config()).clone();
            conf.security.cluster_ssl_ca = option.CAPath.clone();
            conf.security.cluster_ssl_cert = option.CertPath.clone();
            conf.security.cluster_ssl_key = option.KeyPath.clone();
            store_global_config(conf);
        }
        // 测试钩子优先，便于注入失败/自定义 Storage。
        if let Some(hook) = open_hook_slot().lock().expect("open hook lock").clone() {
            return hook(path, option);
        }
        default_open(path, option)
    }

    // OwnsStorage：TiKV Glue 拥有打开的 storage。
    fn OwnsStorage(&self) -> bool {
        true
    }

    // StartProgress：Go 调 utils.StartProgress；utils 经 logutil 拉 grpc，
    // 故用本地 CounterProgress 满足 Inc/Close 契约。
    // StartProgress implements glue.Glue.
    // Go: utils.StartProgress(ctx, cmdName, total, redirectLog, nil).
    // utils pulls grpc via logutil; local Progress matches Inc/Close contract.
    fn StartProgress(
        &self,
        _ctx: Context,
        _cmdName: &str,
        _total: i64,
        _redirectLog: bool,
    ) -> Box<dyn Progress> {
        Box::new(CounterProgress {
            current: AtomicI64::new(0),
        })
    }

    // Record：对齐 Go `summary.CollectSuccessUnit(name, 1, val)`，写入可观测缓冲。
    // Record implements glue.Glue — Go: summary.CollectSuccessUnit(name, 1, val).
    fn Record(&self, name: &str, value: u64) {
        records_slot()
            .lock()
            .expect("records lock")
            .push((name.to_string(), 1, value));
    }

    // GetVersion：前缀 BR + 换行 + build Info，供版本正则断言。
    fn GetVersion(&self) -> String {
        format!("BR\n{}", BuildInfo())
    }

    // UseOneShotSession：纯 TiKV 不创建 session，也不调用 fn（Go: 直接成功返回）。
    // UseOneShotSession — TiKV-only returns nil without invoking fn.
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _closeDomain: bool,
        _fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        Ok(())
    }

    // GetClient：返回 ClientCLP。
    fn GetClient(&self) -> GlueClient {
        ClientCLP
    }

    // AsConsoleGlue：暴露内嵌 StdIOGlue，供控制台交互路径使用。
    fn AsConsoleGlue(&self) -> Option<Arc<dyn ConsoleGlue>> {
        Some(Arc::new(self.StdIOGlue))
    }
}
