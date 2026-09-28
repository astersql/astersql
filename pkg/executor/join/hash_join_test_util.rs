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

// Hash Join 单元/集成测试辅助工具。
//
// 构造 `HashJoinInfo`、mock 左右数据源、执行 v1 执行器并校验排序后的结果行。
// 对应 Go `hash_join_test_util.go`。

use crate::hash_join_base::HashJoinContextBase;
use crate::hash_join_v1::{HashJoinCtxV1, HashJoinV1Exec};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::{Chunk, Value};
use std::cmp::Ordering;

/// 测试用 Hash Join 参数包：连接类型、键列、两侧 chunk 与其它开关。
#[derive(Clone)]
pub struct HashJoinInfo {
    /// 连接类型（INNER/LEFT/RIGHT/SEMI 等）。
    pub join_type: JoinType,
    /// 构建侧连接键列下标。
    pub build_key_indices: Vec<usize>,
    /// 探测侧连接键列下标。
    pub probe_key_indices: Vec<usize>,
    /// 构建侧输入 chunk 列表。
    pub build_chunks: Vec<Chunk>,
    /// 探测侧输入 chunk 列表。
    pub probe_chunks: Vec<Chunk>,
    /// 外连接时右侧是否为 outer。
    pub outer_is_right: bool,
    /// 构建侧是否为 outer（影响空行填充）。
    pub build_side_is_outer: bool,
    /// 是否 null-aware anti join（NAAJ：对 NULL 键有特殊语义）。
    pub null_aware: bool,
    /// Worker 并发度。
    pub concurrency: usize,
    /// 输出 chunk 最大行数。
    pub max_chunk_size: usize,
    /// Outer join 无匹配时的默认内表行。
    pub default_inner: Row,
    /// 连接后过滤谓词。
    pub conditions: Vec<Predicate>,
    /// 两侧实际使用的列投影；`None` 表示保留全部列。
    pub children_used: Option<[Vec<usize>; 2]>,
}

impl HashJoinInfo {
    /// 校验键列数量一致且并发/chunk 大小为正。
    pub fn validate(&self) -> Result<(), String> {
        if self.build_key_indices.len() != self.probe_key_indices.len() {
            return Err("join key counts differ".into());
        }
        if self.concurrency == 0 || self.max_chunk_size == 0 {
            return Err("concurrency and chunk size must be positive".into());
        }
        Ok(())
    }
}

/// 根据 `HashJoinInfo` 构造 v1 Hash Join 执行器。
pub fn build_hash_join_v1_exec(info: &HashJoinInfo) -> Result<HashJoinV1Exec, String> {
    info.validate()?;
    let context = HashJoinCtxV1 {
        base: HashJoinContextBase::default(),
        join_type: info.join_type,
        build_key_indices: info.build_key_indices.clone(),
        probe_key_indices: info.probe_key_indices.clone(),
        null_aware: info.null_aware,
        build_side_is_outer: info.build_side_is_outer,
        concurrency: info.concurrency,
        max_chunk_size: info.max_chunk_size,
    };
    let joiner = Joiner::new(
        info.join_type,
        info.outer_is_right,
        info.default_inner.clone(),
        info.conditions.clone(),
        info.children_used.clone(),
        info.null_aware,
        info.max_chunk_size,
    )?;
    HashJoinV1Exec::new(
        context,
        joiner,
        info.build_chunks.clone(),
        info.probe_chunks.clone(),
    )
}

/// 生成按列逐个比较的行比较器（用于结果排序）。
pub fn generate_cmp_func() -> impl Fn(&Row, &Row) -> Ordering {
    |left, right| {
        left.iter()
            .zip(right)
            .map(|(a, b)| compare_value(a, b))
            .find(|order| *order != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len()))
    }
}

/// 展平 chunk 后按统一比较器排序，便于与期望结果比对。
pub fn sort_rows(chunks: Vec<Chunk>) -> Vec<Row> {
    let mut rows: Vec<Row> = chunks.into_iter().flatten().collect();
    rows.sort_by(generate_cmp_func());
    rows
}

/// 构造整型连接键测试数据。
pub fn build_join_key_int_datums(count: usize) -> Vec<Value> {
    (0..count).map(|value| Value::Int(value as i64)).collect()
}
/// 构造字符串连接键测试数据。
pub fn build_join_key_string_datums(count: usize) -> Vec<Value> {
    (0..count)
        .map(|value| Value::Text(value.to_string()))
        .collect()
}

