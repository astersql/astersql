// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Structured logging field helpers ported from `br/pkg/logutil/logging.go`.
//!
//! 结构化日志字段辅助：对齐 Go `br/pkg/logutil/logging.go`。
//! 用 `Field`/`EncodedValue` 模拟 zap Field，再经 JSON 编码输出到 tracing。
//! 主体包括：Logger 与级别控制、数组/对象 Marshaler、备份元数据字段、
//! Key/Region/SST 脱敏格式化，以及 Histogram 摘要。
//! 敏感键经 `RedactKey`/`NeedRedact` 处理，与 Go redact 包语义对齐。

use std::fmt::{self, Display, Write as FmtWrite};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use astersql_lightning_metric::read_histogram;
use prometheus::Histogram;
use uuid::Uuid;

// 依赖 stubs：脱敏 Key/Value、备份 File、RewriteRule、SST/Region 等本地桩类型。
use crate::stubs::{
    Key as RedactKey, KeyRange, NeedRedact, Value as RedactValue,
    kvproto::brpb::{self as backuppb, File as BackupFile},
    kvproto::import_sstpb::{RewriteRule as ImportRewriteRule, SstMeta as ImportSstMeta},
    kvproto::metapb::{Peer, Region, RegionEpoch},
};

// 字节转小写十六进制，用于 sha256/UUID 等字段展示（对齐 Go encoding/hex）。
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------
// 编码层：把 zap 风格 Field 落到可 JSON 序列化的中间表示。
// Encoded values / field encoding
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
/// 字段值的中间编码；`Skip` 表示该 Field 在输出中应被忽略。
pub enum EncodedValue {
    /// 标记字段应被忽略。
    Skip,
    /// JSON null，对齐 zap.Any 的 nil 值。
    Null,
    /// 字符串。
    String(String),
    /// 有符号整数。
    Int(i64),
    /// 无符号 64 位。
    Uint64(u64),
    /// 布尔。
    Bool(bool),
    /// 浮点。
    Float(f64),
    /// 数组。
    Array(Vec<EncodedValue>),
    /// 对象（有序键值）。
    Object(Vec<(String, EncodedValue)>),
}

#[derive(Clone, Debug, PartialEq)]
/// 结构化日志字段，对应 Go 的 `zap.Field`；`skip` 时不参与编码。
pub struct Field {
    pub key: String,
    value: EncodedValue,
    skip: bool,
}

impl Field {
    /// 构造可跳过字段（如错误为 None 时的 ShortError）。
    pub fn skip() -> Self {
        Self {
            key: String::new(),
            value: EncodedValue::Skip,
            skip: true,
        }
    }

    /// 字符串字段构造器。
    pub fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::String(value.into()),
            skip: false,
        }
    }

    /// 有符号整数字段。
    pub fn int(key: impl Into<String>, value: i64) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::Int(value),
            skip: false,
        }
    }

    /// 无符号 64 位整数字段（版本号、计数等常用）。
    pub fn uint64(key: impl Into<String>, value: u64) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::Uint64(value),
            skip: false,
        }
    }

    /// 布尔字段。
    pub fn bool(key: impl Into<String>, value: bool) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::Bool(value),
            skip: false,
        }
    }

    /// 数组字段；元素已是 EncodedValue。
    pub fn array(key: impl Into<String>, values: Vec<EncodedValue>) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::Array(values),
            skip: false,
        }
    }

    /// 对象字段；键值对列表保持插入顺序以便测试对照。
    pub fn object(key: impl Into<String>, object: Vec<(String, EncodedValue)>) -> Self {
        Self {
            key: key.into(),
            value: EncodedValue::Object(object),
            skip: false,
        }
    }

    /// 通过 ObjectMarshaler 生成对象字段（对齐 zap.Object）。
    pub fn from_object<M: ObjectMarshaler>(key: impl Into<String>, marshaler: M) -> Self {
        let mut enc = ObjectEncoder::default();
        marshaler.marshal_object(&mut enc);
        Self::object(key, enc.fields)
    }

    /// 是否为跳过字段。
    pub fn is_skip(&self) -> bool {
        self.skip
    }

    /// 单字段编码为 JSON 对象；skip 时返回 `{}` 便于测试断言。
    pub fn encode_json(&self) -> String {
        if self.skip {
            return "{}".to_string();
        }
        let mut out = String::from("{");
        write_json_pair(&mut out, &self.key, &self.value);
        out.push('}');
        out
    }

    /// 非 skip 且键值全等时视为同一字段（测试辅助）。
    pub fn equals(&self, other: &Field) -> bool {
        !self.skip && !other.skip && self.key == other.key && self.value == other.value
    }
}

