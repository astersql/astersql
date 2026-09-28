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

//! CRR checkpoint service CLI config — mirrors `br/pkg/stream/crr/config/config.go`.
//! Uses a local FlagSet stand-in instead of pflag.
//!
//! 本模块对齐 Go `br/pkg/stream/crr/config`：把 CLI 标志落到 CRR checkpoint
//! 服务配置（任务名、重试间隔、计算器轮询与 meta 读并发）。
//! Rust 用本地 `FlagSet`（Define/Set/Get）替代 pflag，不解析 argv 字符串。

use std::collections::HashMap;
use std::time::Duration;

use astersql_br_pkg_stream_crr_internal_checkpoint::{
    CheckpointCalculatorConfig, DefaultMetaReadConcurrency, DefaultPollInterval,
};
use astersql_br_pkg_stream_crr_service::{Config as ServiceConfig, DefaultRetryInterval};

/// 上游 log backup 任务名标志。
pub const flagTaskName: &str = "task-name";
/// 服务出错或 watch 失败后的重试间隔标志。
pub const flagRetryInterval: &str = "retry-interval";
/// 计算器下游同步检查的轮询间隔标志。
pub const flagCalcPollInterval: &str = "calc.poll-interval";
/// 计算器读取 backupmeta 的并发度标志。
pub const flagCalcMetaReadConcurrency: &str = "calc.meta-read-concurrency";

/// CLI 层配置包装：内嵌服务侧 `ServiceConfig`。
#[derive(Clone, Debug)]
pub struct Config {
    /// 实际服务配置载荷。
    pub inner: ServiceConfig,
}

impl Default for Config {
    /// 委托 `DefaultConfig`，保证与显式默认构造一致。
    fn default() -> Self {
        DefaultConfig()
    }
}

impl Config {
    /// 上游任务名访问器。
    pub fn TaskName(&self) -> &str {
        &self.inner.CalculatorConfig.TaskName
    }

    /// 服务重试间隔。
    pub fn RetryInterval(&self) -> Duration {
        self.inner.RetryInterval
    }

    /// 计算器轮询间隔。
    pub fn PollInterval(&self) -> Duration {
        self.inner.CalculatorConfig.PollInterval
    }

    /// 读取 backupmeta 的并发度。
    pub fn MetaReadConcurrency(&self) -> i32 {
        self.inner.CalculatorConfig.MetaReadConcurrency
    }
}

/// 使用子包导出的 Default* 常量构造默认配置。
pub fn DefaultConfig() -> Config {
    Config {
        inner: ServiceConfig {
            CalculatorConfig: CheckpointCalculatorConfig {
                PollInterval: DefaultPollInterval,
                MetaReadConcurrency: DefaultMetaReadConcurrency,
                ..Default::default()
            },
            RetryInterval: DefaultRetryInterval,
        },
    }
}

/// Local FlagSet mirroring the pflag getters used by Go `DefineFlags` / `Parse`.
///
/// 本地 FlagSet：分离“定义默认值”与“覆盖值”，Get* 优先读覆盖再回落默认。
/// 未 Define 就 Get 会报错，对齐 pflag 未定义访问行为。
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    /// 字符串覆盖值（SetString）。
    strings: HashMap<String, String>,
    /// Duration 覆盖值。
    durations: HashMap<String, Duration>,
    /// 整型覆盖值。
    ints: HashMap<String, i32>,
    /// 字符串默认值（String 定义）。
    defs_string: HashMap<String, String>,
    /// Duration 默认值。
    defs_duration: HashMap<String, Duration>,
    /// 整型默认值。
    defs_int: HashMap<String, i32>,
}

impl FlagSet {
    /// 空 FlagSet，需先 DefineFlags 再 Get/Parse。
    pub fn new() -> Self {
        Self::default()
    }

    /// 定义字符串标志及其默认值；help 仅占位对齐 Go 签名。
    pub fn String(&mut self, name: &str, default: &str, _help: &str) {
        self.defs_string
            .insert(name.to_string(), default.to_string());
    }

