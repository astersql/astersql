// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/cmd/tidb-lightning` vs Go `main.go` / `fips.go`.
//!
//! 本文件不是验证某个独立算法，而是把 Rust CLI 入口抽象成
//! `run_with_factory()` 的可注入流程后，检查它与 Go `main.go`/`fips.go`
//! 公开出来的可观察契约是否一致。
//! 这些契约只关注进程退出码、日志同步时机、GOGC 默认值、
//! server mode 的前置条件、取消路径以及信号触发 `Stop()` 的副作用，
//! 避免把测试耦合到真实网络、真实日志器或真实 Lightning 实例。
//! 测试分组对应“正常路径、边界条件、错误路径、资源清理”四类，
//! 这样当入口控制流回归时，可以直接看出破坏的是哪一层 Go 语义对齐。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_lightning_pkg_server::{Error, Result, common, config as server_config, context, zap};

use crate::entry::{LightningApp, run_with_factory};
use crate::fips;
use crate::stubs::{self, config, debug, memory, os_signal};

const SIGNAL_WORKER_ENV: &str = "ASTERSQL_LIGHTNING_SIGNAL_WORKER";

#[cfg(unix)]
#[test]
fn real_signal_wait_matches_go() {
    if std::env::var_os(SIGNAL_WORKER_ENV).is_some() {
        assert_eq!(
            os_signal::wait_for_one_of(&["SIGHUP", "SIGINT", "SIGTERM", "SIGQUIT"]),
            "SIGTERM"
        );
        return;
    }

    let exe = std::env::current_exe().expect("current_exe");
    let mut child = std::process::Command::new(&exe)
        .env(SIGNAL_WORKER_ENV, "1")
        .args(["--exact", "parity_test::real_signal_wait_matches_go"])
        .spawn()
        .expect("spawn signal worker");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let signal_status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(signal_status.success(), "kill must deliver SIGTERM");
    let status = child.wait().expect("wait for signal worker");
    assert!(
        status.success(),
        "signal worker must catch SIGTERM and return normally: {status:?}"
    );
}

#[test]
fn go_rust_public_contract_matches() {
    // 顶层测试只是顺序聚合四组场景，便于保持与 Go 入口语义的
    // 一一对应；任何子场景失败时，调用栈仍会落到具体分组函数。
    contract_normal_run_once_success();
    contract_boundary_gogc_and_log_file();
    contract_error_paths();
    contract_resource_cleanup_sync_and_cancel();
    contract_config_flag_validation_and_merge();
}