/// 多字段合并编码为单个 JSON 对象，跳过 `skip` 字段。
pub fn encode_fields_json(fields: &[Field]) -> String {
    let mut out = String::from("{");
    let mut first = true;
    for field in fields {
        if field.skip {
            continue;
        }
        if !first {
            out.push(',');
        }
        write_json_pair(&mut out, &field.key, &field.value);
        first = false;
    }
    out.push('}');
    out
}

// 写入 `"key": value` 片段。
fn write_json_pair(out: &mut String, key: &str, value: &EncodedValue) {
    out.push('"');
    json_escape_str(out, key);
    out.push_str("\": ");
    write_json_value(out, value);
}

// 对象字面量序列化，字段间以逗号+空格分隔以稳定测试期望。
fn write_json_object(out: &mut String, fields: &[(String, EncodedValue)]) {
    out.push('{');
    for (idx, (key, val)) in fields.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        write_json_pair(out, key, val);
    }
    out.push('}');
}

// 按 EncodedValue 变体写出 JSON；Skip 落成 null（通常上层已过滤）。
fn write_json_value(out: &mut String, value: &EncodedValue) {
    match value {
        EncodedValue::Skip | EncodedValue::Null => out.push_str("null"),
        EncodedValue::String(s) => {
            out.push('"');
            json_escape_str(out, s);
            out.push('"');
        }
        EncodedValue::Int(v) => write!(out, "{v}").expect("write int"),
        EncodedValue::Uint64(v) => write!(out, "{v}").expect("write uint"),
        EncodedValue::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        EncodedValue::Float(v) => write!(out, "{v}").expect("write float"),
        EncodedValue::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                write_json_value(out, item);
            }
            out.push(']');
        }
        EncodedValue::Object(map) => write_json_object(out, map),
    }
}

// JSON 字符串转义：引号、反斜杠、常见空白与控制字符。
fn json_escape_str(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => write!(out, "\\u{:04x}", c as u32).expect("escape"),
            c => out.push(c),
        }
    }
}

#[derive(Default)]
/// 对象编码器：收集键值，对应 zapcore.ObjectEncoder 的子集能力。
pub struct ObjectEncoder {
    pub fields: Vec<(String, EncodedValue)>,
}

impl ObjectEncoder {
    /// 追加字符串属性。
    pub fn AddString(&mut self, key: &str, value: impl Into<String>) {
        self.fields
            .push((key.to_string(), EncodedValue::String(value.into())));
    }

    /// 追加 i32（内部提升为 i64 EncodedValue）。
    pub fn AddInt(&mut self, key: &str, value: i32) {
        self.fields
            .push((key.to_string(), EncodedValue::Int(value as i64)));
    }

    /// 追加 u64 属性。
    pub fn AddUint64(&mut self, key: &str, value: u64) {
        self.fields
            .push((key.to_string(), EncodedValue::Uint64(value)));
    }

    /// 追加布尔属性。
    pub fn AddBool(&mut self, key: &str, value: bool) {
        self.fields
            .push((key.to_string(), EncodedValue::Bool(value)));
    }

    /// 追加浮点属性（Histogram total 等）。
    pub fn AddFloat64(&mut self, key: &str, value: f64) {
        self.fields
            .push((key.to_string(), EncodedValue::Float(value)));
    }

    /// 通过 AbbreviatedArrayMarshaler 追加数组属性。
    pub fn AddArray(&mut self, key: &str, marshaler: AbbreviatedArrayMarshaler) {
        let mut enc = ArrayEncoder::default();
        marshaler.marshal_array(&mut enc);
        self.fields
            .push((key.to_string(), EncodedValue::Array(enc.items)));
    }

    /// 嵌套对象：先 marshal 再作为 Object 值挂入。
    pub fn AddObject(&mut self, key: &str, marshaler: impl ObjectMarshaler) {
        let mut enc = ObjectEncoder::default();
        marshaler.marshal_object(&mut enc);
        self.fields
            .push((key.to_string(), EncodedValue::Object(enc.fields)));
    }
}

#[derive(Default)]
/// 数组编码器：收集 EncodedValue 元素。
pub struct ArrayEncoder {
    pub items: Vec<EncodedValue>,
}

impl ArrayEncoder {
    /// 追加字符串元素。
    pub fn AppendString(&mut self, value: impl Into<String>) {
        self.items.push(EncodedValue::String(value.into()));
    }

    /// 追加嵌套对象元素（如 SSTMeta 列表）。
    pub fn AppendObject(&mut self, marshaler: impl ObjectMarshaler) {
        let mut enc = ObjectEncoder::default();
        marshaler.marshal_object(&mut enc);
        self.items.push(EncodedValue::Object(enc.fields));
    }
}

/// 对象可序列化到 ObjectEncoder，对齐 zapcore.ObjectMarshaler。
pub trait ObjectMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder);
}

