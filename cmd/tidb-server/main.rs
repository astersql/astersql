// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! tidb-server 命令入口，负责把 CLI、配置文件与运行时默认值收敛成
//! 单一的全局配置快照，然后按固定顺序拉起存储、DDL、Domain、Server
//! 以及若干后台协程。
//!
//! Rust 版本整体对齐 `cmd/tidb-server/main.go` 的职责分层：
//! 先解析 flag，再覆盖配置，再设置全局变量，最后创建 server 并等待
//! 信号驱动的优雅退出。
//!
//! 这里的大量边界能力通过 `crate::stubs` 提供，以便在 arm64 和单元测试
//! 场景中复用入口语义，而不要求真实依赖全部上线。注释重点说明初始化顺序、
//! 关键约束和与 Go 版保持一致的地方，而不是逐行解释语法。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_config::{StoreTypeMockTiKV, StoreTypeTiKV, StoreTypeUniStore};
use astersql_server::runtime::{
    BootstrapAuthMode, CanonicalConnectionDomain, CanonicalServerDomain, CanonicalServerDriver,
    ConcreteSessionDriver,
};
use astersql_server::server::{
    Server as CanonicalServer, ServerConfig as CanonicalServerConfig,
    StatusConfig as CanonicalStatusConfig,
};
use astersql_session::runtime::ttl_runtime::start_domain_ttl_job_manager;
use astersql_session::runtime::{CanonicalSessionFactory, CreateAnalyzeSession};
use astersql_store as store_registry;
use astersql_store_driver::{PdClientConfig, Security as TiKVSecurity, TiKVDriver};

use crate::stubs::{
    self, Error, Result, Signal, bindinfo, cgmon, chunk, config, copr, cpuprofile, ddl,
    deadlockhistory, deploymode, disk, domain, domainutil, executor, extension, extworkload,
    failpoint, flag, intest, kerneltype, keyspace, kv, kvcache, kvrpcpb, linux, log, logutil,
    maxprocs, memory, metrics, metricsutil, mppcoordmanager, mysql, naming, opentracing,
    parsertypes, plannercore, plugin, printer, privileges, push, pyroscope, redact, repository,
    resourcemanager, sem, semv2, server, session, signal, standby, statistics, stmtsummaryv2,
    storage_sys, syscall, systimemon, tidbmanager, tiflashcompute, tikv, tikvrpc, topsql,
    transaction, txninfo, util, vardef, variable, versioninfo,
};

// 这组常量直接对应 TiDB 的命令行开关名。
// Rust 侧故意保留与 Go 完全一致的字符串，避免配置文件、运维脚本、
// 启动器参数模板和文档出现分叉。
pub const nmVersion: &str = "V";
pub const nmConfig: &str = "config";
pub const nmConfigCheck: &str = "config-check";
pub const nmConfigStrict: &str = "config-strict";
pub const nmStore: &str = "store";
pub const nmStorePath: &str = "path";
pub const nmHost: &str = "host";
pub const nmAdvertiseAddress: &str = "advertise-address";
pub const nmPort: &str = "P";
pub const nmPostgresPort: &str = "postgres-port";
pub const nmCors: &str = "cors";
pub const nmSocket: &str = "socket";
pub const nmRunDDL: &str = "run-ddl";
pub const nmLogLevel: &str = "L";
pub const nmLogFile: &str = "log-file";
pub const nmLogSlowQuery: &str = "log-slow-query";
pub const nmLogGeneral: &str = "log-general";
pub const nmReportStatus: &str = "report-status";
pub const nmStatusHost: &str = "status-host";
pub const nmStatusPort: &str = "status";
pub const nmMetricsAddr: &str = "metrics-addr";
pub const nmMetricsInterval: &str = "metrics-interval";
pub const nmDdlLease: &str = "lease";
pub const nmTokenLimit: &str = "token-limit";
pub const nmPluginDir: &str = "plugin-dir";
pub const nmPluginLoad: &str = "plugin-load";
pub const nmRepairMode: &str = "repair-mode";
pub const nmRepairList: &str = "repair-list";
pub const nmTempDir: &str = "temp-dir";
pub const nmClusterCa: &str = "cluster-ca";
pub const nmClusterCert: &str = "cluster-cert";
pub const nmClusterKey: &str = "cluster-key";
pub const nmSQLCA: &str = "sql-ca";
pub const nmSQLCert: &str = "sql-cert";
pub const nmSQLKey: &str = "sql-key";
pub const nmRedact: &str = "redact";
pub const nmProxyProtocolNetworks: &str = "proxy-protocol-networks";
pub const nmProxyProtocolHeaderTimeout: &str = "proxy-protocol-header-timeout";
pub const nmProxyProtocolFallbackable: &str = "proxy-protocol-fallbackable";
pub const nmAffinityCPU: &str = "affinity-cpus";
pub const nmInitializeSecure: &str = "initialize-secure";
pub const nmInitializeInsecure: &str = "initialize-insecure";
pub const nmInitializeSQLFile: &str = "initialize-sql-file";
pub const nmDisconnectOnExpiredPassword: &str = "disconnect-on-expired-password";
pub const nmKeyspaceName: &str = "keyspace-name";
pub const nmTiDBServiceScope: &str = "tidb-service-scope";
pub const nmStandby: &str = "standby";
pub const nmActivationTimeout: &str = "activation-timeout";
pub const nmMaxIdleSeconds: &str = "max-idle-seconds";
pub const nmKeyspaceActivate: &str = "keyspace-activate";
pub const nmStarterParams: &str = "starter-additional-params";

// 退出码沿用 Go 版约定：
// 0 表示正常退出，1 表示通用错误，SIGINT 使用 128+signal 便于
// 外部 supervisor 判断是否走了强制关闭路径。
pub const exitCodeOK: i32 = 0;
pub const exitCodeErr: i32 = 1;
pub const exitCodeInt: i32 = 128 + syscall::SIGINT;

/// 解析后的 flag 快照。
///
/// Go 版把每个 CLI 参数保存在包级 `*T` 指针中；Rust 版改为集中存入
/// 一个结构体，再放进进程级 `Mutex<Option<_>>`。这样既能维持全局读取
/// 习惯，也更适合测试在一次进程内多次调用入口函数。
#[derive(Clone, Debug)]
pub struct FlagValues {
    // 版本与配置入口相关参数。
    pub version: bool,
    pub configPath: String,
    pub configCheck: bool,
    pub configStrict: bool,
    // 基础网络、存储与插件参数。
    pub store: String,
    pub storePath: String,
    pub host: String,
    pub advertiseAddress: String,
    pub port: String,
    pub postgresPort: String,
    pub cors: String,
    pub socket: String,
    pub runDDL: bool,
    pub ddlLease: String,
    pub tokenLimit: i32,
    pub pluginDir: String,
    pub pluginLoad: String,
    pub affinityCPU: String,
    // 修复模式和临时目录控制运维行为。
    pub repairMode: bool,
    pub repairList: String,
    pub tempDir: String,
    // Starter 部署下 cluster/sql 双套 TLS 分别控制集群侧和 SQL 侧。
    pub clusterCA: String,
    pub clusterCert: String,
    pub clusterKey: String,
    pub sqlCA: String,
    pub sqlCert: String,
    pub sqlKey: String,
    // 日志与状态服务参数会在 overrideConfig 中按“仅覆盖显式传入项”写回。
    pub logLevel: String,
    pub logFile: String,
    pub logSlowQuery: String,
    pub logGeneral: String,
    pub reportStatus: bool,
    pub statusHost: String,
    pub statusPort: String,
    pub metricsAddr: String,
    pub metricsInterval: u32,
    pub redactFlag: bool,
    // PROXY Protocol 和 bootstrap 安全相关参数都有额外约束检查。
    pub proxyProtocolNetworks: String,
    pub proxyProtocolHeaderTimeout: u32,
    pub proxyProtocolFallbackable: bool,
    pub initializeSecure: bool,
    pub initializeInsecure: bool,
    pub initializeSQLFile: String,
    pub disconnectOnExpiredPassword: bool,
    pub keyspaceName: String,
    pub serviceScope: String,
    pub help: bool,
    // standby / keyspace activation 只在 nextgen + starter 语义下使用。
    pub standbyMode: bool,
    pub activationTimeout: u32,
    pub maxIdleSeconds: u32,
    pub keyspaceActivateMode: bool,
    pub starterAdditionalParams: String,
}

impl Default for FlagValues {
    fn default() -> Self {
        Self {
            // 默认值刻意与 Go CLI 保持一致，避免“不传参时”的行为漂移。
            version: false,
            configPath: String::new(),
            configCheck: false,
            configStrict: false,
            store: config::StoreTypeUniStore.into(),
            storePath: "/tmp/tidb".into(),
            host: "0.0.0.0".into(),
            advertiseAddress: String::new(),
            port: "4000".into(),
            postgresPort: String::new(),
            cors: String::new(),
            socket: "/tmp/tidb-{Port}.sock".into(),
            runDDL: true,
            ddlLease: "45s".into(),
            tokenLimit: 1000,
            pluginDir: "/data/deploy/plugin".into(),
            pluginLoad: String::new(),
            affinityCPU: String::new(),
            repairMode: false,
            repairList: String::new(),
            tempDir: config::DefTempDir.into(),
            clusterCA: String::new(),
            clusterCert: String::new(),
            clusterKey: String::new(),
            sqlCA: String::new(),
            sqlCert: String::new(),
            sqlKey: String::new(),
            logLevel: "info".into(),
            logFile: String::new(),
            logSlowQuery: String::new(),
            logGeneral: String::new(),
            reportStatus: true,
            statusHost: "0.0.0.0".into(),
            statusPort: "10080".into(),
            metricsAddr: String::new(),
            metricsInterval: 15,
            redactFlag: false,
            proxyProtocolNetworks: String::new(),
            proxyProtocolHeaderTimeout: 5,
            proxyProtocolFallbackable: false,
            initializeSecure: false,
            initializeInsecure: true,
            initializeSQLFile: String::new(),
            disconnectOnExpiredPassword: true,
            keyspaceName: String::new(),
            serviceScope: String::new(),
            help: false,
            standbyMode: false,
            activationTimeout: 0,
            maxIdleSeconds: 0,
            keyspaceActivateMode: false,
            starterAdditionalParams: String::new(),
        }
    }
}

// 全局 flag 快照只在启动早期写一次，之后主要是只读访问。
// 这里仍保留可覆写能力，方便测试场景在单进程内多轮执行入口。
static FLAGS: Mutex<Option<FlagValues>> = Mutex::new(None);

pub fn flags() -> FlagValues {
    FLAGS.lock().unwrap().clone().unwrap_or_default()
}
pub fn set_flags(f: FlagValues) {
    *FLAGS.lock().unwrap() = Some(f);
}

// starter 参数单独提供 getter/setter，是因为少数路径只关心这一个值，
// 不想把整个结构体从调用链一路向下传递。
pub fn starter_additional_params() -> String {
    flags().starterAdditionalParams
}
pub fn set_starter_additional_params(v: impl Into<String>) {
    let mut g = FLAGS.lock().unwrap();
    let mut f = g.clone().unwrap_or_default();
    f.starterAdditionalParams = v.into();
    *g = Some(f);
}

/// Go 的 `flag.Bool` 在默认值为 false 时不会把“default false”写进帮助文本。
/// 这里手工补齐，确保帮助输出与 Go 版约定一致。
pub fn flagBoolean(fset: &mut flag::FlagSet, name: &str, defaultVal: bool, usage: &str) -> bool {
    if !defaultVal {
        let usage = format!("{usage} (default false)");
        return fset.Bool(name, defaultVal, &usage);
    }
    fset.Bool(name, defaultVal, usage)
}

/// 生产入口从环境变量参数初始化 FlagSet。
/// 单元测试通常走 `initFlagSetWithArgs`，避免依赖真实进程参数。
pub fn initFlagSet() -> flag::FlagSet {
    initFlagSetWithArgs(&stubs::args_from_env())
}

