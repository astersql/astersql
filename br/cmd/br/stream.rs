// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! `br log` (stream) command family — mirrors `br/cmd/br/stream.go`.
//!
//! BR 日志备份（log backup / stream）CLI 入口：装配 `br log` 子命令树，
//! 将用户标志解析为 `StreamConfig`，再委托 `RunStreamCommand` 执行任务层逻辑。
//! 命令树与 Go 版一致：`log → {start,stop,pause,resume,status,truncate,metadata,advancer}`；
//! 本文件只负责 cobra 式命令装配、公共前置与按子命令分流的参数解析，
//! 不实现日志备份状态机本身。
//! 注意：Rust 侧会把内部常量（如 `"log start"`）经 `map_stream_cmd` 映射为短名
//!（如 `"start"`）再交给任务层，与 Go 直接传 `task.StreamStart` 的路径略有差异。
//! Help 回调会先 `HiddenFlagsForStream`，避免向用户展示 backup/restore 专用全局选项。

use std::sync::Arc;

use astersql_br_pkg_streamhelper_config::DefineFlagsForCheckpointAdvancerConfig;
use astersql_br_pkg_task::{
    DefineFilterFlags, DefineStreamCommonFlags, DefineStreamPauseFlags, DefineStreamStartFlags,
    DefineStreamStatusCommonFlags, DefineStreamTruncateLogFlags, HiddenFlagsForStream,
    RunStreamCommand, StreamConfig,
};

use crate::cmd::{
    GetDefaultContext, HasLogFile, Init, acceptAllTables, log_arguments_for, tidbGlue,
};
use crate::stubs::*;

// Go constants from br/pkg/task/stream.go
// 以下常量对齐 Go `task.Stream*`；值带 `"log "` 前缀，供 CLI 侧识别后再映射。
pub const StreamStart: &str = "log start";
pub const StreamStop: &str = "log stop";
pub const StreamPause: &str = "log pause";
pub const StreamResume: &str = "log resume";
pub const StreamStatus: &str = "log status";
pub const StreamTruncate: &str = "log truncate";
pub const StreamMetadata: &str = "log metadata";
/// 对应 Go `task.StreamCtl`；调试用 checkpoint advancer 子命令标识。
pub const StreamCtl: &str = "log advancer";

/// 将 CLI 内部常量映射为任务层期望的短命令名。
///
/// Go 的 `RunStreamCommand` 直接消费带 `"log "` 前缀的常量；
/// Rust 任务层接口使用短名，因此在调用前做一次归一化。
fn map_stream_cmd(cmdName: &str) -> &str {
    match cmdName {
        StreamStart => "start",
        StreamStop => "stop",
        StreamPause => "pause",
        StreamResume => "resume",
        StreamStatus => "status",
        StreamTruncate => "truncate",
        StreamMetadata => "metadata",
        StreamCtl => "advancer",
        other => other,
    }
}

/// NewStreamCommand specifies adding several commands for backup log
///
/// 构造顶层 `br log`：注册 PersistentPreRun（初始化/日志）、全部叶子子命令，
/// 并自定义 Help 以隐藏 stream 场景下不适用的全局标志（对齐 Go SetHelpFunc）。
pub fn NewStreamCommand() -> Command {
    let mut command = Command {
        Use: "log".into(),
        Short: "backup stream log from TiDB/TiKV cluster".into(),
        // 默认静默 usage；解析失败时由 streamCommand 再打开。
        SilenceUsage: true,
        Hidden: false,
        ..Default::default()
    };
    // 与 Go PersistentPreRunE 对齐：子命令执行前统一初始化环境与参数审计。
    command.PersistentPreRunE = Some(Arc::new(|c, _args| {
        Init(c)?;
        build::LogInfo(build::BR);
        logutil::LogEnvVariables();
        log_arguments_for(c);
        Ok(())
    }));

    // 子命令顺序与 Go `command.AddCommand(...)` 保持一致，便于对照文档与测试。
    command.AddCommand(vec![
        newStreamStartCommand(),
        newStreamStopCommand(),
        newStreamPauseCommand(),
        newStreamResumeCommand(),
        newStreamStatusCommand(),
        newStreamTruncateCommand(),
        newStreamCheckCommand(),
        newStreamAdvancerCommand(),
    ]);
    install_stream_help(&mut command);
    command
}

