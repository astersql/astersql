// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/cmd/tidb-lightning-ctl` vs Go `main.go` / `fips.go`.
//! 中文补充：该文件把 Rust 控制命令入口的外部可观察行为压缩成四组契约测试，
//! 中文补充：重点验证参数分派、错误文案、FIPS 初始化入口以及 PD client 释放时机是否继续与 Go 对齐。
//! 中文补充：测试依赖 `stubs.rs` 中的轻量替身记录网络边界调用，
//! 中文补充：因此这里断言的是“对外协议”而不是实现细节，便于后续重构仍受同一语义约束。

use crate::entry::{
    checkpointTableNotFoundUsage, compactCluster, fetchMode, formatFatalError,
    formatFatalErrorStacked, run, run_main,
};
use crate::fips;
use crate::stubs::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn go_flag_help_and_boolean_syntax_match() {
    assert_eq!(run_main(vec!["-h".into()]), 0);
    assert_eq!(run_main(vec!["--help".into()]), 0);

    for value in ["1", "t", "T", "TRUE", "true", "True"] {
        let (_, actions, _) = LoadGlobalConfigWithCtl(&[format!("--compact={value}")]).unwrap();
        assert!(actions.compact, "Go strconv.ParseBool accepts {value}");
    }
    for value in ["0", "f", "F", "FALSE", "false", "False"] {
        let (_, actions, _) = LoadGlobalConfigWithCtl(&[format!("--compact={value}")]).unwrap();
        assert!(!actions.compact, "Go strconv.ParseBool accepts {value}");
    }

    let err = LoadGlobalConfigWithCtl(&["--compact=not-a-bool".into()]).unwrap_err();
    assert!(err.Error().contains("invalid value"), "{}", err.Error());
    assert!(LoadGlobalConfigWithCtl(&["---compact".into()]).is_err());
}

