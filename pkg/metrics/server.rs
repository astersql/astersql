// Copyright 2018 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TiDB Server 层 Prometheus 指标定义。
//
// 覆盖连接、查询耗时、计划缓存（Plan Cache）、慢查询、Token 并发控制、TLS、
// TiFlash/PD API 等服务器入口侧观测点。本文件只构造句柄，不监听网络或执行查询。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这里只构造 Prometheus 指标句柄，不会注册或采集指标，不会监听网络、建立连接或执行查询。

// 测试可把该开关设为 true，以允许重置 plan cache counter；名称中的 FortTest 沿用 Go 源码。
/// 测试专用开关：为 true 时允许重置计划缓存计数器。
pub static mut ResettablePlanCacheCounterFortTest: bool = false;

// 服务器指标句柄。Option 明确表示 InitServerMetrics 调用前的未初始化状态。
/// 网络包收发字节计数。
pub static mut PacketIOCounter: Option<prometheus::CounterVec> = None;
/// 查询处理耗时直方图（秒）。
pub static mut QueryDurationHistogram: Option<prometheus::HistogramVec> = None;
pub static mut CommandDurationHistogram: Option<prometheus::HistogramVec> = None;
/// 单条语句触发的 RPC 次数分布。
pub static mut QueryRPCHistogram: Option<prometheus::HistogramVec> = None;
/// 扫描过程中处理的键数分布。
pub static mut QueryProcessedKeyHistogram: Option<prometheus::HistogramVec> = None;
pub static mut IACacheHitCount: Option<prometheus::CounterVec> = None;
pub static mut IARemoteReadSegmentCount: Option<prometheus::CounterVec> = None;
pub static mut IARemoteReadSegmentSize: Option<prometheus::CounterVec> = None;
pub static mut IARemoteReadSegmentWaitDuration: Option<prometheus::HistogramVec> = None;
/// 查询总次数。
pub static mut QueryTotalCounter: Option<prometheus::CounterVec> = None;
/// 当前连接数（可按资源组划分）。
pub static mut ConnGauge: Option<prometheus::GaugeVec> = None;
/// 断开连接次数。
pub static mut DisconnectionCounter: Option<prometheus::CounterVec> = None;
/// 当前预处理语句（prepared statement）数量。
pub static mut PreparedStmtGauge: Option<prometheus::Gauge> = None;
/// 执行错误次数。
pub static mut ExecuteErrorCounter: Option<prometheus::CounterVec> = None;
/// 严重错误次数。
pub static mut CriticalErrorCounter: Option<prometheus::Counter> = None;

/// 服务器启动事件标签值。
pub const ServerStart: &str = "server-start";
/// 服务器停止事件标签值。
pub const ServerStop: &str = "server-stop";
// EventKill 对应 server.Kill() 被调用时记录的事件标签。
/// Kill 连接/会话事件标签值。
pub const EventKill: &str = "kill";

