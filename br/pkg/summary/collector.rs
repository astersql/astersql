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

//! BR 摘要日志收集器，移植自 `br/pkg/summary/collector.go`。
//! 在备份/恢复过程中聚合成功/失败单元、耗时与计数，最终输出 success/failed summary。
//! 全局单例经 Mutex 保护；日志后端可注入，便于单测捕获字段。
//! 成功路径输出 human-size 与平均速度；失败路径区分取消与真实错误。
//! 字段名中的空格在落日志前会被替换为 `-`（见 `log_key_for`）。

use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

// BackupUnit tells summary in backup
/// 摘要单元名：备份流程。
pub const BackupUnit: &str = "backup";
// RestoreUnit tells summary in restore
/// 摘要单元名：恢复流程。
pub const RestoreUnit: &str = "restore";

// TotalKV is a field we collect during backup/restore
/// 采集字段：总 KV 条数。
pub const TotalKV: &str = "total kv";
// TotalBytes is a field we collect during backup/restore
/// 采集字段：总字节数；成功摘要中会换算成 human size 与平均速度。
pub const TotalBytes: &str = "total bytes";
// BackupDataSize is a field we collect after backup finish
/// 采集字段：备份压缩后数据量。
pub const BackupDataSize: &str = "backup data size(after compressed)";
// RestoreDataSize is a field we collection after restore finish
/// 采集字段：恢复压缩后数据量。
pub const RestoreDataSize: &str = "restore data size(after compressed)";
// SkippedKVCountByCheckpoint is a field we skip during backup/restore
/// 采集字段：因检查点跳过的 KV 条数。
pub const SkippedKVCountByCheckpoint: &str = "skipped kv count by checkpoint";
// SkippedBytesByCheckpoint is a field we skip during backup/restore
/// 采集字段：因检查点跳过的字节数。
pub const SkippedBytesByCheckpoint: &str = "skipped bytes by checkpoint";

/// 对应 Go `context.Canceled` 的哨兵错误，摘要时单独统计为 cancel-unit。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextCanceled;

impl fmt::Display for ContextCanceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context canceled")
    }
}

impl StdError for ContextCanceled {}

/// 摘要收集用的错误类型，对齐 Go 侧 `error` 接口（可跨线程共享）。
pub type SummaryError = Arc<dyn StdError + Send + Sync>;

/// `CollectSuccessUnit` 的第三参，对应 Go `any` 中 Duration / uint64 两分支。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryValue {
    Duration(Duration),
    UInt64(u64),
}

/// 结构化日志字段，覆盖 summary 使用的 zap 字段子集。
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub value: FieldValue,
}

/// 字段值枚举：整型、时长、字符串、无符号与错误字符串。
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Int(i64),
    Duration(Duration),
    String(String),
    Uint64(u64),
    Error(String),
}

/// 精简 zap 风格构造器，避免直接依赖 pingcap/log 的 Field API。
pub mod zap {
    use super::{Field, FieldValue, SummaryError};
    use std::time::Duration;

    /// 构造整型字段（入参 i32，内部存 i64）。
    pub fn Int(key: &str, value: i32) -> Field {
        Field {
            key: key.to_string(),
            value: FieldValue::Int(value as i64),
        }
    }

    /// 构造时长字段。
    pub fn Duration(key: &str, value: Duration) -> Field {
        Field {
            key: key.to_string(),
            value: FieldValue::Duration(value),
        }
    }

    /// 构造字符串字段。
    pub fn String(key: &str, value: impl Into<String>) -> Field {
        Field {
            key: key.to_string(),
            value: FieldValue::String(value.into()),
        }
    }

    /// 构造无符号整型字段。
    pub fn Uint64(key: &str, value: u64) -> Field {
        Field {
            key: key.to_string(),
            value: FieldValue::Uint64(value),
        }
    }

    /// 构造错误字段：序列化为 `err.to_string()`。
    pub fn Error(key: &str, err: &SummaryError) -> Field {
        Field {
            key: key.to_string(),
            value: FieldValue::Error(err.to_string()),
        }
    }
}

