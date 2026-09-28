// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! `br abort` command family — mirrors `br/cmd/br/abort.go`.
//!
//! BR（Backup & Restore）中断恢复任务的 CLI 入口：挂载 `abort` 子命令树，
//! 将用户标志解析为 `RestoreConfig`，再委托 `RunRestoreAbort` 执行真正的中止逻辑。
//! 命令树与 Go 版一致：`abort → restore → {full,db,table,point}`；
//! 备份侧 abort 尚未接入（Go 中仍以注释占位 `newAbortBackupCommand`）。
//! 本文件只负责 cobra 式命令装配与公共前置（日志、统计关闭、tracing），
//! 不实现恢复状态机本身。

use std::sync::Arc;

use astersql_br_pkg_task::{
    DBRestoreCmd, DefineDatabaseFlags, DefineFilterFlags, DefineRestoreFlags,
    DefineRestoreSnapshotFlags, DefineStreamRestoreFlags, DefineTableFlags, FullRestoreCmd,
    IsStreamRestore, PointRestoreCmd, RestoreConfig, RunRestoreAbort, TableRestoreCmd,
};

use crate::cmd::{
    GetDefaultContext, HasLogFile, Init, filterOutSysAndMemKeepAuthAndBind, log_arguments_for,
    tidbGlue,
};
use crate::stubs::*;

/// NewAbortCommand returns an abort subcommand.
///
/// 构造顶层 `br abort`：注册持久化 PreRun（初始化/日志/禁统计）与 restore 子树，
/// 并在 PersistentFlags 上挂载通用恢复标志，供所有叶子命令继承。
pub fn NewAbortCommand() -> Command {
    let mut command = Command {
        Use: "abort".into(),
        Short: "abort restore tasks".into(),
        // 默认静默 usage；解析失败时由 runAbortRestoreCommand 再打开。
        SilenceUsage: true,
        ..Default::default()
    };
    // 与 Go PersistentPreRunE 对齐：子命令执行前统一初始化环境。
    command.PersistentPreRunE = Some(Arc::new(|c, _args| {
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        // 禁统计避免 abort 路径占用过多内存（Go 同注释）。
        session::DisableStats4Test();
        Ok(())
    }));

    // 当前仅挂 restore；备份 abort 与 Go 一样仍为 future 扩展点。
    command.AddCommand(vec![newAbortRestoreCommand()]);
    DefineRestoreFlags(command.PersistentFlags());
    command
}

/// 装配 `abort restore` 中间层，挂载 full/db/table/point 四类中止入口。
fn newAbortRestoreCommand() -> Command {
    let mut command = Command {
        Use: "restore".into(),
        Short: "abort restore tasks".into(),
        SilenceUsage: true,
        ..Default::default()
    };
    command.AddCommand(vec![
        newAbortRestoreFullCommand(),
        newAbortRestoreDBCommand(),
        newAbortRestoreTableCommand(),
        newAbortRestorePointCommand(),
    ]);
    command
}

/// `abort restore full`：中止全量恢复；过滤默认排除系统/内存表，保留 auth/bind。
fn newAbortRestoreFullCommand() -> Command {
    let mut command = Command {
        Use: "full".into(),
        Short: "abort a full restore task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| {
        runAbortRestoreCommand(cmd, FullRestoreCmd)
    }));
    // false：非 stream；与 Go DefineFilterFlags(..., false) 一致。
    DefineFilterFlags(command.Flags(), filterOutSysAndMemKeepAuthAndBind(), false);
    DefineRestoreSnapshotFlags(command.Flags());
    command
}

/// `abort restore db`：按库粒度中止；需 DefineDatabaseFlags 指定目标库。
fn newAbortRestoreDBCommand() -> Command {
    let mut command = Command {
        Use: "db".into(),
        Short: "abort a database restore task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runAbortRestoreCommand(cmd, DBRestoreCmd)));
    DefineDatabaseFlags(command.Flags());
    command
}

/// `abort restore table`：按表粒度中止；标志由 DefineTableFlags 注入。
fn newAbortRestoreTableCommand() -> Command {
    let mut command = Command {
        Use: "table".into(),
        Short: "abort a table restore task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| {
        runAbortRestoreCommand(cmd, TableRestoreCmd)
    }));
    DefineTableFlags(command.Flags());
    command
}

/// `abort restore point`：中止 PITR/流式恢复；过滤开启 stream 语义并定义流标志。
fn newAbortRestorePointCommand() -> Command {
    let mut command = Command {
        Use: "point".into(),
        Short: "abort a point-in-time restore task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| {
        runAbortRestoreCommand(cmd, PointRestoreCmd)
    }));
    // true：stream restore 过滤路径；随后再解析流专有标志。
    DefineFilterFlags(command.Flags(), filterOutSysAndMemKeepAuthAndBind(), true);
    DefineStreamRestoreFlags(command.Flags());
    command
}

/// 叶子命令公共执行路径：解析配置 → 可选流标志 → tracing 包裹 → RunRestoreAbort。
///
/// `cmdName` 区分 Full/DB/Table/Point，决定是否走 `IsStreamRestore` 分支；
/// 与 Go `runAbortRestoreCommand` 数据流一致，Rust 侧通过 `with_tracing` 等价
/// 于 Go 的 TracerStartSpan/FinishSpan。
fn runAbortRestoreCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    // LogProgress 跟随是否配置了日志文件，避免无文件时刷进度干扰。
    let mut cfg = RestoreConfig {
        Config: astersql_br_pkg_task::Config {
            LogProgress: HasLogFile(),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    // 解析失败时打开 SilenceUsage，让用户看到标志帮助（Go 同行为）。
    if let Err(err) = cfg.ParseFromFlags(&flags, false) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }

    // 仅 point（流式）需要二次解析 stream 专有标志。
    if IsStreamRestore(cmdName) {
        if let Err(err) = cfg.ParseStreamRestoreFlags(&flags) {
            return Err(Error::Trace(err.into()));
        }
    }

    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;
    let glue = tidbGlue();
    let g = glue.lock().unwrap();
    // enable 为真时开启 OpenTracing；task glue 持锁期间调用中止实现。
    let result = crate::cmd::with_tracing(enable, ctx, |_ctx| {
        RunRestoreAbort(g.as_task(), cmdName, &mut cfg).map_err(Error::from)
    });
    if let Err(err) = result {
        log::Error("failed to abort restore task", &[zap::Error(&err)]);
        return Err(Error::Trace(err));
    }
    Ok(())
}
