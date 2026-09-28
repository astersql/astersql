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

// 优化器 tracing：基数估计（CE）记录与去重。
//
// 由 `opt_trace.go` 迁移。`CETraceRecord` 保存表达式及估算出的行数；
// `DedupCETrace` 按记录值去重；`OptimizeTracer` 为优化器 tracer 占位类型。

// 优化器 tracing 中用于记录基数估计结果的轻量数据结构，以及按记录值去重的辅助函数。
// 这里仅使用 Rust 标准库 HashSet 来对应 Go 里的 map[CETraceRecord]struct{} 去重表。

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 一条基数估计（Cardinality Estimation）的 trace 记录。
// CETraceRecord records an expression and related cardinality estimation result.
// CETraceRecord 对应 Go 里的同名结构体，用来记录表达式及其相关的基数估计结果。
// 字段顺序、导出性和 Go 结构体标签含义保持原样；JSON 标签以中文注释保留，避免虚构 serde 依赖。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CETraceRecord {
    // 对应 Go 字段 TableName string `json:"table_name"`，记录估计结果所属表名。
    #[serde(rename = "table_name")]
    pub TableName: String,
    // 对应 Go 字段 Type string `json:"type"`，记录 trace 项类型。
    #[serde(rename = "type")]
    pub Type: String,
    // 对应 Go 字段 Expr string `json:"expr"`，记录被估算的表达式文本。
    #[serde(rename = "expr")]
    pub Expr: String,
    // 对应 Go 字段 TableID int64 `json:"-"`，原 Go JSON 序列化会忽略该内部表 ID。
    #[serde(skip)]
    pub TableID: i64,
    // 对应 Go 字段 RowCount uint64 `json:"row_count"`，记录估算出的行数。
    #[serde(rename = "row_count")]
    pub RowCount: u64,
}

/// 按记录字段值对 CETrace 切片去重，保留首次出现顺序。
// DedupCETrace deduplicate a slice of *CETraceRecord and return the deduplicated slice
// DedupCETrace 对应 Go 函数：按 CETraceRecord 的字段值对 []*CETraceRecord 去重，并返回保留首次出现记录的切片。
// Go 代码通过解引用 *rec 作为 map key，因此 nil 指针会在原实现中 panic；用 Box 表达非空指针元素。
#[allow(non_snake_case)]
pub fn DedupCETrace(records: Vec<Box<CETraceRecord>>) -> Vec<Box<CETraceRecord>> {
    // 对应 make([]*CETraceRecord, 0, len(records))，预分配返回切片容量但不改变顺序。
    let mut ret: Vec<Box<CETraceRecord>> = Vec::with_capacity(records.len());
    // 对应 make(map[CETraceRecord]struct{}, len(records))；HashSet 的 key 是记录值本身，不是指针地址。
    let mut exists: HashSet<CETraceRecord> = HashSet::with_capacity(records.len());

    // 对应 Go 的 for _, rec := range records；这里消费输入 Vec，以便把首次出现的 Box 原样移动到返回值。
    for rec in records {
        // Go 的 exists[*rec] 会按结构体所有字段比较；Rust 需要 clone 一份值作为 HashSet key。
        let key = (*rec).clone();
        // 关键分支保持 Go 语义：只有首次出现的记录值才进入 ret，后续重复值会被跳过。
        if !exists.contains(&key) {
            ret.push(rec);
            exists.insert(key);
        }
    }

    ret
}

/// 优化器 tracer 占位类型（对应 Go 空结构体）。
// OptimizeTracer indicates tracer for optimizer
// OptimizeTracer 对应 Go 中的空结构体，表示优化器 tracer 的占位类型。
// 原文件没有给它定义字段或方法；保留空结构，不添加跨文件行为。
#[derive(Clone, Copy, Debug, Default)]
pub struct OptimizeTracer {}
