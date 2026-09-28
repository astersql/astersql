// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! `br restore` 命令族，对齐 `br/cmd/br/restore.go`。
//! 该模块统一装配全量恢复、按库表恢复、raw/txn 恢复与 point restore，
//! 并在调用任务层前补齐日志、tracing、指标与若干全局开关。
//! 和 `backup.rs` 一样，CLI 层只负责参数解析与运行时环境准备，
//! 真正的数据恢复逻辑仍由 `astersql_br_pkg_task` 执行。

use std::sync::Arc;

use astersql_br_pkg_task::{
    Config, DBRestoreCmd, DefineDatabaseFlags, DefineFilterFlags, DefineRawRestoreFlags,
    DefineRestoreFlags, DefineRestoreSnapshotFlags, DefineStreamRestoreFlags, DefineTableFlags,
    FullBackupTypeEBS, FullRestoreCmd, IsStreamRestore, PointRestoreCmd, RawRestoreCmd,
    RestoreConfig, RestoreRawConfig, RunResolveKvData, RunRestore, RunRestoreEBSMetaWithDefaults,
    RunRestoreRaw, RunRestoreTxn, TableRestoreCmd, TxnRestoreCmd,
};

use crate::cmd::{
    GetDefaultContext, HasLogFile, Init, filterOutSysAndMemKeepAuthAndBind, log_arguments_for,
    setTiDBGlueDBFilter, tidbGlue,
};
use crate::stubs::berrors::{self, ErrorEqual};
use crate::stubs::*;

