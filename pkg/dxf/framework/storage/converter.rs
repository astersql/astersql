// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 任务/子任务表行到 proto 结构的列映射转换。
//
// 对应 Go 的 converter.go：按固定列下标把 chunk::Row 转为 TaskBase / Task /
// SubtaskBase / Subtask；未知枚举值通过字符串 intern 保留，解析失败仅记日志。

// 从 pkg/dxf/framework/storage/converter.go 迁移，保持固定列位与容错行为一致。
//

/// 将字符串泄漏到进程内静态表，得到 'static 引用（模拟 Go 字符串驻留）。
pub(crate) fn intern(value: String) -> &'static str {
    static INTERNED: std::sync::OnceLock<
        std::sync::RwLock<std::collections::HashMap<String, &'static str>>,
    > = std::sync::OnceLock::new();
    let interned =
        INTERNED.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    if let Some(value) = interned
        .read()
        .expect("interned string lock poisoned")
        .get(&value)
        .copied()
    {
        return value;
    }
    // 双重检查：读锁未命中后再写锁插入，避免并发重复泄漏。
    let mut values = interned.write().expect("interned string lock poisoned");
    if let Some(value) = values.get(&value).copied() {
        return value;
    }
    let leaked = Box::leak(value.clone().into_boxed_str());
    values.insert(value, leaked);
    leaked
}

/// 已知任务类型映射为常量；未知类型 intern 后原样返回。
fn task_type(value: String) -> proto::TaskType {
    match value.as_str() {
        "Example" => proto::TaskTypeExample,
        "ImportInto" => proto::ImportInto,
        "backfill" => proto::Backfill,
        _ => intern(value),
    }
}

/// 已知任务状态映射为常量；未知状态 intern 后原样返回。
fn task_state(value: String) -> proto::TaskState {
    match value.as_str() {
        "pending" => proto::TaskStatePending,
        "running" => proto::TaskStateRunning,
        "succeed" => proto::TaskStateSucceed,
        "failed" => proto::TaskStateFailed,
        "reverting" => proto::TaskStateReverting,
        "awaiting-resolution" => proto::TaskStateAwaitingResolution,
        "reverted" => proto::TaskStateReverted,
        "cancelling" => proto::TaskStateCancelling,
        "pausing" => proto::TaskStatePausing,
        "paused" => proto::TaskStatePaused,
        "resuming" => proto::TaskStateResuming,
        "modifying" => proto::TaskStateModifying,
        _ => intern(value),
    }
}

/// 已知子任务状态映射为常量；未知状态 intern 后原样返回。
fn subtask_state(value: String) -> proto::SubtaskState {
    match value.as_str() {
        "pending" => proto::SubtaskStatePending,
        "running" => proto::SubtaskStateRunning,
        "succeed" => proto::SubtaskStateSucceed,
        "failed" => proto::SubtaskStateFailed,
        "canceled" => proto::SubtaskStateCanceled,
        "paused" => proto::SubtaskStatePaused,
        _ => intern(value),
    }
}

/// 解析任务 modify_params JSON 为 ModifyParam（prev_state + modifications）。
fn parse_modify_param(bytes: &[u8]) -> Result<proto::ModifyParam, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let prev_state = value
        .get("prev_state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let modifications = value
        .get("modifications")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|modification| proto::Modification {
            Type: intern(
                modification
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ),
            To: modification
                .get("to")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or_default(),
        })
        .collect();
    Ok(proto::ModifyParam {
        PrevState: task_state(prev_state.to_owned()),
        Modifications: modifications,
    })
}

