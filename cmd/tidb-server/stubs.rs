// Copyright 2026 AsterSQL.
//!
//! 该文件集中提供 `cmd/tidb-server/main.rs` 依赖的本地桩实现，
//! 目标是让 Rust 入口能够在不拉起整套 TiDB 子系统的前提下，
//! 仍按 Go 主流程完成参数解析、全局配置同步、启动编排和清理测试。
//!
//! 这里的类型大多不是对真实包的完整移植，而是按调用面裁剪后的边界适配层。
//! 能记录事件的地方优先记录事件，能返回固定值的地方优先返回稳定值，
//! 这样测试可以断言“主流程有没有按顺序触达某个边界”，而不是验证底层实现细节。
//!
//! 因为本文件承担的是 `main.rs` 的胶水依赖，所以很多 API 仍保留 Go 风格命名。
//! 这有助于把 Go 代码逐段翻到 Rust 时，先对齐语义与调用序列，再逐步替换成真实实现。
//!
//! 阅读本文件时要特别注意：返回 `Ok(())`、写入全局状态、记录 event/log，
//! 往往只是在模拟“调用发生过”，并不代表对应的网络、存储、监控或安全能力已经落地。
//! Local stand-ins for server/session/store/config/signal/metrics boundaries (arm64-safe).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
// Error 是本文件统一使用的轻量错误对象。
// 它只保存字符串消息，目的是让不同 stub 能以统一方式回传失败原因。
// 这里不引入复杂错误栈，是因为主流程当前只关心是否 fatal 以及错误文本。
// 保留 `Error()` 方法是为了贴近 Go `error` 接口在迁移代码里的调用习惯。
// 这样 `must_nil`、`terror_log` 等辅助函数可以直接复用 Go 侧的写法。
pub struct Error {
    pub msg: String,
}
impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
    pub fn Error(&self) -> &str {
        &self.msg
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

// `must_nil` / `must_nil_result` / `fatal` 构成 Go `terror.MustNil` 的最小替身。
// 主流程里很多初始化步骤遵循“出错即终止”的约定，因此这里直接 panic。
// 这样测试既能观察失败分支，也能避免为每个 call site 重写错误传递结构。
// 这些辅助函数不负责恢复或包装错误，只负责把失败尽快提升到顶层。
// 因此它们表达的是启动阶段的硬失败语义，而不是通用库的错误处理策略。
pub fn must_nil(err: Option<Error>) {
    if let Some(e) = err {
        fatal(e.Error());
    }
}
pub fn must_nil_result<T>(r: Result<T>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => fatal(e.Error()),
    }
}
pub fn fatal(msg: impl AsRef<str>) -> ! {
    panic!("{}", msg.as_ref());
}
pub fn terror_log(err: Option<Error>) {
    if let Some(e) = err {
        log_warn(&e.msg);
    }
}

// EVENTS / LOGS 是这个文件里最重要的测试观测面。
// 许多 stub 并不执行真实副作用，而是把“某个边界被调用过”记录到全局数组。
// 测试随后通过 `take_events()`、`take_logs()` 验证调用顺序和关键分支是否触发。
// 这种做法让 `main.rs` 的编排逻辑能被验证，而不必接入真实日志器或外部系统。
// 二者都放在 `OnceLock<Mutex<...>>` 中，是为了兼顾进程级全局可见性与测试重置能力。
static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
fn events() -> &'static Mutex<Vec<String>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}
fn logs() -> &'static Mutex<Vec<String>> {
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
}
pub fn record_event(e: impl Into<String>) {
    events().lock().unwrap().push(e.into());
}
pub fn take_events() -> Vec<String> {
    std::mem::take(&mut *events().lock().unwrap())
}
pub fn clear_events() {
    events().lock().unwrap().clear();
}
pub fn take_logs() -> Vec<String> {
    std::mem::take(&mut *logs().lock().unwrap())
}
pub fn clear_logs() {
    logs().lock().unwrap().clear();
}
pub fn log_info(msg: &str) {
    logs().lock().unwrap().push(format!("INFO:{msg}"));
}
pub fn log_warn(msg: &str) {
    logs().lock().unwrap().push(format!("WARN:{msg}"));
}
pub fn log_error(msg: &str) {
    logs().lock().unwrap().push(format!("ERROR:{msg}"));
}

#[derive(Clone, Debug, Default)]
// AtomicBoolVar 是为配置对象里的 Go 风格可变布尔字段准备的包装。
// 真实 TiDB 配置结构里有很多原子布尔，本文件需要在 clone、默认值和全局同步之间共享它们。
// 直接包一层 `Arc<AtomicBool>`，可以让配置拷贝仍保留共享语义。
// 这比在每次读取时手工回写更贴近 Go 里指针共享的使用方式。
// 因此它主要解决的是“配置复制后值仍然可同步观察”的迁移问题。
pub struct AtomicBoolVar(Arc<AtomicBool>);
impl AtomicBoolVar {
    pub fn new(v: bool) -> Self {
        Self(Arc::new(AtomicBool::new(v)))
    }
    pub fn Store(&self, v: bool) {
        self.0.store(v, Ordering::SeqCst);
    }
    pub fn Load(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
#[derive(Clone, Debug, Default)]
// AtomicF64Var 处理的是标准库没有原生原子浮点的场景。
// 这里用 `Mutex<f64>` 包装并共享，是为了满足配置字段的并发可见性而非极致性能。
// 主流程只在启动阶段读写这类值，所以锁开销对这个 stub 可以接受。
// 它的职责是保持接口外观与 Go 版本一致，让调用点不必区分普通值和包装值。
// 因此该类型强调语义对齐，不强调 lock-free 实现。
pub struct AtomicF64Var(Arc<Mutex<f64>>);
impl AtomicF64Var {
    pub fn new(v: f64) -> Self {
        Self(Arc::new(Mutex::new(v)))
    }
    pub fn Store(&self, v: f64) {
        *self.0.lock().unwrap() = v;
    }
    pub fn Load(&self) -> f64 {
        *self.0.lock().unwrap()
    }
}

// flag 模块是 命令行 `FlagSet` 的最小兼容层。
// 它主要服务于 `initFlagSet`、`overrideConfig` 与参数解析测试。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod flag {
    use super::*;
    #[derive(Clone, Debug)]
    pub enum FlagValue {
        Bool(bool),
        String(String),
        Int(i32),
        Uint(u32),
    }
    #[derive(Clone, Debug)]
    pub struct Flag {
        pub Name: String,
        pub usage: String,
        pub value: FlagValue,
    }
    #[derive(Clone, Debug)]
    pub struct FlagSet {
        pub name: String,
        flags: HashMap<String, Flag>,
        visited: HashSet<String>,
        args: Vec<String>,
    }
    pub const ExitOnError: bool = true;
    pub fn NewFlagSet(name: &str, _exit: bool) -> FlagSet {
        FlagSet {
            name: name.to_string(),
            flags: HashMap::new(),
            visited: HashSet::new(),
            args: Vec::new(),
        }
    }
    impl FlagSet {
        pub fn Bool(&mut self, name: &str, default: bool, usage: &str) -> bool {
            self.flags.insert(
                name.to_string(),
                Flag {
                    Name: name.to_string(),
                    usage: usage.to_string(),
                    value: FlagValue::Bool(default),
                },
            );
            default
        }
        pub fn String(&mut self, name: &str, default: &str, usage: &str) -> String {
            self.flags.insert(
                name.to_string(),
                Flag {
                    Name: name.to_string(),
                    usage: usage.to_string(),
                    value: FlagValue::String(default.to_string()),
                },
            );
            default.to_string()
        }
        pub fn Int(&mut self, name: &str, default: i32, usage: &str) -> i32 {
            self.flags.insert(
                name.to_string(),
                Flag {
                    Name: name.to_string(),
                    usage: usage.to_string(),
                    value: FlagValue::Int(default),
                },
            );
            default
        }
        pub fn Uint(&mut self, name: &str, default: u32, usage: &str) -> u32 {
            self.flags.insert(
                name.to_string(),
                Flag {
                    Name: name.to_string(),
                    usage: usage.to_string(),
                    value: FlagValue::Uint(default),
                },
            );
            default
        }
        pub fn Parse(&mut self, args: &[String]) -> Result<()> {
            self.visited.clear();
            self.args.clear();
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                if a == "--" {
                    self.args.extend(args[i + 1..].iter().cloned());
                    break;
                }
                if !a.starts_with('-') {
                    self.args.extend(args[i..].iter().cloned());
                    break;
                }
                let raw = a.trim_start_matches('-');
                let (name, inline) = if let Some((n, v)) = raw.split_once('=') {
                    (n.to_string(), Some(v.to_string()))
                } else {
                    (raw.to_string(), None)
                };
                let Some(flag) = self.flags.get_mut(&name) else {
                    return Err(Error::new(format!(
                        "flag provided but not defined: -{name}"
                    )));
                };
                self.visited.insert(name.clone());
                match &mut flag.value {
                    FlagValue::Bool(v) => {
                        if let Some(inline) = inline {
                            *v = parse_bool(&inline)?;
                        } else if i + 1 < args.len() && looks_like_bool(&args[i + 1]) {
                            i += 1;
                            *v = parse_bool(&args[i])?;
                        } else {
                            *v = true;
                        }
                    }
                    FlagValue::String(v) => {
                        let val = match inline {
                            Some(x) => x,
                            None => {
                                i += 1;
                                if i >= args.len() {
                                    return Err(Error::new(format!(
                                        "flag needs an argument: -{name}"
                                    )));
                                }
                                args[i].clone()
                            }
                        };
                        *v = val;
                    }
                    FlagValue::Int(v) => {
                        let val = match inline {
                            Some(x) => x,
                            None => {
                                i += 1;
                                if i >= args.len() {
                                    return Err(Error::new(format!(
                                        "flag needs an argument: -{name}"
                                    )));
                                }
                                args[i].clone()
                            }
                        };
                        *v = val
                            .parse()
                            .map_err(|e: std::num::ParseIntError| Error::new(e.to_string()))?;
                    }
                    FlagValue::Uint(v) => {
                        let val = match inline {
                            Some(x) => x,
                            None => {
                                i += 1;
                                if i >= args.len() {
                                    return Err(Error::new(format!(
                                        "flag needs an argument: -{name}"
                                    )));
                                }
                                args[i].clone()
                            }
                        };
                        *v = val
                            .parse()
                            .map_err(|e: std::num::ParseIntError| Error::new(e.to_string()))?;
                    }
                }
                i += 1;
            }
            Ok(())
        }
        pub fn Args(&self) -> Vec<String> {
            self.args.clone()
        }
        pub fn Visit<F: FnMut(&Flag)>(&self, mut f: F) {
            let mut names: Vec<_> = self.visited.iter().cloned().collect();
            names.sort();
            for n in names {
                if let Some(flag) = self.flags.get(&n) {
                    f(flag);
                }
            }
        }
        pub fn LookupBool(&self, name: &str) -> bool {
            match self.flags.get(name).map(|f| &f.value) {
                Some(FlagValue::Bool(v)) => *v,
                _ => false,
            }
        }
        pub fn LookupString(&self, name: &str) -> String {
            match self.flags.get(name).map(|f| &f.value) {
                Some(FlagValue::String(v)) => v.clone(),
                _ => String::new(),
            }
        }
        pub fn LookupInt(&self, name: &str) -> i32 {
            match self.flags.get(name).map(|f| &f.value) {
                Some(FlagValue::Int(v)) => *v,
                _ => 0,
            }
        }
        pub fn LookupUint(&self, name: &str) -> u32 {
            match self.flags.get(name).map(|f| &f.value) {
                Some(FlagValue::Uint(v)) => *v,
                _ => 0,
            }
        }
        pub fn Usage(&self) {
            record_event("flag.Usage");
        }
        pub fn was_visited(&self, name: &str) -> bool {
            self.visited.contains(name)
        }
    }
    fn looks_like_bool(s: &str) -> bool {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "1" | "0" | "t" | "f" | "true" | "false"
        )
    }
    fn parse_bool(s: &str) -> Result<bool> {
        match s.to_ascii_lowercase().as_str() {
            "1" | "t" | "true" => Ok(true),
            "0" | "f" | "false" => Ok(false),
            _ => Err(Error::new(format!("invalid boolean value {s}"))),
        }
    }
}

