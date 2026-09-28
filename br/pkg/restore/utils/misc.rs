// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 恢复工具杂项：分区/表/索引 ID 映射与键前缀辅助，对齐 Go `misc.go`。
//! ID 映射按名称匹配旧→新物理 ID，供 rewrite_rule 生成键前缀替换规则；
//! TruncateTS / EncodeKeyPrefix 处理 TiKV 键上的时间戳与 memcomparable 分组。

//! Partition/index ID mapping and key helpers matching `misc.go`.

use std::collections::HashMap;

use crate::stubs::{codec, model};

/// TiKV write CF 名；合并/过滤时按 CF 或文件名包含该串识别。
pub const WriteCFName: &str = "write";
/// TiKV default CF 名；RawKV 备份通常只有 default CF。
pub const DefaultCFName: &str = "default";

/// 按分区名（CIStr.L）对齐旧/新 Partition.Definitions，生成 oldID→newID。
/// 任一侧无分区信息时返回空 map，不 panic。
/// GetPartitionIDMap creates a map maping old physical ID to new physical ID.
pub fn GetPartitionIDMap(
    newTable: &model::TableInfo,
    oldTable: &model::TableInfo,
) -> HashMap<i64, i64> {
    let mut tableIDMap: HashMap<i64, i64> = HashMap::new();

    if let (Some(old_part), Some(new_part)) = (&oldTable.Partition, &newTable.Partition) {
        // 先建旧分区名→ID，再按新分区名回填映射；同名才建立对应关系。
        let mut nameMapID: HashMap<String, i64> = HashMap::new();
        for old in &old_part.Definitions {
            nameMapID.insert(old.Name.L.clone(), old.ID);
        }
        for new in &new_part.Definitions {
            if let Some(oldID) = nameMapID.get(&new.Name.L) {
                tableIDMap.insert(*oldID, new.ID);
            }
        }
    }

    tableIDMap
}

/// 在分区映射基础上再写入整表 oldTable.ID→newTable.ID。
/// GetTableIDMap creates a map maping old tableID to new tableID.
pub fn GetTableIDMap(
    newTable: &model::TableInfo,
    oldTable: &model::TableInfo,
) -> HashMap<i64, i64> {
    let mut tableIDMap = GetPartitionIDMap(newTable, oldTable);
    tableIDMap.insert(oldTable.ID, newTable.ID);
    tableIDMap
}

/// 按索引名匹配，生成 oldIndexID→newIndexID；名称不同的索引不入 map。
/// GetIndexIDMap creates a map maping old indexID to new indexID.
pub fn GetIndexIDMap(
    newTable: &model::TableInfo,
    oldTable: &model::TableInfo,
) -> HashMap<i64, i64> {
    let mut indexIDMap: HashMap<i64, i64> = HashMap::new();
    for srcIndex in &oldTable.Indices {
        for destIndex in &newTable.Indices {
            if srcIndex.Name == destIndex.Name {
                indexIDMap.insert(srcIndex.ID, destIndex.ID);
            }
        }
    }
    indexIDMap
}

/// 去掉 TiKV 键末尾 8 字节时间戳；空键返回 None，长度不足 8 则原样返回。
pub fn TruncateTS(key: &[u8]) -> Option<Vec<u8>> {
    if key.is_empty() {
        return None;
    }
    if key.len() < 8 {
        return Some(key.to_vec());
    }
    Some(key[..key.len() - 8].to_vec())
}

/// 对完整 8 字节组做 EncodeBytes（每组后插 0xff），末尾不足 8 字节的尾巴原样拼接。
/// 用于构造可比较的 key prefix，与 Go `EncodeKeyPrefix` 一致。
pub fn EncodeKeyPrefix(key: &[u8]) -> Vec<u8> {
    let ungrouped_len = key.len() % 8;
    let mut encoded_prefix = codec::EncodeBytes(Vec::new(), &key[..key.len() - ungrouped_len]);
    // EncodeBytes 末尾带标记字节；去掉最后 9 字节（8 数据+1 标记）后接 raw 尾巴。
    let keep = encoded_prefix.len().saturating_sub(9);
    let mut out = encoded_prefix[..keep].to_vec();
    out.extend_from_slice(&key[key.len() - ungrouped_len..]);
    out
}