/// 执行逻辑恢复入口。
///
/// 该路径覆盖 `full`、`db`、`table` 与 `point` 四类命令，
/// 会先解析通用恢复参数，再按命令类型追加特定配置。
fn runRestoreCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = RestoreConfig {
        Config: Config {
            // 与 backup 路径保持一致：只有启用文件日志时才输出详细进度。
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
    // 指标注册依赖最终解析出的 PD/TLS/Keyspace，因此必须在 Parse 之后执行。
    metricsutil::RegisterMetricsForBR(&cfg.Config.PD, &cfg.Config.TLS, &cfg.Config.KeyspaceName)?;

    if IsStreamRestore(cmdName) {
        // point restore 需要额外解析日志恢复相关参数，普通 restore 不走这条分支。
        if let Err(err) = cfg.ParseStreamRestoreFlags(&flags) {
            return Err(Error::Trace(err.into()));
        }
    }

    // have to skip grant table, in order to NotifyUpdatePrivilege in binary mode
    // 恢复过程中需要跳过 grant table 校验，否则无法在二进制模式下刷新权限信息。
    config::GetGlobalConfig().Security_set_SkipGrantTable(true);

    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;

    if cfg.FullBackupType.0 == FullBackupTypeEBS {
        // EBS 恢复分成两条路径：prepare 只恢复元数据，否则继续解析并落地真实 KV 数据。
        return crate::cmd::with_tracing(enable, ctx, |_ctx| {
            if cfg.Prepare {
                // prepare 阶段只恢复 EBS 元信息，不直接回放数据文件。
                let g = TikvGlue;
                RunRestoreEBSMetaWithDefaults(&g, cmdName, &mut cfg).map_err(Error::from)
            } else {
                // 真正落数据时仍要借助 TiDB glue 解析任务环境和元数据依赖。
                let g = tidbGlue().lock().unwrap();
                let storage: std::sync::Arc<dyn astersql_br_pkg_task::stubs::Storage> =
                    std::sync::Arc::new(astersql_br_pkg_task::stubs::MemStorage::new());
                RunResolveKvData(g.as_task(), cmdName, &mut cfg, storage).map_err(Error::from)
            }
        })
        .map_err(|err| {
            if cfg.Prepare {
                log::Error("failed to restore EBS meta", &[zap::Error(&err)]);
            } else {
                log::Error("failed to restore data", &[zap::Error(&err)]);
            }
            Error::Trace(err)
        });
    }

    config::UpdateGlobal(|conf| {
        // 恢复进程不应被当成可承载 cop 任务的 TiDB 节点。
        conf.AdvertiseAddress = config::UnavailableIP.to_string();
        // copr cache 对一次性恢复收益有限，关闭可减少无谓内存占用。
        conf.TiKVClient.CoprCache.CapacityMB = 0.0;
    });

    // Go 版本同样会暂时关闭内存 tuner，因为 BR 看到的并非服务端完整内存视图。
    gctuner::GlobalMemoryLimitTuner.DisableAdjustMemoryLimit();
    let _reenable = {
        struct G;
        impl Drop for G {
            fn drop(&mut self) {
                gctuner::GlobalMemoryLimitTuner.EnableAdjustMemoryLimit();
            }
        }
        G
    };

    let mut restore_filter: Option<Box<dyn FnOnce() + Send>> = None;
    if !cfg.Config.Schemas.is_empty() {
        // 指定 schema 恢复时，需要把 TiDB glue 的可见库集合限制到目标库和系统库。
        let mut extraDBNames = Vec::with_capacity(cfg.Config.Schemas.len());
        for schema in &cfg.Config.Schemas {
            // 先去掉用户输入里的反引号，避免过滤器匹配到带引用符的错误库名。
            extraDBNames.push(utils::UnquoteName(schema));
        }
        let filter = FilterLoadSpecifiedDBAndSysDBs(extraDBNames);
        restore_filter = Some(setTiDBGlueDBFilter(filter));
    }

    // 真实恢复执行保持在 tracing 包裹下，便于沿用统一链路观测行为。
    let result = crate::cmd::with_tracing(enable, GetDefaultContext(), |_ctx| {
        let g = tidbGlue().lock().unwrap();
        RunRestore(g.as_task(), cmdName, &mut cfg).map_err(Error::from)
    });
    if let Some(r) = restore_filter {
        // 无论恢复成功与否都要恢复过滤器，避免污染后续命令路径。
        r();
    }
    if let Err(err) = result {
        log::Error("failed to restore", &[zap::Error(&err)]);
        printWorkaroundOnFullRestoreError(&err);
        return Err(Error::Trace(err));
    }
    Ok(())
}

/// print workaround when we met not fresh or incompatible cluster error on full cluster restore
///
/// 中文补充：这里只在两类用户可操作的典型错误上打印补救建议，
/// 避免普通错误也输出误导性的 workaround。
pub fn printWorkaroundOnFullRestoreError(err: &Error) {
    if !ErrorEqual(err, berrors::ErrRestoreNotFreshCluster)
        && !ErrorEqual(err, berrors::ErrRestoreIncompatibleSys)
    {
        return;
    }
    println!("#######################################################################");
    if ErrorEqual(err, berrors::ErrRestoreNotFreshCluster) {
        // 这类错误通常意味着目标集群残留旧对象，用户需要先清理再重试。
        println!("# the target cluster is not fresh, cannot restore.");
        println!("# you can drop existing databases and tables and start restore again");
    } else if ErrorEqual(err, berrors::ErrRestoreIncompatibleSys) {
        // 系统表不兼容时，给出最直接的降级恢复建议，避免整次恢复被系统表阻塞。
        println!("# the target cluster is not compatible with the backup data,");
        println!("# you can use '--with-sys-table=false' to skip restoring system tables");
    }
    println!("#######################################################################");
}

/// 执行 raw kv 恢复入口。
///
/// 这条路径绕过 SQL 层，直接按 key range 写回 TiKV，
/// 因此使用 `TikvGlue` 而不是 TiDB glue。
fn runRestoreRawCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = RestoreRawConfig {
        RawKvConfig: astersql_br_pkg_task::RawKvConfig {
            Config: Config {
                LogProgress: HasLogFile(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    if let Err(err) = cfg.ParseFromFlags(&flags) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }
    let ctx = GetDefaultContext();
    let enable = cfg.RawKvConfig.Config.EnableOpenTracing;
    crate::cmd::with_tracing(enable, ctx, |_ctx| {
        let g = TikvGlue;
        // raw restore 不依赖 schema 过滤器，CLI 层只需把配置透传给任务执行器。
        RunRestoreRaw(&g, cmdName, &mut cfg).map_err(Error::from)
    })
    .map_err(|err| {
        log::Error("failed to restore raw kv", &[zap::Error(&err)]);
        Error::Trace(err)
    })
}

/// 执行 txn kv 恢复入口。
///
/// 与 raw restore 类似都直连 TiKV，但配置类型和任务常量不同，
/// 因而单独保留函数以维持 Go 版本的命令布局。
fn runRestoreTxnCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut cfg = Config {
        LogProgress: HasLogFile(),
        ..Default::default()
    };
    let flags = effective_task_flags(command);
    if let Err(err) = cfg.ParseFromFlags(&flags) {
        command.SilenceUsage = false;
        return Err(Error::Trace(err.into()));
    }
    let ctx = GetDefaultContext();
    let enable = cfg.EnableOpenTracing;
    crate::cmd::with_tracing(enable, ctx, |_ctx| {
        let g = TikvGlue;
        RunRestoreTxn(&g, cmdName, &mut cfg).map_err(Error::from)
    })
    .map_err(|err| {
        log::Error("failed to restore txn kv", &[zap::Error(&err)]);
        Error::Trace(err)
    })
}

/// NewRestoreCommand returns a restore subcommand.
///
/// 中文补充：这是 `br restore` 顶级命令装配入口，
/// 负责注册公共初始化、恢复通用 flag 与全部一级子命令。
pub fn NewRestoreCommand() -> Command {
    let mut command = Command {
        Use: "restore".into(),
        Short: "restore a TiDB/TiKV cluster".into(),
        SilenceUsage: true,
        ..Default::default()
    };
    command.PersistentPreRunE = Some(Arc::new(|c, _args| {
        // 初始化顺序与 Go 版本保持一致，避免日志、参数审计与摘要统计漂移。
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        // 恢复期间不需要统计 worker，同时需要放宽事务总大小限制以承载大批量写入。
        session::DisableStats4Test();
        kv::TxnTotalSizeLimit.store(
            config::SuperLargeTxnSize,
            std::sync::atomic::Ordering::SeqCst,
        );
        summary::SetUnit(summary::RestoreUnit);
        Ok(())
    }));
    command.AddCommand(vec![
        newFullRestoreCommand(),
        newDBRestoreCommand(),
        newTableRestoreCommand(),
        newRawRestoreCommand(),
        newTxnRestoreCommand(),
        newStreamRestoreCommand(),
    ]);
    DefineRestoreFlags(command.PersistentFlags());
    command
}

/// 构造 `restore full` 子命令。
///
/// 默认过滤器会排除大部分系统库，但保留权限相关表，
/// 对齐 Go 版全量恢复的默认对象集合。
fn newFullRestoreCommand() -> Command {
    let mut command = Command {
        Use: "full".into(),
        Short: "restore all tables".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runRestoreCommand(cmd, FullRestoreCmd)));
    DefineFilterFlags(command.Flags(), filterOutSysAndMemKeepAuthAndBind(), false);
    DefineRestoreSnapshotFlags(command.Flags());
    command
}

/// 构造 `restore db` 子命令。
///
/// 该命令把恢复目标限定为单个数据库，具体库名由数据库 flag 决定。
fn newDBRestoreCommand() -> Command {
    let mut command = Command {
        Use: "db".into(),
        Short: "restore tables in a database from the backup data".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runRestoreCommand(cmd, DBRestoreCmd)));
    DefineDatabaseFlags(command.Flags());
    command
}

