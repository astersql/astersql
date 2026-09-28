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

//! In-memory Lightning import progress: task/table status, checkpoints, JSON output.
//! Ported from `lightning/pkg/progress/progress.go`.
//! 本文件维护任务级别和表级别的导入进度快照，并提供对外可见的 JSON 视图。
//! 它故意把真实并发模型压缩成全局槽位加若干锁，优先保持 Go 的可观察行为。
//! 调用方关心的是状态码、错误文案、表名和写入字节数，而不是内部锁实现细节。
//! `CheckpointsMap` 负责保存每张表最近一次深拷贝后的 checkpoint 视图。
//! `TaskProgressGuarded` 负责保存任务状态、表状态和任务级错误消息。
//! `TaskProgressJson` 负责把内部状态投影成与 Go 对齐的字段名。
//! `EnableCurrentProgress` 负责初始化全局单例。
//! `BroadcastStartTask` 负责进入运行态并清空旧 checkpoint 快照。
//! `BroadcastEndTask` 负责记录任务级完成状态和汇总错误。
//! `BroadcastInitProgress` 负责展开 mydump 元数据并填充总大小。
//! `BroadcastTableCheckpoint` 负责把单表状态推进到运行中，并缓存深拷贝快照。
//! `BroadcastTableProgress` 负责按步骤名去重更新表级进度条。
//! `BroadcastCheckpointDiff` 负责把 checkpoint 差量折算成已写入字节数。
//! `BroadcastError` 负责写入表级错误，但允许缺失表时静默跳过。
//! `MarshalTaskProgress` 负责输出任务级 JSON 快照。
//! `MarshalTableCheckpoints` 负责输出表级 checkpoint JSON 快照。
//! 手工序列化 `marshal_table_checkpoint` 是为了严格匹配 Go 的导出字段外观。
//! 测试辅助函数则负责在多场景间重置全局单例，避免状态串味。
//! 因此阅读本文件时，应把它看成一个“可广播、可快照、可重置”的轻量状态机。
//! 中文注释优先解释状态推进和边界约束，而不是逐行翻译锁代码。
//! 只有当进度被显式启用时，后续广播才会真正生效。
//! 未启用时返回错误或 no-op，也是对 Go 外观的一部分复刻。
//! 表级快照与任务级快照共享同一生命周期，但序列化入口分开。
//! 这让 HTTP 或测试调用者能分别取任务总览与单表明细。
//! 错误消息采用最小字符串表示，方便直接进入 JSON 或日志。
//! checkpoint 读写公式则继续遵循 Go 对 chunk offset 的解释方式。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock}; // Mutex only for global slot

use astersql_lightning_pkg_checkpoints::{
    CheckpointStatusAllWritten, TableCheckpoint, TableCheckpointDiff,
};
use serde::Serialize;

use crate::common;
use crate::errors::{self, Error, Result};
use crate::mydump;

/// Concurrent map (table name → checkpoints), guarded by a single RWMutex like Go.
/// 这里不使用更复杂的并发 map，
/// 因为状态更新频率不高，保持语义简单比追求极致性能更重要。
struct CheckpointsMap {
    checkpoints: RwLock<HashMap<String, TableCheckpoint>>,
}