/// 数组可序列化到 ArrayEncoder，对齐 zapcore.ArrayMarshaler。
pub trait ArrayMarshaler {
    fn marshal_array(&self, enc: &mut ArrayEncoder);
}

// ---------------------------------------------------------------------------
// Logger 与全局级别/终端 logger：Rust 侧用 tracing，测试可切 Capture。
// Logger / global log helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// 日志级别；排序用于与全局阈值比较过滤。
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

// 全局最低输出级别；默认 Info。
// 可选终端 logger，供 WarnTerm 双写到“用户可见”通道。
static LOG_LEVEL: LazyLock<RwLock<Level>> = LazyLock::new(|| RwLock::new(Level::Info));
static LOGGER_TO_TERM: LazyLock<RwLock<Option<Logger>>> = LazyLock::new(|| RwLock::new(None));

#[derive(Clone)]
/// 后端：生产走 tracing，测试走内存 Capture。
enum LoggerBackend {
    Tracing,
    Capture(Arc<Mutex<Vec<CapturedLog>>>),
}

#[derive(Clone, Debug, PartialEq)]
/// Capture 后端单条日志，供单测断言级别/消息/字段。
pub struct CapturedLog {
    pub level: Level,
    pub message: String,
    pub fields: Vec<Field>,
}

#[derive(Clone)]
/// 带预置字段的 Logger；`With` 合并字段后共享同一 backend。
pub struct Logger {
    fields: Vec<Field>,
    backend: LoggerBackend,
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("fields", &self.fields)
            // Debug 不暴露 backend，避免噪音。
            .finish_non_exhaustive()
    }
}

impl Logger {
    /// 构造 Capture Logger，并返回共享缓冲供测试读取。
    pub fn capture() -> (Self, Arc<Mutex<Vec<CapturedLog>>>) {
        let logs = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                fields: Vec::new(),
                backend: LoggerBackend::Capture(logs.clone()),
            },
            logs,
        )
    }

    /// 派生带额外字段的 Logger，不修改原实例（对齐 zap.With）。
    pub fn With(&self, fields: impl IntoIterator<Item = Field>) -> Self {
        let mut merged = self.fields.clone();
        merged.extend(fields);
        Self {
            fields: merged,
            backend: self.backend.clone(),
        }
    }

    /// Info 级别便捷方法。
    pub fn Info(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Info, message, fields);
    }

    /// 与 Info 等价的命名变体，便于迁移调用点区分。
    pub fn Info_with_fields(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Info, message, fields);
    }

    /// Warn 级别便捷方法。
    pub fn Warn(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Warn, message, fields);
    }

    /// 无额外字段的 Warn。
    pub fn Warn_only(&self, message: &str) {
        self.log(Level::Warn, message, []);
    }

    /// Debug 级别便捷方法。
    pub fn Debug(&self, message: &str, fields: impl IntoIterator<Item = Field>) {
        self.log(Level::Debug, message, fields);
    }

    /// 无额外字段的 Debug。
    pub fn Debug_only(&self, message: &str) {
        self.log(Level::Debug, message, []);
    }

    /// 统一出口：低于全局级别则丢弃；否则合并字段后按 backend 输出。
    pub fn log(&self, level: Level, message: &str, extra: impl IntoIterator<Item = Field>) {
        // 低于阈值的级别直接返回，避免无谓编码。
        if level < *LOG_LEVEL.read().expect("log level lock poisoned") {
            return;
        }
        let mut fields = self.fields.clone();
        fields.extend(extra);
        match &self.backend {
            // Tracing：拼 JSON；Capture：推入共享 Vec。
            LoggerBackend::Tracing => emit_tracing(level, message, &fields),
            // 测试路径：原样保留 Field，便于 equals/JSON 断言。
            LoggerBackend::Capture(logs) => {
                logs.lock()
                    .expect("capture lock poisoned")
                    .push(CapturedLog {
                        level,
                        message: message.to_string(),
                        fields,
                    });
            }
        }
    }

    /// Capture 后端返回缓冲；Tracing 返回 None。
    pub fn captured_logs(&self) -> Option<Arc<Mutex<Vec<CapturedLog>>>> {
        match &self.backend {
            LoggerBackend::Capture(logs) => Some(logs.clone()),
            LoggerBackend::Tracing => None,
        }
    }
}

/// 默认 tracing 后端 Logger，无预置字段。
pub fn default_logger() -> Logger {
    Logger {
        fields: Vec::new(),
        backend: LoggerBackend::Tracing,
    }
}

// 将消息与字段 JSON 拼成一条 tracing 事件；target 固定 br::logutil。
fn emit_tracing(level: Level, message: &str, fields: &[Field]) {
    // 无字段时只打消息，避免多余空对象。
    let summary = if fields.is_empty() {
        message.to_string()
    } else {
        format!("{message} {}", encode_fields_json(fields))
    };
    match level {
        // 按级别映射到 tracing 宏。
        Level::Debug => tracing::debug!(target: "br::logutil", "{summary}"),
        Level::Info => tracing::info!(target: "br::logutil", "{summary}"),
        Level::Warn => tracing::warn!(target: "br::logutil", "{summary}"),
        Level::Error => tracing::error!(target: "br::logutil", "{summary}"),
    }
}

