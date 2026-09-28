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
// 维护进程级 MemArbitrator 单例、软限制/工作模式文本配置、启用回调，
// 以及 `mem-state.v1.json` 的原子写入与加载。

#![allow(non_snake_case)]

use serde_json::Value;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use crate::arbitrator::{
    ArbitratorModeDisable, NewMemArbitrator, SoftLimitModeAuto, SoftLimitModeDisable,
    SoftLimitModeSpecified,
};
pub use crate::arbitrator::{
    ArbitratorRuntimeStats, ArbitratorWorkMode as WorkMode, MemArbitrator, SoftLimitMode,
};
use crate::utils::SampleRuntimeMemStats;

/// 内存状态文件版本号。
const MEM_STATE_VERSION: &str = "v1";
/// 状态文件名前缀。
const MEM_STATE_PREFIX: &str = "mem-state.";
/// 状态文件名后缀。
const MEM_STATE_SUFFIX: &str = ".json";

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

/// 进程级全局状态：仲裁器槽位、配置文本、启用标志与回调。
struct GlobalState {
    arbitrator: RwLock<Option<Arc<MemArbitrator>>>,
    soft_limit_text: RwLock<String>,
    work_mode_text: RwLock<String>,
    enabled: AtomicBool,
    callbacks: Mutex<Vec<fn()>>,
    server_limit: AtomicI64,
    runtime_sampler: RwLock<fn() -> ArbitratorRuntimeStats>,
    recorder: RwLock<Option<RuntimeMemStateRecorder>>,
    runtime_updates: AtomicI64,
    record_success: AtomicI64,
    record_failure: AtomicI64,
}

/// 惰性初始化并返回全局状态单例。
fn state() -> &'static GlobalState {
    static STATE: OnceLock<GlobalState> = OnceLock::new();
    STATE.get_or_init(|| GlobalState {
        arbitrator: RwLock::new(Some(Arc::new(NewMemArbitrator(0)))),
        soft_limit_text: RwLock::new("0".to_owned()),
        work_mode_text: RwLock::new("disable".to_owned()),
        enabled: AtomicBool::new(false),
        callbacks: Mutex::new(Vec::with_capacity(1)),
        server_limit: AtomicI64::new(0),
        runtime_sampler: RwLock::new(sample_runtime_mem_stats),
        recorder: RwLock::new(None),
        runtime_updates: AtomicI64::new(0),
        record_success: AtomicI64::new(0),
        record_failure: AtomicI64::new(0),
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

/// 切换全局工作模式；从 Disable 启用时触发回调并刷新限制。
pub fn SetGlobalMemArbitratorWorkMode(value: String) -> bool {
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
            Arc::new(NewMemArbitrator(
                state().server_limit.load(Ordering::SeqCst),
            ))
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
        return true;
    }
    state().enabled.store(true, Ordering::SeqCst);
    // 从 Disable 启用：先跑回调，再同步 limit/soft limit。
    if arbitrator.WorkMode() == WorkMode::Disable {
        for callback in state()
            .callbacks
            .lock()
            .expect("callback lock poisoned")
            .iter()
            .copied()
        {
            callback();
        }
        arbitrator.SetLimit(state().server_limit.load(Ordering::SeqCst).max(0) as u64);
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

/// 注册从 Disable 切到启用时的回调。
pub fn RegisterCallbackForGlobalMemArbitrator(callback: fn()) {
    state()
        .callbacks
        .lock()
        .expect("callback lock poisoned")
        .push(callback);
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
    state()
        .callbacks
        .lock()
        .expect("callback lock poisoned")
        .clear();
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
}

/// 测试后禁用并清空全局仲裁器。
pub fn CleanupGlobalMemArbitratorForTest() {
    let _ = SetGlobalMemArbitratorWorkMode("disable".to_owned());
    state().enabled.store(false, Ordering::SeqCst);
    state()
        .callbacks
        .lock()
        .expect("callback lock poisoned")
        .clear();
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
    let Some(arbitrator) = GlobalMemArbitrator() else {
        return;
    };
    let sampler = *state()
        .runtime_sampler
        .read()
        .expect("runtime sampler lock poisoned");
    let stats = sampler();
    arbitrator.HandleRuntimeStats(stats);
    state().runtime_updates.fetch_add(1, Ordering::AcqRel);

    if arbitrator.AtMemRisk() && arbitrator.SoftLimitConfig().2 == SoftLimitModeAuto {
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
        }
    }

    /// 经临时文件 persist，保证写入原子性。
    pub fn store(&self, value: &Value) -> io::Result<()> {
        fs::create_dir_all(&self.base_dir)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.base_dir)?;
        serde_json::to_writer(&mut temporary, value).map_err(io::Error::other)?;
        temporary.flush()?;
        temporary
            .persist(&self.file_path)
            .map_err(|error| error.error)?;
        Ok(())
    }

    /// 扫描目录加载当前版本前缀的状态文件。
    pub fn load(&self) -> io::Result<Option<Value>> {
        let entries = fs::read_dir(&self.base_dir)?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 只接受当前版本前缀的状态文件。
            if name.starts_with(&format!("{MEM_STATE_PREFIX}{MEM_STATE_VERSION}."))
                && entry.file_type()?.is_file()
            {
                return serde_json::from_reader(File::open(entry.path())?)
                    .map(Some)
                    .map_err(io::Error::other);
            }
        }
        Ok(None)
    }
}
