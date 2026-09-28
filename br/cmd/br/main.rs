// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! BR CLI 根入口，对齐 `br/cmd/br/main.go`。
//! 该模块负责组装顶层 `br` 命令、接入统一公共 flag、
//! 注册所有一级子命令，并把进程级退出信号转成可传播的默认上下文。
//! 这里的重点不是备份/恢复实现本身，而是保证 Rust 版启动顺序
//! 与 Go 版主函数保持一致，减少命令行为偏差。

use crate::abort::NewAbortCommand;
use crate::backup::NewBackupCommand;
use crate::cmd::{DefineCommonFlags, SetDefaultContext};
use crate::debug::NewDebugCommand;
use crate::operator::newOperatorCommand;
use crate::restore::NewRestoreCommand;
use crate::stream::NewStreamCommand;
use crate::stubs::os_stub::{self as os};
use crate::stubs::*;

/// Process entry matching Go `main`.
///
/// 中文补充：根入口只做命令树装配和执行，不直接承载具体业务子命令逻辑。
pub fn main() {
    let gCtx = Context::Background();
    // 退出监听器会把终止信号转换成可取消上下文，供所有下游命令共享。
    let (ctx, cancel) = utils::StartExitSingleListener(gCtx);
    // defer cancel() — runs on normal return; os.Exit skips defer in Go.
    // 用 Drop 守卫模拟 Go 的 `defer cancel()`，同时保留 `os.Exit` 路径不执行 defer 的特性。
    let _cancel_on_return = CancelOnDrop(Some(cancel));

    let mut rootCmd = Command {
        Use: "br".into(),
        Short: "br is a TiDB/TiKV cluster backup restore tool.".into(),
        TraverseChildren: true,
        SilenceUsage: true,
        ..Default::default()
    };
    DefineCommonFlags(&mut rootCmd);
    // 顶层上下文在这里注入，后续各子命令通过公共 helper 读取同一份默认值。
    SetDefaultContext(ctx);

    // BR 是离线工具，不应在进程内触发 TiDB DDL 相关后台行为。
    config::GetGlobalConfig()
        .Instance()
        .TiDBEnableDDL_Store(false);

    // 一级命令布局保持与 Go 版本一致，方便用户与自动化脚本复用既有调用方式。
    rootCmd.AddCommand(vec![
        NewDebugCommand(),
        NewBackupCommand(),
        NewRestoreCommand(),
        NewStreamCommand(),
        newOperatorCommand(),
        NewAbortCommand(),
    ]);
    rootCmd.SetOut(os::Stdout());

    let args = os::Args();
    // 跳过 argv[0]，只把用户参数交给命令解析器。
    let user_args = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        Vec::new()
    };
    rootCmd.SetArgs(user_args);

    if let Err(err) = rootCmd.Execute() {
        log::Error("br failed", &[zap::Error(&err)]);
        // Drop cancel guard before Exit to mirror Go skipping defer on os.Exit.
        // 这里故意泄漏守卫，避免 `Exit(1)` 前触发取消回调，和 Go 的 `os.Exit` 语义对齐。
        std::mem::forget(_cancel_on_return);
        os::Exit(1);
    }
}

/// 用于在普通返回路径上执行取消回调的轻量守卫。
///
/// 单独抽成类型后，入口函数既能表达“默认 defer cancel”，
/// 又能在错误退出路径上显式绕过它。
struct CancelOnDrop(Option<Box<dyn Fn() + Send>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            c();
        }
    }
}