    /// 定义 Duration 标志及其默认值。
    pub fn Duration(&mut self, name: &str, default: Duration, _help: &str) {
        self.defs_duration.insert(name.to_string(), default);
    }

    /// 定义整型标志及其默认值。
    pub fn Int(&mut self, name: &str, default: i32, _help: &str) {
        self.defs_int.insert(name.to_string(), default);
    }

    /// 覆盖字符串标志（模拟 CLI 解析结果）。
    pub fn SetString(&mut self, name: &str, value: impl Into<String>) {
        self.strings.insert(name.to_string(), value.into());
    }

    /// 覆盖 Duration 标志。
    pub fn SetDuration(&mut self, name: &str, value: Duration) {
        self.durations.insert(name.to_string(), value);
    }

    /// 覆盖整型标志。
    pub fn SetInt(&mut self, name: &str, value: i32) {
        self.ints.insert(name.to_string(), value);
    }

    /// Parse long-form command-line flags, matching the pflag forms used by Go.
    ///
    /// Both `--name value` and `--name=value` are accepted. Positional arguments
    /// are ignored here because this package only consumes the registered flags.
    pub fn Parse(&mut self, args: &[&str]) -> Result<(), String> {
        let mut index = 0;
        while index < args.len() {
            let arg = args[index];
            if arg == "--" {
                break;
            }
            if arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") {
                return Err(format!("unknown shorthand flag: {arg}"));
            }
            if !arg.starts_with("--") {
                index += 1;
                continue;
            }

            let flag = &arg[2..];
            let (name, inline_value) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (flag, None),
            };
            let is_string = self.defs_string.contains_key(name);
            let is_duration = self.defs_duration.contains_key(name);
            let is_int = self.defs_int.contains_key(name);
            if !is_string && !is_duration && !is_int {
                return Err(format!("unknown flag: --{name}"));
            }

            let value = match inline_value {
                Some(value) => value,
                None => {
                    index += 1;
                    args.get(index)
                        .copied()
                        .ok_or_else(|| format!("flag needs an argument: --{name}"))?
                }
            };

            if is_string {
                self.SetString(name, value);
            } else if is_duration {
                self.SetDuration(name, parse_duration(value)?);
            } else {
                let parsed = value
                    .parse::<i32>()
                    .map_err(|error| format!("invalid argument {value:?} for --{name}: {error}"))?;
                self.SetInt(name, parsed);
            }
            index += 1;
        }
        Ok(())
    }

    /// 读取字符串：覆盖优先，否则默认；皆无则报未定义。
    pub fn GetString(&self, name: &str) -> Result<String, String> {
        if let Some(v) = self.strings.get(name) {
            return Ok(v.clone());
        }
        self.defs_string
            .get(name)
            .cloned()
            .ok_or_else(|| format!("flag accessed but not defined: {name}"))
    }

    /// 读取 Duration：覆盖优先，否则默认。
    pub fn GetDuration(&self, name: &str) -> Result<Duration, String> {
        if let Some(v) = self.durations.get(name) {
            return Ok(*v);
        }
        self.defs_duration
            .get(name)
            .copied()
            .ok_or_else(|| format!("flag accessed but not defined: {name}"))
    }

    /// 读取整型：覆盖优先，否则默认。
    pub fn GetInt(&self, name: &str) -> Result<i32, String> {
        if let Some(v) = self.ints.get(name) {
            return Ok(*v);
        }
        self.defs_int
            .get(name)
            .copied()
            .ok_or_else(|| format!("flag accessed but not defined: {name}"))
    }
}