// syscall 模块是 信号编号常量的轻量镜像。
// 它主要服务于 退出码计算与信号分支判断。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod syscall {
    pub const SIGINT: i32 = 2;
    pub const SIGTERM: i32 = 15;
    pub const SIGHUP: i32 = 1;
    pub const SIGQUIT: i32 = 3;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Signal 枚举把 Go `os.Signal` 在本文件中的有效取值收敛成可测试的 Rust 枚举。
// 主流程只关心少数退出相关信号，因此这里不追求完整平台枚举。
// 保留 `Other` 与 `None`，是为了让测试覆盖“未知信号”和“空信号”两类边界。
// 这样 `exitCodeForSignal` 与 signal handler 的行为可以在无真实内核信号参与时验证。
// 它表达的是主流程决策所需的最小信号域。
pub enum Signal {
    SIGINT,
    SIGTERM,
    SIGHUP,
    SIGQUIT,
    Other,
    None,
}

// signal 模块是 进程信号处理链路的可控桩。
// 它主要服务于 `main` 的注册、投递与退出等待路径。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod signal {
    use super::*;
    static HANDLER: OnceLock<Mutex<Option<Box<dyn FnMut(Signal) + Send>>>> = OnceLock::new();
    static EXIT_TX: OnceLock<Mutex<Option<Sender<()>>>> = OnceLock::new();
    static EXIT_RX: OnceLock<Mutex<Option<Receiver<()>>>> = OnceLock::new();
    fn handler_slot() -> &'static Mutex<Option<Box<dyn FnMut(Signal) + Send>>> {
        HANDLER.get_or_init(|| Mutex::new(None))
    }
    pub fn SetupUSR1Handler() {
        record_event("signal.SetupUSR1Handler");
        #[cfg(not(test))]
        astersql_util_signal::SetupUSR1Handler();
    }
    pub fn SetupSignalHandler<F>(f: F)
    where
        F: FnMut(Signal) + Send + 'static,
    {
        record_event("signal.SetupSignalHandler");
        let (tx, rx) = mpsc::channel();
        *EXIT_TX.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(tx);
        *EXIT_RX.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(rx);
        *handler_slot().lock().unwrap() = Some(Box::new(f));
        #[cfg(not(test))]
        astersql_util_signal::SetupSignalHandler(|signal| {
            let signal = match signal {
                2 => Signal::SIGINT,
                15 => Signal::SIGTERM,
                1 => Signal::SIGHUP,
                3 => Signal::SIGQUIT,
                _ => Signal::Other,
            };
            deliver(signal);
        });
    }
    pub fn deliver(sig: Signal) {
        if let Some(cb) = handler_slot().lock().unwrap().as_mut() {
            cb(sig);
        }
        if let Some(tx) = EXIT_TX
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().cloned())
        {
            let _ = tx.send(());
        }
    }
    pub fn wait_exited() {
        if let Some(rx) = EXIT_RX.get().and_then(|m| m.lock().unwrap().take()) {
            let _ = rx.recv();
        }
    }
    pub fn reset() {
        *handler_slot().lock().unwrap() = None;
        if let Some(m) = EXIT_TX.get() {
            *m.lock().unwrap() = None;
        }
        if let Some(m) = EXIT_RX.get() {
            *m.lock().unwrap() = None;
        }
    }
}

// kerneltype 模块是 内核类型判断与测试开关。
// 它主要服务于 classic / nextgen 分支选择。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod kerneltype {
    use super::*;
    static NEXTGEN: AtomicBool = AtomicBool::new(false);
    pub fn set_nextgen_for_test(v: bool) {
        NEXTGEN.store(v, Ordering::SeqCst);
    }
    pub fn IsNextGen() -> bool {
        NEXTGEN.load(Ordering::SeqCst)
    }
    pub fn IsClassic() -> bool {
        !IsNextGen()
    }
    pub fn Name() -> &'static str {
        if IsNextGen() { "nextgen" } else { "classic" }
    }
    pub fn IsMatch(pd: &str) -> bool {
        pd.eq_ignore_ascii_case(Name())
    }
}

