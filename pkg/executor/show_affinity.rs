// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `SHOW AFFINITY` 执行逻辑：汇总表/分区亲和组的 Region 放置状态。
//
// Affinity（亲和性）将一组 Region（键空间分片）绑定到指定 Store（存储节点），
// 便于就近计算或隔离工作负载。本模块按表级或分区级展开亲和组，再填充
// leader/voter、阶段与 Region 计数等展示列。

#![allow(non_snake_case)]

use std::collections::HashMap;

/// 亲和作用粒度：整表、分区，或其他未识别级别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffinityLevel {
    Table,
    Partition,
    Other,
}

/// 单个分区在亲和视图中的标识。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinityPartition {
    pub id: i64,
    pub name: String,
}

/// 带亲和配置的表元信息（含分区列表与作用级别）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinityTable {
    pub id: i64,
    pub name: String,
    pub lowercase_name: String,
    pub level: Option<AffinityLevel>,
    pub partitions: Vec<AffinityPartition>,
}

/// 同一库下若干带亲和配置的表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinitySchemaTables {
    pub database_name: String,
    pub tables: Vec<AffinityTable>,
}

/// 某个亲和组当前的放置状态快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffinityState {
    /// 亲和组当前 leader 所在 Store ID；0 表示尚无 leader。
    pub leader_store_id: u64,
    /// 参与投票的 Store ID 列表。
    pub voter_store_ids: Vec<u64>,
    /// 亲和推进阶段，如 pending / preparing / stable。
    pub phase: String,
    /// 组内 Region 总数。
    pub region_count: u64,
    /// 已满足亲和约束的 Region 数。
    pub affinity_region_count: u64,
}

/// `SHOW AFFINITY` 结果行中的单元格值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShowAffinityValue {
    String(String),
    U64(u64),
    Null,
}

/// 运行时边界：过滤条件、表清单、组状态查询与结果行写出。
pub trait ShowAffinityRuntime {
    type Context;
    type Error;

    /// 精确表名过滤（非空时只匹配该表）。
    fn field_filter(&self) -> Option<String>;
    /// LIKE 模式是否匹配小写表名。
    fn field_pattern_matches(&self, lowercase_table_name: &str) -> bool;
    /// 返回所有已配置亲和的库表。
    fn tables_with_affinity(&self) -> Vec<AffinitySchemaTables>;
    /// 表级亲和组 ID。
    fn table_group_id(&self, table_id: i64) -> String;
    /// 分区级亲和组 ID。
    fn partition_group_id(&self, table_id: i64, partition_id: i64) -> String;
    /// 批量拉取各亲和组的放置状态。
    fn all_group_states(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<HashMap<String, AffinityState>, Self::Error>;
    /// 追加一行展示结果。
    fn append_row(&mut self, row: Vec<ShowAffinityValue>);
}

/// 待查询状态的表/分区与亲和组对应关系。
struct TablePartitionInfo {
    database_name: String,
    table_name: String,
    partition_name: Option<String>,
    group_id: String,
}

/// 执行 `SHOW AFFINITY`：按过滤条件展开表/分区，再填充组状态列。
pub fn fetchShowAffinity<R: ShowAffinityRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
) -> Result<(), R::Error> {
    let field_filter = runtime.field_filter().unwrap_or_default();
    let tables = runtime.tables_with_affinity();
    let mut infos = Vec::with_capacity(tables.len());

    // 先按精确名或 LIKE 过滤，再按亲和级别展开为组 ID 列表。
    for schema in tables {
        for table in schema.tables {
            let Some(level) = table.level else {
                continue;
            };
            if !field_filter.is_empty() && table.lowercase_name != field_filter {
                continue;
            }
            if !runtime.field_pattern_matches(&table.lowercase_name) {
                continue;
            }
            match level {
                AffinityLevel::Table => infos.push(TablePartitionInfo {
                    database_name: schema.database_name.clone(),
                    table_name: table.name,
                    partition_name: None,
                    group_id: runtime.table_group_id(table.id),
                }),
                AffinityLevel::Partition => {
                    for partition in table.partitions {
                        infos.push(TablePartitionInfo {
                            database_name: schema.database_name.clone(),
                            table_name: table.name.clone(),
                            partition_name: Some(partition.name),
                            group_id: runtime.partition_group_id(table.id, partition.id),
                        });
                    }
                }
                AffinityLevel::Other => {}
            }
        }
    }

    // 一次拉取全部组状态，再编码为可见列（leader / voters / phase / counts）。
    let states = runtime.all_group_states(context)?;
    for info in infos {
        let (leader, voters, status, regions, affinity_regions) =
            if let Some(state) = states.get(&info.group_id) {
                let leader = (state.leader_store_id != 0)
                    .then_some(ShowAffinityValue::U64(state.leader_store_id))
                    .unwrap_or(ShowAffinityValue::Null);
                let voters = if state.voter_store_ids.is_empty() {
                    ShowAffinityValue::Null
                } else {
                    ShowAffinityValue::String(
                        state
                            .voter_store_ids
                            .iter()
                            .map(u64::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                };
                // 内部小写阶段名映射为展示用首字母大写文案。
                let phase = match state.phase.as_str() {
                    "pending" => "Pending",
                    "preparing" => "Preparing",
                    "stable" => "Stable",
                    other => other,
                };
                (
                    leader,
                    voters,
                    ShowAffinityValue::String(phase.to_owned()),
                    ShowAffinityValue::U64(state.region_count),
                    ShowAffinityValue::U64(state.affinity_region_count),
                )
            } else {
                (
                    ShowAffinityValue::Null,
                    ShowAffinityValue::Null,
                    ShowAffinityValue::Null,
                    ShowAffinityValue::Null,
                    ShowAffinityValue::Null,
                )
            };
        runtime.append_row(vec![
            ShowAffinityValue::String(info.database_name),
            ShowAffinityValue::String(info.table_name),
            info.partition_name
                .map(ShowAffinityValue::String)
                .unwrap_or(ShowAffinityValue::Null),
            leader,
            voters,
            status,
            regions,
            affinity_regions,
        ]);
    }
    Ok(())
}
