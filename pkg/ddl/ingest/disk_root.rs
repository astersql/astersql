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

// 磁盘根空间管理模块。
//
// 在 DDL（数据定义语言，如建表/加索引）的 ingest（数据摄入/导入）流程中，
// 索引会先写入本地磁盘临时排序，再批量导入存储。为避免磁盘写满，
// 本模块跟踪磁盘容量、可用空间以及各任务的磁盘使用量，并据此判断
// 是否应当触发导入或存在磁盘写满风险。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// 磁盘容量占用阈值：当占用超过该比例时视为接近写满。
pub const CAPACITY_THRESHOLD: f64 = 0.9;

/// Go reserves two GiB of local-sort headroom per effective DXF runtime slot.
pub const LOCAL_SORT_HEADROOM_BYTES_PER_SLOT: u64 = 2 * 1024 * 1024 * 1024;

const DEFAULT_DDL_DISK_QUOTA: u64 = 100 * 1024 * 1024 * 1024;

/// Mirrors Go's test-only `TrackerCountForTest` counter.
pub static TRACKER_COUNT_FOR_TEST: AtomicI64 = AtomicI64::new(0);

/// 资源跟踪器：可上报单个 ingest 任务当前占用的磁盘字节数。
pub trait ResourceTracker: Send + Sync {
    /// 返回该任务当前占用的磁盘字节数。
    fn disk_usage(&self) -> u64;
}

/// 磁盘根管理器：维护某个磁盘路径的容量、可用空间及所有 ingest 任务的使用量。
///
/// 内部各字段使用 `Arc<Mutex<...>>` 包装，以便在多线程间共享并安全更新。
#[derive(Clone)]
pub struct DiskRoot {
    /// 被管理的磁盘路径。
    pub path: String,
    /// 磁盘总容量（字节）。
    state: Arc<Mutex<DiskState>>,
    quota: u64,
    updating: Arc<AtomicBool>,
}

struct DiskState {
    capacity: u64,
    used: u64,
    backend_used: u64,
    trackers: BTreeMap<i64, Arc<dyn ResourceTracker>>,
}

impl DiskRoot {
    /// 创建磁盘根管理器，指定路径、总容量与可用空间。
    pub fn new(path: impl Into<String>, capacity: u64, available: u64) -> Self {
        Self::new_with_quota(path, capacity, available, DEFAULT_DDL_DISK_QUOTA)
    }

