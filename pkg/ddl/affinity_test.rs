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

// 亲和性（Affinity）功能的单元测试。
//
// 亲和性用于把表或分区的数据（按 key 范围划分的 Region，Region 是
// 分布式存储层中一段连续 key 空间的数据分片）绑定到同一批亲和组
// （Affinity Group），由 PD（Placement Driver，集群调度中枢）负责把
// 组内 Region 调度到相近的存储节点，从而提升局部性。
//
// 本文件通过内存实现的编解码器与组管理器，验证以下逻辑：
// - 按表级 / 分区级亲和性构建亲和组定义（组 ID 与 key 范围）；
// - 创建 / 删除亲和组的幂等性与部分删除；
// - 删库时对多张表的亲和组做批量删除。

use std::collections::BTreeMap;

use crate::affinity::{
    AffinityCodec, AffinityError, AffinityGroupKeyRange, AffinityGroupManager, AffinityLevel,
    AffinityTable, batch_delete_table_affinity_groups, build_affinity_group_definitions,
    create_table_affinity_groups, delete_table_affinity_groups, get_partition_affinity_group_id,
    get_table_affinity_group_id,
};

/// 测试用的简单编解码器：给原始 key 范围统一加上 `k:` 前缀，
/// 模拟真实场景中把表内 key 编码为存储层 Region key 的过程。
struct PrefixCodec;

impl AffinityCodec for PrefixCodec {
    /// 把逻辑上的起止 key 编码为带 `k:` 前缀的 Region key 范围。
    fn encode_region_range(&self, start: Vec<u8>, end: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
        (
            [b"k:".as_slice(), start.as_slice()].concat(),
            [b"k:".as_slice(), end.as_slice()].concat(),
        )
    }
}

/// 内存版亲和组管理器，替代真实的 PD 客户端，用于在测试中
/// 记录亲和组的存储状态以及创建 / 删除接口的调用次数。
#[derive(Default)]
struct MemoryGroups {
    /// 组 ID 到其 key 范围列表的映射，模拟 PD 中保存的亲和组。
    groups: BTreeMap<String, Vec<AffinityGroupKeyRange>>,
    /// 创建接口被调用的次数，用于断言批量行为。
    create_calls: usize,
    /// 删除接口被调用的次数，用于断言批量行为。
    delete_calls: usize,
}

impl AffinityGroupManager for MemoryGroups {
    /// 幂等地创建亲和组：已存在的组保持原有 key 范围不被覆盖。
    fn create_groups_if_not_exists(
        &mut self,
        groups: &BTreeMap<String, Vec<AffinityGroupKeyRange>>,
    ) -> Result<(), String> {
        self.create_calls += 1;
        for (id, ranges) in groups {
            self.groups
                .entry(id.clone())
                .or_insert_with(|| ranges.clone());
        }
        Ok(())
    }

    /// 删除指定 ID 的亲和组（真实实现会带重试，这里直接移除）。
    fn delete_groups_with_retry(&mut self, group_ids: &[String]) -> Result<(), String> {
        self.delete_calls += 1;
        for id in group_ids {
            self.groups.remove(id);
        }
        Ok(())
    }
}

/// 构造测试用的 [`AffinityTable`]：指定表 ID、亲和级别
/// （表级或分区级）以及分区 ID 列表。
fn table(id: i64, affinity: AffinityLevel, partitions: Vec<i64>) -> AffinityTable {
    AffinityTable {
        id,
        name: format!("t{id}"),
        affinity: Some(affinity),
        partition_ids: partitions,
    }
}

/// Go `tablecodec.EncodeTablePrefix` 使用 `codec.EncodeInt`：先翻转符号位，
/// 再以大端序写入，从而让有符号整数的字节序保持可比较。
fn table_prefix(id: i64) -> Vec<u8> {
    let mut key = b"t".to_vec();
    key.extend_from_slice(&((id as u64) ^ (1_u64 << 63)).to_be_bytes());
    key
}

/// 表级亲和：整张表只生成一个亲和组，key 范围覆盖全表数据。
#[test]
fn test_affinity_build_group_definitions_table() {
    let table = table(123, AffinityLevel::Table, Vec::new());
    let groups = build_affinity_group_definitions(Some(&PrefixCodec), Some(&table), None).unwrap();
    assert_eq!(1, groups.len());
    let ranges = &groups[&get_table_affinity_group_id(123)];
    assert_eq!(1, ranges.len());
    // 编解码器加了 `k:` 前缀，表数据 key 本身以 `t` 开头。
    assert!(ranges[0].start_key.starts_with(b"k:t"));
    assert!(ranges[0].end_key.starts_with(b"k:t"));
    // key 范围严格复用 Go tablecodec 的 memcomparable 有符号整数编码。
    assert_eq!(
        [b"k:".as_slice(), table_prefix(123).as_slice()].concat(),
        ranges[0].start_key
    );
    assert_eq!(
        [b"k:".as_slice(), table_prefix(124).as_slice()].concat(),
        ranges[0].end_key
    );
}

