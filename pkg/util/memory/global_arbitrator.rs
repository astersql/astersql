// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 全局内存仲裁器门面与运行时内存状态落盘。
//
// 维护进程级 MemArbitrator 单例、软限制/工作模式文本配置，
// 以及 `mem-state.v1.json` 的原子写入与加载。

#![allow(non_snake_case)]

use serde_json::Value;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
pub(crate) static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

use crate::arbitrator::{
    ArbitratorModeDisable, NewMemArbitrator, SoftLimitModeAuto, SoftLimitModeDisable,
    SoftLimitModeSpecified,
};
pub use crate::arbitrator::{
    ArbitratorRuntimeStats, ArbitratorWorkMode as WorkMode, MemArbitrator, SoftLimitMode,
};
use crate::heap_profile::HeapProfileCollector;
use crate::meminfo::GetMemTotalIgnoreErr;
use crate::utils::SampleRuntimeMemStats;

/// 内存状态文件版本号。
const MEM_STATE_VERSION: &str = "v1";
/// 状态文件名前缀。
const MEM_STATE_PREFIX: &str = "mem-state.";
/// 状态文件名后缀。
const MEM_STATE_SUFFIX: &str = ".json";

/// Default work mode selected by the server configuration.
pub const DefaultGlobalMemArbitratorModeName: &str = "priority";

/// Select the state directory from the log directory, falling back to the temp directory.
pub fn MemArbitratorStateDir(log_filename: &Path, temp_dir: &Path, port: u64) -> PathBuf {
    match log_filename
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(log_dir) => log_dir.join("mem_arbitrator"),
        None => temp_dir.join(format!("mem_arbitrator-{port}")),
    }
}

/// Runtime hook for the heap profiler owned by the heap-profile module.
pub trait HeapProfileRuntime: Send + Sync {
    fn reset_trigger_state(&self);
    fn should_check(&self) -> bool;
    fn try_capture(&self, arbitrator: &MemArbitrator);
}

/// Implements the exact parsing order used by Go: uint first, then a ratio.
/// 按 Go 相同顺序解析：先无符号整数，再比例；"0"/"auto" 为特殊值。
pub fn parse_soft_limit(value: &str) -> (i64, f64, SoftLimitMode) {
    match value {
        "0" => return (0, 0.0, SoftLimitModeDisable),
        "auto" => return (0, 0.0, SoftLimitModeAuto),
        _ => {}
    }
    if let Ok(integer) = value.parse::<u64>() {
        if integer > 1 && integer <= i64::MAX as u64 {
            return (integer as i64, 0.0, SoftLimitModeSpecified);
        }
    }
    if let Ok(rate) = value.parse::<f64>() {
        if rate > 0.0 && rate <= 1.0 {
            return (0, rate, SoftLimitModeSpecified);
        }
    }
    (0, 0.0, SoftLimitModeDisable)
}

/// 进程级全局状态：仲裁器槽位、配置文本与启用标志。
struct GlobalState {
    arbitrator: RwLock<Option<Arc<MemArbitrator>>>,
    soft_limit_text: RwLock<String>,
    work_mode_text: RwLock<String>,
    enabled: AtomicBool,
    mode_configured: AtomicBool,
    mode_initialization: Mutex<()>,
    server_limit: AtomicI64,
    runtime_sampler: RwLock<fn() -> ArbitratorRuntimeStats>,
    recorder: RwLock<Option<RuntimeMemStateRecorder>>,
    runtime_updates: AtomicI64,
    record_success: AtomicI64,
    record_failure: AtomicI64,
    runtime_handler: Mutex<()>,
    runtime_reset: AtomicBool,
    heap_profiler: RwLock<Option<Arc<dyn HeapProfileRuntime>>>,
}

/// 惰性初始化并返回全局状态单例。
fn state() -> &'static GlobalState {
    static STATE: OnceLock<GlobalState> = OnceLock::new();
    STATE.get_or_init(|| GlobalState {
        arbitrator: RwLock::new(None),
        soft_limit_text: RwLock::new("0".to_owned()),
        work_mode_text: RwLock::new("disable".to_owned()),
        enabled: AtomicBool::new(false),
        mode_configured: AtomicBool::new(false),
        mode_initialization: Mutex::new(()),
        server_limit: AtomicI64::new(0),
        runtime_sampler: RwLock::new(sample_runtime_mem_stats),
        recorder: RwLock::new(None),
        runtime_updates: AtomicI64::new(0),
        record_success: AtomicI64::new(0),
        record_failure: AtomicI64::new(0),
        runtime_handler: Mutex::new(()),
        runtime_reset: AtomicBool::new(false),
        heap_profiler: RwLock::new(None),
    })
}

