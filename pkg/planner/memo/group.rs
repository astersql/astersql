// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Group：Memo 中逻辑等价表达式的集合（等价类）。
//
// Cascades 把语义相同的逻辑计划收进同一 Group，用指纹去重、按 Operand 建首项索引，
// 并缓存物理属性到 Implementation 的映射；ExploreMark 标记各探索轮次是否已处理。

use crate::{GroupExprRef, ImplementationRef, NewGroupExpr};
use astersql_expression::{NewSchema, Schema};
use astersql_planner_cascades_pattern as pattern;
use astersql_planner_core_operator_logicalop::{
    JoinType, LogicalJoin, LogicalMaxOneRow, LogicalPlan, LogicalPlanRef,
};
use astersql_planner_property::{LogicalProperty, PhysicalProperty};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// 探索轮次位图：每位对应一轮 Cascades 探索是否已覆盖该 Group/表达式。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExploreMark(pub u64);

impl ExploreMark {
    /// 将指定轮次标为已探索。
    pub fn SetExplored(&mut self, round: usize) {
        if round < u64::BITS as usize {
            self.0 |= 1_u64 << round;
        }
    }

    /// 清除指定轮次的已探索标记。
    pub fn SetUnexplored(&mut self, round: usize) {
        if round < u64::BITS as usize {
            self.0 &= !(1_u64 << round);
        }
    }

    /// 查询指定轮次是否已探索。
    pub fn Explored(&self, round: usize) -> bool {
        round < u64::BITS as usize && self.0 & (1_u64 << round) != 0
    }
}

/// 自由函数版：标记 ExploreMark 已探索。
pub fn SetExplored(mark: &mut ExploreMark, round: usize) {
    mark.SetExplored(round);
}

/// 自由函数版：清除 ExploreMark 已探索标记。
pub fn SetUnexplored(mark: &mut ExploreMark, round: usize) {
    mark.SetUnexplored(round);
}

/// 自由函数版：查询 ExploreMark 是否已探索。
pub fn Explored(mark: &ExploreMark, round: usize) -> bool {
    mark.Explored(round)
}

/// Group 的共享可变引用。
pub type GroupRef = Rc<RefCell<Group>>;

/// 全局自增 Group ID，保证指纹里子 Group 身份稳定可区分。
static NEXT_GROUP_ID: AtomicU64 = AtomicU64::new(1);

/// A set of logically equivalent group expressions.
/// 逻辑等价的 GroupExpr 集合（Memo 等价类）。
pub struct Group {
    /// 稳定自增 ID，用于指纹与调试。
    id: u64,
    /// 本等价类中的全部表达式（同类 Operand 聚在一起，顺序对齐 Go list）。
    pub Equivalents: Vec<GroupExprRef>,
    /// 每种 Operand（算子类别）在 Equivalents 中的首项下标。
    pub FirstExpr: HashMap<pattern::Operand, usize>,
    /// 表达式指纹 -> Equivalents 下标，用于去重与 Exists。
    pub Fingerprints: HashMap<Vec<u8>, usize>,
    /// 物理属性哈希 -> 最佳物理实现。
    pub ImplMap: HashMap<Vec<u8>, ImplementationRef>,
    /// 逻辑属性：Schema、MaxOneRow 等。
    pub Prop: LogicalProperty,
    /// 本 Group 所属执行引擎类型（TiDB/TiKV/TiFlash 等）。
    pub EngineType: pattern::EngineType,
    /// Group 自身指纹缓存（默认用 id 的大端字节）。
    pub SelfFingerprint: Vec<u8>,
    /// 探索轮次标记。
    pub ExploreMark: ExploreMark,
    /// 是否已推导过键信息（主键/唯一键、MaxOneRow）。
    hasBuiltKeyInfo: bool,
}

/// 以给定 Schema 新建 Group，可选插入初始表达式；默认引擎为 TiDB。
pub fn NewGroupWithSchema(
    expression: impl Into<Option<GroupExprRef>>,
    schema: &Schema,
) -> GroupRef {
    let group = Rc::new(RefCell::new(Group {
        id: NEXT_GROUP_ID.fetch_add(1, Ordering::Relaxed),
        Equivalents: Vec::new(),
        FirstExpr: HashMap::new(),
        Fingerprints: HashMap::new(),
        ImplMap: HashMap::new(),
        Prop: LogicalProperty {
            // 只拷贝 Columns；PKOrUK 等需后续 BuildKeyInfo 或调用方补齐。
            Schema: Some(Box::new(NewSchema(schema.Columns.clone()))),
            ..LogicalProperty::default()
        },
        EngineType: pattern::EngineTiDB,
        SelfFingerprint: Vec::new(),
        ExploreMark: ExploreMark::default(),
        hasBuiltKeyInfo: false,
    }));
    if let Some(expression) = expression.into() {
        Group::Insert(&group, expression);
    }
    group
}

