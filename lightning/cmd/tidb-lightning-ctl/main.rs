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

//! tidb-lightning-ctl entry — mirrors `lightning/cmd/tidb-lightning-ctl/main.go`.
//! 中文补充：该文件承接控制面命令的真正主流程，负责把命令行参数翻译成 PD/TiKV 或 checkpoint 管理动作。
//! 中文补充：整体结构刻意贴近 Go 版本，保证退出码、报错文案和资源释放路径都能被 parity test 直接比对。

use crate::fips;
use crate::stubs::*;
use std::io::Write;
use std::sync::atomic::Ordering;

/// Process entry matching Go `main`.
/// 中文补充：默认入口只做参数收集，便于测试通过 `main_with_args` 直接覆盖不同 argv 组合。
pub fn main() {
    main_with_args(std::env::args().skip(1).collect());
}

/// Process entry with explicit argv, preserving Go's `Must` and fatal-error exit codes.
/// 中文补充：这里先触发 FIPS 初始化钩子，再统一把非零退出码交给可替换的 `call_exit`，以保持测试可控。
pub fn main_with_args(args: Vec<String>) {
    fips::enable_fips_only();
    let code = run_main(args);
    if code != 0 {
        call_exit(code);
    }
}

pub const checkpointTableNotFoundUsage: &str = "valid examples: --checkpoint-error-ignore='`db`.`table`', --checkpoint-error-destroy='`db`.`table`', or 'all'";
// 中文补充：这段常量专门服务于 checkpoint 表缺失报错。
// 中文补充：它把三个可执行的恢复动作直接内嵌到提示里，减少用户再查文档的成本。

/// Fatal error formatting matching Go `formatFatalError`.
/// 中文补充：checkpoint 表不存在属于“给用户纠正参数”的场景，因此只追加示例，不附带栈样式噪声。
pub fn formatFatalError(err: &Error) -> String {
    // Keep stack traces for debugging unexpected failures, but avoid stack noise for
    // the user-facing "checkpoint table not found" guidance error.
    if ErrCheckpointTableNotFound.Equal(err) {
        return format!("{}; {}", err.Error(), checkpointTableNotFoundUsage);
    }
    // Generic path: include a stack-like suffix (Go errors.ErrorStack).
    format!("{}\n{}", err.Error(), "lightning/cmd/tidb-lightning-ctl")
}

/// Format a `StackError` (parity with Go stack-bearing errors).
/// 中文补充：带栈错误沿用更完整的 `ErrorStack` 输出，但仍保留 checkpoint 特判以复用相同提示语。
pub fn formatFatalErrorStacked(err: &StackError) -> String {
    if ErrCheckpointTableNotFound.Equal(&err.inner) {
        return format!("{}; {}", err.Error(), checkpointTableNotFoundUsage);
    }
    ErrorStack(err)
}

/// Runnable CLI body with argv (Go `run`).
/// 中文补充：该路径给库调用和测试复用，直接返回 `Result`，不在这里决定进程级退出码。
pub fn run(args: Vec<String>) -> Result<()> {
    let loaded = LoadGlobalConfigWithCtl(&args)?;
    run_loaded(loaded)
}

/// Testable CLI boundary. Exit codes match Go:
/// `0` success/help, `2` flag/config loading errors, `1` runtime errors.
/// 中文补充：把参数装载错误与运行期错误拆成不同退出码，便于脚本层区分“命令用法问题”和“执行失败”。
pub fn run_main(args: Vec<String>) -> i32 {
    let loaded = match LoadGlobalConfigWithCtl(&args) {
        Ok(v) => v,
        // 中文补充：帮助信息在 Go 中也视为成功返回，因此这里直接映射到退出码 0。
        Err(err) if err.Error() == "flag: help requested" => return 0,
        Err(err) => {
            // 中文补充：配置或 flag 解析阶段尚未进入真正业务逻辑，按 Go 约定返回 2。
            println!("{err}");
            return 2;
        }
    };

    match run_loaded(loaded) {
        Ok(()) => 0,
        Err(err) => {
            // 中文补充：真正执行动作失败时把格式化后的错误写入 stderr，并返回通用失败码 1。
            let _ = writeln!(std::io::stderr(), "{}", formatFatalError(&err));
            1
        }
    }
}

fn run_loaded(loaded: (GlobalConfig, CtlActionFlags, FlagSet)) -> Result<()> {
    let (globalCfg, actions, fs) = loaded;
    // 中文补充：这里把“全局命令行配置”压缩成真正运行控制命令所需的局部 `Config`。
    let ctx = context::Background();
    let mut cfg = config::Config::NewConfig();
    LoadFromGlobal(&mut cfg, &globalCfg)?;
    // 中文补充：`Adjust` 会补齐默认值并校验约束，保持与 Go 初始化阶段的行为一致。
    cfg.Adjust(&ctx)?;

    // 中文补充：控制命令既会连 PD，也可能直连 TiKV，因此 TLS 配置要同时构造并预热。
    let tls = ToTLS(&cfg)?;
    cfg.TiDB.Security.BuildTLSConfig()?;

    let mut opts = Vec::new();
    opts.push(WithTLSConfig(tls.TLSConfig()));
    // 中文补充：PD 地址按 Go `strings.Split` 原样切分，空值及连续逗号产生的空片段也会保留。
    let cli = NewClient(
        "lightning-ctl",
        cfg.TiDB.PdAddr.split(',').map(|s| s.to_string()).collect(),
        opts,
    );

    // Go uses defer cli.Close(); Drop + explicit Close cover all return paths.
    // 中文补充：这里显式 `Close`，确保在 `dispatch` 提前返回时也能保留与 Go `defer` 等价的释放时机。
    let result = dispatch(&ctx, &cfg, &tls, &cli, &actions, &fs);
    cli.Close();
    result
}

