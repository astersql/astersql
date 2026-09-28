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

// 调度器状态（Scheduler Status）数据结构与 JSON 序列化。
//
// DXF（Distributed eXecution Framework，分布式执行框架）Owner 对外暴露的
// 状态快照：任务队列、TiDB/TiKV worker 节点组资源，以及带 TTL 的标志位
//（如暂停缩容 pause_scale_in）。序列化行为对齐 Go 的 json tag 与嵌入语义。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// 判断值是否等于 Default，供 serde skip_serializing_if 模拟 Go omitempty。
fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}

/// Go 的 time.Time 零值（公元前 1 年），用作 ExpireTime 默认。
fn go_zero_time() -> SystemTime {
    SystemTime::UNIX_EPOCH - Duration::from_secs(62_135_596_800)
}

/// Duration ↔ JSON 整数纳秒的 serde 适配（对齐 Go time.Duration）。
mod duration_nanoseconds {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use std::time::Duration;

    /// 将 Duration 序列化为 i64 纳秒。
    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let nanos = i64::try_from(value.as_nanos()).map_err(serde::ser::Error::custom)?;
        serializer.serialize_i64(nanos)
    }

    /// 从 i64 纳秒反序列化为 Duration；负值报错。
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let nanos = i64::deserialize(deserializer)?;
        let nanos = u64::try_from(nanos)
            .map_err(|_| D::Error::custom("negative Go duration cannot fit std::time::Duration"))?;
        Ok(Duration::from_nanos(nanos))
    }
}

/// SystemTime ↔ RFC3339 字符串的 serde 适配（对齐 Go time.Time JSON）。
mod system_time_rfc3339 {
    use super::*;
    use serde::{Deserializer, Serializer, de::Error};

    /// 将 SystemTime 序列化为 UTC RFC3339 字符串。
    pub fn serialize<S>(value: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value: DateTime<Utc> = (*value).into();
        serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::AutoSi, true))
    }

    /// 从 RFC3339 字符串反序列化为 SystemTime。
    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let value = DateTime::parse_from_rfc3339(&value).map_err(D::Error::custom)?;
        Ok(SystemTime::from(value.with_timezone(&Utc)))
    }
}

// Version represents the version of the scheduler status.
// Version 对应 Go 的 int 别名；用 i32 保留数值版本语义。
/// 调度器状态版本号类型（对应 Go 的 Version int 别名）。
pub type Version = i32;

// Version1 is the first version of the scheduler status.
/// 调度器状态第一版版本号。
pub const Version1: Version = 1;

// TaskQueue represents the status of a task queue in the scheduler.
// TaskQueue 对应 Go 的任务队列状态结构，字段顺序和 json tag 含义保持一致。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 任务队列状态：已调度任务数（不含短生命周期的 cancel/pause/resume）。
pub struct TaskQueue {
    // PendingCount is the number of tasks that are scheduled, it only includes
    // the number of tasks in running/modifying state.
    // cancelling , pausing and resuming tasks are also scheduled, but since they
    // mostly run in a short time, they will be excluded.
    // Go 字段名是 ScheduledCount，注释中的 PendingCount 是来源文件现状，迁移时不擅自修正。
    #[serde(rename = "scheduled_count", default, skip_serializing_if = "is_zero")]
    /// 处于 running/modifying 等已调度状态的任务数。
    pub ScheduledCount: i32,
}

// Node represents the status of a node.
// Node 保留 exec_id 与 owner 标记，供状态 JSON 中的 busy_nodes 使用。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 单个执行节点状态：exec_id 与是否为 DXF Owner。
pub struct Node {
    // ID is the unique identifier of the node, it's the same as the exec_id
    // field of mysql.tidb_background_subtask table.
    #[serde(rename = "id", default, skip_serializing_if = "String::is_empty")]
    /// 节点唯一标识，对应 mysql.tidb_background_subtask.exec_id。
    pub ID: String,
    // IsOwner indicates whether the node is the owner of DXF.
    #[serde(rename = "is_owner", default, skip_serializing_if = "is_zero")]
    /// 该节点是否为 DXF Owner（负责调度的主节点）。
    pub IsOwner: bool,
}

// NodeGroup represents the resource status of TiDB or TiKV worker node group.
// NodeGroup 对应 Go 的 worker 资源状态，包含当前节点、所需节点和忙碌节点列表。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// TiDB 或 TiKV worker 节点组的资源与忙碌节点列表。
pub struct NodeGroup {
    // CPUCount is the number of CPUs available for the node.
    #[serde(rename = "cpu_count", default, skip_serializing_if = "is_zero")]
    /// 节点可用 CPU 核数。
    pub CPUCount: i32,
    // RequiredCount is the required number of nodes to run the scheduled tasks.
    // physical cluster controller should try to scale in/out nodes to match
    // this number.
    #[serde(rename = "required_count", default, skip_serializing_if = "is_zero")]
    /// 满足已调度任务所需的节点数；物理集群控制器应按此扩缩容。
    pub RequiredCount: i32,
    // CurrentCount is the current number of nodes available, either idle or busy.
    #[serde(rename = "current_count", default, skip_serializing_if = "is_zero")]
    /// 当前可用节点数（空闲 + 忙碌）。
    pub CurrentCount: i32,
    // BusyNodes is the list of busy nodes, which are currently running subtasks.
    // Nodes in this list shouldn't be selected as the target node to be scaled in,
    // to avoid the subtask being rerun and enlarge its execution time.
    #[serde(rename = "busy_nodes", default, skip_serializing_if = "Vec::is_empty")]
    /// 正在跑子任务的忙碌节点；缩容时不应选中，以免子任务重跑拉长耗时。
    pub BusyNodes: Vec<Node>,
}

