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

//! restore 包本地桩：PD/TiKV/conn/domain/storage/summary/checkpoint 边界。
//! darwin 安全；无 kvproto/grpcio/kv/domain 真实依赖。
//! 仅提供编译与单测所需的最小类型/函数，未实现生产 RPC。
//! 错误与重试语义对齐 Go 测试替身，便于 restore 路径联调。
//! Local stand-ins for PD / TiKV / conn / domain / storage / summary / checkpoint
//! boundaries (darwin-safe; no kvproto/grpcio/kv/domain).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// `Result`：类型别名。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// `Error`：结构体定义，字段语义见成员注释。
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

// impl Error {：方法实现见下。
impl Error {
    /// `new`：见函数体控制流与错误处理。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    /// `with_code`：见函数体控制流与错误处理。
    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }

    /// `Trace`：见函数体控制流与错误处理。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// `Annotate`：见函数体控制流与错误处理。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            code: err.code,
        }
    }

    /// `Annotatef`：见函数体控制流与错误处理。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }

    /// `Errorf`：见函数体控制流与错误处理。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
}

// impl fmt::Display for Error {：方法实现见下。
impl fmt::Display for Error {
    // `fmt`：见函数体控制流与错误处理。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

// impl std::error::Error for Error {}：方法实现见下。
impl std::error::Error for Error {}

/// `berrors` 子模块：桩或辅助实现。
pub mod berrors {
    use super::Error;

    /// `ErrRestoreNotFreshCluster`：见函数体控制流与错误处理。
    pub fn ErrRestoreNotFreshCluster(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrRestoreNotFreshCluster", msg)
    }
}

/// Cancellation token approximating Go `context.Context`.
/// （中文）Cancellation token approximating Go `context.Context`.
#[derive(Clone, Default)]
/// `Context`：结构体定义，字段语义见成员注释。
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
    parent: Option<Arc<Context>>,
    deadline: Option<Instant>,
}

// impl Context {：方法实现见下。
impl Context {
    /// `Background`：见函数体控制流与错误处理。
    pub fn Background() -> Self {
        Self::default()
    }

    /// `WithCancel`：见函数体控制流与错误处理。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
            deadline: None,
        };
        let cancel = CancelFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// `WithTimeout`：见函数体控制流与错误处理。
    pub fn WithTimeout(parent: &Self, timeout: Duration) -> (Self, CancelFunc) {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let child = Self {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
            deadline: Some(deadline),
        };
        let cancel = CancelFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// `cancel`：见函数体控制流与错误处理。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// `Err`：见函数体控制流与错误处理。
    pub fn Err(&self) -> Option<Error> {
        if let Some(err) = self.cancelled.lock().unwrap().clone() {
            return Some(err);
        }
        if let Some(err) = self.parent.as_ref().and_then(|parent| parent.Err()) {
            return Some(err);
        }
        self.deadline
            .filter(|deadline| Instant::now() >= *deadline)
            .map(|_| Error::new("context deadline exceeded"))
    }

    /// `Done`：见函数体控制流与错误处理。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

#[derive(Clone)]
/// `CancelFunc`：结构体定义，字段语义见成员注释。
pub struct CancelFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

// impl CancelFunc {：方法实现见下。
impl CancelFunc {
    /// `call`：见函数体控制流与错误处理。
    pub fn call(&self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }
}

/// `Key`：类型别名。
pub type Key = Vec<u8>;

/// `PhysicalShiftBits`：与 Go 同名常量/静态对齐。
pub const PhysicalShiftBits: i64 = 18;

/// `ComposeTS`：见函数体控制流与错误处理。
pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
    ((physical as u64) << (PhysicalShiftBits as u64)) | (logical as u64)
}

/// `import_sstpb` 子模块：桩或辅助实现。
pub mod import_sstpb {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(i32)]
    /// `SwitchMode`：枚举变体与 Go 对齐。
    pub enum SwitchMode {
        Normal = 0,
        Import = 1,
    }
}

/// `metapb` 子模块：桩或辅助实现。
pub mod metapb {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Store`：结构体定义，字段语义见成员注释。
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub Labels: Vec<(String, String)>,
    }

    // impl Store {：方法实现见下。
    impl Store {
        /// `GetAddress`：见函数体控制流与错误处理。
        pub fn GetAddress(&self) -> &str {
            &self.Address
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Region`：结构体定义，字段语义见成员注释。
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }
}

