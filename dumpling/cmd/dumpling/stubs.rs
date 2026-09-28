// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Local arm64-safe stubs for `pflag` / prometheus collectors used by dumpling CLI.
//! Network, OS process collectors, and real prometheus are mocked at this boundary.
//!
//! 这个文件不是完整重写 `pflag` 或 prometheus，而是只保留 dumpling CLI 运行
//! 和 parity test 真正依赖到的最小行为集合。
//! 目标是让 arm64-safe 版本能稳定复现 Go `main.go` 的控制流与可观测副作用，
//! 同时明确哪些能力只是占位，避免后续误以为这里已经等价支持全量特性。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use astersql_dumpling_export::Registry;

/// Mirrors Go `export.FlagHelp`.
/// 保留同名常量可以减少 CLI 主流程和测试里的魔法字符串。
pub const FlagHelp: &str = "help";

#[derive(Clone, Debug)]
// 只实现 dumpling 当前用到的几种 flag 值类型，避免做无意义的大而全抽象。
pub enum FlagValue {
    // 布尔开关通常对应 `--help`、`--no-data` 这类显式启停项。
    Bool(bool),
    // Int 用于线程数、端口等需要允许负值校验前进入解析器的场景。
    Int(i32),
    // Uint64 覆盖 statement-size、rows 这类只接受非负大整数的参数。
    Uint64(u64),
    String(String),
    // StringSlice 对应 database/filter/tables-list 等可重复或逗号分隔输入。
    StringSlice(Vec<String>),
    // StringToString 目前主要服务 `--params k=v,...`。
    StringToString(HashMap<String, String>),
    Duration(Duration),
}

#[derive(Clone, Debug)]
// 内部 Flag 记录除值以外，还追踪 changed/hidden 这些 pflag 常见状态位。
struct Flag {
    name: String,
    shorthand: Option<char>,
    usage: String,
    value: FlagValue,
    changed: bool,
    hidden: bool,
}

/// Minimal `pflag.FlagSet` for dumpling CLI parsing.
/// 这里关注的是“兼容 dumpling 所需 API”，不是复刻全部 pflag 行为。
#[derive(Clone, Default)]
pub struct FlagSet {
    flags: HashMap<String, Flag>,
    shorthand: HashMap<char, String>,
    args: Vec<String>,
    usage: Option<Arc<dyn Fn(&FlagSet) + Send + Sync>>,
}

