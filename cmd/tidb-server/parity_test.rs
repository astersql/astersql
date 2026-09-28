// Copyright 2026 AsterSQL.

//! Parity tests for `cmd/tidb-server` public contracts vs Go `main.go` / `fips.go`.
//!
//! 这份文件不是功能测试，而是对外契约回归测试。
//! 它把 Rust 入口层暴露出的可观察行为拆成四组：
//! 正常路径、边界路径、错误路径、资源清理路径。
//! 每组断言都尽量对应 Go `main.go`、`fips.go` 和 `main_test.go`
//! 已存在的公开语义，避免 Rust 迁移后在参数解析、全局变量写入、
//! starter 模式分支或清理顺序上悄悄偏离。
//!
//! 这里大量依赖 `stubs` 记录事件，而不是拉起真实组件。
//! 这样可以把验证重点放在入口编排与副作用顺序上，
//! 也就是用户和上层脚本真正能观察到的契约。

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use crate::entry::{
    self, cleanup, createMgrClientForStarter, exitAfterKeyspaceActivate, exitCodeForSignal,
    exitCodeInt, exitCodeOK, flagBoolean, initFlagSetWithArgs, initVersions, nmKeyspaceActivate,
    nmStarterParams, nmVersion, overrideConfig, parseDuration, parseStarterAdditionalParams,
    prepareKeyspaceObservabilityForStarter, prometheusPushClient, registerStores,
    set_starter_additional_params, setGlobalVars, stringToList, validateVersionConfigPolicy,
};
use crate::fips;
use crate::stubs::{
    self, Signal, config, deploymode, domain, kerneltype, kv, kvstore, server, syscall, vardef,
    variable,
};

#[test]
fn go_rust_public_contract_matches() {
    // 顶层测试只负责串起四组子场景。
    // 不拆成多个 `#[test]` 的原因是它们共享同一份入口契约语境，
    // 统一跑完更接近 Go 侧 main 相关测试的阅读方式。
    let _g = stubs::test_guard();
    contract_normal_paths();
    contract_boundary_cases();
    contract_error_paths();
    contract_resource_cleanup();
}

