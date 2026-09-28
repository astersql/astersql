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

//! Local arm64-safe stubs for config CLI, memory hook, GC percent, OS signals,
//! and injectable process exit (no kv/domain/kvproto/grpcio).
//! 中文补充：本文件给 `tidb-lightning` 入口补齐一组“可在本地安全运行”的外围依赖。
//! 中文补充：它不实现真正的导入逻辑，而是把命令行、进程退出、日志同步、信号等待、
//! 中文补充：内存钩子和少量全局配置装载压缩成可测试、可替换的边界。
//! 中文补充：因此 `main.rs` 可以继续沿着 Go 版本的控制流编排启动顺序，
//! 中文补充：同时避免在 arm64 开发环境里直接拉起原始的系统/网络依赖。
//! 中文补充：文件里的“stub”表示依赖范围被收窄，而不是语义可以随意简化。
//! 中文补充：凡是入口流程会观察到的帮助退出码、日志文件默认值、布尔参数解释、
//! 中文补充：server-mode 前置校验和 GOGC 分支，都应与 Go 版本保持同一判断意图。
//! 中文补充：注释重点会解释这些边界为什么存在、哪些字段真正被读取、
//! 中文补充：以及哪些地方故意只保留最小能力，避免维护者误以为这里是完整配置层。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_lightning_pkg_server as server;
use server::{Error, Result, config as server_config, log};

/// Go `flag.ErrHelp` sentinel — `Must` maps this to process exit 0.
/// 中文补充：这里把“请求显示帮助”编码成一个可识别的错误类别，
/// 中文补充：这样后续 `Must` 就能像 Go 一样把它翻译成成功退出而不是异常退出。
pub fn err_help() -> Error {
    Error {
        msg: "flag: help requested".into(),
        not_found: false,
        cause: None,
        class: Some("flag.ErrHelp"),
        empty_num: false,
    }
}

/// 中文补充：帮助错误既可能以 class 传递，也可能在外层包装后只剩文本，
/// 中文补充：因此判断逻辑同时兼容两种路径，减少入口层分支漂移。
pub fn is_err_help(err: &Error) -> bool {
    err.class == Some("flag.ErrHelp") || err.msg.contains("help requested")
}

/// Injectable `os.Exit` (Go `var exit = os.Exit`, overridden in tests).
/// 中文补充：Go 测试会重写包级 `exit` 变量；Rust 无法直接替换函数绑定，
/// 中文补充：于是这里改为“记录退出码 + 可选 hook”模式来保留可测试性。
static EXIT_CODE: AtomicI32 = AtomicI32::new(-1);
static EXIT_HOOK: OnceLock<Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>>> = OnceLock::new();

/// 中文补充：集中初始化一次 hook 槽，避免在测试并发读取时重复分配存储。
fn exit_hook_slot() -> &'static Mutex<Option<Arc<dyn Fn(i32) + Send + Sync>>> {
    EXIT_HOOK.get_or_init(|| Mutex::new(None))
}

/// Install a test exit hook. When set, [`exit`] records the code and returns
/// instead of terminating the process.
/// 中文补充：每次安装新 hook 时都会把上一次记录的退出码清空，
/// 中文补充：保证单测拿到的是本轮执行结果，而不是遗留状态。
pub fn set_exit_hook(hook: Option<Arc<dyn Fn(i32) + Send + Sync>>) {
    *exit_hook_slot().lock().unwrap() = hook;
    EXIT_CODE.store(-1, Ordering::SeqCst);
}

/// 中文补充：读取后即复位，模拟“进程只退出一次”的一次性观察语义。
pub fn take_exit_code() -> Option<i32> {
    let c = EXIT_CODE.swap(-1, Ordering::SeqCst);
    if c < 0 { None } else { Some(c) }
}

/// Go `exit` — process exit or test hook.
/// 中文补充：先无条件写入 `EXIT_CODE`，是为了即使 hook 内部 panic，
/// 中文补充：测试侧也能看到主流程原本意图退出的状态码。
pub fn exit(code: i32) {
    EXIT_CODE.store(code, Ordering::SeqCst);
    if let Some(hook) = exit_hook_slot().lock().unwrap().clone() {
        hook(code);
        return;
    }
    std::process::exit(code);
}

