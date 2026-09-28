// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Region Label（区域标签）属性解析、兼容性检查与还原。
//
// Placement / Label 规则以 `key=value` 字符串描述，对应 PD（Placement Driver）
// 的 RegionLabel。本模块负责解析、去重/冲突检测，以及在还原时过滤内部注入的
// db/table/partition（及 NextGen 下的 keyspace）标签。

#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables)]

use crate::{errors::Error, kerneltype, pd};

/// 内部注入的 keyspace 标签键名。
pub(crate) const keyspaceKey: &str = "keyspace";
/// 内部注入的库名标签键。
pub(crate) const dbKey: &str = "db";
/// 内部注入的表名标签键。
pub(crate) const tableKey: &str = "table";
/// 内部注入的分区名标签键。
pub(crate) const partitionKey: &str = "partition";

// AttributesCompatibility is the return type of CompatibleWith.
// Go 中底层类型是 byte；这里用枚举保留三态语义。
/// 两个 RegionLabel 的兼容性比较结果。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttributesCompatibility {
    // AttributesCompatible indicates two attributes are compatible.
    /// 键不同，彼此兼容。
    AttributesCompatible,
    // AttributesIncompatible indicates two attributes are incompatible.
    /// 同键不同值，冲突。
    AttributesIncompatible,
    // AttributesDuplicated indicates two attributes are duplicated.
    /// 同键同值，视为重复。
    AttributesDuplicated,
}

// NewLabel creates a new label for a given string.
// Go 使用 strings.Split(attr, "=") 要求恰好两个片段；包含多个等号也会被判为格式错误。
/// 解析单个 `key=value` 属性字符串为 RegionLabel（两端空白会被裁剪）。
pub fn NewLabel(attr: &str) -> Result<pd::RegionLabel, Error> {
    let mut l = pd::RegionLabel::default();
    let kv: Vec<&str> = attr.split('=').collect();
    if kv.len() != 2 {
        return Err(Error::InvalidAttributesFormat {
            attribute: attr.to_owned(),
        });
    }

    let key = kv[0].trim().to_string();
    if key.is_empty() {
        return Err(Error::InvalidAttributesFormat {
            attribute: attr.to_owned(),
        });
    }

    let val = kv[1].trim().to_string();
    if val.is_empty() {
        return Err(Error::InvalidAttributesFormat {
            attribute: attr.to_owned(),
        });
    }

    l.Key = key;
    l.Value = val;
    Ok(l)
}

// RestoreRegionLabel converts a Attribute to a string.
// 这里只恢复 key=value 文本，不做转义；与 Go 字符串拼接保持一致。
/// 将 RegionLabel 还原为 `key=value` 文本。
pub fn RestoreRegionLabel(l: &pd::RegionLabel) -> String {
    format!("{}={}", l.Key, l.Value)
}

// CompatibleWith will check if two constraints are compatible.
// Return (compatible, duplicated).
/// 比较两个标签：键不同兼容；键值皆同为重复；同键不同值为冲突。
pub fn CompatibleWith(l: &pd::RegionLabel, o: &pd::RegionLabel) -> AttributesCompatibility {
    if l.Key != o.Key {
        return AttributesCompatibility::AttributesCompatible;
    }

    if l.Value == o.Value {
        return AttributesCompatibility::AttributesDuplicated;
    }

    AttributesCompatibility::AttributesIncompatible
}

// NewLabels creates a slice of Label for given attributes.
// 每个字符串先解析成 RegionLabel，再经过 Add 做重复和冲突检查。
/// 批量解析属性字符串列表为 RegionLabel 切片。
pub fn NewLabels(attrs: Vec<String>) -> Result<Vec<pd::RegionLabel>, Error> {
    let mut labels: Vec<pd::RegionLabel> = Vec::with_capacity(attrs.len());
    for attr in attrs {
        let label = NewLabel(&attr)?;
        Add(&mut labels, label)?;
    }
    Ok(labels)
}

// RestoreRegionLabels converts Attributes to a string.
// Go 这里跳过内部自动注入的 db/table/partition 标签；NextGen 下还会跳过 keyspace 标签。
/// 将标签列表还原为逗号分隔的带引号 `key=value` 列表，并过滤内部标签。
pub fn RestoreRegionLabels(labels: &[pd::RegionLabel]) -> String {
    let mut sb = String::new();
    let mut written = 0;
    for label in labels {
        // 跳过内部标签；NextGen 内核类型下额外跳过 keyspace。
        match label.Key.as_str() {
            dbKey | tableKey | partitionKey => continue,
            keyspaceKey => {
                if kerneltype::IsNextGen() {
                    continue;
                }
            }
            _ => {}
        }

        if written > 0 {
            sb.push(',');
        }
        sb.push('"');
        sb.push_str(&RestoreRegionLabel(label));
        sb.push('"');
        written += 1;
    }
    sb
}

// Add will add a new attribute, with validation of all attributes.
// 重复标签按 Go 语义静默成功；同 key 不同 value 则构造冲突错误。
/// 向标签列表追加新标签：重复则忽略，冲突则报错。
pub fn Add(labels: &mut Vec<pd::RegionLabel>, label: pd::RegionLabel) -> Result<(), Error> {
    for l in labels.iter() {
        let res = CompatibleWith(&label, l);
        if res == AttributesCompatibility::AttributesCompatible {
            continue;
        }
        if res == AttributesCompatibility::AttributesDuplicated {
            return Ok(());
        }
        let s1 = RestoreRegionLabel(&label);
        let s2 = RestoreRegionLabel(l);
        return Err(Error::ConflictingAttributes {
            new_label: s1,
            existing_label: s2,
        });
    }

    labels.push(label);
    Ok(())
}