/// Wrap the command's existing help callback after hiding flags that do not apply to streams.
pub(crate) fn install_stream_help(command: &mut Command) {
    let default_help = command.HelpFunc();
    command.SetHelpFunc(Arc::new(move |command, strings| {
        HiddenFlagsForStream(command.Root().PersistentFlags());
        default_help(command, strings);
    }));
}

/// `log start`：启动日志备份任务；过滤接受全部表，并定义 start 专有标志。
fn newStreamStartCommand() -> Command {
    let mut command = Command {
        Use: "start".into(),
        Short: "start a log backup task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| streamCommand(command, StreamStart)));
    // true：stream 过滤路径；acceptAllTables 表示默认不排除用户表。
    DefineFilterFlags(command.Flags(), acceptAllTables(), true);
    DefineStreamStartFlags(command.Flags());
    command
}

/// `log stop`：停止日志备份；仅需公共 stream 标志（任务名等）。
fn newStreamStopCommand() -> Command {
    let mut command = Command {
        Use: "stop".into(),
        Short: "stop a log backup task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| streamCommand(command, StreamStop)));
    DefineStreamCommonFlags(command.Flags());
    command
}

/// `log pause`：暂停日志备份；挂载 pause 专有标志（如截止时间）。
fn newStreamPauseCommand() -> Command {
    let mut command = Command {
        Use: "pause".into(),
        Short: "pause a log backup task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| streamCommand(command, StreamPause)));
    DefineStreamPauseFlags(command.Flags());
    command
}

/// `log resume`：从暂停状态恢复日志备份；复用公共 stream 标志。
fn newStreamResumeCommand() -> Command {
    let mut command = Command {
        Use: "resume".into(),
        Short: "resume a log backup task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| streamCommand(command, StreamResume)));
    DefineStreamCommonFlags(command.Flags());
    command
}

/// `log status`：查询日志备份任务状态；使用 status 专用公共标志集合。
fn newStreamStatusCommand() -> Command {
    let mut command = Command {
        Use: "status".into(),
        Short: "get status for the log backup task".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|command, _| streamCommand(command, StreamStatus)));
    DefineStreamStatusCommonFlags(command.Flags());
    command
}

/// `log truncate`：按时间截断增量日志；需 truncate 专有标志指定截止点。
fn newStreamTruncateCommand() -> Command {
    let mut command = Command {
        Use: "truncate".into(),
        Short: "truncate the incremental log until sometime.".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| streamCommand(cmd, StreamTruncate)));
    DefineStreamTruncateLogFlags(command.Flags());
    command
}

/// `log metadata`：读取日志目录元数据；无额外专有标志，解析阶段为空操作。
fn newStreamCheckCommand() -> Command {
    let mut command = Command {
        Use: "metadata".into(),
        Short: "get the metadata of log dir.".into(),
        no_args: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| streamCommand(cmd, StreamMetadata)));
    command
}

/// `log advancer`：调试用 checkpoint 推进器；Hidden，最终应并入 TiDB。
///
/// 除公共 stream 标志外，还挂载 advancer 配置标志；
/// 任务层 `StreamConfig` 暂未接入 AdvancerCfg，此处仍校验标志合法性。
fn newStreamAdvancerCommand() -> Command {
    let mut command = Command {
        Use: "advancer".into(),
        Short: "Start a central worker for advancing the checkpoint. (only for debuging, this subcommand should be integrated to TiDB)".into(),
        no_args: true,
        Hidden: true,
        ..Default::default()
    };
    command.RunE = Some(Arc::new(|cmd, _| streamCommand(cmd, StreamCtl)));
    DefineStreamCommonFlags(command.Flags());
    DefineFlagsForCheckpointAdvancerConfig(command.AdvancerFlags());
    command
}

/// 叶子命令公共执行路径：解析配置 → 按子命令分流专有标志 → tracing → RunStreamCommand。
///
/// 数据流对齐 Go `streamCommand`：先 `Config.ParseFromFlags`，再按 `cmdName`
/// 调用对应 `ParseStream*`；advancer 额外从 flags 填充 `AdvancerCommandConfig`。
/// Rust 用 `with_tracing` 等价 Go 的 TracerStartSpan/FinishSpan。
pub fn streamCommand(command: &mut Command, cmdName: &str) -> Result<()> {
    let mut err: Option<Error> = None;
    // LogProgress 跟随是否配置了日志文件，避免无文件时刷进度干扰。
    let mut cfg = StreamConfig {
        Config: astersql_br_pkg_task::Config {
            LogProgress: HasLogFile(),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = effective_task_flags(command);

    // 基础配置解析失败时打开 SilenceUsage，让用户看到标志帮助（Go defer 同行为）。
    if let Err(e) = cfg.Config.ParseFromFlags(&flags) {
        err = Some(e.into());
        command.SilenceUsage = false;
        return Err(Error::Trace(err.unwrap()));
    }

    // 按子命令分流：metadata 无额外解析；其余走对应 Parse*；默认走 common。
    let parse_result = match cmdName {
        StreamMetadata => Ok(()),
        StreamTruncate => cfg
            .ParseStreamTruncateFromFlags(&flags)
            .map_err(Error::from),
        StreamStatus => cfg.ParseStreamStatusFromFlags(&flags).map_err(Error::from),
        StreamStart => cfg.ParseStreamStartFromFlags(&flags).map_err(Error::from),
        StreamPause => cfg.ParseStreamPauseFromFlags(&flags).map_err(Error::from),
        StreamCtl => {
            cfg.ParseStreamCommonFromFlags(&flags)
                .map_err(Error::from)?;
            let mut advancer = astersql_br_pkg_streamhelper_config::DefaultCommandConfig();
            advancer
                .GetFromFlags(command.AdvancerFlags())
                .map_err(Error::new)?;
            cfg.AdvancerCfg = astersql_br_pkg_task::AdvancerCommandConfig {
                BackoffTime: advancer.BackoffTime,
                TickDuration: advancer.TickDuration,
                TryAdvanceThreshold: advancer.TryAdvanceThreshold,
                CheckPointLagLimit: advancer.CheckPointLagLimit,
                OwnershipCycleInterval: advancer.OwnershipCycleInterval,
            };
            Ok(())
        }
        // stop / resume 等：仅解析公共 stream 标志。
        _ => cfg.ParseStreamCommonFromFlags(&flags).map_err(Error::from),
    };

    // 专有标志解析失败同样打开 usage，对齐 Go defer 中的 SilenceUsage=false。
    if let Err(e) = parse_result {
        command.SilenceUsage = false;
        return Err(Error::Trace(e));
    }

    let ctx = GetDefaultContext();
    let enable = cfg.Config.EnableOpenTracing;
    // 映射短名后再交给任务层；与 Go 直接传完整常量不同。
    let mapped = map_stream_cmd(cmdName);
    // 通过 TiDB glue 执行；真正的 start/stop/… 逻辑在 task 包。
    let result = crate::cmd::with_tracing(enable, ctx, |_ctx| {
        let g = tidbGlue().lock().unwrap();
        RunStreamCommand(g.as_task(), mapped, &mut cfg).map_err(Error::from)
    });
    if result.is_err() {
        command.SilenceUsage = false;
    }
    result
}