/// Logger sync boundary (Go `logger.Sync()`).
/// 中文补充：日志 flush 本身不是业务逻辑，但会影响 CLI 退出时是否向 stderr 报错，
/// 中文补充：因此这里单独抽成可注入结果，便于复现成功与失败两条收尾路径。
static SYNC_HOOK: OnceLock<Mutex<Option<Result<()>>>> = OnceLock::new();

/// 中文补充：与退出 hook 类似，这里保存的是“下一次 Sync 应返回什么”。
fn sync_slot() -> &'static Mutex<Option<Result<()>>> {
    SYNC_HOOK.get_or_init(|| Mutex::new(None))
}

/// 中文补充：测试通过该入口预置返回值，主流程本身不需要知道具体注入来源。
pub fn set_logger_sync_result(r: Option<Result<()>>) {
    *sync_slot().lock().unwrap() = r;
}

/// 中文补充：默认返回 `Ok(())`，表示注释桩不主动制造日志系统故障。
pub fn logger_sync() -> Result<()> {
    match sync_slot().lock().unwrap().clone() {
        Some(r) => r,
        None => Ok(()),
    }
}

/// Memory usage hook (Go `memory.InitMemoryHook`). Local stub — no OS probe.
/// 中文补充：真实 Lightning 会初始化内存观测相关能力；
/// 中文补充：这里仅保留“可能失败”的控制点，让入口日志和错误分支可被验证。
pub mod memory {
    use super::{Error, Result};
    static FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// 中文补充：失败开关只服务测试，不尝试模拟真实系统状态探测。
    pub fn set_fail(fail: bool) {
        FAIL.store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    /// 中文补充：返回值只区分成功/失败两态，足够覆盖 `main.rs` 中的告警路径。
    pub fn InitMemoryHook() -> Result<()> {
        if FAIL.load(std::sync::atomic::Ordering::SeqCst) {
            Err(Error::new("memory hook failed"))
        } else {
            Ok(())
        }
    }
}

/// GC percent (Go `debug.SetGCPercent`). Rust has no GOGC; retain branch semantics.
/// 中文补充：Rust 运行时没有 Go 那套 GC 百分比接口，
/// 中文补充：但入口仍会依据 backend 和 `GOGC` 是否设置来决定是否调用该边界。
/// 中文补充：因此这里保留“读环境变量、返回旧值、更新当前值”的最小语义，
/// 中文补充：让测试能够验证控制流而不是验证不存在的垃圾回收实现。
pub mod debug {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// 中文补充：默认值设为 100，对齐 Go 未调整时的常见 `GOGC` 基线。
    static CURRENT: AtomicI32 = AtomicI32::new(100);
    static GOGC_OVERRIDE: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();

    /// 中文补充：二层 `Option` 用来区分“读取真实环境”“显式视为未设置”“显式给定字符串”。
    fn gogc_slot() -> &'static Mutex<Option<Option<String>>> {
        GOGC_OVERRIDE.get_or_init(|| Mutex::new(None))
    }

    /// Test override for `GOGC` env. `Some(None)` => unset; `None` => read real env.
    /// 中文补充：这样测试不必污染进程级环境变量，也能稳定覆盖各种分支。
    pub fn set_gogc_override(v: Option<Option<String>>) {
        *gogc_slot().lock().unwrap() = v;
    }

    /// 中文补充：返回空字符串表示“按 Go 入口理解为未设置 `GOGC`”。
    pub fn gogc_env() -> String {
        if let Some(over) = gogc_slot().lock().unwrap().clone() {
            return over.unwrap_or_default();
        }
        std::env::var("GOGC").unwrap_or_default()
    }

    /// 中文补充：接口签名与 Go 对齐，调用方需要旧值来打印 debug 日志。
    pub fn SetGCPercent(percent: i32) -> i32 {
        CURRENT.swap(percent, Ordering::SeqCst)
    }