// Flag represents a flag in the scheduler.
// Flag 对应 Go 的 string 别名，当前只声明 pause_scale_in。
/// 调度器标志名类型（对应 Go 的 Flag string 别名）。
pub type Flag = String;

// PauseScaleInFlag is the flag to pause the scale-in action of the workers.
/// 暂停 worker 缩容的标志名，用于规避调度与缩容冲突。
pub const PauseScaleInFlag: &str = "pause_scale_in";

// TTLInfo represents the TTL info of a flag or resource tune factors in the
// scheduler.
// TTLInfo 对应 Go 的 TTLInfo；Duration/SystemTime 只是中的 time.Duration/time.Time 近似表达。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// 标志或调参因子的 TTL（Time To Live）信息：存活时长与过期时刻。
pub struct TTLInfo {
    #[serde(
        rename = "ttl",
        default,
        skip_serializing_if = "is_zero",
        with = "duration_nanoseconds"
    )]
    /// TTL 时长（JSON 以纳秒整数表示）。
    pub TTL: Duration,
    #[serde(
        rename = "expire_time",
        default = "go_zero_time",
        with = "system_time_rfc3339"
    )]
    /// 过期时间（JSON 为 RFC3339）。
    pub ExpireTime: SystemTime,
}

impl Default for TTLInfo {
    /// 默认 TTL=0、ExpireTime=Go 零时间。
    fn default() -> Self {
        Self {
            TTL: Duration::ZERO,
            ExpireTime: go_zero_time(),
        }
    }
}

// TTLFlag represents a flag with TTL in the scheduler.
// TTLFlag 对应 Go 的结构体嵌入：Enabled 加上 TTLInfo 的两个字段。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 带 TTL 的布尔标志；serde flatten 将 TTLInfo 字段展平到同级。
pub struct TTLFlag {
    #[serde(rename = "enabled", default, skip_serializing_if = "is_zero")]
    /// 标志是否启用。
    pub Enabled: bool,
    #[serde(flatten)]
    /// 嵌入的 TTL 信息。
    pub TTLInfo: TTLInfo,
}

impl TTLFlag {
    // String implements fmt.Stringer interface for TTLFlag.
    // String 对应 Go 的 json.Marshal(a)，忽略 marshal 错误并返回 JSON 文本。
    /// JSON 字符串化；忽略 marshal 错误（对齐 Go）。
    pub fn String(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// Status represents the status of the scheduler.
// Status is the scheduler status API response; Flags holds flags like PauseScaleInFlag.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 调度器完整状态：版本、任务队列、TiDB/TiKV worker 与标志表。
pub struct Status {
    #[serde(rename = "version", default, skip_serializing_if = "is_zero")]
    /// 状态 schema 版本。
    pub Version: Version,
    #[serde(rename = "task_queue", default)]
    /// 任务队列摘要。
    pub TaskQueue: TaskQueue,
    #[serde(rename = "tidb_worker", default)]
    /// TiDB worker 节点组状态。
    pub TiDBWorker: NodeGroup,
    #[serde(rename = "tikv_worker", default)]
    /// TiKV worker 节点组状态。
    pub TiKVWorker: NodeGroup,
    // Flags is a map of flags, we only have one type of flag right now.
    // PauseScaleInFlag is the flag to notify the cluster controller to pause the
    // scale-in action of the workers. as the schedule and scale-in/out operations
    // are run asynchronously, when there are multiple tasks running in parallel,
    // it's possible that some subtask keeps being scheduled to run on a worker
    // that is being scaled in, which will cause the subtask repeatedly being
    // balanced to other workers, i.e., scale-in/schedule conflict issue.
    // although this issue will disappear when there is no node to be scaled in,
    // it might make the task take a long time to finish, so if we meet this issue
    // we can use this flag to workaround.
    #[serde(rename = "flags", default, skip_serializing_if = "HashMap::is_empty")]
    /// 标志映射；当前主要为 PauseScaleInFlag，缓解并行任务下的缩容/调度冲突。
    pub Flags: HashMap<Flag, TTLFlag>,
}

impl Status {
    // String implements fmt.Stringer interface for Status.
    // String 对应 Go 的状态字符串化：复制一份 bak，避免为了日志输出修改原 Status。
    /// 日志友好的 JSON 字符串：复制后截断 BusyNodes，避免修改原 Status。
    pub fn String(&self) -> String {
        let mut bak = self.clone();
        if bak.TiDBWorker.BusyNodes.len() > 5 {
            // Go 只截断 TiDBWorker.BusyNodes，并追加一个说明节点展示总数。
            let total = self.TiDBWorker.BusyNodes.len();
            bak.TiDBWorker.BusyNodes.truncate(5);
            bak.TiDBWorker.BusyNodes.push(Node {
                ID: format!("... too many nodes, total {} busy nodes ...", total),
                IsOwner: false,
            });
        }
        // Go 忽略 json.Marshal 错误；保留相同的容错返回。
        serde_json::to_string(&bak).unwrap_or_default()
    }
}