#[derive(Clone, Debug, Default)]
/// `RegionInfo`：结构体定义，字段语义见成员注释。
pub struct RegionInfo {
    pub Region: Option<metapb::Region>,
    pub Leader: Option<()>,
}

/// `PdClient`：接口边界，实现见 impl。
pub trait PdClient: Send + Sync {
    // `GetTS`：见函数体控制流与错误处理。
    fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)>;
    // `GetAllStores`：见函数体控制流与错误处理。
    fn GetAllStores(&self, ctx: &Context) -> Result<Vec<metapb::Store>>;
}

/// Minimal region scan client used by `regionScanner` (Go `split.SplitClient`).
/// （中文）Minimal region scan client used by `regionScanner` (Go `split.SplitClient`).
pub trait SplitClient: Send + Sync {
    // `ScanRegions`：见函数体控制流与错误处理。
    fn ScanRegions(
        &self,
        ctx: &Context,
        key: &[u8],
        end_key: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>>;
}

/// 带重试的 region 扫描。
pub fn ScanRegionsWithRetry(
    ctx: &Context,
    client: &dyn SplitClient,
    start_key: &[u8],
    end_key: &[u8],
    limit: i32,
) -> Result<Vec<RegionInfo>> {
    if !end_key.is_empty() && start_key > end_key {
        // 条件分支：见块内处理与 Go 对齐点。
        return Err(Error::new(format!(
            // 错误路径：向上返回，保留上下文。
            "startKey > endKey, startKey: {:?}, endkey: {:?}",
            start_key, end_key
        )));
    }
    let mut last_err = None;
    for _ in 0..3 {
        if let Some(err) = ctx.Err() {
            // 条件分支：见块内处理与 Go 对齐点。
            return Err(err);
            // 错误路径：向上返回，保留上下文。
        }
        match client.ScanRegions(ctx, start_key, end_key, limit) {
            // 分支匹配：各臂处理不同结果/错误。
            Ok(regions) => return Ok(regions),
            Err(err) => last_err = Some(err),
            // 错误路径：向上返回，保留上下文。
        }
    }
    Err(last_err.unwrap_or_else(|| Error::new("scan regions failed")))
    // 错误路径：向上返回，保留上下文。
}

/// `ImportSstSwitcher`：接口边界，实现见 impl。
pub trait ImportSstSwitcher: Send + Sync {
    // `SwitchMode`：见函数体控制流与错误处理。
    fn SwitchMode(&self, ctx: &Context, addr: &str, mode: import_sstpb::SwitchMode) -> Result<()>;
}

#[derive(Clone, Default)]
/// `RecordingImportSstSwitcher`：结构体定义，字段语义见成员注释。
pub struct RecordingImportSstSwitcher {
    pub calls: Arc<Mutex<Vec<(String, import_sstpb::SwitchMode)>>>,
    pub fail: Arc<AtomicBool>,
}

// impl RecordingImportSstSwitcher {：方法实现见下。
impl RecordingImportSstSwitcher {
    /// `new`：见函数体控制流与错误处理。
    pub fn new() -> Self {
        Self::default()
    }
}

// impl ImportSstSwitcher for RecordingImportSstSwitcher {：方法实现见下。
impl ImportSstSwitcher for RecordingImportSstSwitcher {
    // `SwitchMode`：见函数体控制流与错误处理。
    fn SwitchMode(&self, _ctx: &Context, addr: &str, mode: import_sstpb::SwitchMode) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            // 条件分支：见块内处理与 Go 对齐点。
            return Err(Error::new("switch mode failed"));
            // 错误路径：向上返回，保留上下文。
        }
        self.calls.lock().unwrap().push((addr.to_string(), mode));
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// `ClusterConfig`：结构体定义，字段语义见成员注释。
pub struct ClusterConfig {
    pub Schedulers: Vec<String>,
    pub RuleID: String,
}

/// `UndoFunc`：类型别名。
pub type UndoFunc = Arc<dyn Fn(&Context) -> Result<()> + Send + Sync>;

/// `Nop`：见函数体控制流与错误处理。
pub fn Nop(_ctx: &Context) -> Result<()> {
    Ok(())
}

/// `nop_undo`：见函数体控制流与错误处理。
pub fn nop_undo() -> UndoFunc {
    Arc::new(|_ctx| Ok(()))
}

/// `ConnMgr`：接口边界，实现见 impl。
pub trait ConnMgr: Send + Sync {
    // `RemoveSchedulersWithConfig`：见函数体控制流与错误处理。
    fn RemoveSchedulersWithConfig(
        &self,
        ctx: &Context,
    ) -> Result<(UndoFunc, Option<ClusterConfig>)>;
    // `GetOriginPDConfig`：见函数体控制流与错误处理。
    fn GetOriginPDConfig(&self, ctx: &Context) -> Result<ClusterConfig>;
    // `RemoveSchedulersOnRegion`：见函数体控制流与错误处理。
    fn RemoveSchedulersOnRegion(
        &self,
        ctx: &Context,
        key_range: &[[Key; 2]],
    ) -> Result<(String, Arc<dyn Fn() + Send + Sync>)>;
    // `MakeFineGrainedUndoFunction`：见函数体控制流与错误处理。
    fn MakeFineGrainedUndoFunction(
        &self,
        cfg: ClusterConfig,
        wait_pause: Arc<dyn Fn() + Send + Sync>,
    ) -> UndoFunc;
}

#[derive(Default)]
/// `MemConnMgr`：结构体定义，字段语义见成员注释。
pub struct MemConnMgr {
    pub remove_called: AtomicBool,
    pub origin: Mutex<ClusterConfig>,
}

// impl ConnMgr for MemConnMgr {：方法实现见下。
impl ConnMgr for MemConnMgr {
    // `RemoveSchedulersWithConfig`：见函数体控制流与错误处理。
    fn RemoveSchedulersWithConfig(
        &self,
        _ctx: &Context,
    ) -> Result<(UndoFunc, Option<ClusterConfig>)> {
        self.remove_called.store(true, Ordering::SeqCst);
        let cfg = self.origin.lock().unwrap().clone();
        Ok((nop_undo(), Some(cfg)))
    }