/// Decode either task error column using Go's normalized-error fallback.
fn row2TaskError(r: &chunk::Row, index: usize) -> Option<Error> {
    if r.IsNull(index) {
        return None;
    }
    let bytes = r.GetBytes(index);
    #[derive(Default, Deserialize)]
    struct Fields {
        #[serde(default, alias = "Class")]
        class: Option<i32>,
        #[serde(default, alias = "Code")]
        code: Option<i32>,
        #[serde(default, alias = "Message")]
        message: Option<String>,
        #[serde(default, alias = "RFCCode")]
        rfccode: Option<String>,
    }
    let decoded = serde_json::from_slice::<serde_json::Value>(&bytes).and_then(|value| {
        if !value.is_object() && !value.is_null() {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "task error must be an object",
            ));
        }
        serde_json::from_value::<Option<Fields>>(value)
    });
    let error = match decoded {
        Ok(fields) => {
            let fields = fields.unwrap_or_default();
            let mut error = Error::static_new("");
            error.class = fields.class.unwrap_or_default();
            error.code = fields.code.unwrap_or_default();
            let mut rfc = fields.rfccode.unwrap_or_default();
            if rfc.is_empty() && error.class > 0 {
                // Legacy TiDB terror classes used by pingcap/errors.UnmarshalJSON.
                const CLASSES: [&str; 28] = [
                    "",
                    "autoid",
                    "ddl",
                    "domain",
                    "evaluator",
                    "executor",
                    "expression",
                    "admin",
                    "kv",
                    "meta",
                    "planner",
                    "parser",
                    "perfschema",
                    "privilege",
                    "schema",
                    "server",
                    "struct",
                    "variable",
                    "xeval",
                    "table",
                    "types",
                    "global",
                    "mocktikv",
                    "json",
                    "tikv",
                    "session",
                    "plugin",
                    "util",
                ];
                let class = CLASSES
                    .get(error.class as usize)
                    .copied()
                    .unwrap_or_default();
                rfc = format!("{class}:{}", error.code);
            }
            let display_code = if rfc.is_empty() {
                error.code.to_string()
            } else {
                rfc.clone()
            };
            error.message = intern(format!(
                "[{display_code}]{}",
                fields.message.unwrap_or_default()
            ));
            error.rfccode = intern(rfc);
            error
        }
        Err(cause) => {
            eprintln!("unmarshal task error: {cause}");
            Error::new(String::from_utf8_lossy(&bytes))
        }
    };
    Some(error)
}

// row2TaskBasic 对应 Go 的内部转换函数，把 task 表基础列转换成 proto.TaskBase。
// 列下标完全沿用 Go 代码：0-11 分别映射 ID、Key、Type、State、Step 等字段。
/// 将 task 表基础列（下标 0–11）转为 proto::TaskBase。
pub fn row2TaskBasic(r: chunk::Row) -> proto::TaskBase {
    let createTime = r.GetTime(7).GoTime(time::Local).0;
    let mut extraParams = proto::ExtraParams::default();
    if !r.IsNull(10) {
        let str_value = r.GetJSON(10).String();
        // Go 解析 extra params 失败只记录错误，不阻断 row 转换；这里保留同样的容错策略。
        if let Err(err) =
            serde_json::from_str::<proto::ExtraParams>(&str_value).map(|value| extraParams = value)
        {
            eprintln!("unmarshal task extra params: {err}");
        }
    }
    let task = proto::TaskBase {
        ID: r.GetInt64(0),
        Key: r.GetString(1),
        Type: task_type(r.GetString(2)),
        State: task_state(r.GetString(3)),
        Step: r.GetInt64(4),
        Priority: r.GetInt64(5) as i32,
        RequiredSlots: r.GetInt64(6) as i32,
        CreateTime: createTime,
        TargetScope: r.GetString(8),
        MaxNodeCount: r.GetInt64(9) as i32,
        ExtraParams: extraParams,
        Keyspace: r.GetString(11),
    };
    task
}

