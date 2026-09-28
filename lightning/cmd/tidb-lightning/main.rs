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

//! tidb-lightning process entry — mirrors `lightning/cmd/tidb-lightning/main.go`.
//! 该模块是 Lightning CLI 的 Rust 入口，负责把命令行参数转换为一次性导入
//! 或常驻服务两条执行路径，并尽量保持与 Go 版本一致的启动和退出语义。
//! 这里不承载具体导入逻辑，而是串联全局配置、日志、信号处理、进度展示
//! 与最终退出码，确保外部调用方式和运维观测点与原实现保持对齐。

use std::sync::{Arc, Mutex};

use astersql_lightning_pkg_progress as progress;
use astersql_lightning_pkg_server as server;
use server::{Error, Result, common, config as server_config, context, log, zap};

use crate::fips;
use crate::stubs::{self, config, debug, memory, os_signal};

/// Lightning surface used by the CLI (real `server::Lightning` or test mock).
/// 入口层只依赖这一组最小能力，而不直接耦合具体实现，
/// 这样既能复用真实 `server::Lightning`，也能在单测里注入替身。
/// 这些方法名沿用 Go 风格，是为了让 Rust 端迁移后的调用关系
/// 和上游源码保持一一对应，便于逐步核对行为差异。
pub trait LightningApp: Send {
    fn GoServe(&mut self) -> Result<()>;
    fn Stop(&mut self);
    fn RunServer(&mut self) -> Result<()>;
    fn RunOnceWithOptions(
        &mut self,
        ctx: context::Context,
        cfg: server_config::Config,
    ) -> Result<()>;
    fn TaskCanceled(&self) -> bool;
}

/// 把真实的 `server::Lightning` 包装成 trait object 可消费的外观。
/// 包装层本身不增加逻辑，只负责把入口依赖的调用转发给真实服务对象。
struct RealLightning(Box<server::Lightning>);

impl LightningApp for RealLightning {
    fn GoServe(&mut self) -> Result<()> {
        self.0.GoServe()
    }
    fn Stop(&mut self) {
        self.0.Stop();
    }
    fn RunServer(&mut self) -> Result<()> {
        self.0.RunServer()
    }
    fn RunOnceWithOptions(
        &mut self,
        ctx: context::Context,
        cfg: server_config::Config,
    ) -> Result<()> {
        self.0.RunOnceWithOptions(ctx, cfg, Vec::new())
    }
    fn TaskCanceled(&self) -> bool {
        self.0.TaskCanceled()
    }
}

/// Process entry matching Go `main`.
/// 这里先做 FIPS 相关初始化，再运行主体逻辑；
/// 只有在非零退出码时才显式调用 `exit`，从而保留测试钩子接管退出的能力。
pub fn main() {
    fips::init_fips_only_tls_for_boringcrypto_build();
    let code = run(std::env::args().skip(1).collect());
    if code != 0 {
        stubs::exit(code);
    }
}

/// Runnable CLI body. Returns process exit code (0 = success).
/// 对外暴露一个纯函数式入口，便于测试直接校验退出码，
/// 同时把真实 Lightning 的构造细节集中在闭包里。
pub fn run(args: Vec<String>) -> i32 {
    run_with_factory(args, true, |g| {
        let sg = config::to_server_global(&g);
        Ok(RealLightning(server::New(sg)))
    })
}