    // `GetOriginPDConfig`：见函数体控制流与错误处理。
    fn GetOriginPDConfig(&self, _ctx: &Context) -> Result<ClusterConfig> {
        Ok(self.origin.lock().unwrap().clone())
    }

    // `RemoveSchedulersOnRegion`：见函数体控制流与错误处理。
    fn RemoveSchedulersOnRegion(
        &self,
        _ctx: &Context,
        _key_range: &[[Key; 2]],
    ) -> Result<(String, Arc<dyn Fn() + Send + Sync>)> {
        Ok((
            "rule-1".into(),
            Arc::new(|| {}) as Arc<dyn Fn() + Send + Sync>,
        ))
    }

    // `MakeFineGrainedUndoFunction`：见函数体控制流与错误处理。
    fn MakeFineGrainedUndoFunction(
        &self,
        _cfg: ClusterConfig,
        wait_pause: Arc<dyn Fn() + Send + Sync>,
    ) -> UndoFunc {
        Arc::new(move |_ctx| {
            wait_pause();
            Ok(())
        })
    }
}

#[derive(Clone, Debug, Default)]
/// `WalkOption`：结构体定义，字段语义见成员注释。
pub struct WalkOption {
    pub SubDir: String,
}

/// `Storage`：接口边界，实现见 impl。
pub trait Storage: Send + Sync {
    // `WalkDir`：见函数体控制流与错误处理。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    // `ReadFile`：见函数体控制流与错误处理。
    fn ReadFile(&self, ctx: &Context, path: &str) -> Result<Vec<u8>>;
    // `WriteFile`：见函数体控制流与错误处理。
    fn WriteFile(&self, ctx: &Context, path: &str, data: &[u8]) -> Result<()>;
    // `DeleteFile`：见函数体控制流与错误处理。
    fn DeleteFile(&self, ctx: &Context, path: &str) -> Result<()>;
}

#[derive(Default)]
/// `MemStorage`：结构体定义，字段语义见成员注释。
pub struct MemStorage {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

// impl MemStorage {：方法实现见下。
impl MemStorage {
    /// `new`：见函数体控制流与错误处理。
    pub fn new() -> Self {
        Self::default()
    }
}

// impl Storage for MemStorage {：方法实现见下。
impl Storage for MemStorage {
    // `WalkDir`：见函数体控制流与错误处理。
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        let files = self.files.lock().unwrap();
        let mut paths: Vec<_> = files
            .keys()
            .filter(|p| opt.SubDir.is_empty() || p.starts_with(&opt.SubDir))
            .cloned()
            .collect();
        paths.sort();
        for path in paths {
            let size = files.get(&path).map(|d| d.len() as i64).unwrap_or(0);
            f(&path, size)?;
        }
        Ok(())
    }

