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

// 表亲和组（affinity group）管理模块。
//
// 亲和组是 PD（Placement Driver，TiDB 集群的调度中心）提供的一种调度约束：
// 把一张表或一个分区对应的 key 范围声明为一个组，让 PD 尽量将该范围内的
// Region（TiKV 中连续 key 区间的数据分片，也是调度与复制的基本单位）调度到
// 相同或相近的存储节点上，从而提升局部性、减少跨节点访问。
//
// 本模块负责：
// - 按表级 / 分区级两种亲和级别生成组 ID（`_tidb_t_{tableID}` 与
//   `_tidb_pt_{tableID}_p{partitionID}`）；
// - 根据表或分区的物理 ID 构造对应的 key range（键区间）；
// - 在 DDL（数据定义语言，如 CREATE/DROP/TRUNCATE TABLE）执行过程中，
//   通过抽象出的管理器接口在 PD 上创建或删除亲和组。
//

use std::collections::{BTreeMap, BTreeSet};

/// 亲和级别：决定亲和组的粒度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffinityLevel {
    /// 表级：整张表共用一个亲和组。
    Table,
    /// 分区级：表的每个分区各自对应一个亲和组。
    Partition,
}

/// 参与亲和组计算的表元信息（对应 Go 侧 `model.TableInfo` 的简化视图）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffinityTable {
    /// 表的唯一 ID（TiDB 内部分配的 int64 表标识）。
    pub id: i64,
    /// 表名，仅用于错误信息与日志。
    pub name: String,
    /// 亲和级别；`None` 表示该表未配置亲和性，所有操作直接跳过。
    pub affinity: Option<AffinityLevel>,
    /// 表自身携带的分区物理 ID 列表（分区级亲和时的默认来源）。
    pub partition_ids: Vec<i64>,
}

/// 亲和组覆盖的 key 范围（左闭右开区间），对应 PD HTTP API 中的
/// `AffinityGroupKeyRange`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffinityGroupKeyRange {
    /// 起始 key（包含）。
    pub start_key: Vec<u8>,
    /// 结束 key（不包含）。
    pub end_key: Vec<u8>,
}

/// 亲和组操作可能产生的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AffinityError {
    /// 分区级亲和但找不到任何分区定义，通常意味着表元数据损坏。
    MissingPartitions { table_id: i64 },
    /// 非法的亲和级别（保留给未来接线时的校验路径）。
    InvalidLevel,
    /// 后端（PD）调用失败，携带底层错误描述。
    Backend(String),
}

impl std::fmt::Display for AffinityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AffinityError {}

/// key 编码器抽象，对应 Go 侧 `tikv.Codec`。
///
/// 多租户（keyspace）部署下，逻辑 key 需要再包一层租户前缀才能定位到
/// TiKV 中的实际 Region 范围；该 trait 抽象了这一步编码。
pub trait AffinityCodec {
    /// 把逻辑上的 [start, end) 区间编码为实际的 Region key 区间。
    fn encode_region_range(&self, start: Vec<u8>, end: Vec<u8>) -> (Vec<u8>, Vec<u8>);
}

/// 亲和组管理器抽象，封装对 PD 的创建/删除调用（对应 Go 侧
/// `domain/affinity` 包的边界）。
pub trait AffinityGroupManager {
    /// 幂等地创建一批亲和组：key 为组 ID，value 为该组覆盖的 key 范围列表。
    fn create_groups_if_not_exists(
        &mut self,
        groups: &BTreeMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), String>;
    /// 带重试地删除一批亲和组（删除属于尽力而为的清理操作）。
    fn delete_groups_with_retry(&mut self, group_ids: &[String]) -> Result<(), String>;
}

/// 返回表级亲和组 ID，格式为 `_tidb_t_{tableID}`。
pub fn get_table_affinity_group_id(table_id: i64) -> String {
    format!("_tidb_t_{table_id}")
}

/// 返回分区级亲和组 ID，格式为 `_tidb_pt_{tableID}_p{partitionID}`。
pub fn get_partition_affinity_group_id(table_id: i64, partition_id: i64) -> String {
    format!("_tidb_pt_{table_id}_p{partition_id}")
}

/// 对齐 Go `tablecodec.EncodeTablePrefix` 的 `t` + `codec.EncodeInt(id)` 布局。
fn encode_table_prefix(table_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(9);
    key.push(b't');
    key.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    key
}

/// 根据物理 ID（表 ID 或分区 ID）构造该对象数据所在的 key 范围。
///
/// TiDB 的表数据统一以 `t{physicalID}` 为前缀编码存储，因此
/// `[t{id}, t{id+1})` 即覆盖该表/分区的全部行数据。
pub fn build_affinity_group_key_range(
    codec: Option<&dyn AffinityCodec>,
    physical_id: i64,
) -> AffinityGroupKeyRange {
    let mut start = encode_table_prefix(physical_id);
    // Go 的 int64 加法按二进制补码回绕；`wrapping_add` 保留同一边界语义。
    let mut end = encode_table_prefix(physical_id.wrapping_add(1));
    // 提供了 codec 时（多租户场景），再包一层 Region 范围编码。
    if let Some(codec) = codec {
        (start, end) = codec.encode_region_range(start, end);
    }
    AffinityGroupKeyRange {
        start_key: start,
        end_key: end,
    }
}

