// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! 日志备份元数据客户端：在 etcd 上管理任务、暂停信息与检查点键。
//! 与 Go `client.go` 对齐；PauseV2 载荷支持纯文本与 StreamBackupError protobuf。
//! 检查点键按 Store/Region/Global/Task 多种前缀解析。
//! PutTask/DeleteTask 保持任务、range、pause、checkpoint、last-error 键空间一致。
//! PutTask 的 Pausing 标志保留 Go 旧版空 Pause 标记，状态查询按键存在性判断。
//! GetAllTasksWithRevision 转发底层线性化扫描 revision，供上层无缝衔接 watch。
//! PauseV2 的 PayloadType 采用 MIME 风格，参数 `messagetype` 指定 protobuf 消息。
//! 旧版检查点键在无类型前缀时按 8 字节大端 store id 解析，兼容历史数据。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::{
    CheckPointsOf, GlobalCheckpointOf, LastErrorPrefixOf, Pause, PrefixOfTask, RangeKeyOf,
    RangesOf, StorageCheckpointOf, TaskInfo, TaskOf, checkpointTypeGlobal, checkpointTypeRegion,
    checkpointTypeStore,
};
use crate::stubs::{EtcdKV, StreamBackupError, StreamBackupTaskInfo};

/// 暂停严重级别：自动错误触发。
pub const SeverityError: &str = "ERROR";
/// 暂停严重级别：人工操作触发。
pub const SeverityManual: &str = "MANUAL";

/// RFC3339 风格时间包装；序列化为字符串存入 PauseV2。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RFC3339Time(pub String);

impl RFC3339Time {
    /// 生成近似 RFC3339 UTC 字符串（无 chrono 依赖的轻量实现）。
    pub fn now() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days = (secs / 86_400) as i64;
        let (year, month, day) = civil_from_days(days);
        let seconds = secs % 86_400;
        Self(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        ))
    }

    /// 宽松校验：长度与含 `T` 即接受，保留原串。
    pub fn Parse(s: &str) -> Result<Self, String> {
        if !valid_rfc3339(s) {
            return Err(format!(
                "RFC3339Time: the data isn't a valid RFC3339 time ({s})"
            ));
        }
        Ok(Self(s.to_string()))
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn valid_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let parse = |range: std::ops::Range<usize>| s[range].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse(0..4),
        parse(5..7),
        parse(8..10),
        parse(11..13),
        parse(14..16),
        parse(17..19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day < 1
        || day > mdays[(month - 1) as usize]
        || hour >= 24
        || minute >= 60
        || second >= 60
    {
        return false;
    }
    let mut suffix = &s[19..];
    if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        suffix = &fraction[digits..];
    }
    if suffix == "Z" {
        return true;
    }
    let zone = suffix.as_bytes();
    if zone.len() != 6 || !matches!(zone[0], b'+' | b'-') || zone[3] != b':' {
        return false;
    }
    let (Ok(zone_hour), Ok(zone_minute)) =
        (suffix[1..3].parse::<u32>(), suffix[4..6].parse::<u32>())
    else {
        return false;
    };
    zone_hour < 24 && zone_minute < 60
}

impl Serialize for RFC3339Time {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RFC3339Time {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        RFC3339Time::Parse(&s).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for RFC3339Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 暂停记录 V2：操作者、时间、严重级别与可选载荷。
/// 写入 etcd 的 Pause 键；Resume 时删除该键。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PauseV2 {
    /// ERROR 或 MANUAL。
    pub Severity: String,
    #[serde(rename = "operation_hostname")]
    /// 发起暂停的主机名。
    pub OperatorHostName: String,
    #[serde(rename = "operation_pid")]
    /// 发起进程 PID。
    pub OperatorPID: i32,
    #[serde(rename = "operation_time")]
    /// 操作时间。
    pub OperationTime: RFC3339Time,
    #[serde(default)]
    /// MIME 风格载荷类型（text/plain 或 x-protobuf）。
    pub PayloadType: String,
    #[serde(default)]
    /// 载荷字节；类型由 PayloadType 解释。
    pub Payload: Vec<u8>,
}

/// 构造本机人工暂停记录（Severity=MANUAL）。
pub fn NewLocalPauseV2() -> PauseV2 {
    PauseV2 {
        Severity: SeverityManual.to_string(),
        OperatorHostName: hostname(),
        OperatorPID: std::process::id() as i32,
        OperationTime: RFC3339Time::now(),
        PayloadType: String::new(),
        Payload: Vec::new(),
    }
}

/// 读取 HOSTNAME 环境变量，缺省为 localhost。
fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into())
}