// deploymode 模块是 部署模式枚举与全局选择器。
// 它主要服务于 starter / premium 相关启动分支。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod deploymode {
    use super::*;
    pub type Mode = u8;
    pub const Premium: Mode = 1;
    pub const PremiumReserved: Mode = 2;
    pub const Starter: Mode = 3;
    static CURRENT: AtomicU32 = AtomicU32::new(Premium as u32);
    pub fn Get() -> Mode {
        CURRENT.load(Ordering::SeqCst) as Mode
    }
    pub fn Set(mode: Mode) -> Result<()> {
        match mode {
            Premium | PremiumReserved | Starter => {
                CURRENT.store(mode as u32, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(Error::new("invalid deploy mode")),
        }
    }
    pub fn IsStarter() -> bool {
        kerneltype::IsNextGen() && Get() == Starter
    }
}

// config 模块是 启动配置树、全局配置仓库与派生值解析。
// 它主要服务于 配置初始化、全局变量同步和观测字段拼装。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod config {
    use super::*;
    pub const StoreTypeTiKV: &str = "tikv";
    pub const StoreTypeUniStore: &str = "unistore";
    pub const StoreTypeMockTiKV: &str = "mocktikv";
    pub const DefTempDir: &str = "/tmp/tidb";
    pub const DefSchemaLease: Duration = Duration::from_secs(45);
    pub const DefTxnTotalSizeLimit: u64 = 100 * 1024 * 1024;
    pub const SuperLargeTxnSize: u64 = 100u64 * 1024 * 1024 * 1024 * 1024;
    pub const MaxTxnEntrySizeLimit: u64 = 120 * 1024 * 1024;
    pub static CheckTableBeforeDrop: AtomicBool = AtomicBool::new(false);

    #[derive(Clone, Debug)]
    pub struct DeprecatedOption {
        pub SectionName: &'static str,
        pub NameMappings: Vec<&'static str>,
    }
    pub fn DeprecatedOptions() -> Vec<DeprecatedOption> {
        vec![
            DeprecatedOption {
                SectionName: "",
                NameMappings: vec![
                    "check-mb4-value-in-utf8",
                    "enable-collect-execution-info",
                    "max-server-connections",
                    "run-ddl",
                ],
            },
            DeprecatedOption {
                SectionName: "log",
                NameMappings: vec![
                    "enable-slow-log",
                    "slow-threshold",
                    "record-plan-in-slow-log",
                ],
            },
            DeprecatedOption {
                SectionName: "performance",
                NameMappings: vec!["force-priority"],
            },
            DeprecatedOption {
                SectionName: "plugin",
                NameMappings: vec!["load", "dir"],
            },
        ]
    }

    #[derive(Clone, Debug, Default)]
    pub struct FileLogConfig {
        pub Filename: String,
        pub MaxDays: i32,
    }
    #[derive(Clone, Debug)]
    pub struct Log {
        pub Level: String,
        pub File: FileLogConfig,
        pub SlowQueryFile: String,
        pub GeneralLogFile: String,
        pub EnableSlowLog: AtomicBoolVar,
        pub SlowThreshold: u64,
        pub RecordPlanInSlowLog: u32,
    }
    impl Default for Log {
        fn default() -> Self {
            Self {
                Level: "info".into(),
                File: FileLogConfig::default(),
                SlowQueryFile: String::new(),
                GeneralLogFile: String::new(),
                EnableSlowLog: AtomicBoolVar::new(true),
                SlowThreshold: 300,
                RecordPlanInSlowLog: 1,
            }
        }
    }
    impl Log {
        pub fn ToLogConfig(&self) -> SimpleLogConfig {
            SimpleLogConfig {
                level: self.Level.clone(),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct SimpleLogConfig {
        pub level: String,
    }

    #[derive(Clone, Debug)]
    pub struct Status {
        pub ReportStatus: bool,
        pub StatusHost: String,
        pub StatusPort: u32,
        pub MetricsAddr: String,
        pub MetricsInterval: u32,
    }
    impl Default for Status {
        fn default() -> Self {
            Self {
                ReportStatus: true,
                StatusHost: "0.0.0.0".into(),
                StatusPort: 10080,
                MetricsAddr: String::new(),
                MetricsInterval: 15,
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct ProxyProtocol {
        pub Networks: String,
        pub HeaderTimeout: u32,
        pub Fallbackable: bool,
    }
    #[derive(Clone, Debug)]
    pub struct Security {
        pub ClusterSSLCA: String,
        pub ClusterSSLCert: String,
        pub ClusterSSLKey: String,
        pub ClusterVerifyCN: Vec<String>,
        pub SSLCA: String,
        pub SSLCert: String,
        pub SSLKey: String,
        pub AutoTLS: bool,
        pub RSAKeySize: i32,
        pub SecureBootstrap: bool,
        pub DisconnectOnExpiredPassword: bool,
        pub SkipGrantTable: bool,
        pub EnableSEM: bool,
        pub SEMConfig: String,
    }
    impl Default for Security {
        fn default() -> Self {
            Self {
                ClusterSSLCA: String::new(),
                ClusterSSLCert: String::new(),
                ClusterSSLKey: String::new(),
                ClusterVerifyCN: Vec::new(),
                SSLCA: String::new(),
                SSLCert: String::new(),
                SSLKey: String::new(),
                AutoTLS: false,
                RSAKeySize: 4_096,
                SecureBootstrap: false,
                DisconnectOnExpiredPassword: true,
                SkipGrantTable: false,
                EnableSEM: false,
                SEMConfig: String::new(),
            }
        }
    }
    impl Security {
        pub fn ClusterSecurity(&self) -> ClusterSecurity {
            ClusterSecurity {
                ca: self.ClusterSSLCA.clone(),
                cert: self.ClusterSSLCert.clone(),
                key: self.ClusterSSLKey.clone(),
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct ClusterSecurity {
        pub ca: String,
        pub cert: String,
        pub key: String,
    }
    impl ClusterSecurity {
        pub fn ToTLSConfig(&self) -> Result<Option<TlsConfig>> {
            if self.ca.is_empty() && self.cert.is_empty() && self.key.is_empty() {
                return Ok(None);
            }
            Ok(Some(TlsConfig {
                ca: self.ca.clone(),
                cert: self.cert.clone(),
                key: self.key.clone(),
            }))
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct TlsConfig {
        pub ca: String,
        pub cert: String,
        pub key: String,
    }

    #[derive(Clone, Debug)]
    pub struct Instance {
        pub TiDBEnableDDL: AtomicBoolVar,
        pub PluginLoad: String,
        pub PluginDir: String,
        pub CheckMb4ValueInUTF8: AtomicBoolVar,
        pub EnableCollectExecutionInfo: AtomicBoolVar,
        pub MaxConnections: u32,
        pub EnableSlowLog: AtomicBoolVar,
        pub SlowThreshold: u64,
        pub RecordPlanInSlowLog: u32,
        pub ForcePriority: String,
        pub TiDBServiceScope: String,
        pub TiDBGeneralLog: bool,
        pub EnablePProfSQLCPU: bool,
        pub TiDBRCReadCheckTS: bool,
        pub DDLSlowOprThreshold: u32,
        pub ExpensiveQueryTimeThreshold: u64,
        pub ExpensiveTxnTimeThreshold: u64,
        pub StmtSummaryMaxStmtCount: u64,
        pub ServerMemoryLimit: String,
        pub MemArbitratorMode: String,
        pub MemArbitratorSoftLimit: String,
        pub ServerMemoryLimitGCTrigger: String,
        pub InstancePlanCacheMaxMemSize: String,
        pub StatsCacheMemQuota: u64,
        pub MemQuotaBindingCache: u64,
        pub SchemaCacheSize: String,
        pub MemoryUsageAlarmRatio: f64,
        pub StmtSummaryEnablePersistent: bool,
        pub StmtSummaryFilename: String,
        pub StmtSummaryFileMaxSize: i32,
        pub StmtSummaryFileMaxDays: i32,
        pub StmtSummaryFileMaxBackups: i32,
    }
    impl Default for Instance {
        fn default() -> Self {
            Self {
                TiDBEnableDDL: AtomicBoolVar::new(true),
                PluginLoad: String::new(),
                PluginDir: "/data/deploy/plugin".into(),
                CheckMb4ValueInUTF8: AtomicBoolVar::new(true),
                EnableCollectExecutionInfo: AtomicBoolVar::new(true),
                MaxConnections: 0,
                EnableSlowLog: AtomicBoolVar::new(true),
                SlowThreshold: 300,
                RecordPlanInSlowLog: 1,
                ForcePriority: "NO_PRIORITY".into(),
                TiDBServiceScope: String::new(),
                TiDBGeneralLog: false,
                EnablePProfSQLCPU: false,
                TiDBRCReadCheckTS: false,
                DDLSlowOprThreshold: 300,
                ExpensiveQueryTimeThreshold: 60,
                ExpensiveTxnTimeThreshold: 600,
                StmtSummaryMaxStmtCount: 0,
                ServerMemoryLimit: String::new(),
                MemArbitratorMode: String::new(),
                MemArbitratorSoftLimit: String::new(),
                ServerMemoryLimitGCTrigger: String::new(),
                InstancePlanCacheMaxMemSize: String::new(),
                StatsCacheMemQuota: 0,
                MemQuotaBindingCache: 0,
                SchemaCacheSize: String::new(),
                MemoryUsageAlarmRatio: 0.8,
                StmtSummaryEnablePersistent: false,
                StmtSummaryFilename: "tidb-statements.log".into(),
                StmtSummaryFileMaxSize: 0,
                StmtSummaryFileMaxDays: 0,
                StmtSummaryFileMaxBackups: 0,
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Performance {
        pub MaxProcs: u32,
        pub GOGC: i32,
        pub StatsLease: String,
        pub PlanReplayerGCLease: String,
        pub BindInfoLease: String,
        pub PseudoEstimateRatio: f64,
        pub CrossJoin: bool,
        pub TxnTotalSizeLimit: u64,
        pub TxnEntrySizeLimit: u64,
        pub ForcePriority: String,
        pub DistinctAggPushDown: bool,
        pub ProjectionPushDown: bool,
        pub EnforceMPP: bool,
        pub ServerMemoryQuota: u64,
    }
    impl Default for Performance {
        fn default() -> Self {
            Self {
                MaxProcs: 0,
                GOGC: 100,
                StatsLease: "3s".into(),
                PlanReplayerGCLease: "10m".into(),
                BindInfoLease: "3s".into(),
                PseudoEstimateRatio: 0.8,
                CrossJoin: true,
                TxnTotalSizeLimit: DefTxnTotalSizeLimit,
                TxnEntrySizeLimit: 6 * 1024 * 1024,
                ForcePriority: "NO_PRIORITY".into(),
                DistinctAggPushDown: false,
                ProjectionPushDown: false,
                EnforceMPP: false,
                ServerMemoryQuota: 0,
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct Plugin {
        pub Load: String,
        pub Dir: String,
    }
    #[derive(Clone, Debug)]
    pub struct IsolationRead {
        pub Engines: Vec<String>,
    }
    impl Default for IsolationRead {
        fn default() -> Self {
            Self {
                Engines: vec!["tikv".into(), "tiflash".into(), "tidb".into()],
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct Standby {
        pub StandByMode: bool,
        pub ActivationTimeout: u32,
        pub MaxIdleSeconds: u32,
    }
    #[derive(Clone, Debug, Default)]
    pub struct StarterParams {
        pub EnableManagerNotifier: bool,
        pub ManagerAddr: String,
    }
    #[derive(Clone, Debug)]
    pub struct TiKVClient {
        pub CommitTimeout: String,
        pub RegionCacheTTL: i64,
        pub StoreLivenessTimeout: String,
    }
    impl Default for TiKVClient {
        fn default() -> Self {
            Self {
                CommitTimeout: "41s".into(),
                RegionCacheTTL: 600,
                StoreLivenessTimeout: "5s".into(),
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct PessimisticTxn {
        pub ConstraintCheckInPlacePessimistic: bool,
        pub DeadlockHistoryCapacity: usize,
    }
    #[derive(Clone, Debug, Default)]
    pub struct TrxSummary {
        pub TransactionSummaryCapacity: usize,
        pub TransactionIDDigestMinDuration: i64,
    }
    #[derive(Clone, Debug, Default)]
    pub struct OpenTracing {
        pub Enable: bool,
    }
    impl OpenTracing {
        pub fn ToTracingConfig(&self) -> TracingConfig {
            TracingConfig {
                ServiceName: String::new(),
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct TracingConfig {
        pub ServiceName: String,
    }
    impl TracingConfig {
        pub fn NewTracer(&self) -> Result<((), ())> {
            Ok(((), ()))
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct ExternalWorkload {
        pub Enable: bool,
    }
    #[derive(Clone, Debug, Default)]
    pub struct KeyspaceObservabilityField {
        pub Source: String,
        pub MetricLabel: String,
        pub SlowLogField: String,
        pub StmtLogField: String,
        pub Required: bool,
    }
    #[derive(Clone, Debug, Default)]
    pub struct KeyspaceObservability {
        pub Fields: Vec<KeyspaceObservabilityField>,
    }
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KeyspaceObservabilityLogField {
        pub Name: String,
        pub Value: String,
    }
    #[derive(Clone, Debug, Default)]
    pub struct KeyspaceObservabilityValues {
        pub MetricLabels: HashMap<String, String>,
        pub SlowLogFields: Vec<KeyspaceObservabilityLogField>,
        pub StmtLogFields: HashMap<String, String>,
    }
    impl KeyspaceObservabilityValues {
        pub fn Clone(&self) -> Self {
            self.clone()
        }
    }

    #[derive(Clone, Debug)]
    pub struct Config {
        pub Host: String,
        pub AdvertiseAddress: String,
        pub Port: u32,
        pub Cors: String,
        pub Store: String,
        pub Path: String,
        pub Socket: String,
        pub Lease: String,
        pub TokenLimit: u32,
        pub RepairMode: bool,
        pub RepairTableList: Vec<String>,
        pub TempDir: String,
        pub TempStoragePath: String,
        pub TempStorageQuota: i64,
        pub DeployMode: deploymode::Mode,
        pub KeyspaceName: String,
        pub KeyspaceActivateMode: bool,
        pub TiDBEdition: String,
        pub TiDBReleaseVersion: String,
        pub ServerVersion: String,
        pub VersionComment: String,
        pub InitializeSQLFile: String,
        pub SplitTable: bool,
        pub RunDDL: bool,
        pub CheckMb4ValueInUTF8: AtomicBoolVar,
        pub EnableCollectExecutionInfo: bool,
        pub MaxServerConnections: u32,
        pub DisaggregatedTiFlash: bool,
        pub UseAutoScaler: bool,
        pub TiFlashComputeAutoScalerType: String,
        pub TiFlashComputeAutoScalerAddr: String,
        pub AutoScalerClusterID: String,
        pub IsTiFlashComputeFixedPool: bool,
        pub DeprecateIntegerDisplayWidth: bool,
        pub TiDBMaxReuseChunk: usize,
        pub TiDBMaxReuseColumn: usize,
        pub Log: Log,
        pub Status: Status,
        pub ProxyProtocol: ProxyProtocol,
        pub Security: Security,
        pub Instance: Instance,
        pub Performance: Performance,
        pub Plugin: Plugin,
        pub IsolationRead: IsolationRead,
        pub Standby: Standby,
        pub StarterParams: StarterParams,
        pub TiKVClient: TiKVClient,
        pub PessimisticTxn: PessimisticTxn,
        pub TrxSummary: TrxSummary,
        pub OpenTracing: OpenTracing,
        pub ExternalWorkload: ExternalWorkload,
        pub KeyspaceObservability: KeyspaceObservability,
        pub KeyspaceObservabilityValues: KeyspaceObservabilityValues,
    }
    impl Default for Config {
        fn default() -> Self {
            Self {
                Host: "0.0.0.0".into(),
                AdvertiseAddress: String::new(),
                Port: 4000,
                Cors: String::new(),
                Store: StoreTypeUniStore.into(),
                Path: "/tmp/tidb".into(),
                Socket: "/tmp/tidb-{Port}.sock".into(),
                Lease: "45s".into(),
                TokenLimit: 1000,
                RepairMode: false,
                RepairTableList: Vec::new(),
                TempDir: DefTempDir.into(),
                TempStoragePath: String::new(),
                TempStorageQuota: -1,
                DeployMode: deploymode::Premium,
                KeyspaceName: String::new(),
                KeyspaceActivateMode: false,
                TiDBEdition: String::new(),
                TiDBReleaseVersion: String::new(),
                ServerVersion: String::new(),
                VersionComment: String::new(),
                InitializeSQLFile: String::new(),
                SplitTable: true,
                RunDDL: true,
                CheckMb4ValueInUTF8: AtomicBoolVar::new(true),
                EnableCollectExecutionInfo: true,
                MaxServerConnections: 0,
                DisaggregatedTiFlash: false,
                UseAutoScaler: false,
                TiFlashComputeAutoScalerType: String::new(),
                TiFlashComputeAutoScalerAddr: String::new(),
                AutoScalerClusterID: String::new(),
                IsTiFlashComputeFixedPool: false,
                DeprecateIntegerDisplayWidth: false,
                TiDBMaxReuseChunk: 0,
                TiDBMaxReuseColumn: 0,
                Log: Log::default(),
                Status: Status::default(),
                ProxyProtocol: ProxyProtocol::default(),
                Security: Security::default(),
                Instance: Instance::default(),
                Performance: Performance::default(),
                Plugin: Plugin::default(),
                IsolationRead: IsolationRead::default(),
                Standby: Standby::default(),
                StarterParams: StarterParams::default(),
                TiKVClient: TiKVClient::default(),
                PessimisticTxn: PessimisticTxn::default(),
                TrxSummary: TrxSummary::default(),
                OpenTracing: OpenTracing::default(),
                ExternalWorkload: ExternalWorkload::default(),
                KeyspaceObservability: KeyspaceObservability::default(),
                KeyspaceObservabilityValues: KeyspaceObservabilityValues::default(),
            }
        }
    }
    impl Config {
        pub fn Valid(&self) -> Result<()> {
            Ok(())
        }
        pub fn UpdateTempStoragePath(&mut self) {
            if self.TempStoragePath.is_empty() {
                self.TempStoragePath = format!("{}/temp", self.TempDir);
            }
            record_event("config.UpdateTempStoragePath");
        }
        pub fn ResolveKeyspaceObservability(
            &mut self,
            metadata: &HashMap<String, String>,
        ) -> Result<()> {
            let mut values = KeyspaceObservabilityValues::default();
            for f in &self.KeyspaceObservability.Fields {
                let Some(v) = metadata.get(&f.Source) else {
                    if f.Required {
                        return Err(Error::new(format!(
                            "missing required keyspace observability source {}",
                            f.Source
                        )));
                    }
                    continue;
                };
                if !f.MetricLabel.is_empty() {
                    values.MetricLabels.insert(f.MetricLabel.clone(), v.clone());
                }
                if !f.SlowLogField.is_empty() {
                    values.SlowLogFields.push(KeyspaceObservabilityLogField {
                        Name: f.SlowLogField.clone(),
                        Value: v.clone(),
                    });
                }
                if !f.StmtLogField.is_empty() {
                    values
                        .StmtLogFields
                        .insert(f.StmtLogField.clone(), v.clone());
                }
            }
            self.KeyspaceObservabilityValues = values;
            Ok(())
        }
        pub fn GetKeyspaceObservabilityMetricLabels(&self) -> HashMap<String, String> {
            self.KeyspaceObservabilityValues.MetricLabels.clone()
        }
        pub fn GetKeyspaceObservabilitySlowLogFields(&self) -> Vec<KeyspaceObservabilityLogField> {
            self.KeyspaceObservabilityValues.SlowLogFields.clone()
        }
        pub fn GetKeyspaceObservabilityStmtLogFields(&self) -> HashMap<String, String> {
            self.KeyspaceObservabilityValues.StmtLogFields.clone()
        }
    }
    pub fn NewConfig() -> Config {
        Config::default()
    }
    pub fn StoreTypeList() -> Vec<&'static str> {
        vec![StoreTypeTiKV, StoreTypeUniStore, StoreTypeMockTiKV]
    }
    static GLOBAL: OnceLock<Mutex<Config>> = OnceLock::new();
    fn global() -> &'static Mutex<Config> {
        GLOBAL.get_or_init(|| Mutex::new(NewConfig()))
    }
    pub fn GetGlobalConfig() -> Config {
        global().lock().unwrap().clone()
    }
    pub fn set_global_config(cfg: Config) {
        *global().lock().unwrap() = cfg;
    }
    pub fn UpdateGlobal<F: FnOnce(&mut Config)>(f: F) {
        f(&mut global().lock().unwrap());
    }
    pub fn RestoreFunc() -> impl FnOnce() {
        let snap = GetGlobalConfig();
        move || {
            *global().lock().unwrap() = snap;
        }
    }
    pub fn InitializeConfig(
        config_path: &str,
        config_check: bool,
        config_strict: bool,
        override_fn: fn(&mut Config, &flag::FlagSet),
        fset: &flag::FlagSet,
    ) {
        record_event(format!(
            "config.InitializeConfig path={config_path} check={config_check} strict={config_strict}"
        ));
        let mut cfg = NewConfig();
        override_fn(&mut cfg, fset);
        if config_check {
            must_nil(cfg.Valid().err());
            record_event("config.config-check-ok");
        }
        *global().lock().unwrap() = cfg;
    }
}

// util 模块是 若干系统工具函数的占位实现。
// 它主要服务于 本机地址、GOGC 与内部 HTTP 客户端初始化。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod util {
    use super::*;
    pub fn GetLocalIP() -> String {
        "127.0.0.1".into()
    }
    pub fn SetGOGC(_gogc: i32) {
        record_event("util.SetGOGC");
    }
    pub fn InternalHTTPClient() {
        record_event("util.InternalHTTPClient");
    }
}
// naming 模块是 服务作用域命名规则校验。
// 它主要服务于 `overrideConfig` 中的 service scope 检查。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod naming {
    use super::*;
    pub fn Check(scope: &str) -> Result<()> {
        if scope.chars().any(|c| c.is_whitespace()) {
            return Err(Error::new("invalid service scope"));
        }
        Ok(())
    }
}

// mysql 模块是 版本号、优先级与 TiDBX 版本转换逻辑。
// 它主要服务于 版本初始化和系统变量同步。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod mysql {
    use super::*;
    pub const legacyTiDBReleaseVersionSentinel: &str =
        concat!("v8.4.0-this-is-a-", "place", "holder");
    pub const tidbXSentinelReleaseVersion: &str = concat!("v26.3.0-this-is-a-", "place", "holder");
    pub const mysqlCompatibilityVersion: &str = "8.0.11";
    pub const VersionSeparator: &str = "-TiDB-";
    static TIDB_RELEASE: OnceLock<Mutex<String>> = OnceLock::new();
    static SERVER_VERSION: OnceLock<Mutex<String>> = OnceLock::new();
    fn tidb_release() -> &'static Mutex<String> {
        TIDB_RELEASE.get_or_init(|| Mutex::new(legacyTiDBReleaseVersionSentinel.into()))
    }
    fn server_version() -> &'static Mutex<String> {
        SERVER_VERSION.get_or_init(|| {
            Mutex::new(format!(
                "{mysqlCompatibilityVersion}{VersionSeparator}{legacyTiDBReleaseVersionSentinel}"
            ))
        })
    }
    pub fn TiDBReleaseVersion() -> String {
        tidb_release().lock().unwrap().clone()
    }
    pub fn set_TiDBReleaseVersion(v: impl Into<String>) {
        *tidb_release().lock().unwrap() = v.into();
    }
    pub fn ServerVersion() -> String {
        server_version().lock().unwrap().clone()
    }
    pub fn set_ServerVersion(v: impl Into<String>) {
        *server_version().lock().unwrap() = v.into();
    }
    pub fn NormalizeTiDBReleaseVersionForNextGen(release_version: &str) -> String {
        if release_version == legacyTiDBReleaseVersionSentinel {
            tidbXSentinelReleaseVersion.into()
        } else {
            release_version.into()
        }
    }
    pub fn BuildTiDBXReleaseVersion(release_version: &str) -> Result<String> {
        let Some(raw) = release_version.strip_prefix('v') else {
            return Err(Error::new(format!(
                "invalid TiDB release version {release_version:?}, should start with 'v'"
            )));
        };
        let (num, pre) = match raw.split_once('-') {
            Some((n, p)) => (n, Some(p)),
            None => (raw, None),
        };
        let parts: Vec<_> = num.split('.').collect();
        if parts.len() != 3 {
            return Err(Error::new(format!(
                "invalid TiDB release version {release_version:?}, expect a semantic version"
            )));
        }
        let major: u64 = parts[0]
            .parse()
            .map_err(|_| Error::new(format!("invalid TiDB release version {release_version:?}")))?;
        let minor: u64 = parts[1]
            .parse()
            .map_err(|_| Error::new(format!("invalid TiDB release version {release_version:?}")))?;
        let patch: u64 = parts[2]
            .parse()
            .map_err(|_| Error::new(format!("invalid TiDB release version {release_version:?}")))?;
        let year = 2000 + major;
        if !(2020..=2099).contains(&year) || !(1..=12).contains(&minor) {
            return Err(Error::new(format!(
                "invalid TiDB release version {release_version:?}, the semantic version part should be in [2-digit-year].[month].[fix-version]-[xxx] format"
            )));
        }
        let mut out = format!("CLOUD.{year}{minor:02}.{patch}");
        if let Some(pre) = pre {
            out.push('-');
            out.push_str(pre);
        }
        Ok(out)
    }
    pub fn BuildTiDBXServerVersion(release_version: &str) -> Result<String> {
        let x = BuildTiDBXReleaseVersion(release_version)?;
        Ok(format!("{mysqlCompatibilityVersion}{VersionSeparator}{x}"))
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum PriorityEnum {
        NoPriority = 0,
        LowPriority = 1,
        HighPriority = 2,
        DelayedPriority = 3,
    }
    pub fn Str2Priority(val: &str) -> PriorityEnum {
        match val.to_ascii_uppercase().as_str() {
            "NO_PRIORITY" => PriorityEnum::NoPriority,
            "HIGH_PRIORITY" => PriorityEnum::HighPriority,
            "LOW_PRIORITY" => PriorityEnum::LowPriority,
            "DELAYED" => PriorityEnum::DelayedPriority,
            _ => PriorityEnum::NoPriority,
        }
    }
    pub fn Priority2Str(p: PriorityEnum) -> &'static str {
        match p {
            PriorityEnum::NoPriority => "NO_PRIORITY",
            PriorityEnum::LowPriority => "LOW_PRIORITY",
            PriorityEnum::HighPriority => "HIGH_PRIORITY",
            PriorityEnum::DelayedPriority => "DELAYED",
        }
    }
}

// versioninfo 模块是 TiDB edition 的全局保存点。
// 它主要服务于 版本信息打印与系统变量构造。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod versioninfo {
    use super::*;
    static EDITION: OnceLock<Mutex<String>> = OnceLock::new();
    pub fn TiDBEdition() -> String {
        EDITION
            .get_or_init(|| Mutex::new("Community".into()))
            .lock()
            .unwrap()
            .clone()
    }
    pub fn set_TiDBEdition(v: impl Into<String>) {
        *EDITION
            .get_or_init(|| Mutex::new("Community".into()))
            .lock()
            .unwrap() = v.into();
    }
}

// vardef 模块是 会被 `main.go` 写入的全局变量定义。
// 它主要服务于 `setGlobalVars` 的集中落点。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod vardef {
    use super::*;
    pub static EnableTmpStorageOnOOM: AtomicBool = AtomicBool::new(true);
    pub static ForcePriority: AtomicI32 = AtomicI32::new(0);
    pub static ProcessGeneralLog: AtomicBool = AtomicBool::new(false);
    pub static EnablePProfSQLCPU: AtomicBool = AtomicBool::new(false);
    pub static EnableRCReadCheckTS: AtomicBool = AtomicBool::new(false);
    pub static IsSandBoxModeEnabled: AtomicBool = AtomicBool::new(false);
    pub static DDLSlowOprThreshold: AtomicU32 = AtomicU32::new(300);
    pub static ExpensiveQueryTimeThreshold: AtomicU64 = AtomicU64::new(60);
    pub static ExpensiveTxnTimeThreshold: AtomicU64 = AtomicU64::new(600);
    pub static MemoryUsageAlarmRatio: OnceLock<Mutex<f64>> = OnceLock::new();
    pub static GlobalLogMaxDays: AtomicI32 = AtomicI32::new(0);
    static SERVICE_SCOPE: OnceLock<Mutex<String>> = OnceLock::new();
    static SCHEMA_LEASE: OnceLock<Mutex<Duration>> = OnceLock::new();
    static STATS_LEASE: OnceLock<Mutex<Duration>> = OnceLock::new();
    static PLAN_REPLAYER_LEASE: OnceLock<Mutex<Duration>> = OnceLock::new();

    pub const Version: &str = "version";
    pub const VersionComment: &str = "version_comment";
    pub const TiDBForcePriority: &str = "tidb_force_priority";
    pub const TiDBOptDistinctAggPushDown: &str = "tidb_opt_distinct_agg_push_down";
    pub const TiDBOptProjectionPushDown: &str = "tidb_opt_projection_push_down";
    pub const Port: &str = "port";
    pub const Socket: &str = "socket";
    pub const DataDir: &str = "datadir";
    pub const TiDBSlowQueryFile: &str = "tidb_slow_query_file";
    pub const TiDBIsolationReadEngines: &str = "tidb_isolation_read_engines";
    pub const TiDBEnforceMPPExecution: &str = "tidb_enforce_mpp";
    pub const TiDBConstraintCheckInPlacePessimistic: &str =
        "tidb_constraint_check_in_place_pessimistic";
    pub const Hostname: &str = "hostname";
    pub const TiDBEnablePrepPlanCache: &str = "tidb_enable_prepared_plan_cache";
    pub const TiDBStmtSummaryMaxStmtCount: &str = "tidb_stmt_summary_max_stmt_count";
    pub const TiDBServerMemoryLimit: &str = "tidb_server_memory_limit";
    pub const TiDBMemArbitratorMode: &str = "tidb_mem_arbitrator_mode";
    pub const TiDBMemArbitratorSoftLimit: &str = "tidb_mem_arbitrator_soft_limit";
    pub const TiDBServerMemoryLimitGCTrigger: &str = "tidb_server_memory_limit_gc_trigger";
    pub const TiDBInstancePlanCacheMaxMemSize: &str = "tidb_instance_plan_cache_max_size";
    pub const TiDBStatsCacheMemQuota: &str = "tidb_stats_cache_mem_quota";
    pub const TiDBMemQuotaBindingCache: &str = "tidb_mem_quota_binding_cache";
    pub const TiDBSchemaCacheSize: &str = "tidb_schema_cache_size";
    pub const TiDBMemQuotaQuery: &str = "tidb_mem_quota_query";

    pub fn EnableTmpStorageOnOOM_Load() -> bool {
        EnableTmpStorageOnOOM.load(Ordering::SeqCst)
    }
    pub fn SetSchemaLease(d: Duration) {
        *SCHEMA_LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(45)))
            .lock()
            .unwrap() = d;
    }
    pub fn SetStatsLease(d: Duration) {
        *STATS_LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(3)))
            .lock()
            .unwrap() = d;
    }
    pub fn SetPlanReplayerGCLease(d: Duration) {
        *PLAN_REPLAYER_LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(600)))
            .lock()
            .unwrap() = d;
    }
    pub fn schema_lease() -> Duration {
        *SCHEMA_LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(45)))
            .lock()
            .unwrap()
    }
    pub fn MemoryUsageAlarmRatio_Store(v: f64) {
        *MemoryUsageAlarmRatio
            .get_or_init(|| Mutex::new(0.8))
            .lock()
            .unwrap() = v;
    }
    pub fn ServiceScope_Store(v: String) {
        *SERVICE_SCOPE
            .get_or_init(|| Mutex::new(String::new()))
            .lock()
            .unwrap() = v;
    }
    pub fn ServiceScope_Load() -> String {
        SERVICE_SCOPE
            .get_or_init(|| Mutex::new(String::new()))
            .lock()
            .unwrap()
            .clone()
    }
}

// variable 模块是 系统变量表的最小注册表。
// 它主要服务于 从配置写回 `SysVar` 的流程与测试断言。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod variable {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct SysVar {
        pub Name: String,
        pub Value: String,
        pub IsInitedFromConfig: bool,
        pub instance_scope: bool,
    }
    static SYSVARS: OnceLock<Mutex<HashMap<String, SysVar>>> = OnceLock::new();
    fn map() -> &'static Mutex<HashMap<String, SysVar>> {
        SYSVARS.get_or_init(|| Mutex::new(default_sysvars()))
    }
    pub fn GetSysVar(name: &str) -> Option<SysVar> {
        map().lock().unwrap().get(name).cloned()
    }
    pub fn SetSysVar(name: &str, value: &str) {
        let mut m = map().lock().unwrap();
        m.entry(name.to_string())
            .and_modify(|s| s.Value = value.to_string())
            .or_insert(SysVar {
                Name: name.into(),
                Value: value.into(),
                IsInitedFromConfig: false,
                instance_scope: false,
            });
    }
    pub fn RegisterSysVar(sv: &SysVar) {
        let mut m = map().lock().unwrap();
        let mut s = sv.clone();
        s.instance_scope = true;
        m.insert(s.Name.clone(), s);
    }
    pub fn BoolToOnOff(v: bool) -> &'static str {
        if v { "ON" } else { "OFF" }
    }
    pub fn HasInstanceScope(name: &str) -> bool {
        map()
            .lock()
            .unwrap()
            .get(name)
            .map(|s| s.instance_scope)
            .unwrap_or(false)
    }
    fn default_sysvars() -> HashMap<String, SysVar> {
        let mut m = HashMap::new();
        for (n, v) in [
            (vardef::TiDBIsolationReadEngines, "tikv,tiflash,tidb"),
            (vardef::TiDBMemQuotaQuery, "1073741824"),
            (vardef::Version, "5.7.25-TiDB-None"),
            (vardef::VersionComment, "TiDB Server (Apache License 2.0)"),
            (vardef::TiDBForcePriority, "NO_PRIORITY"),
            (vardef::TiDBOptDistinctAggPushDown, "OFF"),
            (vardef::TiDBOptProjectionPushDown, "OFF"),
            (vardef::Port, "4000"),
            (vardef::Socket, ""),
            (vardef::DataDir, "/tmp/tidb"),
            (vardef::TiDBSlowQueryFile, ""),
            (vardef::TiDBEnforceMPPExecution, "OFF"),
            (vardef::TiDBConstraintCheckInPlacePessimistic, "OFF"),
            (vardef::Hostname, ""),
            (vardef::TiDBEnablePrepPlanCache, "OFF"),
            (vardef::TiDBStmtSummaryMaxStmtCount, "0"),
            (vardef::TiDBServerMemoryLimit, "0"),
            (vardef::TiDBMemArbitratorMode, ""),
            (vardef::TiDBMemArbitratorSoftLimit, ""),
            (vardef::TiDBServerMemoryLimitGCTrigger, ""),
            (vardef::TiDBInstancePlanCacheMaxMemSize, "0"),
            (vardef::TiDBStatsCacheMemQuota, "0"),
            (vardef::TiDBMemQuotaBindingCache, "0"),
            (vardef::TiDBSchemaCacheSize, "0"),
        ] {
            m.insert(
                n.into(),
                SysVar {
                    Name: n.into(),
                    Value: v.into(),
                    IsInitedFromConfig: false,
                    instance_scope: false,
                },
            );
        }
        m
    }
    pub fn reset_for_test() {
        *map().lock().unwrap() = default_sysvars();
    }
}

// redact 模块是 `collect-log` 子命令使用的脱敏入口桩。
// 它主要服务于 日志重写命令分支。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod redact {
    use super::*;
    pub fn DeRedactFile(redact: bool, input: &str, output: &str) -> Result<()> {
        record_event(format!(
            "redact.DeRedactFile redact={redact} in={input} out={output}"
        ));
        if !std::path::Path::new(input).exists() {
            // allow in-memory path markers used by tests
            if !input.starts_with("mem://") {
                return Err(Error::new(format!("input not found: {input}")));
            }
        }
        Ok(())
    }
}

// printer 模块是 版本与实例信息打印桩。
// 它主要服务于 `-V` 输出与启动信息打印。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod printer {
    use super::*;
    pub fn GetTiDBInfo() -> String {
        "TiDB stub info".into()
    }
    pub fn PrintTiDBInfo() {
        record_event("printer.PrintTiDBInfo");
        log_info("TiDB stub info");
    }
}

// logutil 模块是 后台日志器与 logger 初始化兼容层。
// 它主要服务于 日志初始化、告警和 fatal 路径。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod logutil {
    use super::*;
    pub struct BgLogger;
    impl BgLogger {
        pub fn Info(msg: &str) {
            log_info(msg);
        }
        pub fn Warn(msg: &str) {
            log_warn(msg);
        }
        pub fn Error(msg: &str) {
            log_error(msg);
        }
        pub fn Fatal(msg: &str) -> ! {
            fatal(msg);
        }
    }
    #[allow(non_snake_case)]
    pub fn bg_logger() -> BgLogger {
        BgLogger
    }
    pub fn InitLogger(_cfg: config::SimpleLogConfig, _wrap: ()) -> Result<()> {
        record_event("logutil.InitLogger");
        Ok(())
    }
    pub fn WrapZapcoreWithKeyspace() {}
}

// log 模块是 全局日志等级与同步行为桩。
// 它主要服务于 打印 TiDB 信息和退出前 `syncLog`。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod log {
    use super::*;
    static LEVEL: OnceLock<Mutex<String>> = OnceLock::new();
    pub fn Sync() -> Result<()> {
        record_event("log.Sync");
        Ok(())
    }
    pub fn SyncStdoutEINVAL() -> Result<()> {
        Err(Error::new("sync /dev/stdout: EINVAL"))
    }
    pub fn GetLevel() -> String {
        LEVEL
            .get_or_init(|| Mutex::new("info".into()))
            .lock()
            .unwrap()
            .clone()
    }
    pub fn SetLevel(l: &str) {
        *LEVEL
            .get_or_init(|| Mutex::new("info".into()))
            .lock()
            .unwrap() = l.into();
    }
    pub fn Info(msg: &str) {
        log_info(msg);
    }
    pub fn Warn(msg: &str) {
        log_warn(msg);
    }
    pub fn Error(msg: &str) {
        log_error(msg);
    }
    pub fn Fatal(msg: &str) -> ! {
        fatal(msg);
    }
}

// `parse_go_duration` 模拟 Go `time.ParseDuration` 在主流程里会用到的那部分语义。
// 它支持纳秒到小时的常见单位，并兼容调用方随后对裸秒数字符串的补救逻辑。
// 这里不尝试完整复刻 Go 的所有格式，只覆盖启动配置和测试涉及的输入。
// 对负值、空串和未知单位保持失败，是为了让配置校验分支与 Go 版一致。
// 因此它是“足够支撑主流程”的解析器，而不是通用 duration 库。
pub fn parse_go_duration(s: &str) -> Result<Duration> {
    // Supports Ns, us, ms, s, m, h and bare digits via caller retry.
    if s.is_empty() {
        return Err(Error::new("empty duration"));
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len()
        && (bytes[i].is_ascii_digit() || bytes[i] == b'.' || bytes[i] == b'-' || bytes[i] == b'+')
    {
        i += 1;
    }
    if i == 0 {
        return Err(Error::new(format!("invalid duration {s}")));
    }
    let num: f64 = s[..i]
        .parse()
        .map_err(|_| Error::new(format!("invalid duration {s}")))?;
    let unit = &s[i..];
    let secs = match unit {
        "ns" => num / 1e9,
        "us" | "µs" => num / 1e6,
        "ms" => num / 1e3,
        "s" | "" => num,
        "m" => num * 60.0,
        "h" => num * 3600.0,
        _ => return Err(Error::new(format!("invalid duration {s}"))),
    };
    if secs < 0.0 {
        return Err(Error::new(format!("invalid duration {s}")));
    }
    Ok(Duration::from_secs_f64(secs))
}

// keyspace 模块是 keyspace 名称读取与日志包装入口。
// 它主要服务于 starter 场景下的 keyspace 相关流程。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod keyspace {
    use super::*;
    pub fn GetKeyspaceNameBySettings() -> String {
        config::GetGlobalConfig().KeyspaceName
    }
    pub fn GetKeyspaceNameBytesBySettings() -> Vec<u8> {
        GetKeyspaceNameBySettings().into_bytes()
    }
    pub fn WrapZapcoreWithKeyspace() {}
}

// kv 模块是 存储抽象、PD 适配与关闭语义占位。
// 它主要服务于 建库、取 codec、检查用户 keyspace 与清理。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod kv {
    use super::*;
    pub static StandAloneTiDB: AtomicBool = AtomicBool::new(false);
    pub static TxnTotalSizeLimit: AtomicU64 = AtomicU64::new(config::DefTxnTotalSizeLimit);
    pub static TxnEntrySizeLimit: AtomicU64 = AtomicU64::new(6 * 1024 * 1024);
    #[derive(Clone, Debug)]
    pub struct KeyspaceMeta {
        pub name: String,
    }
    #[derive(Clone, Debug)]
    pub struct Codec {
        pub meta: Option<KeyspaceMeta>,
    }
    impl Codec {
        pub fn GetKeyspaceMeta(&self) -> Option<KeyspaceMeta> {
            self.meta.clone()
        }
    }
    #[derive(Clone)]
    pub struct Storage {
        pub path: String,
        pub keyspace: String,
        pub with_pd: bool,
        pub pd_kernel: String,
        pub user_ks: bool,
        closed: Arc<(Mutex<bool>, Condvar)>,
        registered: Option<astersql_store::StorageRef>,
    }
    impl std::fmt::Debug for Storage {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("Storage")
                .field("path", &self.path)
                .field("keyspace", &self.keyspace)
                .field("with_pd", &self.with_pd)
                .field("pd_kernel", &self.pd_kernel)
                .field("user_ks", &self.user_ks)
                .field("registered_identity", &self.registered_identity())
                .finish()
        }
    }
    impl Storage {
        pub fn new(path: &str, keyspace: &str) -> Self {
            Self {
                path: path.into(),
                keyspace: keyspace.into(),
                with_pd: false,
                pd_kernel: kerneltype::Name().into(),
                user_ks: false,
                closed: Arc::new((Mutex::new(false), Condvar::new())),
                registered: None,
            }
        }
        pub fn from_registered(
            path: &str,
            keyspace: &str,
            registered: astersql_store::StorageRef,
        ) -> Self {
            let with_pd = path.starts_with("tikv://");
            Self {
                path: path.into(),
                keyspace: keyspace.into(),
                with_pd,
                pd_kernel: kerneltype::Name().into(),
                user_ks: with_pd && !keyspace.is_empty() && keyspace != "SYSTEM",
                closed: Arc::new((Mutex::new(false), Condvar::new())),
                registered: Some(registered),
            }
        }
        pub fn registered_identity(&self) -> Option<usize> {
            self.registered
                .as_ref()
                .map(astersql_store::StorageIdentity)
        }
        pub fn CanonicalTiKVStore(&self) -> Result<astersql_store::TikvStore> {
            self.registered
                .as_ref()
                .ok_or_else(|| Error::new("storage is not backed by the canonical registry"))?
                .CanonicalTiKVStore()
                .map_err(|error| Error::new(error.to_string()))
        }
        pub fn GetClusterID(&self) -> Option<u64> {
            self.registered
                .as_ref()
                .and_then(|storage| storage.GetClusterID())
        }
        pub fn CurrentVersion(&self, txn_scope: &str) -> Result<Option<u64>> {
            self.registered
                .as_ref()
                .map_or(Ok(None), |storage| storage.CurrentVersion(txn_scope))
                .map_err(|error| Error::new(error.to_string()))
        }
        pub fn GetCodec(&self) -> Codec {
            Codec {
                meta: if self.keyspace.is_empty() {
                    None
                } else {
                    Some(KeyspaceMeta {
                        name: self.keyspace.clone(),
                    })
                },
            }
        }
        pub fn AsStorageWithPD(&self) -> Option<StorageWithPD> {
            if self.with_pd {
                Some(StorageWithPD {
                    kernel: self.pd_kernel.clone(),
                })
            } else {
                None
            }
        }
        pub fn Close(&self) -> Result<()> {
            record_event(format!(
                "kv.Storage.Close storage-id={:?}",
                self.registered_identity()
            ));
            let (lock, cv) = &*self.closed;
            *lock.lock().unwrap() = true;
            cv.notify_all();
            self.registered
                .as_ref()
                .map_or(Ok(()), |storage| storage.Close())
                .map_err(|error| Error::new(error.to_string()))
        }
    }
    #[derive(Clone, Debug)]
    pub struct StorageWithPD {
        pub kernel: String,
    }
    impl StorageWithPD {
        pub fn GetPDHTTPClient(&self) -> Option<PDHTTPClient> {
            Some(PDHTTPClient {
                kernel: self.kernel.clone(),
            })
        }
    }
    #[derive(Clone, Debug)]
    pub struct PDHTTPClient {
        pub kernel: String,
    }
    impl PDHTTPClient {
        pub fn GetStatus(&self, _ctx: ()) -> Result<PDStatus> {
            Ok(PDStatus {
                KernelType: self.kernel.clone(),
            })
        }
    }
    #[derive(Clone, Debug)]
    pub struct PDStatus {
        pub KernelType: String,
    }
    pub fn IsUserKS(storage: &Storage) -> bool {
        storage.user_ks
    }
}

// kvstore 模块是 store 注册表与默认存储工厂。
// 它主要服务于 `registerStores` 和 `MustInitStorage`。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod kvstore {
    use super::*;
    static REGISTERED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    static SYSTEM: OnceLock<Mutex<Option<kv::Storage>>> = OnceLock::new();
    fn registered() -> &'static Mutex<HashSet<String>> {
        REGISTERED.get_or_init(|| Mutex::new(HashSet::new()))
    }
    pub fn Register(store_type: &str, _driver: &str) -> Result<()> {
        record_event(format!("kvstore.Register {store_type}"));
        registered().lock().unwrap().insert(store_type.into());
        Ok(())
    }
    pub fn registered_types() -> Vec<String> {
        astersql_store::RegisteredStoreTypes()
    }
    pub fn MustInitStorage(keyspace: &str) -> kv::Storage {
        record_event(format!("kvstore.MustInitStorage ks={keyspace}"));
        let cfg = config::GetGlobalConfig();
        kv::Storage::new(&cfg.Path, keyspace)
    }
    pub fn GetSystemStorage() -> kv::Storage {
        SYSTEM
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| kv::Storage::new("system", ""))
    }
    pub fn reset() {
        registered().lock().unwrap().clear();
        astersql_store::ResetStoreStateForTest();
    }
}