/// 注册全部命令行参数并立刻解析。
///
/// 该函数承担两层职责：
/// 1. 维护与 Go 入口一致的 flag 名称、默认值和帮助文本。
/// 2. 把解析结果搬运进 `FlagValues`，供后续 `overrideConfig` 与启动流程读取。
pub fn initFlagSetWithArgs(argv: &[String]) -> flag::FlagSet {
    let prog = argv.first().map(|s| s.as_str()).unwrap_or("tidb-server");
    let mut fset = flag::NewFlagSet(prog, flag::ExitOnError);
    let mut fv = FlagValues::default();

    // 版本与配置控制参数最先注册，因为它们会决定后续是否继续完整启动。
    let _ = flagBoolean(
        &mut fset,
        nmVersion,
        false,
        "print version information and exit",
    );
    let _ = fset.String(nmConfig, "", "config file path");
    let _ = flagBoolean(
        &mut fset,
        nmConfigCheck,
        false,
        "check config file validity and exit",
    );
    let _ = flagBoolean(
        &mut fset,
        nmConfigStrict,
        false,
        "enforce config file validity",
    );

    // 基础网络/存储/DDL 参数决定 server 对外监听和 schema lease 等核心行为。
    let store_default = config::StoreTypeUniStore;
    let store_usage = format!("registered store name, {:?}", config::StoreTypeList());
    let _ = fset.String(nmStore, store_default, &store_usage);
    let _ = fset.String(nmStorePath, "/tmp/tidb", "tidb storage path");
    let _ = fset.String(nmHost, "0.0.0.0", "tidb server host");
    let _ = fset.String(nmAdvertiseAddress, "", "tidb server advertise IP");
    let _ = fset.String(nmPort, "4000", "tidb server port");
    let _ = fset.String(
        nmPostgresPort,
        "",
        "independent PostgreSQL TCP port (disabled when omitted)",
    );
    let _ = fset.String(nmCors, "", "tidb server allow cors origin");
    let _ = fset.String(
        nmSocket,
        "/tmp/tidb-{Port}.sock",
        "The socket file to use for connection.",
    );
    let _ = flagBoolean(
        &mut fset,
        nmRunDDL,
        true,
        "run ddl worker on this tidb-server",
    );
    let _ = fset.String(
        nmDdlLease,
        "45s",
        "schema lease duration, very dangerous to change only if you know what you do",
    );
    let _ = fset.Int(
        nmTokenLimit,
        1000,
        "the limit of concurrent executed sessions",
    );
    let _ = fset.String(
        nmPluginDir,
        "/data/deploy/plugin",
        "the folder that hold plugin",
    );
    let _ = fset.String(
        nmPluginLoad,
        "",
        "wait load plugin name(separated by comma)",
    );
    let _ = fset.String(
        nmAffinityCPU,
        "",
        "affinity cpu (cpu-no. separated by comma, e.g. 1,2,3)",
    );
    let _ = flagBoolean(&mut fset, nmRepairMode, false, "enable admin repair mode");
    let _ = fset.String(nmRepairList, "", "admin repair table list");
    let _ = fset.String(nmTempDir, config::DefTempDir, "tidb temporary directory");
    let _ = fset.String(nmClusterCa, "", "cluster CA file path");
    let _ = fset.String(nmClusterCert, "", "cluster cert file path");
    let _ = fset.String(nmClusterKey, "", "cluster key file path");
    let _ = fset.String(nmSQLCA, "", "SQL CA file path");
    let _ = fset.String(nmSQLCert, "", "SQL cert file path");
    let _ = fset.String(nmSQLKey, "", "SQL key file path");

    // 日志和状态端口独立于 SQL 服务端口，便于监控与排障。
    let _ = fset.String(
        nmLogLevel,
        "info",
        "log level: info, debug, warn, error, fatal",
    );
    let _ = fset.String(nmLogFile, "", "log file path");
    let _ = fset.String(nmLogSlowQuery, "", "slow query file path");
    let _ = fset.String(nmLogGeneral, "", "general log file path");

    let _ = flagBoolean(
        &mut fset,
        nmReportStatus,
        true,
        "If enable status report HTTP service.",
    );
    let _ = fset.String(nmStatusHost, "0.0.0.0", "tidb server status host");
    let _ = fset.String(nmStatusPort, "10080", "tidb server status port");
    let _ = fset.String(
        nmMetricsAddr,
        "",
        "prometheus pushgateway address, leaves it empty will disable prometheus push.",
    );
    let _ = fset.Uint(
        nmMetricsInterval,
        15,
        "prometheus client push interval in second, set \"0\" to disable prometheus push.",
    );

    // `collect-log` 子命令会读取这个开关决定是否去敏。
    let _ = flagBoolean(
        &mut fset,
        nmRedact,
        false,
        "remove sensitive words from marked tidb logs when using collect-log subcommand, e.g. ./tidb-server --redact=xxx collect-log <input> <output>",
    );

    let _ = fset.String(
        nmProxyProtocolNetworks,
        "",
        "proxy protocol networks allowed IP or *, empty mean disable proxy protocol support",
    );
    let _ = fset.Uint(nmProxyProtocolHeaderTimeout, 5, "proxy protocol header read timeout, unit is second. (Deprecated: as proxy protocol using lazy mode, header read timeout no longer used)");
    let _ = flagBoolean(
        &mut fset,
        nmProxyProtocolFallbackable,
        false,
        "enable proxy protocol fallback mode. If it is enabled, connection will return the client IP address when the client does not send PROXY Protocol Header and it will not return any error. (Note: This feature it does NOT follow the PROXY Protocol SPEC)",
    );

    // Bootstrap/安全/nextgen keyspace 相关参数聚在一起，便于统一做互斥校验。
    let _ = flagBoolean(
        &mut fset,
        nmInitializeSecure,
        false,
        "bootstrap tidb-server in secure mode",
    );
    let _ = flagBoolean(
        &mut fset,
        nmInitializeInsecure,
        true,
        "bootstrap tidb-server in insecure mode",
    );
    let _ = fset.String(
        nmInitializeSQLFile,
        "",
        "SQL file to execute on first bootstrap",
    );
    let _ = flagBoolean(
        &mut fset,
        nmDisconnectOnExpiredPassword,
        true,
        "the server disconnects the client when the password is expired",
    );
    let _ = fset.String(nmKeyspaceName, "", "keyspace name.");
    let _ = fset.String(nmTiDBServiceScope, "", "tidb service scope");
    let _ = fset.Bool("help", false, "show the usage");

    let _ = flagBoolean(&mut fset, nmStandby, false, "start tidb-server as standby");
    let _ = fset.Uint(
        nmActivationTimeout,
        0,
        "max time in second allowed for tidb to activate from standby, 0 means no limit",
    );
    let _ = fset.Uint(
        nmMaxIdleSeconds,
        0,
        "max idle seconds for a connection, 0 means no limit",
    );
    let _ = flagBoolean(
        &mut fset,
        nmKeyspaceActivate,
        false,
        "exit after activating the keyspace",
    );
    let _ = fset.String(
        nmStarterParams,
        "",
        "starter additional params in k=v,k=v format",
    );

    // 与 Go 版一致，在 flag 解析前先注册 session 升级相关测试开关。
    session::RegisterMockUpgradeFlag(&mut fset);
    let parse_args: Vec<String> = if argv.len() > 1 {
        argv[1..].to_vec()
    } else {
        Vec::new()
    };
    let _ = fset.Parse(&parse_args);

    // `FlagSet` 自身只保留字符串接口，后续流程需要一个可复制快照。
    // 这里把所有已解析值提取到 `FlagValues`，避免后续逻辑再次访问 flag API。
    fv.version = fset.LookupBool(nmVersion);
    fv.configPath = fset.LookupString(nmConfig);
    fv.configCheck = fset.LookupBool(nmConfigCheck);
    fv.configStrict = fset.LookupBool(nmConfigStrict);
    fv.store = fset.LookupString(nmStore);
    fv.storePath = fset.LookupString(nmStorePath);
    fv.host = fset.LookupString(nmHost);
    fv.advertiseAddress = fset.LookupString(nmAdvertiseAddress);
    fv.port = fset.LookupString(nmPort);
    fv.postgresPort = fset.LookupString(nmPostgresPort);
    fv.cors = fset.LookupString(nmCors);
    fv.socket = fset.LookupString(nmSocket);
    fv.runDDL = fset.LookupBool(nmRunDDL);
    fv.ddlLease = fset.LookupString(nmDdlLease);
    fv.tokenLimit = fset.LookupInt(nmTokenLimit);
    fv.pluginDir = fset.LookupString(nmPluginDir);
    fv.pluginLoad = fset.LookupString(nmPluginLoad);
    fv.affinityCPU = fset.LookupString(nmAffinityCPU);
    fv.repairMode = fset.LookupBool(nmRepairMode);
    fv.repairList = fset.LookupString(nmRepairList);
    fv.tempDir = fset.LookupString(nmTempDir);
    fv.clusterCA = fset.LookupString(nmClusterCa);
    fv.clusterCert = fset.LookupString(nmClusterCert);
    fv.clusterKey = fset.LookupString(nmClusterKey);
    fv.sqlCA = fset.LookupString(nmSQLCA);
    fv.sqlCert = fset.LookupString(nmSQLCert);
    fv.sqlKey = fset.LookupString(nmSQLKey);
    fv.logLevel = fset.LookupString(nmLogLevel);
    fv.logFile = fset.LookupString(nmLogFile);
    fv.logSlowQuery = fset.LookupString(nmLogSlowQuery);
    fv.logGeneral = fset.LookupString(nmLogGeneral);
    fv.reportStatus = fset.LookupBool(nmReportStatus);
    fv.statusHost = fset.LookupString(nmStatusHost);
    fv.statusPort = fset.LookupString(nmStatusPort);
    fv.metricsAddr = fset.LookupString(nmMetricsAddr);
    fv.metricsInterval = fset.LookupUint(nmMetricsInterval);
    fv.redactFlag = fset.LookupBool(nmRedact);
    fv.proxyProtocolNetworks = fset.LookupString(nmProxyProtocolNetworks);
    fv.proxyProtocolHeaderTimeout = fset.LookupUint(nmProxyProtocolHeaderTimeout);
    fv.proxyProtocolFallbackable = fset.LookupBool(nmProxyProtocolFallbackable);
    fv.initializeSecure = fset.LookupBool(nmInitializeSecure);
    fv.initializeInsecure = fset.LookupBool(nmInitializeInsecure);
    fv.initializeSQLFile = fset.LookupString(nmInitializeSQLFile);
    fv.disconnectOnExpiredPassword = fset.LookupBool(nmDisconnectOnExpiredPassword);
    fv.keyspaceName = fset.LookupString(nmKeyspaceName);
    fv.serviceScope = fset.LookupString(nmTiDBServiceScope);
    fv.help = fset.LookupBool("help");
    fv.standbyMode = fset.LookupBool(nmStandby);
    fv.activationTimeout = fset.LookupUint(nmActivationTimeout);
    fv.maxIdleSeconds = fset.LookupUint(nmMaxIdleSeconds);
    fv.keyspaceActivateMode = fset.LookupBool(nmKeyspaceActivate);
    fv.starterAdditionalParams = fset.LookupString(nmStarterParams);
    set_flags(fv);

    // 帮助模式下 Go 会直接退出；Rust 为了可测试性只记录事件并由上层返回。
    if flags().help {
        fset.Usage();
        // Go: os.Exit(0). In library/tests we return after recording.
        stubs::record_event("main.help-exit");
    }
    fset
}

/// 把 deploy-mode 写入进程全局状态。
/// 只有 nextgen 入口需要这一步，因为后续许多配置校验与行为分支都依赖它。
pub fn initDeployMode(cfg: &config::Config) -> Result<()> {
    deploymode::Set(cfg.DeployMode)
}

/// 为 starter 部署模式初始化外部 workload manager。
///
/// 这条能力链路是“尽力而为”：
/// - 配置未开启时直接跳过；
/// - keyspace 元信息缺失时记录告警但不阻塞；
/// - manager 初始化失败时也允许 TiDB 继续启动。
/// 这样可以避免外围协调组件的短暂异常放大为 TiDB 整体不可用。
pub fn initExternalWorkloadManager(
    _ctx: (),
    storage: &kv::Storage,
) -> Option<extworkload::Manager> {
    let cfg = config::GetGlobalConfig().ExternalWorkload;
    if !cfg.Enable {
        return None;
    }
    let meta = storage.GetCodec().GetKeyspaceMeta();
    let Some(meta) = meta else {
        logutil::BgLogger::Warn(
            "external workload controller enabled but keyspace meta is unavailable; TiDB will continue without external workload coordination",
        );
        return None;
    };
    match extworkload::NewManager((), meta, cfg) {
        Ok(mgr) => Some(mgr),
        Err(_) => {
            logutil::BgLogger::Warn(
                "failed to initialize external workload manager; TiDB will continue without external workload coordination",
            );
            None
        }
    }
}

/// 关闭外部 workload manager。
/// 关闭失败只打告警，因为进程已经进入收尾阶段，不能让清理错误覆盖主退出路径。
pub fn closeExternalWorkloadManager(mgr: Option<extworkload::Manager>) {
    if let Some(mgr) = mgr {
        if let Err(err) = mgr.Close() {
            logutil::BgLogger::Warn(&format!("failed to close external workload manager: {err}"));
        }
    }
}

/// Binary / library entry matching Go `main`.
pub fn main() {
    let _ = run_main(&stubs::args_from_env());
}

/// Testable entry. Returns process exit code (does not call process::exit except help/version paths when `exit_process` is true).
pub fn run_main(argv: &[String]) -> i32 {
    run_main_inner(argv, false)
}