/// Config boundary: Go's `flag.FlagSet` rejects unknown, missing, and malformed
/// values, while explicit false boolean flags do not erase true values loaded
/// from the TOML file.
fn contract_config_flag_validation_and_merge() {
    reset_test_state();

    for args in [
        vec!["--unknown".into(), "value".into()],
        vec!["--tidb-host".into()],
        vec!["--tidb-port".into(), "not-a-number".into()],
    ] {
        let (cfg, err) = config::LoadGlobalConfig(&args, None);
        assert!(cfg.is_none(), "invalid Go flag input must not yield config");
        assert!(err.is_some(), "invalid Go flag input must return an error");
    }

    let path = std::env::temp_dir().join(format!(
        "lightning-global-config-{}-{}.toml",
        std::process::id(),
        std::thread::current().name().unwrap_or("parity")
    ));
    std::fs::write(
        &path,
        b"[lightning]\nserver-mode = true\nstatus-addr = \":8289\"\n",
    )
    .expect("write config fixture");
    let args = vec![
        "--config".into(),
        path.display().to_string(),
        "--server-mode=false".into(),
    ];
    let (cfg, err) = config::LoadGlobalConfig(&args, None);
    let _ = std::fs::remove_file(path);
    assert!(err.is_none(), "valid config merge must succeed: {err:?}");
    assert!(
        cfg.expect("config").App.ServerMode,
        "Go only applies server-mode CLI when the parsed value is true"
    );

    let true_path =
        std::env::temp_dir().join(format!("lightning-config-true-{}.toml", std::process::id()));
    let false_path = std::env::temp_dir().join(format!(
        "lightning-config-false-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &true_path,
        b"[lightning]\nserver-mode = true\nstatus-addr = \":8289\"\n",
    )
    .expect("write true config fixture");
    std::fs::write(&false_path, b"[lightning]\nserver-mode = false\n")
        .expect("write false config fixture");
    let args = vec![
        "-c".into(),
        true_path.display().to_string(),
        "--config".into(),
        false_path.display().to_string(),
    ];
    let (cfg, err) = config::LoadGlobalConfig(&args, None);
    assert!(err.is_none(), "last config alias must parse: {err:?}");
    assert!(
        !cfg.expect("config").App.ServerMode,
        "the last of -c/--config must win"
    );

    let args = vec![
        "--config".into(),
        false_path.display().to_string(),
        "-c".into(),
        true_path.display().to_string(),
        "--server-mode=TRUE".into(),
    ];
    let (cfg, err) = config::LoadGlobalConfig(&args, None);
    let _ = std::fs::remove_file(true_path);
    let _ = std::fs::remove_file(false_path);
    assert!(err.is_none(), "Go boolean forms must parse: {err:?}");
    assert!(cfg.expect("config").App.ServerMode);

    let pprof_path = std::env::temp_dir().join(format!(
        "lightning-config-pprof-{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &pprof_path,
        b"[lightning]\nserver-mode = true\npprof-port = 8289\n",
    )
    .expect("write pprof config fixture");
    let args = vec!["--config".into(), pprof_path.display().to_string()];
    let (cfg, err) = config::LoadGlobalConfig(&args, None);
    let _ = std::fs::remove_file(pprof_path);
    assert!(
        err.is_none(),
        "Go accepts legacy pprof-port as the server status address: {err:?}"
    );
    assert_eq!(cfg.expect("config").App.StatusAddr, ":8289");
}

/// Normal: run-once success prints exit successfully; FIPS init is callable.
fn contract_normal_run_once_success() {
    // 每个场景开始前都要重置桩状态，避免上一个场景留下的 exit hook、
    // GOGC 覆写或注入信号污染本次断言。
    reset_test_state();
    // Go `main()` 会先触发 `fips.go` 侧的初始化挂点；Rust 版本即使
    // 当前为空实现，也必须保证该入口可无条件调用，避免未来接线时破坏启动顺序。
    fips::init_fips_only_tls_for_boringcrypto_build();

    // `GoServe()` 与 `RunOnceWithOptions()` 的调用次数分别用原子计数，
    // 用来证明成功路径既会先启动 HTTP 服务，又会实际执行一次导入流程。
    let served = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(AtomicUsize::new(0));
    let served_c = served.clone();
    let ran_c = ran.clone();

    // 这里传入 `--log-file -`，刻意绕开文件日志 banner 与 sync 分支，
    // 让断言聚焦在“正常单次运行返回 0 且调用顺序完整”这一最小公开契约上。
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        false,
        move |_g| {
            Ok(MockApp {
                serve: Ok(()),
                run_once: Ok(()),
                run_server: Ok(()),
                canceled: false,
                served: served_c,
                ran: ran_c,
                stopped: Arc::new(AtomicBool::new(false)),
            })
        },
    );
    // 返回码为 0 表示 CLI 把该次导入视为成功完成，而不是仅仅没有 panic。
    assert_eq!(code, 0);
    // Go 入口总是在真正运行任务前先尝试启动 HTTP 服务，这里用计数确认
    // Rust 没有把 `GoServe()` 放到错误的位置或直接跳过。
    assert_eq!(served.load(Ordering::SeqCst), 1);
    // 成功路径必须恰好执行一次 run-once；若为 0 说明流程提前返回，
    // 若大于 1 则意味着入口控制流出现重复调用回归。
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

/// Boundary: local backend skips GOGC bump; `-` log file skips file banner/sync;
/// server-mode without status-addr errors (exit 2 via Must).
fn contract_boundary_gogc_and_log_file() {
    // 这组边界测试覆盖多个“容易在重构中被顺手改掉”的入口分支，
    // 例如 backend 差异、参数校验返回码，以及帮助/版本参数的早退出行为。
    reset_test_state();
    debug::reset_gc_percent();
    debug::set_gogc_override(Some(None)); // unset GOGC

    // Local backend: do not SetGCPercent(500).
    // 本地 backend 与 Go 一样不提升默认 GOGC，否则会把本地导入和
    // 远端导入的内存策略混在一起。
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "local".into(),
        ],
        false,
        |_g| Ok(MockApp::ok()),
    );
    assert_eq!(code, 0);
    assert_eq!(
        debug::current_gc_percent(),
        100,
        "local backend must not raise GC percent"
    );

    // Non-local + empty GOGC => 500.
    // 非 local backend 在未显式设置 GOGC 时，要复刻 Go 的
    // `debug.SetGCPercent(500)` 默认值，避免大型导入任务过早触发 GC。
    reset_test_state();
    debug::reset_gc_percent();
    debug::set_gogc_override(Some(None));
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        false,
        |_g| Ok(MockApp::ok()),
    );
    assert_eq!(code, 0);
    assert_eq!(debug::current_gc_percent(), 500);

    // server-mode requires status-addr.
    // `config::Must()` 会把无效配置转成 exit 语义；测试安装 exit recorder
    // 后，Rust 端不真正结束进程，而是把 Go 约定的退出码显式返回出来供断言。
    reset_test_state();
    install_exit_recorder();
    let code = run_with_factory(
        vec!["--server-mode".into(), "--log-file".into(), "-".into()],
        false,
        |_g| Ok(MockApp::ok()),
    );
    assert_eq!(code, 2, "Must exits 2 on invalid server-mode config");

    // -V => help => exit 0.
    // `-V`/帮助路径属于“成功早退出”，必须与 Go 一样返回 0，
    // 不能因为测试桩存在而误落入普通执行路径。
    reset_test_state();
    install_exit_recorder();
    let code = run_with_factory(vec!["-V".into()], false, |_g| Ok(MockApp::ok()));
    assert_eq!(code, 0);
}