/// 可注入的日志回调：`(message, fields)`。
pub type LogFunc = Arc<dyn Fn(&str, &[Field]) + Send + Sync>;

/// 进程内全局日志与 InitLogger 替身；默认 no-op，匹配隔离单测未初始化 pingcap/log。
mod log {
    use super::{Field, LogFunc};
    use std::sync::{Arc, LazyLock, RwLock};

    #[derive(Clone, Debug, Default)]
    pub struct Config;

    /// 持有 LogFunc 的轻量 Logger。
    pub struct Logger {
        log: LogFunc,
    }

    impl Logger {
        /// 输出 Info 级摘要消息。
        pub fn Info(&self, msg: &str, fields: Vec<Field>) {
            (self.log)(msg, &fields);
        }

        /// 取出底层回调，供 InitCollector 双重写入（logger + 全局）。
        pub fn log_func(&self) -> LogFunc {
            Arc::clone(&self.log)
        }
    }

    static GLOBAL_LOG: LazyLock<RwLock<LogFunc>> = LazyLock::new(|| {
        RwLock::new(Arc::new(|_msg: &str, _fields: &[Field]| {
            // Default global logger is a no-op, matching uninitialized pingcap/log usage
            // in isolated unit tests.
            // 默认空实现，避免测试环境强制初始化真实日志后端。
        }))
    });

    /// 经全局锁调用当前 LogFunc。
    pub fn Info(msg: &str, fields: &[Field]) {
        GLOBAL_LOG.read().expect("summary global log lock poisoned")(msg, fields);
    }

    /// 初始化 Logger：其 Info 转发到全局 `Info`。
    pub fn InitLogger(_conf: Config) -> Result<(Logger, ()), String> {
        let log = Arc::new(|msg: &str, fields: &[Field]| {
            Info(msg, fields);
        });
        Ok((Logger { log }, ()))
    }

    /// 测试/注入用：替换全局 LogFunc。
    pub fn set_global_log(log: LogFunc) {
        *GLOBAL_LOG
            .write()
            .expect("summary global log lock poisoned") = log;
    }
}