/// `tidb-server` 的主启动流程。
///
/// 顺序基本与 Go `main` 保持一致：
/// 1. 解析命令行与配置。
/// 2. 根据 kernel/deploy mode 做早期校验。
/// 3. 注册存储、日志、扩展、全局变量和观测能力。
/// 4. 创建 storage/domain/server，安装信号处理器并阻塞运行。
/// 5. 收到退出信号后按固定次序关闭子系统，最后决定退出码。
pub fn run_main_inner(argv: &[String], exit_process: bool) -> i32 {
    let fset = initFlagSetWithArgs(argv);
    let args = fset.Args();
    // `collect-log` 是一个轻量子命令，只依赖去敏逻辑，不需要完整启动 TiDB。
    if !args.is_empty() && args[0] == "collect-log" && args.len() > 1 {
        let output = if args.len() > 2 {
            args[2].clone()
        } else {
            "-".into()
        };
        // 子命令直接复用入口的 redact flag，不再单独维护第二套解析逻辑。
        stubs::must_nil_result(redact::DeRedactFile(flags().redactFlag, &args[1], &output));
        return exitCodeOK;
    }
    if flags().help {
        // 帮助模式只展示 usage，不触发任何全局初始化副作用。
        return exitCodeOK;
    }

    // 配置初始化会读取文件、执行校验，并通过 `overrideConfig` 叠加显式 CLI 参数。
    let fv = flags();
    config::InitializeConfig(
        &fv.configPath,
        fv.configCheck,
        fv.configStrict,
        overrideConfig,
        &fset,
    );
    if kerneltype::IsNextGen() {
        // deploy mode 一旦写入，全局 helper 就会按 nextgen/starter/classic 语义分支。
        stubs::must_nil_result(initDeployMode(&config::GetGlobalConfig()));
    }
    // `-V` 需要先完成版本派生，再输出最终对外展示的版本字符串。
    if fv.version {
        mustInitVersions();
        println!("{}", printer::GetTiDBInfo());
        if exit_process {
            std::process::exit(0);
        }
        return exitCodeOK;
    }

    let gcfg = config::GetGlobalConfig();
    // 这组约束不能提前塞进 `config.Valid()`，因为 `-V` 也依赖全局配置已初始化。
    // 因此入口在完成配置初始化后、真正启动前做额外门禁。
    if kerneltype::IsNextGen() && gcfg.KeyspaceName.is_empty() && !gcfg.Standby.StandByMode {
        eprintln!("invalid config: keyspace name or standby mode is required for nextgen TiDB");
        return exitCodeOK;
    } else if kerneltype::IsClassic()
        && (!gcfg.KeyspaceName.is_empty() || gcfg.Standby.StandByMode || gcfg.KeyspaceActivateMode)
    {
        eprintln!(
            "invalid config: keyspace name, standby mode or keyspace-activate mode is not supported for classic TiDB"
        );
        return exitCodeOK;
    }

    // request origin 会进入下游 TiKV 请求上下文，便于链路侧识别来源组件。
    tikvrpc::SetDefaultRequestOrigin(kvrpcpb::RequestOrigin_RequestOriginTiDB);

    // standby 模式下 server 不是立即启动，而是等待外部激活请求后再继续。
    let mut standbyController: Option<server::StandbyController> = None;
    let mut activationMetadata = HashMap::new();
    if config::GetGlobalConfig().Standby.StandByMode {
        // standby 通过 starter manager 构造激活控制器，而不是立刻开放 SQL 服务。
        let mgrCli = stubs::must_nil_result(createMgrClientForStarter());
        standbyController = Some(standby::NewLoadKeyspaceController(mgrCli));
    }

    // 被激活后需要重新校验配置，因为 standby 期间可能已经由控制面更新配置。
    if let Some(controller) = standbyController.as_mut() {
        controller.WaitForActivate();
        // 激活后重新跑 `Valid()`，确保控制面修改没有留下非法配置组合。
        stubs::must_nil_result(config::GetGlobalConfig().Valid());
        if let Some(c) = controller.AsLoadKeyspaceController() {
            // 激活元数据稍后会被注入 observability 配置和 workload manager。
            activationMetadata = c.ActivationMetadata();
        }
    }

    // USR1 handler 一般用于在线诊断或 dump，需在大部分后台模块启动前就注册。
    signal::SetupUSR1Handler();
    // 存储驱动必须先注册，后面构造 storage/domain 时才知道如何按 store 类型打开。
    stubs::must_nil_result(registerStores());
    if deploymode::IsStarter() {
        // starter 需要把激活元信息合并进观测标签，保证指标/日志能区分 keyspace。
        stubs::must_nil_result(prepareKeyspaceObservabilityForStarter(activationMetadata));
    }
    stubs::must_nil_result(metricsutil::RegisterMetrics());

    // 临时存储目录只在 OOM spill 开关打开时才做初始化和容量校验。
    if vardef::EnableTmpStorageOnOOM_Load() {
        // 临时目录可能依赖当前 keyspace/path 配置重新拼接，因此这里现算现写。
        config::UpdateGlobal(|c| c.UpdateTempStoragePath());
        stubs::must_nil_result(disk::InitializeTempDir());
        stubs::must_nil_result(checkTempStorageQuota());
    }
    // 日志初始化必须早于后续绝大多数组件，否则启动期错误无法按统一格式输出。
    stubs::must_nil_result(setupLog());
    // memory hook 会影响 OOM/内存跟踪行为，应尽早挂载。
    stubs::must_nil_result(memory::InitMemoryHook());
    let _ = stubs::must_nil_result(setupExtensions());
    // 语句摘要、CPU profiler 和自动伸缩拓扑抓取都属于启动后常驻背景能力。
    setupStmtSummary();
    stubs::must_nil_result(cpuprofile::StartCPUProfiler());

    let gcfg = config::GetGlobalConfig();
    if gcfg.DisaggregatedTiFlash && gcfg.UseAutoScaler {
        // 仅在 disaggregated tiflash + autoscaler 同时开启时，才需要全局拓扑抓取器。
        stubs::must_nil_result(tiflashcompute::InitGlobalTopoFetcher(
            &gcfg.TiFlashComputeAutoScalerType,
            &gcfg.TiFlashComputeAutoScalerAddr,
            &gcfg.AutoScalerClusterID,
            gcfg.IsTiFlashComputeFixedPool,
        ));
    }

    // failpoint 状态必须在任何 client-go 实际使用前决定，避免并发竞态。
    if failpoint::Status("github.com/pingcap/tidb/pkg/server/enableTestAPI").is_ok() {
        logutil::BgLogger::Warn(
            "tikv/client-go failpoint is enabled, this should NOT happen in the production environment",
        );
        tikv::EnableFailpoints();
    }
    // UniStore 使用伪造地址，不适合真实 gRPC 健康检查，因此强制注入“可达”结果。
    if config::GetGlobalConfig().Store == config::StoreTypeUniStore {
        tikv::EnableFailpoints();
        if failpoint::Enable("tikvclient/injectLiveness", r#"return("reachable")"#).is_err() {
            logutil::BgLogger::Warn("failed to enable tikvclient/injectLiveness for unistore");
        }
    }
    if intest::EnableInternalCheck {
        // internal check 主要面向测试/研发环境，生产启用需要显式警告。
        logutil::BgLogger::Warn(
            "internal check is enabled, this should NOT happen in the production environment",
        );
    }

    // 这一步把配置投影到大量运行时原子变量/全局 sysvar，必须在真正建站前完成。
    setGlobalVars();
    // SEM 和 CPU 亲和性都依赖最终配置，因此放在全局变量投影之后。
    setupSEM();
    stubs::must_nil_result(setCPUAffinity());
    cgmon::StartCgroupMonitor();
    // tracing 要放在 createServer 前，确保后续组件都能拿到全局 tracer。
    stubs::must_nil_result(setupTracing());
    printInfo();
    setupMetrics();

    // storage/domain/server 的创建顺序不可交换：
    // Domain 依赖 storage，Server 又需要 Domain 中的会话与元信息能力。
    let keyspaceName = keyspace::GetKeyspaceNameBySettings();
    // 执行器与资源管理器需要在 server run 前先启动背景工作线程。
    executor::Start();
    resourcemanager::InstanceResourceManager.Start();
    let (storage, dom) = stubs::must_nil_result(createStoreDDLOwnerMgrAndDomain(&keyspaceName));
    // workload repository 依赖 Domain 暴露的元信息和会话事件流。
    repository::SetupRepository(&dom);
    let mut external_mgr = None;
    if deploymode::IsStarter() {
        // 外部 workload manager 只在 starter 路径启用，classic/普通 nextgen 不需要。
        external_mgr = initExternalWorkloadManager((), &storage);
    }
    let mut svr = createServer(&storage, &dom);
    // standby controller 挂到 server 上后，才能把激活后的回调和状态转换接入连接层。
    if let Some(controller) = standbyController {
        svr.StandbyController = Some(controller);
        let ctrl = svr.StandbyController.as_ref().unwrap();
        stubs::must_nil_result(ctrl.PrepareForActivation(&svr));
        ctrl.OnServerCreated(&svr);
    }
    // keyspace activate 模式只负责完成激活后立即退出，不进入长期服务循环。
    if deploymode::IsStarter() && config::GetGlobalConfig().KeyspaceActivateMode {
        let code = exitAfterKeyspaceActivate(&svr, &storage, &dom);
        closeExternalWorkloadManager(external_mgr);
        if exit_process {
            std::process::exit(code);
        }
        return code;
    }

    let exit_code = Arc::new(Mutex::new(exitCodeOK));
    // 这些 clone 让信号处理闭包持有独立引用，不依赖外层栈帧生命周期。
    let exit_code_h = exit_code.clone();
    let svr_h = svr.clone();
    let storage_h = storage.clone();
    let dom_h = dom.clone();
    signal::SetupSignalHandler(move |sig| {
        // 信号处理遵循“先拒绝新流量，再停后台模块，再做底层资源清理”的顺序。
        svr_h.Close();
        resourcemanager::InstanceResourceManager.Stop();
        cleanup(&svr_h, &storage_h, &dom_h);
        cpuprofile::StopCPUProfiler();
        executor::Stop();
        *exit_code_h.lock().unwrap() = exitCodeForSignal(sig);
    });
    // TopSQL profiling 要在 server/run 之前安装，才能覆盖后续所有连接流量。
    topsql::SetupTopProfiling(keyspace::GetKeyspaceNameBytesBySettings(), &svr, &dom);

    // In test/immediate mode, deliver SIGTERM so Run unblocks via Close in handler.
    if std::env::var("ASTERSQL_TIDB_SERVER_IMMEDIATE_EXIT").is_ok() {
        // 测试环境通过注入一个延迟 SIGTERM，验证退出链路而不必真的常驻运行。
        let svr2 = svr.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            signal::deliver(Signal::SIGTERM);
            svr2.Close();
        });
    }

    // `Run` 返回表示 listener 已结束；真正的退出码仍以信号回调记录结果为准。
    stubs::must_nil_result(svr.Run(&dom));
    // 等待信号处理路径把清理动作跑完，避免主线程过早返回。
    signal::wait_exited();
    closeExternalWorkloadManager(external_mgr);

    // 日志刷盘失败说明退出链路不完整，即使业务上已停服也应返回通用错误码。
    let mut code = *exit_code.lock().unwrap();
    if syncLog().is_err() {
        // 这里不保留原信号码，而是统一折叠成通用错误码，强调“收尾失败”。
        code = exitCodeErr;
    }
    if code != exitCodeOK && exit_process {
        // 只有最外层真实二进制入口才执行进程级退出；测试调用保留返回值即可。
        std::process::exit(code);
    }
    // 返回值让库态调用方可以断言完整启动/退出链路的最终结果。
    code
}

/// 将信号转换成进程退出码。
/// 只有 SIGINT 需要特殊编码，便于外部脚本识别“强制收敛”场景。
pub fn exitCodeForSignal(sig: Signal) -> i32 {
    if sig == Signal::SIGINT {
        return exitCodeInt;
    }
    exitCodeOK
}

/// keyspace 激活模式的收尾路径。
/// 它与正常退出复用同一套清理顺序，只是不进入长期监听循环。
pub fn exitAfterKeyspaceActivate(
    svr: &server::Server,
    storage: &kv::Storage,
    dom: &domain::Domain,
) -> i32 {
    logutil::BgLogger::Info("keyspace activation completed, exiting");
    let mut exit_code = exitCodeOK;
    // 激活成功后不接受新连接，直接走一次完整的停机清理。
    svr.Close();
    resourcemanager::InstanceResourceManager.Stop();
    cleanup(svr, storage, dom);
    cpuprofile::StopCPUProfiler();
    executor::Stop();
    if syncLog().is_err() {
        exit_code = exitCodeErr;
    }
    exit_code
}

/// 封装日志刷盘。
/// `/dev/stdout` 的 fsync 在很多环境会返回无意义错误，因此按 Go 版忽略。
pub fn syncLog() -> Result<()> {
    match log::Sync() {
        Ok(()) => Ok(()),
        Err(err) => {
            // stdout 通常不是可 fsync 的普通文件，这里保持兼容性忽略。
            if err.msg.contains("/dev/stdout") {
                return Ok(());
            }
            eprintln!("sync log err: {err}");
            Err(err)
        }
    }
}

/// 检查临时存储 quota 是否超过目标目录容量。
/// 只有在启用 spill-to-disk 时，这个错误才值得阻塞启动。
pub fn checkTempStorageQuota() -> Result<()> {
    let c = config::GetGlobalConfig();
    if c.TempStorageQuota >= 0 {
        // quota 为负表示不限制；非负时才需要和真实磁盘容量比较。
        let capacityByte = storage_sys::GetTargetDirectoryCapacity(&c.TempStoragePath)?;
        if capacityByte < c.TempStorageQuota as u64 {
            return Err(Error::new(format!(
                "value of [tmp-storage-quota]({} byte) exceeds the capacity({} byte) of the [{}] directory",
                c.TempStorageQuota, capacityByte, c.TempStoragePath
            )));
        }
    }
    Ok(())
}