// driver 模块是 TiKV 驱动名常量桩。
// 它主要服务于 store 注册时的 driver 标识。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod driver {
    pub const TiKVDriver: &str = "tikv-driver";
}
// mockstore 模块是 mocktikv / unistore 驱动名常量桩。
// 它主要服务于 测试型存储注册。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod mockstore {
    pub const MockTiKVDriver: &str = "mocktikv-driver";
    pub const EmbedUnistoreDriver: &str = "unistore-driver";
}

// domain 模块是 domain 生命周期与后台 handle 入口。
// 它主要服务于 bootstrap 后的 domain 建立与清理。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod domain {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct Domain {
        pub name: String,
        closed: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<String>>>,
    }
    impl Domain {
        pub fn new(name: &str) -> Self {
            Self {
                name: name.into(),
                closed: Arc::new(AtomicBool::new(false)),
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }
        pub fn Close(&self) {
            record_event("domain.Close");
            self.closed.store(true, Ordering::SeqCst);
        }
        pub fn StopAutoAnalyze(&self) {
            record_event("domain.StopAutoAnalyze");
        }
        pub fn ExpensiveQueryHandle(&self) -> Handle {
            Handle {
                kind: "expensive".into(),
            }
        }
        pub fn MemoryUsageAlarmHandle(&self) -> Handle {
            Handle {
                kind: "memalarm".into(),
            }
        }
        pub fn ServerMemoryLimitHandle(&self) -> Handle {
            Handle {
                kind: "memlimit".into(),
            }
        }
        pub fn InfoSyncer(&self) -> InfoSyncer {
            InfoSyncer
        }
    }
    #[derive(Clone, Debug)]
    pub struct Handle {
        pub kind: String,
    }
    impl Handle {
        pub fn SetSessionManager(self, _svr: &server::Server) -> Self {
            self
        }
        pub fn Run(self) {
            record_event(format!("domain.Handle.Run {}", self.kind));
        }
    }
    #[derive(Clone, Debug)]
    pub struct InfoSyncer;
    impl InfoSyncer {
        pub fn SetSessionManager(&self, _svr: &server::Server) {
            record_event("domain.InfoSyncer.SetSessionManager");
        }
    }
}