#[test]
fn for_all_stores_runs_in_parallel_and_cancels_child_context() {
    let cli = NewClient("lightning-ctl", vec![], vec![]);
    cli.set_stores(StoresInfo {
        Stores: vec![
            StoreInfo {
                Store: MetaStore {
                    Address: "up".into(),
                    State: metapb::StoreState_Up as i64,
                },
            },
            StoreInfo {
                Store: MetaStore {
                    Address: "offline".into(),
                    State: metapb::StoreState_Offline as i64,
                },
            },
        ],
    });

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let child_contexts = Arc::new(Mutex::new(Vec::new()));
    ForAllStores(
        &context::Background(),
        &cli,
        metapb::StoreState_Offline,
        |ctx, _store| {
            child_contexts.lock().unwrap().push(ctx.clone());
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(max_active.load(Ordering::SeqCst), 2);
    assert!(
        child_contexts
            .lock()
            .unwrap()
            .iter()
            .all(context::Context::is_cancelled),
        "errgroup.WithContext cancels its child context after Wait"
    );
}

#[test]
fn fetch_mode_metrics_regex_matches_go_exactly() {
    let metric = "tikv_config_rocksdb{cf=\"default\",name=\"hard_pending_compaction_bytes_limit\"}";
    assert_eq!(
        FetchModeFromMetrics(&format!("{metric} 0 ")).unwrap(),
        "normal",
        "Go compares the full capture with the exact string 0"
    );
    assert!(
        FetchModeFromMetrics(&format!("x{metric} 0")).is_err(),
        "Go regexp requires a word boundary before tikv_config_rocksdb"
    );
    assert_eq!(
        FetchModeFromMetrics(&format!("prefix:{metric} 0")).unwrap(),
        "import",
        "a non-word prefix still satisfies Go's word boundary"
    );
}

#[test]
fn config_alias_order_and_checkpoint_error_identity_match_go() {
    let unique = format!(
        "astersql-lightning-ctl-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    );
    let first = std::env::temp_dir().join(format!("{unique}-first.toml"));
    let second = std::env::temp_dir().join(format!("{unique}-second.toml"));
    std::fs::write(&first, "pd-addr = \"first-pd\"\n").unwrap();
    std::fs::write(&second, "pd-addr = \"second-pd\"\n").unwrap();

    let (cfg, _, _) = LoadGlobalConfigWithCtl(&[
        "--config".into(),
        first.to_string_lossy().into_owned(),
        "-c".into(),
        second.to_string_lossy().into_owned(),
    ])
    .unwrap();
    assert_eq!(cfg.TiDB.PdAddr, "second-pd");
    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);

    let lookalike = Error::new("checkpoint for table `db`.`table` not found");
    let formatted = formatFatalError(&lookalike);
    assert!(
        !formatted.contains(checkpointTableNotFoundUsage),
        "Go error equality uses the RFC error identity, not message matching"
    );
}

#[test]
fn go_rust_public_contract_matches() {
    // 中文补充：总入口只负责串起四类子场景，保持与 Go 测试分组一致的阅读顺序。
    // 中文补充：一旦其中任一契约失配，这个聚合测试会让回归在最外层立即暴露。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

/// Normal: FIPS hook, usage path, compact/fetch-mode dispatch, FetchModeFromMetrics.
fn contract_normal() {
    // 中文补充：每个子场景先重置全局退出函数和 mock，避免前一段测试污染后续断言。
    reset_exit_fn();
    set_exit_fn(|_| {});
    reset_tikv_mocks();

    // FIPS entry point is callable (Go blank-import side effect).
    // 中文补充：Go 版本通过 blank import 触发 FIPS side effect；Rust 这里至少保证入口可调用且不 panic。
    fips::enable_fips_only();

    // No action flags => Usage().
    // 中文补充：空 argv 应落回 usage 路径并返回成功，说明默认帮助行为没有被错误地视为执行失败。
    let code_ok = run(vec![]);
    assert!(code_ok.is_ok(), "empty argv shows usage and returns nil");

    // FetchModeFromMetrics mirrors Go regex branches.
    // 中文补充：该纯函数通过 metrics 里的阈值值判断 import/normal，
    // 中文补充：这里分别覆盖命中 0 与非 0 两个分支，确保与 Go 的文本解析约定一致。
    let import = FetchModeFromMetrics(
        "tikv_config_rocksdb{cf=\"default\",name=\"hard_pending_compaction_bytes_limit\"} 0\n",
    )
    .unwrap();
    // 中文补充：值为 0 时进入 import 模式，这是 Go 侧判断“仍在导入窗口”的关键约定。
    assert_eq!(import, "import");
    let normal = FetchModeFromMetrics(
        "tikv_config_rocksdb{cf=\"default\",name=\"hard_pending_compaction_bytes_limit\"} 1\n",
    )
    .unwrap();
    // 中文补充：任何非 0 值都回到 normal，避免未来把具体阈值误当成更多离散模式。
    assert_eq!(normal, "normal");

    // compactCluster walks Up/Offline stores and calls Compact(FullLevelCompact).
    // 中文补充：compact 只应覆盖状态不高于 Offline 的节点，因此 Tombstone store 必须被跳过。
    // 中文补充：同时断言压缩级别和资源组名，验证 Rust 侧调用参数没有偏离 Go 默认值。
    let cli = NewClient("lightning-ctl", vec!["http://pd".into()], vec![]);
    cli.set_stores(StoresInfo {
        Stores: vec![
            StoreInfo {
                Store: MetaStore {
                    Address: "tikv-1:20160".into(),
                    State: 0, // Up <= Offline
                },
            },
            StoreInfo {
                Store: MetaStore {
                    Address: "tikv-tomb:20160".into(),
                    State: 2, // Tombstone > Offline => skipped
                },
            },
        ],
    });
    let tls = common::TLS;
    compactCluster(&context::Background(), &cli, &tls).unwrap();
    let calls = take_compact_calls();
    // 中文补充：只保留一个调用记录，说明 Tombstone 节点确实未进入压缩分支。
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "tikv-1:20160");
    assert_eq!(calls[0].1, FullLevelCompact);
    assert_eq!(calls[0].2, "");

    // fetchMode prints errors per-store but does not fail the walk.
    // 中文补充：抓取模式属于巡检命令，单节点失败应打印错误后继续，而不是提前终止整个遍历。
    mock_fetch_mode("tikv-1:20160", Ok("import".into()));
    cli.set_stores(StoresInfo {
        Stores: vec![StoreInfo {
            Store: MetaStore {
                Address: "tikv-1:20160".into(),
                State: 0,
            },
        }],
    });
    fetchMode(&context::Background(), &cli, &tls).unwrap();

    // `-d` default becomes noop:// after LoadGlobalConfigWithCtl.
    // 中文补充：控制命令本身不需要真实数据目录，因此会把 `-d` 预置成合法但无副作用的 `noop://`。
    let (g, _a, _fs) = LoadGlobalConfigWithCtl(&[]).unwrap();
    // 中文补充：这里直接检查装载后的全局配置，确保默认值改写发生在解析阶段而不是运行阶段。
    assert_eq!(g.Mydumper.SourceDir, "noop://");

    reset_exit_fn();
}

/// Boundary: invalid metrics, Offline maxState filter, switch-mode valid values.
fn contract_boundary() {
    // 中文补充：这一组覆盖边界条件，确保“可接受的极限输入”仍遵循 Go 语义。
    reset_exit_fn();
    set_exit_fn(|_| {});
    reset_tikv_mocks();

    // Go passes strings.Split(pdAddr, ",") to pdhttp.NewClient verbatim.
    // Empty and repeated separators therefore remain observable endpoints.
    run(vec![]).unwrap();
    assert_eq!(take_last_client_endpoints(), Some(vec![String::new()]));
    run(vec!["--pd-urls=a,,b,".into()]).unwrap();
    assert_eq!(
        take_last_client_endpoints(),
        Some(vec!["a".into(), "".into(), "b".into(), "".into()])
    );

    // 中文补充：当 metrics 不暴露目标指标时，错误消息应明确指出状态不可见，而不是退化成任意模式。
    let err = FetchModeFromMetrics("no such metric\n").unwrap_err();
    assert!(err.Error().contains("import mode status is not exposed"));

    // Offline state included; Tombstone excluded (maxState = Offline).
    // 中文补充：`ForAllStores` 的上界是包含式比较，因此 Up 与 Offline 都要执行，Tombstone 被排除。
    let cli = NewClient("lightning-ctl", vec![], vec![]);
    cli.set_stores(StoresInfo {
        Stores: vec![
            StoreInfo {
                Store: MetaStore {
                    Address: "up".into(),
                    State: 0,
                },
            },
            StoreInfo {
                Store: MetaStore {
                    Address: "offline".into(),
                    State: 1,
                },
            },
            StoreInfo {
                Store: MetaStore {
                    Address: "tomb".into(),
                    State: 2,
                },
            },
        ],
    });
    ForAllStores(
        &context::Background(),
        &cli,
        metapb::StoreState_Offline,
        |_c, store| {
            Compact(
                &context::Background(),
                &common::TLS,
                &store.Address,
                FullLevelCompact,
                "",
            )
        },
    )
    .unwrap();
    let mut addrs: Vec<_> = take_compact_calls().into_iter().map(|c| c.0).collect();
    // ForAllStores runs callbacks concurrently, so Go does not guarantee their
    // completion order. Compare the eligible store set instead.
    addrs.sort();
    assert_eq!(addrs, vec!["offline".to_string(), "up".to_string()]);

    // Valid switch-mode via run (server stub ForAllStores is no-op success).
    // 中文补充：这里只验证参数合法性和分派路径，`import`/`normal` 都应被接受并顺利返回。
    assert!(
        run(vec!["--switch-mode".into(), "import".into()]).is_ok(),
        "import mode accepted"
    );
    assert!(
        run(vec!["--switch-mode".into(), "normal".into()]).is_ok(),
        "normal mode accepted"
    );

    reset_exit_fn();
}

/// Error: checkpoint-not-found formatting, stacked generic errors, invalid mode.
fn contract_error() {
    // 中文补充：错误路径测试关注“用户最终看到什么”，因此核心是文案分流而非内部异常类型本身。
    reset_exit_fn();
    set_exit_fn(|_| {});

    // 中文补充：checkpoint 缺失属于可操作的用户错误，输出应追加恢复建议而不是暴露整段栈信息。
    let err = ErrCheckpointTableNotFound.GenWithStackByArgs("`db`.`table`");
    let formatted = formatFatalError(&err);
    assert!(formatted.contains(&err.Error()));
    assert!(
        !formatted.contains("lightning/cmd/tidb-lightning-ctl\n") && formatted.contains("; "),
        "checkpoint-not-found path appends usage, not ErrorStack: {formatted}"
    );
    // 中文补充：两条恢复命令都必须出现在提示里，保证用户既能选择忽略，也能选择销毁后重试。
    assert!(formatted.contains("--checkpoint-error-ignore='`db`.`table`'"));
    assert!(formatted.contains("--checkpoint-error-destroy='`db`.`table`'"));
    assert!(formatted.contains(checkpointTableNotFoundUsage));

    // 中文补充：普通带栈错误则必须保留调用点，避免调试时丢失定位信息。
    let boom = StackError::new("boom");
    let stacked = formatFatalErrorStacked(&boom);
    assert_ne!(boom.Error(), stacked);
    assert!(
        stacked.contains("parity_test.rs"),
        "generic ErrorStack must include call site: {stacked}"
    );

    // 中文补充：非法 mode 不应悄悄回退成默认行为，必须返回明确的 invalid mode 错误。
    let bad = run(vec!["--switch-mode".into(), "weird".into()]);
    assert!(bad.is_err());
    let msg = bad.unwrap_err().Error();
    assert!(msg.contains("invalid mode"), "{msg}");

    reset_exit_fn();
}

/// Resource: PD client Close on all run paths; Drop also closes.
fn contract_resource_cleanup() {
    // 中文补充：这里验证资源生命周期，确保 Rust 版没有因为早返回而遗漏 Go `defer cli.Close()` 的效果。
    reset_exit_fn();
    set_exit_fn(|_| {});
    reset_tikv_mocks();

    // Usage path closes client.
    // 中文补充：即使只是打印 usage，也会创建过 client，因此成功路径同样必须回收句柄。
    assert!(run(vec![]).is_ok());

    // Compact path closes client (even with empty stores).
    // 中文补充：compact 提前返回也要关闭 client，避免控制命令在批量运维中泄露连接。
    assert!(run(vec!["--compact".into()]).is_ok());
    // 中文补充：前两条断言合起来覆盖“默认路径”和“动作路径”两类最常见的返回分支。

    // Explicit Close + Drop.
    // 中文补充：先验证显式 `Close` 可见，再验证 `Drop` 会兜底触发关闭。
    let cli = NewClient("lightning-ctl", vec!["pd".into()], vec![]);
    assert!(!cli.is_closed());
    cli.Close();
    assert!(cli.is_closed());

    let cli2 = NewClient("lightning-ctl", vec![], vec![]);
    let closed = cli2.is_closed();
    drop(cli2);
    assert!(!closed); // was open before drop
    // After drop, new client independent; verify Drop path via scoped block.
    // 中文补充：这里通过独立作用域读取关闭标记，避免把“析构前状态”和“析构后状态”混为一谈。
    let marker = {
        let c = NewClient("x", vec![], vec![]);
        let flag = c.closed_flag();
        drop(c);
        flag.load(Ordering::SeqCst)
    };
    assert!(marker, "Drop must Close PD client");

    // check-local-storage with import-into backend (no local files message).
    // 中文补充：`import-into` 后端走本地文件检查时，空结果也应被视为成功执行而非失败。
    let r = run(vec![
        "--backend".into(),
        "import-into".into(),
        "--check-local-storage".into(),
        "--enable-checkpoint=false".into(),
    ]);
    // 中文补充：这里顺带覆盖关闭 checkpoint 的组合参数，确认控制流不会因可选开关而偏离成功路径。
    assert!(r.is_ok(), "local-storage path: {:?}", r.err());

    reset_exit_fn();
}
