// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! BR CLI 公共初始化逻辑，对齐 `br/cmd/br/cmd.go`。
//! 该模块集中管理所有子命令共享的全局状态、日志与内存初始化、
//! status/pprof 服务注册，以及 TiDB glue 的过滤器切换。
//! 这里的代码大多不是业务备份逻辑本身，而是为后续 backup/restore/stream
//! 等命令准备一致的运行环境，避免各命令重复实现启动序列。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use astersql_br_pkg_task::{
    DefineCommonFlags as TaskDefineCommonFlags, LogArguments, ParseTLSTripleFromFlags,
};

use crate::stubs::os_stub::{self as os};
use crate::stubs::*;

static INIT_ONCE: Once = Once::new();
static DEFAULT_CONTEXT: Mutex<Option<Context>> = Mutex::new(None);
static HAS_LOG_FILE: AtomicU64 = AtomicU64::new(0);
static TIDB_GLUE: OnceLock<Mutex<TidbGlue>> = OnceLock::new();
static STATUS_PREPARERS: Mutex<Vec<(u64, StatusServerPreparer)>> = Mutex::new(Vec::new());

/// 控制是否把日志强制输出到终端。
///
/// 默认情况下 BR 会把详细日志落到临时文件，便于长任务排查；
/// 显式设置该环境变量后，命令会改为直接向终端输出。
pub const envLogToTermKey: &str = "BR_LOG_TO_TERM";

/// 公共日志级别参数名。
pub const FlagLogLevel: &str = "log-level";
/// 公共日志文件参数名。
pub const FlagLogFile: &str = "log-file";
/// 公共日志格式参数名。
pub const FlagLogFormat: &str = "log-format";
/// status/pprof 监听地址参数名。
pub const FlagStatusAddr: &str = "status-addr";
/// 慢日志输出参数名。
pub const FlagSlowLogFile: &str = "slow-log-file";
/// 旧版日志脱敏开关，仍保留以兼容现有脚本。
pub const FlagRedactLog: &str = "redact-log";
/// 信息日志脱敏开关。
pub const FlagRedactInfoLog: &str = "redact-info-log";

const flagVersion: &str = "version";
const flagVersionShort: &str = "V";

pub const quarterGiB: u64 = 256 * MB;
pub const halfGiB: u64 = 512 * MB;
pub const fourGiB: u64 = 4 * GB;

/// 控制 heap dump 目录的环境变量。
pub const envBRHeapDumpDir: &str = "BR_HEAP_DUMP_DIR";
/// 未指定环境变量时的 heap dump 默认落盘目录。
pub const defaultHeapDumpDir: &str = "/tmp/br_heap_dumps";

/// 惰性创建全局 TiDB glue。
///
/// CLI 命令会在不同执行路径中按需读取或暂时替换其中的 `InfoSchemaFilter`，
/// 因此这里返回带锁的单例，保证修改与恢复在同一份实例上完成。
pub fn tidbGlue() -> &'static Mutex<TidbGlue> {
    TIDB_GLUE.get_or_init(|| Mutex::new(TidbGlue::New()))
}

/// 返回“排除系统库和临时库，但保留授权与 bind 信息”的默认过滤规则。
///
/// 这组规则直接承接 Go 版本的默认行为，确保恢复或校验类命令
/// 在跳过系统对象的同时，仍能带上用户权限和绑定信息。
pub fn filterOutSysAndMemKeepAuthAndBind() -> Vec<String> {
    vec![
        "*.*".into(),
        format!("!{}.*", utils::TemporaryDBName("*")),
        "!mysql.*".into(),
        "mysql.bind_info".into(),
        "mysql.user".into(),
        "mysql.db".into(),
        "mysql.tables_priv".into(),
        "mysql.columns_priv".into(),
        "mysql.global_priv".into(),
        "mysql.global_grants".into(),
        "mysql.default_roles".into(),
        "mysql.role_edges".into(),
        "!sys.*".into(),
        "!INFORMATION_SCHEMA.*".into(),
        "!PERFORMANCE_SCHEMA.*".into(),
        "!METRICS_SCHEMA.*".into(),
        "!INSPECTION_SCHEMA.*".into(),
    ]
}

/// 返回接受所有表的过滤规则。
///
/// 某些子命令会把空数据库/空表选择解释为全量操作，
/// 这里用显式的 `*.*` 规则避免调用端再手写一份默认值。
pub fn acceptAllTables() -> Vec<String> {
    vec!["*.*".into()]
}