/// 服务器生命周期事件计数。
pub static mut ServerEventCounter: Option<prometheus::CounterVec> = None;
/// 系统时间回拨次数。
pub static mut TimeJumpBackCounter: Option<prometheus::Counter> = None;
/// 命中计划缓存的次数。
pub static mut PlanCacheCounter: Option<prometheus::CounterVec> = None;
/// 计划缓存未命中次数。
pub static mut PlanCacheMissCounter: Option<prometheus::CounterVec> = None;
/// 实例级计划缓存内存占用。
pub static mut PlanCacheInstanceMemoryUsage: Option<prometheus::GaugeVec> = None;
/// 实例级计划缓存中的计划数量。
pub static mut PlanCacheInstancePlanNumCounter: Option<prometheus::GaugeVec> = None;
/// 计划缓存操作耗时（秒）。
pub static mut PlanCacheProcessDuration: Option<prometheus::HistogramVec> = None;
/// 从 table cache 读到结果的次数。
pub static mut ReadFromTableCacheCounter: Option<prometheus::Counter> = None;
/// 握手错误次数。
pub static mut HandShakeErrorCounter: Option<prometheus::Counter> = None;
/// 获取执行 Token 的耗时（桶单位沿用 Go 的 us 帮助文本）。
pub static mut GetTokenDurationHistogram: Option<prometheus::Histogram> = None;
/// 一条 multi-query 中包含的语句数分布。
pub static mut NumOfMultiQueryHistogram: Option<prometheus::Histogram> = None;
/// 慢查询处理耗时（秒）。
pub static mut TotalQueryProcHistogram: Option<prometheus::HistogramVec> = None;
/// 慢查询中全部 Coprocessor 处理耗时（秒）。
pub static mut TotalCopProcHistogram: Option<prometheus::HistogramVec> = None;
/// 慢查询中全部 Coprocessor 等待耗时（秒）。
pub static mut TotalCopWaitHistogram: Option<prometheus::HistogramVec> = None;
/// 慢查询中 Coprocessor total keys / processed keys 比值（MVCC 扫描放大）。
pub static mut CopMVCCRatioHistogram: Option<prometheus::HistogramVec> = None;
/// 慢查询次数。
pub static mut SlowQueryCounter: Option<prometheus::CounterVec> = None;
/// GOMAXPROCS 当前值。
pub static mut MaxProcs: Option<prometheus::Gauge> = None;
/// GOGC 当前值。
pub static mut GOGC: Option<prometheus::Gauge> = None;
/// 连接空闲时长（秒）。
pub static mut ConnIdleDurationHistogram: Option<prometheus::HistogramVec> = None;
/// 服务器信息（值为启动时间戳秒）。
pub static mut ServerInfo: Option<prometheus::GaugeVec> = None;
/// 当前并发执行会话占用的 Token 数。
pub static mut TokenGauge: Option<prometheus::Gauge> = None;
/// 配置项状态。
pub static mut ConfigStatus: Option<prometheus::GaugeVec> = None;
/// TiFlash 查询次数。
pub static mut TiFlashQueryTotalCounter: Option<prometheus::CounterVec> = None;
/// TiFlash MPP store 故障状态。
pub static mut TiFlashFailedMPPStoreState: Option<prometheus::GaugeVec> = None;
/// PD HTTP API 执行耗时（秒）。
pub static mut PDAPIExecutionHistogram: Option<prometheus::HistogramVec> = None;
/// PD HTTP API 请求次数。
pub static mut PDAPIRequestCounter: Option<prometheus::CounterVec> = None;
/// CPU profiling 触发次数。
pub static mut CPUProfileCounter: Option<prometheus::Counter> = None;
/// 加载 table cache 的耗时。
pub static mut LoadTableCacheDurationHistogram: Option<prometheus::Histogram> = None;
/// RCCheckTS 引发的写冲突次数（RC 为 Read Committed 隔离级别）。
pub static mut RCCheckTSWriteConfilictCounter: Option<prometheus::CounterVec> = None;
/// 内存配额字节数。
pub static mut MemoryLimit: Option<prometheus::Gauge> = None;
/// 内部会话数量。
pub static mut InternalSessions: Option<prometheus::Gauge> = None;
/// 活跃用户数。
pub static mut ActiveUser: Option<prometheus::Gauge> = None;
// TLS 指标按协商出的版本和 cipher 分别计数。
/// 按 TLS 版本累计握手次数。
pub static mut TLSVersion: Option<prometheus::CounterVec> = None;
/// 按 TLS cipher 累计握手次数。
pub static mut TLSCipher: Option<prometheus::CounterVec> = None;

/// Record one SQL statement duration using the statement's database and resource-group labels.
pub fn RecordQueryDuration(sql_type: &str, database: &str, resource_group: &str, seconds: f64) {
    let _guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let histogram = unsafe {
        (&*std::ptr::addr_of!(QueryDurationHistogram))
            .as_ref()
            .cloned()
    };
    if let Some(histogram) = histogram {
        histogram
            .with_label_values(&[sql_type, database, resource_group])
            .observe(seconds);
    }
}

