// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 统计信息 HTTP 处理器：导出表统计、历史统计与优先级队列快照。
//
// 对齐 Go 侧 `StatsHandler` / `StatsHistoryHandler` / `StatsPriorityQueueHandler`。
// 统计信息（statistics）供优化器估算代价；历史统计按 snapshot（时间戳快照）回放；
// 优先级队列反映 Analyze 等统计任务的调度顺序。

#![allow(dead_code, non_snake_case)]

/// 统计导出运行时：路由参数、表解析、dump 与历史统计开关等依赖。
pub trait StatisticsRuntime<D> {
    type Error;
    type Table;
    type Payload;

    fn set_json_content_type(&mut self);
    fn route_value(&self, name: &str) -> String;
    fn query_values(&self, name: &str) -> Vec<String>;
    fn current_table(
        &mut self,
        domain: &D,
        database: &str,
        table: &str,
    ) -> Result<Self::Table, Self::Error>;
    fn snapshot_table(
        &mut self,
        domain: &D,
        snapshot: u64,
        database: &str,
        table: &str,
    ) -> Result<Self::Table, Self::Error>;
    fn dump_stats(
        &mut self,
        domain: &D,
        database: &str,
        table: &Self::Table,
        dump_partition_stats: bool,
    ) -> Result<Self::Payload, Self::Error>;
    fn historical_stats_enabled(&mut self, domain: &D) -> Result<bool, Self::Error>;
    fn parse_snapshot(&mut self, value: &str) -> Result<u64, Self::Error>;
    fn dump_historical_stats(
        &mut self,
        domain: &D,
        database: &str,
        table: &Self::Table,
        snapshot: u64,
    ) -> Result<Self::Payload, Self::Error>;
    fn priority_queue_snapshot(&mut self, domain: &D) -> Result<Self::Payload, Self::Error>;
    fn invalid_boolean(&mut self, value: &str) -> Self::Error;
    fn historical_stats_disabled(&mut self) -> Self::Error;
    fn log_snapshot_fallback(&mut self, error: &Self::Error);
    fn write_data(&mut self, data: &Self::Payload);
    fn write_error(&mut self, error: Self::Error);
}

/// 当前表统计 dump 的 HTTP 入口；持有 Domain（服务域）。
pub struct StatsHandler<D> {
    domain: D,
}

/// 构造 StatsHandler。
pub fn NewStatsHandler<D>(domain: D) -> StatsHandler<D> {
    StatsHandler { domain }
}

impl<D> StatsHandler<D> {
    /// 返回内部 Domain 引用。
    pub fn Domain(&self) -> &D {
        &self.domain
    }

    /// 导出当前表统计：解析 dump_partition_stats 查询参数后 dump。
    pub fn ServeHTTP<R>(&self, runtime: &mut R)
    where
        R: StatisticsRuntime<D>,
    {
        runtime.set_json_content_type();
        let database = runtime.route_value("db");
        let table_name = runtime.route_value("table");
        let dump_partition_stats = match runtime
            .query_values("dumpPartitionStats")
            .first()
            .filter(|value| !value.is_empty())
        {
            // 查询参数非法时写错误并中止，避免误 dump。
            Some(value) => match parse_go_boolean(value) {
                Some(value) => value,
                None => {
                    let error = runtime.invalid_boolean(value);
                    runtime.write_error(error);
                    return;
                }
            },
            None => true,
        };

        let table = match runtime.current_table(&self.domain, &database, &table_name) {
            Ok(table) => table,
            Err(error) => {
                runtime.write_error(error);
                return;
            }
        };
        match runtime.dump_stats(&self.domain, &database, &table, dump_partition_stats) {
            Ok(data) => runtime.write_data(&data),
            Err(error) => runtime.write_error(error),
        }
    }
}

/// 历史统计 dump 的 HTTP 入口。
pub struct StatsHistoryHandler<D> {
    domain: D,
}

/// 构造 StatsHistoryHandler。
pub fn NewStatsHistoryHandler<D>(domain: D) -> StatsHistoryHandler<D> {
    StatsHistoryHandler { domain }
}

impl<D> StatsHistoryHandler<D> {
    /// 导出历史统计：校验开关、解析 snapshot，必要时回退到当前表元数据。
    pub fn ServeHTTP<R>(&self, runtime: &mut R)
    where
        R: StatisticsRuntime<D>,
    {
        runtime.set_json_content_type();
        // 历史统计未开启时拒绝请求。
        match runtime.historical_stats_enabled(&self.domain) {
            Ok(true) => {}
            Ok(false) => {
                let error = runtime.historical_stats_disabled();
                runtime.write_error(error);
                return;
            }
            Err(error) => {
                runtime.write_error(error);
                return;
            }
        }

        let database = runtime.route_value("db");
        let table_name = runtime.route_value("table");
        let snapshot_value = runtime.route_value("snapshot");
        let snapshot = match runtime.parse_snapshot(&snapshot_value) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                runtime.write_error(error);
                return;
            }
        };
        let table =
            // 快照表不存在时回退到当前 InfoSchema 中的表。
            match getSnapshotTableInfo(&self.domain, snapshot, &database, &table_name, runtime) {
                Ok(table) => table,
                Err(error) => {
                    runtime.log_snapshot_fallback(&error);
                    match runtime.current_table(&self.domain, &database, &table_name) {
                        Ok(table) => table,
                        Err(error) => {
                            runtime.write_error(error);
                            return;
                        }
                    }
                }
            };
        match runtime.dump_historical_stats(&self.domain, &database, &table, snapshot) {
            Ok(data) => runtime.write_data(&data),
            Err(error) => runtime.write_error(error),
        }
    }
}

/// 按 snapshot 时间戳解析表元数据（InfoSchema 历史视图）。
pub fn getSnapshotTableInfo<D, R>(
    domain: &D,
    snapshot: u64,
    database: &str,
    table: &str,
    runtime: &mut R,
) -> Result<R::Table, R::Error>
where
    R: StatisticsRuntime<D>,
{
    runtime.snapshot_table(domain, snapshot, database, table)
}

/// 统计任务优先级队列快照的 HTTP 入口。
pub struct StatsPriorityQueueHandler<D> {
    domain: D,
}

/// 构造 StatsPriorityQueueHandler。
pub fn NewStatsPriorityQueueHandler<D>(domain: D) -> StatsPriorityQueueHandler<D> {
    StatsPriorityQueueHandler { domain }
}

impl<D> StatsPriorityQueueHandler<D> {
    /// 导出统计优先级队列快照。
    pub fn ServeHTTP<R>(&self, runtime: &mut R)
    where
        R: StatisticsRuntime<D>,
    {
        runtime.set_json_content_type();
        match runtime.priority_queue_snapshot(&self.domain) {
            Ok(data) => runtime.write_data(&data),
            Err(error) => runtime.write_error(error),
        }
    }
}

/// 解析 Go strconv.ParseBool 兼容的布尔字面量。
fn parse_go_boolean(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}