// server 模块是 TiDB server 对象及 standby 协作占位。
// 它主要服务于 建服、运行、关服、drain 与强制退出路径。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod server {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct StandbyController {
        pub metadata: HashMap<String, String>,
        pub activated: Arc<AtomicBool>,
    }
    impl StandbyController {
        pub fn WaitForActivate(&self) {
            record_event("standby.WaitForActivate");
            self.activated.store(true, Ordering::SeqCst);
        }
        pub fn EndStandby(&self, _err: Option<Error>) {
            record_event("standby.EndStandby");
        }
        pub fn PrepareForActivation(&self, _svr: &Server) -> Result<()> {
            record_event("standby.PrepareForActivation");
            Ok(())
        }
        pub fn OnServerCreated(&self, _svr: &Server) {
            record_event("standby.OnServerCreated");
        }
        pub fn ActivationMetadata(&self) -> HashMap<String, String> {
            self.metadata.clone()
        }
        pub fn AsLoadKeyspaceController(&self) -> Option<&Self> {
            Some(self)
        }
    }
    #[derive(Clone)]
    pub struct Server {
        pub force_shutdown: Arc<AtomicBool>,
        pub closed: Arc<(Mutex<bool>, Condvar)>,
        pub StandbyController: Option<StandbyController>,
        pub domain_set: Arc<AtomicBool>,
        canonical: Option<Arc<astersql_server::server::Server>>,
        canonical_domain: Option<Arc<astersql_server::runtime::CanonicalServerDomain>>,
    }
    impl std::fmt::Debug for Server {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("Server")
                .field(
                    "force_shutdown",
                    &self.force_shutdown.load(Ordering::SeqCst),
                )
                .field("domain_set", &self.domain_set.load(Ordering::SeqCst))
                .field("canonical", &self.canonical.is_some())
                .finish()
        }
    }
    impl Server {
        pub fn new() -> Self {
            Self {
                force_shutdown: Arc::new(AtomicBool::new(false)),
                closed: Arc::new((Mutex::new(false), Condvar::new())),
                StandbyController: None,
                domain_set: Arc::new(AtomicBool::new(false)),
                canonical: None,
                canonical_domain: None,
            }
        }
        pub fn from_canonical(
            canonical: Arc<astersql_server::server::Server>,
            canonical_domain: Arc<astersql_server::runtime::CanonicalServerDomain>,
        ) -> Self {
            record_event("canonical.server.created");
            Self {
                canonical: Some(canonical),
                canonical_domain: Some(canonical_domain),
                ..Self::new()
            }
        }
        pub fn SetDomain(&self, _dom: &domain::Domain) {
            record_event("server.SetDomain");
            self.domain_set.store(true, Ordering::SeqCst);
        }
        pub fn Close(&self) {
            record_event("server.Close");
            if let Some(server) = &self.canonical {
                record_event("canonical.server.stop-listeners");
                server.enter_shutdown_mode();
            }
            let (lock, cv) = &*self.closed;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        pub fn Run(&self, _dom: &domain::Domain) -> Result<()> {
            record_event("server.Run");
            if let (Some(server), Some(domain)) = (&self.canonical, &self.canonical_domain) {
                server
                    .run(domain.clone())
                    .map_err(|error| Error::new(format!("run canonical server: {error}")))?;
                record_event(format!(
                    "canonical.server.run mysql={:?} status={:?}",
                    server.listener_addr(),
                    server.status_listener_addr()
                ));
            }
            // Immediate-exit test mode: auto-close after registering.
            if std::env::var("ASTERSQL_TIDB_SERVER_IMMEDIATE_EXIT").is_ok() {
                self.Close();
                // Also deliver SIGTERM so main's signal handler path runs if registered later.
            }
            let (lock, cv) = &*self.closed;
            let mut g = lock.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
            Ok(())
        }
        pub fn GetForceShutdown(&self) -> bool {
            self.force_shutdown.load(Ordering::SeqCst)
        }
        pub fn set_force_shutdown(&self, v: bool) {
            self.force_shutdown.store(v, Ordering::SeqCst);
            if v {
                if let Some(server) = &self.canonical {
                    server.set_force_shutdown();
                }
            }
        }
        pub fn DrainClients(&self, drain: Duration, cancel: Duration) {
            record_event(format!(
                "server.DrainClients drain_ms={} cancel_ms={}",
                drain.as_millis(),
                cancel.as_millis()
            ));
            if let Some(server) = &self.canonical {
                server.drain_clients(drain, cancel);
                server.close();
                record_event("canonical.server.closed");
            }
            if let Some(domain) = &self.canonical_domain {
                domain.domain().close();
                record_event("canonical.domain.closed");
            }
        }
        pub fn KillSysProcesses(&self) {
            record_event("server.KillSysProcesses");
            if let Some(server) = &self.canonical {
                server.kill_system_processes();
            }
        }
        pub fn canonical_listener_addr(&self) -> Option<std::net::SocketAddr> {
            self.canonical
                .as_ref()
                .and_then(|server| server.listener_addr())
        }
        pub fn canonical_status_addr(&self) -> Option<std::net::SocketAddr> {
            self.canonical
                .as_ref()
                .and_then(|server| server.status_listener_addr())
        }
        pub fn uses_canonical_listener(&self) -> bool {
            self.canonical.is_some()
        }
    }
    pub fn NewTiDBDriver(_storage: &kv::Storage) -> String {
        "tidb-driver".into()
    }
    pub fn NewServer(_cfg: &config::Config, _driver: &str) -> Result<Server> {
        record_event("server.NewServer");
        Ok(Server::new())
    }
}