/// 暂停载荷：纯文本说明或结构化流备份错误。
pub enum PausePayload {
    /// 人类可读说明。
    Text(String),
    /// 结构化备份错误（错误码与消息）。
    StreamErr(StreamBackupError),
}

impl PauseV2 {
    /// 按 PayloadType 解码载荷；protobuf 仅支持 brpb.StreamBackupError。
    pub fn GetPayload(&self) -> Result<PausePayload, String> {
        let (m, params) = parse_media_type(&self.PayloadType)?;
        match m.as_str() {
            "text/plain" => Ok(PausePayload::Text(
                String::from_utf8_lossy(&self.Payload).into_owned(),
            )),
            "application/x-protobuf" => {
                let msg_type = params
                    .iter()
                    .find(|(k, _)| k == "messagetype")
                    .map(|(_, v)| v.as_str())
                    .ok_or_else(|| {
                        format!("x-protobuf didn't specified msgType ({})", self.PayloadType)
                    })?;
                // Go 侧同样只接受 StreamBackupError 消息类型。
                if msg_type != "brpb.StreamBackupError" {
                    return Err(format!(
                        "only type brpb.StreamBackupError is supported ({})",
                        self.PayloadType
                    ));
                }
                Ok(PausePayload::StreamErr(StreamBackupError::Unmarshal(
                    &self.Payload,
                )?))
            }
            _ => Err(format!("unsupported payload type {m}")),
        }
    }

    /// 以键值对形式展示暂停详情，供 CLI/日志表格输出。
    pub fn DisplayTable<F: FnMut(&str, &str)>(&self, mut display: F) {
        display("pause-time", &self.OperationTime.to_string());
        display("pause-operator", &self.OperatorHostName);
        display("pause-operator-pid", &self.OperatorPID.to_string());
        match self.GetPayload() {
            Err(e) => display("pause-payload[errparse]", &e),
            Ok(PausePayload::Text(t)) => display("pause-payload", &t),
            Ok(PausePayload::StreamErr(e)) => {
                display("pause-payload[errcode]", &e.ErrorCode);
                display("pause-payload[errmesg]", &e.ErrorMessage);
            }
        }
    }

    /// 设置 UTF-8 纯文本暂停说明。
    pub fn SetTextMessage(&mut self, t: &str) {
        self.PayloadType = "text/plain;charset=UTF-8".into();
        self.Payload = t.as_bytes().to_vec();
    }

    /// 序列化流备份错误为 protobuf 载荷（函数名拼写保留与 Go 一致）。
    pub fn SetBakcupStreamError(&mut self, berr: &StreamBackupError) -> Result<(), String> {
        self.PayloadType = "application/x-protobuf;messagetype=brpb.StreamBackupError".into();
        self.Payload = berr.Marshal()?;
        Ok(())
    }
}

/// 拆分 MIME 主类型与分号参数列表。
fn parse_media_type(s: &str) -> Result<(String, Vec<(String, String)>), String> {
    if s.is_empty() {
        return Err(format!("{s} isn't a valid mime type"));
    }
    let mut parts = s.split(';');
    let main = parts
        .next()
        .ok_or_else(|| format!("{s} isn't a valid mime type"))?
        .trim()
        .to_string();
    // 主类型必须含 `/`，例如 text/plain。
    if !main.contains('/') {
        return Err(format!("{s} isn't a valid mime type"));
    }
    let mut params = Vec::new();
    for p in parts {
        let p = p.trim();
        if let Some((k, v)) = p.split_once('=') {
            params.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok((main, params))
}

/// etcd 中单个检查点条目：Store/Region/Task/Global 由字段组合判定。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    #[serde(default, skip_serializing_if = "is_zero")]
    /// Store 或 Region ID；Task 级为 0。
    pub ID: u64,
    #[serde(default, rename = "epoch_version", skip_serializing_if = "is_zero")]
    /// Region epoch；非 Region 检查点为 0。
    pub Version: u64,
    /// 检查点 TSO（大端 8 字节解码）。
    pub TS: u64,
    #[serde(skip)]
    /// 是否为全局检查点键。
    pub IsGlobal: bool,
}

/// serde 辅助：零值字段在序列化时省略。
fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// 检查点层级分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointType {
    /// Store 级检查点。
    Store,
    /// Region 级检查点（含 epoch）。
    Region,
    /// 任务级检查点（无 ID）。
    Task,
    /// 全局检查点键。
    Global,
    /// 无法归类的非法组合。
    Invalid,
}