/// 构造左右两侧 mock 数据源；右侧默认逆序以打乱匹配顺序。
pub fn build_left_and_right_data_source(
    left_rows: usize,
    right_rows: usize,
    chunk_size: usize,
) -> (Vec<Chunk>, Vec<Chunk>) {
    (
        mock_data_source(left_rows, chunk_size, false),
        mock_data_source(right_rows, chunk_size, true),
    )
}

/// 生成 `(int, text)` 两列的 mock 行，并按 `chunk_size` 切块。
pub fn mock_data_source(row_count: usize, chunk_size: usize, reverse: bool) -> Vec<Chunk> {
    assert!(chunk_size > 0, "chunk size must be positive");
    let mut rows: Vec<Row> = (0..row_count)
        .map(|index| {
            vec![
                Value::Int(index as i64),
                Value::Text(format!("row-{index}")),
            ]
        })
        .collect();
    if reverse {
        rows.reverse();
    }
    rows.chunks(chunk_size).map(<[Row]>::to_vec).collect()
}

/// 构造 `0..column_count` 的列下标 schema。
pub fn build_schema(column_count: usize) -> Vec<usize> {
    (0..column_count).collect()
}

/// 打开并拉取 Hash Join 执行器全部输出 chunk。
pub fn execute_hash_join_exec(executor: &mut HashJoinV1Exec) -> Result<Vec<Chunk>, String> {
    executor.open()?;
    let mut chunks = Vec::new();
    let execution_result = loop {
        match executor.next() {
            Ok(result) => {
                if let Some(error) = result.error {
                    break Err(error);
                }
                if result.rows.is_empty() {
                    break Ok(chunks);
                }
                chunks.push(result.rows);
            }
            Err(error) => break Err(error),
        }
    };
    executor.close();
    execution_result
}

/// 执行并只关心错误（成功返回 `None`）。
pub fn execute_hash_join_exec_and_get_error(executor: &mut HashJoinV1Exec) -> Option<String> {
    execute_hash_join_exec(executor).err()
}

/// 随机失败测试：在产出若干 chunk 后主动 cancel，验证取消路径。
pub fn execute_hash_join_exec_for_random_fail_test(
    executor: &mut HashJoinV1Exec,
    fail_after_chunks: Option<usize>,
) -> Result<(), String> {
    executor.open()?;
    let mut chunks = 0;
    let execution_result = loop {
        // 达到阈值后取消，模拟执行中途失败。
        if fail_after_chunks.is_some_and(|limit| chunks >= limit) {
            executor.context.base.cancel();
        }
        match executor.next() {
            Ok(result) if result.rows.is_empty() => break Ok(()),
            Ok(_) => chunks += 1,
            Err(error) => break Err(error),
        }
    };
    executor.close();
    execution_result
}

/// 执行后返回排序过的结果行。
pub fn get_sorted_results(executor: &mut HashJoinV1Exec) -> Result<Vec<Row>, String> {
    Ok(sort_rows(execute_hash_join_exec(executor)?))
}

/// 逐行比较实际与期望结果；长度或内容不符则返回描述性错误。
pub fn check_results(actual: &[Row], expected: &[Row]) -> Result<(), String> {
    if actual.len() != expected.len() {
        return Err(format!(
            "result length differs: actual {}, expected {}",
            actual.len(),
            expected.len()
        ));
    }
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if compare_rows(actual, expected) != Ordering::Equal {
            return Err(format!(
                "result index {index} differs: actual {actual:?}, expected {expected:?}"
            ));
        }
    }
    Ok(())
}

/// 行比较委托给统一比较器。
fn compare_rows(left: &Row, right: &Row) -> Ordering {
    generate_cmp_func()(left, right)
}

/// 按值类型秩与具体内容比较两个 `Value`。
fn compare_value(left: &Value, right: &Value) -> Ordering {
    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::UInt(_) => 3,
        Value::Float(_) => 4,
        Value::Bytes(_) => 5,
        Value::Text(_) => 6,
    };
    let ranks = rank(left).cmp(&rank(right));
    if ranks != Ordering::Equal {
        return ranks;
    }
    match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::UInt(a), Value::UInt(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}
