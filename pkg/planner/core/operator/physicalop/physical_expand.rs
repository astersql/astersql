// Copyright 2026 AsterSQL.

// Expand 物理算子：把输入行按 grouping set / level 投影展开，
// 供 ROLLUP/CUBE/GROUPING SETS 生成多组聚合输入；当前可推送实现面向 MPP/TiFlash。

use crate::physical_common_plans::{
    Datum, PartitionType, PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
    TaskType,
};

#[derive(Clone, Debug, Default, PartialEq)]
/// 按多层投影展开输入行，并携带生成列名与 grouping 元数据。
pub struct PhysicalExpand {
    /// 各 grouping level 的投影表达式。
    pub levels: Vec<Vec<PhysicalExpr>>,
    /// 生成列名（如 grouping_id）。
    pub generated_column_names: Vec<String>,
    /// 各展开结果对应的 grouping id。
    pub grouping_ids: Vec<u64>,
    /// grouping 相关位置元数据。
    pub grouping_pos: Vec<usize>,
    /// 输出列 ID。
    pub schema: Vec<i64>,
    /// 唯一孩子计划。
    pub child: Option<PhysicalPlanNode>,
}
impl PhysicalExpand {
    /// 估算 levels 与生成列名容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.levels.iter().flatten().count() as i64
                * std::mem::size_of::<PhysicalExpr>() as i64
            + self
                .generated_column_names
                .iter()
                .map(|name| name.capacity() as i64)
                .sum::<i64>()
    }
    /// 按 level 打印投影表达式摘要。
    pub fn explain_info(&self) -> String {
        let levels = self
            .levels
            .iter()
            .map(|level| {
                format!(
                    "[{}]",
                    level.iter().map(format_expr).collect::<Vec<_>>().join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let schema = self
            .schema
            .iter()
            .map(|id| format!("Column#{id}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("level-projection:{levels}; schema: [{schema}]")
    }
    /// 相对孩子 schema 解析各层表达式列索引。
    pub fn resolve_indices(&mut self) -> Result<(), String> {
        let schema = self
            .child
            .as_ref()
            .map(|child| child.schema.as_slice())
            .unwrap_or(&[]);
        self.levels
            .iter_mut()
            .flatten()
            .try_for_each(|expr| expr.resolve_indices(schema))
    }
    /// 封装 ExpandExecutor；是否内嵌孩子由调用方的存储类型决定。
    pub fn to_pb(&self, _store: TaskType) -> Result<ExpandExecutor, String> {
        Ok(ExpandExecutor {
            levels: self.levels.clone(),
            generated_output_names: self.generated_column_names.clone(),
            grouping_ids: self.grouping_ids.clone(),
            grouping_pos: self.grouping_pos.clone(),
        })
    }
}
/// 将物理表达式格式化为 EXPLAIN 片段。
fn format_expr(expr: &PhysicalExpr) -> String {
    match expr {
        PhysicalExpr::Column(id) => format!("Column#{id}"),
        PhysicalExpr::Constant(Datum::Null) => "NULL".into(),
        PhysicalExpr::Constant(value) => format!("{value:?}"),
        PhysicalExpr::Scalar { function, .. } => function.clone(),
        PhysicalExpr::CorrelatedColumn(id) => format!("CorrelatedColumn#{id}"),
        PhysicalExpr::Default { .. } => "DEFAULT".into(),
    }
}
#[derive(Clone, Debug, PartialEq)]
/// 下推到 TiFlash 的 Expand 执行描述。
pub struct ExpandExecutor {
    pub levels: Vec<Vec<PhysicalExpr>>,
    pub generated_output_names: Vec<String>,
    pub grouping_ids: Vec<u64>,
    pub grouping_pos: Vec<usize>,
}
/// 从逻辑 Expand 枚举 Root/MPP 候选；Expand 自身不保持排序或 MPP 分区。
pub fn exhaust_physical_expand(
    expand: PhysicalExpand,
    property: &PhysicalProperty,
    stats: Stats,
) -> (Vec<PhysicalPlanNode>, bool) {
    if !property.sort_items.is_empty() {
        // 与 Go 的 false 一致：上层可以添加 Sort enforcer。
        return (Vec::new(), false);
    }
    if property.task_type != TaskType::Root && property.task_type != TaskType::Mpp {
        return (Vec::new(), true);
    }
    if property.task_type == TaskType::Mpp && property.partition_type != PartitionType::Any {
        return (Vec::new(), true);
    }

    let make_plan = |task_type| PhysicalPlanNode {
        id: 0,
        kind: PhysicalKind::Expand,
        schema: expand.schema.clone(),
        children: expand.child.clone().into_iter().collect(),
        stats: stats.clone(),
        required_properties: vec![PhysicalProperty {
            task_type,
            ..PhysicalProperty::default()
        }],
    };

    if property.task_type == TaskType::Mpp {
        return (vec![make_plan(TaskType::Mpp)], true);
    }

    // 简化模型把 Go 的 CopSingleRead/CopMultiRead 合并为 Cop，因此保留两次枚举。
    (
        [TaskType::Mpp, TaskType::Cop, TaskType::Mpp, TaskType::Root]
            .into_iter()
            .map(make_plan)
            .collect(),
        true,
    )
}