// Row2Task converts a row to a task.
// Row2Task 对应 Go 的导出转换函数：先构造 TaskBase，再补 start/update/meta/scheduler/error/modify 字段。
/// 将完整 task 行转为 proto::Task（含 start/update/meta/scheduler/error/modify）。
pub fn Row2Task(r: chunk::Row) -> proto::Task {
    let taskBase = row2TaskBasic(r.clone());
    let mut task = proto::Task {
        TaskBase: taskBase,
        SchedulerID: String::new(),
        StartTime: std::time::UNIX_EPOCH,
        StateUpdateTime: std::time::UNIX_EPOCH,
        Meta: Vec::new(),
        Error: None,
        ModifyParam: proto::ModifyParam {
            PrevState: "",
            Modifications: Vec::new(),
        },
    };
    let mut startTime = std::time::UNIX_EPOCH;
    let mut updateTime = std::time::UNIX_EPOCH;
    if !r.IsNull(12) {
        startTime = r.GetTime(12).GoTime(time::Local).0;
    }
    if !r.IsNull(13) {
        updateTime = r.GetTime(13).GoTime(time::Local).0;
    }
    task.StartTime = startTime;
    task.StateUpdateTime = updateTime;
    task.Meta = r.GetBytes(14);
    task.SchedulerID = r.GetString(15);
    task.Error = row2TaskError(&r, 16).map(|error| error.to_string());
    if !r.IsNull(17) {
        let str_value = r.GetJSON(17).String();
        // modify param 解析失败同样只记日志，不影响 Task 返回。
        match parse_modify_param(str_value.as_bytes()) {
            Ok(param) => task.ModifyParam = param,
            Err(err) => eprintln!("unmarshal task modify param: {err}"),
        }
    }
    task
}

// row2BasicSubTask converts a row to a subtask with basic info
// row2BasicSubTask 对应 Go 的内部转换函数，把 subtask 表基础列转换成 proto.SubtaskBase。
/// 将 subtask 表基础列转为 proto::SubtaskBase（含 bigint 秒级 start_time）。
pub fn row2BasicSubTask(r: chunk::Row) -> proto::SubtaskBase {
    let taskIDStr = r.GetString(2);
    let tid = match taskIDStr.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            // Go 只 warn unexpected subtask id，tid 保持零值；这里保留这个宽松解析行为。
            eprintln!("unexpected subtask id: {taskIDStr}");
            0
        }
    };
    let createTime = r.GetTime(7).GoTime(time::Local).0;
    let mut ordinal = 0;
    if !r.IsNull(8) {
        ordinal = r.GetInt64(8) as i32;
    }

    // subtask defines start time as bigint, to ensure backward compatible,
    // we keep it that way, and we convert it here.
    let mut startTime = std::time::UNIX_EPOCH;
    if !r.IsNull(9) {
        let ts = r.GetInt64(9);
        // Go 使用 time.Unix(ts, 0) 把 bigint 秒级时间转为 time.Time。
        startTime = time::Unix(ts, 0);
    }

    let subtask = proto::SubtaskBase {
        ID: r.GetInt64(0),
        Step: r.GetInt64(1),
        TaskID: tid,
        Type: proto::Int2Type(r.GetInt64(3) as i32),
        ExecID: r.GetString(4),
        State: subtask_state(r.GetString(5)),
        Concurrency: r.GetInt64(6) as i32,
        CreateTime: createTime,
        Ordinal: ordinal,
        StartTime: startTime,
    };
    subtask
}

// Row2SubTask converts a row to a subtask.
// Row2SubTask 对应 Go 的导出转换函数：基于 SubtaskBase 再填充更新时间、meta 和 summary。
/// 将完整 subtask 行转为 proto::Subtask（含 update_time/meta/summary）。
pub fn Row2SubTask(r: chunk::Row) -> proto::Subtask {
    let mut subtask = proto::Subtask {
        SubtaskBase: row2BasicSubTask(r.clone()),
        UpdateTime: std::time::UNIX_EPOCH,
        Meta: Vec::new(),
        Summary: String::new(),
    };

    // subtask defines update time as bigint, to ensure backward compatible,
    // we keep it that way, and we convert it here.
    let mut updateTime = std::time::UNIX_EPOCH;
    if !r.IsNull(10) {
        let ts = r.GetInt64(10);
        // 与 startTime 一样，这里保留 bigint 秒级时间到 time.Time 的兼容转换。
        updateTime = time::Unix(ts, 0);
    }

    subtask.UpdateTime = updateTime;
    subtask.Meta = r.GetBytes(11);
    subtask.Summary = r.GetJSON(12).String();
    subtask
}