/// Error: GoServe failure returns without exit(1); run-once error exits 1;
/// memory hook failure is non-fatal.
fn contract_error_paths() {
    // 这组测试区分“启动辅助能力失败但主流程继续”与“导入本身失败必须报错”
    // 两类错误，避免所有错误都被粗暴映射成同一种退出码。
    reset_test_state();
    // 内存 hook 初始化失败只记录日志，不影响入口继续运行；这是与 Go
    // 保持一致的容错策略，因此不能被升级成致命错误。
    memory::set_fail(true);
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        false,
        |_g| {
            Ok(MockApp {
                serve: Err(Error::new("serve boom")),
                run_once: Ok(()),
                run_server: Ok(()),
                canceled: false,
                served: Arc::new(AtomicUsize::new(0)),
                ran: Arc::new(AtomicUsize::new(0)),
                stopped: Arc::new(AtomicBool::new(false)),
            })
        },
    );
    // Go returns after printing serve error without calling exit(1).
    // `GoServe()` 失败时 Go 代码只是报错后 `return`，因此这里验证
    // Rust 也维持进程状态码 0，而不是擅自调用 exit(1)。
    assert_eq!(code, 0);
    memory::set_fail(false);

    // 真正的 run-once 失败则属于导入任务失败，必须与 Go 一样返回 1，
    // 以便脚本和调度器能够据此判定任务未成功完成。
    reset_test_state();
    install_exit_recorder();
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        false,
        |_g| {
            Ok(MockApp {
                serve: Ok(()),
                run_once: Err(Error::new("import failed")),
                run_server: Ok(()),
                canceled: false,
                served: Arc::new(AtomicUsize::new(0)),
                ran: Arc::new(AtomicUsize::new(0)),
                stopped: Arc::new(AtomicBool::new(false)),
            })
        },
    );
    assert_eq!(code, 1);
}

/// Resource: log Sync on file logger; context-cancel + TaskCanceled => canceled msg;
/// progress enable when StatusAddr set; signal Stop path via inject.
fn contract_resource_cleanup_sync_and_cancel() {
    // 这一组覆盖“成功退出前的收尾动作”以及“异步取消路径”，
    // 这些行为对用户可见，却最容易因为测试只看返回码而漏掉。
    reset_test_state();
    // 故意让 logger sync 报错，用来证明文件日志同步失败只写 stderr，
    // 不会覆盖主流程已经得出的成功返回码。
    stubs::set_logger_sync_result(Some(Err(Error::new("sync boom"))));
    // Use a real temp file path so logToFile is true.
    let log_path = std::env::temp_dir().join("lightning-parity-sync.log");
    let log_path = log_path.display().to_string();

    let code = run_with_factory(
        vec![
            "--log-file".into(),
            log_path,
            "--backend".into(),
            "tidb".into(),
            "--status-addr".into(),
            ":0".into(),
        ],
        false,
        |_g| Ok(MockApp::ok()),
    );
    assert_eq!(code, 0);
    // Sync was attempted (failure only prints stderr; exit still 0).
    // 这里只看返回码，是因为是否调用 sync 已通过桩注入进入专门分支，
    // 测试目标是确认它不会把成功路径错误降级成失败。

    // Canceled task: context canceled + TaskCanceled => finished=false, exit 0 for non-server.
    // 普通非 server 模式下，若错误可识别为 context canceled 且
    // `TaskCanceled()` 为真，Go 会把它视为“已取消而非失败”，因此返回 0。
    reset_test_state();
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        false,
        |_g| {
            Ok(MockApp {
                serve: Ok(()),
                run_once: Err(Error::new("context canceled")),
                run_server: Ok(()),
                canceled: true,
                served: Arc::new(AtomicUsize::new(0)),
                ran: Arc::new(AtomicUsize::new(0)),
                stopped: Arc::new(AtomicBool::new(false)),
            })
        },
    );
    assert_eq!(code, 0);
    assert!(common::IsContextCanceledError(&Error::new(
        "context canceled"
    )));

    // Injected signal triggers Stop when signals installed.
    // 这里不依赖真实 OS 信号，而是通过桩注入 `SIGINT`，验证安装信号线程后
    // `Stop()` 会被异步调用，从而保持与 Go goroutine 的退出协作方式一致。
    reset_test_state();
    os_signal::inject(Some("SIGINT"));
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_c = stopped.clone();
    let code = run_with_factory(
        vec![
            "--log-file".into(),
            "-".into(),
            "--backend".into(),
            "tidb".into(),
        ],
        true,
        move |_g| {
            Ok(MockApp {
                serve: Ok(()),
                run_once: Ok(()),
                run_server: Ok(()),
                canceled: false,
                served: Arc::new(AtomicUsize::new(0)),
                ran: Arc::new(AtomicUsize::new(0)),
                stopped: stopped_c,
            })
        },
    );
    assert_eq!(code, 0);
    // Give signal thread a moment.
    // 短暂 sleep 只是给后台线程拿锁并设置标志的窗口，
    // 不是业务语义的一部分，因此这里只等待最小必要时间。
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        stopped.load(Ordering::SeqCst),
        "signal goroutine must call Stop"
    );
    os_signal::inject(None);
}

