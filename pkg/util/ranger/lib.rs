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

// ranger crate 根：从谓词条件构造索引/表扫描 Range。
//
// Range 是优化器把 WHERE/索引条件转成可下推键区间的核心结构；本 crate 对齐 TiDB
// `pkg/util/ranger`：端点构造（points）、条件拆分（detacher）、区间合并（ranger）。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 表达式 AST 节点名与函数名常量。
pub mod ast {
    pub use expression::ast::*;
}
/// 字符集与校对规则名称。
pub mod charset {
    pub use expression::charset::*;
}
/// Chunk 行批，供表达式求值路径复用。
pub mod chunk {
    pub use expression::chunk::*;
}
/// Datum/键编码，用于把端点编成可比较的 KV key。
pub mod codec {
    pub use expression::codec::*;
}
/// 校对器，决定字符串端点比较顺序。
pub mod collate {
    pub use expression::collate::*;
}
/// 错误级别上下文。
pub mod errctx {
    pub use expression::errctx::*;
}
/// 错误类型与 Trace 包装。
pub mod errors {
    pub use expression::errors::*;
}
/// MySQL 类型标志与类型码。
pub mod mysql {
    pub use expression::mysql::*;
}
/// 底层字节切片等 hack 工具。
pub mod hack {
    pub use ::hack::*;
}
/// Datum、FieldType 与类型错误码。
pub mod types {
    pub use expression::types::*;
    pub use types_dependency::field::NewFieldTypeWithCollation;
    pub use types_dependency::{
        ErrDataTooLong, ErrOverflow, ErrTruncated, ErrWarnDataOutOfRange, ErrWrongValue,
    };
}
/// SQL 格式化辅助。
pub mod format {
    pub use parser_format::*;
}
/// 字节序比较，对齐 Go `bytes.Compare` 返回 -1/0/1。
pub mod bytes {
    /// 按字典序比较两段字节，返回 -1、0 或 1。
    pub fn Compare(left: &[u8], right: &[u8]) -> i32 {
        match left.cmp(right) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }
}

#[path = "types.rs"]
mod types_impl;
/// 导出 Range/Ranges 等核心类型。
pub use types_impl::*;

#[path = "points.rs"]
mod points_impl;
/// 导出端点构造与全范围工厂函数。
pub use points_impl::*;

#[path = "checker.rs"]
mod checker_impl;

#[path = "detacher.rs"]
mod detacher_impl;
/// 导出条件拆分与索引 Range 构造入口。
pub use detacher_impl::*;

#[path = "ranger.rs"]
mod ranger_impl;
/// 导出表/列 Range 构建与区间并集等工具。
pub use ranger_impl::*;

#[cfg(test)]
#[path = "test_support_aster_unit_test.rs"]
mod test_support_aster_unit_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "points_test.rs"]
mod points_test;
#[cfg(test)]
#[path = "ranger_test.rs"]
mod ranger_test;
#[cfg(test)]
#[path = "types_test.rs"]
mod types_test;