impl Checkpoint {
    /// 按 ID/Version/IsGlobal 推断检查点类型。
    pub fn Type(&self) -> CheckpointType {
        match () {
            _ if self.IsGlobal => CheckpointType::Global,
            _ if self.ID == 0 && self.Version == 0 => CheckpointType::Task,
            _ if self.ID != 0 && self.Version == 0 => CheckpointType::Store,
            _ if self.ID != 0 && self.Version != 0 => CheckpointType::Region,
            _ => CheckpointType::Invalid,
        }
    }
}

/// 解析检查点 etcd 键值：剥离任务前缀后按 store/region/global/legacy 分支解码。
/// value 必须为 8 字节大端 TSO。
pub fn ParseCheckpoint(task: &str, key: &[u8], value: &[u8]) -> Result<Checkpoint, String> {
    let pfx = CheckPointsOf(task).into_bytes();
    if !key.starts_with(&pfx) {
        return Err(format!(
            "the prefix is wrong for key: {}",
            String::from_utf8_lossy(key)
        ));
    }
    let key = &key[pfx.len()..];
    let segs: Vec<&[u8]> = key.split(|b| *b == b'/').collect();
    let mut checkpoint = Checkpoint::default();
    match std::str::from_utf8(segs[0]).unwrap_or("") {
        s if s == checkpointTypeStore => {
            // store/<store-id>
            if segs.len() != 2 {
                return Err(format!(
                    "the store checkpoint seg mismatch; segs = {:?}",
                    segs.iter()
                        .map(|s| String::from_utf8_lossy(s))
                        .collect::<Vec<_>>()
                ));
            }
            checkpoint.ID = std::str::from_utf8(segs[1])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "bad store id".to_string())?;
        }
        s if s == checkpointTypeGlobal => {
            // global：仅标记 IsGlobal
            checkpoint.IsGlobal = true;
        }
        s if s == checkpointTypeRegion => {
            // region/<id>/<epoch>
            if segs.len() != 3 {
                return Err(format!(
                    "the region checkpoint seg mismatch; segs = {:?}",
                    segs.iter()
                        .map(|s| String::from_utf8_lossy(s))
                        .collect::<Vec<_>>()
                ));
            }
            checkpoint.ID = std::str::from_utf8(segs[1])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "bad region id".to_string())?;
            checkpoint.Version = std::str::from_utf8(segs[2])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "bad region version".to_string())?;
        }
        _ => {
            // 旧版：剩余键为 8 字节大端 store id
            if key.len() != 8 {
                return Err(format!(
                    "the store id isn't 64bits (it is {} bytes, value = {:?})",
                    key.len(),
                    key
                ));
            }
            let mut buf = [0u8; 8];
            buf.copy_from_slice(key);
            checkpoint.ID = u64::from_be_bytes(buf);
        }
    }
    if value.len() != 8 {
        return Err(format!(
            "the checkpoint value isn't 64bits (it is {} bytes)",
            value.len()
        ));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(value);
    checkpoint.TS = u64::from_be_bytes(buf);
    Ok(checkpoint)
}

/// 暂停任务时可选的 PauseV2 修改闭包。
pub type PauseTaskOption = Box<dyn FnMut(&mut PauseV2) + Send>;

/// 构造附带文本消息的暂停选项。
pub fn PauseWithMessage(m: String) -> PauseTaskOption {
    Box::new(move |p: &mut PauseV2| p.SetTextMessage(&m))
}

/// 将暂停严重级别设为错误，与 Go `PauseWithErrorSeverity` 一致。
pub fn PauseWithErrorSeverity(p: &mut PauseV2) {
    p.Severity = SeverityError.to_string();
}

/// 面向 etcd 的元数据客户端；封装任务 CRUD 与暂停操作。
#[derive(Clone)]
pub struct MetaDataClient {
    /// 底层 KV（生产为 etcd，测试为内存桩）。
    pub KV: Arc<dyn EtcdKV>,
}

/// 包装 KV 句柄创建客户端。
pub fn NewMetaDataClient(kv: Arc<dyn EtcdKV>) -> MetaDataClient {
    MetaDataClient { KV: kv }
}