    // `ReadFile`：见函数体控制流与错误处理。
    fn ReadFile(&self, _ctx: &Context, path: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {path}")))
    }

    // `WriteFile`：见函数体控制流与错误处理。
    fn WriteFile(&self, _ctx: &Context, path: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }

    // `DeleteFile`：见函数体控制流与错误处理。
    fn DeleteFile(&self, _ctx: &Context, path: &str) -> Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `CIStr`：结构体定义，字段语义见成员注释。
pub struct CIStr {
    pub O: String,
    pub L: String,
}

// impl CIStr {：方法实现见下。
impl CIStr {
    /// `new`：见函数体控制流与错误处理。
    pub fn new(name: impl Into<String>) -> Self {
        let O = name.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

#[derive(Clone, Debug, Default)]
/// `ColumnInfo`：结构体定义，字段语义见成员注释。
pub struct ColumnInfo {
    pub Name: CIStr,
}

#[derive(Clone, Debug, Default)]
/// `TableInfo`：结构体定义，字段语义见成员注释。
pub struct TableInfo {
    pub ID: i64,
    pub Name: CIStr,
    pub Columns: Vec<ColumnInfo>,
    pub IsCommonHandle: bool,
}

#[derive(Clone, Debug, Default)]
/// `DBInfo`：结构体定义，字段语义见成员注释。
pub struct DBInfo {
    pub ID: i64,
    pub Name: CIStr,
}

#[derive(Clone, Debug, Default)]
/// `SimpleTableInfo`：结构体定义，字段语义见成员注释。
pub struct SimpleTableInfo {
    pub ID: i64,
    pub Name: CIStr,
}

/// `InfoSchema`：接口边界，实现见 impl。
pub trait InfoSchema: Send + Sync {
    // `TableByName`：见函数体控制流与错误处理。
    fn TableByName(&self, db: &CIStr, table: &CIStr) -> Result<TableInfo>;
    // `AllSchemas`：见函数体控制流与错误处理。
    fn AllSchemas(&self) -> Vec<DBInfo>;
}

/// `MetaReader`：接口边界，实现见 impl。
pub trait MetaReader: Send + Sync {
    // `ListSimpleTables`：见函数体控制流与错误处理。
    fn ListSimpleTables(&self, db_id: i64) -> Result<Vec<SimpleTableInfo>>;
}

/// `Domain`：接口边界，实现见 impl。
pub trait Domain: Send + Sync {
    // `InfoSchema`：见函数体控制流与错误处理。
    fn InfoSchema(&self) -> &dyn InfoSchema;
    // `MetaReader`：见函数体控制流与错误处理。
    fn MetaReader(&self) -> &dyn MetaReader;
}

/// `IsMemOrSysDB`：见函数体控制流与错误处理。
pub fn IsMemOrSysDB(name: &str) -> bool {
    matches!(
        name,
        "information_schema"
            | "performance_schema"
            | "mysql"
            | "sys"
            | "metrics_schema"
            | "inspection_schema"
    )
}

#[derive(Clone, Default)]
/// `PiTRIdTracker`：结构体定义，字段语义见成员注释。
pub struct PiTRIdTracker {
    pub table_ids: HashSet<i64>,
    pub partition_ids: HashSet<i64>,
    pub db_ids: HashSet<i64>,
}

// impl PiTRIdTracker {：方法实现见下。
impl PiTRIdTracker {
    /// `new`：见函数体控制流与错误处理。
    pub fn new() -> Self {
        Self::default()
    }

    /// `ContainsTableId`：见函数体控制流与错误处理。
    pub fn ContainsTableId(&self, id: i64) -> bool {
        self.table_ids.contains(&id)
    }

    /// `ContainsPartitionId`：见函数体控制流与错误处理。
    pub fn ContainsPartitionId(&self, id: i64) -> bool {
        self.partition_ids.contains(&id)
    }

    /// `ContainsDB`：见函数体控制流与错误处理。
    pub fn ContainsDB(&self, id: i64) -> bool {
        self.db_ids.contains(&id)
    }
}

/// Optional checkpoint append boundary used by restorers (Go CheckpointRunner).
/// （中文）Optional checkpoint append boundary used by restorers (Go CheckpointRunner).
pub trait RestoreCheckpoint: Send + Sync {
    // `AppendFile`：见函数体控制流与错误处理。
    fn AppendFile(&self, ctx: &Context, table_id: i64, name: &str) -> Result<()>;
    // `AppendRangeKey`：见函数体控制流与错误处理。
    fn AppendRangeKey(&self, ctx: &Context, table_id: i64, range_key: &str) -> Result<()>;
}

#[derive(Default)]
/// `MemRestoreCheckpoint`：结构体定义，字段语义见成员注释。
pub struct MemRestoreCheckpoint {
    pub files: Mutex<Vec<(i64, String)>>,
    pub ranges: Mutex<Vec<(i64, String)>>,
}

// impl RestoreCheckpoint for MemRestoreCheckpoint {：方法实现见下。
impl RestoreCheckpoint for MemRestoreCheckpoint {
    // `AppendFile`：见函数体控制流与错误处理。
    fn AppendFile(&self, _ctx: &Context, table_id: i64, name: &str) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .push((table_id, name.to_string()));
        Ok(())
    }

    // `AppendRangeKey`：见函数体控制流与错误处理。
    fn AppendRangeKey(&self, _ctx: &Context, table_id: i64, range_key: &str) -> Result<()> {
        self.ranges
            .lock()
            .unwrap()
            .push((table_id, range_key.to_string()));
        Ok(())
    }
}

#[derive(Default)]
/// `SummaryCollector`：结构体定义，字段语义见成员注释。
pub struct SummaryCollector {
    pub failures: Mutex<Vec<(String, String)>>,
    pub durations: Mutex<Vec<(String, Duration)>>,
    pub success_units: Mutex<Vec<(String, usize, Duration)>>,
}

// impl SummaryCollector {：方法实现见下。
impl SummaryCollector {
    /// `CollectFailureUnit`：见函数体控制流与错误处理。
    pub fn CollectFailureUnit(&self, name: &str, err: &Error) {
        self.failures
            .lock()
            .unwrap()
            .push((name.to_string(), err.msg.clone()));
    }

