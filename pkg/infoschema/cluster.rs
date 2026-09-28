// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 集群表（Cluster Table）路由与实例地址辅助。
//
// 集群表是 `information_schema` / `performance_schema` 下以 `CLUSTER_` 为前缀的
// 内存表视图：把单节点上的慢查询、进程列表、事务摘要等表，扩展为可从多个
// TiDB 节点（或仅从 DDL Owner）汇总数据的形态。DDL Owner 指当前负责执行
// DDL（数据定义语言，如 CREATE/ALTER TABLE）的那一个 TiDB 实例。
//
// 本模块提供：
// - 各集群表名常量与本地内存表 → 集群表的映射；
// - Coprocessor 请求目标（全部 TiDB / DDL Owner）的判定；
// - 按库表名识别是否为集群表；
// - 为结果行前置本实例地址（含 SEM 安全增强模式下的脱敏）。

#![allow(non_upper_case_globals, non_snake_case)]

/// 集群慢查询表名（对应单节点 `SLOW_QUERY`）。
pub const ClusterTableSlowLog: &str = "CLUSTER_SLOW_QUERY";
/// 集群进程列表表名。
pub const ClusterTableProcesslist: &str = "CLUSTER_PROCESSLIST";
/// 集群语句摘要表名。
pub const ClusterTableStatementsSummary: &str = "CLUSTER_STATEMENTS_SUMMARY";
/// 集群语句摘要历史表名。
pub const ClusterTableStatementsSummaryHistory: &str = "CLUSTER_STATEMENTS_SUMMARY_HISTORY";
/// 集群语句摘要淘汰记录表名。
pub const ClusterTableStatementsSummaryEvicted: &str = "CLUSTER_STATEMENTS_SUMMARY_EVICTED";
/// 集群 TiDB 语句统计表名。
pub const ClusterTableTiDBStatementsStats: &str = "CLUSTER_TIDB_STATEMENTS_STATS";
/// 集群 TiDB 事务（Transaction）表名。
pub const ClusterTableTiDBTrx: &str = "CLUSTER_TIDB_TRX";
/// 集群死锁表名。
pub const ClusterTableDeadlocks: &str = "CLUSTER_DEADLOCKS";
/// 集群事务摘要表名。
pub const ClusterTableTrxSummary: &str = "CLUSTER_TRX_SUMMARY";
/// 集群内存使用表名。
pub const ClusterTableMemoryUsage: &str = "CLUSTER_MEMORY_USAGE";
/// 集群内存使用运维历史表名。
pub const ClusterTableMemoryUsageOpsHistory: &str = "CLUSTER_MEMORY_USAGE_OPS_HISTORY";
/// 集群索引使用情况表名。
pub const ClusterTableTiDBIndexUsage: &str = "CLUSTER_TIDB_INDEX_USAGE";
/// 集群计划缓存（Plan Cache）表名。
pub const ClusterTableTiDBPlanCache: &str = "CLUSTER_TIDB_PLAN_CACHE";

/// 本地内存表名 → 需向全部 TiDB 节点拉取的集群表名映射。
const ALL_TIDB_TABLES: &[(&str, &str)] = &[
    ("SLOW_QUERY", ClusterTableSlowLog),
    ("PROCESSLIST", ClusterTableProcesslist),
    ("STATEMENTS_SUMMARY", ClusterTableStatementsSummary),
    (
        "STATEMENTS_SUMMARY_HISTORY",
        ClusterTableStatementsSummaryHistory,
    ),
    (
        "STATEMENTS_SUMMARY_EVICTED",
        ClusterTableStatementsSummaryEvicted,
    ),
    ("TIDB_STATEMENTS_STATS", ClusterTableTiDBStatementsStats),
    ("TIDB_TRX", ClusterTableTiDBTrx),
    ("DEADLOCKS", ClusterTableDeadlocks),
    ("TRX_SUMMARY", ClusterTableTrxSummary),
    ("MEMORY_USAGE", ClusterTableMemoryUsage),
    (
        "MEMORY_USAGE_OPS_HISTORY",
        ClusterTableMemoryUsageOpsHistory,
    ),
    ("TIDB_INDEX_USAGE", ClusterTableTiDBIndexUsage),
    ("TIDB_PLAN_CACHE", ClusterTableTiDBPlanCache),
];
/// 仅向 DDL Owner 请求的表映射（当前为 TiFlash 副本信息）。
const DDL_OWNER_TABLES: &[(&str, &str)] = &[("TIFLASH_REPLICA", "TIFLASH_REPLICA")];

/// 集群表 Coprocessor（协处理器）请求的目标节点类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterTableCopDestination {
    /// 向集群内全部 TiDB 节点收集。
    AllTiDB,
    /// 仅向当前 DDL Owner 收集。
    DDLOwner,
}

/// 根据表名决定集群表数据应从何处拉取；DDL Owner 映射优先。
pub fn GetClusterTableCopDestination(table_name: &str) -> ClusterTableCopDestination {
    if DDL_OWNER_TABLES.iter().any(|(source, cluster)| {
        source.eq_ignore_ascii_case(table_name) || cluster.eq_ignore_ascii_case(table_name)
    }) {
        ClusterTableCopDestination::DDLOwner
    } else {
        ClusterTableCopDestination::AllTiDB
    }
}

/// 判断给定库表是否为 information_schema / performance_schema 下的集群表。
pub fn IsClusterTableByName(db_name: &str, table_name: &str) -> bool {
    if !matches!(db_name, "information_schema" | "performance_schema") {
        return false;
    }
    ALL_TIDB_TABLES
        .iter()
        .chain(DDL_OWNER_TABLES)
        .any(|(_, cluster)| cluster.to_ascii_lowercase() == table_name)
}

/// 行单元格值的简化 Datum（数据单元）枚举，供集群表行改写使用。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    String(String),
    Integer(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
}

/// 本 TiDB 实例的服务身份信息（ID、IP、状态端口）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerInfo {
    pub id: String,
    pub ip: String,
    pub status_port: u16,
}

/// 获取实例地址与 SEM（Security Enhanced Mode，安全增强模式）权限所需的会话上下文。
pub trait ClusterSessionContext {
    fn server_info(&self) -> Result<ServerInfo, String>;
    fn sem_enabled(&self) -> bool;
    fn can_read_restricted_tables(&self) -> bool;
}

/// 返回本实例对外展示的地址：默认 `IP:statusPort`；SEM 开启且无受限表权限时改为 server ID。
pub fn GetInstanceAddr(ctx: &dyn ClusterSessionContext) -> Result<String, String> {
    let server = ctx.server_info()?;
    if ctx.sem_enabled() && !ctx.can_read_restricted_tables() {
        Ok(server.id)
    } else if server.ip.contains(':') {
        Ok(format!(
            "[{}]:{}",
            server.ip.trim_matches(['[', ']']),
            server.status_port
        ))
    } else {
        Ok(format!("{}:{}", server.ip, server.status_port))
    }
}

/// 为每一行结果前置实例地址列；地址获取失败则整批返回错误，避免行形状不一致。
pub fn AppendHostInfoToRows(
    ctx: &dyn ClusterSessionContext,
    rows: Vec<Vec<Datum>>,
) -> Result<Vec<Vec<Datum>>, String> {
    let address = GetInstanceAddr(ctx)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut result = Vec::with_capacity(row.len() + 1);
            result.push(Datum::String(address.clone()));
            result.extend(row);
            result
        })
        .collect())
}