/// Parse the non-negative subset of Go duration syntax representable by
/// `std::time::Duration`, including compound and fractional units.
fn parse_duration(value: &str) -> Result<Duration, String> {
    let original = value;
    let value = value.strip_prefix('+').unwrap_or(value);
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    if value.is_empty() || value.starts_with('-') {
        return Err(format!("invalid duration {original:?}"));
    }

    let mut rest = value;
    let mut total_nanos = 0_u128;
    while !rest.is_empty() {
        let integer_end = rest.bytes().take_while(u8::is_ascii_digit).count();
        let has_integer = integer_end != 0;
        let integer = if has_integer {
            rest[..integer_end]
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {original:?}"))?
        } else {
            0
        };
        rest = &rest[integer_end..];

        let mut fraction = 0_u64;
        let mut fraction_scale = 1_f64;
        let mut has_fraction = false;
        if let Some(after_dot) = rest.strip_prefix('.') {
            let fraction_end = after_dot.bytes().take_while(u8::is_ascii_digit).count();
            has_fraction = fraction_end != 0;
            for digit in after_dot[..fraction_end].bytes() {
                if fraction <= (1_u64 << 63) / 10 {
                    let next = fraction * 10 + u64::from(digit - b'0');
                    if next <= 1_u64 << 63 {
                        fraction = next;
                        fraction_scale *= 10.0;
                    }
                }
            }
            rest = &after_dot[fraction_end..];
        }
        if !has_integer && !has_fraction {
            return Err(format!("invalid duration {original:?}"));
        }

        let (unit, nanos) = [
            ("ns", 1_u128),
            ("us", 1_000_u128),
            ("µs", 1_000_u128),
            ("μs", 1_000_u128),
            ("ms", 1_000_000_u128),
            ("s", 1_000_000_000_u128),
            ("m", 60_000_000_000_u128),
            ("h", 3_600_000_000_000_u128),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))
        .ok_or_else(|| format!("invalid duration {original:?}"))?;
        let fraction_nanos = (fraction as f64 * (nanos as f64 / fraction_scale)) as u128;
        let component = integer
            .checked_mul(nanos)
            .and_then(|whole| whole.checked_add(fraction_nanos))
            .ok_or_else(|| format!("invalid duration {original:?}"))?;
        total_nanos = total_nanos
            .checked_add(component)
            .ok_or_else(|| format!("invalid duration {original:?}"))?;
        if total_nanos > i64::MAX as u128 {
            return Err(format!("invalid duration {original:?}"));
        }
        rest = &rest[unit.len()..];
    }

    Ok(Duration::from_nanos(total_nanos as u64))
}

/// 注册 CRR checkpoint 相关标志，默认值取自 `DefaultConfig`。
pub fn DefineFlags(flags: &mut FlagSet) {
    let defaults = DefaultConfig();
    flags.String(
        flagTaskName,
        "",
        "The name of the upstream log backup task.",
    );
    flags.Duration(
        flagRetryInterval,
        defaults.RetryInterval(),
        "The retry interval after crr-checkpoint service errors or watch failures.",
    );
    flags.Duration(
        flagCalcPollInterval,
        defaults.PollInterval(),
        "The calculator polling interval for downstream sync checks.",
    );
    flags.Int(
        flagCalcMetaReadConcurrency,
        defaults.MetaReadConcurrency(),
        "The calculator concurrency for reading backupmeta files.",
    );
}

impl Config {
    /// 从 FlagSet 填充自身：先重置为默认，再逐项 Get 覆盖。
    ///
    /// 与 Go `cfg.Parse(flags)` 顺序一致；任一 Get 失败则整体失败。
    pub fn Parse(&mut self, flags: &FlagSet) -> Result<(), String> {
        // 先回到默认，避免残留旧字段
        *self = DefaultConfig();
        self.inner.CalculatorConfig.TaskName = flags.GetString(flagTaskName)?;
        self.inner.RetryInterval = flags.GetDuration(flagRetryInterval)?;
        self.inner.CalculatorConfig.PollInterval = flags.GetDuration(flagCalcPollInterval)?;
        self.inner.CalculatorConfig.MetaReadConcurrency =
            flags.GetInt(flagCalcMetaReadConcurrency)?;
        Ok(())
    }
}