    /// `CollectDuration`：见函数体控制流与错误处理。
    pub fn CollectDuration(&self, name: &str, d: Duration) {
        self.durations.lock().unwrap().push((name.to_string(), d));
    }

    /// `CollectSuccessUnit`：见函数体控制流与错误处理。
    pub fn CollectSuccessUnit(&self, name: &str, count: usize, d: Duration) {
        self.success_units
            .lock()
            .unwrap()
            .push((name.to_string(), count, d));
    }
}

thread_local! {
    /// `SUMMARY`：与 Go 同名常量/静态对齐。
    static SUMMARY: SummaryCollector = SummaryCollector::default();
}

/// `summary` 子模块：桩或辅助实现。
pub mod summary {
    use super::*;

    /// `CollectFailureUnit`：见函数体控制流与错误处理。
    pub fn CollectFailureUnit(name: &str, err: &Error) {
        SUMMARY.with(|s| s.CollectFailureUnit(name, err));
    }

    /// `CollectDuration`：见函数体控制流与错误处理。
    pub fn CollectDuration(name: &str, d: Duration) {
        SUMMARY.with(|s| s.CollectDuration(name, d));
    }

    /// `CollectSuccessUnit`：见函数体控制流与错误处理。
    pub fn CollectSuccessUnit(name: &str, count: usize, d: Duration) {
        SUMMARY.with(|s| s.CollectSuccessUnit(name, count, d));
    }
}

/// Lightweight worker pool matching Go `util.WorkerPool.ApplyOnErrorGroup`.
/// （中文）Lightweight worker pool matching Go `util.WorkerPool.ApplyOnErrorGroup`.
pub struct WorkerPool {
    limit: usize,
    active: Arc<Mutex<usize>>,
}

// impl WorkerPool {：方法实现见下。
impl WorkerPool {
    /// `new`：见函数体控制流与错误处理。
    pub fn new(limit: u64, _name: &str) -> Self {
        Self {
            limit: limit.max(1) as usize,
            active: Arc::new(Mutex::new(0)),
        }
    }