/// 包级快捷入口，对齐 Go `github.com/pingcap/log` 的 `L`/`Warn`/`GetLevel`。
pub mod log {
    use super::*;

    /// 返回默认 Logger（每次新建，字段不跨调用累积）。
    pub fn L() -> Logger {
        default_logger()
    }

    /// 包级 Warn。
    pub fn Warn(message: &str, fields: impl IntoIterator<Item = Field>) {
        L().log(Level::Warn, message, fields);
    }

    /// 读取全局日志级别。
    pub fn GetLevel() -> Level {
        *LOG_LEVEL.read().expect("log level lock poisoned")
    }

    /// 设置全局日志级别。
    pub fn SetLevel(level: Level) {
        *LOG_LEVEL.write().expect("log level lock poisoned") = level;
    }

    /// 配置 WarnTerm 的终端双写目标；None 表示关闭。
    pub fn set_logger_to_term(logger: Option<Logger>) {
        *LOGGER_TO_TERM
            .write()
            .expect("logger-to-term lock poisoned") = logger;
    }
}

/// 测试用级别守卫：Drop 时恢复旧级别。
pub struct LevelGuard {
    old: Level,
}

impl Drop for LevelGuard {
    fn drop(&mut self) {
        // RAII：离开作用域自动还原，避免污染后续用例。
        log::SetLevel(self.old);
    }
}

/// 测试期间临时覆盖全局级别，返回守卫。
pub fn OverrideLevelForTest(level: Level) -> LevelGuard {
    let old = log::GetLevel();
    log::SetLevel(level);
    LevelGuard { old }
}

/// 先写常规 Warn，再若配置了终端 logger 则双写一份（对齐 Go WarnTerm）。
pub fn WarnTerm(message: &str, fields: impl IntoIterator<Item = Field>) {
    // 先收集以便常规通道与终端通道各用一份。
    let fields: Vec<Field> = fields.into_iter().collect();
    log::Warn(message, fields.clone());
    if let Some(logger) = LOGGER_TO_TERM
        .read()
        .expect("logger-to-term lock poisoned")
        .clone()
    {
        logger.log(Level::Warn, message, fields);
    }
}

// ---------------------------------------------------------------------------
// 数组/对象 Marshaler：缩略长列表并序列化备份相关 protobuf。
// Array / object marshalers
// ---------------------------------------------------------------------------

#[derive(Clone)]
/// 长度≤4 全量输出；否则只留首尾并插入 `(skip N)`（对齐 Go AbbreviatedArrayMarshaler）。
pub struct AbbreviatedArrayMarshaler(pub Vec<String>);

impl ArrayMarshaler for AbbreviatedArrayMarshaler {
    fn marshal_array(&self, enc: &mut ArrayEncoder) {
        // 短数组：逐项 Append；长数组：首项 + skip 提示 + 末项。
        if self.0.len() <= 4 {
            for item in &self.0 {
                enc.AppendString(item.clone());
            }
        } else {
            let total = self.0.len();
            // skip 个数等于 total-2，中间元素不展开。
            enc.AppendString(self.0[0].clone());
            enc.AppendString(format!("(skip {})", total - 2));
            enc.AppendString(self.0[total - 1].clone());
        }
    }
}

/// 先用 marshal_func 转成字符串列表，再按缩略规则编码为数组 Field。
pub fn AbbreviatedArray<T, F>(key: &str, elements: T, marshal_func: F) -> Field
where
    F: FnOnce(T) -> Vec<String>,
{
    Field::array(key, {
        let mut enc = ArrayEncoder::default();
        AbbreviatedArrayMarshaler(marshal_func(elements)).marshal_array(&mut enc);
        enc.items
    })
}

/// Display 列表的缩略字段；阈值 `<4` 全量，否则首/skip/尾（对齐 Go）。
pub fn AbbreviatedStringers<T: fmt::Display>(key: &str, stringers: Vec<T>) -> Field {
    // 与 AbbreviatedArray 阈值略有不同：此处是 <4 全量（对齐 Go）。
    if stringers.len() < 4 {
        Field::array(
            key,
            stringers
                .into_iter()
                .map(|item| EncodedValue::String(item.to_string()))
                .collect(),
        )
    } else {
        Field::array(
            key,
            vec![
                EncodedValue::String(stringers[0].to_string()),
                EncodedValue::String(format!("(skip {})", stringers.len() - 2)),
                EncodedValue::String(stringers[stringers.len() - 1].to_string()),
            ],
        )
    }
}

