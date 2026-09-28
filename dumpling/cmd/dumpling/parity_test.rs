// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/cmd/dumpling` public contracts vs Go `main.go`.
//!
//! 这些测试把 Rust CLI 的外部契约固定在与 Go `main.go` 一致的范围内，
//! 重点关注帮助/版本提前返回、默认参数、失败退出码以及资源清理时机。
//! 由于真实导出依赖数据库和 prometheus，这里通过注入 session 与 stub gatherer
//! 来验证控制流，而不是要求完整联通外部环境。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use astersql_dumpling_cli::{LongVersion, reset_version_vars};
use astersql_dumpling_export::{self as export, Error, Result as ExportResult};
use astersql_dumpling_log::{Level, Logger, ZapLogger};

use crate::config_flags::{DefineFlags, ParseFromFlags};
use crate::entry::{DumpSession, long_version_output, reset_cli_globals, run, run_with_factory};
use crate::stubs::{FlagHelp, FlagSet, take_default_gatherer};

#[test]
fn go_rust_public_contract_matches() {
    // 四个子场景覆盖正常路径、默认值、错误路径和清理语义。
    contract_normal_help_version_and_flags();
    contract_boundary_defaults_after_parse();
    contract_error_paths();
    contract_resource_cleanup_close_and_gatherer();
}

#[test]
fn string_slice_first_assignment_replaces_go_default() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags.Parse(&["--filter".into(), "sales.*".into()]).unwrap();

    assert_eq!(
        flags.GetStringSlice("filter").unwrap(),
        vec!["sales.*"],
        "pflag StringSlice clears its registered default on the first explicit assignment"
    );
}

#[test]
fn shorthand_accepts_an_attached_value_like_go_pflag() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags.Parse(&["-P4001".into()]).unwrap();

    assert_eq!(flags.GetInt("port").unwrap(), 4001);
}

#[test]
fn default_output_directory_uses_go_rfc3339_shape() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    let output = flags.GetString("output").unwrap();
    let timestamp = output.strip_prefix("./export-").unwrap();

    assert_eq!(timestamp.as_bytes().get(4), Some(&b'-'));
    assert_eq!(timestamp.as_bytes().get(7), Some(&b'-'));
    assert_eq!(timestamp.as_bytes().get(10), Some(&b'T'));
    assert_eq!(timestamp.as_bytes().get(13), Some(&b':'));
    assert_eq!(timestamp.as_bytes().get(16), Some(&b':'));
    assert!(
        timestamp.ends_with('Z')
            || timestamp
                .get(19..)
                .is_some_and(|zone| zone.starts_with('+') || zone.starts_with('-')),
        "Go time.RFC3339 output includes an explicit timezone: {timestamp}"
    );
}

#[test]
fn local_flag_parser_matches_pflag_value_contracts() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&[
            "--database".into(),
            "\"sales,archive\",analytics".into(),
            "--params".into(),
            "a=1".into(),
            "--params".into(),
            "b=2".into(),
            "--read-timeout".into(),
            "1h30m".into(),
        ])
        .unwrap();

    assert_eq!(
        flags.GetStringSlice("database").unwrap(),
        vec!["sales,archive", "analytics"]
    );
    assert_eq!(
        flags.GetStringToString("params").unwrap(),
        [("a".into(), "1".into()), ("b".into(), "2".into())]
            .into_iter()
            .collect()
    );
    assert_eq!(
        flags.GetDuration("read-timeout").unwrap(),
        std::time::Duration::from_secs(5_400)
    );

    let mut invalid_duration = FlagSet::new();
    DefineFlags(&mut invalid_duration);
    assert!(
        invalid_duration
            .Parse(&["--read-timeout".into(), "5".into()])
            .is_err(),
        "time.ParseDuration requires a unit except for the literal zero"
    );

    let mut invalid_bool = FlagSet::new();
    DefineFlags(&mut invalid_bool);
    assert!(
        invalid_bool.Parse(&["--help=yes".into()]).is_err(),
        "strconv.ParseBool does not accept yes/no aliases"
    );
}

#[test]
fn command_line_parse_errors_use_go_pflag_exit_code() {
    assert_eq!(
        run(vec!["--not-a-dumpling-flag".into()]),
        2,
        "pflag.CommandLine uses ExitOnError for command-line syntax errors"
    );
    assert_eq!(
        run(vec!["--host".into()]),
        2,
        "a missing flag value is also a pflag parse error"
    );
}