/// Record one protocol command duration independently from its individual SQL statements.
pub fn RecordCommandDuration(sql_type: &str, database: &str, resource_group: &str, seconds: f64) {
    let _guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let histogram = unsafe {
        (&*std::ptr::addr_of!(CommandDurationHistogram))
            .as_ref()
            .cloned()
    };
    if let Some(histogram) = histogram {
        histogram
            .with_label_values(&[sql_type, database, resource_group])
            .observe(seconds);
    }
}

fn counter(subsystem: &'static str, name: &'static str, help: &'static str) -> prometheus::Counter {
    metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: subsystem,
        Name: name,
        Help: help,
        ..Default::default()
    })
}

fn counter_vec(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    labels: &[&'static str],
) -> prometheus::CounterVec {
    metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: subsystem,
            Name: name,
            Help: help,
            ..Default::default()
        },
        labels.to_vec(),
    )
}

fn gauge(subsystem: &'static str, name: &'static str, help: &'static str) -> prometheus::Gauge {
    metricscommon::NewGauge(prometheus::GaugeOpts {
        Namespace: "tidb",
        Subsystem: subsystem,
        Name: name,
        Help: help,
        ..Default::default()
    })
}

fn gauge_vec(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    labels: &[&'static str],
) -> prometheus::GaugeVec {
    metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: subsystem,
            Name: name,
            Help: help,
            ..Default::default()
        },
        labels.to_vec(),
    )
}

// histogram 与 histogram_vec 保留 Go 的显式 buckets；调用处旁注原始量级，便于核对时间单位。
/// 构造带显式桶边界的 Histogram。
fn histogram(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
) -> prometheus::Histogram {
    metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: subsystem,
        Name: name,
        Help: help,
        Buckets: buckets,
        ..Default::default()
    })
}

fn histogram_vec(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
    labels: &[&'static str],
) -> prometheus::HistogramVec {
    metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: subsystem,
            Name: name,
            Help: help,
            Buckets: buckets,
            ..Default::default()
        },
        labels.to_vec(),
    )
}