    /// Creates a disk root with an explicit DDL quota.
    ///
    /// Go reads this value from `vardef.DDLDiskQuota`; accepting it here keeps
    /// instances deterministic while preserving the same import decision.
    pub fn new_with_quota(
        path: impl Into<String>,
        capacity: u64,
        available: u64,
        quota: u64,
    ) -> Self {
        Self {
            path: path.into(),
            state: Arc::new(Mutex::new(DiskState {
                capacity,
                used: capacity.wrapping_sub(available),
                backend_used: 0,
                trackers: BTreeMap::new(),
            })),
            quota,
            updating: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 注册一个任务的资源跟踪器。
    pub fn add(&self, id: i64, tracker: Arc<dyn ResourceTracker>) {
        self.state.lock().unwrap().trackers.insert(id, tracker);
        TRACKER_COUNT_FOR_TEST.fetch_add(1, Ordering::Relaxed);
    }
    /// 移除指定任务的资源跟踪器。
    pub fn remove(&self, id: i64) {
        self.state.lock().unwrap().trackers.remove(&id);
        TRACKER_COUNT_FOR_TEST.fetch_sub(1, Ordering::Relaxed);
    }
    /// 返回当前已注册的跟踪器数量。
    pub fn count(&self) -> usize {
        self.state.lock().unwrap().trackers.len()
    }
    /// 汇总所有任务当前的磁盘使用总量（字节）。
    pub fn tracked_usage(&self) -> u64 {
        self.state.lock().unwrap().backend_used
    }
    /// 刷新磁盘容量与可用空间的最新数值。
    pub fn update_usage(&self, capacity: u64, available: u64) {
        if self
            .updating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let mut state = self.state.lock().unwrap();
        state.backend_used = state.trackers.values().fold(0_u64, |total, tracker| {
            total.wrapping_add(tracker.disk_usage())
        });
        state.capacity = capacity;
        state.used = capacity.wrapping_sub(available);
        self.updating.store(false, Ordering::Release);
    }
    /// 判断是否应触发导入（将本地临时数据写入存储）。
    ///
    /// 当缓存的后端使用量超过 DDL 配额，或磁盘使用率达到 90% 时返回 true。
    pub fn should_import(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.backend_used > self.quota
            || (state.used != 0 || state.capacity != 0)
                && (state.used as f64) >= (state.capacity as f64) * CAPACITY_THRESHOLD
    }
    /// 预检磁盘使用情况：若存在写满风险则返回错误信息，否则返回 Ok。
    pub fn pre_check_usage(&self) -> Result<(), String> {
        let failure = |message: String| {
            astersql_util_dbterror::ErrIngestCheckEnvFailed
                .GenWithStackByArgs(&[message.into()])
                .to_string()
        };
        fail::fail_point!(
            "github.com/pingcap/tidb/pkg/ddl/ingest/mockIngestCheckEnvFailed",
            |_| Err(failure("mock error".into()))
        );
        std::fs::create_dir_all(&self.path).map_err(|error| failure(error.to_string()))?;
        let capacity = fs2::total_space(&self.path).map_err(|error| failure(error.to_string()))?;
        let available =
            fs2::available_space(&self.path).map_err(|error| failure(error.to_string()))?;
        if risk_of_disk_full(available, capacity) && !cfg!(target_os = "macos") {
            Err(failure(format!("no enough space in {}", self.path)))
        } else {
            Ok(())
        }
    }

    /// 启动时检查磁盘可用空间是否不小于 DDL 配额。
    pub fn startup_check(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        let available = state.capacity.wrapping_sub(state.used);
        if available < self.quota {
            Err(format!(
                "the available disk space({available}) in {} should be greater than @@tidb_ddl_disk_quota({})",
                self.path, self.quota
            ))
        } else {
            Ok(())
        }
    }
    /// 返回描述当前磁盘使用状况的可读字符串，便于日志与报错。
    pub fn usage_info(&self) -> String {
        let state = self.state.lock().unwrap();
        format!(
            "disk usage: {}/{}, backend usage: {}",
            state.used, state.capacity, state.backend_used
        )
    }
}

/// 判断磁盘是否存在写满风险：可用空间低于总容量的 1/10 时视为高风险。
pub fn risk_of_disk_full(available: u64, capacity: u64) -> bool {
    available < min_free_disk_bytes(capacity)
}

/// Returns the minimum free bytes using Go's `capacity - uint64(float64(capacity)*0.9)`
/// conversion order. This deliberately preserves its rounding boundary.
pub fn min_free_disk_bytes(capacity: u64) -> u64 {
    capacity.wrapping_sub((capacity as f64 * CAPACITY_THRESHOLD) as u64)
}

/// Inputs to the deterministic local-sort admission calculation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalSortDiskSpaceCheck<'a> {
    pub exec_id: &'a str,
    pub sort_path: &'a str,
    pub available_bytes: u64,
    pub total_capacity_bytes: u64,
    pub current_task_runtime_slots: i32,
    pub ddl_disk_quota: u64,
}

/// Error returned when the filesystem was measured successfully but local sort
/// does not have the headroom required by the current DXF task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalSortDiskSpaceError(String);

impl LocalSortDiskSpaceError {
    pub const fn is_ingest_check_env_failed(&self) -> bool {
        true
    }
}

impl std::fmt::Display for LocalSortDiskSpaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LocalSortDiskSpaceError {}

/// Applies the Go local-sort disk admission rule after the filesystem probe.
pub fn check_local_sort_disk_space(
    check: LocalSortDiskSpaceCheck<'_>,
) -> Result<(), LocalSortDiskSpaceError> {
    let task_headroom = (check.current_task_runtime_slots as u64)
        .wrapping_mul(LOCAL_SORT_HEADROOM_BYTES_PER_SLOT)
        .min(check.ddl_disk_quota);
    let free_threshold =
        min_free_disk_bytes(check.total_capacity_bytes).wrapping_add(task_headroom);
    if check.available_bytes > free_threshold {
        return Ok(());
    }

    let message = format!(
        "insufficient free disk space on TiDB node {} at {}: {} bytes available; available free disk space must be greater than {} bytes; the add-index job cannot start because low disk space would degrade SST ingestion. Free disk space on this TiDB node by removing unnecessary logs or files",
        check.exec_id, check.sort_path, check.available_bytes, free_threshold
    );
    let classified = astersql_util_dbterror::ErrIngestCheckEnvFailed
        .GenWithStackByArgs(&[message.into()])
        .to_string();
    Err(LocalSortDiskSpaceError(classified))
}

/// Probes a concrete local-sort path and then applies the admission rule.
/// Probe failures stay plain strings so the distributed executor may retry;
/// confirmed low space remains an ingest-environment error.
pub fn check_local_sort_disk_space_at_path(
    exec_id: &str,
    sort_path: &std::path::Path,
    current_task_runtime_slots: i32,
    ddl_disk_quota: u64,
) -> Result<(), String> {
    std::fs::create_dir_all(sort_path).map_err(|error| error.to_string())?;
    let total_capacity_bytes = fs2::total_space(sort_path).map_err(|error| error.to_string())?;
    let available_bytes = fs2::available_space(sort_path).map_err(|error| error.to_string())?;
    let result = check_local_sort_disk_space(LocalSortDiskSpaceCheck {
        exec_id,
        sort_path: &sort_path.to_string_lossy(),
        available_bytes,
        total_capacity_bytes,
        current_task_runtime_slots,
        ddl_disk_quota,
    });
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        result.map_err(|error| error.to_string())
    }
}