impl Group {
    /// 返回稳定 Group ID。
    pub fn ID(&self) -> u64 {
        self.id
    }

    /// 设置执行引擎类型并返回自身，便于链式调用。
    pub fn SetEngineType(&mut self, engine: pattern::EngineType) -> &mut Group {
        self.EngineType = engine;
        self
    }

    /// 标记本 Group 在指定轮次已探索。
    pub fn SetExplored(&mut self, round: usize) {
        self.ExploreMark.SetExplored(round);
    }

    /// 清除本 Group 在指定轮次的已探索标记。
    pub fn SetUnexplored(&mut self, round: usize) {
        self.ExploreMark.SetUnexplored(round);
    }

    /// 查询本 Group 在指定轮次是否已探索。
    pub fn Explored(&self, round: usize) -> bool {
        self.ExploreMark.Explored(round)
    }

    /// 返回 Group 自指纹；首次调用时用 id 大端字节填充缓存。
    pub fn FingerPrint(&mut self) -> Vec<u8> {
        if self.SelfFingerprint.is_empty() {
            self.SelfFingerprint = self.id.to_be_bytes().to_vec();
        }
        self.SelfFingerprint.clone()
    }

    /// Inserts after the first expression of the same operand, matching Go's list order.
    /// 插入到同类 Operand 首项之后，以对齐 Go list 顺序；指纹已存在则拒绝。
    pub fn Insert(group: &GroupRef, expression: GroupExprRef) -> bool {
        let (fingerprint, operand) = {
            let mut expression = expression.borrow_mut();
            (
                expression.FingerPrint(),
                pattern::GetOperand(expression.ExprNode.as_ref()),
            )
        };
        let mut current = group.borrow_mut();
        // 指纹冲突：语义上已是同一表达式，静默拒绝。
        if current.Fingerprints.contains_key(&fingerprint) {
            return false;
        }
        // 插在该 Operand 首项之后（无同类则追加到末尾）。
        let index = current
            .FirstExpr
            .get(&operand)
            .map_or(current.Equivalents.len(), |first| first + 1);
        current.Equivalents.insert(index, expression.clone());
        current.rebuild_indexes();
        drop(current);
        // 回填所属 Group 弱引用。
        expression.borrow_mut().Group = Rc::downgrade(group);
        true
    }

    /// 按指纹删除表达式；不存在则 no-op，并清空调用参数的 Group 弱引用。
    pub fn Delete(group: &GroupRef, expression: &GroupExprRef) {
        let fingerprint = expression.borrow_mut().FingerPrint();
        let mut current = group.borrow_mut();
        let Some(index) = current.Fingerprints.get(&fingerprint).copied() else {
            return;
        };
        current.Equivalents.remove(index);
        current.rebuild_indexes();
        drop(current);
        expression.borrow_mut().Group = std::rc::Weak::new();
    }

    /// 清空全部等价表达式与索引；与 Go 一致，原表达式仍保留所属 Group。
    pub fn DeleteAll(group: &GroupRef) {
        let mut current = group.borrow_mut();
        current.Equivalents.clear();
        current.FirstExpr.clear();
        current.Fingerprints.clear();
        current.SelfFingerprint.clear();
    }

    /// 按指纹判断表达式是否仍在本 Group 中。
    pub fn Exists(&self, expression: &GroupExprRef) -> bool {
        self.Fingerprints
            .contains_key(&expression.borrow_mut().FingerPrint())
    }

    /// 返回指定 Operand 的首项下标；OperandAny 返回链表首项。
    pub fn GetFirstElem(&self, operand: pattern::Operand) -> Option<usize> {
        if operand == pattern::OperandAny {
            (!self.Equivalents.is_empty()).then_some(0)
        } else {
            self.FirstExpr.get(&operand).copied()
        }
    }

    /// 按物理属性哈希查找已缓存的物理实现。
    pub fn GetImpl(&self, property: &PhysicalProperty) -> Option<ImplementationRef> {
        self.ImplMap.get(&property.HashCode()).cloned()
    }

    /// 缓存某物理属性下的实现。
    pub fn InsertImpl(&mut self, property: &PhysicalProperty, implementation: ImplementationRef) {
        self.ImplMap.insert(property.HashCode(), implementation);
    }