// standby 模块是 standby 控制器工厂。
// 它主要服务于 starter 待激活模式。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod standby {
    use super::*;
    pub fn NewLoadKeyspaceController(
        _cli: Option<tidbmanager::Client>,
    ) -> server::StandbyController {
        record_event("standby.NewLoadKeyspaceController");
        server::StandbyController {
            metadata: HashMap::new(),
            activated: Arc::new(AtomicBool::new(false)),
        }
    }
}

// tidbmanager 模块是 starter manager notifier 客户端桩。
// 它主要服务于 创建 manager client 并携带 Pod 身份。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod tidbmanager {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct Client {
        pub addr: String,
        pub pod_name: String,
        pub pod_ip: String,
        pub namespace: String,
    }
    pub fn NewClient(
        addr: &str,
        _tls: Option<config::TlsConfig>,
        pod_name: &str,
        pod_ip: &str,
        namespace: &str,
    ) -> Client {
        record_event(format!("tidbmanager.NewClient addr={addr} pod={pod_name}"));
        Client {
            addr: addr.into(),
            pod_name: pod_name.into(),
            pod_ip: pod_ip.into(),
            namespace: namespace.into(),
        }
    }
}

// session 模块是 session bootstrap 入口桩。
// 它主要服务于 创建 domain 前的 bootstrap 会话。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod session {
    use super::*;
    pub fn RegisterMockUpgradeFlag(_fset: &mut flag::FlagSet) {
        record_event("session.RegisterMockUpgradeFlag");
    }
    pub fn BootstrapSession(storage: &kv::Storage) -> Result<domain::Domain> {
        record_event(format!(
            "session.BootstrapSession path={} storage-id={:?}",
            storage.path,
            storage.registered_identity()
        ));
        Ok(domain::Domain::new("bootstrap"))
    }
}