/// 字节人类可读格式，对齐 `github.com/docker/go-units.HumanSize`（十进制千分位）。
pub mod units {
    const DECIMAP_ABBRS: [&str; 9] = ["B", "kB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];

    /// Mirrors `github.com/docker/go-units.HumanSize`.
    /// 将字节数格式化为最多约 4 位有效数字 + 单位后缀。
    pub fn HumanSize(size: f64) -> String {
        let mut value = size;
        let mut unit_index = 0usize;
        while value >= 1000.0 && unit_index < DECIMAP_ABBRS.len() - 1 {
            value /= 1000.0;
            unit_index += 1;
        }
        format!(
            "{}{}",
            format_significant(value, 4),
            DECIMAP_ABBRS[unit_index]
        )
    }

    /// 按有效数字精度舍入；整数则去掉小数部分。
    fn format_significant(value: f64, precision: i32) -> String {
        if value.is_nan() {
            return "NaN".to_string();
        }
        if value == f64::INFINITY {
            return "+Inf".to_string();
        }
        if value == f64::NEG_INFINITY {
            return "-Inf".to_string();
        }
        if value == 0.0 {
            return if value.is_sign_negative() {
                "-0".to_string()
            } else {
                "0".to_string()
            };
        }

        // Go's `%.*g` uses decimal notation for exponents in [-4, precision)
        // and scientific notation otherwise. Formatting with the calculated
        // number of decimal places preserves significant digits below 0.001,
        // where a fixed four-decimal representation would lose information.
        let exponent = value.abs().log10().floor() as i32;
        if exponent >= -4 && exponent < precision {
            let decimal_places = (precision - 1 - exponent).max(0) as usize;
            let formatted = format!("{value:.decimal_places$}");
            return if formatted.contains('.') {
                formatted
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            } else {
                formatted
            };
        }

        let scientific = format!("{value:.*e}", (precision - 1) as usize);
        let (mantissa, raw_exponent) = scientific
            .split_once('e')
            .expect("Rust scientific formatting always contains an exponent");
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let exponent: i32 = raw_exponent
            .parse()
            .expect("Rust scientific formatting emits a numeric exponent");
        format!("{mantissa}e{exponent:+03}")
    }
}

/// 摘要收集器接口，对齐 Go `LogCollector`。
/// 命令行与库代码通过包级函数操作全局实现，测试可 `SetLogCollector` 替换。
pub trait LogCollector: Send {
    /// 设置当前单元标签（影响业务侧分类，未必直接出现在每条日志）。
    fn SetUnit(&mut self, unit: &str);
    /// 记录成功 range/单元；`unit_count` 仅 Duration 分支累加成功计数。
    fn CollectSuccessUnit(&mut self, name: &str, unit_count: i32, arg: SummaryValue);
    /// 记录失败原因；同名只保留首次。
    fn CollectFailureUnit(&mut self, name: &str, reason: SummaryError);
    /// 累加耗时字段。
    fn CollectDuration(&mut self, name: &str, t: Duration);
    /// 累加整型字段。
    fn CollectInt(&mut self, name: &str, t: i32);
    /// 累加无符号字段。
    fn CollectUInt(&mut self, name: &str, t: u64);
    /// 设置最终成功标志。
    fn SetSuccessStatus(&mut self, success: bool);
    /// 返回自启动以来的耗时。
    fn NowDureTime(&self) -> Duration;
    /// 把起始时间前移，合并前置阶段耗时。
    fn AdjustStartTimeToEarlierTime(&mut self, t: Duration);
    /// 刷出 summary 并清理部分状态。
    fn Summary(&mut self, name: &str);
    /// 直接写一条带字段的日志。
    fn Log(&self, msg: &str, fields: &[Field]);
}

/// 具体收集器状态：成功/失败计数、各类聚合 map 与起止时间。
/// 字段与 Go 结构体一一对应，便于对照 `collector.go`。
struct logCollector {
    /// 当前单元名（backup/restore）。
    unit: String,
    success_unit_count: i32,
    failure_unit_count: i32,
    /// 成功单元耗时累加（按 name）。
    success_costs: HashMap<String, Duration>,
    /// 成功数据量累加（TotalBytes 等）。
    success_data: HashMap<String, u64>,
    /// 失败原因（name → 首次错误）。
    failure_reasons: HashMap<String, SummaryError>,
    durations: HashMap<String, Duration>,
    ints: HashMap<String, i32>,
    uints: HashMap<String, u64>,
    success_status: bool,
    start_time: Instant,
    log: LogFunc,
}

/// 进程级全局收集器；包内 API 经 `with_collector*` 访问。
static COLLECTOR: LazyLock<Mutex<Box<dyn LogCollector>>> =
    LazyLock::new(|| Mutex::new(NewLogCollector(default_log_func())));

/// 默认日志：转发到 `log::Info`。
fn default_log_func() -> LogFunc {
    Arc::new(|msg: &str, fields: &[Field]| {
        log::Info(msg, fields);
    })
}

/// 持锁执行闭包，无返回值。
pub(crate) fn with_collector<F>(f: F)
where
    F: FnOnce(&mut dyn LogCollector),
{
    let mut guard = COLLECTOR.lock().expect("summary collector mutex poisoned");
    f(guard.as_mut());
}

/// 持锁执行闭包并返回结果。
pub(crate) fn with_collector_result<F, T>(f: F) -> T
where
    F: FnOnce(&mut dyn LogCollector) -> T,
{
    let mut guard = COLLECTOR.lock().expect("summary collector mutex poisoned");
    f(guard.as_mut())
}

/// InitCollector initilize global collector instance.
/// 初始化全局收集器；`has_log_file` 为真时尝试 InitLogger 并双重写入。
pub fn InitCollector(has_log_file: bool) {
    let log_f = if has_log_file {
        match log::InitLogger(log::Config::default()) {
            Ok((logger, _)) => {
                let logger_log = logger.log_func();
                // 同时写 Logger 与全局 Info，兼容 Go 侧双通道行为。
                Arc::new(move |msg: &str, fields: &[Field]| {
                    logger_log(msg, fields);
                    log::Info(msg, fields);
                }) as LogFunc
            }
            // Init 失败回退默认 no-op 链，避免启动中断。
            Err(_) => default_log_func(),
        }
    } else {
        default_log_func()
    };
    SetLogCollector(NewLogCollector(log_f));
}

/// NewLogCollector returns a new LogCollector.
/// 构造空状态收集器，`start_time` 取当前时刻。
pub fn NewLogCollector(logf: LogFunc) -> Box<dyn LogCollector> {
    Box::new(logCollector {
        unit: String::new(),
        success_unit_count: 0,
        failure_unit_count: 0,
        success_costs: HashMap::new(),
        success_data: HashMap::new(),
        failure_reasons: HashMap::new(),
        durations: HashMap::new(),
        ints: HashMap::new(),
        uints: HashMap::new(),
        success_status: false,
        log: logf,
        start_time: Instant::now(),
    })
}

impl LogCollector for logCollector {
    /// 设置当前业务单元名（backup/restore 等）。
    fn SetUnit(&mut self, unit: &str) {
        self.unit = unit.to_string();
    }

