// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! BR 操作上下文 — 对齐 `br/pkg/operation/context.go`。
//! 为单次 backup/restore/truncate 等命令生成 OperationID、启动时间与 hint 字段，
//! 并据此拼装锁元数据（LockMeta）供外部存储互斥。
//! 日志侧提供线程本地 capture，便于单测断言 zap 等价消息而不污染并行用例。
//! 时间格式化走 Howard Hinnant civil day 算法，输出与 Go `time.RFC3339` UTC 一致。
//! Hint 字符串供锁文件人工排查；本模块不持有真实分布式锁。
//! 命令名仅写入 started 日志，不进入 Context 持久字段。
//! 拷贝 Context 后各自 clone hint 切片，保证 worker 间隔离。

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

/// 单条 hint：键值对，会写入锁 Hint 字符串与日志字段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintField {
    pub Key: String,
    pub Value: String,
}

/// 一次 BR 操作的运行时上下文：ID、启动时刻、可演进的 hint 列表。
#[derive(Clone, Debug)]
pub struct Context {
    pub OperationID: String,
    pub StartedAt: SystemTime,
    /// 私有：按插入顺序保存；空 value 表示删除该键。
    hintFields: Vec<HintField>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            OperationID: String::new(),
            // Go `time.Time{}` is 0001-01-01T00:00:00Z, not Unix epoch.
            StartedAt: go_zero_time(),
            hintFields: Vec::new(),
        }
    }
}

/// 锁资源类型标签；字符串值与 Go 常量完全一致，跨语言互认。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockResourceType(pub &'static str);

/// 日志截断互斥锁：同一时刻仅允许一个 truncate 持有。
pub const LockResourceLogTruncateExclusive: LockResourceType =
    LockResourceType("log-truncate-exclusive");
/// 迁移元数据只读锁。
pub const LockResourceMigrationRead: LockResourceType = LockResourceType("migration-read");
/// 迁移元数据写锁。
pub const LockResourceMigrationWrite: LockResourceType = LockResourceType("migration-write");
/// 迁移追加写锁（append-only 场景）。
pub const LockResourceMigrationAppend: LockResourceType = LockResourceType("migration-append");

/// `LockMeta` 输出：OwnerID/LockType/Hint，供存储层写入锁文件。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockMetaInput {
    pub OwnerID: String,
    pub LockType: String,
    pub Hint: String,
}

/// Captured log entry for Go zaptest/observer-equivalent assertions.
/// 捕获的单条日志：级别、消息与字符串化字段，供 Filter 断言。
#[derive(Clone, Debug)]
pub struct CapturedLog {
    pub level: &'static str,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

// Thread-local sink so parallel rustc tests do not race (Go package tests are non-Parallel here).
// 线程本地缓冲：并行 rustc 测试互不干扰（Go 侧这些用例未开 Parallel）。
thread_local! {
    static LOG_CAPTURE: RefCell<Option<Vec<CapturedLog>>> = const { RefCell::new(None) };
}

/// Guard that restores the previous thread-local log capture sink on drop (Go t.Cleanup).
/// Drop 时恢复进入捕获前的 sink，避免泄漏到后续测试。
pub struct LogCaptureGuard {
    prev: Option<Vec<CapturedLog>>,
}

impl Drop for LogCaptureGuard {
    fn drop(&mut self) {
        // 无论测试成功失败都还原，对齐 Go t.Cleanup。
        LOG_CAPTURE.with(|c| {
            *c.borrow_mut() = self.prev.take();
        });
    }
}

/// ReplaceGlobals / observer.New equivalent: begin capturing Info/Warn into a buffer.
/// 开始捕获：替换全局 sink 为空 Vec，返回 Guard。
pub fn begin_log_capture() -> LogCaptureGuard {
    let prev = LOG_CAPTURE.with(|c| c.borrow_mut().replace(Vec::new()));
    LogCaptureGuard { prev }
}

/// Snapshot of captured logs while a [`LogCaptureGuard`] is active.
/// 未开启捕获时返回空切片，避免 panic。
pub fn captured_logs() -> Vec<CapturedLog> {
    LOG_CAPTURE.with(|c| c.borrow().clone().unwrap_or_default())
}

/// 若当前线程开启了 capture，则追加一条；否则写入生产 stderr 日志。
fn emit_log(level: &'static str, message: &str, fields: &[(&str, String)]) {
    let entry = CapturedLog {
        level,
        message: message.to_string(),
        fields: fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    };
    let captured = LOG_CAPTURE.with(|c| {
        let mut capture = c.borrow_mut();
        if let Some(buf) = capture.as_mut() {
            buf.push(entry.clone());
            true
        } else {
            false
        }
    });
    if !captured {
        eprintln!("{}", render_production_log(&entry));
    }
}

/// 无日志框架依赖时的稳定结构化输出；字段值使用 Go Quote 转义。
fn render_production_log(entry: &CapturedLog) -> String {
    let mut line = format!("{} {}", entry.level.to_ascii_uppercase(), entry.message);
    for (key, value) in &entry.fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&quote_go_string(value));
    }
    line
}

