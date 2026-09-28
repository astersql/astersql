// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 二进制执行计划解码：将 tipb ExplainData 展开为 EXPLAIN 风格文本/列。
//
// 对应 Go `binary_plan_decode.go`。二进制计划经 base64+Snappy 压缩后持久化；
// 本模块解压并递归遍历算子树（含 CTE、子查询），格式化访问对象与运行时统计。
// 执行计划（plan）是优化器产出的算子树；EXPLAIN ANALYZE 可附带实际行数/耗时等。

use crate::codec::{Decompress, Error, PLAN_DISCARDED_DECODED};
use crate::{memory, texttree, types};
use tipb::AccessObject_oneof_access_object;

/// DecodeBinaryPlan decodes a binary plan and displays it like EXPLAIN ANALYZE.
///
/// 解码二进制计划并排成类似 EXPLAIN ANALYZE 的表格文本；过长丢弃则返回哨兵文案。
pub fn DecodeBinaryPlan(binaryPlan: &str) -> Result<String, Error> {
    let proto_bytes = Decompress(binaryPlan)?;
    let data: tipb::ExplainData = protobuf::parse_from_bytes(&proto_bytes)?;
    if data.get_discarded_due_to_too_long() {
        return Ok(PLAN_DISCARDED_DECODED.to_owned());
    }

    // 主树 + CTE + 子查询依次展开为行；再按列宽对齐输出。
    let has_runtime_stats = data.get_with_runtime_stats();
    let mut rows = decodeBinaryOperator(
        data.get_main(),
        "",
        true,
        has_runtime_stats,
        Vec::new(),
        false,
    );
    for cte in data.get_ctes() {
        rows = decodeBinaryOperator(cte, "", true, has_runtime_stats, rows, false);
    }
    for subquery in data.get_subqueries() {
        rows = decodeBinaryOperator(subquery, "", true, has_runtime_stats, rows, false);
    }
    if rows.is_empty() {
        return Ok(String::new());
    }

    let (rune_max_lengths, byte_max_lengths) = calculateMaxFieldLens(&rows, has_runtime_stats);
    let single_row_length = byte_max_lengths
        .iter()
        .map(|length| length + 3)
        .sum::<usize>()
        + 3;
    let total_bytes = single_row_length * (rows.len() + 1) + 1;
    let title_fields = if has_runtime_stats {
        fullTitleFields
    } else {
        noRuntimeStatsTitleFields
    };

    let mut output = String::with_capacity(total_bytes);
    output.push('\n');
    writeRow(&mut output, title_fields.iter().copied(), &rune_max_lengths);
    for row in &rows {
        writeRow(
            &mut output,
            row.iter().map(String::as_str),
            &rune_max_lengths,
        );
    }
    Ok(output)
}

/// 按 rune 宽度左对齐写入一行 `| field | ... |`。
fn writeRow<'a>(
    output: &mut String,
    fields: impl ExactSizeIterator<Item = &'a str>,
    rune_max_lengths: &[usize],
) {
    let field_count = fields.len();
    for (index, field) in fields.enumerate() {
        output.push_str("| ");
        output.push_str(field);
        let rune_length = field.chars().count();
        if rune_length < rune_max_lengths[index] {
            output.push_str(&" ".repeat(rune_max_lengths[index] - rune_length));
        }
        output.push(' ');
        if index + 1 == field_count {
            output.push_str(" |\n");
        }
    }
}

/// DecodeBinaryPlan4Connection decodes a binary plan into selected EXPLAIN columns.
///
/// 按连接侧 EXPLAIN 格式挑选列（brief/row/plan_tree/verbose）；TopSQL 可省略部分运行时列。
pub fn DecodeBinaryPlan4Connection(
    binaryPlan: &str,
    format: &str,
    forTopsql: bool,
) -> Result<Option<Vec<Vec<String>>>, Error> {
    let proto_bytes = Decompress(binaryPlan)?;
    let data: tipb::ExplainData = protobuf::parse_from_bytes(&proto_bytes)?;
    if data.get_discarded_due_to_too_long() {
        return Ok(None);
    }

    let has_runtime_stats = data.get_with_runtime_stats();
    let is_brief = format == types::ExplainFormatBrief;
    let mut rows = decodeBinaryOperator(
        data.get_main(),
        "",
        true,
        has_runtime_stats,
        Vec::new(),
        is_brief,
    );
    for cte in data.get_ctes() {
        rows = decodeBinaryOperator(cte, "", true, has_runtime_stats, rows, is_brief);
    }
    if rows.is_empty() {
        return Ok(None);
    }

    // 按 format / 是否含 runtime / TopSQL 选择列下标。
    let column_indices: &[usize] = if has_runtime_stats && !forTopsql {
        match format {
            types::ExplainFormatBrief | types::ExplainFormatROW => &[0, 1, 3, 4, 5, 6, 7, 8, 9],
            types::ExplainFormatPlanTree => &[0, 2, 3, 4, 5, 6, 7, 8],
            types::ExplainFormatVerbose => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            _ => &[],
        }
    } else {
        match format {
            types::ExplainFormatBrief | types::ExplainFormatROW => &[0, 1, 3, 4, 5],
            types::ExplainFormatPlanTree => &[0, 2, 3, 4, 5],
            types::ExplainFormatVerbose => &[0, 1, 2, 3, 4, 5],
            _ => &[],
        }
    };
    Ok(Some(
        rows.iter()
            .map(|row| {
                column_indices
                    .iter()
                    .map(|&index| row[index].clone())
                    .collect()
            })
            .collect(),
    ))
}