/// 单备份文件对象字段：名称、CF、校验与起止键（键已脱敏）。
struct FileMarshaler<'a>(&'a BackupFile);

impl ObjectMarshaler for FileMarshaler<'_> {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let file = self.0;
        // 字段名与 Go zapFileMarshaler 保持一致，便于对照日志。
        enc.AddString("name", file.get_name());
        enc.AddString("CF", file.get_cf());
        enc.AddString("sha256", hex_encode(file.get_sha256()));
        // 起止键走 RedactKey，避免明文落日志。
        enc.AddString("startKey", RedactKey(file.get_start_key()));
        enc.AddString("endKey", RedactKey(file.get_end_key()));
        enc.AddUint64("startVersion", file.get_start_version());
        enc.AddUint64("endVersion", file.get_end_version());
        enc.AddUint64("totalKvs", file.get_total_kvs());
        enc.AddUint64("totalBytes", file.get_total_bytes());
        enc.AddUint64("CRC64Xor", file.get_crc64xor());
    }
}

/// 构造 `file` 对象字段。
pub fn File(file: BackupFile) -> Field {
    Field::from_object("file", FileMarshaler(&file))
}

/// 多文件聚合：总数、缩略文件名列表，以及 KV/字节/Size 合计。
struct FilesMarshaler(Vec<BackupFile>);

impl ObjectMarshaler for FilesMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let total = self.0.len();
        enc.AddInt("total", total as i32);
        let names: Vec<String> = self.0.iter().map(|f| f.get_name().to_string()).collect();
        // 文件名列表走缩略，避免超长备份日志。
        enc.AddArray("files", AbbreviatedArrayMarshaler(names));

        // 累加各文件统计，便于一条日志看清备份规模。
        let mut total_kvs = 0_u64;
        let mut total_bytes = 0_u64;
        let mut total_size = 0_u64;
        for file in &self.0 {
            total_kvs += file.get_total_kvs();
            total_bytes += file.get_total_bytes();
            total_size += file.get_size();
        }
        // 合计指标字段名对齐 Go（totalKVs/totalBytes/totalSize）。
        enc.AddUint64("totalKVs", total_kvs);
        enc.AddUint64("totalBytes", total_bytes);
        enc.AddUint64("totalSize", total_size);
    }
}

/// 供外部对象把 Files 子字段写入 encoder（对齐 Go MarshalLogObjectForFiles）。
pub fn MarshalLogObjectForFiles(files: &[BackupFile], enc: &mut ObjectEncoder) {
    FilesMarshaler(files.to_vec()).marshal_object(enc);
}

/// 构造顶层 `files` 对象字段。
pub fn Files(fs: Vec<BackupFile>) -> Field {
    Field::from_object("files", FilesMarshaler(fs))
}

/// 日志备份任务信息：名称、起止 TS、表过滤（逗号拼接）。
struct StreamBackupTaskInfoMarshaler(backuppb::StreamBackupTaskInfo);

impl ObjectMarshaler for StreamBackupTaskInfoMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let info = &self.0;
        // 任务名与起止 TS 是流备份排障主线索。
        enc.AddString("taskName", info.get_name());
        enc.AddUint64("startTs", info.get_start_ts());
        enc.AddUint64("endTS", info.get_end_ts());
        enc.AddString(
            "tableFilter",
            info.get_table_filter()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
    }
}

/// 构造 `streamTaskInfo` 对象字段。
pub fn StreamBackupTaskInfo(t: backuppb::StreamBackupTaskInfo) -> Field {
    Field::from_object("streamTaskInfo", StreamBackupTaskInfoMarshaler(t))
}

/// 重写规则：旧/新 key 前缀（hex）与新时间戳。
pub struct RewriteRuleMarshaler(ImportRewriteRule);

impl ObjectMarshaler for RewriteRuleMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let rule = &self.0;
        // 前缀用 hex 而非 RedactKey：规则调试需要可见字节。
        enc.AddString("oldKeyPrefix", hex_encode(rule.get_old_key_prefix()));
        enc.AddString("newKeyPrefix", hex_encode(rule.get_new_key_prefix()));
        enc.AddUint64("newTimestamp", rule.get_new_timestamp());
    }
}

/// 构造 `rewriteRule` 对象字段。
pub fn RewriteRule(rewrite_rule: ImportRewriteRule) -> Field {
    Field::from_object("rewriteRule", RewriteRuleMarshaler(rewrite_rule))
}

/// 返回可嵌入其他对象的 RewriteRule Marshaler。
pub fn RewriteRuleObject(rewrite_rule: ImportRewriteRule) -> RewriteRuleMarshaler {
    RewriteRuleMarshaler(rewrite_rule)
}

