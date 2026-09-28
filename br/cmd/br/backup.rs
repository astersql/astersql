// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! `br backup` 命令族，对齐 `br/cmd/br/backup.go` 的入口组织方式。
//! 这里不承载真正的备份实现，而是负责把 Cobra 风格子命令组装成 Rust 侧命令树，
//! 再把解析后的参数转交给 `astersql_br_pkg_task` 中的任务执行层。
//! 模块还负责在命令入口补齐运行时环境，例如 tracing、指标注册、TiDB glue 过滤器
//! 以及与 Go 版本保持一致的内存调优开关。

use std::sync::Arc;

use astersql_br_pkg_task::stubs::{MemStorage, Storage};
use astersql_br_pkg_task::{
    BackupConfig, DBBackupCmd, DefineBackupEBSFlags, DefineBackupFlags, DefineDatabaseFlags,
    DefineFilterFlags, DefineRawBackupFlags, DefineTableFlags, DefineTxnBackupFlags, FullBackupCmd,
    FullBackupTypeEBS, RawBackupCmd, RawKvConfig, RunBackupEBS, RunBackupRawWithDefaults,
    RunBackupTxnWithDefaults, RunBackupWithDefaults, TableBackupCmd, TxnBackupCmd, TxnKvConfig,
};

use crate::cmd::{
    GetDefaultContext, HasLogFile, Init, acceptAllTables, log_arguments_for, setTiDBGlueDBFilter,
    tidbGlue,
};
use crate::stubs::*;

/// 执行逻辑备份入口。
///
/// 该路径服务于 `full`、`db`、`table` 三类命令，统一解析共享备份参数，
/// 并在调用任务层前补齐 Go 实现里同样存在的运行时保护。
/// 一旦参数解析失败，会显式打开 `SilenceUsage = false`，让 CLI 把用法输出给用户。
fn runBackupCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = BackupConfig {
        Config: astersql_br_pkg_task::Config {
            // 只有在用户显式配置日志文件时才输出进度，避免默认场景下额外刷屏。
            LogProgress: HasLogFile(),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    if let Err(err) = cfg.ParseFromFlags(&flags, false) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }
    // 指标注册依赖最终解析出的 PD/TLS/Keyspace 配置，因此必须在 Parse 之后执行。
    metricsutil::RegisterMetricsForBR(&cfg.Config.PD, &cfg.Config.TLS, &cfg.Config.KeyspaceName)?;

    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;

    if cfg.FullBackupType.0 == FullBackupTypeEBS {
        // EBS 快照备份走独立执行器，不需要后面的 TiDB coprocessor 与 GC tuner 调整。
        return crate::cmd::with_tracing(enable, ctx, |_ctx| {
            let g = tidbGlue().lock().unwrap();
            // Rust 迁移阶段用内存存储桩承接外部依赖，保持调用链形状与 Go 版本一致。
            let storage: Arc<dyn Storage> = Arc::new(MemStorage::new());
            RunBackupEBS(g.as_task(), &mut cfg, storage).map_err(Error::from)
        })
        .map_err(|err| {
            log::Error("failed to backup", &[zap::Error(&err)]);
            Error::Trace(err)
        });
    }

    let result = crate::cmd::with_tracing(enable, ctx, |_ctx| {
        config::UpdateGlobal(|conf| {
            // 备份过程中不应把本进程当作可被访问的 TiDB 节点，从而避免触发错误的 cop 任务路由。
            conf.AdvertiseAddress = config::UnavailableIP.to_string();
            // BR 是批处理命令，copp cache 只会额外占用内存，不像长活服务那样能摊薄成本。
            conf.TiKVClient.CoprCache.CapacityMB = 0.0;
        });

        // Go 版本会临时关闭全局内存限制调优器，因为 BR 进程拿不到 TiDB 节点那套可信内存视图。
        gctuner::GlobalMemoryLimitTuner.DisableAdjustMemoryLimit();
        let _reenable = scopeguard_enable_gctuner();

        // 两个守卫的逆序析构与 Go defer 一致：先恢复过滤器，再恢复 GC tuner，最后结束 tracing。
        let _restore_filter =
            scopeguard_restore_filter(setTiDBGlueDBFilter(Arc::new(FilterLoadSysDBs)));
        let g = tidbGlue().lock().unwrap();
        RunBackupWithDefaults(g.as_task(), cmdName, &mut cfg).map_err(Error::from)
    });
    if let Err(err) = result {
        log::Error("failed to backup", &[zap::Error(&err)]);
        return Err(Error::Trace(err));
    }
    Ok(())
}

/// 返回一个离开作用域时自动重新启用 GC tuner 的守卫。
///
/// 这里拆成独立辅助函数，是为了让调用端清楚表达“先禁用、后自动恢复”的配对关系，
/// 同时避免遗漏恢复逻辑导致全局配置泄漏到其他命令路径。
fn scopeguard_enable_gctuner() -> impl Drop {
    struct G;
    impl Drop for G {
        fn drop(&mut self) {
            gctuner::GlobalMemoryLimitTuner.EnableAdjustMemoryLimit();
        }
    }
    G
}

/// 将 TiDB glue 过滤器恢复动作转成 unwind-safe 的作用域守卫，对齐 Go `defer`。
pub(super) fn scopeguard_restore_filter(restore: Box<dyn FnOnce() + Send>) -> impl Drop {
    struct G(Option<Box<dyn FnOnce() + Send>>);
    impl Drop for G {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                restore();
            }
        }
    }
    G(Some(restore))
}

