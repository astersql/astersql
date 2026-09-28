// Copyright 2023 PingCAP, Inc.
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

// Server metric handles derived from the package-wide metric vectors.
//
// 服务端 Prometheus 指标句柄：由包级 CounterVec/HistogramVec 派生出按命令、结果、
// 资源组等标签预绑定的 Counter/Histogram，对齐 Go `metrics.go` 的 init 预热行为。

use prometheus::{Counter, CounterVec, Histogram, HistogramOpts, HistogramVec, Opts};
use std::sync::LazyLock;

/// 默认资源组名（未指定资源组时打到该标签）。
pub const DEFAULT_RESOURCE_GROUP_NAME: &str = "default";

/// MySQL 命令字节：COM_SLEEP。
pub const COM_SLEEP: u8 = 0;
/// MySQL 命令字节：COM_QUIT。
pub const COM_QUIT: u8 = 1;
/// MySQL 命令字节：COM_INIT_DB（切换库）。
pub const COM_INIT_DB: u8 = 2;
/// MySQL 命令字节：COM_QUERY（文本协议查询）。
pub const COM_QUERY: u8 = 3;
/// MySQL 命令字节：COM_FIELD_LIST。
pub const COM_FIELD_LIST: u8 = 4;
/// MySQL 命令字节：COM_CREATE_DB（指标向量中故意稀疏，不预绑定）。
pub const COM_CREATE_DB: u8 = 5;
/// MySQL 命令字节：COM_PING。
pub const COM_PING: u8 = 14;
/// MySQL 命令字节：COM_STMT_PREPARE。
pub const COM_STMT_PREPARE: u8 = 22;
/// MySQL 命令字节：COM_STMT_EXECUTE。
pub const COM_STMT_EXECUTE: u8 = 23;
/// MySQL 命令字节：COM_STMT_SEND_LONG_DATA。
pub const COM_STMT_SEND_LONG_DATA: u8 = 24;
/// MySQL 命令字节：COM_STMT_CLOSE。
pub const COM_STMT_CLOSE: u8 = 25;
/// MySQL 命令字节：COM_STMT_RESET。
pub const COM_STMT_RESET: u8 = 26;
/// MySQL 命令字节：COM_SET_OPTION。
pub const COM_SET_OPTION: u8 = 27;
/// MySQL 命令字节：COM_STMT_FETCH。
pub const COM_STMT_FETCH: u8 = 28;

/// 查询总数 CounterVec，标签：type / result / resource_group。
pub static QUERY_TOTAL_COUNTER: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new("query_total", "Counter of queries.")
            .namespace("tidb")
            .subsystem("server"),
        &["type", "result", "resource_group"],
    )
    .expect("query metric labels are valid")
});

/// 断连总数 CounterVec，标签：result。
static DISCONNECTION_COUNTER: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new(
            "disconnection_total",
            "Counter of connections disconnected.",
        )
        .namespace("tidb")
        .subsystem("server"),
        &["result"],
    )
    .expect("disconnection metric labels are valid")
});

/// 连接空闲时长直方图（秒），标签：in_txn（是否处于事务中）。
static CONN_IDLE_DURATION_HISTOGRAM: LazyLock<HistogramVec> = LazyLock::new(|| {
    HistogramVec::new(
        HistogramOpts::new(
            "conn_idle_duration_seconds",
            "Bucketed histogram of connection idle time (s).",
        )
        .namespace("tidb")
        .subsystem("server")
        .buckets(prometheus::exponential_buckets(0.0005, 2.0, 29).unwrap()),
        &["in_txn"],
    )
    .expect("idle-duration metric labels are valid")
});

/// 包 IO 字节 CounterVec，标签：type（In/Out）。
static PACKET_IO_COUNTER: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new("packet_io_bytes", "Counters of packet IO bytes.")
            .namespace("tidb")
            .subsystem("server"),
        &["type"],
    )
    .expect("packet metric labels are valid")
});

/// 需要预绑定查询计数器的命令字节 → 指标 type 标签名。
const QUERY_COMMANDS: &[(u8, &str)] = &[
    (COM_SLEEP, "Sleep"),
    (COM_QUIT, "Quit"),
    (COM_INIT_DB, "InitDB"),
    (COM_QUERY, "Query"),
    (COM_PING, "Ping"),
    (COM_FIELD_LIST, "FieldList"),
    (COM_STMT_PREPARE, "StmtPrepare"),
    (COM_STMT_EXECUTE, "StmtExecute"),
    (COM_STMT_FETCH, "StmtFetch"),
    (COM_STMT_CLOSE, "StmtClose"),
    (COM_STMT_SEND_LONG_DATA, "StmtSendLongData"),
    (COM_STMT_RESET, "StmtReset"),
    (COM_SET_OPTION, "SetOption"),
];

/// 按 result（OK/Error）为各已知命令预绑定 Counter；未列出命令槽位为 None（稀疏索引）。
fn query_counters(result: &str) -> Vec<Option<Counter>> {
    let mut counters = vec![None; COM_STMT_FETCH as usize + 1];
    for &(command, name) in QUERY_COMMANDS {
        counters[command as usize] = Some(QUERY_TOTAL_COUNTER.with_label_values(&[
            name,
            result,
            DEFAULT_RESOURCE_GROUP_NAME,
        ]));
    }
    counters
}