/// 无运行时统计时的表头列。
pub static noRuntimeStatsTitleFields: &[&str] = &[
    "id",
    "estRows",
    "estCost",
    "task",
    "access object",
    "operator info",
];

/// 含 actRows / execution info / memory / disk 的完整表头。
pub static fullTitleFields: &[&str] = &[
    "id",
    "estRows",
    "estCost",
    "actRows",
    "task",
    "access object",
    "execution info",
    "operator info",
    "memory",
    "disk",
];

/// 计算各列最大 rune/字节宽度（含表头），用于对齐与预分配。
fn calculateMaxFieldLens(rows: &[Vec<String>], hasRuntimeStats: bool) -> (Vec<usize>, Vec<usize>) {
    let mut rune_lengths = vec![0; rows[0].len()];
    let mut byte_lengths = vec![0; rows[0].len()];
    for row in rows {
        for (index, field) in row.iter().enumerate() {
            rune_lengths[index] = rune_lengths[index].max(field.chars().count());
            byte_lengths[index] = byte_lengths[index].max(field.len());
        }
    }
    let titles = if hasRuntimeStats {
        fullTitleFields
    } else {
        noRuntimeStatsTitleFields
    };
    for index in 0..byte_lengths.len() {
        rune_lengths[index] = rune_lengths[index].max(titles[index].chars().count());
        byte_lengths[index] = byte_lengths[index].max(titles[index].len());
    }
    (rune_lengths, byte_lengths)
}

/// 递归解码一个 ExplainOperator 及其子节点为表格行。
fn decodeBinaryOperator(
    operator: &tipb::ExplainOperator,
    indent: &str,
    isLastChild: bool,
    hasRuntimeStats: bool,
    mut output: Vec<Vec<String>>,
    isBrief: bool,
) -> Vec<Vec<String>> {
    // brief 模式优先 brief_name / brief_operator_info。
    let name = if isBrief {
        operator.get_brief_name()
    } else {
        operator.get_name()
    };
    let explain_id = texttree::PrettyIdentifier(
        &format!("{}{}", name, printDriverSide(operator.get_labels())),
        indent,
        isLastChild,
    );
    let mut row = Vec::with_capacity(10);
    row.push(explain_id);
    row.push(formatFloatFixed2(operator.get_est_rows()));
    row.push(formatFloatFixed2(operator.get_cost()));

    if hasRuntimeStats {
        // Go formats `int64(op.ActRows)`, including two's-complement wrapping
        // for protobuf values above MaxInt64.
        row.push((operator.get_act_rows() as i64).to_string());
    }
    let task_type = operator.get_task_type();
    let mut task = taskTypeName(task_type).to_owned();
    if task_type != tipb::TaskType::Unknown && task_type != tipb::TaskType::Root {
        task.push('[');
        task.push_str(storeTypeName(operator.get_store_type()));
        task.push(']');
    }
    row.push(task);
    row.push(printAccessObject(operator.get_access_objects()));

    if hasRuntimeStats {
        let mut execution_info = operator.get_root_basic_exec_info().to_owned();
        appendExecutionInfo(
            &mut execution_info,
            &operator.get_root_group_exec_info().join(", "),
        );
        appendExecutionInfo(&mut execution_info, operator.get_cop_exec_info());
        row.push(execution_info);
    }

    let operator_info = if isBrief && !operator.get_brief_operator_info().is_empty() {
        operator.get_brief_operator_info()
    } else {
        operator.get_operator_info()
    };
    row.push(operator_info.to_owned());

    if hasRuntimeStats {
        row.push(formatBytesOrUnavailable(operator.get_memory_bytes()));
        row.push(formatBytesOrUnavailable(operator.get_disk_bytes()));
    }
    output.push(row);

    // Probe/Build 标签时交换孩子顺序，使 Build 先于 Probe 展示（与 Go 一致）。
    let mut children: Vec<&tipb::ExplainOperator> = operator.get_children().iter().collect();
    if children.len() == 2
        && children[0].get_labels().first() == Some(&tipb::OperatorLabel::ProbeSide)
        && children[1].get_labels().first() == Some(&tipb::OperatorLabel::BuildSide)
    {
        children.swap(0, 1);
    }
    let child_indent = texttree::Indent4Child(indent, isLastChild);
    for (index, child) in children.iter().enumerate() {
        output = decodeBinaryOperator(
            child,
            &child_indent,
            index + 1 == children.len(),
            hasRuntimeStats,
            output,
            isBrief,
        );
    }
    output
}