impl CheckpointsMap {
    fn new() -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
        }
    }

    fn clear(&self) {
        // Replace map like Go: `cpm.checkpoints = make(...)`
        // 直接替换整张表，确保旧任务残留快照不会泄漏到新任务。
        *self.checkpoints.write().unwrap() = HashMap::new();
    }

    fn insert(&self, key: String, cp: TableCheckpoint) {
        // 这里保存深拷贝后的 checkpoint，
        // 避免调用方继续修改原对象造成共享状态混淆。
        self.checkpoints.write().unwrap().insert(key, cp);
    }

    fn update(&self, diffs: &HashMap<String, TableCheckpointDiff>) -> Vec<TotalWritten> {
        // 返回聚合后的 `TotalWritten`，
        // 这样调用方只需要一次写锁就能把写入字节数回填到表状态。
        let mut total_writtens = Vec::with_capacity(diffs.len());
        let mut map = self.checkpoints.write().unwrap();
        for (key, diff) in diffs {
            let cp = map
                .get_mut(key)
                .expect("checkpoint must exist for diff key");
            cp.Apply(diff);

            let mut tw: i64 = 0;
            for engine in cp.Engines.values() {
                for chunk in &engine.Chunks {
                    if engine.Status >= CheckpointStatusAllWritten {
                        // 引擎一旦进入“全部写完”状态，就按整块总大小计数。
                        tw += chunk.TotalSize();
                    } else {
                        // 未完成时只能按当前 offset 推进量估算，
                        // 这与 Go 版本展示的“已写入字节数”一致。
                        tw += chunk.Chunk.Offset - chunk.Key.Offset;
                    }
                }
            }
            total_writtens.push(TotalWritten {
                key: key.clone(),
                total_written: tw,
            });
        }
        total_writtens
    }

    fn marshal(&self, key: &str) -> Result<Vec<u8>> {
        // 表级 checkpoint 读取只暴露序列化结果，
        // 未命中则返回带 not_found 语义的错误，便于 HTTP 层区分。
        let map = self.checkpoints.read().unwrap();
        if let Some(cp) = map.get(key) {
            return marshal_table_checkpoint(cp);
        }
        Err(errors::NotFoundf(format!("table {key}")))
    }

    #[cfg(test)]
    fn contains(&self, key: &str) -> bool {
        self.checkpoints.read().unwrap().contains_key(key)
    }
}

// 这个轻量结果对象只在内部流转，
// 用于把 checkpoint 聚合结果传回任务快照。
struct TotalWritten {
    key: String,
    total_written: i64,
}

type TaskStatus = u8;

// 状态码沿用 Go 的裸 `uint8` 值，
// 对外 JSON 里也直接暴露这些数字。
const TASK_STATUS_RUNNING: TaskStatus = 1;
const TASK_STATUS_COMPLETED: TaskStatus = 2;

#[derive(Clone, Debug, Serialize)]
struct TableProgress {
    #[serde(rename = "step")]
    Step: String,
    #[serde(rename = "progress")]
    Progress: f64,
}

#[derive(Clone, Debug, Serialize)]
struct TableInfo {
    #[serde(rename = "w")]
    TotalWritten: i64,
    #[serde(rename = "z")]
    TotalSize: i64,
    #[serde(rename = "s")]
    Status: TaskStatus,
    #[serde(rename = "m", skip_serializing_if = "String::is_empty")]
    Message: String,
    #[serde(rename = "progresses", skip_serializing_if = "Vec::is_empty")]
    Progresses: Vec<TableProgress>,
}

// `TaskProgressGuarded` 承载真正受锁保护的任务可变状态，
// 把 checkpoint map 拆出去是为了复用其独立锁。
struct TaskProgressGuarded {
    Tables: Option<HashMap<String, TableInfo>>,
    Status: TaskStatus,
    Message: String,
}

struct TaskProgress {
    mu: RwLock<TaskProgressGuarded>,
    checkpoints: CheckpointsMap,
}

#[derive(Serialize)]
struct TaskProgressJson<'a> {
    #[serde(rename = "t")]
    Tables: Option<&'a HashMap<String, TableInfo>>,
    #[serde(rename = "s")]
    Status: TaskStatus,
    #[serde(rename = "m", skip_serializing_if = "str::is_empty")]
    Message: &'a str,
}

static CURRENT_PROGRESS: OnceLock<Mutex<Option<TaskProgress>>> = OnceLock::new();
static PROGRESS_ENABLED: AtomicBool = AtomicBool::new(false);

fn current() -> &'static Mutex<Option<TaskProgress>> {
    CURRENT_PROGRESS.get_or_init(|| Mutex::new(None))
}

/// EnableCurrentProgress init current progress struct on demand.
/// NOTE: this call is not thread safe, so it should only be inited once at the very beginning of progress start.
/// 启用动作会重建整个全局槽位，
/// 因此调用方应在任务生命周期开始前完成一次性初始化。
pub fn EnableCurrentProgress() {
    let mut slot = current().lock().unwrap();
    *slot = Some(TaskProgress {
        mu: RwLock::new(TaskProgressGuarded {
            Tables: None,
            Status: 0,
            Message: String::new(),
        }),
        checkpoints: CheckpointsMap::new(),
    });
    PROGRESS_ENABLED.store(true, Ordering::SeqCst);
}