    /// 中文补充：暴露当前值仅用于测试断言，不属于生产入口必须依赖的 API。
    pub fn current_gc_percent() -> i32 {
        CURRENT.load(Ordering::SeqCst)
    }

    /// 中文补充：重置辅助确保不同测试案例之间不会互相污染 GC 配置状态。
    pub fn reset_gc_percent() {
        CURRENT.store(100, Ordering::SeqCst);
        set_gogc_override(None);
    }
}

/// OS signal wait boundary.
/// 中文补充：真实实现会阻塞等待 SIGHUP/SIGINT/SIGTERM/SIGQUIT，
/// 中文补充：这里把等待抽象成可注入字符串，避免测试真正依赖系统信号。
pub mod os_signal {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// 中文补充：只保存一个下一次可观测到的信号名，足以覆盖入口退出流程。
    static INJECT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    #[cfg(unix)]
    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    #[cfg(unix)]
    unsafe extern "C" fn record_signal(signal: i32) {
        RECEIVED.store(signal, Ordering::SeqCst);
    }

    #[cfg(unix)]
    unsafe extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }

    /// 中文补充：slot 封装避免模块外直接碰触共享状态。
    fn slot() -> &'static Mutex<Option<String>> {
        INJECT.get_or_init(|| Mutex::new(None))
    }

    /// Inject a signal name for tests; `None` restores blocking wait (prod).
    /// 中文补充：传入 `None` 时回到生产语义，表示后续调用会一直等待外部事件。
    pub fn inject(sig: Option<&str>) {
        *slot().lock().unwrap() = sig.map(str::to_string);
    }

    /// Wait for SIGHUP/SIGINT/SIGTERM/SIGQUIT (or injected test signal).
    /// 中文补充：Unix 生产路径会注册调用方列出的信号，并返回实际收到的名称。
    /// 中文补充：若测试预先注入了信号，则立即返回，便于同步断言停止逻辑。
    pub fn wait_for_one_of(names: &[&str]) -> String {
        if let Some(sig) = slot().lock().unwrap().clone() {
            return sig;
        }

        #[cfg(unix)]
        {
            RECEIVED.store(0, Ordering::SeqCst);
            for name in names {
                let number = match *name {
                    "SIGHUP" => 1,
                    "SIGINT" => 2,
                    "SIGQUIT" => 3,
                    "SIGTERM" => 15,
                    other => panic!("unsupported signal name: {other}"),
                };
                // POSIX `signal` installs a process-wide handler. The handler
                // only stores an integer, keeping the signal context minimal.
                let previous = unsafe { signal(number, record_signal as *const () as usize) };
                assert_ne!(previous, usize::MAX, "failed to register {name}");
            }
            loop {
                match RECEIVED.swap(0, Ordering::SeqCst) {
                    1 => return "SIGHUP".into(),
                    2 => return "SIGINT".into(),
                    3 => return "SIGQUIT".into(),
                    15 => return "SIGTERM".into(),
                    _ => std::thread::park_timeout(std::time::Duration::from_millis(10)),
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = names;
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::mem::forget(tx);
            let _ = rx.recv();
            unreachable!("signal wait channel has no sender")
        }
    }
}

/// Global config loading matching `pkg/lightning/config/global.go` algorithms.
/// 中文补充：该子模块负责把命令行和极小 TOML 子集装载到入口真正会读取的全局配置里。
/// 中文补充：它不是完整的 `pkg/lightning/config` Rust 移植，而是面向 `main.rs` 的窄接口。
/// 中文补充：因此只保留启动流程、日志、后端选择、checkpoint 和 TLS 路径相关字段。
pub mod config {
    use super::*;

    #[derive(Clone, Debug, Default)]
    pub struct GlobalLightning {
        pub Config: log::Config,
        /// Go `log.Config.Level` (server stub Config only carries File).
        /// 中文补充：日志级别暂存在这里，是因为 server stub 的 `log::Config`
        /// 中文补充：只承载文件路径，不覆盖入口侧对日志级别的解析职责。
        pub Level: String,
        /// 中文补充：状态地址同时控制 HTTP 状态服务和进度展示是否启用。
        pub StatusAddr: String,
        /// 中文补充：该开关决定主流程走常驻 server 还是一次性导入模式。
        pub ServerMode: bool,
        /// 中文补充：保持与 Go 配置同名，方便继续沿用 `LoadFromGlobal` 语义。
        pub CheckRequirements: bool,
        /// 中文补充：这里只保留从旧参数推导状态地址所需的端口字段。
        pub PProfPort: i32,
    }

    impl GlobalLightning {
        /// Go embeds `log.Config`, so `App.File` == `App.Config.File`.
        /// 中文补充：提供方法而非公开重复字段，可避免双份状态不一致。
        pub fn File(&self) -> &str {
            &self.Config.File
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalTiDB {
        /// 中文补充：这些字段覆盖入口向下游配置传播时真正会读到的 TiDB 连接参数。
        pub Host: String,
        pub Port: i32,
        pub User: String,
        pub Psw: String,
        pub StatusPort: i32,
        pub PdAddr: String,
        pub LogLevel: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalMydumper {
        /// 中文补充：数据目录和过滤规则会直接影响一次性导入模式的输入集合。
        pub SourceDir: String,
        pub NoSchema: bool,
        pub Filter: Vec<String>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalImporter {
        /// 中文补充：这里只保留入口会判断的 backend 与本地排序目录。
        pub Backend: String,
        pub SortedKVDir: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalCheckpoint {
        /// 中文补充：checkpoint 开关会影响是否启用恢复点逻辑，因此必须保留。
        pub Enable: bool,
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalPostRestore {
        /// 中文补充：后置校验与分析策略虽然不在本文件执行，
        /// 中文补充：但会被传给后续真正的运行配置，因此保持字段透传。
        pub Checksum: String,
        pub Analyze: String,
    }

    #[derive(Clone, Debug, Default)]
    pub struct Security {
        /// 中文补充：TLS 路径和日志脱敏是入口最常用的安全相关全局参数。
        pub CAPath: String,
        pub CertPath: String,
        pub KeyPath: String,
        pub RedactInfoLog: bool,
    }

    #[derive(Clone, Debug, Default)]
    pub struct GlobalConfig {
        /// 中文补充：结构布局基本沿 Go 侧大类组织，方便调用方按同名字段迁移。
        pub App: GlobalLightning,
        pub Checkpoint: GlobalCheckpoint,
        pub TiDB: GlobalTiDB,
        pub Mydumper: GlobalMydumper,
        pub TikvImporter: GlobalImporter,
        pub PostRestore: GlobalPostRestore,
        pub Security: Security,
        pub ConfigFileContent: Vec<u8>,
    }

    /// 中文补充：这些常量限定当前 stub 认可的 backend 名称，
    /// 中文补充：既用于参数校验，也用于与 `main.rs` 分支保持一致。
    pub const BackendLocal: &str = "local";
    pub const BackendTiDB: &str = "tidb";
    pub const BackendImportInto: &str = "import-into";

    /// 中文补充：默认值要尽量贴近 Go，原因是很多调用点依赖“未显式传参时”的行为。
    pub fn NewGlobalConfig() -> GlobalConfig {
        GlobalConfig {
            App: GlobalLightning {
                // 中文补充：默认不是 server 模式，但默认仍进行环境要求检查。
                ServerMode: false,
                CheckRequirements: true,
                ..Default::default()
            },
            // 中文补充：checkpoint 默认开启，符合 Lightning 的恢复点常态行为。
            Checkpoint: GlobalCheckpoint { Enable: true },
            TiDB: GlobalTiDB {
                // 中文补充：这里保留 Go 的常见本地默认连接参数。
                Host: "127.0.0.1".into(),
                User: "root".into(),
                StatusPort: 10080,
                LogLevel: "error".into(),
                ..Default::default()
            },
            Mydumper: GlobalMydumper {
                // 中文补充：过滤器默认排除系统库，避免导入无关元数据。
                Filter: default_filter(),
                ..Default::default()
            },
            TikvImporter: GlobalImporter {
                // 中文补充：backend 留空，表示由显式配置决定具体导入模式。
                Backend: String::new(),
                ..Default::default()
            },
            PostRestore: GlobalPostRestore {
                // 中文补充：后置 checksum/analyze 默认值沿用 Go 的推荐策略。
                Checksum: "required".into(),
                Analyze: "optional".into(),
            },
            Security: Security::default(),
            ConfigFileContent: Vec::new(),
        }
    }

    /// 中文补充：该默认过滤列表直接复用 Lightning 常见排除系统 schema 的约定。
    fn default_filter() -> Vec<String> {
        vec![
            "*.*".into(),
            "!mysql.*".into(),
            "!sys.*".into(),
            "!INFORMATION_SCHEMA.*".into(),
            "!PERFORMANCE_SCHEMA.*".into(),
            "!METRICS_SCHEMA.*".into(),
            "!INSPECTION_SCHEMA.*".into(),
        ]
    }

    /// 中文补充：未指定日志文件时，入口会生成一个临时路径，
    /// 中文补充：这样既能复现 Go 的“默认落盘”行为，也方便测试断言路径前缀。
    fn timestamp_log_file_name() -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!("lightning.log.{secs}"));
        p.display().to_string()
    }

    /// Convert cmd global config into server stub `GlobalConfig` for `server::New`.
    /// 中文补充：这里只投影 `server::New` 真正需要的那部分字段，
    /// 中文补充：避免把命令行层所有配置都强行复制进 server stub。
    pub fn to_server_global(g: &GlobalConfig) -> server_config::GlobalConfig {
        server_config::GlobalConfig {
            App: server_config::LightningApp {
                Config: g.App.Config.clone(),
                StatusAddr: g.App.StatusAddr.clone(),
                RegionConcurrency: 0,
                TableConcurrency: 0,
                MetaSchemaName: String::new(),
                CheckRequirements: g.App.CheckRequirements,
                TaskInfoSchemaName: String::new(),
            },
            Security: server_config::Security {
                CAPath: g.Security.CAPath.clone(),
                CertPath: g.Security.CertPath.clone(),
                KeyPath: g.Security.KeyPath.clone(),
                RedactInfoLog: g.Security.RedactInfoLog,
                ..Default::default()
            },
            TiDB: server_config::GlobalTiDB {
                LogLevel: g.TiDB.LogLevel.clone(),
            },
        }
    }

    /// Go `config.Must` — exit 0 on help, exit 2 on other errors.
    /// 中文补充：该函数模拟 Go 里“把 `(cfg, err)` 对折叠成一个必得配置”的入口助手。
    /// 中文补充：帮助请求属于正常控制流，因此退出码为 0；其他解析失败则退出 2。
    /// 中文补充：测试模式下 `exit` 会返回，所以仍需给调用方一个可继续传递的配置对象。
    pub fn Must(cfg: Option<GlobalConfig>, err: Option<Error>) -> GlobalConfig {
        match err {
            None => cfg.expect("Must requires cfg when err is nil"),
            Some(e) if is_err_help(&e) => {
                exit(0);
                // test hook returns; provide empty cfg so callers can continue
                cfg.unwrap_or_else(NewGlobalConfig)
            }
            Some(e) => {
                println!("{e}");
                exit(2);
                cfg.unwrap_or_else(NewGlobalConfig)
            }
        }
    }

    /// Minimal flag parse matching Go `LoadGlobalConfig` control flow.
    /// 中文补充：这是本文件最核心的配置装载逻辑，按 Go 主流程顺序处理帮助、
    /// 中文补充：版本输出、flag 解析、配置文件加载、命令行覆盖和最终一致性校验。
    /// 中文补充：它只实现当前入口需要的最小参数集合，不承诺兼容全部 Lightning CLI 语法。
    pub fn LoadGlobalConfig(
        args: &[String],
        _extra_flags: Option<()>,
    ) -> (Option<GlobalConfig>, Option<Error>) {
        let mut cfg = NewGlobalConfig();
        let mut map: HashMap<String, String> = HashMap::new();
        let mut filter: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            // 中文补充：帮助和版本打印都借用 `err_help` 终止后续流程，
            // 中文补充：从而复用 `Must` 中统一的退出码处理。
            if a == "-h" || a == "--help" {
                return (None, Some(err_help()));
            }
            if a == "-V" || a == "--V" {
                println!("{}", build_info());
                return (None, Some(err_help()));
            }
            let (key, val, consumed) = match parse_flag(args, i) {
                Ok(parsed) => parsed,
                Err(err) => return (None, Some(err)),
            };
            // 中文补充：`-f` 在 Go 中可重复出现，因此单独追加到过滤数组而不是放入 map。
            if let Some(k) = key {
                if k == "f" {
                    if let Some(v) = val {
                        filter.push(v);
                    }
                } else if k == "c" {
                    if let Some(v) = val {
                        // Both aliases write the same Go variable, so the last
                        // occurrence wins regardless of which spelling it uses.
                        map.insert("config".into(), v);
                    }
                } else if k == "V" {
                    if val.as_deref() == Some("true") {
                        println!("{}", build_info());
                        return (None, Some(err_help()));
                    }
                } else if let Some(v) = val {
                    map.insert(k, v);
                }
                i += consumed;
            } else {
                // Go's flag package stops at the first positional argument and
                // leaves the remainder in FlagSet.Args(). Lightning ignores it.
                break;
            }
        }

        // 中文补充：配置文件先加载，再由命令行覆盖，保持 Go 常见优先级。
        if let Some(path) = map.get("config").or_else(|| map.get("c")) {
            match std::fs::read(path) {
                Ok(data) => {
                    apply_toml_lite(&mut cfg, &data);
                    cfg.ConfigFileContent = data;
                }
                Err(e) => {
                    return (
                        None,
                        Some(Error::new(format!("read config file {path}: {e}"))),
                    );
                }
            }
        }

        // 中文补充：日志文件若未显式提供，则延迟生成带时间戳的临时路径。
        if let Some(v) = map.get("L").filter(|v| !v.is_empty()) {
            cfg.App.Level = v.clone();
        }
        if let Some(v) = map.get("log-file").filter(|v| !v.is_empty()) {
            cfg.App.Config.File = v.clone();
        }
        if cfg.App.Config.File.is_empty() {
            cfg.App.Config.File = timestamp_log_file_name();
        }
        if let Some(v) = map.get("tidb-host").filter(|v| !v.is_empty()) {
            cfg.TiDB.Host = v.clone();
        }
        if let Some(v) = map.get("tidb-port") {
            let value = v.parse().expect("integer flag validated by parse_flag");
            if value != 0 {
                cfg.TiDB.Port = value;
            }
        }
        if let Some(v) = map.get("tidb-status") {
            let value = v.parse().expect("integer flag validated by parse_flag");
            if value != 0 {
                cfg.TiDB.StatusPort = value;
            }
        }
        if let Some(v) = map.get("tidb-user").filter(|v| !v.is_empty()) {
            cfg.TiDB.User = v.clone();
        }
        if let Some(v) = map.get("tidb-password").filter(|v| !v.is_empty()) {
            cfg.TiDB.Psw = v.clone();
        }
        if let Some(v) = map.get("pd-urls").filter(|v| !v.is_empty()) {
            cfg.TiDB.PdAddr = v.clone();
        }
        if let Some(v) = map.get("d").filter(|v| !v.is_empty()) {
            cfg.Mydumper.SourceDir = v.clone();
        }
        // 中文补充：server-mode 在无显式值时默认为 true，兼容布尔 flag 习惯用法。
        if map.get("server-mode").is_some_and(|v| v == "true") {
            cfg.App.ServerMode = true;
        }
        if let Some(v) = map.get("status-addr").filter(|v| !v.is_empty()) {
            cfg.App.StatusAddr = v.clone();
        }
        // 中文补充：backend 名称只接受当前 stub 能解释的三种模式，
        // 中文补充：其余值立即报错，避免把未知模式静默吞掉。
        if let Some(v) = map.get("backend").filter(|v| !v.is_empty()) {
            match v.as_str() {
                "local" | "tidb" | "import-into" => cfg.TikvImporter.Backend = v.clone(),
                other => {
                    return (None, Some(Error::new(format!("invalid backend: {other}"))));
                }
            }
        }
        if let Some(v) = map.get("sorted-kv-dir").filter(|v| !v.is_empty()) {
            cfg.TikvImporter.SortedKVDir = v.clone();
        }
        if map.get("enable-checkpoint").is_some_and(|v| v == "false") {
            cfg.Checkpoint.Enable = false;
        }
        if map.get("no-schema").is_some_and(|v| v == "true") {
            cfg.Mydumper.NoSchema = true;
        }
        if let Some(v) = map.get("checksum").filter(|v| !v.is_empty()) {
            cfg.PostRestore.Checksum = v.clone();
        }
        if let Some(v) = map.get("analyze").filter(|v| !v.is_empty()) {
            cfg.PostRestore.Analyze = v.clone();
        }
        // 中文补充：`status-addr` 为空但旧参数 `pprof-port` 非零时，仍保留兼容推导。
        if cfg.App.StatusAddr.is_empty() && cfg.App.PProfPort != 0 {
            cfg.App.StatusAddr = format!(":{}", cfg.App.PProfPort);
        }
        if map.get("check-requirements").is_some_and(|v| v == "false") {
            cfg.App.CheckRequirements = false;
        }
        if let Some(v) = map.get("ca").filter(|v| !v.is_empty()) {
            cfg.Security.CAPath = v.clone();
        }
        if let Some(v) = map.get("cert").filter(|v| !v.is_empty()) {
            cfg.Security.CertPath = v.clone();
        }
        if let Some(v) = map.get("key").filter(|v| !v.is_empty()) {
            cfg.Security.KeyPath = v.clone();
        }
        if map.get("redact-info-log").is_some_and(|v| v == "true") {
            cfg.Security.RedactInfoLog = true;
        }
        if !filter.is_empty() {
            cfg.Mydumper.Filter = filter;
        }

        // 中文补充：开启 server-mode 时必须有可监听地址，否则主流程无法安全启动 HTTP 服务。
        if cfg.App.StatusAddr.is_empty() && cfg.App.ServerMode {
            return (
                None,
                Some(Error::new(
                    "If server-mode is enabled, the status-addr must be a valid listen address",
                )),
            );
        }

        // 中文补充：日志级别标准化放在所有来源合并完成之后，确保最终值统一。
        adjust_log_level(&mut cfg.App.Level);
        (Some(cfg), None)
    }

    /// 中文补充：当前只做入口真正依赖的标准化规则，不扩展更多别名映射。
    fn adjust_log_level(level: &mut String) {
        if level.is_empty() {
            *level = "info".into();
        }
        if level.as_str() == "warning" {
            *level = "warn".into();
        }
    }

    /// 中文补充：版本信息在 stub 中固定为 unknown，
    /// 中文补充：目标只是保留 `-V` 的输出形状，而非接入真实构建元数据。
    fn build_info() -> String {
        "Release Version: unknown\nGit Commit Hash: unknown\n".into()
    }

    /// Parse one CLI flag at `i`; returns (key, value, args consumed).
    /// 中文补充：这里采用单遍类型化解析，保留 Go flag 的已知参数、
    /// 中文补充：布尔形式、缺值、整数和枚举校验语义。
    fn parse_flag(args: &[String], i: usize) -> Result<(Option<String>, Option<String>, usize)> {
        let a = &args[i];
        if !a.starts_with('-') {
            return Ok((None, None, 1));
        }
        let raw = a.trim_start_matches('-');
        let (name, inline_value) = match raw.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (raw, None),
        };

        const BOOL_FLAGS: &[&str] = &[
            "server-mode",
            "no-schema",
            "redact-info-log",
            "enable-checkpoint",
            "check-requirements",
            "V",
        ];
        const INT_FLAGS: &[&str] = &["tidb-port", "tidb-status"];
        const STRING_FLAGS: &[&str] = &[
            "c",
            "config",
            "L",
            "log-file",
            "tidb-host",
            "tidb-user",
            "tidb-password",
            "pd-urls",
            "d",
            "backend",
            "sorted-kv-dir",
            "checksum",
            "analyze",
            "ca",
            "cert",
            "key",
            "status-addr",
            "f",
        ];

        if BOOL_FLAGS.contains(&name) {
            let value = inline_value.unwrap_or("true");
            let normalized = match value {
                "1" | "t" | "T" | "true" | "TRUE" | "True" => "true",
                "0" | "f" | "F" | "false" | "FALSE" | "False" => "false",
                _ => {
                    return Err(Error::new(format!(
                        "invalid value {value:?} for boolean flag -{name}"
                    )));
                }
            };
            return Ok((Some(name.to_string()), Some(normalized.to_string()), 1));
        }

        if !INT_FLAGS.contains(&name) && !STRING_FLAGS.contains(&name) {
            return Err(Error::new(format!(
                "flag provided but not defined: -{name}"
            )));
        }

        let (value, consumed) = match inline_value {
            Some(value) => (value.to_string(), 1),
            None => match args.get(i + 1) {
                Some(value) => (value.clone(), 2),
                None => return Err(Error::new(format!("flag needs an argument: -{name}"))),
            },
        };
        if INT_FLAGS.contains(&name) && value.parse::<i32>().is_err() {
            return Err(Error::new(format!(
                "invalid value {value:?} for flag -{name}: parse error"
            )));
        }
        let choices: Option<&[&str]> = match name {
            "L" => Some(&["", "info", "debug", "warn", "warning", "error", "fatal"]),
            "backend" => Some(&["", "local", "tidb", "import-into"]),
            "checksum" | "analyze" => Some(&["", "required", "optional", "off", "true", "false"]),
            _ => None,
        };
        if choices.is_some_and(|allowed| !allowed.contains(&value.as_str())) {
            return Err(Error::new(format!(
                "invalid value {value:?} for flag -{name}"
            )));
        }
        Ok((Some(name.to_string()), Some(value), consumed))
    }

    /// Tiny TOML-ish applicator for keys used by lightning global config.
    /// 中文补充：这里只解析任务文件要求覆盖的那一小组键，
    /// 中文补充：目的是让 `config` 文件参与优先级合并，而不是实现完整 TOML 语义。
    fn apply_toml_lite(cfg: &mut GlobalConfig, data: &[u8]) {
        let text = String::from_utf8_lossy(data);
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            // 中文补充：空行和注释行直接跳过，保持解析器足够简单。
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line.trim_matches(|c| c == '[' || c == ']').to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim().trim_matches('"');
            // 中文补充：未识别键直接忽略，贴近 Go 在只读取关心字段时的容忍策略。
            match (section.as_str(), k) {
                ("lightning", "status-addr") => cfg.App.StatusAddr = v.into(),
                ("lightning", "server-mode") => cfg.App.ServerMode = v == "true",
                ("lightning", "pprof-port") => cfg.App.PProfPort = v.parse().unwrap_or(0),
                ("lightning", "file") => cfg.App.Config.File = v.into(),
                ("lightning", "level") => cfg.App.Level = v.into(),
                ("lightning", "check-requirements") => cfg.App.CheckRequirements = v != "false",
                ("tikv-importer", "backend") => cfg.TikvImporter.Backend = v.into(),
                ("tikv-importer", "sorted-kv-dir") => cfg.TikvImporter.SortedKVDir = v.into(),
                ("tidb", "host") => cfg.TiDB.Host = v.into(),
                ("tidb", "port") => cfg.TiDB.Port = v.parse().unwrap_or(0),
                ("tidb", "user") => cfg.TiDB.User = v.into(),
                ("tidb", "log-level") => cfg.TiDB.LogLevel = v.into(),
                ("mydumper", "data-source-dir") => cfg.Mydumper.SourceDir = v.into(),
                ("checkpoint", "enable") => cfg.Checkpoint.Enable = v != "false",
                _ => {}
            }
        }
    }

    /// Path helper exposed for tests.
    /// 中文补充：测试只关心日志路径是否落在临时目录，不需要知道完整文件名生成策略。
    pub fn temp_log_path_prefix() -> PathBuf {
        std::env::temp_dir()
    }
}