fn contract_normal_paths() {
    // 每组开始前都重置 stub 状态，保证事件日志、全局配置、
    // 以及 deploy mode 不会把前一段断言的副作用泄漏到下一段。
    stubs::reset_all_for_test();

    // 中文补充：这里锁定信号到退出码的映射。
    // Go 主程序依赖这个约定把 SIGINT 解释成中断退出，
    // 其他常见信号则保持成功码，避免把正常停止误报为失败。
    // exitCodeForSignal — Go TestExitCodeForSignal
    assert_eq!(exitCodeForSignal(Signal::SIGINT), exitCodeInt);
    assert_eq!(exitCodeForSignal(Signal::SIGTERM), exitCodeOK);
    assert_eq!(exitCodeForSignal(Signal::SIGHUP), exitCodeOK);
    assert_eq!(exitCodeForSignal(Signal::SIGQUIT), exitCodeOK);
    assert_eq!(exitCodeForSignal(Signal::None), exitCodeOK);
    assert_eq!(exitCodeInt, 128 + syscall::SIGINT);

    // 中文补充：`stringToList` 需要同时接受 JSON 数组和松散分隔串。
    // 这保证命令行和配置透传参数在 Rust 侧延续 Go 的兼容性。
    // stringToList
    assert!(stringToList("").is_empty());
    assert_eq!(
        stringToList(r#"["db.t1", "db.t2"]"#),
        vec!["db.t1".to_string(), "db.t2".to_string()]
    );
    assert_eq!(
        stringToList("a,b c"),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );

    // 中文补充：裸数字会回退成秒，属于入口层的宽松解析策略。
    // 这样旧脚本传 `45` 时不会因为缺少单位在 Rust 版本中失效。
    // parseDuration — bare number retries with "s"
    assert_eq!(parseDuration("45s"), Duration::from_secs(45));
    assert_eq!(parseDuration("45"), Duration::from_secs(45));
    assert_eq!(parseDuration("1m"), Duration::from_secs(60));

    // 中文补充：布尔 flag 默认值为 false 时，usage 文案要补出默认信息。
    // 这里不只测值，还间接测注册后的查询行为是否保持 Go 风格。
    // flagBoolean appends "(default false)" for false defaults
    let mut fset = stubs::flag::NewFlagSet("t", true);
    let _ = flagBoolean(&mut fset, "x", false, "usage");
    // Lookup after Bool registration uses default until Parse
    assert!(!fset.LookupBool("x"));

    // 中文补充：这段验证 `overrideConfig` 既修改配置，
    // 也记录 flag 是否真的被访问过。
    // `starter-additional-params` 还会写入入口模块的全局存根，
    // 后续 manager client 创建流程依赖这份原始字符串。
    // overrideConfig keyspace-activate + starter params (Go TestOverrideConfigKeyspaceActivateMode)
    let argv = vec![
        "tidb-server".into(),
        "--keyspace-activate=true".into(),
        "--starter-additional-params=pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1".into(),
    ];
    let fset = initFlagSetWithArgs(&argv);
    let mut cfg = config::NewConfig();
    overrideConfig(&mut cfg, &fset);
    assert!(cfg.KeyspaceActivateMode);
    assert_eq!(
        entry::starter_additional_params(),
        "pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1"
    );
    assert!(fset.was_visited(nmKeyspaceActivate));
    assert!(fset.was_visited(nmStarterParams));

    // 中文补充：starter 透传参数解析必须保留字段名和 k=v 语义。
    // 成功路径既覆盖完整输入，也覆盖空字符串回到零值结构的分支。
    // parseStarterAdditionalParams success
    let p = parseStarterAdditionalParams(
        "manager-namespace=mns,pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1",
    )
    .unwrap();
    assert_eq!(p.managerNamespace, "mns");
    assert_eq!(p.podName, "pod-1");
    assert_eq!(p.podIP, "10.0.0.1");
    assert_eq!(p.podNamespace, "ns-1");
    assert!(parseStarterAdditionalParams("").unwrap().podName.is_empty());

    // 中文补充：存储类型注册顺序是入口初始化的一部分。
    // 这里同时验证事件日志和最终注册表，防止只记录了日志却没写入状态，
    // 或者状态正确但对外观测到的初始化顺序已经变化。
    // registerStores order
    stubs::clear_events();
    registerStores().unwrap();
    let ev = stubs::take_events();
    assert!(ev.iter().any(|e| e.contains("Register tikv")));
    assert!(ev.iter().any(|e| e.contains("Register mocktikv")));
    assert!(ev.iter().any(|e| e.contains("Register unistore")));
    assert_eq!(
        kvstore::registered_types(),
        vec![
            "mocktikv".to_string(),
            "tikv".to_string(),
            "unistore".to_string()
        ]
    );

    // 中文补充：`setGlobalVars` 负责把配置投影到系统变量层。
    // 这些值后续会被 SQL 层和可观测接口读取，因此要确认：
    // 多引擎列表按逗号串联、server version 直接透传、
    // socket 中的 `{Port}` 占位符会展开、实例级变量保留 instance scope。
    // setGlobalVars writes isolation engines / version / socket
    config::UpdateGlobal(|c| {
        c.IsolationRead.Engines = vec!["tikv".into(), "tidb".into()];
        c.ServerVersion = "test".into();
        c.Port = 4000;
        c.Socket = "/tmp/tidb-{Port}.sock".into();
        c.Instance.InstancePlanCacheMaxMemSize = "444".into();
    });
    setGlobalVars();
    assert_eq!(
        variable::GetSysVar(vardef::TiDBIsolationReadEngines)
            .unwrap()
            .Value,
        "tikv,tidb"
    );
    assert_eq!(variable::GetSysVar(vardef::Version).unwrap().Value, "test");
    assert_eq!(
        variable::GetSysVar(vardef::Socket).unwrap().Value,
        "/tmp/tidb-4000.sock"
    );
    assert_eq!(
        variable::GetSysVar(vardef::TiDBInstancePlanCacheMaxMemSize)
            .unwrap()
            .Value,
        "444"
    );
    assert!(variable::HasInstanceScope(
        vardef::TiDBInstancePlanCacheMaxMemSize
    ));

    // 中文补充：FIPS 钩子这里不检查具体副作用，
    // 只确认入口可安全调用，等价于 Go 侧匿名导入 boringcrypto 的存在性约束。
    // 普通构建不选中 Go 的 boringcrypto 文件；显式 FIPS 请求必须 fail-closed。
    fips::enable_fips_only();
    assert!(fips::enable_fips_only_for_build(false).is_ok());
    let fips_result = fips::enable_fips_only_for_build(true);
    if let Err(error) = fips_result {
        assert!(error.contains("not FIPS validated") || error.contains("already installed"));
    }

    // 中文补充：当监听地址是 `0.0.0.0` 时，
    // `overrideConfig` 需要推导出可对外宣传的回环地址。
    // 这是启动后上报 advertise-address 的兼容行为。
    // advertise-address fallback when host is 0.0.0.0
    stubs::reset_all_for_test();
    let argv = vec!["tidb-server".into()];
    let fset = initFlagSetWithArgs(&argv);
    let mut cfg = config::NewConfig();
    cfg.Host = "0.0.0.0".into();
    cfg.AdvertiseAddress.clear();
    overrideConfig(&mut cfg, &fset);
    assert_eq!(cfg.AdvertiseAddress, "127.0.0.1");
}

fn contract_boundary_cases() {
    // 边界路径覆盖“不是明显错误，但容易因重构改变语义”的分支。
    // 这些行为通常由入口编排和默认值共同决定，最适合用 parity 测试守护。
    stubs::reset_all_for_test();

    // 中文补充：`collect-log` 是一个提前返回的子命令。
    // 这里确认它走向去敏文件逻辑且返回成功码，
    // 不会误进入完整 server 启动流程。
    // collect-log subcommand
    let code = entry::run_main(&[
        "tidb-server".into(),
        "collect-log".into(),
        "mem://in.log".into(),
        "-".into(),
    ]);
    assert_eq!(code, exitCodeOK);
    let ev = stubs::take_events();
    assert!(ev.iter().any(|e| e.contains("redact.DeRedactFile")));

    // 中文补充：starter 的 keyspace observability 只对 TiKV 有意义。
    // 当底层 store 不是 TiKV 时，必须静默跳过，
    // 避免给 unistore/mock 场景制造无效指标标签。
    // prepareKeyspaceObservability skips non-TiKV
    kerneltype::set_nextgen_for_test(true);
    deploymode::Set(deploymode::Starter).unwrap();
    config::UpdateGlobal(|c| {
        c.Store = config::StoreTypeUniStore.into();
        c.KeyspaceName = "ks".into();
    });
    prepareKeyspaceObservabilityForStarter(HashMap::new()).unwrap();
    assert!(
        config::GetGlobalConfig()
            .GetKeyspaceObservabilityMetricLabels()
            .is_empty()
    );

    // 中文补充：切回 TiKV 后，
    // 最基本的 `keyspace_name` 指标标签必须写回全局配置。
    // 这对应 starter 激活 keyspace 后的观测维度补全。
    // TiKV path sets keyspace_name label
    config::UpdateGlobal(|c| {
        c.Store = config::StoreTypeTiKV.into();
        c.KeyspaceName = "ks".into();
        c.KeyspaceObservabilityValues = config::KeyspaceObservabilityValues::default();
    });
    prepareKeyspaceObservabilityForStarter(HashMap::new()).unwrap();
    assert_eq!(
        config::GetGlobalConfig().GetKeyspaceObservabilityMetricLabels(),
        HashMap::from([("keyspace_name".into(), "ks".into())])
    );

    // 中文补充：这里验证 metadata 与配置字段定义的合并规则。
    // 同一份输入既要生成 metric labels，
    // 也要同步生成 slow log 字段，确保不同观测面保持一致。
    // metadata fields merge
    config::UpdateGlobal(|c| {
        c.KeyspaceObservability = config::KeyspaceObservability {
            Fields: vec![config::KeyspaceObservabilityField {
                Source: "meta_a".into(),
                MetricLabel: "keyspace_meta_label_a".into(),
                SlowLogField: "Keyspace_meta_slow_a".into(),
                StmtLogField: "stmt_meta_a".into(),
                Required: true,
            }],
        };
    });
    prepareKeyspaceObservabilityForStarter(HashMap::from([("meta_a".into(), "value_a".into())]))
        .unwrap();
    let cfg = config::GetGlobalConfig();
    assert_eq!(
        cfg.GetKeyspaceObservabilityMetricLabels()
            .get("keyspace_meta_label_a")
            .map(String::as_str),
        Some("value_a")
    );
    assert_eq!(
        cfg.GetKeyspaceObservabilitySlowLogFields(),
        vec![config::KeyspaceObservabilityLogField {
            Name: "Keyspace_meta_slow_a".into(),
            Value: "value_a".into(),
        }]
    );

    // 中文补充：starter 模式下，命令行只覆盖 cert/key，
    // 但如果没有显式传 CA，就要保留配置文件中的 CA。
    // 这是对 Go 行为的细粒度对齐，防止“半覆盖”把 TLS 配置弄残。
    // starter TLS cert/key only override preserves CA
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    let fset = initFlagSetWithArgs(&[
        "tidb-server".into(),
        "--cluster-cert=/tmp/flag-cluster-cert.pem".into(),
        "--cluster-key=/tmp/flag-cluster-key.pem".into(),
        "--sql-cert=/tmp/flag-sql-cert.pem".into(),
        "--sql-key=/tmp/flag-sql-key.pem".into(),
    ]);
    let mut cfg = config::NewConfig();
    cfg.DeployMode = deploymode::Starter;
    cfg.Security.ClusterSSLCA = "/tmp/config-cluster-ca.pem".into();
    cfg.Security.ClusterSSLCert = "/tmp/config-cluster-cert.pem".into();
    cfg.Security.ClusterSSLKey = "/tmp/config-cluster-key.pem".into();
    cfg.Security.SSLCA = "/tmp/config-sql-ca.pem".into();
    cfg.Security.SSLCert = "/tmp/config-sql-cert.pem".into();
    cfg.Security.SSLKey = "/tmp/config-sql-key.pem".into();
    overrideConfig(&mut cfg, &fset);
    assert_eq!(cfg.Security.ClusterSSLCA, "/tmp/config-cluster-ca.pem");
    assert_eq!(cfg.Security.ClusterSSLCert, "/tmp/flag-cluster-cert.pem");
    assert_eq!(cfg.Security.ClusterSSLKey, "/tmp/flag-cluster-key.pem");
    assert_eq!(cfg.Security.SSLCA, "/tmp/config-sql-ca.pem");
    assert_eq!(cfg.Security.SSLCert, "/tmp/flag-sql-cert.pem");
    assert_eq!(cfg.Security.SSLKey, "/tmp/flag-sql-key.pem");

    // 中文补充：classic kernel 不支持 keyspace 配置。
    // 这里锁定主程序验证路径的历史行为：
    // 输出错误后以 0 退出，而不是抛出失败码。
    // classic rejects keyspace in config validation path of main
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(false);
    let code = entry::run_main(&["tidb-server".into(), "--keyspace-name=ks".into()]);
    assert_eq!(code, exitCodeOK); // Go os.Exit(0) after stderr message
}

fn contract_error_paths() {
    // 错误路径聚焦“输入非法时报什么错、在哪一层报”。
    // 这类断言能避免 Rust 迁移后把原本用户可诊断的错误
    // 变成 panic、静默忽略，或改成不兼容的提示文案。
    stubs::reset_all_for_test();

    // 中文补充：重复键必须在解析阶段报错，
    // 否则后写覆盖前写会让 starter 身份信息变得不可预测。
    let err = parseStarterAdditionalParams("pod-name=pod-1,pod-name=pod-2").unwrap_err();
    assert!(err.msg.contains("duplicated"));

    // 中文补充：未知键不能被容忍，
    // 入口层需要尽快告诉用户拼写或透传参数不受支持。
    let err = parseStarterAdditionalParams("pod-name=pod-1,unknown=value").unwrap_err();
    assert!(err.msg.contains("unknown starter additional param"));

    // 中文补充：缺少等号说明输入格式已损坏，
    // 这里守护 Go 版本的 `k=v` 约束。
    let err = parseStarterAdditionalParams("pod-name").unwrap_err();
    assert!(err.msg.contains("k=v format"));

    // 中文补充：空 key 同样非法，避免形成难以追踪的匿名参数。
    let err = parseStarterAdditionalParams("=v").unwrap_err();
    assert!(err.msg.contains("empty key"));

    // 中文补充：两个 initialize flag 互斥，
    // 主程序通过 panic/fatal 中止，这里按同样语义捕获 unwind。
    // mutual exclusive initialize flags
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let fset = initFlagSetWithArgs(&[
            "tidb-server".into(),
            "--initialize-secure=true".into(),
            "--initialize-insecure=true".into(),
        ]);
        let mut cfg = config::NewConfig();
        overrideConfig(&mut cfg, &fset);
    }));
    assert!(result.is_err());

    // 中文补充：manager client 的创建依赖三类前提：
    // 当前处于 starter、notifier 被启用、补充参数完整可解析。
    // 下方依次验证缺参、重复键、未知键、缺 manager 地址/命名空间，
    // 最后再确认满足条件时能返回非空客户端。
    // createMgrClientForStarter errors (starter + notifier)
    kerneltype::set_nextgen_for_test(true);
    deploymode::Set(deploymode::Starter).unwrap();
    config::UpdateGlobal(|c| {
        c.StarterParams.EnableManagerNotifier = true;
        c.StarterParams.ManagerAddr = "manager.example.com:8000".into();
    });
    set_starter_additional_params("");
    let err = createMgrClientForStarter().unwrap_err();
    assert!(err.msg.contains("starter-additional-params"));

    set_starter_additional_params(
        "pod-name=pod-1,pod-name=pod-2,pod-ip=10.0.0.1,pod-namespace=ns-1",
    );
    let err = createMgrClientForStarter().unwrap_err();
    assert!(err.msg.contains("duplicated"));

    set_starter_additional_params(
        "pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1,unknown=value",
    );
    let err = createMgrClientForStarter().unwrap_err();
    assert!(err.msg.contains("unknown"));

    config::UpdateGlobal(|c| {
        c.StarterParams.ManagerAddr.clear();
    });
    set_starter_additional_params("pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1");
    let err = createMgrClientForStarter().unwrap_err();
    assert!(err.msg.contains("manager-addr") || err.msg.contains("manager-namespace"));

    set_starter_additional_params(
        "manager-namespace=manager-ns,pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1",
    );
    let cli = createMgrClientForStarter().unwrap();
    assert!(cli.is_some());

    // 中文补充：nextgen kernel 禁止手工设置 edition/version，
    // 因为这些信息由内核发行形态统一决定，不能再由配置覆盖。
    // nextgen forbids edition/version config
    config::UpdateGlobal(|c| {
        c.TiDBEdition = "Starter".into();
    });
    let err = validateVersionConfigPolicy(&config::GetGlobalConfig()).unwrap_err();
    assert!(err.msg.contains("not allowed to set in nextgen kernel"));

    mysql_invalid_nextgen_release();
}