fn dispatch(
    ctx: &context::Context,
    cfg: &config::Config,
    tls: &common::TLS,
    cli: &PdClient,
    actions: &CtlActionFlags,
    fs: &FlagSet,
) -> Result<()> {
    // 中文补充：动作选择保持“首个命中立即返回”的串行优先级，避免多个互斥控制命令被同时执行。
    if actions.compact {
        return compactCluster(ctx, cli, tls).map_err(errors::Trace);
    }
    if actions.fetch_mode {
        return fetchMode(ctx, cli, tls).map_err(errors::Trace);
    }
    // 中文补充：显式 mode 切换直接走 server client 路径，不再继续评估 checkpoint 相关参数。
    if !actions.mode.is_empty() {
        return SwitchMode(
            ctx,
            cli.as_server_client(),
            &tls.TLSConfig(),
            &actions.mode,
            vec![],
        )
        .map_err(errors::Trace);
    }

    // 中文补充：下面几组 checkpoint 动作都延迟创建控制器，避免无关命令额外建立连接或读取状态。
    if !actions.cp_remove.is_empty() {
        // 中文补充：删除 checkpoint 影响最大，因此只有显式提供目标集合时才进入该路径。
        let mut ctl = NewCheckpointControl(cfg, tls).map_err(errors::Trace)?;
        return ctl.Remove(ctx, &actions.cp_remove).map_err(errors::Trace);
    }
    if !actions.cp_err_ignore.is_empty() {
        // 中文补充：忽略错误用于保留导入进度但跳过坏记录，控制器仍按需即时创建。
        let mut ctl = NewCheckpointControl(cfg, tls).map_err(errors::Trace)?;
        return ctl
            .IgnoreError(ctx, &actions.cp_err_ignore)
            .map_err(errors::Trace);
    }
    if !actions.cp_err_destroy.is_empty() {
        // 中文补充：销毁错误记录是更激进的恢复手段，因此与 ignore 保持独立旗标和独立返回路径。
        let mut ctl = NewCheckpointControl(cfg, tls).map_err(errors::Trace)?;
        return ctl
            .DestroyError(ctx, &actions.cp_err_destroy)
            .map_err(errors::Trace);
    }
    if !actions.cp_dump.is_empty() {
        // 中文补充：dump 走只读查询路径，仍复用同一个 checkpoint 控制器封装数据库访问细节。
        let mut ctl = NewCheckpointControl(cfg, tls).map_err(errors::Trace)?;
        return ctl.Dump(ctx, &actions.cp_dump).map_err(errors::Trace);
    }
    if actions.local_storing_tables {
        // 中文补充：该命令用于排查本地中间文件丢失，不修改 checkpoint，只报告当前观测结果。
        let mut ctl = NewCheckpointControl(cfg, tls).map_err(errors::Trace)?;
        let tables = ctl.GetLocalStoringTables(ctx).map_err(errors::Trace)?;
        // 中文补充：`None` 与空 map 都表示“没有需要报告的表”，输出上保持同一条提示即可。
        let empty = match &tables {
            None => true,
            Some(m) => m.is_empty(),
        };
        if empty {
            // 中文补充：空结果也写 stderr，是为了与其他控制命令的运维输出渠道保持一致。
            let _ = writeln!(
                std::io::stderr(),
                "No table has lost intermediate files according to given config"
            );
        } else {
            // 中文补充：这里只输出表名集合，不试图恢复文件详情，和 Go 控制命令的人类可读输出保持一致。
            let tableNames: Vec<String> = tables.unwrap().into_keys().collect();
            let _ = writeln!(
                std::io::stderr(),
                "These tables are missing intermediate files: {:?}",
                tableNames
            );
        }
        return Ok(());
    }

    // 中文补充：没有命中任何动作时退回 flag usage，这也是控制命令的默认“帮助”行为。
    fs.Usage();
    let _ = actions.usage_invoked.load(Ordering::SeqCst);
    Ok(())
}

/// Go `compactCluster`.
/// 中文补充：遍历所有状态不高于 `Offline` 的 store，并对每个 TiKV 触发 full-level compact。
pub fn compactCluster(ctx: &context::Context, cli: &PdClient, tls: &common::TLS) -> Result<()> {
    // 中文补充：`ForAllStores` 已经负责跳过状态超过 `Offline` 的节点，这里只关心对存活目标执行压缩。
    ForAllStores(ctx, cli, metapb::StoreState_Offline, |c, store| {
        Compact(c, tls, &store.Address, FullLevelCompact, "")
    })
}

/// Go `fetchMode`.
/// 中文补充：逐个 store 查询当前导入模式；单个节点失败只打印错误，不中断整个遍历流程。
pub fn fetchMode(ctx: &context::Context, cli: &PdClient, tls: &common::TLS) -> Result<()> {
    ForAllStores(ctx, cli, metapb::StoreState_Offline, |c, store| {
        match FetchMode(c, tls, &store.Address) {
            Err(err) => {
                // 中文补充：单点异常打印后继续后续节点，便于一次巡检看到全局状态。
                let _ = writeln!(std::io::stderr(), "{:<30} | Error: {}", store.Address, err);
            }
            Ok(mode) => {
                // 中文补充：正常分支沿用 Go 的表格样式输出，方便人工对齐多节点模式。
                let _ = writeln!(std::io::stderr(), "{:<30} | {} mode", store.Address, mode);
            }
        }
        Ok(())
    })
}