// ddl 模块是 DDL owner manager 生命周期桩。
// 它主要服务于 owner manager 启停与 split table 标志。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod ddl {
    use super::*;
    pub static EnableSplitTableRegion: AtomicU32 = AtomicU32::new(0);
    pub fn StartOwnerManager(_ctx: (), _storage: &kv::Storage) -> Result<()> {
        record_event("ddl.StartOwnerManager");
        Ok(())
    }
    pub fn CloseOwnerManager(_storage: &kv::Storage) {
        record_event("ddl.CloseOwnerManager");
    }
}

// extworkload 模块是 外部 workload 管理器占位。
// 它主要服务于 starter 场景下的 keyspace workload 协调。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod extworkload {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct Manager {
        pub name: String,
    }
    impl Manager {
        pub fn Close(&self) -> Result<()> {
            record_event("extworkload.Manager.Close");
            Ok(())
        }
    }
    pub fn NewManager(
        _ctx: (),
        meta: kv::KeyspaceMeta,
        _cfg: config::ExternalWorkload,
    ) -> Result<Manager> {
        record_event(format!("extworkload.NewManager ks={}", meta.name));
        Ok(Manager { name: meta.name })
    }
}

// executor 模块是 执行器全局生命周期与资源 tracker 桩。
// 它主要服务于 启动/停止执行器及内存磁盘上限设置。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod executor {
    use super::*;
    pub fn Start() {
        record_event("executor.Start");
    }
    pub fn Stop() {
        record_event("executor.Stop");
    }
    pub struct Tracker {
        limit: Mutex<i64>,
    }
    impl Tracker {
        pub fn SetBytesLimit(&self, v: i64) {
            *self.limit.lock().unwrap() = v;
            record_event(format!("executor.Tracker.SetBytesLimit {v}"));
        }
        pub fn limit(&self) -> i64 {
            *self.limit.lock().unwrap()
        }
    }
    pub static GlobalDiskUsageTracker: OnceLock<Tracker> = OnceLock::new();
    pub static GlobalMemoryUsageTracker: OnceLock<Tracker> = OnceLock::new();
    pub fn disk_tracker() -> &'static Tracker {
        GlobalDiskUsageTracker.get_or_init(|| Tracker {
            limit: Mutex::new(0),
        })
    }
    pub fn memory_tracker() -> &'static Tracker {
        GlobalMemoryUsageTracker.get_or_init(|| Tracker {
            limit: Mutex::new(0),
        })
    }
}

// resourcemanager 模块是 实例级资源管理器桩。
// 它主要服务于 启动和退出时的资源管理生命周期。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod resourcemanager {
    use super::*;
    pub struct ResourceManager;
    impl ResourceManager {
        pub fn Start(&self) {
            record_event("resourcemanager.Start");
        }
        pub fn Stop(&self) {
            record_event("resourcemanager.Stop");
        }
    }
    pub static InstanceResourceManager: ResourceManager = ResourceManager;
}

// repository 模块是 workload repository 开关桩。
// 它主要服务于 server 运行前后仓库启停。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod repository {
    use super::*;
    pub fn SetupRepository(_dom: &domain::Domain) {
        record_event("repository.SetupRepository");
    }
    pub fn StopRepository() {
        record_event("repository.StopRepository");
    }
}

// topsql 模块是 TopSQL 采集启停入口。
// 它主要服务于 server 启动后 profiling 注册与退出清理。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod topsql {
    use super::*;
    pub fn SetupTopProfiling(_ks: Vec<u8>, _svr: &server::Server, _dom: &domain::Domain) {
        record_event("topsql.SetupTopProfiling");
    }
    pub fn Close() {
        record_event("topsql.Close");
    }
}

// plugin 模块是 插件关闭入口桩。
// 它主要服务于 清理阶段的 plugin shutdown。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod plugin {
    use super::*;
    pub fn Shutdown(_ctx: ()) {
        record_event("plugin.Shutdown");
    }
}

// disk 模块是 临时目录初始化与清理占位。
// 它主要服务于 OOM spill 目录准备和退出清理。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod disk {
    use super::*;
    pub fn InitializeTempDir() -> Result<()> {
        record_event("disk.InitializeTempDir");
        Ok(())
    }
    pub fn CleanUp() {
        record_event("disk.CleanUp");
    }
}

// memory 模块是 内存 hook 与总量探测桩。
// 它主要服务于 启动期内存钩子与配额计算。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod memory {
    use super::*;
    pub fn InitMemoryHook() -> Result<()> {
        record_event("memory.InitMemoryHook");
        Ok(())
    }
    pub fn MemTotal() -> Result<u64> {
        Ok(16 * 1024 * 1024 * 1024)
    }
}

// cpuprofile 模块是 CPU profiler 开关桩。
// 它主要服务于 启动与退出的 profiler 生命周期。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod cpuprofile {
    use super::*;
    pub fn StartCPUProfiler() -> Result<()> {
        record_event("cpuprofile.StartCPUProfiler");
        Ok(())
    }
    pub fn StopCPUProfiler() {
        record_event("cpuprofile.StopCPUProfiler");
    }
}

// cgmon 模块是 cgroup 监控启停桩。
// 它主要服务于 server 生命周期内的 cgroup monitor。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod cgmon {
    use super::*;
    pub fn StartCgroupMonitor() {
        record_event("cgmon.StartCgroupMonitor");
    }
    pub fn StopCgroupMonitor() {
        record_event("cgmon.StopCgroupMonitor");
    }
}

// metricsutil 模块是 metrics 注册入口桩。
// 它主要服务于 启动期指标注册。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod metricsutil {
    use super::*;
    pub fn RegisterMetrics() -> Result<()> {
        record_event("metricsutil.RegisterMetrics");
        Ok(())
    }
}

// metrics 模块是 少量计数器的占位实现。
// 它主要服务于 时间回拨监控回调。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod metrics {
    use super::*;
    pub struct Counter;
    impl Counter {
        pub fn Inc(&self) {
            record_event("metrics.TimeJumpBackCounter.Inc");
        }
    }
    pub static TimeJumpBackCounter: Counter = Counter;
}

// systimemon 模块是 系统时间监控入口桩。
// 它主要服务于 `setupMetrics` 中的时钟回拨监控。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod systimemon {
    use super::*;
    pub fn StartMonitor<F>(_now: fn() -> std::time::SystemTime, handler: F)
    where
        F: Fn() + Send + 'static,
    {
        record_event("systimemon.StartMonitor");
        let _ = handler;
    }
}

// extension 模块是 extension 子系统初始化与获取桩。
// 它主要服务于 启动期扩展装配。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod extension {
    use super::*;
    #[derive(Clone, Debug, Default)]
    pub struct Extensions;
    pub fn Setup() -> Result<()> {
        record_event("extension.Setup");
        Ok(())
    }
    pub fn GetExtensions() -> Result<Extensions> {
        Ok(Extensions)
    }
}

// stmtsummaryv2 模块是 持久化 statements summary 的最小接口。
// 它主要服务于 按实例配置启停 statements summary。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod stmtsummaryv2 {
    use super::*;
    #[derive(Clone, Debug)]
    pub struct Config {
        pub Filename: String,
        pub FileMaxSize: i32,
        pub FileMaxDays: i32,
        pub FileMaxBackups: i32,
    }
    pub fn Setup(_cfg: &Config) -> Result<()> {
        record_event("stmtsummaryv2.Setup");
        Ok(())
    }
    pub fn Close() {
        record_event("stmtsummaryv2.Close");
    }
}

// tiflashcompute 模块是 TiFlash compute 自动扩缩容拓扑拉取入口。
// 它主要服务于 Disaggregated TiFlash 场景初始化。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod tiflashcompute {
    use super::*;
    pub fn InitGlobalTopoFetcher(_t: &str, _addr: &str, _id: &str, _fixed: bool) -> Result<()> {
        record_event("tiflashcompute.InitGlobalTopoFetcher");
        Ok(())
    }
}

// failpoint 模块是 failpoint 查询与启用兼容层。
// 它主要服务于 测试 API 与 unistore 特殊分支。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod failpoint {
    use super::*;
    pub fn Status(_name: &str) -> Result<()> {
        Err(Error::new("not enabled"))
    }
    pub fn Enable(_name: &str, _val: &str) -> Result<()> {
        record_event("failpoint.Enable");
        Ok(())
    }
}

// tikv 模块是 client-go 全局开关与超时参数桩。
// 它主要服务于 failpoint、region cache TTL 与 store liveness 配置。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod tikv {
    use super::*;
    pub static FailpointsEnabled: AtomicBool = AtomicBool::new(false);
    pub static RegionCacheTTL: AtomicI32 = AtomicI32::new(0);
    static STORE_LIVENESS: OnceLock<Mutex<Duration>> = OnceLock::new();
    pub fn EnableFailpoints() {
        FailpointsEnabled.store(true, Ordering::SeqCst);
        record_event("tikv.EnableFailpoints");
    }
    pub fn SetRegionCacheTTLSec(v: i64) {
        RegionCacheTTL.store(v as i32, Ordering::SeqCst);
    }
    pub fn SetStoreLivenessTimeout(d: Duration) {
        *STORE_LIVENESS
            .get_or_init(|| Mutex::new(Duration::from_secs(5)))
            .lock()
            .unwrap() = d;
    }
    pub fn StoreShuttingDown(_v: i32) {
        record_event("tikv.StoreShuttingDown");
    }
}