    /// 根据当前 Equivalents 重建 FirstExpr 与 Fingerprints 索引。
    fn rebuild_indexes(&mut self) {
        self.FirstExpr.clear();
        self.Fingerprints.clear();
        for (index, expression) in self.Equivalents.iter().enumerate() {
            let mut expression = expression.borrow_mut();
            let operand = pattern::GetOperand(expression.ExprNode.as_ref());
            self.FirstExpr.entry(operand).or_insert(index);
            self.Fingerprints.insert(expression.FingerPrint(), index);
        }
    }
}

/// 把逻辑计划树转为 GroupExpr：子节点各自 Convert2Group 后再挂上。
pub fn Convert2GroupExpr(mut node: LogicalPlanRef) -> GroupExprRef {
    let children = node
        .TakeChildren()
        .into_iter()
        .map(Convert2Group)
        .collect::<Vec<_>>();
    let expression = NewGroupExpr(node);
    expression.borrow_mut().SetChildren(children);
    expression
}

/// 把逻辑计划树转为 Group：先 Convert2GroupExpr，再以节点 Schema 建 Group。
pub fn Convert2Group(node: LogicalPlanRef) -> GroupRef {
    let schema = node.Schema().Clone();
    let expression = Convert2GroupExpr(node);
    NewGroupWithSchema(expression, &schema)
}

/// Recursively derives key and max-one-row metadata once for each group.
/// 递归推导每个 Group 的键信息与 MaxOneRow，每个 Group 只构建一次。
pub fn BuildKeyInfo(group: &GroupRef) {
    {
        let mut group = group.borrow_mut();
        if group.hasBuiltKeyInfo {
            return;
        }
        group.hasBuiltKeyInfo = true;
    }

    let Some(expression) = group.borrow().Equivalents.first().cloned() else {
        return;
    };
    let children = expression.borrow().Children.clone();
    // 自底向上：先推导子 Group。
    for child in &children {
        BuildKeyInfo(child);
    }
    let child_schemas = children
        .iter()
        .filter_map(|child| child.borrow().Prop.Schema.as_deref().map(Schema::Clone))
        .collect::<Vec<_>>();
    let child_max_one_row = children
        .iter()
        .map(|child| child.borrow().Prop.MaxOneRow)
        .collect::<Vec<_>>();

    let mut schema = group
        .borrow()
        .Prop
        .Schema
        .as_deref()
        .map(Schema::Clone)
        .unwrap_or_else(|| NewSchema(Vec::new()));
    // 单孩子时直接继承孩子的主键/唯一键集合。
    if child_schemas.len() == 1 {
        schema.PKOrUK = child_schemas[0].Clone().PKOrUK;
    }

    let (schema, max_one_row) = {
        let mut expression = expression.borrow_mut();
        expression.ExprNode.SetSchema(schema);
        expression.ExprNode.BuildKeyInfo();
        let inherited = inherits_max_one_row(expression.ExprNode.as_ref(), &child_max_one_row);
        (
            expression.ExprNode.Schema().Clone(),
            expression.ExprNode.MaxOneRow() || inherited,
        )
    };
    let mut group = group.borrow_mut();
    group.Prop.Schema = Some(Box::new(schema));
    group.Prop.MaxOneRow = max_one_row;
}

/// 按算子类别决定是否从孩子继承 MaxOneRow（最多一行）语义。
fn inherits_max_one_row(plan: &dyn LogicalPlan, children: &[bool]) -> bool {
    if children.is_empty() {
        return false;
    }
    if plan.as_any().is::<LogicalMaxOneRow>() {
        return true;
    }

    match pattern::GetOperand(plan) {
        // 单输入透传类算子：取第一个孩子。
        pattern::OperandLock
        | pattern::OperandLimit
        | pattern::OperandSort
        | pattern::OperandSelection
        | pattern::OperandApply
        | pattern::OperandProjection
        | pattern::OperandWindow
        | pattern::OperandAggregation => children.first().copied().unwrap_or(false),
        pattern::OperandJoin => {
            let Some(join) = plan.as_any().downcast_ref::<LogicalJoin>() else {
                return false;
            };
            match join.JoinType {
                JoinType::SemiJoin
                | JoinType::AntiSemiJoin
                | JoinType::LeftOuterSemiJoin
                | JoinType::AntiLeftOuterSemiJoin => children[0],
                _ => children.len() == 2 && children.iter().all(|value| *value),
            }
        }
        _ => false,
    }
}