/// 根据表的亲和配置构造「组 ID -> key 范围列表」的完整定义。
///
/// - 表级亲和：生成单个组 `_tidb_t_{tableID}`，覆盖整表范围；
/// - 分区级亲和：每个分区一个组 `_tidb_pt_{tableID}_p{partitionID}`。
///
/// `partition_ids` 允许调用方显式指定分区列表（例如 TRUNCATE PARTITION
/// 只涉及部分分区）；传 `None` 则回退到表元信息中的分区列表。
/// 表不存在或未配置亲和性时返回空 map，表示无事可做。
pub fn build_affinity_group_definitions(
    codec: Option<&dyn AffinityCodec>,
    table: Option<&AffinityTable>,
    partition_ids: Option<&[i64]>,
) -> Result<BTreeMap<String, Vec<AffinityGroupKeyRange>>, AffinityError> {
    // 无表或未配置亲和性：直接返回空定义（对应 Go 的 nil, nil）。
    let Some(table) = table else {
        return Ok(BTreeMap::new());
    };
    let Some(level) = table.affinity else {
        return Ok(BTreeMap::new());
    };
    let mut groups = BTreeMap::new();
    match level {
        AffinityLevel::Table => {
            // 表级：整表一个组，key 范围按表 ID 计算。
            groups.insert(
                get_table_affinity_group_id(table.id),
                vec![build_affinity_group_key_range(codec, table.id)],
            );
        }
        AffinityLevel::Partition => {
            // 分区级：优先使用调用方显式传入的分区列表。
            let partitions = partition_ids.unwrap_or(&table.partition_ids);
            // 分区级亲和却拿不到分区定义，说明表元数据可能已损坏。
            if partitions.is_empty() {
                return Err(AffinityError::MissingPartitions { table_id: table.id });
            }
            for partition_id in partitions {
                groups.insert(
                    get_partition_affinity_group_id(table.id, *partition_id),
                    vec![build_affinity_group_key_range(codec, *partition_id)],
                );
            }
        }
    }
    Ok(groups)
}

/// 收集定义 map 中的全部组 ID（BTreeMap 保证结果有序、确定）。
pub fn collect_affinity_group_ids(
    groups: &BTreeMap<String, Vec<AffinityGroupKeyRange>>,
) -> Vec<String> {
    groups.keys().cloned().collect()
}

/// 在 PD 上为一张表创建亲和组。
///
/// 这是关键路径操作：创建失败时应让 DDL 失败。
/// 使用场景：CREATE TABLE、ALTER TABLE AFFINITY = 'xxx'、
/// TRUNCATE TABLE、TRUNCATE PARTITION。
pub fn create_table_affinity_groups(
    manager: &mut dyn AffinityGroupManager,
    codec: Option<&dyn AffinityCodec>,
    table: Option<&AffinityTable>,
) -> Result<(), AffinityError> {
    let groups = build_affinity_group_definitions(codec, table, None)?;
    // 未配置亲和性的表会得到空定义，直接跳过 PD 调用。
    if groups.is_empty() {
        return Ok(());
    }
    manager
        .create_groups_if_not_exists(&groups)
        .map_err(AffinityError::Backend)
}

/// 在 PD 上删除一张表的亲和组。
///
/// 属于尽力而为的清理操作：Go 侧失败仅记录日志不阻塞流程。
/// 使用场景：DROP TABLE、ALTER TABLE AFFINITY = ''、
/// TRUNCATE TABLE、TRUNCATE PARTITION。
/// `partition_ids` 可指定仅清理部分分区对应的组。
pub fn delete_table_affinity_groups(
    manager: &mut dyn AffinityGroupManager,
    codec: Option<&dyn AffinityCodec>,
    table: Option<&AffinityTable>,
    partition_ids: Option<&[i64]>,
) -> Result<(), AffinityError> {
    let groups = build_affinity_group_definitions(codec, table, partition_ids)?;
    // 没有需要删除的组时直接返回，避免无谓的 PD 请求。
    if groups.is_empty() {
        return Ok(());
    }
    manager
        .delete_groups_with_retry(&collect_affinity_group_ids(&groups))
        .map_err(AffinityError::Backend)
}

/// 批量删除多张表的亲和组，用于 DROP DATABASE 一次性清理库内所有表。
///
/// 返回错误交由调用方决定是继续还是失败。
pub fn batch_delete_table_affinity_groups(
    manager: &mut dyn AffinityGroupManager,
    codec: Option<&dyn AffinityCodec>,
    tables: &[AffinityTable],
) -> Result<(), AffinityError> {
    // 用 BTreeSet 汇总所有表的组 ID，天然去重且保持有序。
    let mut ids = BTreeSet::new();
    for table in tables {
        ids.extend(build_affinity_group_definitions(codec, Some(table), None)?.into_keys());
    }
    if ids.is_empty() {
        return Ok(());
    }
    manager
        .delete_groups_with_retry(&ids.into_iter().collect::<Vec<_>>())
        .map_err(AffinityError::Backend)
}