/// 精确匹配 message 文本（非子串），对应 zap observer FilterMessage。
fn filter_message<'a>(logs: &'a [CapturedLog], msg: &str) -> Vec<&'a CapturedLog> {
    logs.iter().filter(|e| e.message == msg).collect()
}

/// Test helper mirroring zap observer FilterMessage.
/// 返回拥有所有权的克隆列表，便于断言后继续持有。
pub fn filter_captured_message(logs: &[CapturedLog], msg: &str) -> Vec<CapturedLog> {
    filter_message(logs, msg).into_iter().cloned().collect()
}

/// 创建已初始化 Context：随机 UUID、当前时间，并打 “BR operation started” 信息日志。
pub fn NewContext(command: &str) -> Result<Context, String> {
    let operationID = uuid::Uuid::new_v4().to_string();
    let ctx = Context {
        OperationID: operationID,
        StartedAt: SystemTime::now(),
        hintFields: Vec::new(),
    };
    let host = hostname();
    // 字段集合与 Go 启动日志一致，供运维检索。
    emit_log(
        "info",
        "BR operation started",
        &[
            ("operation_id", ctx.OperationID.clone()),
            ("operation_started_at", format_time_rfc3339(ctx.StartedAt)),
            ("host", host),
            ("pid", std::process::id().to_string()),
            ("command", command.to_string()),
        ],
    );
    Ok(ctx)
}

/// 主机名解析：读取操作系统 hostname，失败时回退 `"unknown"`。
fn hostname() -> String {
    // `os.Hostname` does not consult HOSTNAME/COMPUTERNAME environment variables.
    hostname_from_command().unwrap_or_else(|| "unknown".to_string())
}

/// 调用系统 `hostname`；非零退出或空输出视为失败。
fn hostname_from_command() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

impl Context {
    /// 返回 hint 快照副本，避免外部持有可变别名。
    pub fn HintFields(&self) -> Vec<HintField> {
        self.hintFields.clone()
    }

    /// 设置/更新/删除 hint：空 key 忽略；未初始化时拒绝非空 value；空 value 删除键。
    pub fn SetHintField(&mut self, key: &str, value: &str) {
        if key.is_empty() {
            // 空键无意义，直接忽略（Go 同）。
            return;
        }
        if !value.is_empty() && !self.isInitialized() {
            // 未 NewContext 前不允许写入非空 hint，防止半初始化状态被锁引用。
            return;
        }

        let fieldIdx = self.hintFieldIndex(key);
        if value.is_empty() {
            if fieldIdx >= 0 {
                // Go clones the slice before mutating so value copies stay independent.
                // 先 clone 再 remove，保证历史副本与 Go 切片语义一致。
                self.hintFields = self.hintFields.clone();
                self.hintFields.remove(fieldIdx as usize);
            }
            return;
        }

        // 写入路径同样先 clone，避免与已导出的 HintFields 快照共享底层缓冲语义混淆。
        self.hintFields = self.hintFields.clone();
        if fieldIdx >= 0 {
            let oldValue = self.hintFields[fieldIdx as usize].Value.clone();
            if oldValue != value {
                // 值变化打 warn，便于排查运维误改。
                emit_log(
                    "warn",
                    "BR operation hint field changed",
                    &[
                        ("operation_id", self.OperationID.clone()),
                        ("operation_started_at", format_time_rfc3339(self.StartedAt)),
                        ("hint_key", key.to_string()),
                        ("old_value", oldValue),
                        ("new_value", value.to_string()),
                    ],
                );
            }
            self.hintFields[fieldIdx as usize].Value = value.to_string();
        } else {
            // 新键追加到末尾，保持插入序。
            self.hintFields.push(HintField {
                Key: key.to_string(),
                Value: value.to_string(),
            });
        }

        // 每次成功 resolve 都打 info，与 Go 观测点对齐。
        emit_log(
            "info",
            "BR operation hint field resolved",
            &[
                ("operation_id", self.OperationID.clone()),
                ("operation_started_at", format_time_rfc3339(self.StartedAt)),
                ("hint_key", key.to_string()),
                ("hint_value", value.to_string()),
            ],
        );
    }