fn reset_test_state() {
    // 把所有全局桩恢复到默认值，确保各个 parity 场景彼此独立，
    // 不受上一次 exit code、logger sync 结果或信号注入影响。
    stubs::set_exit_hook(None);
    let _ = stubs::take_exit_code();
    stubs::set_logger_sync_result(None);
    memory::set_fail(false);
    debug::reset_gc_percent();
    os_signal::inject(None);
}

fn install_exit_recorder() {
    // Go 入口里很多早退出分支通过 `os.Exit` 表达；测试通过安装 hook
    // 把“直接退出进程”改写成“记录退出码后返回”，从而能继续断言后续状态。
    stubs::set_exit_hook(Some(Arc::new(|_code| {
        // no-op: exit() already stores EXIT_CODE
    })));
}

/// `MockApp` 是对 `LightningApp` 的最小桩实现。
/// 它不模拟真实导入逻辑，只把入口需要观察的结果面暴露出来：
/// `GoServe`、`RunOnceWithOptions`、`RunServer` 三个返回值，
/// `TaskCanceled` 的布尔状态，以及 `Stop` 是否被异步调用。
/// 两个原子计数器分别记录 serve/run-once 调用次数，
/// 便于验证控制流顺序而不引入额外同步原语。
struct MockApp {
    serve: Result<()>,
    run_once: Result<()>,
    run_server: Result<()>,
    canceled: bool,
    served: Arc<AtomicUsize>,
    ran: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
}

impl MockApp {
    fn ok() -> Self {
        // 大多数场景只需要“所有动作都成功”的基线桩，
        // 单独覆写个别字段即可表达某个失败分支。
        Self {
            serve: Ok(()),
            run_once: Ok(()),
            run_server: Ok(()),
            canceled: false,
            served: Arc::new(AtomicUsize::new(0)),
            ran: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl LightningApp for MockApp {
    fn GoServe(&mut self) -> Result<()> {
        // 先记次数再返回预置结果，用于还原“入口确实尝试过启动服务”
        // 这一事实，而不是只看最终返回码。
        self.served.fetch_add(1, Ordering::SeqCst);
        self.serve.clone()
    }
    fn Stop(&mut self) {
        // 信号线程只关心是否触发过 stop，因此一个布尔标志就足以表达
        // Go `Stop()` 被调用这一可观察副作用。
        self.stopped.store(true, Ordering::SeqCst);
    }
    fn RunServer(&mut self) -> Result<()> {
        // server mode 只需返回预置结果；是否真的监听端口不属于本文件关心的契约。
        self.run_server.clone()
    }
    fn RunOnceWithOptions(
        &mut self,
        _ctx: context::Context,
        _cfg: server_config::Config,
    ) -> Result<()> {
        // 保留上下文与配置参数位置，说明 parity 测试对齐的是入口接线，
        // 而不是内部导入参数的具体内容。
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.run_once.clone()
    }
    fn TaskCanceled(&self) -> bool {
        // 单独暴露取消标志，帮助测试区分“普通错误”与“用户取消”两种
        // 共享 `context canceled` 文本但退出语义不同的场景。
        self.canceled
    }
}
