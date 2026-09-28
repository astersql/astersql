// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc. Licensed under Apache-2.0.

//! 隐藏的 `br operator` 命令集合，对齐 `br/cmd/br/operator.go`。
//! 这一组命令主要面向运维或控制面组件，而不是普通备份用户：
//! 它们负责环境预处理、迁移管理、日志备份强制刷新、外部存储探测
//! 以及若干校验类工具，因此默认都挂在隐藏子命令下。
//! Rust 版延续 Go 的命令布局，把真正的业务逻辑继续委托给
//! `astersql_br_pkg_task_operator`，CLI 层只做参数装配和上下文衔接。

use std::sync::{Arc, Mutex};

use astersql_br_pkg_task::DefineFilterFlags;
use astersql_br_pkg_task_operator::{
    AdaptEnvForSnapshotBackup, Base64ify, Base64ifyConfig, CRRCheckpointConfig,
    ChecksumUpstreamConfig, ChecksumWithPitrIdMapConfig, ChecksumWithRewriteRulesConfig,
    DefineFlagsForBase64ifyConfig, DefineFlagsForCRRCheckpointConfig,
    DefineFlagsForChecksumPitrTableConfig, DefineFlagsForChecksumTableConfig,
    DefineFlagsForChecksumUpstreamTableConfig, DefineFlagsForForceFlushConfig,
    DefineFlagsForListMigrationConfig, DefineFlagsForMigrateToConfig,
    DefineFlagsForPrepareSnapBackup, DefineFlagsForTestStorageConfig, ForceFlushConfig,
    ListMigrationConfig, MigrateToConfig, NewCRRCheckpointService, PauseGcConfig, RunChecksumTable,
    RunForceFlush, RunListMigrations, RunMigrateTo, RunPitrChecksumTable, RunTestStorage,
    RunUpstreamChecksumTable, TestStorageConfig, cleanupFunc,
};

use crate::cmd::{
    GetDefaultContext, Init, log_arguments_for, registerStatusServerPreparer, tidbGlue,
};
use crate::stubs::*;

const CRR_STATE_KEY: u64 = 0x0C_11_01;

/// 保存 CRR checkpoint 服务的运行态。
///
/// 状态对象同时承载服务实例、注册后的清理函数和是否执行过的标记，
/// 让“准备 status server”与“真正运行服务”可以跨两个回调共享同一份状态。
struct CrrCheckpointServiceState {
    service: astersql_br_pkg_task_operator::stubs::CRRService,
    cleanup: Mutex<Option<cleanupFunc>>,
    ran: std::sync::atomic::AtomicBool,
}

impl CrrCheckpointServiceState {
    /// 运行 checkpoint 服务主体。
    ///
    /// 真正的 slim stub 不会像 Go 版那样阻塞在长活服务中，
    /// 这里只保留最关键的调用时序与输入校验，供 parity 测试验证。
    fn Run(&self) -> Result<()> {
        self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
        // Go blocks in service.Run; stub records invocation for parity tests.
        if self.service.cfg.TaskName.is_empty() {
            return Err(Error::new("empty task name"));
        }
        Ok(())
    }
    /// 把服务暴露的状态接口挂到公共 status mux。
    ///
    /// CLI 层不关心 handler 细节，只保证注册时机和路径存在性与 Go 对齐。
    fn Register(&self, mux: &mut ServeMux) {
        mux.Handle("/crr/status");
        let _ = &self.service;
    }
}