impl std::fmt::Debug for FlagSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // usage 回调不可直接打印，调试输出里只展示它是否存在。
        f.debug_struct("FlagSet")
            .field("flags", &self.flags)
            .field("shorthand", &self.shorthand)
            .field("args", &self.args)
            .field("usage", &self.usage.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

impl FlagSet {
    pub fn new() -> Self {
        // 默认构造即空命令行状态，后续由 DefineFlags 逐步填充。
        Self::default()
    }

    pub fn set_usage<F>(&mut self, f: F)
    where
        F: Fn(&FlagSet) + Send + Sync + 'static,
    {
        // usage 作为闭包保存，模拟 Go 中可被外层重写的 `pflag.Usage`。
        self.usage = Some(Arc::new(f));
    }

    pub fn Usage(&self) {
        // 有自定义 usage 时优先执行；否则退回默认 flag 列表打印。
        // 这样主流程既能输出 Go 原版说明，也保留通用回退路径。
        if let Some(u) = &self.usage {
            u(self);
        } else {
            self.PrintDefaults();
        }
    }

    pub fn PrintDefaults(&self) {
        // 为了让帮助输出稳定可测，这里先按 flag 名排序再逐条打印。
        let mut names: Vec<_> = self.flags.keys().cloned().collect();
        names.sort();
        for name in names {
            let f = &self.flags[&name];
            if f.hidden {
                // 隐藏 flag 仍可解析，但不应该出现在帮助信息里。
                continue;
            }
            let short = f.shorthand.map(|c| format!("-{c}, ")).unwrap_or_default();
            eprintln!("      {short}--{name}\t{}", f.usage);
        }
    }

    fn define(&mut self, name: &str, shorthand: Option<char>, usage: &str, value: FlagValue) {
        // shorthand 独立建索引，是为了后续解析 `-P` 这类短参数时能快速查表。
        // 同名 flag 后定义会覆盖前定义，这与 Go 初始化阶段“后写生效”一致。
        if let Some(c) = shorthand {
            self.shorthand.insert(c, name.to_string());
        }
        self.flags.insert(
            name.to_string(),
            Flag {
                name: name.to_string(),
                shorthand,
                usage: usage.to_string(),
                value,
                changed: false,
                hidden: false,
            },
        );
    }

    pub fn BoolP(&mut self, name: &str, shorthand: char, value: bool, usage: &str) -> bool {
        // 返回默认值只是为了贴近 Go `BoolP` 的调用形态。
        self.define(name, Some(shorthand), usage, FlagValue::Bool(value));
        value
    }

    pub fn Bool(&mut self, name: &str, value: bool, usage: &str) {
        self.define(name, None, usage, FlagValue::Bool(value));
    }

    pub fn StringP(&mut self, name: &str, shorthand: char, value: &str, usage: &str) {
        // 字符串 flag 会复制成内部 String，避免借用外部临时值。
        self.define(
            name,
            Some(shorthand),
            usage,
            FlagValue::String(value.to_string()),
        );
    }

    pub fn String(&mut self, name: &str, value: &str, usage: &str) {
        self.define(name, None, usage, FlagValue::String(value.to_string()));
    }

    pub fn StringSliceP(&mut self, name: &str, shorthand: char, value: Vec<String>, usage: &str) {
        // slice 默认值直接整体保存，解析阶段再做 append 语义。
        self.define(name, Some(shorthand), usage, FlagValue::StringSlice(value));
    }

    pub fn StringSlice(&mut self, name: &str, value: Vec<String>, usage: &str) {
        self.define(name, None, usage, FlagValue::StringSlice(value));
    }

    pub fn IntP(&mut self, name: &str, shorthand: char, value: i32, usage: &str) {
        self.define(name, Some(shorthand), usage, FlagValue::Int(value));
    }

    pub fn Int(&mut self, name: &str, value: i32, usage: &str) {
        self.define(name, None, usage, FlagValue::Int(value));
    }

    pub fn Uint64P(&mut self, name: &str, shorthand: char, value: u64, usage: &str) {
        self.define(name, Some(shorthand), usage, FlagValue::Uint64(value));
    }

    pub fn Uint64(&mut self, name: &str, value: u64, usage: &str) {
        self.define(name, None, usage, FlagValue::Uint64(value));
    }

    pub fn Duration(&mut self, name: &str, value: Duration, usage: &str) {
        self.define(name, None, usage, FlagValue::Duration(value));
    }

    pub fn StringToString(&mut self, name: &str, value: HashMap<String, String>, usage: &str) {
        self.define(name, None, usage, FlagValue::StringToString(value));
    }

    pub fn MarkHidden(&mut self, name: &str) -> Result<(), String> {
        // hidden 只是帮助输出层面的开关，不影响解析与取值。
        let f = self
            .flags
            .get_mut(name)
            .ok_or_else(|| format!("flag {name} not defined"))?;
        f.hidden = true;
        Ok(())
    }

    pub fn Changed(&self, name: &str) -> bool {
        // 模板校验等逻辑要区分“用户显式传了默认值”和“根本没改过”。
        self.flags.get(name).map(|f| f.changed).unwrap_or(false)
    }

    pub fn GetBool(&self, name: &str) -> Result<bool, String> {
        // getter 同时承担类型保护，防止调用方错把 string/int 当成 bool 读取。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::Bool(v)) => Ok(*v),
            Some(_) => Err(format!("flag {name} is not a bool")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetString(&self, name: &str) -> Result<String, String> {
        // 返回 clone 是为了保持调用方 API 简单，不暴露内部存储借用。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::String(v)) => Ok(v.clone()),
            Some(_) => Err(format!("flag {name} is not a string")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetInt(&self, name: &str) -> Result<i32, String> {
        // 这里使用 i32 与 Go dumpling 当前线程/端口等字段类型保持一致。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::Int(v)) => Ok(*v),
            Some(_) => Err(format!("flag {name} is not an int")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetUint64(&self, name: &str) -> Result<u64, String> {
        // statement size、rows 等 flag 需要无符号整型以覆盖大值范围。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::Uint64(v)) => Ok(*v),
            Some(_) => Err(format!("flag {name} is not a uint64")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetStringSlice(&self, name: &str) -> Result<Vec<String>, String> {
        // 返回拷贝后，调用方可以安全追加而不破坏内部状态。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::StringSlice(v)) => Ok(v.clone()),
            Some(_) => Err(format!("flag {name} is not a string slice")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetDuration(&self, name: &str) -> Result<Duration, String> {
        // duration 已在解析阶段转换成标准库类型，后续无需再关心文本格式。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::Duration(v)) => Ok(*v),
            Some(_) => Err(format!("flag {name} is not a duration")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn GetStringToString(&self, name: &str) -> Result<HashMap<String, String>, String> {
        // map 型参数用于 session variables，直接克隆最直观。
        match self.flags.get(name).map(|f| &f.value) {
            Some(FlagValue::StringToString(v)) => Ok(v.clone()),
            Some(_) => Err(format!("flag {name} is not a string-to-string")),
            None => Err(format!("flag accessed but not defined: {name}")),
        }
    }

    pub fn NArg(&self) -> usize {
        // 与 Go pflag 一样，返回的是未被任何 flag 消费的剩余位置参数个数。
        self.args.len()
    }

    pub fn Args(&self) -> &[String] {
        // 返回切片引用即可，调用方只读这些剩余参数。
        &self.args
    }

    /// Parse argv (without program name), matching pflag ContinueOnError style.
    pub fn Parse(&mut self, argv: &[String]) -> Result<(), String> {
        // 解析前先清空上次遗留的 position args，允许同一个 FlagSet 被复用。
        self.args.clear();
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if a == "--" {
                // `--` 之后全部视为位置参数，完全停止 flag 解析。
                self.args.extend(argv[i + 1..].iter().cloned());
                break;
            }
            if let Some(rest) = a.strip_prefix("--") {
                // 长参数支持 `--name=value` 和 `--name value` 两种常见写法。
                let (name, inline) = match rest.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (rest, None),
                };
                i = self.set_from_parse(name, inline, argv, i)?;
                continue;
            }
            if a.starts_with('-') && a.len() >= 2 && !a.starts_with("--") {
                // pflag permits clustered booleans and attaches the remainder to the first
                // non-boolean shorthand (`-P4000`, `-o=dir`).
                let short_flags = &a[1..];
                let mut consumed_value = false;
                for (byte_index, c) in short_flags.char_indices() {
                    let name = self
                        .shorthand
                        .get(&c)
                        .cloned()
                        .ok_or_else(|| format!("unknown shorthand flag: -{c}"))?;
                    let remainder = &short_flags[byte_index + c.len_utf8()..];
                    match self.flags.get(&name).map(|f| f.value.clone()) {
                        Some(FlagValue::Bool(_)) => {
                            if let Some(value) = remainder.strip_prefix('=') {
                                self.apply_value(&name, FlagValue::Bool(parse_bool(value)?))?;
                                consumed_value = true;
                                break;
                            }
                            self.apply_value(&name, FlagValue::Bool(true))?;
                        }
                        Some(_) => {
                            let inline = if remainder.is_empty() {
                                None
                            } else {
                                Some(remainder.strip_prefix('=').unwrap_or(remainder).to_string())
                            };
                            i = self.set_from_parse(&name, inline, argv, i)?;
                            consumed_value = true;
                            break;
                        }
                        None => unreachable!("shorthand index must reference a defined flag"),
                    }
                }
                if !consumed_value {
                    i += 1;
                }
                continue;
            }
            // 无法识别为 flag 的项保留到 args，交由上层决定是否报错。
            self.args.push(a.clone());
            i += 1;
        }
        Ok(())
    }

    fn set_from_parse(
        &mut self,
        name: &str,
        inline: Option<String>,
        argv: &[String],
        i: usize,
    ) -> Result<usize, String> {
        // 先按注册时的值类型分派，保持解析行为与定义表同步。
        let kind = self
            .flags
            .get(name)
            .ok_or_else(|| format!("unknown flag: --{name}"))?
            .value
            .clone();
        match kind {
            FlagValue::Bool(_) => {
                // bool flag 允许省略显式值，省略时等价于 true。
                let v = match inline {
                    Some(s) => parse_bool(&s)?,
                    None => true,
                };
                self.apply_value(name, FlagValue::Bool(v))?;
                Ok(i + 1)
            }
            FlagValue::String(_) => {
                // 字符串参数不做额外语义校验，只负责搬运文本。
                let (v, next) = take_value(name, inline, argv, i)?;
                self.apply_value(name, FlagValue::String(v))?;
                Ok(next)
            }
            FlagValue::Int(_) => {
                // 数字解析失败时直接指出原始文本，便于用户定位。
                let (v, next) = take_value(name, inline, argv, i)?;
                let n: i32 = v
                    .parse()
                    .map_err(|_| format!("invalid argument {v:?} for --{name}"))?;
                self.apply_value(name, FlagValue::Int(n))?;
                Ok(next)
            }
            FlagValue::Uint64(_) => {
                // 无符号整型与有符号整型分开报错，可减少歧义。
                let (v, next) = take_value(name, inline, argv, i)?;
                let n: u64 = v
                    .parse()
                    .map_err(|_| format!("invalid argument {v:?} for --{name}"))?;
                self.apply_value(name, FlagValue::Uint64(n))?;
                Ok(next)
            }
            FlagValue::StringSlice(_) => {
                // pflag clears the registered default on the first assignment, then appends
                // values from later occurrences.
                let (v, next) = take_value(name, inline, argv, i)?;
                let parts = parse_csv_record(&v)?;
                // append semantics like pflag StringSlice
                // 先读取已有值再扩展，保证多次出现同一 flag 时不会覆盖前值。
                let mut cur = if self.Changed(name) {
                    self.GetStringSlice(name).unwrap_or_default()
                } else {
                    Vec::new()
                };
                cur.extend(parts);
                self.apply_value(name, FlagValue::StringSlice(cur))?;
                Ok(next)
            }
            FlagValue::Duration(_) => {
                // duration 统一在这里转成 `std::time::Duration`。
                // 这样上层配置结构里只会看到标准化后的时长值。
                let (v, next) = take_value(name, inline, argv, i)?;
                let d = parse_duration(&v)?;
                self.apply_value(name, FlagValue::Duration(d))?;
                Ok(next)
            }
            FlagValue::StringToString(_) => {
                let (v, next) = take_value(name, inline, argv, i)?;
                let mut parsed = HashMap::new();
                let pairs = if v.matches('=').count() == 1 {
                    vec![v.trim_matches('"').to_string()]
                } else {
                    parse_csv_record(&v)?
                };
                for pair in pairs {
                    let (key, value) = pair
                        .split_once('=')
                        .ok_or_else(|| format!("{pair} must be formatted as key=value"))?;
                    parsed.insert(key.to_string(), value.to_string());
                }
                let mut values = if self.Changed(name) {
                    self.GetStringToString(name).unwrap_or_default()
                } else {
                    HashMap::new()
                };
                values.extend(parsed);
                self.apply_value(name, FlagValue::StringToString(values))?;
                Ok(next)
            }
        }
    }

    fn apply_value(&mut self, name: &str, value: FlagValue) -> Result<(), String> {
        // changed 位在这里统一置 true，避免每个分支单独维护。
        let f = self
            .flags
            .get_mut(name)
            .ok_or_else(|| format!("unknown flag: --{name}"))?;
        f.value = value;
        f.changed = true;
        Ok(())
    }
}

fn take_value(
    name: &str,
    inline: Option<String>,
    argv: &[String],
    i: usize,
) -> Result<(String, usize), String> {
    if let Some(v) = inline {
        // `--name=value` 已经在同一 argv 项中携带了值。
        return Ok((v, i + 1));
    }
    let next = i + 1;
    if next >= argv.len() {
        // 对缺值错误直接引用长参数名，便于统一诊断文案。
        return Err(format!("flag needs an argument: --{name}"));
    }
    Ok((argv[next].clone(), next + 1))
}

fn parse_bool(s: &str) -> Result<bool, String> {
    // Match strconv.ParseBool exactly; notably, yes/no are not accepted.
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("invalid boolean value {s:?}")),
    }
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    // Match the positive subset of time.ParseDuration used by read-timeout, including
    // combined units such as 1h30m and the special unitless literal 0.
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let input = s.strip_prefix('+').unwrap_or(s);
    if input.is_empty() || input.starts_with('-') {
        return Err(format!("invalid duration {s:?}"));
    }
    let mut rest = input;
    let mut total_seconds = 0.0;
    while !rest.is_empty() {
        let mut number_end = 0;
        let mut saw_digit = false;
        let mut saw_dot = false;
        for (index, ch) in rest.char_indices() {
            if ch.is_ascii_digit() {
                saw_digit = true;
                number_end = index + ch.len_utf8();
            } else if ch == '.' && !saw_dot {
                saw_dot = true;
                number_end = index + 1;
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(format!("invalid duration {s:?}"));
        }
        let number: f64 = rest[..number_end]
            .parse()
            .map_err(|_| format!("invalid duration {s:?}"))?;
        rest = &rest[number_end..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3_600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))
        .ok_or_else(|| format!("invalid duration {s:?}"))?;
        total_seconds += number * multiplier;
        rest = &rest[unit.len()..];
    }
    if !total_seconds.is_finite() || total_seconds > u64::MAX as f64 {
        return Err(format!("invalid duration {s:?}"));
    }
    Ok(Duration::from_secs_f64(total_seconds))
}

fn parse_csv_record(input: &str) -> Result<Vec<String>, String> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = input.chars().peekable();
    loop {
        field.clear();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        field.push('"');
                    }
                    Some('"') => break,
                    Some(ch) => field.push(ch),
                    None => return Err(format!("unterminated quoted CSV field in {input:?}")),
                }
            }
            if chars.peek().is_some_and(|ch| *ch != ',') {
                return Err(format!("invalid quoted CSV field in {input:?}"));
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch == ',' {
                    break;
                }
                if ch == '"' {
                    return Err(format!("bare quote in CSV field in {input:?}"));
                }
                field.push(ch);
                chars.next();
            }
        }
        fields.push(field.clone());
        match chars.next() {
            Some(',') => {
                if chars.peek().is_none() {
                    fields.push(String::new());
                    break;
                }
            }
            None => break,
            Some(_) => unreachable!(),
        }
    }
    Ok(fields)
}

// --- prometheus collector stubs ---
// 下面这一段不尝试模拟真实采集器实现，只保留名称级占位与默认 gatherer 副作用。
// 真正的指标内容不在本任务范围内，这里只关心注册动作是否发生。

#[derive(Clone, Debug, Default)]
pub struct ProcessCollectorOpts {}

pub fn NewProcessCollector(_opts: ProcessCollectorOpts) -> &'static str {
    // 返回静态名称即可满足 registry 注册与测试断言需求。
    "process_collector"
}

pub fn NewGoCollector() -> &'static str {
    // Go collector 同理，只需要一个稳定标识符。
    "go_collector"
}

static DEFAULT_GATHERER: OnceLock<Mutex<Option<Arc<dyn Registry>>>> = OnceLock::new();

fn default_gatherer_slot() -> &'static Mutex<Option<Arc<dyn Registry>>> {
    // 使用 OnceLock + Mutex 模拟 Go 全局默认 gatherer 的可变单例。
    // Mutex 让测试并发访问时仍能保持状态一致性。
    DEFAULT_GATHERER.get_or_init(|| Mutex::new(None))
}

/// Go `prometheus.DefaultGatherer = gatherer` replacement.
pub fn set_default_gatherer(registry: Arc<dyn Registry>) {
    // 直接替换整个槽位，保持“最后一次设置生效”的简单语义。
    *default_gatherer_slot().lock().unwrap() = Some(registry);
}

pub fn take_default_gatherer() -> Option<Arc<dyn Registry>> {
    // 这里不消费值，只是克隆当前快照供测试检查。
    // 返回 Arc 副本后，调用方不会影响槽位里的原始对象。
    default_gatherer_slot().lock().unwrap().clone()
}

pub fn clear_default_gatherer() {
    // 显式清空是测试隔离的关键步骤。
    // 主流程本身不会主动清理，行为上更贴近 Go 全局变量模型。
    *default_gatherer_slot().lock().unwrap() = None;
}

/// Register process/go collectors onto the dumpling PromRegistry (name-level stub).
pub fn register_runtime_collectors(registry: &dyn Registry) {
    // 注册顺序与 Go 主流程保持一致，方便 parity test 比较副作用。
    // registry 具体如何存储 collector，由 export stub 自己决定。
    registry.MustRegister(NewProcessCollector(ProcessCollectorOpts {}));
    registry.MustRegister(NewGoCollector());
}

#[derive(Debug)]
// FlagError 目前主要作为兼容占位，供未来需要标准 error 类型时复用。
pub struct FlagError(pub String);

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 直接透传内部字符串，保持错误展示简单明确。
        // 这样无论它被包进哪层错误链，最终人类可读文本都不会变化。
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FlagError {}
// 实现标准 Error 只是为了与 Rust 生态通用接口兼容。