    /// 成功单元：Duration 累加耗时并增加成功计数；UInt64 累加到 success_data。
    fn CollectSuccessUnit(&mut self, name: &str, unit_count: i32, arg: SummaryValue) {
        match arg {
            SummaryValue::Duration(v) => {
                self.success_unit_count += unit_count;
                *self
                    .success_costs
                    .entry(name.to_string())
                    .or_insert(Duration::ZERO) += v;
            }
            SummaryValue::UInt64(v) => {
                *self.success_data.entry(name.to_string()).or_insert(0) += v;
            }
        }
    }

    /// 失败单元：同名只记首次原因（Vacant），并增加失败计数。
    fn CollectFailureUnit(&mut self, name: &str, reason: SummaryError) {
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(entry) = self.failure_reasons.entry(name.to_string()) {
            entry.insert(reason);
            self.failure_unit_count += 1;
        }
    }

    /// 累加命名耗时。
    fn CollectDuration(&mut self, name: &str, t: Duration) {
        *self
            .durations
            .entry(name.to_string())
            .or_insert(Duration::ZERO) += t;
    }

    /// 累加命名整型计数。
    fn CollectInt(&mut self, name: &str, t: i32) {
        *self.ints.entry(name.to_string()).or_insert(0) += t;
    }

    /// 累加命名无符号计数。
    fn CollectUInt(&mut self, name: &str, t: u64) {
        *self.uints.entry(name.to_string()).or_insert(0) += t;
    }

    /// 标记整体是否成功；失败摘要路径会参考该标志。
    fn SetSuccessStatus(&mut self, success: bool) {
        self.success_status = success;
    }

    /// 自 `start_time` 起的已用时长。
    fn NowDureTime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// 将起始时间前移 `t`，用于把前置阶段耗时并入总时长。
    fn AdjustStartTimeToEarlierTime(&mut self, t: Duration) {
        self.start_time -= t;
    }

