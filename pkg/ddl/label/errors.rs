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

// Region Label（区域标签）属性解析相关的错误定义。
//
// Label 是挂在 PD（Placement Driver，集群调度中心）Label Rule 上的
// `key=value` 键值对，用于描述表/分区在 TiKV 上的调度属性
//（如 merge_option、db、table、partition、keyspace 等）。
// 本模块定义解析与合并这些属性时可能出现的错误类型与错误文案常量。

#![allow(non_snake_case, dead_code)]

// ErrInvalidAttributesFormat is from attributes.go.
/// 属性格式非法时的错误文案：要求严格为 `key=value` 形式。
pub const ErrInvalidAttributesFormat: &str = "attributes should be in format 'key=value'";

/// Label 属性处理过程中的错误枚举。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    /// 单条属性字符串不符合 `key=value` 格式。
    #[error("attributes should be in format 'key=value': {attribute}")]
    InvalidAttributesFormat { attribute: String },

    /// 同一 key 出现冲突的 value（例如已有 `merge_option=allow` 又加入 `merge_option=deny`）。
    #[error("'{new_label}' and '{existing_label}' are conflicted")]
    ConflictingAttributes {
        new_label: String,
        existing_label: String,
    },

    /// AttributesSpec（属性规格，来自 AST）整体解析失败时的错误，携带底层详情。
    #[error("{0}")]
    InvalidAttributesSpec(String),
}
