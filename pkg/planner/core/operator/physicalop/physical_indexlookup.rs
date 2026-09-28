// Copyright 2026 AsterSQL.
// 物理算子：本地索引回表（PhysicalLocalIndexLookup）。
//
// IndexLookUp：索引侧产出 handle（行句柄），表扫描侧据此取回完整行。
// 本文件前半为 Go 对齐草稿（块注释内，已含详细中文），后半为可编译骨架实现。

/*
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 索引侧产生 handle，表扫描侧据此取回完整行，并可重新挂回原表侧父节点。

use std::mem::size_of;

/// 对应 Go `PhysicalLocalIndexLookUp`，包含索引计划、表扫描计划以及 handle 在索引输出中的位置。
#[derive(Clone)]
pub struct PhysicalLocalIndexLookUp {
    pub physical_schema_producer: PhysicalSchemaProducer,
    /// handle 在 indexPlan 输出 Schema 中的偏移；公共句柄直接从索引值读取，因此可为空。
    pub index_handle_offsets: Vec<u32>,
}

/// 递归重置克隆后计划树的 ID，避免下推子树与原计划节点 ID 冲突。
pub fn reset_plan_id_recursively(ctx: &PlanContext, plan: &mut PhysicalPlan) {
    plan.set_id(ctx.session_vars().next_plan_id());
    for child in plan.children_mut() {
        reset_plan_id_recursively(ctx, child);
    }
}

/// 克隆表侧计划并组装本地 IndexLookUp；返回的树仍保留表扫描上方原有的一元节点。
pub fn build_push_down_index_lookup_plan(
    ctx: &PlanContext,
    index_plan: PhysicalPlan,
    table_plan: &PhysicalPlan,
    is_common_handle: bool,
) -> Result<PhysicalPlan, Error> {
    let mut table_plan = table_plan.clone_for_context(ctx)?;
    reset_plan_id_recursively(ctx, &mut table_plan);

    let mut handle_offsets = Vec::new();
    if !is_common_handle {
        // 整数 handle 通常位于最后；分区场景可能在末尾追加负 ID 的 ExtraPhysTblID，需向前跳过。
        let schema = index_plan.schema();
        let offset = schema.columns.iter().rposition(|column| {
            column.id >= 0 || column.id == EXTRA_HANDLE_ID
        }).ok_or_else(|| Error::message("cannot find handle column in index schema"))?;
        handle_offsets.push(offset as u32);
    }

    // 先把根 TableScan 从表侧链条中摘出，作为 IndexLookUp 的第二个孩子。
    let (table_scan, parent_path) = detach_root_table_scan_plan(&mut table_plan)?;
    let query_block_offset = table_plan.query_block_offset();
    let lookup = PhysicalLocalIndexLookUp {
        physical_schema_producer: PhysicalSchemaProducer::empty(),
        index_handle_offsets: handle_offsets,
    }.init(ctx, index_plan, table_scan, query_block_offset);
    if let Some(path) = parent_path {
        table_plan.node_mut_at_path(&path).set_children(vec![PhysicalPlan::LocalIndexLookUp(lookup)]);
        Ok(table_plan)
    } else {
        Ok(PhysicalPlan::LocalIndexLookUp(lookup))
    }
}

impl PhysicalLocalIndexLookUp {
    /// 初始化基础节点，并令输出统计与 Schema 跟随表扫描侧。
    pub fn new(
        ctx: &PlanContext,
        index_plan: PhysicalPlan,
        table_scan: PhysicalTableScan,
        offset: i32,
    ) -> Self {
        let stats = table_scan.stats_info().clone();
        let schema = table_scan.schema().clone();
        let mut producer = PhysicalSchemaProducer::new(ctx, TYPE_LOCAL_INDEX_LOOKUP, offset);
        producer.set_children(vec![index_plan, PhysicalPlan::TableScan(table_scan)]);
        producer.set_stats(stats);
        producer.set_schema(schema);
        Self { physical_schema_producer: producer, index_handle_offsets: Vec::new() }
    }

    /// 与 Go `Init` 一致，但允许构造器在挂接前写入已算出的 handle 偏移。
    pub fn init(
        mut self,
        ctx: &PlanContext,
        index_plan: PhysicalPlan,
        table_scan: PhysicalTableScan,
        offset: i32,
    ) -> Self {
        let offsets = std::mem::take(&mut self.index_handle_offsets);
        let mut initialized = Self::new(ctx, index_plan, table_scan, offset);
        initialized.index_handle_offsets = offsets;
        initialized
    }

    /// 编码 TiKV IndexLookUp protobuf；本地回表目前不支持 TiFlash 等其它存储。
    pub fn to_pb(&self, _ctx: &BuildPbContext, store_type: StoreType) -> Result<PbExecutor, Error> {
        if store_type != StoreType::TiKv {
            return Err(Error::message(format!(
                "unsupported store type {store_type:?} for LocalIndexLookUp"
            )));
        }
        Ok(PbExecutor::index_lookup(PbIndexLookUp {
            index_handle_offsets: self.index_handle_offsets.clone(),
        }))
    }

    /// EXPLAIN 输出保留 Go 的偏移数组格式，便于定位 handle 来自索引输出的哪一列。
    /// EXPLAIN：local index lookup 与保序标志。
    pub fn explain_info(&self) -> String {
        format!("index handle offsets:{:?}", self.index_handle_offsets)
    }

    /// 计算节点内存；子计划由 PhysicalSchemaProducer 统计，偏移数组按 u32 容量计入。
    /// 估算结构体与两侧子计划内存。
    pub fn memory_usage(&self) -> i64 {
        let own_size = size_of::<Self>() - size_of::<PhysicalSchemaProducer>();
        self.physical_schema_producer.memory_usage()
            + own_size as i64
            + (self.index_handle_offsets.len() * size_of::<u32>()) as i64
    }

    /// 深拷贝基础计划及 handle 偏移，并把会话上下文切换到新 PlanContext。
    pub fn clone_for_context(&self, new_ctx: &PlanContext) -> Result<Self, Error> {
        let mut cloned = self.clone();
        cloned.physical_schema_producer = self
            .physical_schema_producer
            .clone_with_self(new_ctx, &cloned)?;
        cloned.physical_schema_producer.set_sctx(new_ctx);
        cloned.index_handle_offsets = self.index_handle_offsets.clone();
        Ok(cloned)
    }
}

/// 从只有单子节点的表侧计划链中摘下最深处的 `PhysicalTableScan`。
/// 返回路径而非 Go 指针，以便 Rust 在结束遍历后再取得唯一可变借用并清空父节点孩子。
pub fn detach_root_table_scan_plan(
    plan: &mut PhysicalPlan,
) -> Result<(PhysicalTableScan, Option<Vec<usize>>), Error> {
    let mut path = Vec::new();
    let mut current = &*plan;
    loop {
        match current.children() {
            [] => break,
            [child] => {
                path.push(0);
                current = child;
            }
            _ => return Err(Error::message("table-side lookup plan must be a unary chain")),
        }
    }
    let leaf = plan.take_node_at_path(&path);
    let table_scan = match leaf {
        PhysicalPlan::TableScan(scan) => scan,
        _ => return Err(Error::message("root of table-side lookup plan is not PhysicalTableScan")),
    };
    // 根即 Scan：直接返回克隆，无父路径。
    if path.is_empty() {
        Ok((table_scan, None))
    } else {
        let parent_path = path[..path.len() - 1].to_vec();
        // 摘除后父节点暂时无孩子，调用方随后会把 LocalIndexLookUp 接回该位置。
        plan.node_mut_at_path(&parent_path).set_children(Vec::new());
        Ok((table_scan, Some(parent_path)))
    }
}
*/

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