    /// `ApplyOnErrorGroup`：见函数体控制流与错误处理。
    pub fn ApplyOnErrorGroup<F>(&self, eg: &ErrorGroup, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        // Acquire the concurrency slot inside the worker thread so nested
        // ApplyOnErrorGroup (BatchRestorer) can queue without deadlocking.
        let active = self.active.clone();
        let limit = self.limit;
        eg.spawn(move || {
            loop {
                {
                    let mut n = active.lock().unwrap();
                    if *n < limit {
                        // 条件分支：见块内处理与 Go 对齐点。
                        *n += 1;
                        break;
                        // 结束当前循环。
                    }
                }
                thread::yield_now();
            }
            let res = f();
            *active.lock().unwrap() -= 1;
            res
        });
    }
}

/// `NewWorkerPool`：见函数体控制流与错误处理。
pub fn NewWorkerPool(limit: u64, name: &str) -> WorkerPool {
    WorkerPool::new(limit, name)
}

/// errgroup-like join set.
/// （中文）errgroup-like join set.
#[derive(Clone)]
/// `ErrorGroup`：结构体定义，字段语义见成员注释。
pub struct ErrorGroup {
    handles: Arc<Mutex<Vec<JoinHandle<Result<()>>>>>,
    cancelled: Context,
    inflight: Arc<AtomicUsize>,
}

// impl ErrorGroup {：方法实现见下。
impl ErrorGroup {
    /// `with_context`：见函数体控制流与错误处理。
    pub fn with_context(parent: &Context) -> (Self, Context) {
        let (child, _cancel) = Context::WithCancel(parent);
        (
            Self {
                handles: Arc::new(Mutex::new(Vec::new())),
                cancelled: child.clone(),
                inflight: Arc::new(AtomicUsize::new(0)),
            },
            child,
        )
    }

    /// `spawn`：见函数体控制流与错误处理。
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let ctx = self.cancelled.clone();
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let inflight = self.inflight.clone();
        let handle = thread::spawn(move || {
            let res = if let Some(err) = ctx.Err() {
                Err(err)
                // 错误路径：向上返回，保留上下文。
            } else {
                f()
            };
            inflight.fetch_sub(1, Ordering::SeqCst);
            res
        });
        self.handles.lock().unwrap().push(handle);
    }