/// Match `strconv.FormatFloat(value, 'f', 2, 64)`, including Go's spelling
/// for non-finite values.
fn formatFloatFixed2(value: f64) -> String {
    if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else if value.is_nan() {
        "NaN".to_owned()
    } else {
        format!("{value:.2}")
    }
}

/// 追加非空执行信息片段，用逗号分隔。
fn appendExecutionInfo(output: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push_str(", ");
    }
    output.push_str(value);
}

/// 负值表示不可用，显示 `N/A`；否则格式化字节数。
fn formatBytesOrUnavailable(bytes: i64) -> String {
    if bytes < 0 {
        "N/A".to_owned()
    } else {
        memory::FormatBytes(bytes)
    }
}

/// tipb TaskType → 可读任务名。
fn taskTypeName(task_type: tipb::TaskType) -> &'static str {
    match task_type {
        tipb::TaskType::Unknown => "unknown",
        tipb::TaskType::Root => "root",
        tipb::TaskType::Cop => "cop",
        tipb::TaskType::BatchCop => "batchCop",
        tipb::TaskType::Mpp => "mpp",
    }
}

/// tipb StoreType → 可读存储名（tikv/tiflash 等）。
fn storeTypeName(store_type: tipb::StoreType) -> &'static str {
    match store_type {
        tipb::StoreType::Unspecified => "unspecified",
        tipb::StoreType::Tidb => "tidb",
        tipb::StoreType::Tikv => "tikv",
        tipb::StoreType::Tiflash => "tiflash",
    }
}

/// 将 Build/Probe/Seed/Recursive 等标签拼到算子 id 后缀。
fn printDriverSide(labels: &[tipb::OperatorLabel]) -> String {
    labels
        .iter()
        .map(|label| match label {
            tipb::OperatorLabel::Empty => "",
            tipb::OperatorLabel::BuildSide => "(Build)",
            tipb::OperatorLabel::ProbeSide => "(Probe)",
            tipb::OperatorLabel::SeedPart => "(Seed Part)",
            tipb::OperatorLabel::RecursivePart => "(Recursive Part)",
        })
        .collect()
}

/// 动态分区访问对象：all / dual / 显式分区列表。
fn printDynamicPartitionObject(object: &tipb::DynamicPartitionAccessObject) -> String {
    if object.get_all_partitions() {
        "partition:all".to_owned()
    } else if object.get_partitions().is_empty() {
        "partition:dual".to_owned()
    } else {
        format!("partition:{}", object.get_partitions().join(","))
    }
}

/// 汇总 access object：动态分区、扫描对象（表/分区/索引）或其它字符串。
fn printAccessObject(access_objects: &[tipb::AccessObject]) -> String {
    let mut values = Vec::with_capacity(access_objects.len());
    for access_object in access_objects {
        match access_object.access_object.as_ref() {
            Some(AccessObject_oneof_access_object::DynamicPartitionObjects(objects)) => {
                let objects = objects.get_objects();
                if objects.is_empty() {
                    return String::new();
                }
                if objects.len() == 1 {
                    return printDynamicPartitionObject(&objects[0]);
                }
                let value = objects
                    .iter()
                    .map(|object| {
                        format!(
                            "{} of {}",
                            printDynamicPartitionObject(object),
                            object.get_table()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                values.push(value);
            }
            Some(AccessObject_oneof_access_object::ScanObject(object)) => {
                let mut value = String::new();
                if !object.get_table().is_empty() {
                    value.push_str("table:");
                    value.push_str(object.get_table());
                }
                if !object.get_partitions().is_empty() {
                    value.push_str(", partition:");
                    value.push_str(&object.get_partitions().join(","));
                }
                for index in object.get_indexes() {
                    if index.get_is_clustered_index() {
                        value.push_str(", clustered index:");
                    } else {
                        value.push_str(", index:");
                    }
                    value.push_str(index.get_name());
                    value.push('(');
                    value.push_str(&index.get_cols().join(", "));
                    value.push(')');
                }
                values.push(value);
            }
            Some(AccessObject_oneof_access_object::OtherObject(value)) => {
                values.push(value.clone());
            }
            None => {}
        }
    }
    values.join("")
}