/// 构造隐藏的 `br operator` 顶级命令。
///
/// 所有子命令都共享公共初始化，但服务的对象是运维流程而非最终用户，
/// 因此顶层命令本身保持 `Hidden = true`。
pub fn newOperatorCommand() -> Command {
    let mut cmd = Command {
        Use: "operator <subcommand>".into(),
        Short: "utilities for operators like tidb-operator.".into(),
        Hidden: true,
        ..Default::default()
    };
    cmd.PersistentPreRunE = Some(Arc::new(|c, _args| {
        // 与 backup/restore 等主命令共用初始化序列，保证日志和参数审计一致。
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        Ok(())
    }));
    cmd.AddCommand(vec![
        newPrepareForSnapshotBackupCommand(
            "pause-gc-and-schedulers",
            "(Will be replaced with `prepare-for-snapshot-backup`) pause gc, schedulers and importing until the program exits.",
        ),
        newPrepareForSnapshotBackupCommand(
            "prepare-for-snapshot-backup",
            "pause gc, schedulers and importing until the program exits, for snapshot backup.",
        ),
        newBase64ifyCommand(),
        newListMigrationsCommand(),
        newMigrateToCommand(),
        newForceFlushCommand(),
        newCRRCheckpointCommand(),
        newChecksumCommand(),
        newTestStorageCommand(),
        newPitrChecksumCommand(),
        newUpstreamChecksumCommand(),
    ]);
    cmd
}

/// 构造“为快照备份暂停 GC/调度器”的命令。
///
/// 这里同时服务旧命令名 `pause-gc-and-schedulers` 与新命令名
/// `prepare-for-snapshot-backup`，以兼容已有脚本。
fn newPrepareForSnapshotBackupCommand(use_: &str, short: &str) -> Command {
    let mut cmd = Command {
        Use: use_.into(),
        Short: short.into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = PauseGcConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let ctx = GetDefaultContext();
        // 真正的暂停逻辑由 operator 层处理，CLI 这里只负责把解析后的配置转发下去。
        AdaptEnvForSnapshotBackup(
            astersql_br_pkg_task_operator::stubs::Context::FromCancellationFlag(
                ctx.cancelled.clone(),
            ),
            cfg,
        )
        .map_err(Error::from)
    }));
    DefineFlagsForPrepareSnapBackup(cmd.OpFlags());
    cmd
}

/// 构造 `base64ify` 子命令。
///
/// 该工具把存储配置编码成 base64，方便传递给 `tikv-ctl compact-log-backup`
/// 等只接受单字符串输入的运维命令。
fn newBase64ifyCommand() -> Command {
    let mut cmd = Command {
        Use: "base64ify [-r] -s <storage>".into(),
        Short:
            "generate base64 for a storage. this may be passed to `tikv-ctl compact-log-backup`."
                .into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = Base64ifyConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let ctx = GetDefaultContext();
        Base64ify(
            astersql_br_pkg_task_operator::stubs::Context::FromCancellationFlag(
                ctx.cancelled.clone(),
            ),
            cfg,
        )
        .map_err(Error::from)
    }));
    DefineFlagsForBase64ifyConfig(cmd.OpFlags());
    cmd
}

/// 构造“列出全部迁移记录”的命令。
///
/// 该命令只读查询迁移元信息，不修改集群状态，主要用于人工核对迁移历史。
fn newListMigrationsCommand() -> Command {
    let mut cmd = Command {
        Use: "list-migrations".into(),
        Short: "list all migrations".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = ListMigrationConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        // 保持单纯查询语义，避免在 CLI 层混入额外过滤或排序逻辑。
        RunListMigrations(cfg).map_err(Error::from)
    }));
    DefineFlagsForListMigrationConfig(cmd.OpFlags());
    cmd
}

/// 构造隐藏的 `unsafe-migrate-to` 命令。
///
/// 该命令可强制把状态迁移到指定版本，危险性很高，
/// 因此默认隐藏并在帮助信息中直接警告不要误用。
fn newMigrateToCommand() -> Command {
    let mut cmd = Command {
        Use: "unsafe-migrate-to".into(),
        Short: "migrate to a specific version, use truncate will auto migrate to correct version, you should never use this command unless you know what you are doing".into(),
        no_args: true,
        Hidden: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = MigrateToConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        // 迁移动作必须完全交给 operator 层，CLI 不能擅自补救或自动回退。
        RunMigrateTo(cfg).map_err(Error::from)
    }));
    DefineFlagsForMigrateToConfig(cmd.OpFlags());
    cmd
}