/// 执行 raw kv 备份入口。
///
/// 与逻辑备份不同，这条路径直接使用 `TikvGlue` 访问 TiKV，
/// 因此不需要 TiDB glue 过滤器和与 SQL 层相关的全局配置调整。
fn runBackupRawCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = RawKvConfig {
        Config: astersql_br_pkg_task::Config {
            // raw/txn 命令与普通 backup 保持相同的进度日志开关行为，方便统一脚本接入。
            LogProgress: HasLogFile(),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    if let Err(err) = cfg.ParseBackupConfigFromFlags(&flags) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }
    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;
    crate::cmd::with_tracing(enable, ctx, |_ctx| {
        // 这里使用无状态的 TiKV glue 值对象，对齐 Go 中 `gluetikv.Glue{}` 的轻量调用方式。
        let g = TikvGlue;
        RunBackupRawWithDefaults(&g, cmdName, &mut cfg).map_err(Error::from)
    })
    .map_err(|err| {
        log::Error("failed to backup raw kv", &[zap::Error(&err)]);
        Error::Trace(err)
    })
}

/// 执行 txn kv 备份入口。
///
/// 事务 kv 备份与 raw kv 共享直连 TiKV 的运行方式，但配置解析和任务常量不同，
/// 所以单独保留函数以匹配 Go 版本命令布局和错误日志文案。
fn runBackupTxnCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = TxnKvConfig {
        Config: astersql_br_pkg_task::Config {
            LogProgress: HasLogFile(),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    if let Err(err) = cfg.ParseBackupConfigFromFlags(&flags) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }
    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;
    crate::cmd::with_tracing(enable, ctx, |_ctx| {
        let g = TikvGlue;
        RunBackupTxnWithDefaults(&g, cmdName, &mut cfg).map_err(Error::from)
    })
    .map_err(|err| {
        log::Error("failed to backup txn kv", &[zap::Error(&err)]);
        Error::Trace(err)
    })
}

/// NewBackupCommand return a full backup subcommand.
///
/// 中文补充：这是 `br backup` 顶级命令的装配入口，
/// 负责注册公共前置初始化和所有子命令，但不直接执行任何备份任务。
pub fn NewBackupCommand() -> Command {
    let mut command = Command {
        Use: "backup".into(),
        Short: "backup a TiDB/TiKV cluster".into(),
        SilenceUsage: true,
        ..Default::default()
    };
    command.PersistentPreRunE = Some(Arc::new(|c, _args| {
        // 初始化顺序保持与 Go 版本一致，避免日志、参数打印和摘要统计出现语义偏差。
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        // BR 不需要统计 worker，关闭后可减少和 TiDB 服务端语义不同的后台活动。
        session::DisableStats4Test();
        summary::SetUnit(summary::BackupUnit);
        Ok(())
    }));
    command.AddCommand(vec![
        newFullBackupCommand(),
        newDBBackupCommand(),
        newTableBackupCommand(),
        newRawBackupCommand(),
        newTxnBackupCommand(),
    ]);
    DefineBackupFlags(command.PersistentFlags());
    command
}

/// 构造 `backup full` 子命令。
///
/// 空数据库/表过滤条件在任务层会被解释为“整集群逻辑备份”，
/// 因此这里额外挂载过滤器默认值与 EBS 相关开关。
fn newFullBackupCommand() -> Command {
    let mut command = Command {
        Use: "full".into(),
        Short: "backup all database".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        // `FullBackupCmd` 是任务层分支选择键，CLI 只负责原样透传。
        runBackupCommand(command, FullBackupCmd)
    }));
    DefineFilterFlags(command.Flags(), acceptAllTables(), false);
    DefineBackupEBSFlags(command.PersistentFlags());
    command
}

/// 构造 `backup db` 子命令。
///
/// 该命令限制为单个数据库维度备份，具体库名与过滤逻辑由 `DefineDatabaseFlags` 注入。
fn newDBBackupCommand() -> Command {
    let mut command = Command {
        Use: "db".into(),
        Short: "backup a database".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        runBackupCommand(command, DBBackupCmd)
    }));
    DefineDatabaseFlags(command.Flags());
    command
}

/// 构造 `backup table` 子命令。
///
/// 这里仍复用统一的逻辑备份执行器，只是在 flag 层把用户输入收敛到表级目标。
fn newTableBackupCommand() -> Command {
    let mut command = Command {
        Use: "table".into(),
        Short: "backup a table".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        runBackupCommand(command, TableBackupCmd)
    }));
    DefineTableFlags(command.Flags());
    command
}

/// 构造 `backup raw` 子命令。
///
/// raw kv 仍标记为 experimental，是因为它绕过 SQL 语义直接操作 TiKV key range，
/// 调用者需要自己保证 key 编码和下游恢复流程与业务数据兼容。
fn newRawBackupCommand() -> Command {
    let mut command = Command {
        Use: "raw".into(),
        Short: "(experimental) backup a raw kv range from TiKV cluster".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        runBackupRawCommand(command, RawBackupCmd)
    }));
    DefineRawBackupFlags(command.Flags());
    command
}

/// 构造 `backup txn` 子命令。
///
/// 它面向事务 kv 范围备份，保留独立子命令是为了让 CLI 帮助信息
/// 和任务层命令常量与 Go 版本一一对应，降低迁移对使用者的认知差异。
fn newTxnBackupCommand() -> Command {
    let mut command = Command {
        Use: "txn".into(),
        Short: "(experimental) backup a txn kv range from TiKV cluster".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        runBackupTxnCommand(command, TxnBackupCmd)
    }));
    DefineTxnBackupFlags(command.Flags());
    command
}