#[test]
fn version_output_matches_go_builtin_println_bytes() {
    let output = long_version_output();
    assert_eq!(output, format!("{}\n", LongVersion()));
    assert!(output.ends_with("\n\n"));
}

#[test]
fn tables_list_works_with_default_filter_and_is_case_insensitive_by_default() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&["--tables-list".into(), "Sales.Orders".into()])
        .unwrap();
    let mut conf = export::DefaultConfig();
    ParseFromFlags(&mut conf, &flags).unwrap();

    assert!(conf.SpecifiedTables);
    assert!(conf.TableFilter.MatchTable("sales", "orders"));
    assert!(!conf.TableFilter.MatchTable("sales", "customers"));
}

#[test]
fn tables_list_splits_the_qualified_name_only_once() {
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    flags
        .Parse(&["--tables-list".into(), "Sales.Order.History".into()])
        .unwrap();
    let mut conf = export::DefaultConfig();
    ParseFromFlags(&mut conf, &flags).unwrap();

    assert!(conf.TableFilter.MatchTable("sales", "order.history"));
}

/// Normal: help / version early-exit and DefineFlags registers help.
fn contract_normal_help_version_and_flags() {
    // 每次都重置全局 gatherer 和版本变量，避免前一个测试留下可见状态。
    reset_cli_globals();
    reset_version_vars();

    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    // 先确认 help flag 已注册，再去验证后续提前返回逻辑。
    assert!(flags.GetBool(FlagHelp).is_ok());

    let code = run(vec!["--help".into()]);
    // help 路径只打印 usage，不应因为“没有真正导出”被当成失败。
    assert_eq!(code, 0, "Go help path returns without os.Exit");

    let code = run(vec!["-V".into()]);
    // 版本路径同样是正常提前结束，且 LongVersion 需要已经可读。
    assert_eq!(
        code, 0,
        "Go version path returns after printing LongVersion"
    );
    // 这里只检查关键信息存在即可，避免和具体构建元数据耦合过深。
    assert!(LongVersion().contains("Release version:"));

    // Successful dump path with injected session (DB/network mocked).
    // 这里用 mock session 覆盖真实导出器，专门锁定主流程拼装和退出码。
    let code = run_with_factory(vec!["--status-addr".into(), "".into()], |mut conf| {
        conf.StatusAddr.clear();
        conf.Logger = Some(Logger {
            Logger: ZapLogger::capture(Level::Info),
        });
        Ok(MockSession::ok(conf.Logger.clone().unwrap()))
    });
    assert_eq!(code, 0);
    // 成功路径会把 registry 安装成默认 gatherer，后续指标读取依赖这个副作用。
    assert!(take_default_gatherer().is_some());
}

/// Boundary: empty argv ParseFromFlags applies Go DefineFlags defaults (port 4000).
fn contract_boundary_defaults_after_parse() {
    reset_cli_globals();
    let mut flags = FlagSet::new();
    DefineFlags(&mut flags);
    // 空 argv 时应全部落回 Go DefineFlags 注册的默认值，而不是 DefaultConfig 原值。
    flags.Parse(&[]).unwrap();
    let mut conf = export::DefaultConfig();
    assert_eq!(conf.Port, 3306, "DefaultConfig port before flags");
    ParseFromFlags(&mut conf, &flags).unwrap();
    // 这里逐项验证几组最关键的默认字段，防止 Rust/Go 默认值悄悄分叉。
    // 端口最容易暴露“DefaultConfig 默认值”与“flag 默认值”被混用的问题。
    assert_eq!(conf.Port, 4000, "DefineFlags default port is 4000");
    assert_eq!(conf.Host, "127.0.0.1");
    assert_eq!(conf.Threads, 4);
    assert_eq!(conf.User, "root");
    assert!(!conf.OutputDirPath.is_empty());
    assert_eq!(conf.StatusAddr, ":8281");
    assert_eq!(conf.Consistency, export::ConsistencyTypeAuto);
}