/// BroadcastStartTask sets the current task status to running.
/// 开始任务只更新任务级状态，不会自动补建表列表，
/// 表信息仍由 `BroadcastInitProgress` 单独初始化。
pub fn BroadcastStartTask() {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    {
        let mut g = tp.mu.write().unwrap();
        g.Status = TASK_STATUS_RUNNING;
    }
    tp.checkpoints.clear();
}

/// BroadcastEndTask sets the current task status to completed.
/// 任务级错误消息是单独字段，
/// 即使部分表已写入自己的错误，也不会覆盖总任务消息的来源。
pub fn BroadcastEndTask(err: Option<&Error>) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let err_string = errors::ErrorStack(err);
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let mut g = tp.mu.write().unwrap();
    g.Status = TASK_STATUS_COMPLETED;
    g.Message = err_string;
}

/// BroadcastInitProgress sets the total size of each table.
/// 这里把 mydump 元数据展平成“唯一表名 -> 表进度”的 map，
/// 后续所有广播都以这个唯一表名作为索引键。
pub fn BroadcastInitProgress(databases: &[mydump::MDDatabaseMeta]) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let mut tables = HashMap::with_capacity(databases.len());
    for db in databases {
        for tbl in &db.Tables {
            let name = common::UniqueTable(&db.Name, &tbl.Name);
            tables.insert(
                name,
                TableInfo {
                    TotalWritten: 0,
                    TotalSize: tbl.TotalSize,
                    Status: 0,
                    Message: String::new(),
                    Progresses: Vec::new(),
                },
            );
        }
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let mut g = tp.mu.write().unwrap();
    g.Tables = Some(tables);
}

/// BroadcastTableCheckpoint updates the checkpoint of a table.
/// 表一旦收到 checkpoint，就被视为已经进入运行态，
/// 这与 Go 端监听 checkpoint 更新时的状态推进一致。
pub fn BroadcastTableCheckpoint(table_name: &str, cp: &TableCheckpoint) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    {
        let mut g = tp.mu.write().unwrap();
        let tables = g.Tables.as_mut().expect("tables must be initialized");
        tables.get_mut(table_name).expect("table must exist").Status = TASK_STATUS_RUNNING;
    }
    // create a deep copy to avoid false sharing
    tp.checkpoints.insert(table_name.to_string(), cp.DeepCopy());
}

/// BroadcastTableProgress updates the progress of a table.
/// 同一步骤重复上报时只更新已有项，
/// 避免 JSON 中为同一 `step` 累积重复条目。
pub fn BroadcastTableProgress(table_name: &str, step: &str, progress: f64) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let mut g = tp.mu.write().unwrap();
    let tables = g.Tables.as_mut().expect("tables must be initialized");
    let tbl = tables.get_mut(table_name).expect("table must exist");
    let progresses = &mut tbl.Progresses;
    let mut present = false;
    for p in progresses.iter_mut() {
        if p.Step == step {
            p.Progress = progress;
            present = true;
        }
    }
    if !present {
        progresses.push(TableProgress {
            Step: step.to_string(),
            Progress: progress,
        });
    }
}

/// BroadcastCheckpointDiff updates the total written size of each table.
/// 真正的写入字节数来自 checkpoint 聚合，
/// 这里不自行推导 chunk 细节，只消费聚合后的结果。
pub fn BroadcastCheckpointDiff(diffs: &HashMap<String, TableCheckpointDiff>) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let total_writtens = tp.checkpoints.update(diffs);
    let mut g = tp.mu.write().unwrap();
    let tables = g.Tables.as_mut().expect("tables must be initialized");
    for tw in total_writtens {
        tables
            .get_mut(&tw.key)
            .expect("table must exist")
            .TotalWritten = tw.total_written;
    }
}