/// 读取软限制配置原文。
pub fn GetGlobalMemArbitratorSoftLimitText() -> String {
    state()
        .soft_limit_text
        .read()
        .expect("soft limit text lock poisoned")
        .clone()
}

/// 更新软限制文本并同步到当前仲裁器实例。
pub fn SetGlobalMemArbitratorSoftLimit(value: String) {
    // 文本未变则跳过，避免无谓写锁与重解析。
    if GetGlobalMemArbitratorSoftLimitText() == value {
        return;
    }
    *state()
        .soft_limit_text
        .write()
        .expect("soft limit text lock poisoned") = value.clone();
    if let Some(arbitrator) = state()
        .arbitrator
        .read()
        .expect("arbitrator lock poisoned")
        .as_ref()
    {
        let (bytes, rate, mode) = parse_soft_limit(&value);
        arbitrator.SetSoftLimit(bytes, rate, mode);
    }
}

/// 读取工作模式配置原文。
pub fn GetGlobalMemArbitratorWorkModeText() -> String {
    state()
        .work_mode_text
        .read()
        .expect("work mode text lock poisoned")
        .clone()
}

/// 切换全局工作模式；从 Disable 启用时刷新限制。
/// Apply the server default once without overriding an explicitly selected mode.
pub fn InitializeGlobalMemArbitratorMode(value: String) {
    let _guard = state()
        .mode_initialization
        .lock()
        .expect("mode initialization lock poisoned");
    if !state().mode_configured.load(Ordering::Acquire) {
        SetGlobalMemArbitratorWorkMode(value);
    }
}

pub fn SetGlobalMemArbitratorWorkMode(value: String) -> bool {
    state().mode_configured.store(true, Ordering::Release);
    if GetGlobalMemArbitratorWorkModeText() == value {
        return false;
    }
    let new_mode = WorkMode::from_text(&value);
    *state()
        .work_mode_text
        .write()
        .expect("work mode text lock poisoned") = value;
    let arbitrator = {
        let mut slot = state()
            .arbitrator
            .write()
            .expect("arbitrator lock poisoned");
        slot.get_or_insert_with(|| {
            let cfg = config_crate::get_global_config();
            let base_dir = MemArbitratorStateDir(
                Path::new(&cfg.log.file.filename),
                Path::new(&cfg.temp_dir),
                cfg.port,
            );
            *state()
                .heap_profiler
                .write()
                .expect("heap profiler lock poisoned") = Some(Arc::new(
                HeapProfileCollector::new_default(base_dir.join("heap_profiles")),
            ));
            let recorder = RuntimeMemStateRecorder::new(base_dir);
            let previous = recorder.load().ok().flatten();
            *state().recorder.write().expect("recorder lock poisoned") = Some(recorder);
            let configured_limit = state().server_limit.load(Ordering::SeqCst);
            let limit = if configured_limit == 0 {
                GetMemTotalIgnoreErr().min(i64::MAX as u64) as i64
            } else {
                configured_limit
            };
            let arbitrator = Arc::new(NewMemArbitrator(limit));
            if let Some(previous) = previous {
                arbitrator.RestoreRuntimeMemState(
                    previous["magnif"].as_i64().unwrap_or(0),
                    previous["pool-medium-cap"].as_i64().unwrap_or(0),
                );
            }
            arbitrator
        })
        .clone()
    };
    if arbitrator.WorkMode() == new_mode {
        return false;
    }
    // 切到 Disable：只改模式并关闭 enabled。
    if new_mode == WorkMode::Disable {
        arbitrator.SetWorkMode(new_mode);
        state().enabled.store(false, Ordering::SeqCst);
        state().runtime_reset.store(true, Ordering::Release);
        return true;
    }
    state().enabled.store(true, Ordering::SeqCst);
    // 从 Disable 启用时同步 limit/soft limit。
    if arbitrator.WorkMode() == WorkMode::Disable {
        let limit = state().server_limit.load(Ordering::SeqCst);
        arbitrator.SetLimit(if limit == 0 {
            GetMemTotalIgnoreErr()
        } else {
            limit.max(0) as u64
        });
        let parsed = parse_soft_limit(&GetGlobalMemArbitratorSoftLimitText());
        arbitrator.SetSoftLimit(parsed.0, parsed.1, parsed.2);
        arbitrator.StartAutoRun(Duration::from_millis(10));
    }
    arbitrator.SetWorkMode(new_mode);
    true
}