/// 临时替换 TiDB glue 的库过滤器，并返回恢复闭包。
///
/// 备份和恢复命令会在特定阶段限制可见数据库集合，
/// 为了避免全局单例长期持有修改后的过滤器，这里要求调用方显式执行恢复闭包。
pub fn setTiDBGlueDBFilter(newFilter: DBFilter) -> Box<dyn FnOnce() + Send> {
    let glue = tidbGlue();
    let mut g = glue.lock().unwrap();
    let old = g.InfoSchemaFilter.clone();
    g.InfoSchemaFilter = InfoSchemaFilter { filter: newFilter };
    Box::new(move || {
        tidbGlue().lock().unwrap().InfoSchemaFilter = old;
    })
}

/// 生成默认日志文件名。
///
/// 复用 trace 模块的 Go 兼容时间格式，生成“临时目录 + 本地时间/时区文件名”。
pub fn timestampLogFileName() -> String {
    let trace_path = astersql_br_pkg_trace::timestampTraceFileName();
    let stamp = std::path::Path::new(&trace_path)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("br.trace."))
        .expect("trace timestamp path must use the Go-compatible layout");
    std::path::Path::new(&os::TempDir())
        .join(format!("br.log.{stamp}"))
        .to_string_lossy()
        .into_owned()
}

/// 为特定命令注册 status server 的额外处理器。
///
/// 有些子命令需要把专有指标或调试接口挂到公共 mux 上，
/// 这里以命令 id 为键保存准备器，避免不同命令之间相互污染。
pub fn registerStatusServerPreparer(cmd: &Command, preparer: StatusServerPreparer) {
    let mut preparers = STATUS_PREPARERS.lock().unwrap();
    if let Some((_, current)) = preparers.iter_mut().find(|(id, _)| *id == cmd.id) {
        *current = preparer;
    } else {
        preparers.push((cmd.id, preparer));
    }
}

/// 根据命令 id 查找并执行对应的 status server 准备器。
///
/// 返回 `Ok(None)` 代表该命令不需要额外挂载 handler，
/// 而不是准备失败，这与 Go 版“未注册则跳过”的语义保持一致。
pub fn prepareStatusServer(cmd: &mut Command) -> Result<Option<StatusServerRegistrar>> {
    let preparer = {
        let list = STATUS_PREPARERS.lock().unwrap();
        list.iter()
            .find(|(id, _)| *id == cmd.id)
            .map(|(_, p)| p.clone())
    };
    match preparer {
        Some(p) => p(cmd),
        None => Ok(None),
    }
}

/// DefineCommonFlags defines the common flags for all BR cmd operation.
///
/// 中文补充：这里统一挂载所有 BR 子命令共享的 flag，
/// 让普通 CLI 命令与 SQL/BRIE 入口能够复用同一套任务层参数定义。
pub fn DefineCommonFlags(cmd: &mut Command) {
    cmd.Version = build::Info();
    cmd.Flags().DefineBool(flagVersion, false);
    // BoolP short form is not on TaskFlagSet; version short is informational.
    let _ = flagVersionShort;
    cmd.SetVersionTemplate("{{printf \"%s\" .Version}}\n");

    cmd.PersistentFlags().DefineString(FlagLogLevel, "info");
    cmd.PersistentFlags()
        .DefineString(FlagLogFile, &timestampLogFileName());
    cmd.PersistentFlags().DefineString(FlagLogFormat, "text");
    cmd.PersistentFlags().DefineBool(FlagRedactLog, false);
    cmd.PersistentFlags().DefineBool(FlagRedactInfoLog, false);
    cmd.PersistentFlags().DefineString(FlagStatusAddr, "");

    TaskDefineCommonFlags(cmd.PersistentFlags());

    cmd.PersistentFlags().DefineString(FlagSlowLogFile, "");
    let _ = cmd.PersistentFlags().MarkHidden(FlagSlowLogFile);
    let _ = cmd.PersistentFlags().MarkHidden(FlagRedactLog);
}

/// 根据可用内存估算 BR 可使用的 GOMEMLIMIT。
///
/// 公式与 Go 版本一致：剩余内存越大，预留给系统和额外缓存的空间越接近 512 MiB；
/// 剩余内存很小时，则优先避免减法下溢和过度压缩可用工作集。
pub fn calculateMemoryLimit(memleft: u64) -> u64 {
    // Special case: if no memory left, return 0
    if memleft == 0 {
        return 0;
    }

    // memreserved = f(memleft) = 512MB * memleft / (memleft + 4GB)
    // Implemented as: halfGiB / (1 + fourGiB/(memleft|1))
    // `memleft | 1` 用于避免除以 0，同时不改变大多数输入下的数量级。
    let memreserved = halfGiB / (1 + fourGiB / (memleft | 1));

    if memreserved >= memleft {
        log::Warn(
            "insufficient memory left for BR, capping to available",
            &[
                zap::Uint64("memleft", memleft),
                zap::Uint64("memreserved", memreserved),
            ],
        );
        return memleft;
    }

    memleft - memreserved
}