// RegionEpoch 紧凑字符串，便于嵌进单字段。
fn format_region_epoch(epoch: &RegionEpoch) -> String {
    format!(
        "conf_ver:{} version:{} ",
        epoch.get_conf_ver(),
        epoch.get_version()
    )
}

// Peer 紧凑字符串：id + store_id。
fn format_peer(peer: &Peer) -> String {
    format!("id:{} store_id:{} ", peer.get_id(), peer.get_store_id())
}

/// Region 日志对象：ID、脱敏起止键、epoch、peers 列表。
struct RegionMarshaler(Region);

impl ObjectMarshaler for RegionMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let region = &self.0;
        let peers = region
            .get_peers()
            .iter()
            .map(format_peer)
            .collect::<Vec<_>>()
            .join(",");
        // Region ID 用 Uint64，与 metapb 一致。
        enc.AddUint64("ID", region.get_id());
        enc.AddString("startKey", RedactKey(region.get_start_key()));
        enc.AddString("endKey", RedactKey(region.get_end_key()));
        enc.AddString("epoch", format_region_epoch(region.get_region_epoch()));
        enc.AddString("peers", peers);
    }
}

/// 构造键名为 `region` 的对象字段。
pub fn Region(region: Region) -> Field {
    Field::from_object("region", RegionMarshaler(region))
}

/// 自定义键名的 Region 对象字段。
pub fn RegionBy(key: &str, region: Region) -> Field {
    Field::from_object(key, RegionMarshaler(region))
}

/// Leader peer 字符串字段。
pub fn Leader(peer: Peer) -> Field {
    Field::string("leader", format_peer(&peer))
}

/// 普通 peer 字符串字段。
pub fn Peer(peer: Peer) -> Field {
    Field::string("peer", format_peer(&peer))
}

/// SST 元数据：CF、范围、CRC、region 与 UUID（非法 UUID 时回退 hex）。
struct SSTMetaMarshaler(ImportSstMeta);

impl ObjectMarshaler for SSTMetaMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let meta = &self.0;
        // CF 名优先输出，便于区分 default/write。
        enc.AddString("CF", meta.get_cf_name());
        // 区间是否右开，影响 restore 边界解释。
        enc.AddBool("endKeyExclusive", meta.get_end_key_exclusive());
        enc.AddUint64("CRC32", meta.get_crc32() as u64);
        enc.AddUint64("length", meta.get_length());
        enc.AddUint64("regionID", meta.get_region_id());
        enc.AddString("regionEpoch", format_region_epoch(meta.get_region_epoch()));
        enc.AddString("startKey", RedactKey(meta.get_range().get_start()));
        enc.AddString("endKey", RedactKey(meta.get_range().get_end()));
        // UUID 解析失败时保留 hex，避免日志字段丢失。
        let uuid_field = match Uuid::from_slice(meta.get_uuid()) {
            Ok(id) => id.to_string(),
            Err(_) => format!("invalid UUID {}", hex_encode(meta.get_uuid())),
        };
        enc.AddString("UUID", uuid_field);
    }
}

/// 构造 `sstMeta` 对象字段。
pub fn SSTMeta(sst_meta: ImportSstMeta) -> Field {
    Field::from_object("sstMeta", SSTMetaMarshaler(sst_meta))
}

/// SST 元数据数组 Marshaler。
struct SSTMetasMarshaler(Vec<ImportSstMeta>);

impl ArrayMarshaler for SSTMetasMarshaler {
    fn marshal_array(&self, enc: &mut ArrayEncoder) {
        for meta in &self.0 {
            enc.AppendObject(SSTMetaMarshaler(meta.clone()));
        }
    }
}

/// 构造 `sstMetas` 数组字段。
pub fn SSTMetas(sst_metas: Vec<ImportSstMeta>) -> Field {
    Field::array("sstMetas", {
        let mut enc = ArrayEncoder::default();
        SSTMetasMarshaler(sst_metas).marshal_array(&mut enc);
        enc.items
    })
}

/// 多 SST 摘要：覆盖起止键并汇总量；适合大批量时避免逐条刷屏。
pub fn BriefSSTMetas(key: &str, sst_metas: Vec<ImportSstMeta>) -> Field {
    let mut start_key = Vec::new();
    let mut end_key = Vec::new();
    let mut total = 0_i32;
    let mut total_size = 0_u64;
    let mut total_kv = 0_u64;
    let mut total_kv_size = 0_u64;

    for meta in &sst_metas {
        let range = meta.get_range();
        let range_start = range.get_start();
        let range_end = range.get_end();
        // 首个 meta 初始化覆盖范围，随后按字节序扩展 min start / max end。
        if total == 0 {
            start_key = range_start.to_vec();
            end_key = range_end.to_vec();
        }
        // 扩展覆盖的最小 start / 最大 end。
        if range_start < start_key.as_slice() {
            start_key = range_start.to_vec();
        }
        if range_end > end_key.as_slice() {
            end_key = range_end.to_vec();
        }
        // length 计入 totalSize；KV 数/字节分别累计。
        total_size += meta.get_length();
        total_kv += meta.get_total_kvs();
        total_kv_size += meta.get_total_bytes();
        total += 1;
    }

    Field::from_object(
        key,
        BriefSSTMetasObject {
            total,
            start_key,
            end_key,
            total_size,
            total_kv,
            total_kv_size,
        },
    )
}