/// Error: unparsed args, invalid --threads, create-dumper / dump failure.
fn contract_error_paths() {
    reset_cli_globals();

    let code = run(vec!["orphan-arg".into()]);
    // Go 会把未解析的位置参数视为错误；Rust 不能把它们静默吞掉。
    assert_eq!(code, 1, "unparsed positional args => os.Exit(1)");

    let code = run(vec!["--threads".into(), "0".into()]);
    // 线程数校验属于 ParseFromFlags 的职责，失败时必须返回 1。
    assert_eq!(code, 1, "ParseFromFlags rejects threads<=0");

    // Real NewDumper fails against empty stub DB (SelectVersion empty).
    // 这里走真实工厂失败分支，证明主流程不会把创建错误误判成导出错误。
    let code = run(vec![]);
    assert_eq!(code, 1, "create dumper failed => os.Exit(1)");

    let code = run_with_factory(vec![], |_conf| -> ExportResult<MockSession> {
        Err(Error::new("boom-create"))
    });
    // 注入工厂失败与真实工厂失败都应复用同一退出码语义。
    assert_eq!(code, 1);

    let code = run_with_factory(vec!["--status-addr".into(), "".into()], |mut conf| {
        conf.StatusAddr.clear();
        conf.Logger = Some(Logger {
            Logger: ZapLogger::capture(Level::Error),
        });
        Ok(MockSession::fail_dump(conf.Logger.clone().unwrap()))
    });
    // Dump 失败时主流程既要记录日志，也要向调用方返回失败码。
    assert_eq!(code, 1, "dump failed => os.Exit(1)");
}

/// Resource: Dump failure still Close(); gatherer cleared on reset.
fn contract_resource_cleanup_close_and_gatherer() {
    reset_cli_globals();

    let closed = Arc::new(AtomicBool::new(false));
    let dumped = Arc::new(AtomicUsize::new(0));
    let closed_c = closed.clone();
    let dumped_c = dumped.clone();

    let code = run_with_factory(vec!["--status-addr".into(), "".into()], move |mut conf| {
        conf.StatusAddr.clear();
        conf.Logger = Some(Logger {
            Logger: ZapLogger::capture(Level::Error),
        });
        Ok(MockSession {
            logger: conf.Logger.clone().unwrap(),
            fail_dump: true,
            closed: closed_c,
            dumped: dumped_c,
        })
    });
    assert_eq!(code, 1);
    // 计数器帮助区分“根本没执行 Dump”和“执行后才失败”两类回归。
    assert_eq!(dumped.load(Ordering::SeqCst), 1, "Dump was invoked");
    assert!(
        closed.load(Ordering::SeqCst),
        "Close must run even when Dump fails (Go ignores Close error)"
    );
    // gatherer 在运行期间要保留，清理动作由 reset helper 统一负责。
    assert!(take_default_gatherer().is_some());

    reset_cli_globals();
    // 清理后必须回到空状态，否则后续测试会误以为当前流程成功注册过 gatherer。
    assert!(take_default_gatherer().is_none());
}

struct MockSession {
    // logger 由主流程注入，便于保留与真实 dumper 相同的日志调用面。
    logger: Logger,
    // fail_dump 用来在不引入外部依赖时切换成功/失败路径。
    fail_dump: bool,
    closed: Arc<AtomicBool>,
    dumped: Arc<AtomicUsize>,
}

impl MockSession {
    fn ok(logger: Logger) -> Self {
        // 成功实例仍保留计数器，便于和失败实例共享同一断言结构。
        Self {
            logger,
            fail_dump: false,
            closed: Arc::new(AtomicBool::new(false)),
            dumped: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn fail_dump(logger: Logger) -> Self {
        // 失败实例只在 Dump 阶段报错，不模拟 Close 失败，贴近 Go 当前处理。
        Self {
            logger,
            fail_dump: true,
            closed: Arc::new(AtomicBool::new(false)),
            dumped: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl DumpSession for MockSession {
    fn Dump(&mut self) -> ExportResult<()> {
        // 先记账再决定是否失败，保证测试能看到 Dump 确实被调用过。
        self.dumped.fetch_add(1, Ordering::SeqCst);
        if self.fail_dump {
            Err(Error::new("mock dump failed"))
        } else {
            Ok(())
        }
    }
    fn Close(&mut self) -> ExportResult<()> {
        // Close 固定成功，专门验证主流程“总是调用”而非“如何处理 Close 错误”。
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn L(&self) -> Logger {
        self.logger.clone()
    }
}