// InitServerMetrics 对应 Go 的完整服务器指标初始化；这里不执行注册，重复调用仍会替换包级句柄。
/// 按 Go 顺序初始化全部服务器侧指标句柄。
pub fn InitServerMetrics() {
    unsafe {
        PacketIOCounter = Some(counter_vec(
            "server",
            "packet_io_bytes",
            "Counters of packet IO bytes.",
            &[LblType],
        ));
        // 0.5ms 到约 1.5 天。
        QueryDurationHistogram = Some(histogram_vec(
            "server",
            "handle_query_duration_seconds",
            "Bucketed histogram of processing time (s) of individual SQL statements.",
            prometheus::ExponentialBuckets(0.0005, 2.0, 29),
            &[LblSQLType, LblDb, LblResourceGroup],
        ));
        CommandDurationHistogram = Some(histogram_vec(
            "server",
            "handle_command_duration_seconds",
            "Bucketed histogram of processing time (s) of handled commands and restricted SQL operations.",
            prometheus::ExponentialBuckets(0.0005, 2.0, 29),
            &[LblSQLType, LblDb, LblResourceGroup],
        ));
        QueryRPCHistogram = Some(histogram_vec(
            "server",
            "query_statement_rpc_count",
            "Bucketed histogram of execution rpc count of handled query statements.",
            prometheus::ExponentialBuckets(1.0, 1.5, 23),
            &[LblSQLType, LblDb],
        ));
        QueryProcessedKeyHistogram = Some(histogram_vec(
            "server",
            "query_statement_processed_keys",
            "Bucketed histogram of processed key count during the scan of handled query statements.",
            prometheus::ExponentialBuckets(1.0, 2.0, 32),
            &[LblSQLType, LblDb],
        ));
        IACacheHitCount = Some(counter_vec(
            "server",
            "ia_cache_hit_count",
            "Counter of IA segment cache hits observed by TiDB.",
            &[LblSQLType, LblDb],
        ));
        IARemoteReadSegmentCount = Some(counter_vec(
            "server",
            "ia_remote_read_segment_count",
            "Counter of IA remote read segments observed by TiDB.",
            &[LblSQLType, LblDb],
        ));
        IARemoteReadSegmentSize = Some(counter_vec(
            "server",
            "ia_remote_read_segment_size_bytes",
            "Counter of IA remote read segment bytes observed by TiDB.",
            &[LblSQLType, LblDb],
        ));
        IARemoteReadSegmentWaitDuration = Some(histogram_vec(
            "server",
            "ia_remote_read_segment_wait_duration_seconds",
            "Bucketed histogram of IA remote read segment wait time observed by TiDB.",
            prometheus::ExponentialBuckets(0.00005, 2.0, 20),
            &[LblSQLType, LblDb],
        ));
        QueryTotalCounter = Some(counter_vec(
            "server",
            "query_total",
            "Counter of queries.",
            &[LblType, LblResult, LblResourceGroup],
        ));
        ConnGauge = Some(gauge_vec(
            "server",
            "connections",
            "Number of connections.",
            &[LblResourceGroup],
        ));
        DisconnectionCounter = Some(counter_vec(
            "server",
            "disconnection_total",
            "Counter of connections disconnected.",
            &[LblResult],
        ));
        PreparedStmtGauge = Some(gauge(
            "server",
            "prepared_stmts",
            "number of prepared statements.",
        ));
        ExecuteErrorCounter = Some(counter_vec(
            "server",
            "execute_error_total",
            "Counter of execute errors.",
            &[LblType, LblDb, LblResourceGroup],
        ));
        CriticalErrorCounter = Some(counter(
            "server",
            "critical_error_total",
            "Counter of critical errors.",
        ));
        ServerEventCounter = Some(counter_vec(
            "server",
            "event_total",
            "Counter of tidb-server event.",
            &[LblType],
        ));
        TimeJumpBackCounter = Some(counter(
            "monitor",
            "time_jump_back_total",
            "Counter of system time jumps backward.",
        ));
        PlanCacheCounter = Some(counter_vec(
            "server",
            "plan_cache_total",
            "Counter of query using plan cache.",
            &[LblType],
        ));
        PlanCacheMissCounter = Some(counter_vec(
            "server",
            "plan_cache_miss_total",
            "Counter of plan cache miss.",
            &[LblType],
        ));
        PlanCacheInstanceMemoryUsage = Some(gauge_vec(
            "server",
            "plan_cache_instance_memory_usage",
            "Total plan cache memory usage of all sessions in a instance",
            &[LblType],
        ));
        PlanCacheInstancePlanNumCounter = Some(gauge_vec(
            "server",
            "plan_cache_instance_plan_num_total",
            "Counter of plan of all prepared plan cache in a instance",
            &[LblType],
        ));
        PlanCacheProcessDuration = Some(histogram_vec(
            "server",
            "plan_cache_process_duration_seconds",
            "Bucketed histogram of processing time (s) of plan cache operations.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblType],
        ));
        ReadFromTableCacheCounter = Some(counter(
            "server",
            "read_from_tablecache_total",
            "Counter of query read from table cache.",
        ));
        HandShakeErrorCounter = Some(counter(
            "server",
            "handshake_error_total",
            "Counter of hand shake error.",
        ));
        // Go 的帮助文字写 us，但桶值原样保留 1、2 倍增 30 桶。
        GetTokenDurationHistogram = Some(histogram(
            "server",
            "get_token_duration_seconds",
            "Duration (us) for getting token, it should be small until concurrency limit is reached.",
            prometheus::ExponentialBuckets(1.0, 2.0, 30),
        ));
        NumOfMultiQueryHistogram = Some(histogram(
            "server",
            "multi_query_num",
            "The number of queries contained in a multi-query statement.",
            prometheus::ExponentialBuckets(1.0, 2.0, 20),
        ));
        TotalQueryProcHistogram = Some(histogram_vec(
            "server",
            "slow_query_process_duration_seconds",
            "Bucketed histogram of processing time (s) of of slow queries.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblSQLType],
        ));
        TotalCopProcHistogram = Some(histogram_vec(
            "server",
            "slow_query_cop_duration_seconds",
            "Bucketed histogram of all cop processing time (s) of of slow queries.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblSQLType],
        ));
        TotalCopWaitHistogram = Some(histogram_vec(
            "server",
            "slow_query_wait_duration_seconds",
            "Bucketed histogram of all cop waiting time (s) of of slow queries.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblSQLType],
        ));
        CopMVCCRatioHistogram = Some(histogram_vec(
            "server",
            "slow_query_cop_mvcc_ratio",
            "Bucketed histogram of all cop total keys / processed keys in slow queries.",
            prometheus::ExponentialBuckets(0.5, 2.0, 21),
            &[LblSQLType],
        ));
        SlowQueryCounter = Some(counter_vec(
            "server",
            "slow_query_total",
            "Counter of slow queries.",
            &[LblSQLType],
        ));
        MaxProcs = Some(gauge("server", "maxprocs", "The value of GOMAXPROCS."));
        GOGC = Some(gauge("server", "gogc", "The value of GOGC"));
        ConnIdleDurationHistogram = Some(histogram_vec(
            "server",
            "conn_idle_duration_seconds",
            "Bucketed histogram of connection idle time (s).",
            prometheus::ExponentialBuckets(0.0005, 2.0, 29),
            &[LblInTxn],
        ));
        ServerInfo = Some(gauge_vec(
            "server",
            "info",
            "Indicate the tidb server info, and the value is the start timestamp (s).",
            &[LblVersion, LblHash],
        ));
        TokenGauge = Some(gauge(
            "server",
            "tokens",
            "The number of concurrent executing session",
        ));
        ConfigStatus = Some(gauge_vec(
            "config",
            "status",
            "Status of the TiDB server configurations.",
            &[LblType],
        ));
        TiFlashQueryTotalCounter = Some(counter_vec(
            "server",
            "tiflash_query_total",
            "Counter of TiFlash queries.",
            &[LblType, LblResult],
        ));
        TiFlashFailedMPPStoreState = Some(gauge_vec(
            "server",
            "tiflash_failed_store",
            "Statues of failed tiflash mpp store,-1 means detector heartbeat,0 means reachable,1 means abnormal.",
            &[LblAddress],
        ));
        PDAPIExecutionHistogram = Some(histogram_vec(
            "server",
            "pd_api_execution_duration_seconds",
            "Bucketed histogram of all pd api execution time (s)",
            prometheus::ExponentialBuckets(0.001, 2.0, 20),
            &[LblType],
        ));
        PDAPIRequestCounter = Some(counter_vec(
            "server",
            "pd_api_request_total",
            "Counter of the pd http api requests",
            &[LblType, LblResult],
        ));
        CPUProfileCounter = Some(counter(
            "server",
            "cpu_profile_total",
            "Counter of cpu profiling",
        ));
        LoadTableCacheDurationHistogram = Some(histogram(
            "server",
            "load_table_cache_seconds",
            "Duration (us) for loading table cache.",
            prometheus::ExponentialBuckets(1.0, 2.0, 30),
        ));
        RCCheckTSWriteConfilictCounter = Some(counter_vec(
            "server",
            "rc_check_ts_conflict_total",
            "Counter of WriteConflict caused by RCCheckTS.",
            &[LblType],
        ));
        MemoryLimit = Some(gauge(
            "server",
            "memory_quota_bytes",
            "The value of memory quota bytes.",
        ));
        InternalSessions = Some(gauge(
            "server",
            "internal_sessions",
            "The total count of internal sessions.",
        ));
        ActiveUser = Some(gauge(
            "server",
            "active_users",
            "The total count of active user.",
        ));
        TLSVersion = Some(counter_vec(
            "server",
            "tls_version",
            "Counter per TLS Version.",
            &[LblVersion],
        ));
        TLSCipher = Some(counter_vec(
            "server",
            "tls_cipher",
            "Counter per TLS Cipher.",
            &[LblCipher],
        ));
    }
}

// ExecuteErrorToLabel 把上游解析出的 terror RFCCode 作为指标标签；普通错误传 None。
/// 将 terror RFC 错误码转为执行错误指标标签；缺失时返回 `"unknown"`。
pub fn ExecuteErrorToLabel(rfc_code: Option<&str>) -> String {
    rfc_code.unwrap_or("unknown").to_owned()
}