/// BroadcastError sets the error message of a table.
/// 表级错误是幂等的 best-effort 更新，
/// 找不到表时直接忽略，匹配 Go 对 nil map entry 的处理。
pub fn BroadcastError(table_name: &str, err: Option<&Error>) {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let err_string = errors::ErrorStack(err);
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let mut g = tp.mu.write().unwrap();
    if let Some(tables) = g.Tables.as_mut() {
        if let Some(tbl) = tables.get_mut(table_name) {
            tbl.Status = TASK_STATUS_COMPLETED;
            tbl.Message = err_string;
        }
    }
}

/// MarshalTaskProgress returns the current progress in JSON format.
/// 对外 JSON 视图刻意走 `TaskProgressJson`，
/// 这样可以精确控制字段名和 `omitempty` 语义。
pub fn MarshalTaskProgress() -> Result<Vec<u8>> {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return Err(errors::New("progress is not enabled"));
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    let g = tp.mu.read().unwrap();
    let view = TaskProgressJson {
        Tables: g.Tables.as_ref(),
        Status: g.Status,
        Message: &g.Message,
    };
    serde_json::to_vec(&view).map_err(|e| Error::new(e.to_string()))
}

/// MarshalTableCheckpoints returns the checkpoint of a table in JSON format.
/// 表级 checkpoint 输出走专门序列化函数，
/// 因为 Rust 结构体字段名和 Go JSON 外观并不完全等价。
pub fn MarshalTableCheckpoints(table_name: &str) -> Result<Vec<u8>> {
    if !PROGRESS_ENABLED.load(Ordering::SeqCst) {
        return Err(errors::New("progress is not enabled"));
    }
    let slot = current().lock().unwrap();
    let tp = slot.as_ref().expect("progress enabled");
    tp.checkpoints.marshal(table_name)
}

fn marshal_table_checkpoint(cp: &TableCheckpoint) -> Result<Vec<u8>> {
    // Match Go encoding/json default field names for exported TableCheckpoint fields.
    // 这里手工组装 JSON，是为了输出与 Go 导出字段完全相同的名字和结构。
    let mut engines = serde_json::Map::new();
    for (id, engine) in &cp.Engines {
        let chunks: Vec<serde_json::Value> = engine
            .Chunks
            .iter()
            .map(|chunk| {
                serde_json::json!({
                    "Key": {
                        "Path": chunk.Key.Path,
                        "Offset": chunk.Key.Offset,
                    },
                    "FileMeta": {
                        "Path": chunk.FileMeta.Path,
                        "Type": chunk.FileMeta.Type.0,
                        "Compression": chunk.FileMeta.Compression.0,
                        "SortKey": chunk.FileMeta.SortKey,
                        "FileSize": chunk.FileMeta.FileSize,
                    },
                    "ColumnPermutation": chunk.ColumnPermutation,
                    "Chunk": {
                        "Offset": chunk.Chunk.Offset,
                        "RealOffset": chunk.Chunk.RealOffset,
                        "EndOffset": chunk.Chunk.EndOffset,
                        "PrevRowIDMax": chunk.Chunk.PrevRowIDMax,
                        "RowIDMax": chunk.Chunk.RowIDMax,
                    },
                    "Checksum": {},
                    "Timestamp": chunk.Timestamp,
                })
            })
            .collect();
        engines.insert(
            id.to_string(),
            serde_json::json!({
                "Status": engine.Status,
                "Chunks": chunks,
            }),
        );
    }
    let value = serde_json::json!({
        "Status": cp.Status,
        "Engines": engines,
        "TableID": cp.TableID,
        "TableInfo": null,
        "Checksum": {},
        "AutoRandBase": cp.AutoRandBase,
        "AutoIncrBase": cp.AutoIncrBase,
        "AutoRowIDBase": cp.AutoRowIDBase,
    });
    serde_json::to_vec(&value).map_err(|e| Error::new(e.to_string()))
}

#[cfg(test)]
pub(crate) fn reset_progress_for_test() {
    // 测试之间必须显式清空全局单例，
    // 否则前一个场景残留状态会污染下一个断言。
    PROGRESS_ENABLED.store(false, Ordering::SeqCst);
    *current().lock().unwrap() = None;
}

#[cfg(test)]
pub(crate) fn checkpoints_contains_for_test(key: &str) -> bool {
    let slot = current().lock().unwrap();
    slot.as_ref()
        .map(|tp| tp.checkpoints.contains(key))
        .unwrap_or(false)
}