/// BriefSSTMetas 的聚合结果载体。
struct BriefSSTMetasObject {
    total: i32,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
    total_size: u64,
    total_kv: u64,
    total_kv_size: u64,
}

impl ObjectMarshaler for BriefSSTMetasObject {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        enc.AddInt("total", self.total);
        enc.AddString("startKey", RedactKey(&self.start_key));
        enc.AddString("endKey", RedactKey(&self.end_key));
        enc.AddUint64("totalSize", self.total_size);
        enc.AddUint64("totalKvs", self.total_kv);
        enc.AddUint64("totalKvSize", self.total_kv_size);
    }
}

/// Keys 聚合：总数 + 缩略脱敏 key 列表。
struct KeysMarshaler(Vec<Vec<u8>>);

impl ObjectMarshaler for KeysMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        let total = self.0.len();
        enc.AddInt("total", total as i32);
        // 逐 key 脱敏后再缩略。
        let elements: Vec<String> = self.0.iter().map(|k| RedactKey(k)).collect();
        enc.AddArray("keys", AbbreviatedArrayMarshaler(elements));
    }
}

/// 单 key 字段，值经 RedactKey 脱敏。
pub fn Key(field_key: &str, key: impl AsRef<[u8]>) -> Field {
    Field::string(field_key, RedactKey(key.as_ref()))
}

/// 多 key 对象字段。
pub fn Keys(keys: Vec<Vec<u8>>) -> Field {
    Field::from_object("keys", KeysMarshaler(keys))
}

/// 可选错误字段：None → skip，Some → 字符串（对齐 Go AShortError）。
pub fn AShortError(key: &str, err: Option<&dyn Display>) -> Field {
    match err {
        // 无错误时不输出字段，避免 `"error": null` 噪声。
        None => Field::skip(),
        // Display 即短错误文本，不展开 cause 链。
        Some(err) => Field::string(key, err.to_string()),
    }
}

/// 键固定为 `error` 的短错误字段。
pub fn ShortError(err: Option<&dyn Display>) -> Field {
    AShortError("error", err)
}

/// 将 Rust 值转换成 zap.Any 对应的结构化值。
pub trait IntoEncodedValue {
    fn into_encoded_value(self) -> EncodedValue;
}

impl IntoEncodedValue for EncodedValue {
    fn into_encoded_value(self) -> EncodedValue {
        self
    }
}

impl IntoEncodedValue for bool {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::Bool(self)
    }
}

macro_rules! impl_encoded_signed {
    ($($ty:ty),* $(,)?) => {$(
        impl IntoEncodedValue for $ty {
            fn into_encoded_value(self) -> EncodedValue {
                EncodedValue::Int(self as i64)
            }
        }
    )*};
}

macro_rules! impl_encoded_unsigned {
    ($($ty:ty),* $(,)?) => {$(
        impl IntoEncodedValue for $ty {
            fn into_encoded_value(self) -> EncodedValue {
                EncodedValue::Uint64(self as u64)
            }
        }
    )*};
}

impl_encoded_signed!(i8, i16, i32, i64, isize);
impl_encoded_unsigned!(u8, u16, u32, u64, usize);

impl IntoEncodedValue for f32 {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::Float(self as f64)
    }
}

impl IntoEncodedValue for f64 {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::Float(self)
    }
}

impl IntoEncodedValue for String {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::String(self)
    }
}

impl IntoEncodedValue for &str {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::String(self.to_string())
    }
}

impl<T: IntoEncodedValue> IntoEncodedValue for Vec<T> {
    fn into_encoded_value(self) -> EncodedValue {
        EncodedValue::Array(
            self.into_iter()
                .map(IntoEncodedValue::into_encoded_value)
                .collect(),
        )
    }
}

impl<T: IntoEncodedValue> IntoEncodedValue for Option<T> {
    fn into_encoded_value(self) -> EncodedValue {
        self.map_or(EncodedValue::Null, IntoEncodedValue::into_encoded_value)
    }
}

/// 任意结构化值按需脱敏：NeedRedact 时输出 `?`，否则保持 JSON 类型。
pub fn RedactAny<T: IntoEncodedValue>(field_key: &str, value: T) -> Field {
    // 全局脱敏开关打开时统一掩码。
    if NeedRedact() {
        Field::string(field_key, "?")
    } else {
        Field {
            key: field_key.to_string(),
            value: value.into_encoded_value(),
            skip: false,
        }
    }
}