/// 解析并应用 CPU 亲和性配置。
/// 这里不会主动缩减线程池大小，只在 CPU 数少于并行度时记录提醒。
pub fn setCPUAffinity() -> Result<()> {
    let raw = flags().affinityCPU;
    if raw.is_empty() {
        return Ok(());
    }
    let mut cpu = Vec::new();
    // 允许用户写成 `1, 2,3` 这种混合空格格式，这里统一 trim 后解析。
    for af in raw.split(',') {
        let af = af.trim();
        if !af.is_empty() {
            match af.parse::<i32>() {
                Ok(c) => cpu.push(c),
                Err(err) => {
                    eprint!("wrong affinity cpu config: {raw}");
                    return Err(Error::new(err.to_string()));
                }
            }
        }
    }
    if let Err(err) = linux::SetAffinity(&cpu) {
        // 亲和性设置失败要阻塞启动，因为实例的 CPU 隔离预期已被破坏。
        eprint!("set cpu affinity failure: {err}");
        return Err(err);
    }
    let maxprocs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    if cpu.len() < maxprocs {
        // Rust stub 不像 Go 那样调整 GOMAXPROCS，只记录“绑核数少于并行度”这一事实。
        log::Info("cpu number less than maxprocs");
    }
    Ok(())
}

/// 注册默认 TiKV 驱动。
/// 单独拆出 `registerStoresWithTiKVDriver` 是为了给测试注入受控驱动实例。
pub fn registerStores() -> Result<()> {
    registerStoresWithTiKVDriver(TiKVDriver::default())
}

/// Testable registration boundary: production passes `TiKVDriver::default`;
/// focused tests may inject a real TiKVDriver with a scripted network backend.
#[doc(hidden)]
pub fn registerStoresWithTiKVDriver(mut tikv_driver: TiKVDriver) -> Result<()> {
    let cfg = config::GetGlobalConfig();
    // cluster TLS 会直接影响 TiKV/PD 客户端出站连接，必须在 register 前灌入驱动。
    tikv_driver.security = TiKVSecurity {
        cluster_ssl_ca: cfg.Security.ClusterSSLCA,
        cluster_ssl_cert: cfg.Security.ClusterSSLCert,
        cluster_ssl_key: cfg.Security.ClusterSSLKey,
    };
    let pd_timeout = stubs::parse_go_duration(&cfg.TiKVClient.StoreLivenessTimeout)
        .map(|timeout| timeout.as_secs().max(1))
        .unwrap_or_else(|_| PdClientConfig::default().pd_server_timeout);
    // PD 超时最小钳到 1 秒，避免异常配置把 client 置于几乎立即超时的状态。
    tikv_driver.pd_config = PdClientConfig {
        pd_server_timeout: pd_timeout,
    };
    // TiKVDriver::Open deliberately reloads process configuration before
    // applying per-open options, matching Go. Keep that source synchronized
    // with tidb-server's already-finalized CLI/config values.
    // 因此这里还要把 tidb-server 已经最终确定的全局配置回写给 store driver，
    // 避免 driver 在 Open 时看到陈旧配置。
    let mut driver_config = astersql_store_driver::get_global_config();
    driver_config.path = cfg.Path;
    driver_config.security = tikv_driver.security.clone();
    driver_config.tikv_client = tikv_driver.tikv_config.clone();
    driver_config.txn_local_latches = tikv_driver.txn_local_latches.clone();
    driver_config.pd_client = tikv_driver.pd_config.clone();
    astersql_store_driver::set_global_config(driver_config);

    // 三类 store 都要在入口统一注册，后续 `store_registry::New` 才能按 scheme 打开。
    store_registry::Register(
        StoreTypeTiKV,
        Arc::new(store_registry::TiKVStoreDriver::new(tikv_driver)),
    )
    .map_err(|error| Error::new(error.to_string()))?;
    // 事件记录主要服务测试，证明注册顺序与 Go 入口一致。
    stubs::record_event("kvstore.Register tikv");
    store_registry::Register(
        StoreTypeMockTiKV,
        Arc::new(store_registry::LocalStoreDriver),
    )
    .map_err(|error| Error::new(error.to_string()))?;
    stubs::record_event("kvstore.Register mocktikv");
    store_registry::Register(
        StoreTypeUniStore,
        Arc::new(store_registry::LocalStoreDriver),
    )
    .map_err(|error| Error::new(error.to_string()))?;
    stubs::record_event("kvstore.Register unistore");
    Ok(())
}

/// 通过 store registry 打开目标 keyspace 的底层 storage。
/// nextgen 下用户 keyspace 之外还需要显式准备 SYSTEM storage。
fn initRegisteredStorage(keyspaceName: &str) -> Result<kv::Storage> {
    let cfg = config::GetGlobalConfig();
    let full_path = store_registry::BuildStoragePath(&cfg.Store, &cfg.Path, keyspaceName);
    let registered = store_registry::New(&full_path)
        .map_err(|error| Error::new(format!("initialize storage: {error}")))?;
    // `full_path` 已经把 store 类型、基础路径和 keyspace 名拼成最终定位串。

    if kerneltype::IsNextGen() {
        // SYSTEM keyspace 由全局单例持有，供用户 keyspace 之外的公共元数据访问。
        if keyspaceName == "SYSTEM" {
            store_registry::SetSystemStorage(Some(registered.clone()));
        } else {
            let system_path = store_registry::BuildStoragePath(&cfg.Store, &cfg.Path, "SYSTEM");
            let system = store_registry::New(&system_path)
                .map_err(|error| Error::new(format!("initialize system storage: {error}")))?;
            store_registry::SetSystemStorage(Some(system));
        }
    }

    Ok(kv::Storage::from_registered(
        &full_path,
        keyspaceName,
        registered,
    ))
}