/// 分区级亲和：每个分区各生成一个亲和组；也可以用显式的分区
/// ID 列表覆盖表元信息中的分区（如 TRUNCATE 后替换新分区 ID）。
#[test]
fn test_affinity_build_group_definitions_partition() {
    let table = table(50, AffinityLevel::Partition, vec![1, 3]);
    let groups = build_affinity_group_definitions(None, Some(&table), None).unwrap();
    assert_eq!(2, groups.len());
    for partition_id in [1_i64, 3] {
        let ranges = &groups[&get_partition_affinity_group_id(50, partition_id)];
        assert_eq!(1, ranges.len());
        // 每个分区组的 key 范围为 [分区 ID, 分区 ID + 1) 的 tablecodec 编码。
        assert_eq!(table_prefix(partition_id), ranges[0].start_key);
        assert_eq!(table_prefix(partition_id + 1), ranges[0].end_key);
    }

    // 传入替换分区列表时，按新分区 ID（7、9）生成组定义。
    let replacement = build_affinity_group_definitions(None, Some(&table), Some(&[7, 9])).unwrap();
    assert!(replacement.contains_key(&get_partition_affinity_group_id(50, 7)));
    assert!(replacement.contains_key(&get_partition_affinity_group_id(50, 9)));
}

/// Go 的 int64 加法按二进制补码回绕；最大物理 ID 的结束边界因此编码
/// `i64::MIN`，不能饱和停留在 `i64::MAX`。
#[test]
fn test_affinity_key_range_max_physical_id_wraps_like_go() {
    let table = table(i64::MAX, AffinityLevel::Table, Vec::new());
    let groups = build_affinity_group_definitions(None, Some(&table), None).unwrap();
    let range = &groups[&get_table_affinity_group_id(i64::MAX)][0];
    assert_eq!(table_prefix(i64::MAX), range.start_key);
    assert_eq!(table_prefix(i64::MIN), range.end_key);
}

/// 异常路径：分区级亲和但缺少分区信息时应报错；
/// 完全没有表信息时应返回空的组定义集合。
#[test]
fn test_affinity_build_group_definitions_partition_missing() {
    let table = table(1, AffinityLevel::Partition, Vec::new());
    assert_eq!(
        Err(AffinityError::MissingPartitions { table_id: 1 }),
        build_affinity_group_definitions(None, Some(&table), None)
    );
    // 无表信息时不生成任何亲和组。
    assert!(
        build_affinity_group_definitions(None, None, None)
            .unwrap()
            .is_empty()
    );
}

/// 模拟与 PD 的交互流程：创建的幂等性、整表删除、
/// 按指定分区删除，以及 TRUNCATE 分区后为新分区建组。
#[test]
fn test_affinity_pd_interaction() {
    let mut manager = MemoryGroups::default();
    let table_level = table(10, AffinityLevel::Table, Vec::new());
    // 重复创建同一张表的亲和组，结果应保持幂等。
    create_table_affinity_groups(&mut manager, None, Some(&table_level)).unwrap();
    create_table_affinity_groups(&mut manager, None, Some(&table_level)).unwrap();
    assert_eq!(1, manager.groups.len(), "create is idempotent");

    delete_table_affinity_groups(&mut manager, None, Some(&table_level), None).unwrap();
    assert!(manager.groups.is_empty());

    let old = table(20, AffinityLevel::Partition, vec![21, 22]);
    create_table_affinity_groups(&mut manager, None, Some(&old)).unwrap();
    assert_eq!(2, manager.groups.len());
    // 只删除分区 21 对应的组，分区 22 的组应保留。
    delete_table_affinity_groups(&mut manager, None, Some(&old), Some(&[21])).unwrap();
    assert!(
        !manager
            .groups
            .contains_key(&get_partition_affinity_group_id(20, 21))
    );
    assert!(
        manager
            .groups
            .contains_key(&get_partition_affinity_group_id(20, 22))
    );

    // 模拟 TRUNCATE 分区：分区 21 被替换为新分区 23，需要为其新建组。
    let truncated = table(20, AffinityLevel::Partition, vec![23, 22]);
    create_table_affinity_groups(&mut manager, None, Some(&truncated)).unwrap();
    assert!(
        manager
            .groups
            .contains_key(&get_partition_affinity_group_id(20, 23))
    );
}

/// 删库场景：多张表（含分区表）的亲和组应通过一次批量
/// 删除调用全部清理，避免逐表向 PD 发送删除请求。
#[test]
fn test_affinity_drop_database() {
    let mut manager = MemoryGroups::default();
    let tables = vec![
        table(1, AffinityLevel::Table, Vec::new()),
        table(2, AffinityLevel::Table, Vec::new()),
        table(3, AffinityLevel::Partition, vec![31, 32]),
    ];
    for table in &tables {
        create_table_affinity_groups(&mut manager, None, Some(table)).unwrap();
    }
    // 两张表级组 + 表 3 的两个分区组，共 4 个亲和组。
    assert_eq!(4, manager.groups.len());
    batch_delete_table_affinity_groups(&mut manager, None, &tables).unwrap();
    assert!(manager.groups.is_empty());
    // 批量删除只应触发一次 PD 删除调用。
    assert_eq!(
        1, manager.delete_calls,
        "drop database batches the PD delete"
    );
}
