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

// ingest 全局环境与临时目录管理。
//
// Lightning / ingest 模式会把索引回填的中间数据写到本地临时目录。
// 本模块维护全局根目录，并按 DDL 任务（job）生成子目录名；
// 同时提供识别“仍在处理中的任务”与“过期临时目录”的辅助函数，
// 便于清理已结束任务留下的磁盘占用。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
/// 全局 ingest 临时数据根目录，进程内只初始化一次。
static INGEST_ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
/// 初始化全局 Lightning/ingest 环境，设置临时数据根目录。
///
/// 返回 `true` 表示首次设置成功；若已初始化过则返回 `false`。
pub fn init_global_lightning_env(path: impl Into<PathBuf>) -> bool {
    let mut root = INGEST_ROOT.get_or_init(|| Mutex::new(None)).lock().unwrap();
    if root.is_some() {
        return false;
    }
    *root = Some(path.into());
    true
}
/// 获取当前全局 ingest 临时数据根目录；未初始化时返回 `None`。
pub fn ingest_temp_data_dir() -> Option<PathBuf> {
    INGEST_ROOT
        .get()
        .and_then(|root| root.lock().unwrap().clone())
}
/// 为指定 DDL 任务生成其 ingest 临时数据子目录路径。
///
/// 子目录名由 `job_id` 与是否查重（`check_duplicates`）编码而成，
/// 例如 `42` 或 `42-dup`。
pub fn generate_ingest_temp_data_dir(
    job_id: i64,
    check_duplicates: bool,
) -> Result<PathBuf, String> {
    let root =
        ingest_temp_data_dir().ok_or_else(|| "ingest environment is not initialized".to_owned())?;
    Ok(root.join(crate::backend_mgr::encode_backend_tag(
        job_id,
        check_duplicates,
    )))
}
/// 从目录名列表中筛出仍处于活跃状态的任务 ID。
///
/// 仅保留能成功解码且出现在 `active_job_ids` 中的 ID，并排序去重。
pub fn processing_job_ids(
    directory_names: impl IntoIterator<Item = String>,
    active_job_ids: &BTreeSet<i64>,
) -> Vec<i64> {
    let mut ids = directory_names
        .into_iter()
        .filter_map(|name| crate::backend_mgr::decode_backend_tag(&name).ok())
        .filter(|id| active_job_ids.contains(id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids
}
/// 找出属于已结束任务的过期临时目录路径，供后续清理使用。
///
/// 能解码出 job ID、且该 ID 不在 `active_job_ids` 中的目录视为过期。
pub fn stale_temp_directories(
    root: &Path,
    directory_names: impl IntoIterator<Item = String>,
    active_job_ids: &BTreeSet<i64>,
) -> Vec<PathBuf> {
    // Go classifies jobs from their regular directory, then removes both the
    // regular and duplicate-check directories for every finished job.
    directory_names
        .into_iter()
        .filter_map(|name| {
            crate::backend_mgr::decode_backend_tag(&name)
                .ok()
                .filter(|id| !active_job_ids.contains(id))
        })
        .flat_map(|id| {
            [
                root.join(crate::backend_mgr::encode_backend_tag(id, false)),
                root.join(crate::backend_mgr::encode_backend_tag(id, true)),
            ]
        })
        .collect()
}

/// The initialized ingest root owns the local disk precheck resource.
pub fn initialized_disk_root() -> Option<crate::disk_root::DiskRoot> {
    ingest_temp_data_dir().map(|path| crate::disk_root::DiskRoot::new(path.to_string_lossy(), 0, 0))
}

/// Mirrors restoration of Go's mutable LitInitialized/LitDiskRoot in tests.
#[doc(hidden)]
pub fn replace_global_lightning_env_for_test(path: Option<PathBuf>) -> Option<PathBuf> {
    std::mem::replace(
        &mut *INGEST_ROOT.get_or_init(|| Mutex::new(None)).lock().unwrap(),
        path,
    )
}