/// 创建 storage、DDL owner manager 与 Domain。
/// 这是 SQL 层真正“可用”的前置条件，任何一步失败都必须阻塞启动。
pub fn createStoreDDLOwnerMgrAndDomain(
    keyspaceName: &str,
) -> Result<(kv::Storage, domain::Domain)> {
    if config::GetGlobalConfig().Store == config::StoreTypeUniStore {
        // UniStore 是单机嵌入式模式，需要标记成 stand-alone TiDB。
        kv::StandAloneTiDB.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let storage = initRegisteredStorage(keyspaceName)?;
    if let Some(tikvStore) = storage.AsStorageWithPD() {
        // 只有带 PD 能力的 storage 才能做内核类型探测。
        if let Some(pdhttpCli) = tikvStore.GetPDHTTPClient() {
            // 内核类型必须与 PD 侧一致，避免 classic/nextgen 混连导致语义漂移。
            let pdStatus = pdhttpCli.GetStatus(())?;
            if !kerneltype::IsMatch(&pdStatus.KernelType) {
                log::Error("kernel type mismatch");
                return Err(Error::new("kernel type mismatch"));
            }
        }
    }
    // 这些后台组件需要在 session bootstrap 前就处于可用状态。
    copr::GlobalMPPFailedStoreProber.Run();
    mppcoordmanager::InstanceMPPCoordinatorManager.Run();
    ddl::StartOwnerManager((), &storage)?;
    // BootstrapSession 会装配 infoschema、系统库和会话级元信息入口。
    let dom = session::BootstrapSession(&storage)?;
    Ok((storage, dom))
}

/// 统一表示“禁用 prometheus push”。
/// 直接用零 duration，而不是 `Option<Duration>`，是为了贴近 Go 版接口形态。
pub const zeroDuration: Duration = Duration::from_secs(0);

/// 启动 Prometheus Pushgateway 上报线程。
/// 与 Go 版本一样，后台客户端会永久按 interval 循环上报。
pub fn pushMetric(addr: &str, interval: Duration) {
    if interval == zeroDuration || addr.is_empty() {
        // 任一条件成立都视为用户关闭 Pushgateway 上报。
        log::Info("disable Prometheus push client");
        return;
    }
    log::Info("start prometheus push client");
    let addr = addr.to_string();
    std::thread::spawn(move || {
        prometheusPushClient(addr, interval);
    });
}

/// 按固定间隔持续推送指标。
/// `instance` 标签必须稳定，才能让多个 TiDB 实例在 Pushgateway 中正确分组。
pub fn prometheusPushClient(addr: String, interval: Duration) {
    let job = "tidb";
    // job 固定为 `tidb`，实例区分依赖 grouping 的 `instance` 标签。
    let mut pusher = push::New(&addr, job);
    // gatherer 与 grouping 的装配顺序保持和 Go 一致，便于对照行为。
    pusher = pusher.Gatherer(prometheus_default());
    pusher = pusher.Grouping("instance", &instanceName());
    #[cfg(test)]
    let mut pushes = 0;
    loop {
        if let Err(err) = pusher.Push() {
            // 指标上报失败只记日志，不影响主服务可用性，也不终止后续重试。
            log::Error(&format!(
                "could not push metrics to prometheus pushgateway: {err}"
            ));
        }
        #[cfg(test)]
        {
            pushes += 1;
            if pushes == 2 {
                break;
            }
        }
        std::thread::sleep(interval);
    }
}

/// 取默认 gatherer。
/// 单独包一层是为了让 stub 版本保持与 Go 调用点相同的抽象边界。
fn prometheus_default() {
    crate::stubs::prometheus::DefaultGatherer()
}

/// 生成 Prometheus push 的实例名。
/// 形式与 Go 一致：`hostname_port`，hostname 取不到时退化成 `unknown`。
pub fn instanceName() -> String {
    let cfg = config::GetGlobalConfig();
    match hostname() {
        // 端口来自最终全局配置，因此与真实监听端口保持一致。
        Ok(h) => format!("{h}_{}", cfg.Port),
        Err(_) => "unknown".into(),
    }
}

/// 获取主机名。
/// 这里走轻量实现，优先环境变量，失败时退化为 `localhost`。
fn hostname() -> Result<String> {
    // 单独封装后，未来若接入更真实的平台 API，不必改调用方。
    hostname::get_hostname()
}

mod hostname {
    use super::*;
    // 内嵌模块只是为了把平台相关细节局部化，不把入口文件污染成条件编译拼图。
    pub fn get_hostname() -> Result<String> {
        // std::env hostname via nix-less approach
        // 测试环境通常只提供 HOSTNAME 环境变量，因此先走最便宜的路径。
        std::env::var("HOSTNAME")
            .or_else(|_| {
                // try unix gethostname via libc-free: read /etc/hostname or use "localhost"
                Ok::<String, std::env::VarError>("localhost".into())
            })
            .map_err(|e| Error::new(e.to_string()))
    }
}

/// 解析 Go 风格 duration。
/// 为兼容历史用法，如果用户只写数字，这里会补一个 `s` 再尝试解析。
pub fn parseDuration(lease: &str) -> Duration {
    let mut parsed = stubs::parse_go_duration(lease);
    if parsed.is_err() {
        parsed = stubs::parse_go_duration(&format!("{lease}s"));
    }
    match parsed {
        Ok(dur) => dur,
        Err(_) => {
            log::Fatal(&format!("invalid lease duration lease={lease}"));
        }
    }
}

/// 用显式传入的 CLI 参数覆盖配置文件。
///
/// 关键点不是“把所有 flag 复制到 config”，而是只覆盖用户明确传入的项。
/// 这样配置文件仍是大多数字段的事实来源，CLI 只作为高优先级增量补丁。
pub fn overrideConfig(cfg: &mut config::Config, fset: &flag::FlagSet) {
    let mut actualFlags = HashMap::new();
    fset.Visit(|f| {
        actualFlags.insert(f.Name.clone(), true);
    });
    let fv = flags();
    if actualFlags.contains_key(nmStarterParams) && cfg.DeployMode == deploymode::Starter {
        if let Err(error) = applyStarterAdditionalParams(cfg, &getStarterAdditionalParams()) {
            stubs::must_nil(Some(error));
        }
    }

    // 网络地址优先由 CLI 显式覆盖；advertise-address 为空时需要推导一个可对外通告的值。
    if actualFlags.contains_key(nmHost) {
        cfg.Host = fv.host.clone();
    }
    if actualFlags.contains_key(nmAdvertiseAddress) {
        if fv.advertiseAddress.split(' ').count() > 1 {
            stubs::must_nil(Some(Error::new("Only support one advertise-address")));
        }
        cfg.AdvertiseAddress = fv.advertiseAddress.clone();
    }
    if cfg.AdvertiseAddress.is_empty() && cfg.Host == "0.0.0.0" {
        cfg.AdvertiseAddress = util::GetLocalIP();
    }
    if cfg.AdvertiseAddress.is_empty() {
        cfg.AdvertiseAddress = cfg.Host.clone();
    }
    // The entry adapter owns a separate Config; project only the new optional
    // listener setting through the canonical loader without changing MySQL fields.
    if !fv.configPath.is_empty() {
        cfg.PostgresPort = astersql_config::config::load_postgres_port(&fv.configPath)
            .unwrap_or_else(|error| stubs::fatal(error.to_string()));
    }
    if actualFlags.contains_key(nmPostgresPort) {
        cfg.PostgresPort = Some(
            fv.postgresPort
                .parse::<u16>()
                .unwrap_or_else(|error| stubs::fatal(format!("invalid PostgreSQL port: {error}"))),
        );
    }
    if actualFlags.contains_key(nmPort) {
        cfg.Port = fv
            .port
            .parse::<u32>()
            .unwrap_or_else(|e| stubs::fatal(e.to_string()));
    }
    if actualFlags.contains_key(nmCors) {
        cfg.Cors = fv.cors.clone();
    }
    if actualFlags.contains_key(nmStore) {
        cfg.Store = fv.store.clone();
    }
    if actualFlags.contains_key(nmStorePath) {
        cfg.Path = fv.storePath.clone();
    }
    if actualFlags.contains_key(nmSocket) {
        cfg.Socket = fv.socket.clone();
    }
    if actualFlags.contains_key(nmRunDDL) {
        cfg.Instance.TiDBEnableDDL.Store(fv.runDDL);
    }
    if actualFlags.contains_key(nmDdlLease) {
        cfg.Lease = fv.ddlLease.clone();
    }
    if actualFlags.contains_key(nmTokenLimit) {
        cfg.TokenLimit = fv.tokenLimit as u32;
    }
    if actualFlags.contains_key(nmPluginLoad) {
        cfg.Instance.PluginLoad = fv.pluginLoad.clone();
    }
    if actualFlags.contains_key(nmPluginDir) {
        cfg.Instance.PluginDir = fv.pluginDir.clone();
    }
    // repair-list 只有在 repair-mode 开启时才有意义，避免误把普通字符串写入修复目标。
    if actualFlags.contains_key(nmRepairMode) {
        cfg.RepairMode = fv.repairMode;
    }
    if actualFlags.contains_key(nmRepairList) && cfg.RepairMode {
        cfg.RepairTableList = stringToList(&fv.repairList);
    }
    if actualFlags.contains_key(nmTempDir) {
        cfg.TempDir = fv.tempDir.clone();
    }

    // Starter 模式下 cluster/sql 两套 TLS 分离，分别服务于内部控制面和 SQL 服务。
    // 这里只在 deploy mode 为 starter 时做额外配对校验，保持与 Go 版一致。
    if cfg.DeployMode == deploymode::Starter {
        let cluster_tls_overridden = actualFlags.contains_key(nmClusterCa)
            || actualFlags.contains_key(nmClusterCert)
            || actualFlags.contains_key(nmClusterKey);
        if actualFlags.contains_key(nmClusterCa) {
            cfg.Security.ClusterSSLCA = fv.clusterCA.clone();
        }
        if actualFlags.contains_key(nmClusterCert) {
            cfg.Security.ClusterSSLCert = fv.clusterCert.clone();
        }
        if actualFlags.contains_key(nmClusterKey) {
            cfg.Security.ClusterSSLKey = fv.clusterKey.clone();
        }
        if cluster_tls_overridden {
            // 证书和私钥必须成对出现；只给一个意味着配置无法组成有效 TLS 身份。
            if actualFlags.contains_key(nmClusterCert) != actualFlags.contains_key(nmClusterKey) {
                stubs::must_nil(Some(Error::new(
                    "cluster-cert and cluster-key must be set together",
                )));
            }
            if !cfg.Security.ClusterSSLCA.is_empty()
                && (cfg.Security.ClusterSSLCert.is_empty() || cfg.Security.ClusterSSLKey.is_empty())
            {
                // 指定 CA 却没有客户端证书/私钥，会让双向认证语义不完整。
                stubs::must_nil(Some(Error::new(
                    "cluster-ca requires both cluster-cert and cluster-key",
                )));
            }
        }
        let sql_tls_overridden = actualFlags.contains_key(nmSQLCA)
            || actualFlags.contains_key(nmSQLCert)
            || actualFlags.contains_key(nmSQLKey);
        if actualFlags.contains_key(nmSQLCA) {
            cfg.Security.SSLCA = fv.sqlCA.clone();
        }
        if actualFlags.contains_key(nmSQLCert) {
            cfg.Security.SSLCert = fv.sqlCert.clone();
        }
        if actualFlags.contains_key(nmSQLKey) {
            cfg.Security.SSLKey = fv.sqlKey.clone();
        }
        if sql_tls_overridden {
            // SQL TLS 的配对要求与 cluster TLS 相同，只是作用对象变成 SQL listener。
            if actualFlags.contains_key(nmSQLCert) != actualFlags.contains_key(nmSQLKey) {
                stubs::must_nil(Some(Error::new(
                    "sql-cert and sql-key must be set together",
                )));
            }
            if !cfg.Security.SSLCA.is_empty()
                && (cfg.Security.SSLCert.is_empty() || cfg.Security.SSLKey.is_empty())
            {
                stubs::must_nil(Some(Error::new(
                    "sql-ca requires both sql-cert and sql-key",
                )));
            }
        }
    }

    // 日志、状态页和 metrics 配置都属于“显式传入才覆盖”，避免命令行默认值污染配置文件。
    if actualFlags.contains_key(nmLogLevel) {
        cfg.Log.Level = fv.logLevel.clone();
    }
    if actualFlags.contains_key(nmLogFile) {
        cfg.Log.File.Filename = fv.logFile.clone();
    }
    if actualFlags.contains_key(nmLogSlowQuery) {
        cfg.Log.SlowQueryFile = fv.logSlowQuery.clone();
    }
    if actualFlags.contains_key(nmLogGeneral) {
        cfg.Log.GeneralLogFile = fv.logGeneral.clone();
    }
    if actualFlags.contains_key(nmReportStatus) {
        cfg.Status.ReportStatus = fv.reportStatus;
    }
    if actualFlags.contains_key(nmStatusHost) {
        cfg.Status.StatusHost = fv.statusHost.clone();
    }
    if actualFlags.contains_key(nmStatusPort) {
        cfg.Status.StatusPort = fv
            .statusPort
            .parse::<u32>()
            .unwrap_or_else(|e| stubs::fatal(e.to_string()));
    }
    if actualFlags.contains_key(nmMetricsAddr) {
        cfg.Status.MetricsAddr = fv.metricsAddr.clone();
    }
    if actualFlags.contains_key(nmMetricsInterval) {
        cfg.Status.MetricsInterval = fv.metricsInterval;
    }
    if actualFlags.contains_key(nmProxyProtocolNetworks) {
        cfg.ProxyProtocol.Networks = fv.proxyProtocolNetworks.clone();
    }
    if actualFlags.contains_key(nmProxyProtocolHeaderTimeout) {
        cfg.ProxyProtocol.HeaderTimeout = fv.proxyProtocolHeaderTimeout;
    }
    if actualFlags.contains_key(nmProxyProtocolFallbackable) {
        cfg.ProxyProtocol.Fallbackable = fv.proxyProtocolFallbackable;
    }

    // secure/insecure 是互斥开关，二者都出现时直接报错，避免启动模式含糊。
    if actualFlags.contains_key(nmInitializeSecure)
        && actualFlags.contains_key(nmInitializeInsecure)
    {
        stubs::must_nil(Some(Error::new(
            "the options -initialize-insecure and -initialize-secure are mutually exclusive",
        )));
    }
    if actualFlags.contains_key(nmInitializeSecure) {
        cfg.Security.SecureBootstrap = fv.initializeSecure;
    }
    if actualFlags.contains_key(nmInitializeInsecure) {
        cfg.Security.SecureBootstrap = !fv.initializeInsecure;
    }
    if actualFlags.contains_key(nmDisconnectOnExpiredPassword) {
        cfg.Security.DisconnectOnExpiredPassword = fv.disconnectOnExpiredPassword;
    }
    // Go 版在 Windows 下不支持 secure bootstrap；Rust 侧保持同一限制，避免跨平台语义分叉。
    if std::env::consts::OS == "windows" && cfg.Security.SecureBootstrap {
        stubs::must_nil(Some(Error::new(
            "the option -initialize-secure is not supported on Windows",
        )));
    }
    if actualFlags.contains_key(nmInitializeSQLFile) {
        let sql_file = fv.initializeSQLFile.clone();
        // 允许 `mem://` 是为了兼容测试或内存文件系统注入，不要求真实磁盘路径存在。
        if !std::path::Path::new(&sql_file).exists() && !sql_file.starts_with("mem://") {
            stubs::must_nil(Some(Error::new(format!(
                "can not access -initialize-sql-file {sql_file}"
            ))));
        }
        cfg.InitializeSQLFile = sql_file;
    }
    if actualFlags.contains_key(nmKeyspaceName) {
        cfg.KeyspaceName = fv.keyspaceName.clone();
    }
    if actualFlags.contains_key(nmTiDBServiceScope) {
        // service scope 会参与实例发现与隔离，因此进入配置前先走命名校验。
        stubs::must_nil_result(naming::Check(&fv.serviceScope));
        cfg.Instance.TiDBServiceScope = fv.serviceScope.clone();
    }
    if actualFlags.contains_key(nmStandby) {
        cfg.Standby.StandByMode = fv.standbyMode;
    }
    if actualFlags.contains_key(nmActivationTimeout) {
        cfg.Standby.ActivationTimeout = fv.activationTimeout;
    }
    if actualFlags.contains_key(nmMaxIdleSeconds) {
        cfg.Standby.MaxIdleSeconds = fv.maxIdleSeconds;
    }
    if actualFlags.contains_key(nmKeyspaceActivate) {
        cfg.KeyspaceActivateMode = fv.keyspaceActivateMode;
    }
}

/// 校验 nextgen 下版本字符串的配置策略。
/// nextgen 版本号由构建信息推导，不允许再通过配置手工覆盖。
pub fn validateVersionConfigPolicy(cfg: &config::Config) -> Result<()> {
    if kerneltype::IsNextGen()
        && (!cfg.TiDBEdition.is_empty()
            || !cfg.TiDBReleaseVersion.is_empty()
            || !cfg.ServerVersion.is_empty())
    {
        return Err(Error::new(
            "config options tidb-edition, tidb-release-version and server-version are not allowed to set in nextgen kernel",
        ));
    }
    Ok(())
}

/// 根据构建产物派生运行时对外暴露的 release/server version。
/// 这是 nextgen 入口特有的版本拼装规则。
pub fn deriveRuntimeVersionsFromBuildInfo(releaseVersion: &str) -> Result<(String, String)> {
    let normalized = mysql::NormalizeTiDBReleaseVersionForNextGen(releaseVersion);
    let serverVersion = mysql::BuildTiDBXServerVersion(&normalized).map_err(|e| {
        Error::new(format!(
            "invalid tidb release version for nextgen kernel: {e}"
        ))
    })?;
    Ok((normalized, serverVersion))
}

/// 初始化版本相关全局变量。
/// classic 允许从配置覆盖，nextgen 则强制从构建信息派生。
pub fn initVersions(cfg: &config::Config) -> Result<()> {
    validateVersionConfigPolicy(cfg)?;
    if kerneltype::IsNextGen() {
        let (normalized, serverVersion) =
            deriveRuntimeVersionsFromBuildInfo(&mysql::TiDBReleaseVersion())?;
        mysql::set_TiDBReleaseVersion(normalized);
        mysql::set_ServerVersion(serverVersion);
        return Ok(());
    }
    if !cfg.TiDBEdition.is_empty() {
        versioninfo::set_TiDBEdition(cfg.TiDBEdition.clone());
    }
    if !cfg.TiDBReleaseVersion.is_empty() {
        mysql::set_TiDBReleaseVersion(cfg.TiDBReleaseVersion.clone());
    }
    if !cfg.ServerVersion.is_empty() {
        mysql::set_ServerVersion(cfg.ServerVersion.clone());
    }
    Ok(())
}

/// 包装版版本初始化，失败时直接走统一 fatal 逻辑。
pub fn mustInitVersions() {
    let cfg = config::GetGlobalConfig();
    stubs::must_nil_result(initVersions(&cfg));
}

/// 把 `config::Config` 映射到运行时全局变量、原子值和 sysvar。
///
/// 这一步是入口最“分散”的部分，但语义非常关键：
/// - 配置文件里的值先经过去兼容、修正和默认化。
/// - 然后落到各个子系统真正读取的全局位置。
/// - 最后把少量被入口重写过的字段再持久回全局配置。
///
/// 也就是说，后面运行中的很多模块并不再直接读原始 config，而是读这里设置的
/// vardef / variable / 原子变量，因此初始化顺序不能随意调整。
pub fn setGlobalVars() {
    let mut cfg = config::GetGlobalConfig();

    // 先把历史废弃配置名映射回新的 instance 字段，兼容老版本配置文件。
    for deprecatedOption in config::DeprecatedOptions() {
        for oldName in &deprecatedOption.NameMappings {
            match deprecatedOption.SectionName {
                "" => match *oldName {
                    // 根级旧配置项直接回填到 instance 或兼容原子值。
                    "check-mb4-value-in-utf8" => {
                        cfg.Instance
                            .CheckMb4ValueInUTF8
                            .Store(cfg.CheckMb4ValueInUTF8.Load());
                    }
                    "enable-collect-execution-info" => {
                        cfg.Instance
                            .EnableCollectExecutionInfo
                            .Store(cfg.EnableCollectExecutionInfo);
                    }
                    "max-server-connections" => {
                        cfg.Instance.MaxConnections = cfg.MaxServerConnections;
                    }
                    "run-ddl" => {
                        cfg.Instance.TiDBEnableDDL.Store(cfg.RunDDL);
                    }
                    _ => {}
                },
                "log" => match *oldName {
                    // 历史 `log.*` 选项迁移到 instance 后仍要保留老配置兼容性。
                    "enable-slow-log" => {
                        cfg.Instance
                            .EnableSlowLog
                            .Store(cfg.Log.EnableSlowLog.Load());
                    }
                    "slow-threshold" => {
                        cfg.Instance.SlowThreshold = cfg.Log.SlowThreshold;
                    }
                    "record-plan-in-slow-log" => {
                        cfg.Instance.RecordPlanInSlowLog = cfg.Log.RecordPlanInSlowLog;
                    }
                    _ => {}
                },
                "performance" => {
                    // `force-priority` 仍会影响 SQL 层 sysvar 和调度优先级。
                    if *oldName == "force-priority" {
                        cfg.Instance.ForcePriority = cfg.Performance.ForcePriority.clone();
                    }
                }
                "plugin" => match *oldName {
                    // 插件目录和待加载清单在新旧配置结构之间做一次镜像。
                    "load" => cfg.Instance.PluginLoad = cfg.Plugin.Load.clone(),
                    "dir" => cfg.Instance.PluginDir = cfg.Plugin.Dir.clone(),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    // Go 版用 automaxprocs；Rust stub 只保留边界和错误处理时机。
    let nop_log = |_s: &str| {};
    stubs::must_nil_result(maxprocs::Set(nop_log));
    let _ = cfg.Performance.MaxProcs; // GOMAXPROCS noop when 0

    // GOGC、schema lease、stats lease 等是很多后台任务的核心时序参数。
    util::SetGOGC(cfg.Performance.GOGC);

    // lease 家族配置会影响 infoschema 刷新、统计信息和 bind plan 的后台节奏。
    let mut schemaLeaseDuration = parseDuration(&cfg.Lease);
    if schemaLeaseDuration.is_zero() {
        // schema lease 不能为 0；维持 Go 版做法，回退到默认值继续启动。
        log::Warn("schema lease is invalid, use default value");
        schemaLeaseDuration = config::DefSchemaLease;
    }
    vardef::SetSchemaLease(schemaLeaseDuration);
    vardef::SetStatsLease(parseDuration(&cfg.Performance.StatsLease));
    vardef::SetPlanReplayerGCLease(parseDuration(&cfg.Performance.PlanReplayerGCLease));
    // bindinfo lease 单独走自己的全局入口，但仍遵守相同的 duration 解析规则。
    bindinfo::set_Lease(parseDuration(&cfg.Performance.BindInfoLease));
    statistics::Ratio_Store(cfg.Performance.PseudoEstimateRatio);
    if cfg.SplitTable {
        // split table region 是一次性启动期开关，后续主要由 DDL 路径读取。
        ddl::EnableSplitTableRegion.store(1, std::sync::atomic::Ordering::SeqCst);
    }
    // 这些原子变量会被执行器、权限系统和事务层在热路径直接读取。
    plannercore::AllowCartesianProduct.store(
        cfg.Performance.CrossJoin,
        std::sync::atomic::Ordering::SeqCst,
    );
    privileges::SkipWithGrant.store(
        cfg.Security.SkipGrantTable,
        std::sync::atomic::Ordering::SeqCst,
    );
    // 事务大小限制直接作用于 KV 提交前检查，因此必须在建连前写好。
    if cfg.Performance.TxnTotalSizeLimit == config::DefTxnTotalSizeLimit {
        // 默认值按 Go 逻辑提升到 `SuperLargeTxnSize`，保留历史兼容语义。
        kv::TxnTotalSizeLimit.store(
            config::SuperLargeTxnSize,
            std::sync::atomic::Ordering::SeqCst,
        );
    } else {
        kv::TxnTotalSizeLimit.store(
            cfg.Performance.TxnTotalSizeLimit,
            std::sync::atomic::Ordering::SeqCst,
        );
    }
    if cfg.Performance.TxnEntrySizeLimit > config::MaxTxnEntrySizeLimit {
        log::Fatal("cannot set txn entry size limit larger than 120M");
    }
    kv::TxnEntrySizeLimit.store(
        cfg.Performance.TxnEntrySizeLimit,
        std::sync::atomic::Ordering::SeqCst,
    );

    let priority = mysql::Str2Priority(&cfg.Instance.ForcePriority);
    // SQL 优先级在 MySQL 字符串和内部整数常量之间做一次规范化。
    vardef::ForcePriority.store(priority as i32, std::sync::atomic::Ordering::SeqCst);

    // 这组原子值主要服务于 SQL 执行期行为开关和慢操作阈值。
    vardef::ProcessGeneralLog.store(
        cfg.Instance.TiDBGeneralLog,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::EnablePProfSQLCPU.store(
        cfg.Instance.EnablePProfSQLCPU,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::EnableRCReadCheckTS.store(
        cfg.Instance.TiDBRCReadCheckTS,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::IsSandBoxModeEnabled.store(
        !cfg.Security.DisconnectOnExpiredPassword,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::DDLSlowOprThreshold.store(
        cfg.Instance.DDLSlowOprThreshold,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::ExpensiveQueryTimeThreshold.store(
        cfg.Instance.ExpensiveQueryTimeThreshold,
        std::sync::atomic::Ordering::SeqCst,
    );
    vardef::ExpensiveTxnTimeThreshold.store(
        cfg.Instance.ExpensiveTxnTimeThreshold,
        std::sync::atomic::Ordering::SeqCst,
    );

    // 版本相关 sysvar 要在 release/server version 初始化之后再写入。
    stubs::must_nil_result(initVersions(&cfg));
    variable::SetSysVar(vardef::Version, &mysql::ServerVersion());

    // 版本注释优先级为：Edition 推导默认值 < 显式 VersionComment 配置。
    if !cfg.TiDBEdition.is_empty() {
        variable::SetSysVar(
            vardef::VersionComment,
            &format!(
                "TiDB Server (Apache License 2.0) {} Edition, MySQL 8.0 compatible",
                versioninfo::TiDBEdition()
            ),
        );
    }
    if !cfg.VersionComment.is_empty() {
        variable::SetSysVar(vardef::VersionComment, &cfg.VersionComment);
    }

    // 某些 instance 变量只有配置值非空/非 0 时才写入，避免把“未配置”伪装成显式覆盖。
    let setInstanceVar = |name: &str, value: &str| {
        if value.is_empty() || value == "0" {
            return;
        }
        // 这里保留原 sysvar 的其它元信息，只替换值和“来自配置”的标记。
        let old = variable::GetSysVar(name).unwrap_or(variable::SysVar {
            Name: name.into(),
            Value: String::new(),
            IsInitedFromConfig: false,
            instance_scope: false,
        });
        let mut tmp = old;
        tmp.Value = value.into();
        tmp.IsInitedFromConfig = true;
        variable::RegisterSysVar(&tmp);
    };
    // 先注册语句摘要、内存治理和 schema cache 这类实例级容量参数。
    setInstanceVar(
        vardef::TiDBStmtSummaryMaxStmtCount,
        &cfg.Instance.StmtSummaryMaxStmtCount.to_string(),
    );
    setInstanceVar(
        vardef::TiDBServerMemoryLimit,
        &cfg.Instance.ServerMemoryLimit,
    );
    setInstanceVar(
        vardef::TiDBMemArbitratorMode,
        &cfg.Instance.MemArbitratorMode,
    );
    setInstanceVar(
        vardef::TiDBMemArbitratorSoftLimit,
        &cfg.Instance.MemArbitratorSoftLimit,
    );
    setInstanceVar(
        vardef::TiDBServerMemoryLimitGCTrigger,
        &cfg.Instance.ServerMemoryLimitGCTrigger,
    );
    setInstanceVar(
        vardef::TiDBInstancePlanCacheMaxMemSize,
        &cfg.Instance.InstancePlanCacheMaxMemSize,
    );
    setInstanceVar(
        vardef::TiDBStatsCacheMemQuota,
        &cfg.Instance.StatsCacheMemQuota.to_string(),
    );
    setInstanceVar(
        vardef::TiDBMemQuotaBindingCache,
        &cfg.Instance.MemQuotaBindingCache.to_string(),
    );
    setInstanceVar(vardef::TiDBSchemaCacheSize, &cfg.Instance.SchemaCacheSize);

    // 把常见配置镜像进 SQL 层 sysvar，方便会话内 SQL 查看实例状态。
    // 这些值会出现在 `show variables` 或诊断 SQL 中，因此需要保持最终值一致。
    variable::SetSysVar(vardef::TiDBForcePriority, mysql::Priority2Str(priority));
    variable::SetSysVar(
        vardef::TiDBOptDistinctAggPushDown,
        variable::BoolToOnOff(cfg.Performance.DistinctAggPushDown),
    );
    variable::SetSysVar(
        vardef::TiDBOptProjectionPushDown,
        variable::BoolToOnOff(cfg.Performance.ProjectionPushDown),
    );
    variable::SetSysVar(vardef::Port, &format!("{}", cfg.Port));
    // Socket 路径支持 `{Port}` 模板，占位符必须在这里展开成最终值。
    cfg.Socket = cfg.Socket.replacen("{Port}", &format!("{}", cfg.Port), 1);
    variable::SetSysVar(vardef::Socket, &cfg.Socket);
    variable::SetSysVar(vardef::DataDir, &cfg.Path);
    variable::SetSysVar(vardef::TiDBSlowQueryFile, &cfg.Log.SlowQueryFile);
    variable::SetSysVar(
        vardef::TiDBIsolationReadEngines,
        &cfg.IsolationRead.Engines.join(","),
    );
    variable::SetSysVar(
        vardef::TiDBEnforceMPPExecution,
        variable::BoolToOnOff(config::GetGlobalConfig().Performance.EnforceMPP),
    );
    vardef::MemoryUsageAlarmRatio_Store(cfg.Instance.MemoryUsageAlarmRatio);
    // 约束检查策略会影响悲观事务路径，因此也需要映射到 sysvar 层。
    variable::SetSysVar(
        vardef::TiDBConstraintCheckInPlacePessimistic,
        variable::BoolToOnOff(cfg.PessimisticTxn.ConstraintCheckInPlacePessimistic),
    );
    if let Ok(h) = hostname() {
        // 主机名不是强制能力，取到时才写回 sysvar。
        variable::SetSysVar(vardef::Hostname, &h);
    }
    vardef::GlobalLogMaxDays.store(
        config::GetGlobalConfig().Log.File.MaxDays,
        std::sync::atomic::Ordering::SeqCst,
    );

    // 部分回归开关会联动开启 prepare plan cache，保持与 Go 版兼容分支。
    if config::CheckTableBeforeDrop.load(std::sync::atomic::Ordering::SeqCst) {
        variable::SetSysVar(vardef::TiDBEnablePrepPlanCache, variable::BoolToOnOff(true));
    }
    plannercore::PreparedPlanCacheMaxMemory.store(
        cfg.Performance.ServerMemoryQuota,
        std::sync::atomic::Ordering::SeqCst,
    );
    // 计划缓存默认沿用 server memory quota，再按物理内存做最终兜底裁剪。
    let total = stubs::must_nil_result(memory::MemTotal());
    let cur = plannercore::PreparedPlanCacheMaxMemory.load(std::sync::atomic::Ordering::SeqCst);
    if cur > total || cur == 0 {
        // 计划缓存内存上限不能超过物理内存，也不能维持为 0。
        plannercore::PreparedPlanCacheMaxMemory.store(total, std::sync::atomic::Ordering::SeqCst);
    }

    // 事务提交超时和 store liveness timeout 最终都要转换成下游库读得懂的形式。
    let commit_ms = (parseDuration(&cfg.TiKVClient.CommitTimeout).as_secs_f64() * 1000.0) as u64;
    transaction::CommitMaxBackoff.store(commit_ms, std::sync::atomic::Ordering::SeqCst);
    // region cache TTL、repair info 和 spill quota 都属于执行期直接读取的数据结构。
    tikv::SetRegionCacheTTLSec(cfg.TiKVClient.RegionCacheTTL);
    domainutil::repair_info().SetRepairMode(cfg.RepairMode);
    domainutil::repair_info().SetRepairTableList(cfg.RepairTableList.clone());
    executor::disk_tracker().SetBytesLimit(cfg.TempStorageQuota);
    if cfg.Performance.ServerMemoryQuota < 1 {
        // `<1` 表示不设硬限制，沿用 Go 版以 -1 表达 unlimited。
        executor::memory_tracker().SetBytesLimit(-1);
    } else {
        executor::memory_tracker().SetBytesLimit(cfg.Performance.ServerMemoryQuota as i64);
    }
    kvcache::GlobalLRUMemUsageTracker.AttachToGlobalTracker(executor::memory_tracker());

    // store liveness timeout 必须在 TiKV client 初始化前规范化。
    let t = stubs::parse_go_duration(&cfg.TiKVClient.StoreLivenessTimeout);
    match t {
        Ok(d) => tikv::SetStoreLivenessTimeout(d),
        Err(_) => {
            // 该值进入 TiKV client 前必须是合法 duration，否则宁可直接失败。
            logutil::BgLogger::Fatal("invalid duration value for store-liveness-timeout");
        }
    }
    parsertypes::TiDBStrictIntegerDisplayWidth.store(
        cfg.DeprecateIntegerDisplayWidth,
        std::sync::atomic::Ordering::SeqCst,
    );
    // 剩余这些结构更偏“后台治理参数”，在启动阶段统一灌入全局单例即可。
    deadlockhistory::GlobalDeadlockHistory.Resize(cfg.PessimisticTxn.DeadlockHistoryCapacity);
    txninfo::Recorder.ResizeSummaries(cfg.TrxSummary.TransactionSummaryCapacity);
    txninfo::Recorder.SetMinDuration(Duration::from_millis(
        cfg.TrxSummary.TransactionIDDigestMinDuration as u64,
    ));
    chunk::InitChunkAllocSize(cfg.TiDBMaxReuseChunk, cfg.TiDBMaxReuseColumn);

    if !cfg.Instance.TiDBServiceScope.is_empty() {
        // service scope 对外统一转小写，保证拓扑匹配不受大小写差异影响。
        vardef::ServiceScope_Store(cfg.Instance.TiDBServiceScope.to_lowercase());
    }

    astersql_config::update_global(|canonical| {
        canonical.starter_params.enable_rg_fallback = cfg.StarterParams.EnableRGFallback;
    });
    // Persist mutated socket back to global config.
    // `cfg.Socket` 经过 `{Port}` 展开后已经不再是原始配置，需要回写供后续读取。
    config::UpdateGlobal(|g| {
        g.Socket = cfg.Socket.clone();
        g.Instance = cfg.Instance.clone();
    });
}

/// 初始化日志系统。
/// 这里同时准备内部 HTTP client，因为它依赖日志与 keyspace 包装器就绪。
pub fn setupLog() -> Result<()> {
    let cfg = config::GetGlobalConfig();
    // keyspace wrapper 会把 keyspace 信息编进日志字段，便于 starter 多 keyspace 排障。
    logutil::InitLogger(cfg.Log.ToLogConfig(), keyspace::WrapZapcoreWithKeyspace())?;
    // 初始化内部 HTTP client，确保后面各组件拿到的是带统一配置的单例。
    util::InternalHTTPClient();
    Ok(())
}

/// 初始化扩展系统并返回当前加载结果。
/// 入口只关心 setup 是否成功，不在这里消费具体扩展列表。
pub fn setupExtensions() -> Result<extension::Extensions> {
    // setup 负责执行扩展注册副作用，GetExtensions 只是返回最终快照。
    extension::Setup()?;
    extension::GetExtensions()
}

/// 打印 TiDB 启动信息。
/// 为了避免被高日志级别抑制，临时把 level 调到 `info` 再恢复。
pub fn printInfo() {
    let level = log::GetLevel();
    // 输出启动信息时总是提升到 info，避免在 warn/error 级别下看不到版本横幅。
    log::SetLevel("info");
    printer::PrintTiDBInfo();
    log::SetLevel(&level);
}

/// 创建 SQL server 并把 Domain 的多个后台 handle 绑定到它。
///
/// 一旦 `NewServer` 失败，需要主动回收已经建立的 storage/domain，
/// 避免启动半途泄漏 DDL owner 或底层 store 连接。
pub fn createServer(storage: &kv::Storage, dom: &domain::Domain) -> server::Server {
    let cfg = config::GetGlobalConfig();
    let canonical_config = match canonicalServerConfig(&cfg) {
        Ok(config) => config,
        Err(err) => {
            closeDDLOwnerMgrDomainAndStorage(storage, dom);
            log::Fatal(&format!("failed to create the server: {err}"));
        }
    };
    stubs::record_event(format!(
        "canonical.session.storage storage-id={:?}",
        storage.registered_identity()
    ));
    let auth_mode = if cfg.Security.SecureBootstrap {
        BootstrapAuthMode::SecureUnsupported
    } else {
        BootstrapAuthMode::InsecureRootOnly
    };
    let (canonical_domain, session_driver) = match storage.CanonicalTiKVStore() {
        Ok(tikv_store) => {
            let factory = match CanonicalSessionFactory::from_tikv_store(tikv_store.clone()) {
                Ok(factory) => Arc::new(factory),
                Err(err) => {
                    closeDDLOwnerMgrDomainAndStorage(storage, dom);
                    log::Fatal(&format!("failed to initialize canonical Domain: {err}"));
                }
            };
            if let Err(error) =
                startStarterResourceGroupController(&cfg, &tikv_store, factory.domain())
            {
                closeDDLOwnerMgrDomainAndStorage(storage, dom);
                log::Fatal(&format!(
                    "failed to initialize Starter resource controller: {error}"
                ));
            }
            (
                Arc::clone(factory.domain()),
                Arc::new(ConcreteSessionDriver::new(factory, auth_mode)),
            )
        }
        Err(err) if cfg.Store == config::StoreTypeTiKV => {
            closeDDLOwnerMgrDomainAndStorage(storage, dom);
            log::Fatal(&format!("failed to get canonical TiKV storage: {err}"));
        }
        Err(_) => {
            // Local UniStore/MockTiKV registry handles do not expose the
            // client-rust TiKV type. The canonical SQL runtime therefore owns
            // an in-process transactional mock store, matching the local
            // development mode without opening a second external client.
            let (domain, _) = match CreateAnalyzeSession() {
                Ok(session) => session,
                Err(err) => {
                    closeDDLOwnerMgrDomainAndStorage(storage, dom);
                    log::Fatal(&format!(
                        "failed to initialize local canonical Domain: {err}"
                    ));
                }
            };
            let driver = Arc::new(ConcreteSessionDriver::from_initialized_domain(
                Arc::clone(&domain),
                auth_mode,
            ));
            (domain, driver)
        }
    };
    if let Err(error) = start_domain_ttl_job_manager(&canonical_domain) {
        closeDDLOwnerMgrDomainAndStorage(storage, dom);
        log::Fatal(&format!(
            "failed to start canonical TTL job manager: {error}"
        ));
    }
    let svr = match assembleCanonicalServer(canonical_config, canonical_domain, session_driver) {
        Ok(server) => server,
        Err(err) => {
            closeDDLOwnerMgrDomainAndStorage(storage, dom);
            log::Fatal(&format!("failed to assemble the canonical server: {err}"));
        }
    };
    svr.SetDomain(dom);
    // 这些 handle 依赖 session manager 做会话观察和内存/慢查询治理。
    let _ = dom.ExpensiveQueryHandle().SetSessionManager(&svr);
    dom.ExpensiveQueryHandle().SetSessionManager(&svr).Run();
    // 这些 Domain handle 在 server 创建后立即启动，确保实例一上线就有治理能力。
    dom.MemoryUsageAlarmHandle().SetSessionManager(&svr).Run();
    dom.ServerMemoryLimitHandle().SetSessionManager(&svr).Run();
    dom.InfoSyncer().SetSessionManager(&svr);
    svr
}

pub(crate) fn assembleCanonicalServer(
    config: CanonicalServerConfig,
    domain: Arc<astersql_domain::Domain>,
    session_driver: Arc<ConcreteSessionDriver>,
) -> Result<server::Server> {
    let connection_domain = Arc::new(CanonicalConnectionDomain::new(Arc::clone(&domain)));
    let canonical_server = CanonicalServer::new(config, Arc::new(CanonicalServerDriver))
        .map_err(|error| Error::new(format!("create canonical server: {error}")))?;
    canonical_server
        .set_connection_runtime(session_driver, connection_domain)
        .map_err(|error| Error::new(format!("configure canonical server: {error}")))?;
    Ok(server::Server::from_canonical(
        canonical_server,
        Arc::new(CanonicalServerDomain::new(domain)),
    ))
}

/// 将入口配置投影到 canonical listener。
pub fn canonicalServerConfig(cfg: &config::Config) -> Result<CanonicalServerConfig> {
    let port = u16::try_from(cfg.Port)
        .map_err(|_| Error::new(format!("SQL port {} is outside 0..=65535", cfg.Port)))?;
    let status_port = u16::try_from(cfg.Status.StatusPort).map_err(|_| {
        Error::new(format!(
            "status port {} is outside 0..=65535",
            cfg.Status.StatusPort
        ))
    })?;
    if cfg.Security.SSLCert.is_empty() != cfg.Security.SSLKey.is_empty() {
        return Err(Error::new("SQL TLS requires both ssl-cert and ssl-key"));
    }
    if cfg.Security.ClusterSSLCert.is_empty() != cfg.Security.ClusterSSLKey.is_empty() {
        return Err(Error::new(
            "status TLS requires both cluster-ssl-cert and cluster-ssl-key",
        ));
    }
    Ok(CanonicalServerConfig {
        host: cfg.Host.clone(),
        advertise_address: cfg.AdvertiseAddress.clone(),
        port,
        postgres_port: cfg.PostgresPort,
        socket: (!cfg.Socket.is_empty()).then(|| cfg.Socket.clone()),
        max_connections: cfg.MaxServerConnections as usize,
        proxy_protocol_enabled: !cfg.ProxyProtocol.Networks.trim().is_empty(),
        proxy_protocol_networks: cfg.ProxyProtocol.Networks.clone(),
        proxy_protocol_fallbackable: cfg.ProxyProtocol.Fallbackable,
        proxy_protocol_header_timeout: Duration::from_secs(cfg.ProxyProtocol.HeaderTimeout as u64),
        sql_tls_ca: (!cfg.Security.SSLCA.is_empty()).then(|| cfg.Security.SSLCA.clone()),
        sql_tls_certificate: (!cfg.Security.SSLCert.is_empty())
            .then(|| cfg.Security.SSLCert.clone()),
        sql_tls_key: (!cfg.Security.SSLKey.is_empty()).then(|| cfg.Security.SSLKey.clone()),
        sql_auto_tls: cfg.Security.AutoTLS,
        rsa_key_size: cfg.Security.RSAKeySize,
        temp_storage_path: (!cfg.TempStoragePath.is_empty()).then(|| cfg.TempStoragePath.clone()),
        status: CanonicalStatusConfig {
            report_status: cfg.Status.ReportStatus,
            host: cfg.Status.StatusHost.clone(),
            port: status_port,
            tls_ca: (!cfg.Security.ClusterSSLCA.is_empty())
                .then(|| cfg.Security.ClusterSSLCA.clone()),
            tls_certificate: (!cfg.Security.ClusterSSLCert.is_empty())
                .then(|| cfg.Security.ClusterSSLCert.clone()),
            tls_key: (!cfg.Security.ClusterSSLKey.is_empty())
                .then(|| cfg.Security.ClusterSSLKey.clone()),
            tls_verify_common_names: cfg.Security.ClusterVerifyCN.clone(),
            ..CanonicalStatusConfig::default()
        },
        ..CanonicalServerConfig::default()
    })
}

/// 启动与指标相关的后台能力。
/// 包括可选的 pyroscope、系统时钟回拨检测和 Prometheus push。
pub fn setupMetrics() {
    enablePyroscope();
    let cfg = config::GetGlobalConfig();
    // 系统时钟回拨会影响 lease、TTL 和事务超时判断，因此单独记指标。
    let systimeErrHandler = || {
        metrics::TimeJumpBackCounter.Inc();
    };
    // 系统时钟监控在独立线程中运行，不阻塞主启动链路。
    std::thread::spawn(move || {
        systimemon::StartMonitor(std::time::SystemTime::now, systimeErrHandler);
    });
    // Pushgateway 地址为空或间隔为 0 时，`pushMetric` 内部会自动禁用。
    pushMetric(
        &cfg.Status.MetricsAddr,
        Duration::from_secs(cfg.Status.MetricsInterval as u64),
    );
}

/// 初始化全局 tracing provider。
/// Jaeger/Tracing 失败不是静默降级，而是作为启动失败返回上层决定。
pub fn setupTracing() -> Result<()> {
    let cfg = config::GetGlobalConfig();
    let mut tracingCfg = cfg.OpenTracing.ToTracingConfig();
    tracingCfg.ServiceName = "TiDB".into();
    match tracingCfg.NewTracer() {
        Ok((_tracer, _)) => {
            opentracing::SetGlobalTracer(());
            Ok(())
        }
        Err(err) => {
            log::Error(&format!("setup jaeger tracer failed: {err}"));
            Err(err)
        }
    }
}

/// 关闭 DDL owner、Domain 和底层 storage。
/// 该函数只处理“核心资源”清理，外围 server/plugin/topsql 在 `cleanup` 中处理。
pub fn closeDDLOwnerMgrDomainAndStorage(storage: &kv::Storage, dom: &domain::Domain) {
    // 标记 store shutting down 可以让下游尽快停止新请求与重试。
    tikv::StoreShuttingDown(1);
    // 先关 Domain，再关 owner manager 和底层 storage，尽量减少新后台任务产生。
    dom.Close();
    ddl::CloseOwnerManager(storage);
    copr::GlobalMPPFailedStoreProber.Stop();
    mppcoordmanager::InstanceMPPCoordinatorManager.Stop();
    stubs::terror_log(storage.Close().err());
    if kv::IsUserKS(storage) {
        // 用户 keyspace 可能伴随一个额外的 SYSTEM storage，需要一并回收。
        stubs::terror_log(
            store_registry::GetSystemStorage()
                .map(|system_storage| {
                    system_storage
                        .Close()
                        .map_err(|error| Error::new(format!("close system storage: {error}")))
                })
                .transpose()
                .err(),
        );
    }
}

// 优雅关闭默认给 15 秒 drain 连接时间。
// 这是入口级超时，不是 SQL 层单个查询的超时配置。
pub static gracefulCloseConnectionsTimeout: Duration = Duration::from_secs(15);

/// 收敛 server 相关的所有清理动作。
///
/// 顺序上先停自动分析和新连接，再停插件/仓库/TopSQL，
/// 最后关闭 Domain 与底层存储，尽量减少清理阶段的交叉访问。
pub fn cleanup(svr: &server::Server, storage: &kv::Storage, dom: &domain::Domain) {
    // 自动分析会继续调度后台 SQL，清理时要尽早停止。
    dom.StopAutoAnalyze();

    let mut drainClientWait = gracefulCloseConnectionsTimeout;
    if deploymode::IsStarter() && svr.GetForceShutdown() {
        // Starter 强制关闭场景不等待客户端主动断开，直接把 drain 时间降为 0。
        drainClientWait = Duration::from_secs(0);
    }
    let cancelClientWait = Duration::from_secs(1);
    // 先 drain 正常连接，再取消剩余连接与系统会话，尽量贴近 Go 的优雅退出语义。
    svr.DrainClients(drainClientWait, cancelClientWait);
    // 系统会话往往不受普通客户端 drain 控制，需要显式杀掉。
    svr.KillSysProcesses();
    plugin::Shutdown(());
    // workload repository 和 TopSQL 都会持续消费会话/语句事件，需先于 storage 关闭。
    repository::StopRepository();
    topsql::Close();
    closeDDLOwnerMgrDomainAndStorage(storage, dom);
    disk::CleanUp();
    closeStmtSummary();
    cgmon::StopCgroupMonitor();
}

/// 把 repair 列表字符串切成表名列表。
/// 同时兼容 `[a,b]`、`a,b` 和带空格/引号的历史输入格式。
pub fn stringToList(repairString: &str) -> Vec<String> {
    if repairString.is_empty() {
        return Vec::new();
    }
    let mut s = repairString.to_string();
    // 兼容旧配置可能带上的方括号包裹形式。
    if s.starts_with('[') && s.ends_with(']') {
        s = s[1..s.len() - 1].to_string();
    }
    // 空格、逗号和引号都被视为分隔符，最大化兼容人工书写格式。
    s.split(|r| r == ',' || r == ' ' || r == '"')
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// 初始化持久化语句摘要。
/// 失败只记录错误，因为该能力不是 SQL 服务可用性的硬依赖。
pub fn setupStmtSummary() {
    let instanceCfg = config::GetGlobalConfig().Instance;
    if instanceCfg.StmtSummaryEnablePersistent {
        // 配置结构直接取自 instance 字段，保持与 Go v2 summary 初始化路径一致。
        let cfg = stmtsummaryv2::Config {
            Filename: instanceCfg.StmtSummaryFilename,
            FileMaxSize: instanceCfg.StmtSummaryFileMaxSize,
            FileMaxDays: instanceCfg.StmtSummaryFileMaxDays,
            FileMaxBackups: instanceCfg.StmtSummaryFileMaxBackups,
        };
        // 语句摘要持久化失败不应阻塞实例提供 SQL 服务，只保留错误日志。
        if let Err(_) = stmtsummaryv2::Setup(&cfg) {
            logutil::BgLogger::Error("failed to setup statements summary");
        }
    }
}

/// 关闭持久化语句摘要。
/// 只在启用持久化时调用对应 close，避免无意义的全局收尾动作。
pub fn closeStmtSummary() {
    let instanceCfg = config::GetGlobalConfig().Instance;
    if instanceCfg.StmtSummaryEnablePersistent {
        // 关闭动作对未启用持久化的实例没有意义，因此保持条件对称。
        stmtsummaryv2::Close();
    }
}

// starter 为 keyspace 注入默认 metrics 标签时使用的字段名。
// 之所以单独抽常量，是为了和可配置 observability 标签 merge 时复用。
pub const keyspaceNameMetricLabel: &str = "keyspace_name";

/// 在 starter 模式下解析并补全 keyspace observability 配置。
///
/// 默认先写入 `keyspace_name` 标签，再把控制面传下来的 metadata 与
/// 配置文件中的 observability 规则解析合并，最终回写全局配置。
pub fn prepareKeyspaceObservabilityForStarter(metadata: HashMap<String, String>) -> Result<()> {
    let cfg = config::GetGlobalConfig();
    if cfg.Store != config::StoreTypeTiKV {
        // 只有 TiKV 路径真正消费 keyspace observability；其它 store 直接跳过。
        return Ok(());
    }
    let mut resolvedValues = config::KeyspaceObservabilityValues {
        MetricLabels: HashMap::from([(keyspaceNameMetricLabel.into(), cfg.KeyspaceName.clone())]),
        SlowLogFields: Vec::new(),
        StmtLogFields: HashMap::new(),
    };
    let mut copiedConfig = config::GetGlobalConfig();
    // 解析过程可能读取 metadata 占位符，因此基于复制配置执行，避免污染原始全局值。
    copiedConfig.ResolveKeyspaceObservability(&metadata)?;
    let configuredValues = copiedConfig.KeyspaceObservabilityValues.Clone();
    // 显式配置优先级高于默认 keyspace_name，但不丢掉默认集合中的其它字段。
    for (k, v) in configuredValues.MetricLabels {
        resolvedValues.MetricLabels.insert(k, v);
    }
    resolvedValues.SlowLogFields = configuredValues.SlowLogFields;
    resolvedValues.StmtLogFields = configuredValues.StmtLogFields;
    config::UpdateGlobal(|conf| {
        conf.KeyspaceObservabilityValues = resolvedValues.clone();
    });
    Ok(())
}

/// starter 额外参数的解析结果。
/// 这些字段主要用于和外部 manager 建立回调连接，不直接暴露给 SQL 层。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct starterParams {
    // manager 所在 namespace 可用于拼默认服务地址。
    pub managerNamespace: String,
    // pod 身份信息会回传给 manager 作为注册元数据。
    pub podName: String,
    pub podIP: String,
    pub podNamespace: String,
    pub enableRGFallback: bool,
}

/// 解析 `k=v,k=v` 形式的 starter 额外参数。
///
/// 这里刻意做严格校验：
/// - 不允许空项、空 key、空 value；
/// - 不允许重复 key；
/// - 不接受未知字段。
/// 这样可以让启动错误尽早暴露，而不是在 manager 交互时才出现半隐式失败。
pub fn parseStarterAdditionalParams(raw: &str) -> Result<starterParams> {
    let mut params = starterParams::default();
    let raw = raw.trim();
    // 空串合法，表示调用方没有提供任何 starter 补充参数。
    if raw.is_empty() {
        return Ok(params);
    }
    // 用 `seen` 做重复检测，避免后面的 key 覆盖前面的值造成行为不透明。
    let mut seen = HashMap::new();
    for item in raw.split(',') {
        let item = item.trim();
        // 连续逗号或尾逗号会制造空项，直接视为输入错误。
        if item.is_empty() {
            return Err(Error::new(
                "starter additional params contains an empty item",
            ));
        }
        let Some((key, value)) = item.split_once('=') else {
            return Err(Error::new(format!(
                "starter additional param {item:?} must be in k=v format"
            )));
        };
        let key = key.trim();
        let value = value.trim();
        // key/value 任一为空都说明参数不能形成稳定的语义绑定。
        if key.is_empty() {
            return Err(Error::new(format!(
                "starter additional param {item:?} has an empty key"
            )));
        }
        if value.is_empty() {
            return Err(Error::new(format!(
                "starter additional param {key:?} has an empty value"
            )));
        }
        if seen.contains_key(key) {
            return Err(Error::new(format!(
                "starter additional param {key:?} is duplicated"
            )));
        }
        seen.insert(key.to_string(), ());
        // 这里只接受入口真实会消费的字段，未知键直接视为配置错误。
        match key {
            "manager-namespace" => params.managerNamespace = value.into(),
            "pod-name" => params.podName = value.into(),
            "pod-ip" => params.podIP = value.into(),
            "pod-namespace" => params.podNamespace = value.into(),
            "enable-rg-fallback" => {
                params.enableRGFallback = match value {
                    "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                    "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                    _ => {
                        return Err(Error::new(format!(
                            "starter additional param {key:?} must be a bool: invalid syntax"
                        )));
                    }
                };
            }
            _ => {
                return Err(Error::new(format!(
                    "unknown starter additional param {key:?}"
                )));
            }
        }
    }
    Ok(params)
}

/// Applies the CLI-only fallback flag after validating all Starter params.
pub fn applyStarterAdditionalParams(cfg: &mut config::Config, raw: &str) -> Result<()> {
    let params = parseStarterAdditionalParams(raw)?;
    cfg.StarterParams.EnableRGFallback = params.enableRGFallback;
    Ok(())
}

/// 读取 starter 额外参数的当前全局值。
/// 该包装函数主要用于保持与 Go 版命名边界一致。
pub fn getStarterAdditionalParams() -> String {
    starter_additional_params()
}

/// 为 starter 模式构造 manager client。
///
/// 返回 `Option` 是因为两类情况都不是错误：
/// - 当前并非 starter 部署；
/// - starter 明确关闭了 manager notifier。
pub fn createMgrClientForStarter() -> Result<Option<tidbmanager::Client>> {
    if !deploymode::IsStarter() {
        return Ok(None);
    }
    let cfg = config::GetGlobalConfig();
    if !cfg.StarterParams.EnableManagerNotifier {
        return Ok(None);
    }
    let clusterSecurity = cfg.Security.ClusterSecurity();
    // manager client 复用 cluster security，因为它属于控制面内部通信。
    let tlsConfig = clusterSecurity.ToTLSConfig()?;
    let params = parseStarterAdditionalParams(&getStarterAdditionalParams())?;
    let mut managerAddr = cfg.StarterParams.ManagerAddr.clone();
    if managerAddr.is_empty() {
        // 未显式配置 manager 地址时，按约定的 Kubernetes service 名称推导。
        let managerNs = params.managerNamespace.clone();
        if managerNs.is_empty() {
            return Err(Error::new(
                "manager notifier requires manager-addr config or manager-namespace in --starter-additional-params",
            ));
        }
        managerAddr = format!("manager-server.{managerNs}.svc:8000");
    }
    let podName = params.podName;
    let podIP = params.podIP;
    let namespace = params.podNamespace;
    // manager notifier 依赖 pod 三元组标识本实例，因此缺一不可。
    if podName.is_empty() || podIP.is_empty() || namespace.is_empty() {
        return Err(Error::new(format!(
            "manager notifier requires --starter-additional-params with pod-name, pod-ip and pod-namespace: pod-name={podName:?}, pod-ip={podIP:?}, pod-namespace={namespace:?}"
        )));
    }
    Ok(Some(tidbmanager::NewClient(
        &managerAddr,
        tlsConfig,
        &podName,
        &podIP,
        &namespace,
    )))
}

/// 根据环境变量按需开启 pyroscope。
/// 未配置地址时静默跳过，配置了但启动失败则视为硬错误。
pub fn enablePyroscope() {
    if let Ok(addr) = std::env::var("PYROSCOPE_SERVER_ADDRESS") {
        if !addr.is_empty() {
            // 地址存在就视为用户显式要求启用 profiling，失败不能静默忽略。
            if let Err(_) = pyroscope::Start(&addr) {
                log::Fatal("fail to start pyroscope");
            }
        }
    }
}

/// 启用安全增强模式（SEM）。
/// 若提供自定义配置则走 `semv2`，否则使用默认 `sem` 开关。
pub fn setupSEM() {
    let cfg = config::GetGlobalConfig();
    if cfg.Security.EnableSEM {
        if !cfg.Security.SEMConfig.is_empty() {
            // 自定义 SEM 配置加载失败时直接 fatal，避免实例带着错误安全策略运行。
            if let Err(_) = semv2::Enable(&cfg.Security.SEMConfig) {
                logutil::BgLogger::Fatal("failed to enable SEM");
            }
        } else {
            // 没有自定义配置时启用默认 SEM 规则集。
            sem::Enable();
        }
    }
}

/// Wire the CLI opt-in to the shared Domain controller using the store's keyspace.
pub fn startStarterResourceGroupController(
    cfg: &config::Config,
    store: &astersql_store::TikvStore,
    domain: &Arc<astersql_domain::Domain>,
) -> Result<()> {
    if !kerneltype::IsNextGen()
        || cfg.DeployMode != deploymode::Starter
        || !cfg.StarterParams.EnableRGFallback
    {
        return Ok(());
    }
    let keyspace_id = astersql_metaservice::EtcdMetadataStore::keyspace_meta(store)
        .map_err(|error| Error::new(error.to_string()))?
        .map_or(u32::MAX, |meta| meta.id);
    let security = if cfg.Security.ClusterSSLCA.is_empty() {
        tikv_client::SecurityManager::default()
    } else {
        tikv_client::SecurityManager::load(
            &cfg.Security.ClusterSSLCA,
            &cfg.Security.ClusterSSLCert,
            &cfg.Security.ClusterSSLKey,
        )
        .map_err(|error| Error::new(error.to_string()))?
    };
    let endpoints = cfg
        .Path
        .trim_start_matches("tikv://")
        .split(',')
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .map(str::to_owned)
        .collect();
    let provider = tikv_client::resource_group_provider::GrpcResourceGroupProvider::connect_pd(
        endpoints,
        keyspace_id,
        Arc::new(security),
        Duration::from_secs(3),
    )
    .map_err(|error| Error::new(error.to_string()))?;
    domain
        .init_resource_groups_controller(Some(Arc::new(provider)), keyspace_id, true, true)
        .map_err(|error| Error::new(error.to_string()))
}