// tikvrpc 模块是 请求来源默认值设置桩。
// 它主要服务于 启动初期设置 request origin。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod tikvrpc {
    use super::*;
    pub fn SetDefaultRequestOrigin(_origin: i32) {
        record_event("tikvrpc.SetDefaultRequestOrigin");
    }
}
// kvrpcpb 模块是 请求来源枚举常量占位。
// 它主要服务于 给 tikvrpc 提供 TiDB 请求来源值。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod kvrpcpb {
    pub const RequestOrigin_RequestOriginTiDB: i32 = 1;
}

// intest 模块是 内部检查开关常量桩。
// 它主要服务于 生产环境告警路径的测试替身。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod intest {
    pub static EnableInternalCheck: bool = false;
}

// linux 模块是 CPU 亲和性设置入口桩。
// 它主要服务于 `setCPUAffinity` 的最终调用点。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod linux {
    use super::*;
    pub fn SetAffinity(cpu: &[i32]) -> Result<()> {
        record_event(format!("linux.SetAffinity {:?}", cpu));
        Ok(())
    }
}

// storage_sys 模块是 目录容量探测桩。
// 它主要服务于 临时存储配额校验。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod storage_sys {
    use super::*;
    pub fn GetTargetDirectoryCapacity(_path: &str) -> Result<u64> {
        Ok(1024 * 1024 * 1024 * 100)
    }
}

// bindinfo 模块是 bind info lease 全局值。
// 它主要服务于 `setGlobalVars` 中的 lease 同步。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod bindinfo {
    use super::*;
    static LEASE: OnceLock<Mutex<Duration>> = OnceLock::new();
    pub fn set_Lease(d: Duration) {
        *LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(3)))
            .lock()
            .unwrap() = d;
    }
    pub fn Lease() -> Duration {
        *LEASE
            .get_or_init(|| Mutex::new(Duration::from_secs(3)))
            .lock()
            .unwrap()
    }
}

// statistics 模块是 统计信息相关全局参数桩。
// 它主要服务于 pseudo estimate ratio 同步。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod statistics {
    use super::*;
    pub static RatioOfPseudoEstimate: OnceLock<Mutex<f64>> = OnceLock::new();
    pub fn Ratio_Store(v: f64) {
        *RatioOfPseudoEstimate
            .get_or_init(|| Mutex::new(0.8))
            .lock()
            .unwrap() = v;
    }
}

// plannercore 模块是 planner 全局行为开关与缓存上限桩。
// 它主要服务于 cross join 与 prepared plan cache 配置。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod plannercore {
    use super::*;
    pub static AllowCartesianProduct: AtomicBool = AtomicBool::new(true);
    pub static PreparedPlanCacheMaxMemory: AtomicU64 = AtomicU64::new(0);
}

// privileges 模块是 权限系统全局布尔位桩。
// 它主要服务于 skip grant table 设置。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod privileges {
    use super::*;
    pub static SkipWithGrant: AtomicBool = AtomicBool::new(false);
}

// domainutil 模块是 repair mode 共享状态容器。
// 它主要服务于 repair-mode / repair-list 生效点。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod domainutil {
    use super::*;
    pub struct RepairInfoState {
        mode: AtomicBool,
        list: Mutex<Vec<String>>,
    }
    impl RepairInfoState {
        pub fn SetRepairMode(&self, v: bool) {
            self.mode.store(v, Ordering::SeqCst);
        }
        pub fn SetRepairTableList(&self, v: Vec<String>) {
            *self.list.lock().unwrap() = v;
        }
        pub fn mode(&self) -> bool {
            self.mode.load(Ordering::SeqCst)
        }
        pub fn list(&self) -> Vec<String> {
            self.list.lock().unwrap().clone()
        }
    }
    pub static RepairInfo: OnceLock<RepairInfoState> = OnceLock::new();
    pub fn repair_info() -> &'static RepairInfoState {
        RepairInfo.get_or_init(|| RepairInfoState {
            mode: AtomicBool::new(false),
            list: Mutex::new(Vec::new()),
        })
    }
}

// kvcache 模块是 KV cache 全局 tracker 挂接桩。
// 它主要服务于 把 LRU tracker 接到 memory tracker。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod kvcache {
    use super::*;
    pub struct LRUTracker;
    impl LRUTracker {
        pub fn AttachToGlobalTracker(&self, _t: &executor::Tracker) {
            record_event("kvcache.AttachToGlobalTracker");
        }
    }
    pub static GlobalLRUMemUsageTracker: LRUTracker = LRUTracker;
}

// transaction 模块是 事务退避上限全局变量桩。
// 它主要服务于 commit timeout 转换后的毫秒值。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod transaction {
    use super::*;
    pub static CommitMaxBackoff: AtomicU64 = AtomicU64::new(0);
}

// parsertypes 模块是 parser 类型层面的兼容开关。
// 它主要服务于 整数显示宽度弃用标志同步。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod parsertypes {
    use super::*;
    pub static TiDBStrictIntegerDisplayWidth: AtomicBool = AtomicBool::new(false);
}

// deadlockhistory 模块是 死锁历史容量控制桩。
// 它主要服务于 配置驱动的 `Resize`。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod deadlockhistory {
    use super::*;
    pub struct DeadlockHistory;
    impl DeadlockHistory {
        pub fn Resize(&self, n: usize) {
            record_event(format!("deadlockhistory.Resize {n}"));
        }
    }
    pub static GlobalDeadlockHistory: DeadlockHistory = DeadlockHistory;
}

// txninfo 模块是 事务摘要 recorder 桩。
// 它主要服务于 summary 容量与最小时长同步。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod txninfo {
    use super::*;
    pub struct RecorderState;
    impl RecorderState {
        pub fn ResizeSummaries(&self, n: usize) {
            record_event(format!("txninfo.ResizeSummaries {n}"));
        }
        pub fn SetMinDuration(&self, d: Duration) {
            record_event(format!("txninfo.SetMinDuration {:?}", d));
        }
    }
    pub static Recorder: RecorderState = RecorderState;
}

// chunk 模块是 chunk 复用参数初始化桩。
// 它主要服务于 chunk/column 复用上限设置。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod chunk {
    use super::*;
    pub fn InitChunkAllocSize(a: usize, b: usize) {
        record_event(format!("chunk.InitChunkAllocSize {a} {b}"));
    }
}

// copr 模块是 MPP failed store prober 生命周期桩。
// 它主要服务于 存储与 domain 启停时的后台探测器。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod copr {
    use super::*;
    pub struct Prober;
    impl Prober {
        pub fn Run(&self) {
            record_event("copr.GlobalMPPFailedStoreProber.Run");
        }
        pub fn Stop(&self) {
            record_event("copr.GlobalMPPFailedStoreProber.Stop");
        }
    }
    pub static GlobalMPPFailedStoreProber: Prober = Prober;
}

// mppcoordmanager 模块是 MPP coordinator manager 生命周期桩。
// 它主要服务于 MPP 协调器启动与停止。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod mppcoordmanager {
    use super::*;
    pub struct Manager;
    impl Manager {
        pub fn Run(&self) {
            record_event("mppcoordmanager.Run");
        }
        pub fn Stop(&self) {
            record_event("mppcoordmanager.Stop");
        }
    }
    pub static InstanceMPPCoordinatorManager: Manager = Manager;
}

// maxprocs 模块是 automaxprocs 调整入口桩。
// 它主要服务于 `setGlobalVars` 中的 `maxprocs.Set`。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod maxprocs {
    use super::*;
    pub fn Set(_logger: fn(&str)) -> Result<usize> {
        record_event("maxprocs.Set");
        Ok(1)
    }
}

// sem 模块是 传统 SEM 开关桩。
// 它主要服务于 未提供配置文件时直接启用 SEM。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod sem {
    use super::*;
    pub fn Enable() {
        record_event("sem.Enable");
    }
}
// semv2 模块是 带配置的 SEM v2 开关桩。
// 它主要服务于 带 `SEMConfig` 的安全增强模式。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod semv2 {
    use super::*;
    pub fn Enable(_cfg: &str) -> Result<()> {
        record_event("semv2.Enable");
        Ok(())
    }
}

// push 模块是 Prometheus Pushgateway 客户端占位。
// 它主要服务于 后台 metrics push 循环。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod push {
    use super::*;
    pub struct Pusher {
        pub addr: String,
        pub job: String,
    }
    impl Pusher {
        pub fn Gatherer(self, _g: ()) -> Self {
            self
        }
        pub fn Grouping(self, _k: &str, _v: &str) -> Self {
            self
        }
        pub fn Push(&self) -> Result<()> {
            record_event("push.Push");
            Ok(())
        }
    }
    pub fn New(addr: &str, job: &str) -> Pusher {
        Pusher {
            addr: addr.into(),
            job: job.into(),
        }
    }
}
// prometheus 模块是 默认 gatherer 占位函数。
// 它主要服务于 给 push client 组装 gatherer。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod prometheus {
    pub fn DefaultGatherer() {}
}

// opentracing 模块是 全局 tracer 注册入口桩。
// 它主要服务于 Jaeger/OpenTracing 初始化后挂全局。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod opentracing {
    use super::*;
    pub fn SetGlobalTracer(_t: ()) {
        record_event("opentracing.SetGlobalTracer");
    }
}

// pyroscope 模块是 pyroscope profiler 启动桩。
// 它主要服务于 按环境变量启用 profile 上传。
// 这里保留的状态和返回值只覆盖 `cmd/tidb-server/main.rs` 当前会触达的最小集合。
// 未被主流程和现有测试引用的真实能力，在这个桩里会有意省略或折叠为固定行为。
// 接口命名、字段形状和错误边界尽量贴近 Go 版本，方便迁移时按调用点逐步替换。
// 因此看到 no-op、固定常量或事件记录时，应理解为启动编排占位，而不是完整子系统实现。
pub mod pyroscope {
    use super::*;
    pub fn Start(_addr: &str) -> Result<()> {
        record_event("pyroscope.Start");
        Ok(())
    }
}

// `args_from_env` 让测试可以在不真正执行二进制的情况下复用 `std::env::args()` 语义。
// 当环境里没有 argv 时，它会补一个 `tidb-server` 作为程序名。
// 这与 Go `os.Args` 至少包含可执行名的惯例保持一致。
// 调用方因此不必为“空参数数组”单独写防御分支。
// 它解决的是测试环境与真实进程入口之间的小差异。
pub fn args_from_env() -> Vec<String> {
    let mut a: Vec<String> = std::env::args().collect();
    if a.is_empty() {
        a.push("tidb-server".into());
    }
    a
}

/// Serialize tests that mutate process-global stub/config/kernel state.
// `test_guard` 用单全局锁串行化会修改进程级状态的测试。
// 这个文件里大量 stub 依赖 `OnceLock`、静态原子值和全局配置，因此天然不是并行安全的。
// 把锁暴露成显式辅助函数，比在每个测试里各自造锁更容易统一约束。
// 返回 `MutexGuard` 而不是高层包装，方便测试按作用域自动释放。
// 它保护的不是性能，而是跨测试状态污染的可控性。
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// `reset_all_for_test` 把本文件里最常见的全局状态一次性恢复到启动前基线。
// 这包括事件日志、signal handler、store 注册表、deploy mode、全局配置和版本字段。
// 许多测试会多次执行主流程片段，因此需要在每轮之间消除残留副作用。
// 这里的重置顺序也尽量贴近主流程依赖关系，避免后续读取到半更新状态。
// 它相当于这个 stub 世界中的“轻量进程重启”。
pub fn reset_all_for_test() {
    clear_events();
    clear_logs();
    signal::reset();
    kvstore::reset();
    kerneltype::set_nextgen_for_test(false);
    let _ = deploymode::Set(deploymode::Premium);
    config::set_global_config(config::NewConfig());
    variable::reset_for_test();
    mysql::set_TiDBReleaseVersion(mysql::legacyTiDBReleaseVersionSentinel);
    mysql::set_ServerVersion(format!(
        "{}{}{}",
        mysql::mysqlCompatibilityVersion,
        mysql::VersionSeparator,
        mysql::legacyTiDBReleaseVersionSentinel
    ));
}