/// 返回非 Disable 模式下的全局仲裁器。
pub fn GlobalMemArbitrator() -> Option<Arc<MemArbitrator>> {
    let value = state()
        .arbitrator
        .read()
        .expect("arbitrator lock poisoned")
        .clone();
    value.filter(|arbitrator| arbitrator.WorkMode() != WorkMode::Disable)
}

/// 全局仲裁是否已启用。
pub fn UsingGlobalMemArbitration() -> bool {
    state().enabled.load(Ordering::SeqCst)
}

/// 设置服务端内存上限并尝试同步到仲裁器。
pub fn SetGlobalMemArbitratorLimit(limit: i64) {
    state().server_limit.store(limit, Ordering::SeqCst);
    AjustGlobalMemArbitratorLimit();
}

/// 将 server_limit 写回当前全局仲裁器（函数名保留 Go 拼写）。
pub fn AjustGlobalMemArbitratorLimit() {
    if let Some(arbitrator) = GlobalMemArbitrator() {
        arbitrator.SetLimit(state().server_limit.load(Ordering::SeqCst).max(0) as u64);
    }
}

/// 从全局仲裁器移除指定根池。
pub fn RemovePoolFromGlobalMemArbitrator(uid: u64) -> bool {
    state()
        .arbitrator
        .read()
        .expect("arbitrator lock poisoned")
        .as_ref()
        .is_some_and(|arbitrator| arbitrator.RemoveRootPoolByID(uid))
}

/// 测试前重置全局仲裁状态。
pub fn SetupGlobalMemArbitratorForTest(base_dir: String) {
    state().mode_configured.store(false, Ordering::Release);
    if let Some(arbitrator) = state()
        .arbitrator
        .read()
        .expect("arbitrator lock poisoned")
        .as_ref()
    {
        arbitrator.StopAutoRun();
    }
    state().enabled.store(false, Ordering::SeqCst);
    state().server_limit.store(0, Ordering::SeqCst);
    *state()
        .soft_limit_text
        .write()
        .expect("soft limit text lock poisoned") = "0".to_owned();
    *state()
        .work_mode_text
        .write()
        .expect("work mode text lock poisoned") = "disable".to_owned();
    *state()
        .arbitrator
        .write()
        .expect("arbitrator lock poisoned") = Some(Arc::new(NewMemArbitrator(0)));
    *state()
        .runtime_sampler
        .write()
        .expect("runtime sampler lock poisoned") = sample_runtime_mem_stats;
    let recorder = RuntimeMemStateRecorder::new(&base_dir);
    // Go deliberately ignores removal errors here: setup must not reuse stale
    // runtime state, while an unwritable test directory is handled on store.
    let _ = fs::remove_file(&recorder.file_path);
    *state().recorder.write().expect("recorder lock poisoned") = Some(recorder);
    state().runtime_updates.store(0, Ordering::Release);
    state().record_success.store(0, Ordering::Release);
    state().record_failure.store(0, Ordering::Release);
    state().runtime_reset.store(false, Ordering::Release);
    *state()
        .heap_profiler
        .write()
        .expect("heap profiler lock poisoned") = None;
}

/// 测试后禁用并清空全局仲裁器。
pub fn CleanupGlobalMemArbitratorForTest() {
    let _ = SetGlobalMemArbitratorWorkMode("disable".to_owned());
    state().enabled.store(false, Ordering::SeqCst);
    if let Some(arbitrator) = state()
        .arbitrator
        .read()
        .expect("arbitrator lock poisoned")
        .as_ref()
    {
        arbitrator.StopAutoRun();
    }
    *state()
        .arbitrator
        .write()
        .expect("arbitrator lock poisoned") = None;
    *state()
        .runtime_sampler
        .write()
        .expect("runtime sampler lock poisoned") = sample_runtime_mem_stats;
    *state().recorder.write().expect("recorder lock poisoned") = None;
    *state()
        .heap_profiler
        .write()
        .expect("heap profiler lock poisoned") = None;
}