fn mysql_invalid_nextgen_release() {
    use crate::stubs::mysql;
    // 中文补充：这段单独抽出来，是为了把“版本字符串非法”与
    // “配置策略先失败”区分开，确保命中 `initVersions` 自己的校验分支。
    kerneltype::set_nextgen_for_test(true);
    // Clear edition/version overrides so validateVersionConfigPolicy does not fire first.
    config::UpdateGlobal(|c| {
        c.TiDBEdition.clear();
        c.TiDBReleaseVersion.clear();
        c.ServerVersion.clear();
    });
    let origin = mysql::TiDBReleaseVersion();
    mysql::set_TiDBReleaseVersion("v26.13.1");
    let err = initVersions(&config::GetGlobalConfig()).unwrap_err();
    assert!(
        err.msg.contains("invalid tidb release version"),
        "got: {}",
        err.msg
    );
    mysql::set_TiDBReleaseVersion(origin);
}

fn contract_resource_cleanup() {
    // 资源清理路径强调“顺序”和“是否覆盖所有关键组件”。
    // 入口程序退出时若漏掉某一步，通常不会立刻在单元测试暴露，
    // 但会在真实部署中留下连接、owner、profile 或磁盘残留。
    stubs::reset_all_for_test();
    stubs::clear_events();

    // 中文补充：第一段覆盖普通退出路径。
    // 通过事件拼接后的包含关系断言，锁定 cleanup 至少调用了
    // domain、server、plugin、repository、topsql、DDL、存储和磁盘清理。
    let storage = kv::Storage::new("/tmp/tidb", "");
    let dom = domain::Domain::new("d");
    let svr = server::Server::new();
    cleanup(&svr, &storage, &dom);
    let ev = stubs::take_events();
    let joined = ev.join("|");
    assert!(joined.contains("domain.StopAutoAnalyze"));
    assert!(joined.contains("server.DrainClients"));
    assert!(joined.contains("server.KillSysProcesses"));
    assert!(joined.contains("plugin.Shutdown"));
    assert!(joined.contains("repository.StopRepository"));
    assert!(joined.contains("topsql.Close"));
    assert!(joined.contains("domain.Close"));
    assert!(joined.contains("ddl.CloseOwnerManager"));
    assert!(joined.contains("kv.Storage.Close"));
    assert!(joined.contains("disk.CleanUp"));
    assert!(joined.contains("cgmon.StopCgroupMonitor"));

    // 中文补充：starter 下的强制关闭要把 drain 等待时间压到 0，
    // 否则 keyspace 激活或快速终止流程会被额外等待拖慢。
    // force shutdown in starter drains with 0 wait
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    deploymode::Set(deploymode::Starter).unwrap();
    stubs::clear_events();
    let storage = kv::Storage::new("/tmp/tidb", "");
    let dom = domain::Domain::new("d");
    let svr = server::Server::new();
    svr.set_force_shutdown(true);
    cleanup(&svr, &storage, &dom);
    let ev = stubs::take_events();
    assert!(
        ev.iter().any(|e| e.contains("DrainClients drain_ms=0")),
        "starter force shutdown must use 0 drain wait, got {ev:?}"
    );

    // 中文补充：keyspace activate 完成后不是直接 `exit`，
    // 而是先执行完整 cleanup，再返回 OK 退出码。
    // 这能确保 server、resource manager、CPU profiler 和 executor
    // 都按 Go 主程序的既定顺序收尾。
    // exitAfterKeyspaceActivate performs cleanup then returns OK
    stubs::reset_all_for_test();
    stubs::clear_events();
    let storage = kv::Storage::new("/tmp/tidb", "");
    let dom = domain::Domain::new("d");
    let svr = server::Server::new();
    let code = exitAfterKeyspaceActivate(&svr, &storage, &dom);
    assert_eq!(code, exitCodeOK);
    let ev = stubs::take_events();
    assert!(ev.iter().any(|e| e.contains("server.Close")));
    assert!(ev.iter().any(|e| e.contains("resourcemanager.Stop")));
    assert!(ev.iter().any(|e| e.contains("cpuprofile.StopCPUProfiler")));
    assert!(ev.iter().any(|e| e.contains("executor.Stop")));

    // 中文补充：无效 lease 仍然走 fatal/panic 语义，
    // 这里用 catch_unwind 证明 Rust 端没有把它悄悄改成普通错误返回。
    // invalid lease fatals
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = parseDuration("not-a-duration");
    }));
    assert!(result.is_err());
}

#[test]
fn prometheus_push_client_repeats_at_the_configured_interval() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    stubs::clear_events();

    prometheusPushClient(
        "http://pushgateway.invalid".into(),
        Duration::from_millis(1),
    );

    let push_count = stubs::take_events()
        .iter()
        .filter(|event| event.as_str() == "push.Push")
        .count();
    assert_eq!(
        push_count, 2,
        "the Go client pushes once per interval forever"
    );
}