/// 构造 `checksum-as` 命令。
///
/// 它通过备份生成的 rewrite rule 把当前集群视角映射回上游 key 空间，
/// 用于在恢复后核对数据一致性。
fn newChecksumCommand() -> Command {
    let mut cmd = Command {
        Use: "checksum-as".into(),
        Short: "calculate the checksum with rewrite rules".into(),
        Long: "Calculate the checksum of the current cluster (specified by `-u`) with applying the rewrite rules generated from a backup (specified by `-s`). This can be used when you have the checksum of upstream elsewhere.".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = ChecksumWithRewriteRulesConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        let g = tidbGlue().lock().unwrap();
        // 这里必须拿到 TiDB glue，因为 checksum 需要访问当前集群元信息和执行环境。
        RunChecksumTable(g.as_op(), cfg).map_err(Error::from)
    }));
    // 默认过滤规则设为 `!*.*`，强制调用者显式指定对象范围，避免误扫全库。
    DefineFilterFlags(cmd.Flags(), vec!["!*.*".into()], false);
    DefineFlagsForChecksumTableConfig(cmd.OpFlags());
    cmd
}

/// 构造 `checksum-pitr` 命令。
///
/// 与普通 rewrite-rule checksum 不同，这条路径改用 PITR 生成的 id map
/// 来恢复对象对应关系，适合日志恢复后的校验场景。
fn newPitrChecksumCommand() -> Command {
    let mut cmd = Command {
        Use: "checksum-pitr".into(),
        Short: "calculate the checksum with pitr id map".into(),
        Long: "Calculate the checksum of the current cluster (specified by `-u`) with applying the rewrite rules generated from pitr id map (specified by `-s` if saved in external storage). This can be used when you have the checksum of upstream elsewhere.".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = ChecksumWithPitrIdMapConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        let g = tidbGlue().lock().unwrap();
        // 调用链和 `checksum-as` 类似，但规则来源不同，因此保留独立命令。
        RunPitrChecksumTable(g.as_op(), cfg).map_err(Error::from)
    }));
    DefineFilterFlags(cmd.Flags(), vec!["!*.*".into()], false);
    DefineFlagsForChecksumPitrTableConfig(cmd.OpFlags());
    cmd
}

/// 构造 `checksum-upstream` 命令。
///
/// 该命令直接对当前集群求 checksum，不依赖 rewrite rule 或 PITR id map，
/// 适合把本地结果与外部记录的上游摘要做比对。
fn newUpstreamChecksumCommand() -> Command {
    let mut cmd = Command {
        Use: "checksum-upstream".into(),
        Short: "calculate the checksum".into(),
        Long: "Calculate the checksum of the current cluster (specified by `-u`). This can be used when you have the checksum of upstream elsewhere".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = ChecksumUpstreamConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        let g = tidbGlue().lock().unwrap();
        // 同样复用 operator 层执行器，CLI 只负责 glue 和参数透传。
        RunUpstreamChecksumTable(g.as_op(), cfg).map_err(Error::from)
    }));
    DefineFilterFlags(cmd.Flags(), vec!["!*.*".into()], false);
    DefineFlagsForChecksumUpstreamTableConfig(cmd.OpFlags());
    cmd
}

/// 构造 `force-flush` 命令。
///
/// 用于强制日志备份任务立即 flush，常见于排障或在某些运维窗口内
/// 人工触发一次落盘推进。
fn newForceFlushCommand() -> Command {
    let mut cmd = Command {
        Use: "force-flush".into(),
        Short: "force a log backup task to flush".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = ForceFlushConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        // 配置按引用传入，与 Go 版对同一结构体地址的使用习惯保持一致。
        RunForceFlush(&cfg).map_err(Error::from)
    }));
    DefineFlagsForForceFlushConfig(cmd.OpFlags());
    cmd
}