/// Install the profiler used by the serialized runtime handler.
pub fn InstallHeapProfileRuntime(profiler: Option<Arc<dyn HeapProfileRuntime>>) {
    *state()
        .heap_profiler
        .write()
        .expect("heap profiler lock poisoned") = profiler;
}

#[cfg(test)]
pub fn SetHeapProfileRuntimeForTest(profiler: Option<Arc<dyn HeapProfileRuntime>>) {
    InstallHeapProfileRuntime(profiler);
}

fn sample_runtime_mem_stats() -> ArbitratorRuntimeStats {
    let stats = SampleRuntimeMemStats();
    ArbitratorRuntimeStats {
        heap_alloc: stats.HeapAlloc as i64,
        heap_inuse: stats.HeapInuse as i64,
        mem_off_heap: stats.MemOffHeap as i64,
        total_free: stats.TotalFree as i64,
        last_gc_unix_nano: 0,
    }
}

/// 测试注入运行时采样器；Setup/Cleanup 会恢复真实采样器。
pub fn SetRuntimeMemStatsSamplerForTest(sampler: fn() -> ArbitratorRuntimeStats) {
    *state()
        .runtime_sampler
        .write()
        .expect("runtime sampler lock poisoned") = sampler;
}

/// 采样运行时内存并交给共享核心仲裁器处理。
pub fn HandleGlobalMemArbitratorRuntime() {
    let Ok(_handler) = state().runtime_handler.try_lock() else {
        return;
    };
    let profiler = state()
        .heap_profiler
        .read()
        .expect("heap profiler lock poisoned")
        .clone();
    if state().runtime_reset.swap(false, Ordering::AcqRel) {
        if let Some(profiler) = profiler.as_ref() {
            profiler.reset_trigger_state();
        }
        state().runtime_updates.store(0, Ordering::Release);
        state().record_success.store(0, Ordering::Release);
        state().record_failure.store(0, Ordering::Release);
    }
    let Some(arbitrator) = GlobalMemArbitrator() else {
        return;
    };
    let sampler = *state()
        .runtime_sampler
        .read()
        .expect("runtime sampler lock poisoned");
    let stats = sampler();
    let was_at_mem_risk = arbitrator.AtMemRisk();
    arbitrator.HandleRuntimeStats(stats);
    state().runtime_updates.fetch_add(1, Ordering::AcqRel);
    if let Some(profiler) = profiler.as_ref() {
        if profiler.should_check() {
            profiler.try_capture(&arbitrator);
        }
    }

    if arbitrator.AtMemRisk()
        && !was_at_mem_risk
        && arbitrator.SoftLimitConfig().2 == SoftLimitModeAuto
    {
        let quota = arbitrator.Allocated();
        if quota > 0 && stats.heap_alloc > quota {
            let value = serde_json::json!({
                "version": 1,
                "last-risk": {
                    "heap": stats.heap_alloc,
                    "quota": quota,
                },
                "magnif": (stats.heap_alloc.saturating_mul(1000) / quota + 100).min(10_000),
                "pool-medium-cap": arbitrator.SuggestPoolInitCap(),
            });
            if let Some(recorder) = state()
                .recorder
                .read()
                .expect("recorder lock poisoned")
                .as_ref()
            {
                match recorder.store(&value) {
                    Ok(()) => {
                        state().record_success.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(_) => {
                        state().record_failure.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
        }
    }

    if let Some(recorder) = state()
        .recorder
        .read()
        .expect("recorder lock poisoned")
        .as_ref()
    {
        let now_unix_milli = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        match recorder.persist_pool_medium_if_changed(
            arbitrator.MemMagnif(),
            arbitrator.SuggestPoolInitCap(),
            now_unix_milli,
        ) {
            Ok(true) => {
                state().record_success.fetch_add(1, Ordering::AcqRel);
            }
            Err(_) => {
                state().record_failure.fetch_add(1, Ordering::AcqRel);
            }
            Ok(false) => {}
        }
        match recorder.persist_magnif_if_decreased(arbitrator.MemMagnif()) {
            Ok(true) => {
                state().record_success.fetch_add(1, Ordering::AcqRel);
            }
            Err(_) => {
                state().record_failure.fetch_add(1, Ordering::AcqRel);
            }
            Ok(false) => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalArbitratorMetrics {
    pub runtime_updates: i64,
    pub record_success: i64,
    pub record_failure: i64,
}

pub fn GlobalMemArbitratorMetrics() -> GlobalArbitratorMetrics {
    GlobalArbitratorMetrics {
        runtime_updates: state().runtime_updates.load(Ordering::Acquire),
        record_success: state().record_success.load(Ordering::Acquire),
        record_failure: state().record_failure.load(Ordering::Acquire),
    }
}

#[derive(Clone, Debug)]
/// 将运行时内存状态原子写入版本化 JSON 文件。
pub struct RuntimeMemStateRecorder {
    base_dir: PathBuf,
    file_path: PathBuf,
    last_state: Arc<Mutex<Option<Value>>>,
    last_record_unix_milli: Arc<AtomicI64>,
}

impl RuntimeMemStateRecorder {
    /// 基于目录构造 recorder，文件名为 mem-state.v1.json。
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let base_dir = base_dir.as_ref().to_path_buf();
        let file_path = base_dir.join(format!(
            "{MEM_STATE_PREFIX}{MEM_STATE_VERSION}{MEM_STATE_SUFFIX}"
        ));
        Self {
            base_dir,
            file_path,
            last_state: Arc::new(Mutex::new(None)),
            last_record_unix_milli: Arc::new(AtomicI64::new(0)),
        }
    }

    /// 最近一次成功持久化的状态；失败的写入不会覆盖它。
    pub fn last_state(&self) -> Option<Value> {
        self.last_state
            .lock()
            .expect("runtime state lock poisoned")
            .clone()
    }

    /// 安全时间窗降低放大率后，保留上一次风险与建议初始配额。
    pub fn persist_magnif_if_decreased(&self, magnif: i64) -> io::Result<bool> {
        let Some(mut state) = self.last_state() else {
            return Ok(false);
        };
        let Some(previous) = state["magnif"].as_i64() else {
            return Ok(false);
        };
        if magnif >= previous {
            return Ok(false);
        }
        state["magnif"] = Value::from(magnif);
        self.store(&state)?;
        Ok(true)
    }

    /// 与 Go 的 pool-medium-cap 定期持久化对应；保留最近一次风险快照。
    pub fn persist_pool_medium_if_changed(
        &self,
        magnif: i64,
        pool_medium_cap: i64,
        now_unix_milli: i64,
    ) -> io::Result<bool> {
        if pool_medium_cap <= 0 {
            return Ok(false);
        }
        let previous = self.last_state();
        if previous
            .as_ref()
            .and_then(|state| state["pool-medium-cap"].as_i64())
            == Some(pool_medium_cap)
            || self
                .last_record_unix_milli
                .load(Ordering::Acquire)
                .saturating_add(10_000)
                > now_unix_milli
        {
            return Ok(false);
        }
        let last_risk = previous
            .as_ref()
            .and_then(|state| state.get("last-risk"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"heap": 0, "quota": 0}));
        let value = serde_json::json!({
            "version": 1,
            "last-risk": last_risk,
            "magnif": magnif,
            "pool-medium-cap": pool_medium_cap,
        });
        self.store(&value)?;
        Ok(true)
    }

    /// 经临时文件 persist，保证写入原子性。
    pub fn store(&self, value: &Value) -> io::Result<()> {
        let mut last_state = self.last_state.lock().expect("runtime state lock poisoned");
        fs::create_dir_all(&self.base_dir)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.base_dir)?;
        serde_json::to_writer(&mut temporary, value).map_err(io::Error::other)?;
        temporary.flush()?;
        temporary
            .persist(&self.file_path)
            .map_err(|error| error.error)?;
        *last_state = Some(value.clone());
        let now_unix_milli = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        self.last_record_unix_milli
            .store(now_unix_milli, Ordering::Release);
        Ok(())
    }

    /// 只加载当前版本的固定状态文件。
    pub fn load(&self) -> io::Result<Option<Value>> {
        let file = match File::open(&self.file_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::read_dir(&self.base_dir)?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let value: Value = serde_json::from_reader(file).map_err(io::Error::other)?;
        *self.last_state.lock().expect("runtime state lock poisoned") = Some(value.clone());
        Ok(Some(value))
    }
}