    /// 输出失败或成功摘要；结束后清空部分聚合 map（与 Go 一致）。
    fn Summary(&mut self, name: &str) {
        let mut log_fields =
            Vec::with_capacity(self.durations.len() + self.ints.len() + self.uints.len() + 3);

        // 总是附带 range 成功/失败计数。
        log_fields.push(zap::Int(
            "total-ranges",
            self.failure_unit_count + self.success_unit_count,
        ));
        log_fields.push(zap::Int("ranges-succeed", self.success_unit_count));
        log_fields.push(zap::Int("ranges-failed", self.failure_unit_count));

        for (key, val) in &self.durations {
            log_fields.push(zap::Duration(&log_key_for(key), *val));
        }
        for (key, val) in &self.ints {
            log_fields.push(zap::Int(&log_key_for(key), *val));
        }
        for (key, val) in &self.uints {
            log_fields.push(zap::Uint64(&log_key_for(key), *val));
        }

        // 有失败原因或未置成功：走 failed summary；取消类错误单独计数。
        if !self.failure_reasons.is_empty() || !self.success_status {
            let mut canceled_units = 0;
            for (unit_name, reason) in &self.failure_reasons {
                if !is_context_canceled(reason) {
                    log_fields.push(zap::String("unit-name", unit_name.clone()));
                    log_fields.push(zap::Error("error", reason));
                } else {
                    canceled_units += 1;
                }
            }
            log::Info("units canceled", &[zap::Int("cancel-unit", canceled_units)]);
            (self.log)(format!("{name} failed summary").as_str(), &log_fields);
            // 失败路径也清空聚合，避免泄漏到下次 Summary。
            self.durations = HashMap::new();
            self.ints = HashMap::new();
            self.success_costs = HashMap::new();
            self.failure_reasons = HashMap::new();
            return;
        }

        let total_dure_time = self.start_time.elapsed();
        log_fields.push(zap::Duration("total-take", total_dure_time));
        for (data_name, data) in &self.success_data {
            // TotalBytes：人类可读大小 + 平均速度（字节/总秒）。
            if data_name == TotalBytes {
                log_fields.push(zap::String("total-kv-size", units::HumanSize(*data as f64)));
                log_fields.push(zap::String(
                    "average-speed",
                    format!(
                        "{}/s",
                        units::HumanSize(*data as f64 / total_dure_time.as_secs_f64())
                    ),
                ));
                continue;
            }
            if data_name == SkippedBytesByCheckpoint {
                log_fields.push(zap::String(
                    "skipped-kv-size-by-checkpoint",
                    units::HumanSize(*data as f64),
                ));
                continue;
            }
            // 无 range 且未成功：输出 “Nothing to bakcup” 文案（保留 Go 拼写）。
            if data_name == BackupDataSize {
                if self.failure_unit_count + self.success_unit_count == 0 && !self.success_status {
                    log_fields.push(zap::String("Result", "Nothing to bakcup"));
                } else {
                    log_fields.push(zap::String(
                        &log_key_for(BackupDataSize),
                        units::HumanSize(*data as f64),
                    ));
                }
                continue;
            }
            if data_name == RestoreDataSize {
                if self.failure_unit_count + self.success_unit_count == 0 && !self.success_status {
                    log_fields.push(zap::String("Result", "Nothing to restore"));
                } else {
                    log_fields.push(zap::String(
                        &log_key_for(RestoreDataSize),
                        units::HumanSize(*data as f64),
                    ));
                }
                continue;
            }
            // 其余 success_data 以 Uint64 原样输出。
            log_fields.push(zap::Uint64(&log_key_for(data_name), *data));
        }

        (self.log)(format!("{name} success summary").as_str(), &log_fields);
        self.durations = HashMap::new();
        self.ints = HashMap::new();
        self.success_costs = HashMap::new();
        self.failure_reasons = HashMap::new();
    }

    /// 透传到底层 LogFunc。
    fn Log(&self, msg: &str, fields: &[Field]) {
        (self.log)(msg, fields);
    }
}

/// 日志键规范化：空格替换为 `-`，与 Go `logKey` 行为一致。
pub fn log_key_for(key: &str) -> String {
    key.replace(' ', "-")
}

/// SetLogCollector allow pass LogCollector outside.
/// 替换全局收集器实例（测试注入自定义实现时使用）。
pub fn SetLogCollector(l: Box<dyn LogCollector>) {
    *COLLECTOR.lock().expect("summary collector mutex poisoned") = l;
}

/// 沿 error source 链识别 context canceled 哨兵类型。
fn is_context_canceled(err: &SummaryError) -> bool {
    let mut current: Option<&(dyn StdError + 'static)> = Some(err.as_ref());
    while let Some(e) = current {
        if e.downcast_ref::<ContextCanceled>().is_some() {
            return true;
        }
        current = e.source();
    }
    false
}

#[cfg(test)]
/// 测试辅助：重置全局收集器为默认空状态。
pub(crate) fn reset_global_collector_for_test() {
    *COLLECTOR.lock().expect("summary collector mutex poisoned") =
        NewLogCollector(default_log_func());
}