impl MetaDataClient {
    /// 写入任务 JSON、各 range 键；若 Pausing 则附带 PauseV2。
    pub fn PutTask(&self, task: &TaskInfo) -> Result<(), String> {
        let data = serde_json::to_vec(&task.PBInfo).map_err(|e| e.to_string())?;
        // 内存实现没有 etcd txn；先落 ranges、最后发布 task 键，
        // 使 task watch 观察到 EventAdd 时 Ranges 已经完整可见。
        for r in &task.Ranges {
            self.KV
                .PutBytes(&RangeKeyOf(&task.PBInfo.Name, &r.StartKey), &r.EndKey)?;
        }
        self.KV.Put(&TaskOf(&task.PBInfo.Name), &data)?;
        if task.Pausing {
            self.KV.Put(&Pause(&task.PBInfo.Name), &[])?;
        }
        Ok(())
    }

    /// 删除任务及其 ranges/pause/checkpoints/last-error 前缀。
    pub fn DeleteTask(&self, taskName: &str) -> Result<(), String> {
        self.KV.Delete(&TaskOf(taskName))?;
        self.KV.DeletePrefix(&RangesOf(taskName))?;
        self.KV.Delete(&Pause(taskName))?;
        self.KV.DeletePrefix(&CheckPointsOf(taskName))?;
        self.KV.DeletePrefix(&LastErrorPrefixOf(taskName))?;
        self.KV.Delete(&GlobalCheckpointOf(taskName))?;
        self.KV.DeletePrefix(&StorageCheckpointOf(taskName))?;
        Ok(())
    }

    /// 写入 Pause 键；opts 可定制 PauseV2 字段。
    pub fn PauseTask(&self, taskName: &str, mut opts: Vec<PauseTaskOption>) -> Result<(), String> {
        let mut pause = NewLocalPauseV2();
        for o in &mut opts {
            o(&mut pause);
        }
        let raw = serde_json::to_vec(&pause).map_err(|e| e.to_string())?;
        self.KV.Put(&Pause(taskName), &raw)
    }

    /// 删除 Pause 键以恢复任务。
    pub fn ResumeTask(&self, taskName: &str) -> Result<(), String> {
        self.KV.Delete(&Pause(taskName))
    }

    /// 清理任务上次错误前缀下的全部键。
    pub fn CleanLastErrorOfTask(&self, taskName: &str) -> Result<(), String> {
        self.KV.DeletePrefix(&LastErrorPrefixOf(taskName))
    }

    /// 按名读取任务；键缺失返回 not found。
    pub fn GetTask(&self, taskName: &str) -> Result<Task, String> {
        let data = self.KV.Get(&TaskOf(taskName))?;
        if data.is_empty() {
            return Err(format!("task {taskName} not found"));
        }
        let info: StreamBackupTaskInfo =
            serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        Ok(self.TaskByInfo(info))
    }

    /// 读取任务并附带是否存在 Pause 键。
    pub fn GetTaskWithPauseStatus(&self, taskName: &str) -> Result<(Task, bool), String> {
        let task = self.GetTask(taskName)?;
        let pause = self.KV.GetWithRevision(&Pause(taskName))?;
        Ok((task, pause.Value.is_some()))
    }

    /// 由任务信息包装为带客户端句柄的 `Task`。
    pub fn TaskByInfo(&self, t: StreamBackupTaskInfo) -> Task {
        Task {
            cli: self.clone(),
            Info: t,
        }
    }

    /// 列举全部任务（忽略 revision）。
    pub fn GetAllTasks(&self) -> Result<Vec<Task>, String> {
        let (tasks, _) = self.GetAllTasksWithRevision()?;
        Ok(tasks)
    }

    /// 按任务前缀做线性化扫描，并返回可用于后续 watch 的 revision。
    pub fn GetAllTasksWithRevision(&self) -> Result<(Vec<Task>, i64), String> {
        let pfx = PrefixOfTask();
        let (kvs, revision) = self.KV.GetPrefixWithRevision(&pfx)?;
        let mut tasks = Vec::new();
        for (_, v) in kvs {
            let info: StreamBackupTaskInfo =
                serde_json::from_slice(&v).map_err(|e| e.to_string())?;
            tasks.push(self.TaskByInfo(info));
        }
        Ok((tasks, revision))
    }