/// setupMemoryMonitoring configures memory limits and starts the memory monitor.
///
/// 中文补充：该函数只在 runtime 支持动态内存上限时生效，
/// 它先计算 BR 自身可用内存，再把 heap dump 与超限监控交给公共工具层。
pub fn setupMemoryMonitoring(ctx: &Context, memTotal: u64, memUsed: u64) -> Result<()> {
    if memUsed >= memTotal {
        log::Warn(
            "failed to obtain memory size, skip setting memory limit",
            &[
                zap::Uint64("memused", memUsed),
                zap::Uint64("memtotal", memTotal),
            ],
        );
        return Ok(());
    }

    let memleft = memTotal - memUsed;
    let mut memlimit = calculateMemoryLimit(memleft);
    // 即使剩余内存很小，也尽量给 BR 保留一个最小工作集，避免过早把上限压到不可用。
    // BR command needs 256 MiB at least
    memlimit = memlimit.max(quarterGiB);

    log::Info(
        "calculate the rest memory",
        &[
            zap::Uint64("memtotal", memTotal),
            zap::Uint64("memused", memUsed),
            zap::Uint64("memlimit", memlimit),
        ],
    );

    if memlimit >= i64::MAX as u64 {
        return Ok(());
    }

    debug_runtime::SetMemoryLimit(memlimit as i64);

    let mut dumpDir = os::Getenv(envBRHeapDumpDir);
    if dumpDir.is_empty() {
        // 没有显式目录时回退到稳定默认值，便于统一收集 OOM 现场。
        dumpDir = defaultHeapDumpDir.to_string();
    }

    if let Err(err) = utils::RunMemoryMonitor(ctx, &dumpDir, memlimit) {
        log::Warn("Failed to start memory monitor", &[zap::Error(&err)]);
        return Err(err);
    }

    Ok(())
}

/// Init initializes BR cli.
///
/// 中文补充：初始化通过 `Once` 保证进程级只执行一次，
/// 防止多个子命令或测试重复改写全局 logger、内存 hook 和脱敏开关。
pub fn Init(cmd: &mut Command) -> Result<()> {
    let mut init_err: Option<Error> = None;
    INIT_ONCE.call_once(|| {
        if let Err(e) = init_inner(cmd) {
            init_err = Some(e);
        }
    });
    match init_err {
        Some(e) => Err(Error::Trace(e)),
        None => Ok(()),
    }
}

/// `Init` 的真正实现体。
///
/// 拆分成独立函数后，外层可以把 `Once` 的错误收集逻辑保持简洁，
/// 同时也便于测试直接覆盖初始化细节。
fn init_inner(cmd: &mut Command) -> Result<()> {
    let flags = effective_task_flags(cmd);
    let slowLogFilename = flags.GetString(FlagSlowLogFile).unwrap_or_default();
    let mut tidbLogCfg = logutil::LogConfig::default();
    if !slowLogFilename.is_empty() {
        // 慢日志单独启用时，TiDB 风格日志也需要一个文件承接 gRPC 等额外输出。
        tidbLogCfg.SlowQueryFile = slowLogFilename;
        tidbLogCfg.File.Filename = timestampLogFileName();
    } else {
        // BR 作为离线命令默认关闭 TiDB slow log，避免无意义地持续刷盘。
        config::GetGlobalConfig()
            .Instance()
            .EnableSlowLog_Store(false);
    }
    logutil::InitLogger(&tidbLogCfg)?;

    let mut conf = log::Config::default();
    conf.Level = flags.GetString(FlagLogLevel)?;
    conf.File.Filename = flags.GetString(FlagLogFile)?;
    conf.Format = flags.GetString(FlagLogFormat)?;

    let outputLogToTerm = os::LookupEnv(envLogToTermKey).is_some();
    if outputLogToTerm {
        // 环境变量优先级高于 flag，方便在容器或临时排障时无侵入切换输出位置。
        conf.File.Filename = String::new();
    }
    if !conf.File.Filename.is_empty() {
        HAS_LOG_FILE.store(1, Ordering::SeqCst);
        summary::InitCollector(true);
        // 与 Go 版一样打印到 stderr，让调用脚本能看到日志文件位置但不污染 stdout。
        cmd.PrintErr(format!("Detail BR log in {} \n", conf.File.Filename));
    }
    let (lg, p) = log::InitLogger(&conf)?;
    log::ReplaceGlobals(lg, p);
    memory::InitMemoryHook()?;

    if debug_runtime::SetMemoryLimit(-1) == i64::MAX {
        // 返回 `i64::MAX` 代表当前尚未启用显式上限，此时才需要自动计算可用值。
        let memtotal = memory::MemTotal()?;
        let memused = memory::MemUsed()?;
        if let Err(e) = setupMemoryMonitoring(&GetDefaultContext(), memtotal, memused) {
            // 内存监控失败只记日志，不中断主命令，和 Go 版容错策略一致。
            log::Error("Failed to setup memory monitoring", &[zap::Error(&e)]);
        }
    }

    let redactLog = flags.GetBool(FlagRedactLog)?;
    let redactInfoLog = flags.GetBool(FlagRedactInfoLog)?;
    redact::InitRedact(redactLog || redactInfoLog);
    startStatusServer(cmd)?;
    Ok(())
}

