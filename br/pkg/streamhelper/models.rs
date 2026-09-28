// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! 流备份任务在 etcd 上的键路径与任务模型；对齐 Go `models.go`。
//!
//! 路径常量决定 PD/etcd 元数据布局；`TaskInfo` 以建造者模式组装 protobuf 任务，
//! 在写入前经 `Check` 校验存储、表过滤与任务名约束。

use std::sync::LazyLock;

use regex::Regex;

use crate::stubs::{KeyRange, StorageBackend, StreamBackupTaskInfo};

/// 流备份相关键的公共前缀，对应 Go `streamKeyPrefix`。
pub const streamKeyPrefix: &str = "/tidb/br-stream";
/// 任务信息子路径：`<prefix>/info/<name>`。
pub const taskInfoPath: &str = "/info";
/// 检查点子路径：`<prefix>/checkpoint/<task>/...`。
pub const taskCheckpointPath: &str = "/checkpoint";
/// 外部存储侧检查点路径前缀。
pub const storageCheckPoint: &str = "/storage-checkpoint";
/// 任务表范围键前缀；尾斜杠用于避免同前缀任务误扫。
pub const taskRangesPath: &str = "/ranges";
/// 暂停标记路径。
pub const taskPausePath: &str = "/pause";
/// 最近错误路径。
pub const taskLastErrorPath: &str = "/last-error";
/// 全局（central）检查点类型名，与 Go 常量一致。
pub const checkpointTypeGlobal: &str = "central_global";
/// Region 级检查点类型名。
pub const checkpointTypeRegion: &str = "region";
/// Store 级检查点类型名。
pub const checkpointTypeStore: &str = "store";

/// 任务名仅允许字母数字与下划线，对齐 Go `taskNameRe`。
static TASK_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-zA-Z_]+$").expect("task name re"));

/// 模拟 Go `path.Join`：清洗空段/`./`/`..`，首段以 `/` 开头则保留绝对路径。
fn path_join(parts: &[&str]) -> String {
    // Match Go path.Join: clean slashes, no trailing slash unless root-like.
    let mut segs = Vec::new();
    for p in parts {
        for s in p.split('/') {
            if s.is_empty() || s == "." {
                continue;
            }
            if s == ".." {
                segs.pop();
                continue;
            }
            segs.push(s);
        }
    }
    if parts.first().map(|p| p.starts_with('/')).unwrap_or(false) {
        format!("/{}", segs.join("/"))
    } else {
        segs.join("/")
    }
}

/// 所有任务信息键的公共前缀，形如 `<prefix>/info/`。
pub fn PrefixOfTask() -> String {
    format!("{}/", path_join(&[streamKeyPrefix, taskInfoPath]))
}

/// 单个任务信息键：`<prefix>/info/<name>` → protobuf 任务。
pub fn TaskOf(name: &str) -> String {
    path_join(&[streamKeyPrefix, taskInfoPath, name])
}

/// 某任务 ranges 前缀；必须带尾 `/`，否则会扫到同前缀的其他任务。
pub fn RangesOf(name: &str) -> String {
    format!("{}/", path_join(&[streamKeyPrefix, taskRangesPath, name]))
}

/// 某任务一条 range 的键；startKey 是任意二进制，必须逐字节保留且不能走 `path.Join`。
pub fn RangeKeyOf(name: &str, startKey: &[u8]) -> Vec<u8> {
    let mut key = RangesOf(name).into_bytes();
    key.extend_from_slice(startKey);
    key
}

/// 大端编码 u64，用于检查点时间戳等二进制值。
pub fn encodeUint64(num: u64) -> Vec<u8> {
    num.to_be_bytes().to_vec()
}

/// 任务检查点前缀，保证恰好一个尾 `/`，与 Go `CheckPointsOf` 一致。
pub fn CheckPointsOf(task: &str) -> String {
    let mut s = path_join(&[streamKeyPrefix, taskCheckpointPath, task]);
    while s.ends_with('/') {
        s.pop();
    }
    format!("{s}/")
}