/// Same control flow as Go `main`, with injectable Lightning factory for tests.
///
/// `install_signals`: when false, skip the OS signal goroutine (unit tests).
/// 这是实际的控制流实现：它兼顾生产入口与测试场景，
/// 通过工厂函数和信号开关把环境相关依赖隔离出去。
pub fn run_with_factory<F, A>(args: Vec<String>, install_signals: bool, new_app: F) -> i32
where
    F: FnOnce(config::GlobalConfig) -> Result<A>,
    A: LightningApp + 'static,
{
    let (cfg_opt, err_opt) = config::LoadGlobalConfig(&args, None);
    // Go `Must` may call exit(0)/exit(2); with a test hook it returns.
    // 先解析全局配置，并立即读取替身 `exit` 写下的状态码。
    // 这样可以复现 Go 版在参数错误或帮助输出时“提前退出”的语义，
    // 同时避免测试进程真的被终止。
    let globalCfg = config::Must(cfg_opt, err_opt);
    if let Some(code) = stubs::take_exit_code() {
        return code;
    }

    // 仅当日志真正写入文件时，才在标准输出打印提示并在结束时尝试 `Sync`；
    // 这样与 Go 版一致，避免对 stdout/stderr logger 做多余同步。
    let log_to_file = {
        let f = globalCfg.App.File();
        !f.is_empty() && f != "-"
    };
    if log_to_file {
        println!(
            "Verbose debug logs will be written to {}\n",
            globalCfg.App.Config.File
        );
    }

    // 应用实例创建失败属于启动前错误，直接返回非零退出码，
    // 不进入后续的信号注册、HTTP 服务和导入流程。
    let app = match new_app(globalCfg.clone()) {
        Ok(a) => a,
        Err(err) => {
            eprintln!("failed to create lightning: {err}");
            return 1;
        }
    };
    let app = Arc::new(Mutex::new(app));

    // 内存观测钩子失败不会阻止主流程，只记录日志；
    // 这说明它属于附加诊断能力，而不是启动硬依赖。
    if let Err(err) = memory::InitMemoryHook() {
        log::L().Error("failed to initialize memory usage hook", zap::Error(err));
    }

    // 生产模式下安装信号处理线程，把一次外部终止请求转换为 `Stop()`；
    // 单测可关闭该逻辑，避免测试进程里引入额外线程和全局状态竞争。
    if install_signals {
        let signal_app = app.clone();
        std::thread::spawn(move || {
            let sig = os_signal::wait_for_one_of(&["SIGHUP", "SIGINT", "SIGTERM", "SIGQUIT"]);
            log::L().Info("got signal to exit", zap::String("signal", &sig));
            if let Ok(mut g) = signal_app.lock() {
                g.Stop();
            }
        });
    }

    let logger = log::L();

    // Non-local backends: default GOGC 500 when unset (Go debug.SetGCPercent).
    // 这里延续 Go 版经验参数：非 local 后端会产生大量短命对象，
    // 如果没有显式配置 `GOGC`，就把默认值提高到 500 以减少 GC 频率。
    // local 后端内存压力更高，因此保持默认行为，不在这里放大堆增长。
    if globalCfg.TikvImporter.Backend != config::BackendLocal {
        let gogc = debug::gogc_env();
        if gogc.is_empty() {
            let old = debug::SetGCPercent(500);
            // Go logs both old and new; stub logger accepts one field.
            let _ = old;
            log::L().Debug("set gc percentage", zap::Int("new", 500));
        }
    }

    // HTTP 状态服务要先于主任务启动，因为后续导入或 server mode
    // 都依赖它暴露状态、调试入口或对外管理接口。
    {
        let mut guard = app.lock().unwrap();
        if let Err(serve_err) = guard.GoServe() {
            logger.Error("failed to start HTTP server", zap::Error(&serve_err));
            eprintln!("failed to start HTTP server: {serve_err}");
            return 0; // Go `return` without exit(1) — process status 0
        }
    }

    // 只有配置了状态地址时才启用当前进度展示，
    // 保证无状态监听场景不会额外暴露进度对象。
    if !globalCfg.App.StatusAddr.is_empty() {
        progress::EnableCurrentProgress();
    }

    // 两种运行模式共享同一入口：
    // server mode 进入常驻服务；否则构造一次性任务配置并直接执行导入。
    // 这里把全局配置再映射成 server 侧配置，是因为运行层需要的是
    // 更贴近执行时语义的结构，而不是 CLI 原始配置视图。
    let mut err: Result<()> = if globalCfg.App.ServerMode {
        app.lock().unwrap().RunServer()
    } else {
        let mut cfg = server_config::Config::NewConfig();
        match cfg.LoadFromGlobal(&config::to_server_global(&globalCfg)) {
            Err(e) => Err(e),
            Ok(()) => app
                .lock()
                .unwrap()
                .RunOnceWithOptions(context::Background(), cfg),
        }
    };

    // 上下文取消不总是意味着失败。
    // 如果是外部取消且任务对象确认未完成，应把最终文案标记为 canceled；
    // 若只是伴随正常收尾出现的取消错误，则把它视为成功退出。
    let mut finished = true;
    if let Err(ref e) = err {
        if common::IsContextCanceledError(e) {
            err = Ok(());
            if app.lock().unwrap().TaskCanceled() {
                finished = false;
            }
        }
    }

    // 最终输出同时兼顾机器日志和命令行可读性：
    // 错误路径写日志并输出 stderr，成功路径记录 finished 状态并打印人类可读结果。
    if let Err(ref run_err) = err {
        logger.Error(
            "tidb lightning encountered error stack info",
            zap::Error(run_err),
        );
        eprintln!("tidb lightning encountered error: {run_err}");
    } else {
        logger.Info("tidb lightning exit", zap::Bool("finished", finished));
        let exit_msg = if finished {
            "tidb lightning exit successfully"
        } else {
            "tidb lightning canceled"
        };
        println!("{exit_msg}");
    }

    // 仅文件日志需要显式同步；若同步失败，只补充 stderr 提示，
    // 不覆盖主流程已经得到的执行结果。
    if log_to_file {
        if let Err(sync_err) = stubs::logger_sync() {
            eprintln!("sync log failed {sync_err}");
        }
    }

    // 退出码约束与 Go 版保持一致：
    // 运行错误返回 1；server mode 被取消也返回 1；
    // 一次性导入被取消但已按约定转成非错误时，允许返回 0。
    if err.is_err() || (globalCfg.App.ServerMode && !finished) {
        stubs::exit(1);
        return 1;
    }
    0
}