/// Initialize the metrics/pprof server.
///
/// 中文补充：该函数负责拼出最终监听器。
/// 若用户显式提供 `status-addr`，则启动固定地址服务；
/// 否则仅打开动态 pprof 监听，减少默认暴露面。
pub fn startStatusServer(cmd: &mut Command) -> Result<()> {
    let registrar = prepareStatusServer(cmd)?;
    let flags = effective_task_flags(cmd);
    let statusAddr = flags.GetString(FlagStatusAddr)?;
    let (ca, cert, key) = ParseTLSTripleFromFlags(&flags).map_err(Error::from)?;
    // 主机名在此处只用于构造 TLS 配置，实际监听地址由 status flag 决定。
    let tls = NewTLS(&ca, &cert, &key, "localhost")?;

    let mut mux = ServeMux::default();
    // 默认 handler 必须始终存在，额外注册器只能在此基础上追加。
    utils::RegisterDefaultStatusHandlers(&mut mux);
    if let Some(reg) = registrar {
        reg(&mut mux);
    }

    if !statusAddr.is_empty() {
        return utils::StartStatusListenerWithHandler(&statusAddr, &tls, mux);
    }
    utils::StartDynamicPProfListener(&tls);
    Ok(())
}

/// HasLogFile returns whether we set a log file.
///
/// 中文补充：该标记会被 backup/restore 等命令读取，
/// 以决定是否输出进度条或启用与文件日志配套的摘要收集。
pub fn HasLogFile() -> bool {
    HAS_LOG_FILE.load(Ordering::SeqCst) != 0
}

/// SetDefaultContext sets the default context for command line usage.
///
/// 中文补充：测试或嵌入式调用方可以预先注入上下文，
/// 让后续初始化与 tracing 使用同一份取消/超时语义。
pub fn SetDefaultContext(ctx: Context) {
    *DEFAULT_CONTEXT.lock().unwrap() = Some(ctx);
}

/// GetDefaultContext returns the default context for command line usage.
///
/// 中文补充：若调用方未主动设置，则回退到后台上下文，
/// 保证 CLI 路径始终能拿到可用的默认值。
pub fn GetDefaultContext() -> Context {
    DEFAULT_CONTEXT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(Context::Background)
}

/// Reset init-once state for tests.
///
/// 中文补充：由于 `Once` 无法真正重置，这里只清理可变的全局副作用，
/// 让测试通过直接 helper 调用覆盖不同初始化场景。
pub fn reset_init_for_test() {
    HAS_LOG_FILE.store(0, Ordering::SeqCst);
    summary::InitCollector(false);
    redact::InitRedact(false);
    config::reset_for_test();
    STATUS_PREPARERS.lock().unwrap().clear();
    // Once cannot be reset; tests that need re-init use direct helpers.
}

/// 统一记录命令行参数，便于任务层生成与 Go 版本一致的审计日志。
pub fn log_arguments_for(cmd: &Command) {
    let flags = effective_task_flags(cmd);
    LogArguments(&cmd.Use, &flags);
}

/// 按需包装 tracing 生命周期。
///
/// tracing 关闭时直接执行闭包，避免给普通命令增加额外开销；
/// 打开时则显式补齐 span 的开始与结束，保持调用接口对业务层透明。
pub fn with_tracing<T>(
    enable: bool,
    ctx: Context,
    f: impl FnOnce(Context) -> Result<T>,
) -> Result<T> {
    if !enable {
        return f(ctx);
    }
    let (next_ctx, store) =
        astersql_br_pkg_trace::TracerStartSpan(astersql_br_pkg_trace::Context::Background());
    let result = f(ctx);
    astersql_br_pkg_trace::TracerFinishSpan(next_ctx, store);
    result
}