/// 任务全局检查点键：`.../checkpoint/<task>/central_global`。
pub fn GlobalCheckpointOf(task: &str) -> String {
    path_join(&[
        streamKeyPrefix,
        taskCheckpointPath,
        task,
        checkpointTypeGlobal,
    ])
}

/// 外部存储检查点键路径。
pub fn StorageCheckpointOf(task: &str) -> String {
    path_join(&[streamKeyPrefix, storageCheckPoint, task])
}

/// 任务暂停标记键。
pub fn Pause(task: &str) -> String {
    path_join(&[streamKeyPrefix, taskPausePath, task])
}

/// 全部暂停键的前缀（带尾 `/`）。
pub fn PrefixOfPause() -> String {
    format!("{}/", path_join(&[streamKeyPrefix, taskPausePath]))
}

/// 任务最近错误前缀，同样规范化尾 `/`。
pub fn LastErrorPrefixOf(task: &str) -> String {
    let mut s = path_join(&[streamKeyPrefix, taskLastErrorPath, task]);
    while s.ends_with('/') {
        s.pop();
    }
    format!("{s}/")
}

/// 备份范围列表别名，对齐 Go `Ranges`。
pub type Ranges = Vec<KeyRange>;
/// 单条键范围别名。
pub type Range = KeyRange;

/// 流备份任务的本地视图：protobuf 信息 + ranges + 暂停态。
#[derive(Clone, Debug, Default)]
pub struct TaskInfo {
    pub PBInfo: StreamBackupTaskInfo,
    pub Ranges: Ranges,
    pub Pausing: bool,
}

/// 以任务名创建空 `TaskInfo`，后续用建造者方法填充。
pub fn NewTaskInfo(name: &str) -> TaskInfo {
    TaskInfo {
        PBInfo: StreamBackupTaskInfo {
            Name: name.to_string(),
            ..Default::default()
        },
        Ranges: Vec::new(),
        Pausing: false,
    }
}

impl TaskInfo {
    /// 追加一条 `[startKey, endKey)` 备份范围。
    pub fn WithRange(mut self, startKey: &[u8], endKey: &[u8]) -> Self {
        self.Ranges.push(KeyRange {
            StartKey: startKey.to_vec(),
            EndKey: endKey.to_vec(),
        });
        self
    }

    /// 批量追加 ranges。
    pub fn WithRanges(mut self, ranges: &[KeyRange]) -> Self {
        self.Ranges.extend_from_slice(ranges);
        self
    }

    /// 设置备份起始 TS（含）。
    pub fn FromTS(mut self, ts: u64) -> Self {
        self.PBInfo.StartTs = ts;
        self
    }

    /// 设置备份结束 TS（可为空表示持续流）。
    pub fn UntilTS(mut self, ts: u64) -> Self {
        self.PBInfo.EndTs = ts;
        self
    }

    /// 设置表过滤器链；空链会在 `Check` 失败。
    pub fn WithTableFilter(mut self, filterChain: &[&str]) -> Self {
        self.PBInfo.TableFilter = filterChain.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// 绑定外部存储后端；`Check` 要求非空。
    pub fn ToStorage(mut self, backend: StorageBackend) -> Self {
        self.PBInfo.Storage = Some(backend);
        self
    }

    /// 写入前校验：必须有存储、非空表过滤、合法任务名；错误文案对齐 Go。
    pub fn Check(self) -> Result<Self, String> {
        if self.PBInfo.Storage.is_none() {
            return Err("the storage backend is null".into());
        }
        if self.PBInfo.TableFilter.is_empty() {
            return Err(
                "the table filter is empty, maybe add '*.*' for including all tables".into(),
            );
        }
        if !TASK_NAME_RE.is_match(&self.PBInfo.Name) {
            return Err(format!(
                "the task name can contain alphanumeric characters and underscore only (re = {}, name = {})",
                TASK_NAME_RE.as_str(),
                self.PBInfo.Name
            ));
        }
        Ok(self)
    }
}