/// 构造 `crr-checkpoint` 命令。
///
/// 这条路径比较特殊：在真正运行服务之前，需要先借助 status server
/// 的准备阶段构造服务实例并把状态塞回命令上下文。
fn newCRRCheckpointCommand() -> Command {
    let mut cmd = Command {
        Use: "crr-checkpoint".into(),
        Short: "run the CRR checkpoint service".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let state = getCRRCheckpointServiceState(cmd)?;
        let result = state.Run();
        // 不论 Run 成功还是失败，都要尝试执行一次清理函数，模拟 Go 里的 defer cleanup。
        if let Some(cleanup) = state.cleanup.lock().unwrap().take() {
            cleanup();
        }
        result
    }));
    DefineFlagsForCRRCheckpointConfig(cmd.OpFlags());
    // status server 注册器会提前准备服务并把状态回填到 command context。
    registerStatusServerPreparer(&cmd, Arc::new(|cmd| prepareCRRCheckpointStatusServer(cmd)));
    cmd
}

/// 为 CRR checkpoint 命令准备 status server 注册器。
///
/// 它既创建真正的服务对象，也负责把状态放进命令上下文，
/// 让后续 `RunE` 阶段能复用同一份实例而不是再次初始化。
fn prepareCRRCheckpointStatusServer(cmd: &mut Command) -> Result<Option<StatusServerRegistrar>> {
    let mut cfg = CRRCheckpointConfig::default();
    if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
        return Err(err.into());
    }
    let g = tidbGlue().lock().unwrap();
    let (svc, cleanup) = NewCRRCheckpointService(g.as_op(), cfg).map_err(Error::from)?;
    let state = Arc::new(CrrCheckpointServiceState {
        service: svc,
        cleanup: Mutex::new(Some(cleanup)),
        ran: std::sync::atomic::AtomicBool::new(false),
    });
    // 继承已有 command context，避免覆盖掉上游已经放入的取消/追踪信息。
    let baseCtx = cmd.Context().cloned().unwrap_or_else(Context::Background);
    let ctx = Context::WithValue(
        &baseCtx,
        CRR_STATE_KEY,
        state.clone() as Arc<dyn std::any::Any + Send + Sync>,
    );
    cmd.SetContext(ctx);
    let state_for_reg = state;
    // 注册器只做路由挂载，不再重复创建 service。
    let registrar: StatusServerRegistrar = Arc::new(move |mux| {
        state_for_reg.Register(mux);
    });
    Ok(Some(registrar))
}

/// 从命令上下文中取回 CRR checkpoint 服务状态。
///
/// 如果准备阶段未执行成功，运行阶段必须明确报错，
/// 否则会在没有 service 的情况下继续向下执行。
fn getCRRCheckpointServiceState(cmd: &Command) -> Result<Arc<CrrCheckpointServiceState>> {
    let ctx = cmd
        .Context()
        .ok_or_else(|| Error::new("crr checkpoint service context is missing"))?;
    let any = ctx
        .Value(CRR_STATE_KEY)
        .ok_or_else(|| Error::new("crr checkpoint service is not prepared"))?;
    // Stored as Arc<CrrCheckpointServiceState> coerced to Arc<dyn Any>.
    // 这里显式 downcast，确保取出的上下文值类型正确且不是其他命令留下的脏状态。
    Arc::downcast::<CrrCheckpointServiceState>(any)
        .map_err(|_| Error::new("crr checkpoint service is not prepared"))
}

/// 构造 `test-storage` 命令。
///
/// 它会覆盖外部存储的主要 CRUD/遍历/流式接口，
/// 用于在真正执行备份恢复前验证存储配置与权限是否可用。
fn newTestStorageCommand() -> Command {
    let mut cmd = Command {
        Use: "test-storage".into(),
        Short: "test all operations of an external storage".into(),
        Long: "Test all ExternalStorage operations including read, write, delete, rename, walk, and streaming operations. This helps verify storage configuration and permissions before using it for backup/restore.".into(),
        no_args: true,
        ..Default::default()
    };
    cmd.RunE = Some(Arc::new(|cmd, _args| {
        let mut cfg = TestStorageConfig::default();
        if let Err(err) = cfg.ParseFromFlags(cmd.OpFlags()) {
            return Err(err.into());
        }
        let _ctx = GetDefaultContext();
        // 把完整探测逻辑放在 operator 层，CLI 仅承担一次无副作用的探针入口。
        RunTestStorage(cfg).map_err(Error::from)
    }));
    DefineFlagsForTestStorageConfig(cmd.OpFlags());
    cmd
}