/// 构造 `restore table` 子命令。
///
/// 表级恢复仍复用统一的逻辑恢复执行器，只是在 flag 层收敛到单表目标。
fn newTableRestoreCommand() -> Command {
    let mut command = Command {
        Use: "table".into(),
        Short: "restore a table from the backup data".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runRestoreCommand(cmd, TableRestoreCmd)));
    DefineTableFlags(command.Flags());
    command
}

/// 构造 `restore raw` 子命令。
///
/// raw kv 恢复仍标记为 experimental，因为调用者需要自行保证 key 编码与业务兼容。
fn newRawRestoreCommand() -> Command {
    let mut command = Command {
        Use: "raw".into(),
        Short: "(experimental) restore a raw kv range to TiKV cluster".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runRestoreRawCommand(cmd, RawRestoreCmd)));
    DefineRawRestoreFlags(command.Flags());
    command
}

/// 构造 `restore txn` 子命令。
///
/// 事务 kv 恢复与 raw restore 同样走 TiKV 直连，但处理的是事务编码数据。
fn newTxnRestoreCommand() -> Command {
    let mut command = Command {
        Use: "txn".into(),
        Short: "(experimental) restore txn kv to TiKV cluster".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| runRestoreTxnCommand(cmd, TxnRestoreCmd)));
    DefineRawRestoreFlags(command.Flags());
    command
}

/// 构造 `restore point` 子命令。
///
/// point restore 依赖日志备份把数据恢复到指定 commit ts，
/// 因此额外挂载 stream restore 专用参数。
fn newStreamRestoreCommand() -> Command {
    let mut command = Command {
        Use: "point".into(),
        Short: "restore data from log until specify commit timestamp".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| {
        // point restore 最终仍复用统一恢复入口，只是命令常量和 flag 语义不同。
        runRestoreCommand(command, PointRestoreCmd)
    }));
    DefineFilterFlags(command.Flags(), filterOutSysAndMemKeepAuthAndBind(), true);
    DefineStreamRestoreFlags(command.Flags());
    command
}