    /// 已分配非空 OperationID 且 StartedAt 非占位零值。
    fn isInitialized(&self) -> bool {
        !self.OperationID.is_empty() && self.StartedAt != go_zero_time()
    }

    /// 线性查找 hint 下标；未找到返回 -1（对齐 Go int 索引习惯）。
    fn hintFieldIndex(&self, key: &str) -> isize {
        for (i, field) in self.hintFields.iter().enumerate() {
            if field.Key == key {
                return i as isize;
            }
        }
        -1
    }

    /// 组装锁元数据：校验 ID/时间/资源类型后生成 OwnerID、LockType、Hint。
    pub fn LockMeta(
        &self,
        resource: LockResourceType,
        hint: &str,
    ) -> Result<LockMetaInput, String> {
        if self.OperationID.is_empty() {
            return Err("operation ID is required".into());
        }
        if self.StartedAt == go_zero_time() {
            // 零时间不允许加锁，避免无启动时间的 Owner 冲突难排查。
            return Err("operation started time is required".into());
        }
        if resource.0.is_empty() {
            return Err("lock resource type is required".into());
        }

        Ok(LockMetaInput {
            OwnerID: self.OperationID.clone(),
            LockType: resource.0.to_string(),
            Hint: self.lockHint(hint),
        })
    }

    /// 拼接 Hint：启动时间 + 全部非空 hint + 可选 detail（Go strconv.Quote 风格）。
    fn lockHint(&self, detail: &str) -> String {
        let mut fields = vec![format!(
            "operation_started_at={}",
            format_time_rfc3339(self.StartedAt)
        )];
        for field in &self.hintFields {
            if !field.Key.is_empty() && !field.Value.is_empty() {
                fields.push(format!("{}={}", field.Key, field.Value));
            }
        }
        if !detail.is_empty() {
            fields.push(format!("detail={}", quote_go_string(detail)));
        }
        fields.join(" ")
    }
}

/// Format SystemTime as Go time.RFC3339 UTC (`2006-01-02T15:04:05Z07:00` with Z).
/// 输出固定 `Z` 后缀的 UTC RFC3339，不含亚秒。
pub fn format_time_rfc3339(t: SystemTime) -> String {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(dur) => dur.as_secs() as i64,
        Err(err) => {
            let dur = err.duration();
            // RFC3339 without fractions formats a pre-epoch subsecond in the
            // preceding civil second, just like Go's time.Format.
            -(dur.as_secs() as i64) - i64::from(dur.subsec_nanos() != 0)
        }
    };
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Build a UTC SystemTime from Y-M-D h:m:s (test fixture helper / Go time.Date).
/// 测试夹具：按 UTC 日历分量构造 SystemTime。
pub fn time_utc(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> SystemTime {
    let days = days_from_civil(year, month as i32, day as i32);
    let secs = days * 86_400 + (hour * 3600 + min * 60 + sec) as i64;
    if secs >= 0 {
        UNIX_EPOCH + std::time::Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH - std::time::Duration::from_secs(secs.unsigned_abs())
    }
}

/// `SystemTime` representation of Go's zero `time.Time` value.
fn go_zero_time() -> SystemTime {
    UNIX_EPOCH - std::time::Duration::from_secs(62_135_596_800)
}

/// Quote valid UTF-8 with Go `strconv.Quote` escape spellings.
fn quote_go_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '"' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '\u{7}' => quoted.push_str("\\a"),
            '\u{8}' => quoted.push_str("\\b"),
            '\u{c}' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{b}' => quoted.push_str("\\v"),
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {
                quoted.push_str(&format!("\\x{:02x}", ch as u32));
            }
            '\'' => quoted.push('\''),
            ch => {
                let escaped = ch.escape_debug().to_string();
                if let Some(hex) = escaped
                    .strip_prefix("\\u{")
                    .and_then(|s| s.strip_suffix('}'))
                {
                    let codepoint = u32::from_str_radix(hex, 16).expect("escape_debug hex");
                    if codepoint < 0x10000 {
                        quoted.push_str(&format!("\\u{codepoint:04x}"));
                    } else {
                        quoted.push_str(&format!("\\U{codepoint:08x}"));
                    }
                } else {
                    quoted.push(ch);
                }
            }
        }
    }
    quoted.push('"');
    quoted
}

// Howard Hinnant civil_from_days / days_from_civil (UTC).
// 公历日序号 ↔ 年月日；算法与 C++ date.h 同源，保证跨平台一致。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// 年月日 → 相对 Unix epoch 的日序号（可负）。
fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146_097 + (doe as i64) - 719_468
}