/// 已有 Field 按需整体替换为 `?`，保留原 key。
pub fn Redact(field: Field) -> Field {
    // 整字段替换为问号，避免嵌套结构泄漏。
    if NeedRedact() {
        Field::string(field.key.clone(), "?")
    } else {
        // 未开启脱敏时原样返回。
        field
    }
}

#[derive(Clone)]
/// 可 Display 的半开区间 `[start, end)`；空 end 显示为脱敏后的 `inf`。
pub struct StringifyRange {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

/// 从 stubs::KeyRange 转换。
impl From<KeyRange> for StringifyRange {
    fn from(rng: KeyRange) -> Self {
        Self {
            StartKey: rng.StartKey.0,
            EndKey: rng.EndKey.0,
        }
    }
}

/// 由起止字节构造 StringifyRange。
pub fn StringifyRangeOf(start: impl Into<Vec<u8>>, end: impl Into<Vec<u8>>) -> StringifyRange {
    StringifyRange {
        StartKey: start.into(),
        EndKey: end.into(),
    }
}

/// 多区间集合，Display 为花括号包裹的区间列表。
pub struct StringifyKeys(pub Vec<KeyRange>);

impl fmt::Display for StringifyRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {})", RedactKey(&self.StartKey), {
            // 空 EndKey 表示正无穷，与 BR 半开区间约定一致。
            if self.EndKey.is_empty() {
                // 正无穷用 Value 脱敏通道，与 Go 一致。
                RedactValue("inf")
            } else {
                RedactKey(&self.EndKey)
            }
        })
    }
}

impl fmt::Display for StringifyKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 集合以花括号包裹，元素逗号分隔。
        f.write_str("{")?;
        for (idx, rng) in self.0.iter().enumerate() {
            if idx > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", StringifyRange::from(rng.clone()))?;
        }
        f.write_str("}")
    }
}

/// Display 列表的数组 Marshaler 包装。
pub struct StringifyManyArray<T>(Vec<T>);

/// 包装为可 marshal 的多值数组。
pub fn StringifyMany<T: fmt::Display>(items: Vec<T>) -> StringifyManyArray<T> {
    StringifyManyArray(items)
}

impl<T: fmt::Display> ArrayMarshaler for StringifyManyArray<T> {
    fn marshal_array(&self, enc: &mut ArrayEncoder) {
        // 逐项 Display 后作为字符串数组元素。
        for item in &self.0 {
            enc.AppendString(item.to_string());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 十六进制可展示/可 JSON 的字节包装。
pub struct HexBytes(pub Vec<u8>);

impl fmt::Display for HexBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display 与 JSON 均基于同一 hex 编码。
        write!(f, "{}", hex_encode(&self.0))
    }
}

impl HexBytes {
    /// 输出带引号的 hex JSON 字符串字节（对齐 Go json.Marshaler 形态）。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, String> {
        // 手动拼 JSON 字符串，避免依赖 serde。
        let encoded = hex_encode(&self.0);
        Ok(format!("\"{encoded}\"").into_bytes())
    }
}

/// Prometheus Histogram 摘要 Marshaler；无数据时静默不写字段。
pub struct HistogramMarshaler {
    histogram: Option<Histogram>,
}

/// 包装可选 Histogram。
pub fn MarshalHistogram(m: Option<Histogram>) -> HistogramMarshaler {
    // Option 透传；None 时 marshal 为空操作。
    HistogramMarshaler { histogram: m }
}

impl ObjectMarshaler for HistogramMarshaler {
    fn marshal_object(&self, enc: &mut ObjectEncoder) {
        // 任一环节缺数据则直接返回，避免空对象噪音。
        let Some(histogram) = &self.histogram else {
            return;
        };
        let Some(metric) = read_histogram(histogram) else {
            return;
        };
        let Some(hist) = metric.histogram.as_ref() else {
            return;
        };
        // 桶上界写成 lt_<bound>，再附 count/total。
        for bucket in hist.get_bucket() {
            // 上界写入键名，累积计数写入值。
            let key = format!("lt_{:.6}", bucket.get_upper_bound());
            enc.AddUint64(&key, bucket.get_cumulative_count());
        }
        enc.AddUint64("count", hist.get_sample_count());
        enc.AddFloat64("total", hist.get_sample_sum());
    }
}

#[cfg(test)]
/// 测试辅助：比较字段 JSON 与期望字符串。
pub(crate) fn assert_trim_equal(field: Field, expect: &str) {
    // 直接比较完整 JSON，失败时带上两侧文本。
    let actual = field.encode_json();
    // 期望串需与 encode_json 空格风格完全一致。
    assert_eq!(expect, actual, "encoded field mismatch");
}
