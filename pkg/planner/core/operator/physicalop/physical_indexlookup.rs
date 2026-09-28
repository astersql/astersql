// Copyright 2026 AsterSQL.
// 物理算子：本地索引回表（PhysicalLocalIndexLookup）。
//
// IndexLookUp：索引侧产出 handle（行句柄），表扫描侧据此取回完整行。
// 本文件前半为 Go 对齐草稿（块注释内，已含详细中文），后半为可编译骨架实现。

use crate::physical_common_plans::{PhysicalExpr, PhysicalKind, PhysicalPlanNode, Stats};

#[derive(Clone, Debug, PartialEq)]
/// 本地 IndexLookUp 骨架：索引计划、表计划、是否保序与输出 Schema。
pub struct PhysicalLocalIndexLookup {
    /// 索引侧子计划（产出 handle）。
    pub index_plan: PhysicalPlanNode,
    /// 表侧子计划（按 handle 回表）。
    pub table_plan: PhysicalPlanNode,
    /// 是否保持索引扫描顺序。
    pub keep_order: bool,
    /// 输出列 ID 列表（简化 Schema）。
    pub schema: Vec<i64>,
    /// handle 在索引侧输出 schema 中的偏移；公共句柄从索引值直接读取，因此为空。
    pub index_handle_offsets: Vec<u32>,
}
impl PhysicalLocalIndexLookup {
    /// 按 Go `buildPushDownIndexLookUpPlan` 的规则定位整数 handle。
    pub fn index_handle_offsets_for_schema(
        index_schema: &[i64],
        is_common_handle: bool,
    ) -> Result<Vec<u32>, String> {
        if is_common_handle {
            return Ok(Vec::new());
        }
        index_schema
            .iter()
            .rposition(|column_id| *column_id >= 0 || *column_id == model::ExtraHandleID)
            .map(|offset| vec![offset as u32])
            .ok_or_else(|| "cannot find handle column in index schema".to_owned())
    }

    /// 组装可下推的 IndexLookUp：重置子树 ID，可选挂 Selection/Projection 到表侧。
    pub fn build_push_down_plan(
        mut self,
        table_filters: Vec<PhysicalExpr>,
        projection: Option<Vec<PhysicalExpr>>,
        next_plan_id: &mut i64,
    ) -> Result<PhysicalPlanNode, String> {
        // 避免下推子树与原计划节点 ID 冲突。
        self.table_plan.reset_ids(next_plan_id);
        // 表侧过滤下推为 Selection 包在 table_plan 外。
        if !table_filters.is_empty() {
            let schema = self.table_plan.schema.clone();
            let stats = self.table_plan.stats.clone();
            self.table_plan = PhysicalPlanNode {
                id: *next_plan_id,
                kind: PhysicalKind::Selection {
                    predicates: table_filters,
                },
                schema,
                children: vec![self.table_plan],
                stats,
                required_properties: Vec::new(),
            };
            *next_plan_id += 1;
        }
        // 可选 Projection 再包一层，Schema 用 lookup 输出列。
        if let Some(expressions) = projection {
            let schema = self.schema.clone();
            let stats = self.table_plan.stats.clone();
            self.table_plan = PhysicalPlanNode {
                id: *next_plan_id,
                kind: PhysicalKind::Projection { expressions },
                schema,
                children: vec![self.table_plan],
                stats,
                required_properties: Vec::new(),
            };
            *next_plan_id += 1;
        }
        let stats = self.table_plan.stats.clone();
        let id = *next_plan_id;
        *next_plan_id += 1;
        Ok(PhysicalPlanNode {
            id,
            kind: PhysicalKind::LocalIndexLookup,
            schema: self.schema,
            children: vec![self.index_plan, self.table_plan],
            stats,
            required_properties: Vec::new(),
        })
    }
    pub fn explain_info(&self) -> String {
        format!("index handle offsets:{:?}", self.index_handle_offsets)
    }
    /// 仅 TiKV 支持本地 IndexLookUp，编码内容与 Go `ToPB` 一致。
    pub fn to_pb(&self, store_type: kv::StoreType) -> Result<tipb::Executor, String> {
        if store_type != kv::StoreType::TiKV {
            return Err(format!(
                "unsupported store type {store_type:?} for LocalIndexLookUp"
            ));
        }
        let mut lookup = tipb::IndexLookUp::new();
        lookup.set_index_handle_offsets(self.index_handle_offsets.clone());
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeIndexLookUp);
        executor.set_index_lookup(lookup);
        Ok(executor)
    }
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.index_plan.memory_usage()
            + self.table_plan.memory_usage()
            + (self.index_handle_offsets.len() * std::mem::size_of::<u32>()) as i64
    }
}

/// 从表侧一元链条中摘下最深处的 Scan 节点，返回扫描与父路径。
pub fn detach_root_table_scan(
    plan: &mut PhysicalPlanNode,
) -> Result<(PhysicalPlanNode, Option<Vec<usize>>), String> {
    /// 沿单孩子链向下定位 Scan，路径记为孩子下标序列。
    fn locate(node: &PhysicalPlanNode, path: &mut Vec<usize>) -> bool {
        if matches!(node.kind, PhysicalKind::Scan { .. }) {
            return node.children.is_empty();
        }
        if node.children.len() != 1 {
            return false;
        }
        path.push(0);
        if locate(&node.children[0], path) {
            true
        } else {
            path.pop();
            false
        }
    }
    let mut path = Vec::new();
    if !locate(plan, &mut path) {
        return Err("table-side lookup plan has no PhysicalTableScan root".into());
    }
    if path.is_empty() {
        return Ok((plan.clone(), None));
    }
    let parent_path = path[..path.len() - 1].to_vec();
    // 沿 parent_path 走到父节点，再 remove 末级孩子得到 Scan。
    let mut parent = plan;
    for index in &parent_path {
        parent = &mut parent.children[*index];
    }
    let scan = parent.children.remove(path[path.len() - 1]);
    Ok((scan, Some(parent_path)))
}