    /// 返回当前任务数量。
    pub fn GetTaskCount(&self) -> Result<usize, String> {
        Ok(self.GetAllTasks()?.len())
    }
}

/// 绑定客户端与任务信息的便捷句柄。
#[derive(Clone)]
pub struct Task {
    /// 用于 Pause 等写操作的客户端副本。
    pub cli: MetaDataClient,
    /// 任务静态信息（名称、StartTs、存储等）。
    pub Info: StreamBackupTaskInfo,
}

pub fn NewTask(client: MetaDataClient, info: StreamBackupTaskInfo) -> Task {
    Task {
        cli: client,
        Info: info,
    }
}

impl Task {
    /// 通过客户端暂停本任务。
    pub fn Pause(&self, opts: Vec<PauseTaskOption>) -> Result<(), String> {
        self.cli.PauseTask(&self.Info.Name, opts)
    }

    pub fn Resume(&self) -> Result<(), String> {
        self.cli.ResumeTask(&self.Info.Name)
    }

    pub fn GetPauseV2(&self) -> Result<Option<PauseV2>, String> {
        let raw = self.cli.KV.Get(&Pause(&self.Info.Name))?;
        if raw.is_empty() {
            return Ok(None);
        }
        serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub fn IsPaused(&self) -> Result<bool, String> {
        Ok(self
            .cli
            .KV
            .GetWithRevision(&Pause(&self.Info.Name))?
            .Value
            .is_some())
    }

    pub fn Ranges(&self) -> Result<Vec<crate::stubs::KeyRange>, String> {
        let prefix = RangesOf(&self.Info.Name);
        self.cli
            .KV
            .GetPrefix(&prefix)?
            .into_iter()
            .map(|(key, value)| {
                if !key.starts_with(prefix.as_bytes()) {
                    return Err("range prefix mismatch".into());
                }
                Ok(crate::stubs::KeyRange {
                    StartKey: key[prefix.len()..].to_vec(),
                    EndKey: value,
                })
            })
            .collect()
    }

    pub fn NextBackupTSList(&self) -> Result<Vec<Checkpoint>, String> {
        self.cli
            .KV
            .GetPrefix(&CheckPointsOf(&self.Info.Name))?
            .into_iter()
            .map(|(key, value)| ParseCheckpoint(&self.Info.Name, &key, &value))
            .collect()
    }

    pub fn GetStorageCheckpoint(&self) -> Result<u64, String> {
        let mut checkpoint = self.Info.StartTs;
        for (_, value) in self
            .cli
            .KV
            .GetPrefix(&StorageCheckpointOf(&self.Info.Name))?
        {
            if value.len() != 8 {
                return Err(format!(
                    "the value isn't 64bits (it is {} bytes)",
                    value.len()
                ));
            }
            checkpoint = checkpoint.max(u64::from_be_bytes(value.try_into().unwrap()));
        }
        Ok(checkpoint)
    }

    pub fn GetGlobalCheckPointTS(&self) -> Result<u64, String> {
        let mut initialized = false;
        let mut checkpoint = self.Info.StartTs;
        for cp in self.NextBackupTSList()? {
            if cp.Type() == CheckpointType::Global {
                return Ok(cp.TS);
            }
            if cp.Type() == CheckpointType::Store && (!initialized || cp.TS < checkpoint) {
                initialized = true;
                checkpoint = cp.TS;
            }
        }
        Ok(checkpoint.max(self.GetStorageCheckpoint()?))
    }

    pub fn UploadGlobalCheckpoint(&self, ts: u64) -> Result<(), String> {
        self.cli
            .KV
            .Put(&GlobalCheckpointOf(&self.Info.Name), &ts.to_be_bytes())
    }

    pub fn LastError(&self) -> Result<HashMap<u64, StreamBackupError>, String> {
        let prefix = LastErrorPrefixOf(&self.Info.Name);
        let mut result = HashMap::new();
        for (key, value) in self.cli.KV.GetPrefix(&prefix)? {
            let suffix = std::str::from_utf8(&key[prefix.len()..]).map_err(|e| e.to_string())?;
            let store_id = suffix.parse::<u64>().map_err(|e| e.to_string())?;
            result.insert(store_id, StreamBackupError::Unmarshal(&value)?);
        }
        Ok(result)
    }
}

// 确保 StreamBackupTaskInfo 可被 serde JSON 编解码（与 Go protobuf JSON 路径对应）。