    /// `Wait`：见函数体控制流与错误处理。
    pub fn Wait(&self) -> Result<()> {
        let mut first_err = None;
        loop {
            let handles = std::mem::take(&mut *self.handles.lock().unwrap());
            if handles.is_empty() {
                // 条件分支：见块内处理与 Go 对齐点。
                if self.inflight.load(Ordering::SeqCst) == 0 {
                    // 条件分支：见块内处理与 Go 对齐点。
                    break;
                    // 结束当前循环。
                }
                thread::yield_now();
                continue;
                // 继续下一轮重试或迭代。
            }
            for h in handles {
                match h.join() {
                    // 分支匹配：各臂处理不同结果/错误。
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        self.cancelled.cancel(err.clone());
                        if first_err.is_none() {
                            // 条件分支：见块内处理与 Go 对齐点。
                            first_err = Some(err);
                        }
                    }
                    Err(_) => {
                        // 错误路径：向上返回，保留上下文。
                        let err = Error::new("worker panicked");
                        self.cancelled.cancel(err.clone());
                        if first_err.is_none() {
                            // 条件分支：见块内处理与 Go 对齐点。
                            first_err = Some(err);
                        }
                    }
                }
            }
        }
        match first_err {
            // 分支匹配：各臂处理不同结果/错误。
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// Aggressive PD backoff: a few retries (Go utils.NewAggressivePDBackoffStrategy).
/// （中文）Aggressive PD backoff: a few retries (Go utils.NewAggressivePDBackoffStrategy).
pub fn WithRetryAggressive<F>(ctx: &Context, mut f: F) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    let mut last = None;
    for attempt in 0..6 {
        if let Some(err) = ctx.Err() {
            // 条件分支：见块内处理与 Go 对齐点。
            return Err(err);
            // 错误路径：向上返回，保留上下文。
        }
        match f() {
            // 分支匹配：各臂处理不同结果/错误。
            Ok(()) => return Ok(()),
            Err(err) => {
                // 错误路径：向上返回，保留上下文。
                last = Some(err);
                if attempt + 1 < 6 {
                    // 条件分支：见块内处理与 Go 对齐点。
                    thread::sleep(Duration::from_millis(1 << attempt.min(4)));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| Error::new("retry exhausted")))
    // 错误路径：向上返回，保留上下文。
}

/// WaitGroup for import-mode background goroutine lifecycle.
/// （中文）WaitGroup for import-mode background goroutine lifecycle.
#[derive(Default)]
/// `WaitGroup`：结构体定义，字段语义见成员注释。
pub struct WaitGroup {
    count: AtomicUsize,
    done_flag: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

// impl WaitGroup {：方法实现见下。
impl WaitGroup {
    /// `new`：见函数体控制流与错误处理。
    pub fn new() -> Self {
        Self::default()
    }

    /// `Add`：见函数体控制流与错误处理。
    pub fn Add(&self, delta: isize) {
        if delta >= 0 {
            // 条件分支：见块内处理与 Go 对齐点。
            self.count.fetch_add(delta as usize, Ordering::SeqCst);
        } else {
            self.Done();
        }
    }

    /// `Done`：见函数体控制流与错误处理。
    pub fn Done(&self) {
        let prev = self.count.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            // 条件分支：见块内处理与 Go 对齐点。
            let (lock, cv) = &*self.done_flag;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
    }

    /// `Wait`：见函数体控制流与错误处理。
    pub fn Wait(&self) {
        if self.count.load(Ordering::SeqCst) == 0 {
            // 条件分支：见块内处理与 Go 对齐点。
            return;
        }
        let (lock, cv) = &*self.done_flag;
        let mut guard = lock.lock().unwrap();
        while self.count.load(Ordering::SeqCst) > 0 {
            guard = cv.wait(guard).unwrap();
        }
    }
}

/// `log` 子模块：桩或辅助实现。
pub mod log {
    /// `Info`：见函数体控制流与错误处理。
    pub fn Info(_msg: &str) {}
    /// `Warn`：见函数体控制流与错误处理。
    pub fn Warn(_msg: &str) {}
    /// `Error`：见函数体控制流与错误处理。
    pub fn Error(_msg: &str) {}
}

/// Local SplitStrategy / PipelineRegionsSplitter stand-ins for `PipelineRestorerWrapper`.
/// （中文）Local SplitStrategy / PipelineRegionsSplitter stand-ins for `PipelineRestorerWrapper`.
pub trait SplitHelperLike: Send {
    // `as_any`：见函数体控制流与错误处理。
    fn as_any(&self) -> &dyn std::any::Any;
}

/// `SplitHelperIterator`：结构体定义，字段语义见成员注释。
pub struct SplitHelperIterator {
    pub items: Vec<Box<dyn SplitHelperLike>>,
}

/// `SplitStrategy`：接口边界，实现见 impl。
pub trait SplitStrategy<T>: Send {
    // `Accumulate`：见函数体控制流与错误处理。
    fn Accumulate(&mut self, v: T);
    // `ShouldSplit`：见函数体控制流与错误处理。
    fn ShouldSplit(&self) -> bool;
    // `ShouldSkip`：见函数体控制流与错误处理。
    fn ShouldSkip(&self, v: &T) -> bool;
    // `GetAccumulations`：见函数体控制流与错误处理。
    fn GetAccumulations(&self) -> SplitHelperIterator;
    // `ResetAccumulations`：见函数体控制流与错误处理。
    fn ResetAccumulations(&mut self);
}

/// `PipelineRegionsSplitter`：接口边界，实现见 impl。
pub trait PipelineRegionsSplitter: Send + Sync {
    // `ExecuteRegions`：见函数体控制流与错误处理。
    fn ExecuteRegions(&self, ctx: &Context, split_helper: &SplitHelperIterator) -> Result<()>;
}

/// In-memory PD client for tests.
/// （中文）In-memory PD client for tests.
#[derive(Default)]
/// `MemPdClient`：结构体定义，字段语义见成员注释。
pub struct MemPdClient {
    pub stores: Mutex<Vec<metapb::Store>>,
    pub ts: Mutex<(i64, i64)>,
    pub fail_ts: AtomicBool,
    pub fail_ts_times: AtomicUsize,
}

// impl MemPdClient {：方法实现见下。
impl MemPdClient {
    /// `new`：见函数体控制流与错误处理。
    pub fn new(stores: Vec<metapb::Store>) -> Self {
        Self {
            stores: Mutex::new(stores),
            ts: Mutex::new((100, 1)),
            fail_ts: AtomicBool::new(false),
            fail_ts_times: AtomicUsize::new(0),
        }
    }
}

// impl PdClient for MemPdClient {：方法实现见下。
impl PdClient for MemPdClient {
    // `GetTS`：见函数体控制流与错误处理。
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        if self.fail_ts.load(Ordering::SeqCst) {
            // 条件分支：见块内处理与 Go 对齐点。
            let n = self.fail_ts_times.fetch_add(1, Ordering::SeqCst);
            if n < 3 {
                // 条件分支：见块内处理与 Go 对齐点。
                return Err(Error::new(
                    // 错误路径：向上返回，保留上下文。
                    "rpc error: code = Unknown desc = [PD:tso:ErrGenerateTimestamp]generate timestamp failed, requested pd is not leader of cluster",
                ));
            }
        }
        Ok(*self.ts.lock().unwrap())
    }

    // `GetAllStores`：见函数体控制流与错误处理。
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

/// In-memory split client returning a fixed region list.
/// （中文）In-memory split client returning a fixed region list.
#[derive(Default)]
/// `MemSplitClient`：结构体定义，字段语义见成员注释。
pub struct MemSplitClient {
    pub regions: Mutex<Vec<RegionInfo>>,
}

// impl MemSplitClient {：方法实现见下。
impl MemSplitClient {
    /// `new`：见函数体控制流与错误处理。
    pub fn new(regions: Vec<RegionInfo>) -> Self {
        Self {
            regions: Mutex::new(regions),
        }
    }
}

// impl SplitClient for MemSplitClient {：方法实现见下。
impl SplitClient for MemSplitClient {
    // `ScanRegions`：见函数体控制流与错误处理。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        key: &[u8],
        _end_key: &[u8],
        limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        let regions = self.regions.lock().unwrap();
        let start_idx = regions
            .iter()
            .position(|r| {
                let Some(reg) = &r.Region else {
                    return false;
                };
                reg.StartKey.as_slice() <= key
                    && (reg.EndKey.is_empty() || reg.EndKey.as_slice() > key)
            })
            .unwrap_or(0);
        let mut out = Vec::new();
        for r in regions.iter().skip(start_idx) {
            out.push(r.clone());
            if limit > 0 && out.len() >= limit as usize {
                // 条件分支：见块内处理与 Go 对齐点。
                break;
                // 结束当前循环。
            }
        }
        if out.is_empty() && !regions.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            out.push(regions[0].clone());
        }
        Ok(out)
    }
}

#[derive(Default)]
/// `MemInfoSchema`：结构体定义，字段语义见成员注释。
pub struct MemInfoSchema {
    pub tables: Mutex<HashMap<(String, String), TableInfo>>,
    pub schemas: Mutex<Vec<DBInfo>>,
}

// impl InfoSchema for MemInfoSchema {：方法实现见下。
impl InfoSchema for MemInfoSchema {
    // `TableByName`：见函数体控制流与错误处理。
    fn TableByName(&self, db: &CIStr, table: &CIStr) -> Result<TableInfo> {
        self.tables
            .lock()
            .unwrap()
            .get(&(db.L.clone(), table.L.clone()))
            .cloned()
            .ok_or_else(|| Error::new(format!("table {}.{} not found", db.O, table.O)))
    }

    // `AllSchemas`：见函数体控制流与错误处理。
    fn AllSchemas(&self) -> Vec<DBInfo> {
        self.schemas.lock().unwrap().clone()
    }
}

#[derive(Default)]
/// `MemMetaReader`：结构体定义，字段语义见成员注释。
pub struct MemMetaReader {
    pub tables: Mutex<HashMap<i64, Vec<SimpleTableInfo>>>,
}

// impl MetaReader for MemMetaReader {：方法实现见下。
impl MetaReader for MemMetaReader {
    // `ListSimpleTables`：见函数体控制流与错误处理。
    fn ListSimpleTables(&self, db_id: i64) -> Result<Vec<SimpleTableInfo>> {
        Ok(self
            .tables
            .lock()
            .unwrap()
            .get(&db_id)
            .cloned()
            .unwrap_or_default())
    }
}

/// `MemDomain`：结构体定义，字段语义见成员注释。
pub struct MemDomain {
    pub info: MemInfoSchema,
    pub meta: MemMetaReader,
}

// impl Domain for MemDomain {：方法实现见下。
impl Domain for MemDomain {
    // `InfoSchema`：见函数体控制流与错误处理。
    fn InfoSchema(&self) -> &dyn InfoSchema {
        &self.info
    }

    // `MetaReader`：见函数体控制流与错误处理。
    fn MetaReader(&self) -> &dyn MetaReader {
        &self.meta
    }
}