/// 成功查询计数器稀疏向量，下标为命令字节。
pub static QUERY_TOTAL_COUNT_OK: LazyLock<Vec<Option<Counter>>> =
    LazyLock::new(|| query_counters("OK"));
/// 失败查询计数器稀疏向量，下标为命令字节。
pub static QUERY_TOTAL_COUNT_ERR: LazyLock<Vec<Option<Counter>>> =
    LazyLock::new(|| query_counters("Error"));

/// 正常断连计数。
pub static DISCONNECT_NORMAL: LazyLock<Counter> =
    LazyLock::new(|| DISCONNECTION_COUNTER.with_label_values(&["ok"]));
/// 客户端报错导致的断连计数。
pub static DISCONNECT_BY_CLIENT_WITH_ERROR: LazyLock<Counter> =
    LazyLock::new(|| DISCONNECTION_COUNTER.with_label_values(&["error"]));
/// 原因未判定的断连计数。
pub static DISCONNECT_ERROR_UNDETERMINED: LazyLock<Counter> =
    LazyLock::new(|| DISCONNECTION_COUNTER.with_label_values(&["undetermined"]));

/// 非事务中的连接空闲时长直方图。
pub static CONN_IDLE_DURATION_HISTOGRAM_NOT_IN_TXN: LazyLock<Histogram> =
    LazyLock::new(|| CONN_IDLE_DURATION_HISTOGRAM.with_label_values(&["0"]));
/// 事务中的连接空闲时长直方图。
pub static CONN_IDLE_DURATION_HISTOGRAM_IN_TXN: LazyLock<Histogram> =
    LazyLock::new(|| CONN_IDLE_DURATION_HISTOGRAM.with_label_values(&["1"]));

/// 入站包字节计数。
pub static IN_PACKET_BYTES: LazyLock<Counter> =
    LazyLock::new(|| PACKET_IO_COUNTER.with_label_values(&["In"]));
/// 出站包字节计数。
pub static OUT_PACKET_BYTES: LazyLock<Counter> =
    LazyLock::new(|| PACKET_IO_COUNTER.with_label_values(&["Out"]));

/// Converts a MySQL command byte to the label used by the Go server metrics.
/// 将 MySQL 命令字节转为指标 type 标签；未知命令回退为十进制字符串。
pub fn cmd_to_string(cmd: u8) -> String {
    QUERY_COMMANDS
        .iter()
        .find_map(|&(command, name)| (command == cmd).then_some(name.to_owned()))
        .unwrap_or_else(|| cmd.to_string())
}

/// Initializes all package-level metric handles, matching Go's `init` hook.
/// 强制初始化全部包级指标句柄，对齐 Go 包 `init` 预热。
pub fn init_metrics_vars() {
    LazyLock::force(&QUERY_TOTAL_COUNT_OK);
    LazyLock::force(&QUERY_TOTAL_COUNT_ERR);
    LazyLock::force(&DISCONNECT_NORMAL);
    LazyLock::force(&DISCONNECT_BY_CLIENT_WITH_ERROR);
    LazyLock::force(&DISCONNECT_ERROR_UNDETERMINED);
    LazyLock::force(&CONN_IDLE_DURATION_HISTOGRAM_NOT_IN_TXN);
    LazyLock::force(&CONN_IDLE_DURATION_HISTOGRAM_IN_TXN);
    LazyLock::force(&IN_PACKET_BYTES);
    LazyLock::force(&OUT_PACKET_BYTES);
}

/// Explicit entry point for callers that previously relied on Go package init.
/// 显式入口，供原先依赖 Go 包 init 的调用方主动触发。
pub fn init() {
    init_metrics_vars();
}

// Preserve the exported Go names for package integration while keeping the
// implementation idiomatic and lint-clean internally.
// 保留与 Go 导出名一致的别名，便于包间集成。
pub use CONN_IDLE_DURATION_HISTOGRAM_IN_TXN as ConnIdleDurationHistogramInTxn;
pub use CONN_IDLE_DURATION_HISTOGRAM_NOT_IN_TXN as ConnIdleDurationHistogramNotInTxn;
pub use DISCONNECT_BY_CLIENT_WITH_ERROR as DisconnectByClientWithError;
pub use DISCONNECT_ERROR_UNDETERMINED as DisconnectErrorUndetermined;
pub use DISCONNECT_NORMAL as DisconnectNormal;
pub use IN_PACKET_BYTES as InPacketBytes;
pub use OUT_PACKET_BYTES as OutPacketBytes;
pub use QUERY_TOTAL_COUNT_ERR as QueryTotalCountErr;
pub use QUERY_TOTAL_COUNT_OK as QueryTotalCountOk;

/// Go 风格命名：命令字节 → 指标标签字符串。
#[allow(non_snake_case)]
pub fn CmdToString(cmd: u8) -> String {
    cmd_to_string(cmd)
}

/// Go 风格命名：初始化指标变量。
#[allow(non_snake_case)]
pub fn InitMetricsVars() {
    init_metrics_vars();
}
