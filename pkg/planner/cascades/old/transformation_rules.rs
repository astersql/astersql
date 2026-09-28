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

// Cascades 逻辑变换规则集（对应 Go `transformation_rules.go`）。
//
// 每条 `Transformation` 规则：缓存 Pattern → `matches` 附加条件 → `on_transform`
// 产出新 GroupExpr，并返回 eraseOld / eraseAll 擦除标志。规则按 TiDB 层、
// TiKV 层、收尾三批次组织，由 Optimizer 探索阶段依次应用。

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use astersql_expression_aggregation as aggregation;
use astersql_parser_ast as ast;
use astersql_parser_mysql::r#type as mysql;
use astersql_planner_cascades_pattern as pattern;
use astersql_planner_core as plannercore;
use astersql_planner_core_operator_logicalop as logicalop;
use astersql_planner_core_operator_physicalop as physicalop;
use astersql_planner_core_rule_util as ruleutil;
use astersql_planner_planctx as planctx;
use astersql_planner_util as util;
use astersql_planner_util_coreusage as coreusage;
use astersql_types as types;
use astersql_util_dbterror_plannererrors as plannererrors;
use astersql_util_intset as intset;
use astersql_util_ranger as ranger;

use logicalop::LogicalPlan;

/// 从 core_base 再导出 JoinType 等，缩短规则实现中的路径。
mod base {
    pub use astersql_planner_core_base::JoinType::{
        AntiLeftOuterSemiJoin, InnerJoin, LeftOuterJoin, LeftOuterSemiJoin, RightOuterJoin,
        SemiJoin,
    };
    pub use astersql_planner_core_base::*;
}

/// KV 存储类型再导出；TiKV 常量供下推判断使用。
mod kv {
    pub use astersql_kv::*;
    pub const TiKV: StoreType = StoreType::TiKV;
}

/// 表达式辅助：下推分类、常量传播，以及 NOT / IS NULL / SetVar 检测。
mod expression {
    pub use astersql_expression::*;

    fn tiny_field_type() -> types::FieldType {
        let mut field_type = types::FieldType::default();
        field_type.SetType(astersql_parser_mysql::r#type::TypeTiny);
        field_type
    }

    /// 按 PB 编码与 KV client 能力拆分可下推、不可下推谓词。
    pub fn PushDownExprs(
        ctx: PushDownContext,
        expressions: Vec<ExprBox>,
        _store_type: astersql_kv::StoreType,
    ) -> (Vec<ExprBox>, Vec<ExprBox>) {
        let converter = ctx.PbConverter();
        let mut pushed = Vec::with_capacity(expressions.len());
        let mut remained = Vec::with_capacity(expressions.len());
        for expression in expressions {
            if converter.ExprToPB(expression.as_ref()).is_some() {
                pushed.push(expression);
            } else {
                remained.push(expression);
            }
        }
        (pushed, remained)
    }

    /// 复用 expression crate 的完整常量传播求解器。
    pub fn PropagateConstant(
        ctx: &dyn exprctx::ExprContext,
        expressions: Vec<ExprBox>,
    ) -> Vec<ExprBox> {
        astersql_expression::PropagateConstantRef(ctx, None, expressions)
    }

    pub fn not(expression: ExprBox, ctx: &dyn BuildContext) -> ExprBox {
        NewFunctionInternal(
            ctx,
            astersql_parser_ast::UnaryNot,
            tiny_field_type(),
            vec![expression],
        )
        .unwrap_or_else(|| Box::new(NewNull()))
    }

    pub fn is_null(expression: ExprBox, ctx: &dyn BuildContext) -> ExprBox {
        NewFunctionInternal(
            ctx,
            astersql_parser_ast::IsNull,
            tiny_field_type(),
            vec![expression],
        )
        .unwrap_or_else(|| Box::new(NewNull()))
    }

    /// 检测表达式树是否含带赋值语义的 SET_VAR。
    pub fn HasAssignSetVarFunc(expr: &ExprBox) -> bool {
        let Some(scalar) = expr.as_any().downcast_ref::<ScalarFunction>() else {
            return false;
        };
        (scalar.FuncName.L == astersql_parser_ast::SetVar
            && scalar
                .GetArgs()
                .iter()
                .any(|argument| argument.as_any().is::<ScalarFunction>()))
            || scalar.GetArgs().iter().any(HasAssignSetVarFunc)
    }

    /// 检测表达式树是否含 SET_VAR / GET_VAR。
    pub fn HasGetSetVarFunc(expr: &ExprBox) -> bool {
        let Some(scalar) = expr.as_any().downcast_ref::<ScalarFunction>() else {
            return false;
        };
        matches!(
            scalar.FuncName.L.as_str(),
            astersql_parser_ast::SetVar | astersql_parser_ast::GetVar
        ) || scalar.GetArgs().iter().any(HasGetSetVarFunc)
    }
}

/// Memo 适配层：GroupHandle、PlanHandle、ExprIter 扩展，便于规则读写 Group。
mod memo {
    use astersql_expression::Schema;
    use astersql_planner_cascades_pattern::EngineType;
    use astersql_planner_core_operator_logicalop::{BaseLogicalPlan, LogicalPlan, LogicalPlanRef};
    pub use astersql_planner_memo::*;
    use std::any::Any;
    use std::marker::PhantomData;
    use std::ops::{Deref, DerefMut};

    #[derive(Clone)]
    /// 持有 GroupRef 并以 Deref 暴露 Group；变换阶段单线程使用。
    pub struct GroupHandle(pub astersql_planner_memo::GroupRef);

    impl Deref for GroupHandle {
        type Target = astersql_planner_memo::Group;

        fn deref(&self) -> &Self::Target {
            // Memo transformations are single-threaded. GroupHandle owns the
            // Rc and never removes/replaces the Group allocation.
            unsafe { &*self.0.as_ptr() }
        }
    }

    impl DerefMut for GroupHandle {
        fn deref_mut(&mut self) -> &mut Self::Target {
            unsafe { &mut *self.0.as_ptr() }
        }
    }

    impl GroupHandle {
        pub fn SetEngineType(mut self, engine: EngineType) -> Self {
            self.EngineType = engine;
            self
        }

        pub fn BuildKeyInfo(&mut self) {
            astersql_planner_memo::BuildKeyInfo(&self.0);
        }
    }

    pub type Group = GroupHandle;

    /// 统一把 GroupHandle / GroupRef 转成 GroupRef。
    pub trait IntoGroupRef {
        fn into_group_ref(self) -> astersql_planner_memo::GroupRef;
    }

    impl IntoGroupRef for GroupHandle {
        fn into_group_ref(self) -> astersql_planner_memo::GroupRef {
            self.0
        }
    }

    impl IntoGroupRef for astersql_planner_memo::GroupRef {
        fn into_group_ref(self) -> astersql_planner_memo::GroupRef {
            self
        }
    }

    /// 从多种 Schema 持有形式取出克隆 Schema。
    pub trait SchemaSource {
        fn schema(&self) -> Schema;
    }

    impl SchemaSource for Schema {
        fn schema(&self) -> Schema {
            self.Clone()
        }
    }

    impl SchemaSource for &Schema {
        fn schema(&self) -> Schema {
            (*self).Clone()
        }
    }

    impl SchemaSource for Box<Schema> {
        fn schema(&self) -> Schema {
            self.as_ref().Clone()
        }
    }

    impl SchemaSource for Option<Box<Schema>> {
        fn schema(&self) -> Schema {
            self.as_deref()
                .map(Schema::Clone)
                .expect("memo group must carry a schema")
        }
    }

    impl SchemaSource for Option<Schema> {
        fn schema(&self) -> Schema {
            self.as_ref()
                .map(Schema::Clone)
                .expect("memo group must carry a schema")
        }
    }

    pub struct PlanHandle<T: LogicalPlan + 'static> {
        expression: GroupExprRef,
        marker: PhantomData<T>,
    }

    impl<T: LogicalPlan + 'static> Clone for PlanHandle<T> {
        fn clone(&self) -> Self {
            Self {
                expression: self.expression.clone(),
                marker: PhantomData,
            }
        }
    }

    impl<T: LogicalPlan + 'static> Deref for PlanHandle<T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            unsafe {
                (*self.expression.as_ptr())
                    .ExprNode
                    .as_any()
                    .downcast_ref::<T>()
                    .expect("memo pattern and logical operator must agree")
            }
        }
    }

    impl<T: LogicalPlan + 'static> LogicalPlan for PlanHandle<T> {
        fn as_any(&self) -> &dyn Any {
            self.deref().as_any()
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            unsafe { (*self.expression.as_ptr()).ExprNode.as_any_mut() }
        }

        fn base(&self) -> &BaseLogicalPlan {
            self.deref().base()
        }

        fn base_mut(&mut self) -> &mut BaseLogicalPlan {
            unsafe { (*self.expression.as_ptr()).ExprNode.base_mut() }
        }
    }

    impl PlanHandle<astersql_planner_core_operator_logicalop::DataSource> {
        pub fn SourceRef(&self) -> astersql_planner_core_operator_logicalop::DataSourceRef {
            use astersql_planner_core_operator_logicalop::DataSource;
            use std::cell::RefCell;
            use std::rc::Rc;

            let mut source = DataSource {
                TableInfo: self.TableInfo.clone(),
                Columns: self.Columns.clone(),
                DBName: self.DBName.clone(),
                TableAsName: self.TableAsName.clone(),
                PushedDownConds: self.PushedDownConds.clone(),
                AllConds: self.AllConds.clone(),
                TableStats: self.TableStats.clone(),
                AllPossibleAccessPaths: self
                    .AllPossibleAccessPaths
                    .iter()
                    .map(astersql_planner_util::AccessPath::Clone)
                    .collect(),
                PossibleAccessPaths: self
                    .PossibleAccessPaths
                    .iter()
                    .map(astersql_planner_util::AccessPath::Clone)
                    .collect(),
                PartitionDefIdx: self.PartitionDefIdx,
                PhysicalTableID: self.PhysicalTableID,
                PartitionNames: self.PartitionNames.clone(),
                HandleCols: self
                    .HandleCols
                    .as_ref()
                    .map(|value| value.CloneHandleCols()),
                UnMutableHandleCols: self
                    .UnMutableHandleCols
                    .as_ref()
                    .map(|value| value.CloneHandleCols()),
                TblCols: self.TblCols.clone(),
                TblColsByID: self.TblColsByID.clone(),
                CommonHandleCols: self.CommonHandleCols.clone(),
                CommonHandleLens: self.CommonHandleLens.clone(),
                PreferStoreType: self.PreferStoreType,
                IsForUpdateRead: self.IsForUpdateRead,
                ContainExprPrefixUk: self.ContainExprPrefixUk,
                ColsRequiringFullLen: self.ColsRequiringFullLen.clone(),
                AccessPathMinSelectivity: self.AccessPathMinSelectivity,
                AskedColumnGroup: self.AskedColumnGroup.clone(),
                InterestingColumns: self.InterestingColumns.clone(),
                ..Default::default()
            }
            .Init(
                self.SCtx()
                    .cloned()
                    .expect("data source must carry a plan context"),
                self.QueryBlockOffset(),
            );
            source.SetSchema(self.Schema().Clone());
            source.SetOutputNames(self.OutputNames().Shallow());
            Rc::new(RefCell::new(source))
        }
    }

    impl PlanHandle<astersql_planner_core_operator_logicalop::LogicalJoin> {
        pub fn has_conditions(&self) -> bool {
            !self.EqualConditions.is_empty()
                || !self.NAEQConditions.is_empty()
                || !self.LeftConditions.is_empty()
                || !self.RightConditions.is_empty()
                || !self.OtherConditions.is_empty()
        }
    }

    #[derive(Clone)]
    pub struct PlanNodeHandle(pub GroupExprRef);

    impl Deref for PlanNodeHandle {
        type Target = dyn LogicalPlan;

        fn deref(&self) -> &Self::Target {
            unsafe { (*self.0.as_ptr()).ExprNode.as_ref() }
        }
    }

    impl LogicalPlan for PlanNodeHandle {
        fn as_any(&self) -> &dyn Any {
            self.deref().as_any()
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            unsafe { (*self.0.as_ptr()).ExprNode.as_any_mut() }
        }

        fn base(&self) -> &BaseLogicalPlan {
            self.deref().base()
        }

        fn base_mut(&mut self) -> &mut BaseLogicalPlan {
            unsafe { (*self.0.as_ptr()).ExprNode.base_mut() }
        }
    }

    pub struct ExprView {
        pub ExprNode: PlanNodeHandle,
        pub Children: Vec<GroupHandle>,
        pub Group: GroupHandle,
        expression: GroupExprRef,
    }

    impl ExprView {
        pub fn Schema(&self) -> Schema {
            self.Group
                .Prop
                .Schema
                .as_deref()
                .map(Schema::Clone)
                .expect("memo expression group must carry a schema")
        }

        pub fn AddAppliedRule(&mut self, rule: &dyn super::Transformation) {
            self.expression.borrow_mut().AddAppliedRule(rule.rule_id());
        }

        pub fn HasAppliedRule(&self, rule: &dyn super::Transformation) -> bool {
            self.expression.borrow().HasAppliedRule(rule.rule_id())
        }
    }

    pub trait ExprIterExt {
        fn child(&self, index: usize) -> &ExprIter;
        fn group(&self) -> GroupHandle;
        fn expr(&self) -> ExprView;
        fn logical_selection(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalSelection>;
        fn logical_table_scan(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalTableScan>;
        fn logical_index_scan(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalIndexScan>;
        fn tikv_single_gather(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::TiKVSingleGather>;
        fn data_source(&self) -> PlanHandle<astersql_planner_core_operator_logicalop::DataSource>;
        fn logical_aggregation(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalAggregation>;
        fn logical_sort(&self)
        -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalSort>;
        fn logical_projection(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalProjection>;
        fn logical_window(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalWindow>;
        fn logical_limit(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalLimit>;
        fn logical_top_n(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalTopN>;
        fn logical_union_all(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalUnionAll>;
        fn logical_join(&self)
        -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalJoin>;
        fn logical_apply(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalApply>;
    }

    impl ExprIterExt for ExprIter {
        fn child(&self, index: usize) -> &ExprIter {
            &self.Children[index]
        }

        fn group(&self) -> GroupHandle {
            GroupHandle(
                self.Group
                    .as_ref()
                    .cloned()
                    .expect("matched iterator must have a group"),
            )
        }

        fn expr(&self) -> ExprView {
            let expression = self
                .GetExpr()
                .expect("matched iterator must have an expression");
            let borrowed = expression.borrow();
            let children = borrowed.Children.iter().cloned().map(GroupHandle).collect();
            let group = borrowed
                .Group
                .upgrade()
                .map(GroupHandle)
                .expect("memo expression must belong to a group");
            drop(borrowed);
            ExprView {
                ExprNode: PlanNodeHandle(expression.clone()),
                Children: children,
                Group: group,
                expression,
            }
        }

        fn logical_selection(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalSelection> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_table_scan(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalTableScan> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_index_scan(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalIndexScan> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn tikv_single_gather(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::TiKVSingleGather> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn data_source(&self) -> PlanHandle<astersql_planner_core_operator_logicalop::DataSource> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_aggregation(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalAggregation> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_sort(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalSort> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_projection(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalProjection> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_window(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalWindow> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_limit(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalLimit> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_top_n(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalTopN> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_union_all(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalUnionAll> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_join(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalJoin> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
        fn logical_apply(
            &self,
        ) -> PlanHandle<astersql_planner_core_operator_logicalop::LogicalApply> {
            PlanHandle {
                expression: self.GetExpr().unwrap(),
                marker: PhantomData,
            }
        }
    }

    pub trait IntoLogicalPlanRef {
        fn into_logical_plan_ref(self) -> LogicalPlanRef;
    }

    impl<T: LogicalPlan + 'static> IntoLogicalPlanRef for T {
        fn into_logical_plan_ref(self) -> LogicalPlanRef {
            Box::new(self)
        }
    }

    pub fn NewGroupExpr<T: IntoLogicalPlanRef>(node: T) -> GroupExprRef {
        astersql_planner_memo::NewGroupExpr(node.into_logical_plan_ref())
    }

    pub fn NewGroupExprWithChildren<T: IntoLogicalPlanRef>(
        node: T,
        children: Vec<impl IntoGroupRef>,
    ) -> GroupExprRef {
        let expression = NewGroupExpr(node);
        expression.borrow_mut().SetChildren(
            children
                .into_iter()
                .map(IntoGroupRef::into_group_ref)
                .collect(),
        );
        expression
    }

    pub fn NewGroupWithSchema(
        expression: impl Into<Option<GroupExprRef>>,
        schema: impl SchemaSource,
    ) -> GroupHandle {
        GroupHandle(astersql_planner_memo::NewGroupWithSchema(
            expression,
            &schema.schema(),
        ))
    }
}

use memo::ExprIterExt as _;

trait SchemaClone {
    fn clone(&self) -> expression::Schema;
}

impl SchemaClone for expression::Schema {
    fn clone(&self) -> expression::Schema {
        self.Clone()
    }
}

impl SchemaClone for Option<Box<expression::Schema>> {
    fn clone(&self) -> expression::Schema {
        self.as_deref()
            .map(expression::Schema::Clone)
            .expect("memo group must carry a schema")
    }
}

/// 规则构造新逻辑节点时取得非空 Session/Plan Context。
fn required_context(context: Option<&base::ContextRef>) -> base::ContextRef {
    context
        .cloned()
        .expect("logical memo operators always carry a plan context")
}

trait ContextOptionExt<'a> {
    fn plan_context(self) -> &'a dyn base::PlanContext;
    fn GetExprCtx(self) -> &'a dyn planctx::exprctx::ExprContext;
    fn GetSessionVars(self) -> &'a planctx::variable::SessionVars;
    fn GetRangerCtx(self) -> &'a planctx::rangerctx::RangerContext<'a>;
}

impl<'a> ContextOptionExt<'a> for Option<&'a base::ContextRef> {
    fn plan_context(self) -> &'a dyn base::PlanContext {
        self.expect("logical memo operators always carry a plan context")
            .as_ref()
    }

    fn GetExprCtx(self) -> &'a dyn planctx::exprctx::ExprContext {
        self.plan_context().GetExprCtx()
    }

    fn GetSessionVars(self) -> &'a planctx::variable::SessionVars {
        self.plan_context().GetSessionVars()
    }

    fn GetRangerCtx(self) -> &'a planctx::rangerctx::RangerContext<'a> {
        self.plan_context().GetRangerCtx()
    }
}

trait NewLogicalSelection {
    fn new(
        conditions: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalSelection for logicalop::LogicalSelection {
    fn new(
        conditions: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            Conditions: conditions,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalProjection {
    fn new(
        expressions: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalProjection for logicalop::LogicalProjection {
    fn new(
        expressions: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalLimit {
    fn new(
        offset: u64,
        count: u64,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalLimit for logicalop::LogicalLimit {
    fn new(
        offset: u64,
        count: u64,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            Offset: offset,
            Count: count,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalTopN {
    fn new(
        by_items: Vec<util::ByItems>,
        offset: u64,
        count: u64,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalTopN for logicalop::LogicalTopN {
    fn new(
        by_items: Vec<util::ByItems>,
        offset: u64,
        count: u64,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            ByItems: by_items,
            Offset: offset,
            Count: count,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalAggregation {
    fn new(
        functions: Vec<aggregation::AggFuncDesc>,
        group_by: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalAggregation for logicalop::LogicalAggregation {
    fn new(
        functions: Vec<aggregation::AggFuncDesc>,
        group_by: Vec<expression::ExprBox>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            AggFuncs: functions,
            GroupByItems: group_by,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalTableDual {
    fn new(row_count: i32, context: Option<&base::ContextRef>, query_block_offset: i32) -> Self;
}

impl NewLogicalTableDual for logicalop::LogicalTableDual {
    fn new(row_count: i32, context: Option<&base::ContextRef>, query_block_offset: i32) -> Self {
        Self {
            RowCount: row_count,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalWindow {
    fn new(
        functions: Vec<logicalop::WindowFuncDesc>,
        partition_by: Vec<logicalop::SortItem>,
        order_by: Vec<logicalop::SortItem>,
        frame: Option<logicalop::WindowFrame>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalWindow for logicalop::LogicalWindow {
    fn new(
        functions: Vec<logicalop::WindowFuncDesc>,
        partition_by: Vec<logicalop::SortItem>,
        order_by: Vec<logicalop::SortItem>,
        frame: Option<logicalop::WindowFrame>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            WindowFuncDescs: functions,
            PartitionBy: partition_by,
            OrderBy: order_by,
            Frame: frame,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait NewLogicalApply {
    fn new(
        join: logicalop::LogicalJoin,
        correlated_columns: Vec<expression::CorrelatedColumn>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self;
}

impl NewLogicalApply for logicalop::LogicalApply {
    fn new(
        join: logicalop::LogicalJoin,
        correlated_columns: Vec<expression::CorrelatedColumn>,
        context: Option<&base::ContextRef>,
        query_block_offset: i32,
    ) -> Self {
        Self {
            LogicalJoin: join,
            CorCols: correlated_columns,
            ..Default::default()
        }
        .Init(required_context(context), query_block_offset)
    }
}

trait CloneLogicalTableScan {
    fn from_scan(scan: memo::PlanHandle<logicalop::LogicalTableScan>) -> Self;
}

impl CloneLogicalTableScan for logicalop::LogicalTableScan {
    fn from_scan(scan: memo::PlanHandle<logicalop::LogicalTableScan>) -> Self {
        let mut cloned = Self {
            Source: scan.Source.clone(),
            HandleCols: scan
                .HandleCols
                .as_ref()
                .map(|handle| handle.CloneHandleCols()),
            AccessConds: scan.AccessConds.clone(),
            Ranges: scan.Ranges.clone(),
            ..Default::default()
        }
        .Init(required_context(scan.SCtx()), scan.QueryBlockOffset());
        cloned.SetSchema(scan.Schema().Clone());
        cloned
    }
}

trait CloneLogicalIndexScan {
    fn with_ranges(
        scan: memo::PlanHandle<logicalop::LogicalIndexScan>,
        ranges: &ranger::DetachRangeResult,
    ) -> Self;
}

impl CloneLogicalIndexScan for logicalop::LogicalIndexScan {
    fn with_ranges(
        scan: memo::PlanHandle<logicalop::LogicalIndexScan>,
        ranges: &ranger::DetachRangeResult,
    ) -> Self {
        let mut cloned = Self {
            Source: scan.Source.clone(),
            IsDoubleRead: scan.IsDoubleRead,
            EqCondCount: ranges.EqCondCount,
            AccessConds: ranges.AccessConds.clone(),
            Ranges: ranges.Ranges.to_vec(),
            Index: scan.Index.clone(),
            Columns: scan.Columns.clone(),
            FullIdxCols: scan.FullIdxCols.clone(),
            FullIdxColLens: scan.FullIdxColLens.clone(),
            IdxCols: scan.IdxCols.clone(),
            IdxColLens: scan.IdxColLens.clone(),
            ..Default::default()
        }
        .Init(required_context(scan.SCtx()), scan.QueryBlockOffset());
        cloned.SetSchema(scan.Schema().Clone());
        cloned
    }
}

/// GroupExpr 上记录已应用规则、设置子 Group 的便捷扩展。
trait GroupExprRuleExt {
    fn AddAppliedRule(&self, rule: &dyn Transformation);
    fn SetChildren(&self, children: Vec<memo::Group>);
}

impl GroupExprRuleExt for memo::GroupExprRef {
    fn AddAppliedRule(&self, rule: &dyn Transformation) {
        self.borrow_mut().AddAppliedRule(rule.rule_id());
    }

    fn SetChildren(&self, children: Vec<memo::Group>) {
        self.borrow_mut().SetChildren(
            children
                .into_iter()
                .map(memo::IntoGroupRef::into_group_ref)
                .collect(),
        );
    }
}

/// 变换结果：新表达式列表 + eraseOld + eraseAll。
pub type TransformResult = logicalop::Result<(Vec<memo::GroupExprRef>, bool, bool)>;

// Transformation 对应 Go 接口：规则先暴露缓存 Pattern，再做附加匹配，最后返回新表达式及两个擦除标志。
/// Cascades 变换规则接口：Pattern、附加匹配、执行变换。
///
/// `on_transform` 返回的 eraseOld 表示删除当前绑定表达式；eraseAll 表示清空整个 Group。
pub trait Transformation {
    fn rule_id(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        std::any::type_name::<Self>().hash(&mut hasher);
        hasher.finish()
    }
    fn get_pattern(&self) -> &pattern::Pattern;
    fn matches(&self, expr: &memo::ExprIter) -> bool;
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult;
}

/// 按根 Operand 分组的一批变换规则。
pub type TransformationRuleBatch = HashMap<pattern::Operand, Vec<Box<dyn Transformation>>>;

// default_rule_batches 保持 Go 的三阶段执行顺序：TiDB 层、TiKV 层、执行器约束收尾。
/// 默认三阶段规则批次：TiDB 层 → TiKV 层 → 收尾。
pub fn default_rule_batches() -> Vec<TransformationRuleBatch> {
    vec![
        tidb_layer_optimization_batch(),
        tikv_layer_optimization_batch(),
        post_transformation_batch(),
    ]
}

// TiDB 层规则按根 Operand 分组；同一组内顺序会影响 memo 中候选表达式的产生顺序。
/// TiDB 计算层变换：下推 Selection/Limit/TopN、消除/合并 Projection 等。
pub fn tidb_layer_optimization_batch() -> TransformationRuleBatch {
    HashMap::from([
        (
            pattern::OperandSelection,
            vec![
                NewRulePushSelDownSort(),
                NewRulePushSelDownProjection(),
                NewRulePushSelDownAggregation(),
                NewRulePushSelDownJoin(),
                NewRulePushSelDownUnionAll(),
                NewRulePushSelDownWindow(),
                NewRuleMergeAdjacentSelection(),
            ],
        ),
        (
            pattern::OperandAggregation,
            vec![
                NewRuleMergeAggregationProjection(),
                NewRuleEliminateSingleMaxMin(),
                NewRuleEliminateOuterJoinBelowAggregation(),
                NewRuleTransformAggregateCaseToSelection(),
                NewRuleTransformAggToProj(),
            ],
        ),
        (
            pattern::OperandLimit,
            vec![
                NewRuleTransformLimitToTopN(),
                NewRulePushLimitDownProjection(),
                NewRulePushLimitDownUnionAll(),
                NewRulePushLimitDownOuterJoin(),
                NewRuleMergeAdjacentLimit(),
                NewRuleTransformLimitToTableDual(),
            ],
        ),
        (
            pattern::OperandProjection,
            vec![
                NewRuleEliminateProjection(),
                NewRuleMergeAdjacentProjection(),
                NewRuleEliminateOuterJoinBelowProjection(),
            ],
        ),
        (
            pattern::OperandTopN,
            vec![
                NewRulePushTopNDownProjection(),
                NewRulePushTopNDownOuterJoin(),
                NewRulePushTopNDownUnionAll(),
                NewRuleMergeAdjacentTopN(),
            ],
        ),
        (
            pattern::OperandApply,
            vec![NewRuleTransformApplyToJoin(), NewRulePullSelectionUpApply()],
        ),
        (pattern::OperandJoin, vec![NewRuleTransformJoinCondToSel()]),
        (pattern::OperandWindow, vec![NewRuleMergeAdjacentWindow()]),
    ])
}

// TiKV 层批次只包含访问路径枚举以及可下推到 coprocessor 的 Selection/Agg/Limit/TopN。
/// TiKV/coprocessor 层：路径枚举与可下推算子推入 Gather/Scan。
pub fn tikv_layer_optimization_batch() -> TransformationRuleBatch {
    HashMap::from([
        (pattern::OperandDataSource, vec![NewRuleEnumeratePaths()]),
        (
            pattern::OperandSelection,
            vec![
                NewRulePushSelDownTiKVSingleGather(),
                NewRulePushSelDownTableScan(),
                NewRulePushSelDownIndexScan(),
                NewRuleMergeAdjacentSelection(),
            ],
        ),
        (
            pattern::OperandAggregation,
            vec![NewRulePushAggDownGather()],
        ),
        (
            pattern::OperandLimit,
            vec![NewRulePushLimitDownTiKVSingleGather()],
        ),
        (
            pattern::OperandTopN,
            vec![NewRulePushTopNDownTiKVSingleGather()],
        ),
    ])
}

// 收尾批次为 TiDB 执行器预先计算 TopN/Agg 中的标量表达式，并再次清理 Projection。
/// 收尾：为 Agg/TopN 注入 Projection，并再次清理多余 Projection。
pub fn post_transformation_batch() -> TransformationRuleBatch {
    HashMap::from([
        (
            pattern::OperandProjection,
            vec![
                NewRuleEliminateProjection(),
                NewRuleMergeAdjacentProjection(),
            ],
        ),
        (
            pattern::OperandAggregation,
            vec![NewRuleInjectProjectionBelowAgg()],
        ),
        (
            pattern::OperandTopN,
            vec![NewRuleInjectProjectionBelowTopN()],
        ),
    ])
}

// BaseRule 对应 Go baseRule，所有具体规则内嵌它以缓存模式树。
/// 规则基类：缓存 Pattern，默认 matches 恒为 true。
pub struct BaseRule {
    pub pattern: pattern::Pattern,
}
impl BaseRule {
    fn matches(&self, _expr: &memo::ExprIter) -> bool {
        true
    }
    fn get_pattern(&self) -> &pattern::Pattern {
        &self.pattern
    }
}

/// 无改写结果：空列表且不擦除。
fn unchanged() -> TransformResult {
    Ok((Vec::new(), false, false))
}
/// 改写结果：返回新表达式并 eraseOld=true。
fn rewritten(exprs: Vec<memo::GroupExprRef>) -> TransformResult {
    Ok((exprs, true, false))
}

#[cfg(test)]
#[path = "transformation_rules_test.rs"]
mod transformation_rules_test;

// PushSelDownTableScan 把 Selection 中可形成主键范围的条件并入 TableScan。
/// 把 Selection 可形成主键范围的条件并入 TableScan。
pub struct PushSelDownTableScan {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownTableScan 规则。
pub fn NewRulePushSelDownTableScan() -> Box<dyn Transformation> {
    let scan = pattern::NewPattern(pattern::OperandTableScan, pattern::EngineTiKVOrTiFlash);
    Box::new(PushSelDownTableScan {
        base: BaseRule {
            pattern: pattern::BuildPattern(
                pattern::OperandSelection,
                pattern::EngineTiKVOrTiFlash,
                vec![scan],
            ),
        },
    })
}
impl Transformation for PushSelDownTableScan {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let scan = old.child(0).logical_table_scan();
        let Some(handle_col) = scan
            .HandleCols
            .as_ref()
            .and_then(|columns| columns.GetCol(0))
            .cloned()
        else {
            return unchanged();
        };
        let (access, remained) = ranger::DetachCondsForColumn(
            scan.SCtx().GetRangerCtx(),
            sel.Conditions.clone(),
            handle_col,
        );
        if access.is_empty() {
            return unchanged();
        }
        let mut new_scan = logicalop::LogicalTableScan::from_scan(scan);
        new_scan.AccessConds.extend(access);
        let scan_expr = memo::NewGroupExpr(new_scan);
        if remained.is_empty() {
            return rewritten(vec![scan_expr]);
        }
        // 尚有过滤条件时重新包一层 Selection，Schema 沿用原组。
        let scan_group = memo::NewGroupWithSchema(scan_expr, old.expr().Group.Prop.Schema.clone());
        let mut sel_expr = memo::NewGroupExpr(logicalop::LogicalSelection::new(
            remained,
            sel.SCtx(),
            sel.QueryBlockOffset(),
        ));
        sel_expr.SetChildren(vec![scan_group]);
        rewritten(vec![sel_expr])
    }
}

// PushSelDownIndexScan 合并旧访问条件后重新构造索引范围，不能下推的条件继续留在 Selection。
/// 把 Selection 条件推入 IndexScan 的索引范围。
pub struct PushSelDownIndexScan {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownIndexScan 规则。
pub fn NewRulePushSelDownIndexScan() -> Box<dyn Transformation> {
    let scan = pattern::NewPattern(pattern::OperandIndexScan, pattern::EngineTiKVOnly);
    Box::new(PushSelDownIndexScan {
        base: BaseRule {
            pattern: pattern::BuildPattern(
                pattern::OperandSelection,
                pattern::EngineTiKVOnly,
                vec![scan],
            ),
        },
    })
}
impl Transformation for PushSelDownIndexScan {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let scan = old.child(0).logical_index_scan();
        if scan.IdxCols.is_empty() {
            return unchanged();
        }
        let mut conditions = sel.Conditions.clone();
        conditions.extend(scan.AccessConds.clone());
        let ranges = ranger::DetachCondAndBuildRangeForIndex(
            scan.SCtx().GetRangerCtx(),
            conditions,
            scan.IdxCols.clone(),
            scan.IdxColLens
                .iter()
                .map(|length| *length as i32)
                .collect(),
            scan.SCtx().GetSessionVars().RangeMaxSize,
        )
        .map_err(|error| logicalop::PlannerError(error.to_string()))?;
        let eval_context = scan.SCtx().GetExprCtx().GetEvalCtx();
        if ranges.AccessConds.len() == scan.AccessConds.len()
            && ranges
                .AccessConds
                .iter()
                .zip(&scan.AccessConds)
                .all(|(left, right)| left.Equal(eval_context, right.as_ref()))
        {
            return unchanged();
        }
        let scan_expr = memo::NewGroupExpr(logicalop::LogicalIndexScan::with_ranges(scan, &ranges));
        if ranges.RemainedConds.is_empty() {
            return rewritten(vec![scan_expr]);
        }
        let scan_group =
            memo::NewGroupWithSchema(scan_expr, old.child(0).expr().Group.Prop.Schema.clone());
        let mut sel_expr = memo::NewGroupExpr(logicalop::LogicalSelection::new(
            ranges.RemainedConds,
            sel.SCtx(),
            sel.QueryBlockOffset(),
        ));
        sel_expr.SetChildren(vec![scan_group]);
        rewritten(vec![sel_expr])
    }
}

// PushSelDownTiKVSingleGather 将能由 TiKV 执行的条件放到 Gather 下方，其余条件仍置于 Gather 上方。
/// 把 Selection 下推到 TiKVSingleGather 之下。
pub struct PushSelDownTiKVSingleGather {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownTiKVSingleGather 规则。
pub fn NewRulePushSelDownTiKVSingleGather() -> Box<dyn Transformation> {
    let any = pattern::NewPattern(pattern::OperandAny, pattern::EngineTiKVOrTiFlash);
    let gather = pattern::BuildPattern(
        pattern::OperandTiKVSingleGather,
        pattern::EngineTiDBOnly,
        vec![any],
    );
    Box::new(PushSelDownTiKVSingleGather {
        base: BaseRule {
            pattern: pattern::BuildPattern(
                pattern::OperandSelection,
                pattern::EngineTiDBOnly,
                vec![gather],
            ),
        },
    })
}
impl Transformation for PushSelDownTiKVSingleGather {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let gather = old.child(0).tikv_single_gather();
        let child = old.child(0).child(0).group();
        let (pushed, remained) = expression::PushDownExprs(
            util::GetPushDownCtx(gather.SCtx().plan_context()),
            sel.Conditions.clone(),
            kv::TiKV,
        );
        if pushed.is_empty() {
            return unchanged();
        }
        let pushed_expr = memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(pushed, sel.SCtx(), sel.QueryBlockOffset()),
            vec![child.clone()],
        );
        let pushed_group = memo::NewGroupWithSchema(pushed_expr, child.Prop.Schema.clone())
            .SetEngineType(child.EngineType);
        let gather_expr = memo::NewGroupExprWithChildren(gather.clone(), vec![pushed_group]);
        if remained.is_empty() {
            return rewritten(vec![gather_expr]);
        }
        let gather_group = memo::NewGroupWithSchema(gather_expr, child.Prop.Schema.clone());
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(remained, sel.SCtx(), sel.QueryBlockOffset()),
            vec![gather_group],
        )])
    }
}

// EnumeratePaths 对应 DataSource.Convert2Gathers：每个访问路径产生一个候选，并把叶子引擎标为 TiKV。
/// 为 DataSource 枚举物理访问路径（表扫/索引扫等）。
pub struct EnumeratePaths {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 EnumeratePaths 规则。
pub fn NewRuleEnumeratePaths() -> Box<dyn Transformation> {
    Box::new(EnumeratePaths {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandDataSource, pattern::EngineTiDBOnly),
        },
    })
}
impl Transformation for EnumeratePaths {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let mut result = Vec::new();
        let source = old.data_source();
        for gather in logicalop::DataSource::Convert2Gathers(source.SourceRef()) {
            let expr = memo::Convert2GroupExpr(gather);
            if let Some(child) = expr.borrow().Children.first() {
                child.borrow_mut().SetEngineType(pattern::EngineTiKV);
            }
            result.push(expr);
        }
        rewritten(result)
    }
}

// PushAggDownGather 把完整聚合拆成 TiKV Partial1 与 TiDB Final 两阶段；Distinct 和引擎能力在 Match 中过滤。
/// 把 Aggregation 下推到 Gather 之下以在存储侧预聚合。
pub struct PushAggDownGather {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushAggDownGather 规则。
pub fn NewRulePushAggDownGather() -> Box<dyn Transformation> {
    let gather = pattern::NewPattern(pattern::OperandTiKVSingleGather, pattern::EngineTiDBOnly);
    Box::new(PushAggDownGather {
        base: BaseRule {
            pattern: pattern::BuildPattern(
                pattern::OperandAggregation,
                pattern::EngineTiDBOnly,
                vec![gather],
            ),
        },
    })
}
impl Transformation for PushAggDownGather {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        if expr.expr().HasAppliedRule(self) {
            return false;
        }
        let agg = expr.logical_aggregation();
        if agg
            .AggFuncs
            .iter()
            .any(|f| f.Mode != aggregation::CompleteMode)
        {
            return false;
        }
        let allow_distinct_push_down = agg
            .SCtx()
            .GetSessionVars()
            .GetSystemVar("tidb_opt_distinct_agg_push_down")
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("on"));
        if agg.HasDistinct() && !allow_distinct_push_down {
            return false;
        }
        expr.child(0).expr().Children[0].EngineType == pattern::EngineTiKV
            && physicalop::CheckAggCanPushCop(
                agg.SCtx().plan_context(),
                &agg.AggFuncs,
                &agg.GroupByItems,
                kv::TiKV,
            )
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let schema = old.expr().Group.Prop.Schema.clone();
        let partial_group_by = agg
            .GroupByItems
            .iter()
            .map(|item| item.CloneExpr())
            .collect::<Vec<_>>();
        let mut final_functions = agg
            .AggFuncs
            .iter()
            .map(aggregation::AggFuncDesc::Clone)
            .collect::<Vec<_>>();
        for function in &mut final_functions {
            function.Mode = aggregation::FinalMode;
        }
        let partial_functions = physicalop::RemoveUnnecessaryFirstRow(
            agg.AggFuncs
                .iter()
                .map(aggregation::AggFuncDesc::Clone)
                .collect(),
            &partial_group_by,
        );
        let child = old.child(0).expr().Children[0].clone();
        let mut partial_agg = logicalop::LogicalAggregation::new(
            partial_functions,
            partial_group_by.clone(),
            agg.SCtx(),
            agg.QueryBlockOffset(),
        );
        partial_agg.CopyAggHints(&agg);
        let partial_expr = memo::NewGroupExprWithChildren(partial_agg, vec![child.clone()]);
        let partial_group =
            memo::NewGroupWithSchema(partial_expr, schema.Clone()).SetEngineType(child.EngineType);
        let gather_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                old.child(0).tikv_single_gather().clone(),
                vec![partial_group],
            ),
            schema.Clone(),
        );
        let mut final_agg = logicalop::LogicalAggregation::new(
            final_functions,
            agg.GroupByItems
                .iter()
                .map(|item| item.CloneExpr())
                .collect(),
            agg.SCtx(),
            agg.QueryBlockOffset(),
        );
        final_agg.CopyAggHints(&agg);
        let final_expr = memo::NewGroupExprWithChildren(final_agg, vec![gather_group]);
        final_expr.AddAppliedRule(self);
        // 拆分不一定更优，所以与 Go 一样保留旧完整聚合。
        Ok((vec![final_expr], false, false))
    }
}

/// 构造一元 Pattern：父 Operand 下挂单个子 Pattern。
fn unary_pattern(
    root: pattern::Operand,
    child: pattern::Operand,
    engine: pattern::EngineTypeSet,
) -> pattern::Pattern {
    pattern::BuildPattern(root, engine, vec![pattern::NewPattern(child, engine)])
}

// PushSelDownSort 交换 Selection 与 Sort；排序不改变行内容，因此 Schema 可直接沿用子组。
/// 把 Selection 下推穿过 Sort。
pub struct PushSelDownSort {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownSort 规则。
pub fn NewRulePushSelDownSort() -> Box<dyn Transformation> {
    Box::new(PushSelDownSort {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandSort,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushSelDownSort {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let child = old.child(0).expr().Children[0].clone();
        let sel_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(old.logical_selection().clone(), vec![child.clone()]),
            child.Prop.Schema.clone(),
        );
        rewritten(vec![memo::NewGroupExprWithChildren(
            old.child(0).logical_sort().clone(),
            vec![sel_group],
        )])
    }
}

// PushSelDownProjection 用投影表达式替换过滤列；含 set-var 副作用或替换失败的条件不能下推。
/// 把 Selection 下推穿过 Projection（改写列引用）。
pub struct PushSelDownProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownProjection 规则。
pub fn NewRulePushSelDownProjection() -> Box<dyn Transformation> {
    Box::new(PushSelDownProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandProjection,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushSelDownProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let proj = old.child(0).logical_projection();
        if proj.Exprs.iter().any(expression::HasAssignSetVarFunc) {
            return unchanged();
        }
        let mut pushed = Vec::new();
        let mut remained = Vec::new();
        for condition in &sel.Conditions {
            let (substituted, failed, filter) = expression::ColumnSubstituteImpl(
                sel.SCtx().GetExprCtx(),
                condition.clone(),
                old.child(0)
                    .group()
                    .Prop
                    .Schema
                    .as_deref()
                    .expect("memo group schema"),
                &proj.Exprs,
                true,
            );
            if substituted && !failed && !expression::HasGetSetVarFunc(&filter) {
                pushed.push(filter);
            } else {
                remained.push(condition.clone());
            }
        }
        if pushed.is_empty() {
            return unchanged();
        }
        let child = old.child(0).expr().Children[0].clone();
        let bottom = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                logicalop::LogicalSelection::new(pushed, sel.SCtx(), sel.QueryBlockOffset()),
                vec![child.clone()],
            ),
            child.Prop.Schema.clone(),
        );
        let proj_expr = memo::NewGroupExprWithChildren(proj.clone(), vec![bottom]);
        if remained.is_empty() {
            return rewritten(vec![proj_expr]);
        }
        let proj_group =
            memo::NewGroupWithSchema(proj_expr, old.child(0).group().Prop.Schema.clone());
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(remained, sel.SCtx(), sel.QueryBlockOffset()),
            vec![proj_group],
        )])
    }
}

// PushSelDownAggregation 仅下推只引用 GroupBy 列的标量条件；常量条件上下各保留一份以维持空输入语义。
/// 把与聚合无关的 Selection 下推到 Aggregation 之下。
pub struct PushSelDownAggregation {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownAggregation 规则。
pub fn NewRulePushSelDownAggregation() -> Box<dyn Transformation> {
    Box::new(PushSelDownAggregation {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandAggregation,
                pattern::EngineAll,
            ),
        },
    })
}
impl Transformation for PushSelDownAggregation {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let agg = old.child(0).logical_aggregation();
        let group_by = expression::NewSchema(agg.GetGroupByCols());
        let mut pushed = Vec::new();
        let mut remained = Vec::new();
        for cond in &sel.Conditions {
            if cond.as_constant().is_some() {
                pushed.push(cond.clone());
                remained.push(cond.clone());
            } else if cond.as_scalar_function().is_some()
                && expression::ExtractColumns(cond.as_ref())
                    .iter()
                    .all(|c| group_by.Contains(c))
            {
                pushed.push(cond.clone());
            } else {
                remained.push(cond.clone());
            }
        }
        if pushed.is_empty() {
            return unchanged();
        }
        let child = old.child(0).expr().Children[0].clone();
        let pushed_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                logicalop::LogicalSelection::new(pushed, sel.SCtx(), sel.QueryBlockOffset()),
                vec![child.clone()],
            ),
            child.Prop.Schema.clone(),
        );
        let agg_expr = memo::NewGroupExprWithChildren(agg.clone(), vec![pushed_group]);
        if remained.is_empty() {
            return rewritten(vec![agg_expr]);
        }
        let agg_group =
            memo::NewGroupWithSchema(agg_expr, old.child(0).group().Prop.Schema.clone());
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(remained, sel.SCtx(), sel.QueryBlockOffset()),
            vec![agg_group],
        )])
    }
}

// PushSelDownWindow 只把完全来自 PartitionBy 列的条件下推，避免改变窗口分区内的计算结果。
/// 把与窗口无关的 Selection 下推到 Window 之下。
pub struct PushSelDownWindow {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownWindow 规则。
pub fn NewRulePushSelDownWindow() -> Box<dyn Transformation> {
    Box::new(PushSelDownWindow {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandWindow,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushSelDownWindow {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let window = old.child(0).logical_window();
        let partition_schema = expression::NewSchema(window.GetPartitionByCols());
        let (pushed, remained): (Vec<_>, Vec<_>) = sel
            .Conditions
            .iter()
            .cloned()
            .partition(|cond| expression::ExprFromSchema(cond.as_ref(), &partition_schema));
        if pushed.is_empty() {
            return unchanged();
        }
        let child = old.child(0).expr().Children[0].clone();
        let bottom = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                logicalop::LogicalSelection::new(pushed, sel.SCtx(), sel.QueryBlockOffset()),
                vec![child.clone()],
            ),
            child.Prop.Schema.clone(),
        );
        let window_expr = memo::NewGroupExprWithChildren(window.clone(), vec![bottom]);
        if remained.is_empty() {
            return rewritten(vec![window_expr]);
        }
        let window_group =
            memo::NewGroupWithSchema(window_expr, old.child(0).group().Prop.Schema.clone());
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(remained, sel.SCtx(), sel.QueryBlockOffset()),
            vec![window_group],
        )])
    }
}

// TransformLimitToTopN 把 Limit->Sort 合成携带相同 ByItems/Offset/Count 的 TopN。
/// Limit 叠在 Sort 上时改写为 TopN。
pub struct TransformLimitToTopN {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 TransformLimitToTopN 规则。
pub fn NewRuleTransformLimitToTopN() -> Box<dyn Transformation> {
    Box::new(TransformLimitToTopN {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandSort,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for TransformLimitToTopN {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let limit = old.logical_limit();
        let sort = old.child(0).logical_sort();
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalTopN::new(
                sort.ByItems.clone(),
                limit.Offset,
                limit.Count,
                limit.SCtx(),
                limit.QueryBlockOffset(),
            ),
            old.child(0).expr().Children.clone(),
        )])
    }
}

// PushLimitDownProjection 交换 Limit 与无 set-var 副作用的 Projection。
/// 把 Limit 下推穿过 Projection。
pub struct PushLimitDownProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushLimitDownProjection 规则。
pub fn NewRulePushLimitDownProjection() -> Box<dyn Transformation> {
    Box::new(PushLimitDownProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandProjection,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushLimitDownProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr
            .child(0)
            .logical_projection()
            .Exprs
            .iter()
            .any(expression::HasAssignSetVarFunc)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let child = old.child(0).expr().Children[0].clone();
        let limit_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(old.logical_limit().clone(), vec![child.clone()]),
            child.Prop.Schema.clone(),
        );
        rewritten(vec![memo::NewGroupExprWithChildren(
            old.child(0).logical_projection().clone(),
            vec![limit_group],
        )])
    }
}

// PushLimitDownUnionAll 给每个 UnionAll 分支增加 Count=Offset+Count 的局部 Limit，并保留顶层 Limit。
/// 把 Limit 下推到 UnionAll 各分支。
pub struct PushLimitDownUnionAll {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushLimitDownUnionAll 规则。
pub fn NewRulePushLimitDownUnionAll() -> Box<dyn Transformation> {
    Box::new(PushLimitDownUnionAll {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandUnionAll,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushLimitDownUnionAll {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let limit = old.logical_limit();
        let union = old.child(0).logical_union_all();
        let children = old
            .child(0)
            .expr()
            .Children
            .iter()
            .map(|child| {
                let partial = logicalop::LogicalLimit::new(
                    0,
                    limit.Count.wrapping_add(limit.Offset),
                    limit.SCtx(),
                    limit.QueryBlockOffset(),
                );
                memo::NewGroupWithSchema(
                    memo::NewGroupExprWithChildren(partial, vec![child.clone()]),
                    child.Prop.Schema.clone(),
                )
            })
            .collect();
        let union_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(union.clone(), children),
            old.child(0)
                .group()
                .Prop
                .Schema
                .as_ref()
                .map(|schema| schema.as_ref().Clone()),
        );
        let mut final_expr = memo::NewGroupExprWithChildren(limit.clone(), vec![union_group]);
        final_expr.AddAppliedRule(self);
        rewritten(vec![final_expr])
    }
}

// PushDownJoin 对应 Go pushDownJoin：按 JoinType 分解谓词，并为外连接保留空值扩展侧的条件。
/// Join 条件下推到左右子树的辅助实现体。
pub struct PushDownJoin;
impl PushDownJoin {
    fn predicate_push_down(
        &self,
        predicates: Vec<expression::ExprBox>,
        join: &mut logicalop::LogicalJoin,
        left: &expression::Schema,
        right: &expression::Schema,
    ) -> (
        Vec<expression::ExprBox>,
        Vec<expression::ExprBox>,
        Vec<expression::ExprBox>,
        bool,
    ) {
        match join.JoinType {
            base::SemiJoin | base::InnerJoin => {
                let mut all = join.EqualConditions.clone();
                all.extend(join.NAEQConditions.clone());
                all.extend(join.LeftConditions.clone());
                all.extend(join.RightConditions.clone());
                all.extend(join.OtherConditions.clone());
                all.extend(predicates);
                all = expression::PropagateConstant(join.SCtx().GetExprCtx(), all);
                if logicalop::Conds2TableDual(&all) {
                    return (vec![], vec![], vec![], true);
                }
                let mut equal = Vec::new();
                let mut left_cond = Vec::new();
                let mut right_cond = Vec::new();
                let mut other = Vec::new();
                for condition in all {
                    let columns = expression::ExtractColumns(condition.as_ref());
                    let is_cross_equality = condition
                        .as_scalar_function()
                        .is_some_and(|function| function.FuncName.L == ast::EQ)
                        && columns.iter().any(|column| left.Contains(column))
                        && columns.iter().any(|column| right.Contains(column));
                    if is_cross_equality {
                        equal.push(condition);
                    } else if expression::ExprFromSchema(condition.as_ref(), left) {
                        left_cond.push(condition);
                    } else if expression::ExprFromSchema(condition.as_ref(), right) {
                        right_cond.push(condition);
                    } else {
                        other.push(condition);
                    }
                }
                join.EqualConditions = equal;
                join.NAEQConditions.clear();
                join.LeftConditions.clear();
                join.RightConditions.clear();
                join.OtherConditions = other;
                (
                    expression::RemoveDupExprs(left_cond),
                    expression::RemoveDupExprs(right_cond),
                    vec![],
                    false,
                )
            }
            base::LeftOuterJoin
            | base::LeftOuterSemiJoin
            | base::AntiLeftOuterSemiJoin
            | base::RightOuterJoin => {
                let mut left_cond = Vec::new();
                let mut right_cond = Vec::new();
                let mut remain = Vec::new();
                for predicate in predicates {
                    if matches!(
                        join.JoinType,
                        base::LeftOuterJoin | base::LeftOuterSemiJoin | base::AntiLeftOuterSemiJoin
                    ) && expression::ExprFromSchema(predicate.as_ref(), left)
                    {
                        left_cond.push(predicate);
                    } else if join.JoinType == base::RightOuterJoin
                        && expression::ExprFromSchema(predicate.as_ref(), right)
                    {
                        right_cond.push(predicate);
                    } else {
                        remain.push(predicate);
                    }
                }
                (left_cond, right_cond, remain, false)
            }
            _ => (vec![], vec![], predicates, false),
        }
    }
}

/// 为 Join 子节点构造带 Selection 的新 Group。
fn build_child_selection_group(
    ctx: Option<&base::ContextRef>,
    qb_offset: i32,
    conditions: Vec<expression::ExprBox>,
    child: memo::Group,
) -> memo::Group {
    if conditions.is_empty() {
        return child;
    }
    let schema = child.Prop.Schema.clone();
    memo::NewGroupWithSchema(
        memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(conditions, ctx, qb_offset),
            vec![child],
        ),
        schema,
    )
}

// PushSelDownJoin 将 Selection 条件拆到 Join 两侧，常量假条件直接把整个结果改写为 TableDual。
/// 把 Selection 谓词按 Join 类型下推到子节点。
pub struct PushSelDownJoin {
    base: BaseRule,
    helper: PushDownJoin,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownJoin 规则。
pub fn NewRulePushSelDownJoin() -> Box<dyn Transformation> {
    Box::new(PushSelDownJoin {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandJoin,
                pattern::EngineTiDBOnly,
            ),
        },
        helper: PushDownJoin,
    })
}
impl Transformation for PushSelDownJoin {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let mut join = old.child(0).logical_join().LogicalJoinShallowRef();
        let mut left = old.child(0).expr().Children[0].clone();
        let mut right = old.child(0).expr().Children[1].clone();
        let (lc, rc, remain, dual) = self.helper.predicate_push_down(
            sel.Conditions.clone(),
            &mut join,
            left.Prop.Schema.as_deref().expect("memo group schema"),
            right.Prop.Schema.as_deref().expect("memo group schema"),
        );
        if dual {
            let mut dual_plan =
                logicalop::LogicalTableDual::new(0, sel.SCtx(), sel.QueryBlockOffset());
            dual_plan.SetSchema(old.expr().Schema());
            return Ok((vec![memo::NewGroupExpr(dual_plan)], true, true));
        }
        left = build_child_selection_group(sel.SCtx(), sel.QueryBlockOffset(), lc, left);
        right = build_child_selection_group(sel.SCtx(), sel.QueryBlockOffset(), rc, right);
        let join_expr = memo::NewGroupExprWithChildren(join, vec![left, right]);
        if remain.is_empty() {
            return rewritten(vec![join_expr]);
        }
        let join_group =
            memo::NewGroupWithSchema(join_expr, old.child(0).group().Prop.Schema.clone());
        let mut sel_expr = memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(remain, sel.SCtx(), sel.QueryBlockOffset()),
            vec![join_group],
        );
        sel_expr.AddAppliedRule(self);
        rewritten(vec![sel_expr])
    }
}

// TransformJoinCondToSel 在没有父 Selection 时也调用同一谓词拆分器，把 Join 自身条件下沉到孩子。
/// 把 Join 条件转为子节点上的 Selection。
pub struct TransformJoinCondToSel {
    base: BaseRule,
    helper: PushDownJoin,
}
#[allow(non_snake_case)]
/// 构造 TransformJoinCondToSel 规则。
pub fn NewRuleTransformJoinCondToSel() -> Box<dyn Transformation> {
    Box::new(TransformJoinCondToSel {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandJoin, pattern::EngineTiDBOnly),
        },
        helper: PushDownJoin,
    })
}
impl Transformation for TransformJoinCondToSel {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self) && expr.logical_join().has_conditions()
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let mut join = old.logical_join().LogicalJoinShallowRef();
        let mut left = old.expr().Children[0].clone();
        let mut right = old.expr().Children[1].clone();
        let (lc, rc, _, dual) = self.helper.predicate_push_down(
            vec![],
            &mut join,
            left.Prop.Schema.as_deref().expect("memo group schema"),
            right.Prop.Schema.as_deref().expect("memo group schema"),
        );
        if dual {
            let mut dual_plan =
                logicalop::LogicalTableDual::new(0, join.SCtx(), join.QueryBlockOffset());
            dual_plan.SetSchema(old.expr().Schema());
            return Ok((vec![memo::NewGroupExpr(dual_plan)], true, true));
        }
        left = build_child_selection_group(join.SCtx(), join.QueryBlockOffset(), lc, left);
        right = build_child_selection_group(join.SCtx(), join.QueryBlockOffset(), rc, right);
        let mut expr = memo::NewGroupExprWithChildren(join, vec![left, right]);
        expr.AddAppliedRule(self);
        rewritten(vec![expr])
    }
}

// PushSelDownUnionAll 为每个 UnionAll 分支复制 Selection，输出列位置保持不变。
/// 把 Selection 下推到 UnionAll 各分支。
pub struct PushSelDownUnionAll {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushSelDownUnionAll 规则。
pub fn NewRulePushSelDownUnionAll() -> Box<dyn Transformation> {
    Box::new(PushSelDownUnionAll {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandUnionAll,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushSelDownUnionAll {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let sel = old.logical_selection();
        let union = old.child(0).logical_union_all();
        let children = old
            .child(0)
            .expr()
            .Children
            .iter()
            .map(|child| {
                memo::NewGroupWithSchema(
                    memo::NewGroupExprWithChildren(sel.clone(), vec![child.clone()]),
                    child.Prop.Schema.clone(),
                )
            })
            .collect();
        rewritten(vec![memo::NewGroupExprWithChildren(
            union.clone(),
            children,
        )])
    }
}

// EliminateProjection 仅在父子 Schema 列逐一相同时提升子组全部等价表达式。
/// 消除与子节点 schema 完全一致的 Projection。
pub struct EliminateProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 EliminateProjection 规则。
pub fn NewRuleEliminateProjection() -> Box<dyn Transformation> {
    Box::new(EliminateProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandProjection,
                pattern::OperandAny,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for EliminateProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let child = old.child(0).group();
        let child_schema = child.Prop.Schema.as_deref().expect("memo group schema");
        let parent = old.expr();
        let parent_schema = parent
            .Group
            .Prop
            .Schema
            .as_deref()
            .expect("memo group schema");
        if child_schema.Columns.len() != parent_schema.Columns.len()
            || !child_schema
                .Columns
                .iter()
                .zip(&parent_schema.Columns)
                .all(|(a, b)| a.EqualColumn(b))
        {
            return unchanged();
        }
        let promoted = child
            .Equivalents
            .iter()
            .map(|expression| {
                let children = expression.borrow().Children.clone();
                memo::NewGroupExprWithChildren(memo::PlanNodeHandle(expression.clone()), children)
            })
            .collect();
        rewritten(promoted)
    }
}

// MergeAdjacentProjection 用下层 Projection 的表达式替换上层列引用；含副作用时保持原树。
/// 合并相邻两层 Projection。
pub struct MergeAdjacentProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAdjacentProjection 规则。
pub fn NewRuleMergeAdjacentProjection() -> Box<dyn Transformation> {
    Box::new(MergeAdjacentProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandProjection,
                pattern::OperandProjection,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for MergeAdjacentProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let upper = old.logical_projection();
        let lower = old.child(0).logical_projection();
        if expression::ExprsHasSideEffects(&lower.Exprs) {
            return unchanged();
        }
        let child_group = old.child(0).group();
        let schema = child_group
            .Prop
            .Schema
            .as_deref()
            .expect("memo group schema");
        let exprs = upper
            .Exprs
            .iter()
            .map(|expr| ruleutil::ReplaceColumnOfExpr(expr.CloneExpr(), &lower.Exprs, schema))
            .collect();
        let mut projection =
            logicalop::LogicalProjection::new(exprs, upper.SCtx(), upper.QueryBlockOffset());
        projection.SetSchema(
            old.expr()
                .Group
                .Prop
                .Schema
                .as_deref()
                .map(expression::Schema::Clone)
                .expect("memo group schema"),
        );
        rewritten(vec![memo::NewGroupExprWithChildren(
            projection,
            old.child(0).expr().Children.clone(),
        )])
    }
}

/// 把 TopN 推到外连接的外表一侧。
fn push_top_n_to_outer_child(top_n: &logicalop::LogicalTopN, outer: memo::Group) -> memo::Group {
    // 排序表达式只要引用内表列，就不能安全地下推到外表一侧。
    let outer_schema = outer.Prop.Schema.as_deref().expect("memo group schema");
    if top_n
        .ByItems
        .iter()
        .flat_map(|item| expression::ExtractColumns(item.Expr.as_ref()))
        .any(|col| !outer_schema.Contains(&col))
    {
        return outer;
    }
    let schema = outer_schema.Clone();
    let partial = logicalop::LogicalTopN::new(
        top_n.ByItems.iter().map(|item| item.Clone()).collect(),
        0,
        top_n.Count.wrapping_add(top_n.Offset),
        top_n.SCtx(),
        top_n.QueryBlockOffset(),
    );
    memo::NewGroupWithSchema(memo::NewGroupExprWithChildren(partial, vec![outer]), schema)
}

// PushTopNDownOuterJoin 只向外连接的保留侧下推局部 TopN，并用 applied-rule 防止重复包裹。
/// 把 TopN 下推到 Outer Join 的外表。
pub struct PushTopNDownOuterJoin {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushTopNDownOuterJoin 规则。
pub fn NewRulePushTopNDownOuterJoin() -> Box<dyn Transformation> {
    Box::new(PushTopNDownOuterJoin {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandTopN,
                pattern::OperandJoin,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushTopNDownOuterJoin {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self) && expr.child(0).logical_join().JoinType.is_outer_join()
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let top_n = old.logical_top_n();
        let join = old.child(0).logical_join();
        let mut left = old.child(0).expr().Children[0].clone();
        let mut right = old.child(0).expr().Children[1].clone();
        match join.JoinType {
            base::LeftOuterJoin | base::LeftOuterSemiJoin | base::AntiLeftOuterSemiJoin => {
                left = push_top_n_to_outer_child(&*top_n, left)
            }
            base::RightOuterJoin => right = push_top_n_to_outer_child(&*top_n, right),
            _ => return unchanged(),
        }
        let join_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(join.clone(), vec![left, right]),
            old.child(0)
                .group()
                .Prop
                .Schema
                .as_ref()
                .map(|schema| schema.as_ref().Clone()),
        );
        let mut result = memo::NewGroupExprWithChildren(top_n.clone(), vec![join_group]);
        result.AddAppliedRule(self);
        rewritten(vec![result])
    }
}

// PushTopNDownProjection 先用 Projection 表达式替换 ByItems，再把 TopN 放到 Projection 下方。
/// 把 TopN 下推穿过 Projection。
pub struct PushTopNDownProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushTopNDownProjection 规则。
pub fn NewRulePushTopNDownProjection() -> Box<dyn Transformation> {
    Box::new(PushTopNDownProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandTopN,
                pattern::OperandProjection,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushTopNDownProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr
            .child(0)
            .logical_projection()
            .Exprs
            .iter()
            .any(expression::HasAssignSetVarFunc)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let top_n = old.logical_top_n();
        let projection = old.child(0).logical_projection();
        let by_items = top_n
            .ByItems
            .iter()
            .map(|item| {
                let expr = expression::ColumnSubstitute(
                    top_n.SCtx().GetExprCtx(),
                    item.Expr.clone(),
                    old.child(0)
                        .group()
                        .Prop
                        .Schema
                        .as_deref()
                        .expect("memo group schema"),
                    &projection.Exprs,
                );
                util::ByItems {
                    Expr: expr,
                    Desc: item.Desc,
                }
            })
            .collect();
        let child = old.child(0).expr().Children[0].clone();
        let top_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                logicalop::LogicalTopN::new(
                    by_items,
                    top_n.Offset,
                    top_n.Count,
                    top_n.SCtx(),
                    top_n.QueryBlockOffset(),
                ),
                vec![child.clone()],
            ),
            child.Prop.Schema.clone(),
        );
        rewritten(vec![memo::NewGroupExprWithChildren(
            projection.clone(),
            vec![top_group],
        )])
    }
}

// PushTopNDownUnionAll 给每个分支增加 Offset=0、Count=原 Offset+Count 的局部 TopN。
/// 把 TopN 下推到 UnionAll 各分支。
pub struct PushTopNDownUnionAll {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushTopNDownUnionAll 规则。
pub fn NewRulePushTopNDownUnionAll() -> Box<dyn Transformation> {
    Box::new(PushTopNDownUnionAll {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandTopN,
                pattern::OperandUnionAll,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushTopNDownUnionAll {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let top_n = old.logical_top_n();
        let union = old.child(0).logical_union_all();
        let children = old
            .child(0)
            .expr()
            .Children
            .iter()
            .map(|child| {
                let partial = logicalop::LogicalTopN::new(
                    top_n.ByItems.clone(),
                    0,
                    top_n.Count.wrapping_add(top_n.Offset),
                    top_n.SCtx(),
                    top_n.QueryBlockOffset(),
                );
                memo::NewGroupWithSchema(
                    memo::NewGroupExprWithChildren(partial, vec![child.clone()]),
                    child.Prop.Schema.clone(),
                )
            })
            .collect();
        let union_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(union.clone(), children),
            union.Schema(),
        );
        let mut result = memo::NewGroupExprWithChildren(top_n.clone(), vec![union_group]);
        result.AddAppliedRule(self);
        rewritten(vec![result])
    }
}

// PushTopNDownTiKVSingleGather 构造 TiDB Final TopN -> Gather -> TiKV Partial TopN。
/// 把 TopN 下推到 TiKVSingleGather 之下。
pub struct PushTopNDownTiKVSingleGather {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushTopNDownTiKVSingleGather 规则。
pub fn NewRulePushTopNDownTiKVSingleGather() -> Box<dyn Transformation> {
    Box::new(PushTopNDownTiKVSingleGather {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandTopN,
                pattern::OperandTiKVSingleGather,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushTopNDownTiKVSingleGather {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let top_n = old.logical_top_n();
        let gather = old.child(0).tikv_single_gather();
        let child = old.child(0).expr().Children[0].clone();
        let schema = old
            .child(0)
            .group()
            .Prop
            .Schema
            .as_deref()
            .map(expression::Schema::Clone)
            .expect("memo group schema");
        let partial = logicalop::LogicalTopN::new(
            top_n.ByItems.clone(),
            0,
            top_n.Count.wrapping_add(top_n.Offset),
            top_n.SCtx(),
            top_n.QueryBlockOffset(),
        );
        let partial_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(partial, vec![child.clone()]),
            schema.clone(),
        )
        .SetEngineType(child.EngineType);
        let gather_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(gather.clone(), vec![partial_group]),
            schema,
        );
        let mut result = memo::NewGroupExprWithChildren(top_n.clone(), vec![gather_group]);
        result.AddAppliedRule(self);
        rewritten(vec![result])
    }
}

// MergeAdjacentTopN 要求父排序项是子排序项前缀；区间无交集时直接产生空 TableDual。
/// 合并相邻 TopN（取更紧的 offset/count）。
pub struct MergeAdjacentTopN {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAdjacentTopN 规则。
pub fn NewRuleMergeAdjacentTopN() -> Box<dyn Transformation> {
    Box::new(MergeAdjacentTopN {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandTopN,
                pattern::OperandTopN,
                pattern::EngineAll,
            ),
        },
    })
}
impl Transformation for MergeAdjacentTopN {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        let parent = expr.logical_top_n();
        let child = expr.child(0).logical_top_n();
        child.ByItems.len() >= parent.ByItems.len()
            && parent
                .ByItems
                .iter()
                .zip(&child.ByItems)
                .all(|(a, b)| a.Equal(parent.SCtx().GetExprCtx().GetEvalCtx(), b))
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let parent = old.logical_top_n();
        let child = old.child(0).logical_top_n();
        if child.Count <= parent.Offset {
            let mut dual =
                logicalop::LogicalTableDual::new(0, child.SCtx(), child.QueryBlockOffset());
            dual.SetSchema(old.expr().Schema());
            return Ok((vec![memo::NewGroupExpr(dual)], true, true));
        }
        let count = (child.Count - parent.Offset).min(parent.Count);
        let offset = child.Offset.wrapping_add(parent.Offset);
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalTopN::new(
                child.ByItems.clone(),
                offset,
                count,
                child.SCtx(),
                child.QueryBlockOffset(),
            ),
            old.child(0).expr().Children.clone(),
        )])
    }
}

// MergeAggregationProjection 把聚合参数与 GroupBy 表达式中的投影列替换为下层原始表达式。
/// 把 Projection 并入其上的 Aggregation。
pub struct MergeAggregationProjection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAggregationProjection 规则。
pub fn NewRuleMergeAggregationProjection() -> Box<dyn Transformation> {
    Box::new(MergeAggregationProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandAggregation,
                pattern::OperandProjection,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for MergeAggregationProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expression::ExprsHasSideEffects(&expr.child(0).logical_projection().Exprs)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let projection = old.child(0).logical_projection();
        let schema = old.child(0).expr().Schema();
        let group_by = agg
            .GroupByItems
            .iter()
            .map(|item| {
                expression::ColumnSubstitute(
                    agg.SCtx().GetExprCtx(),
                    item.clone(),
                    &schema,
                    &projection.Exprs,
                )
            })
            .collect();
        let funcs = agg
            .AggFuncs
            .iter()
            .map(|func| {
                let mut copy = func.Clone();
                copy.Args = func
                    .Args
                    .iter()
                    .map(|arg| {
                        expression::ColumnSubstitute(
                            agg.SCtx().GetExprCtx(),
                            arg.clone(),
                            &schema,
                            &projection.Exprs,
                        )
                    })
                    .collect();
                copy
            })
            .collect();
        let merged =
            logicalop::LogicalAggregation::new(funcs, group_by, agg.SCtx(), agg.QueryBlockOffset());
        Ok((
            vec![memo::NewGroupExprWithChildren(
                merged,
                old.child(0).expr().Children.clone(),
            )],
            false,
            false,
        ))
    }
}

// EliminateSingleMaxMin 将无 GroupBy 的单个 MAX/MIN 改写为非空过滤加 Top1，常量参数则使用 Limit1。
/// 单列 MAX/MIN 可改写为更廉价计划时消除聚合形态。
pub struct EliminateSingleMaxMin {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 EliminateSingleMaxMin 规则。
pub fn NewRuleEliminateSingleMaxMin() -> Box<dyn Transformation> {
    Box::new(EliminateSingleMaxMin {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandAggregation,
                pattern::OperandAny,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for EliminateSingleMaxMin {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        let agg = expr.logical_aggregation();
        !expr.expr().HasAppliedRule(self)
            && agg.IsCompleteModeAgg()
            && agg.GroupByItems.is_empty()
            && agg.AggFuncs.len() == 1
            && matches!(
                agg.AggFuncs[0].Name.as_str(),
                ast::AggFuncMax | ast::AggFuncMin
            )
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let func = &agg.AggFuncs[0];
        let mut child = old.expr().Children[0].clone();
        if !expression::ExtractColumns(func.Args[0].as_ref()).is_empty() {
            if !mysql::HasNotNullFlag(
                func.Args[0]
                    .GetType(agg.SCtx().GetExprCtx().GetEvalCtx())
                    .GetFlag(),
            ) {
                let not_null = expression::not(
                    expression::is_null(func.Args[0].clone(), agg.SCtx().GetExprCtx()),
                    agg.SCtx().GetExprCtx(),
                );
                let schema = child.Prop.Schema.clone();
                child = memo::NewGroupWithSchema(
                    memo::NewGroupExprWithChildren(
                        logicalop::LogicalSelection::new(
                            vec![not_null],
                            agg.SCtx(),
                            agg.QueryBlockOffset(),
                        ),
                        vec![child],
                    ),
                    schema,
                );
            }
            let schema = child.Prop.Schema.clone();
            let top = logicalop::LogicalTopN::new(
                vec![util::ByItems {
                    Expr: func.Args[0].clone(),
                    Desc: func.Name == ast::AggFuncMax,
                }],
                0,
                1,
                agg.SCtx(),
                agg.QueryBlockOffset(),
            );
            child =
                memo::NewGroupWithSchema(memo::NewGroupExprWithChildren(top, vec![child]), schema);
        } else {
            let schema = child.Prop.Schema.clone();
            child = memo::NewGroupWithSchema(
                memo::NewGroupExprWithChildren(
                    logicalop::LogicalLimit::new(0, 1, agg.SCtx(), agg.QueryBlockOffset()),
                    vec![child],
                ),
                schema,
            );
        }
        let mut result = memo::NewGroupExprWithChildren(agg.clone(), vec![child]);
        result.AddAppliedRule(self);
        Ok((vec![result], false, false))
    }
}

// MergeAdjacentSelection 按父后子顺序拼接条件，不在该规则中额外做常量化简。
/// 合并相邻 Selection 谓词。
pub struct MergeAdjacentSelection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAdjacentSelection 规则。
pub fn NewRuleMergeAdjacentSelection() -> Box<dyn Transformation> {
    Box::new(MergeAdjacentSelection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandSelection,
                pattern::OperandSelection,
                pattern::EngineAll,
            ),
        },
    })
}
impl Transformation for MergeAdjacentSelection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let parent = old.logical_selection();
        let mut conditions = parent.Conditions.clone();
        conditions.extend(old.child(0).logical_selection().Conditions.clone());
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalSelection::new(conditions, parent.SCtx(), parent.QueryBlockOffset()),
            old.child(0).expr().Children.clone(),
        )])
    }
}

// MergeAdjacentLimit 计算两层 Limit 区间交集；无交集时 eraseAll，避免保留无意义候选。
/// 合并相邻 Limit：求窗口交集，空则变空 TableDual。
pub struct MergeAdjacentLimit {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAdjacentLimit 规则。
pub fn NewRuleMergeAdjacentLimit() -> Box<dyn Transformation> {
    Box::new(MergeAdjacentLimit {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandLimit,
                pattern::EngineAll,
            ),
        },
    })
}
impl Transformation for MergeAdjacentLimit {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let parent = old.logical_limit();
        let child = old.child(0).logical_limit();
        if child.Count <= parent.Offset {
            let mut dual =
                logicalop::LogicalTableDual::new(0, child.SCtx(), child.QueryBlockOffset());
            dual.SetSchema(old.expr().Schema());
            return Ok((vec![memo::NewGroupExpr(dual)], true, true));
        }
        let offset = child.Offset.wrapping_add(parent.Offset);
        let count = (child.Count - parent.Offset).min(parent.Count);
        rewritten(vec![memo::NewGroupExprWithChildren(
            logicalop::LogicalLimit::new(offset, count, parent.SCtx(), parent.QueryBlockOffset()),
            old.child(0).expr().Children.clone(),
        )])
    }
}

// TransformLimitToTableDual 专门处理 Count=0，直接生成同 Schema 的空表。
/// Count=0 的 Limit 直接变为空 TableDual。
pub struct TransformLimitToTableDual {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 TransformLimitToTableDual 规则。
pub fn NewRuleTransformLimitToTableDual() -> Box<dyn Transformation> {
    Box::new(TransformLimitToTableDual {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandLimit, pattern::EngineAll),
        },
    })
}
impl Transformation for TransformLimitToTableDual {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        expr.logical_limit().Count == 0
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let limit = old.logical_limit();
        let mut dual = logicalop::LogicalTableDual::new(0, limit.SCtx(), limit.QueryBlockOffset());
        dual.SetSchema(old.expr().Schema());
        Ok((vec![memo::NewGroupExpr(dual)], true, true))
    }
}

/// 把 Limit 推到外连接外表一侧。
fn push_limit_to_outer_child(limit: &logicalop::LogicalLimit, outer: memo::Group) -> memo::Group {
    let schema = outer
        .Prop
        .Schema
        .as_deref()
        .map(expression::Schema::Clone)
        .expect("memo group schema");
    let partial = logicalop::LogicalLimit::new(
        0,
        limit.Count.wrapping_add(limit.Offset),
        limit.SCtx(),
        limit.QueryBlockOffset(),
    );
    memo::NewGroupWithSchema(memo::NewGroupExprWithChildren(partial, vec![outer]), schema)
}

// PushLimitDownOuterJoin 只把扩大后的 Limit 下推到外连接保留侧，顶层 Limit 仍负责 Offset。
/// 把 Limit 下推到 Outer Join 的外表。
pub struct PushLimitDownOuterJoin {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushLimitDownOuterJoin 规则。
pub fn NewRulePushLimitDownOuterJoin() -> Box<dyn Transformation> {
    Box::new(PushLimitDownOuterJoin {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandJoin,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushLimitDownOuterJoin {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self) && expr.child(0).logical_join().JoinType.is_outer_join()
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let limit = old.logical_limit();
        let join = old.child(0).logical_join();
        let mut left = old.child(0).expr().Children[0].clone();
        let mut right = old.child(0).expr().Children[1].clone();
        match join.JoinType {
            base::LeftOuterJoin | base::LeftOuterSemiJoin | base::AntiLeftOuterSemiJoin => {
                left = push_limit_to_outer_child(&*limit, left)
            }
            base::RightOuterJoin => right = push_limit_to_outer_child(&*limit, right),
            _ => return unchanged(),
        }
        let join_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(join.clone(), vec![left, right]),
            old.child(0)
                .group()
                .Prop
                .Schema
                .as_ref()
                .map(|schema| schema.as_ref().Clone()),
        );
        let mut result = memo::NewGroupExprWithChildren(limit.clone(), vec![join_group]);
        result.AddAppliedRule(self);
        rewritten(vec![result])
    }
}

// PushLimitDownTiKVSingleGather 构造 Final Limit -> Gather -> Partial Limit，Partial 的 Count 含原 Offset。
/// 把 Limit 下推到 TiKVSingleGather 之下。
pub struct PushLimitDownTiKVSingleGather {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PushLimitDownTiKVSingleGather 规则。
pub fn NewRulePushLimitDownTiKVSingleGather() -> Box<dyn Transformation> {
    Box::new(PushLimitDownTiKVSingleGather {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandLimit,
                pattern::OperandTiKVSingleGather,
                pattern::EngineTiDBOnly,
            ),
        },
    })
}
impl Transformation for PushLimitDownTiKVSingleGather {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        !expr.expr().HasAppliedRule(self)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let limit = old.logical_limit();
        let gather = old.child(0).tikv_single_gather();
        let child = old.child(0).expr().Children[0].clone();
        let schema = old
            .child(0)
            .group()
            .Prop
            .Schema
            .as_deref()
            .map(expression::Schema::Clone)
            .expect("memo group schema");
        let partial = logicalop::LogicalLimit::new(
            0,
            limit.Count.wrapping_add(limit.Offset),
            limit.SCtx(),
            limit.QueryBlockOffset(),
        );
        let partial_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(partial, vec![child.clone()]),
            schema.clone(),
        )
        .SetEngineType(child.EngineType);
        let gather_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(gather.clone(), vec![partial_group]),
            schema,
        );
        let mut result = memo::NewGroupExprWithChildren(limit.clone(), vec![gather_group]);
        result.AddAppliedRule(self);
        rewritten(vec![result])
    }
}

// OuterJoinEliminator 汇总两个消除规则共享的外/内侧定位与唯一键判定逻辑。
/// 判断 Outer Join 是否可被消除的辅助逻辑。
pub struct OuterJoinEliminator;
impl OuterJoinEliminator {
    fn prepare(
        &self,
        join_expr: &memo::ExprView,
    ) -> Option<(usize, memo::Group, memo::Group, intset::FastIntSet)> {
        let join = join_expr
            .ExprNode
            .as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .expect("join rule pattern must contain LogicalJoin");
        let inner = match join.JoinType {
            base::LeftOuterJoin => 1,
            base::RightOuterJoin => 0,
            _ => return None,
        };
        let outer_group = join_expr.Children[1 ^ inner].clone();
        let inner_group = join_expr.Children[inner].clone();
        let mut ids = intset::NewFastIntSet(Vec::new());
        for col in &outer_group
            .Prop
            .Schema
            .as_deref()
            .expect("memo group schema")
            .Columns
        {
            ids.Insert(col.UniqueID as i32);
        }
        Some((inner, outer_group, inner_group, ids))
    }
    fn inner_keys_contain_unique_key(
        &self,
        inner: &mut memo::Group,
        join_keys: &expression::Schema,
        null_eq: &intset::FastIntSet,
    ) -> logicalop::Result<bool> {
        inner.BuildKeyInfo();
        let inner_schema = inner.Prop.Schema.as_deref().expect("memo group schema");
        if inner_schema
            .PKOrUK
            .iter()
            .any(|key| key.iter().all(|col| join_keys.Contains(col)))
        {
            return Ok(true);
        }
        // NullableUK 只有在对应连接键不是 NULL-safe equal 时才能保证至多匹配一行。
        Ok(inner_schema.NullableUK.iter().any(|key| {
            key.iter()
                .all(|col| join_keys.Contains(col) && !null_eq.Has(col.UniqueID as i32))
        }))
    }
}

/// 收集内表上可用于空值拒绝的等值键列。
fn inner_null_eq_keys(join: &logicalop::LogicalJoin, inner_index: usize) -> intset::FastIntSet {
    let mut keys = intset::NewFastIntSet(Vec::new());
    for equal in &join.EqualConditions {
        if let Some(equal) = equal.as_scalar_function()
            && equal.FuncName.L == ast::NullEQ
        {
            if let Some(col) = equal.GetArgs()[inner_index].as_column() {
                keys.Insert(col.UniqueID as i32);
            }
        }
    }
    keys
}

/// 取出内表 Join Key 组成的 Schema。
fn inner_join_keys(join: &logicalop::LogicalJoin, inner_index: usize) -> expression::Schema {
    expression::NewSchema(
        join.EqualConditions
            .iter()
            .filter_map(|condition| condition.as_scalar_function())
            .filter_map(|condition| condition.GetArgs()[inner_index].as_column())
            .cloned()
            .collect(),
    )
}

// EliminateOuterJoinBelowAggregation：聚合只引用外表且重复不敏感，或内表连接键包含唯一键时，可移除外连接。
/// 聚合下方满足键条件时可消除 Outer Join。
pub struct EliminateOuterJoinBelowAggregation {
    base: BaseRule,
    helper: OuterJoinEliminator,
}
#[allow(non_snake_case)]
/// 构造 EliminateOuterJoinBelowAggregation 规则。
pub fn NewRuleEliminateOuterJoinBelowAggregation() -> Box<dyn Transformation> {
    Box::new(EliminateOuterJoinBelowAggregation {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandAggregation,
                pattern::OperandJoin,
                pattern::EngineTiDBOnly,
            ),
        },
        helper: OuterJoinEliminator,
    })
}
impl Transformation for EliminateOuterJoinBelowAggregation {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        matches!(
            expr.child(0).logical_join().JoinType,
            base::LeftOuterJoin | base::RightOuterJoin
        )
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let join_expr = old.child(0).expr();
        let join = old.child(0).logical_join();
        let Some((inner_index, outer, mut inner, outer_ids)) = self.helper.prepare(&join_expr)
        else {
            return unchanged();
        };
        let used_columns = agg.GetUsedCols();
        if !ruleutil::IsColsAllFromOuterTable(&used_columns, &outer_ids) {
            return unchanged();
        }
        let mut duplicate_agnostic_columns = Vec::new();
        let all_duplicate_agnostic = agg.AggFuncs.iter().all(|function| {
            let duplicate_agnostic = function.HasDistinct
                || matches!(
                    function.Name.as_str(),
                    ast::AggFuncFirstRow
                        | ast::AggFuncMax
                        | ast::AggFuncMin
                        | ast::AggFuncApproxCountDistinct
                );
            if duplicate_agnostic {
                for argument in &function.Args {
                    duplicate_agnostic_columns.extend(
                        expression::ExtractColumns(argument.as_ref())
                            .into_iter()
                            .cloned(),
                    );
                }
            }
            duplicate_agnostic
        });
        if all_duplicate_agnostic && !duplicate_agnostic_columns.is_empty() {
            return rewritten(vec![memo::NewGroupExprWithChildren(
                agg.clone(),
                vec![outer],
            )]);
        }
        let keys = inner_join_keys(&*join, inner_index);
        let nullable = inner_null_eq_keys(&*join, inner_index);
        if self
            .helper
            .inner_keys_contain_unique_key(&mut inner, &keys, &nullable)?
        {
            return rewritten(vec![memo::NewGroupExprWithChildren(
                agg.clone(),
                vec![outer],
            )]);
        }
        unchanged()
    }
}

// EliminateOuterJoinBelowProjection：Projection 只读外表且内侧连接键唯一时，连接不会增加外表行数，可直接删除。
/// Projection 下方满足条件时可消除 Outer Join。
pub struct EliminateOuterJoinBelowProjection {
    base: BaseRule,
    helper: OuterJoinEliminator,
}
#[allow(non_snake_case)]
/// 构造 EliminateOuterJoinBelowProjection 规则。
pub fn NewRuleEliminateOuterJoinBelowProjection() -> Box<dyn Transformation> {
    Box::new(EliminateOuterJoinBelowProjection {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandProjection,
                pattern::OperandJoin,
                pattern::EngineTiDBOnly,
            ),
        },
        helper: OuterJoinEliminator,
    })
}
impl Transformation for EliminateOuterJoinBelowProjection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        matches!(
            expr.child(0).logical_join().JoinType,
            base::LeftOuterJoin | base::RightOuterJoin
        )
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let projection = old.logical_projection();
        let join_expr = old.child(0).expr();
        let join = old.child(0).logical_join();
        let Some((inner_index, outer, mut inner, outer_ids)) = self.helper.prepare(&join_expr)
        else {
            return unchanged();
        };
        let used_columns = projection.GetUsedCols();
        if !ruleutil::IsColsAllFromOuterTable(&used_columns, &outer_ids) {
            return unchanged();
        }
        let keys = inner_join_keys(&*join, inner_index);
        let nullable = inner_null_eq_keys(&*join, inner_index);
        if self
            .helper
            .inner_keys_contain_unique_key(&mut inner, &keys, &nullable)?
        {
            return rewritten(vec![memo::NewGroupExprWithChildren(
                projection.clone(),
                vec![outer],
            )]);
        }
        unchanged()
    }
}

// TransformAggregateCaseToSelection 把受支持的 AGG(CASE WHEN ...) 拆为 Selection 与简化后的聚合参数。
/// 把聚合中的 CASE 形态改写为 Selection+聚合。
pub struct TransformAggregateCaseToSelection {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 TransformAggregateCaseToSelection 规则。
pub fn NewRuleTransformAggregateCaseToSelection() -> Box<dyn Transformation> {
    Box::new(TransformAggregateCaseToSelection {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandAggregation, pattern::EngineTiDBOnly),
        },
    })
}
impl TransformAggregateCaseToSelection {
    fn is_two_or_three_arg_case(&self, expr: &dyn expression::Expression) -> bool {
        expr.as_scalar_function()
            .is_some_and(|f| f.FuncName.L == ast::Case && matches!(f.GetArgs().len(), 2 | 3))
    }
    fn allows_selection(&self, name: &str) -> bool {
        name != ast::AggFuncFirstRow
    }
    fn only_one_not_null(
        &self,
        args: &[expression::ExprBox],
        output: usize,
        eval: &dyn expression::EvalContext,
    ) -> bool {
        !args[output].Equal(eval, &expression::NewNull())
            && (args.len() == 2 || args[3 - output].Equal(eval, &expression::NewNull()))
    }
    fn transform(
        &self,
        agg: &logicalop::LogicalAggregation,
    ) -> Option<(Vec<expression::ExprBox>, Vec<aggregation::AggFuncDesc>)> {
        let desc = &agg.AggFuncs[0];
        let case = desc.Args[0].as_scalar_function()?;
        let args = case.GetArgs();
        let eval = agg.SCtx().GetExprCtx().GetEvalCtx();
        let null_flip = args.len() == 3
            && args[1].Equal(eval, &expression::NewNull())
            && !args[2].Equal(eval, &expression::NewNull());
        let zero_flip =
            !null_flip && args.len() == 3 && args[1].Equal(eval, &expression::NewZero());
        let output = if null_flip || zero_flip { 2 } else { 1 };
        let conditions = if output == 2 {
            vec![expression::not(
                args[0].CloneExpr(),
                agg.SCtx().GetExprCtx(),
            )]
        } else {
            expression::SplitCNFItems(args[0].as_ref())
        };
        if desc.HasDistinct
            && !(desc.Name == ast::AggFuncCount && self.only_one_not_null(args, output, eval))
        {
            return None;
        }
        let other_is_null = args.len() == 2 || args[3 - output].Equal(eval, &expression::NewNull());
        let other_is_zero = args.len() == 3 && args[3 - output].Equal(eval, &expression::NewZero());
        if !desc.HasDistinct
            && !((self.allows_selection(&desc.Name) && other_is_null)
                || (desc.Name == ast::AggFuncSum && other_is_zero))
        {
            return None;
        }
        let mut new_desc = desc.Clone();
        new_desc.Args = vec![args[output].CloneExpr()];
        Some((conditions, vec![new_desc]))
    }
}
impl Transformation for TransformAggregateCaseToSelection {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        let agg = expr.logical_aggregation();
        agg.IsCompleteModeAgg()
            && agg.GroupByItems.is_empty()
            && agg.AggFuncs.len() == 1
            && agg.AggFuncs[0].Args.len() == 1
            && self.is_two_or_three_arg_case(agg.AggFuncs[0].Args[0].as_ref())
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let Some((conditions, funcs)) = self.transform(&agg) else {
            return unchanged();
        };
        let child = old.expr().Children[0].clone();
        let selection_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(
                logicalop::LogicalSelection::new(conditions, agg.SCtx(), agg.QueryBlockOffset()),
                vec![child.clone()],
            ),
            child.Prop.Schema.clone(),
        );
        let mut new_agg = logicalop::LogicalAggregation::new(
            funcs,
            agg.GroupByItems.clone(),
            agg.SCtx(),
            agg.QueryBlockOffset(),
        );
        new_agg.CopyAggHints(&agg);
        rewritten(vec![memo::NewGroupExprWithChildren(
            new_agg,
            vec![selection_group],
        )])
    }
}

// TransformAggToProj 在 GroupBy 列覆盖子组唯一键时消除聚合；GROUP_CONCAT 因空值语义不参与。
/// 无聚合函数的 Aggregation 退化为 Projection。
pub struct TransformAggToProj {
    base: BaseRule,
}

/// 把聚合节点转换为等价 Projection。
fn convert_agg_to_proj(
    agg: &logicalop::LogicalAggregation,
    schema: &expression::Schema,
) -> Option<logicalop::LogicalProjection> {
    let context = agg.SCtx()?.clone();
    let expression_context = context.GetExprCtx();
    let mut expressions = Vec::with_capacity(agg.AggFuncs.len());
    for function in &agg.AggFuncs {
        let argument = function.Args.first()?.CloneExpr();
        let target_type = function.RetTp.clone()?;
        let rewritten = match function.Name.as_str() {
            ast::AggFuncCount => {
                let mut null_checks = Vec::with_capacity(function.Args.len());
                for argument in &function.Args {
                    if mysql::HasNotNullFlag(
                        argument.GetType(expression_context.GetEvalCtx()).GetFlag(),
                    ) {
                        null_checks.push(Box::new(expression::NewZero()) as expression::ExprBox);
                    } else {
                        null_checks.push(expression::NewFunctionInternal(
                            expression_context,
                            ast::IsNull,
                            {
                                let mut field_type = expression::types::FieldType::default();
                                field_type.SetType(mysql::TypeTiny);
                                field_type
                            },
                            vec![argument.CloneExpr()],
                        )?);
                    }
                }
                let any_null = expression::ComposeDNFCondition(expression_context, &null_checks)?;
                expression::NewFunctionInternal(
                    expression_context,
                    ast::If,
                    target_type,
                    vec![
                        any_null,
                        Box::new(expression::NewZero()),
                        Box::new(expression::NewOne()),
                    ],
                )?
            }
            ast::AggFuncMax
            | ast::AggFuncMin
            | ast::AggFuncSum
            | ast::AggFuncSumInt
            | ast::AggFuncAvg
            | ast::AggFuncFirstRow
            | ast::AggFuncGroupConcat => {
                expression::BuildCastFunction(expression_context, &argument, &target_type)
            }
            ast::AggFuncBitAnd | ast::AggFuncBitOr | ast::AggFuncBitXor => {
                let integer = expression::WrapWithCastAsInt(expression_context, argument, None);
                let cast =
                    expression::BuildCastFunction(expression_context, &integer, &target_type);
                if function.Name == ast::AggFuncBitAnd {
                    cast
                } else {
                    expression::NewFunctionInternal(
                        expression_context,
                        ast::Ifnull,
                        target_type,
                        vec![cast, Box::new(expression::NewZero())],
                    )?
                }
            }
            _ => return None,
        };
        expressions.push(rewritten);
    }
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(context, agg.QueryBlockOffset());
    projection.SetSchema(schema.Clone());
    Some(projection)
}

#[allow(non_snake_case)]
/// 构造 TransformAggToProj 规则。
pub fn NewRuleTransformAggToProj() -> Box<dyn Transformation> {
    Box::new(TransformAggToProj {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandAggregation, pattern::EngineTiDBOnly),
        },
    })
}
impl Transformation for TransformAggToProj {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        let agg = expr.logical_aggregation();
        if !agg.IsCompleteModeAgg()
            || agg
                .AggFuncs
                .iter()
                .any(|f| f.Name == ast::AggFuncGroupConcat)
        {
            return false;
        }
        let mut child = expr.expr().Children[0].clone();
        child.BuildKeyInfo();
        let by = expression::NewSchema(agg.GetGroupByCols());
        child
            .Prop
            .Schema
            .as_deref()
            .expect("memo child must carry a schema")
            .PKOrUK
            .iter()
            .any(|key| by.ColumnsIndices(key).is_some())
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let schema = old.expr().Schema();
        if let Some(projection) = convert_agg_to_proj(&agg, &schema) {
            return rewritten(vec![memo::NewGroupExprWithChildren(
                projection,
                old.expr().Children.clone(),
            )]);
        }
        unchanged()
    }
}

// InjectProjectionBelowTopN 把标量 ByItem 提前到下层 Projection 计算，再用上层 Projection 裁掉临时列。
/// 在 TopN 下注入 Projection 以预计算排序表达式。
pub struct InjectProjectionBelowTopN {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 InjectProjectionBelowTopN 规则。
pub fn NewRuleInjectProjectionBelowTopN() -> Box<dyn Transformation> {
    Box::new(InjectProjectionBelowTopN {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandTopN, pattern::EngineTiDBOnly),
        },
    })
}
impl Transformation for InjectProjectionBelowTopN {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        expr.logical_top_n()
            .ByItems
            .iter()
            .any(|item| item.Expr.as_scalar_function().is_some())
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let top_n = old.logical_top_n();
        let old_schema = old.expr().Schema();
        let eval = top_n.SCtx().GetExprCtx().GetEvalCtx();
        let top_exprs = old_schema
            .Columns
            .iter()
            .map(|column| Box::new(column.Clone()) as expression::ExprBox)
            .collect::<Vec<_>>();
        let mut bottom_exprs = top_exprs
            .iter()
            .map(|item| item.CloneExpr())
            .collect::<Vec<_>>();
        let mut bottom_cols = old_schema.Columns.clone();
        let mut by_items = Vec::new();
        for item in &top_n.ByItems {
            if item.Expr.as_scalar_function().is_none() {
                by_items.push(item.clone());
                continue;
            }
            bottom_exprs.push(item.Expr.CloneExpr());
            let column = expression::Column::new(
                item.Expr.GetType(eval).clone(),
                0,
                top_n.SCtx().GetSessionVars().AllocPlanColumnID(),
                bottom_cols.len() as isize,
            );
            bottom_cols.push(column.clone());
            by_items.push(util::ByItems {
                Expr: Box::new(column),
                Desc: item.Desc,
            });
        }
        let bottom_schema = expression::NewSchema(bottom_cols);
        let mut bottom =
            logicalop::LogicalProjection::new(bottom_exprs, top_n.SCtx(), top_n.QueryBlockOffset());
        bottom.SetSchema(bottom_schema.clone());
        let bottom_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(bottom, vec![old.expr().Children[0].clone()]),
            bottom_schema.clone(),
        );
        let middle = logicalop::LogicalTopN::new(
            by_items,
            top_n.Offset,
            top_n.Count,
            top_n.SCtx(),
            top_n.QueryBlockOffset(),
        );
        let middle_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(middle, vec![bottom_group]),
            bottom_schema,
        );
        let mut top =
            logicalop::LogicalProjection::new(top_exprs, top_n.SCtx(), top_n.QueryBlockOffset());
        top.SetSchema(old_schema);
        rewritten(vec![memo::NewGroupExprWithChildren(
            top,
            vec![middle_group],
        )])
    }
}

// InjectProjectionBelowAgg 将聚合参数和 GroupBy 中的标量函数变为 Projection 输出列，常量保持原位。
/// 在 Aggregation 下注入 Projection 以预计算聚合参数。
pub struct InjectProjectionBelowAgg {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 InjectProjectionBelowAgg 规则。
pub fn NewRuleInjectProjectionBelowAgg() -> Box<dyn Transformation> {
    Box::new(InjectProjectionBelowAgg {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandAggregation, pattern::EngineTiDBOnly),
        },
    })
}
impl Transformation for InjectProjectionBelowAgg {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        expr.logical_aggregation().IsCompleteModeAgg()
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let agg = old.logical_aggregation();
        let eval = agg.SCtx().GetExprCtx().GetEvalCtx();
        let mut funcs: Vec<_> = agg
            .AggFuncs
            .iter()
            .map(|f| {
                let mut copy = f.Clone();
                copy.WrapCastForAggArgs(agg.SCtx().GetExprCtx());
                copy
            })
            .collect();
        let has_scalar = funcs
            .iter()
            .flat_map(|f| &f.Args)
            .chain(&agg.GroupByItems)
            .any(|expr| expr.as_scalar_function().is_some());
        if !has_scalar {
            return unchanged();
        }
        let mut projection_exprs = Vec::new();
        let mut projection_cols = Vec::new();
        for func in &mut funcs {
            for arg in &mut func.Args {
                if arg.as_constant().is_some() {
                    continue;
                }
                if let Some(col) = arg.as_column() {
                    projection_exprs.push(arg.CloneExpr());
                    projection_cols.push(col.clone());
                } else {
                    projection_exprs.push(arg.CloneExpr());
                    let col = expression::Column::new(
                        arg.GetType(eval).clone(),
                        0,
                        agg.SCtx().GetSessionVars().AllocPlanColumnID(),
                        projection_cols.len() as isize,
                    );
                    projection_cols.push(col.clone());
                    *arg = Box::new(col);
                }
            }
        }
        let mut group_by = Vec::new();
        for item in &agg.GroupByItems {
            if item.as_constant().is_some() {
                group_by.push(item.CloneExpr());
            } else if let Some(col) = item.as_column() {
                projection_exprs.push(item.CloneExpr());
                projection_cols.push(col.clone());
                group_by.push(item.CloneExpr());
            } else {
                projection_exprs.push(item.CloneExpr());
                let col = expression::Column::new(
                    item.GetType(eval).clone(),
                    0,
                    agg.SCtx().GetSessionVars().AllocPlanColumnID(),
                    projection_cols.len() as isize,
                );
                projection_cols.push(col.clone());
                group_by.push(Box::new(col));
            }
        }
        let projection_schema = expression::NewSchema(projection_cols);
        let mut projection =
            logicalop::LogicalProjection::new(projection_exprs, agg.SCtx(), agg.QueryBlockOffset());
        projection.SetSchema(projection_schema.clone());
        let projection_group = memo::NewGroupWithSchema(
            memo::NewGroupExprWithChildren(projection, vec![old.expr().Children[0].clone()]),
            projection_schema,
        );
        let mut new_agg =
            logicalop::LogicalAggregation::new(funcs, group_by, agg.SCtx(), agg.QueryBlockOffset());
        new_agg.CopyAggHints(&agg);
        rewritten(vec![memo::NewGroupExprWithChildren(
            new_agg,
            vec![projection_group],
        )])
    }
}

// TransformApplyToJoin 递归收集内侧关联列；若没有引用外侧 Schema，则 Apply 可降级为普通 Join。
/// 把 Apply（相关子查询）转换为普通 Join。
pub struct TransformApplyToJoin {
    base: BaseRule,
}

/// 深拷贝 LogicalJoin 供 Apply→Join 改写使用。
fn clone_logical_join(join: &logicalop::LogicalJoin) -> logicalop::LogicalJoin {
    let clone_exprs = |items: &[expression::ExprBox]| {
        items
            .iter()
            .map(|item| item.CloneExpr())
            .collect::<Vec<_>>()
    };
    let mut cloned = logicalop::LogicalJoin {
        JoinType: join.JoinType,
        Reordered: join.Reordered,
        StraightJoin: join.StraightJoin,
        PreferJoinType: join.PreferJoinType,
        PreferJoinOrder: join.PreferJoinOrder,
        InternalPreferJoinOrder: join.InternalPreferJoinOrder,
        LeftPreferJoinType: join.LeftPreferJoinType,
        RightPreferJoinType: join.RightPreferJoinType,
        EqualConditions: clone_exprs(&join.EqualConditions),
        NAEQConditions: clone_exprs(&join.NAEQConditions),
        LeftConditions: clone_exprs(&join.LeftConditions),
        RightConditions: clone_exprs(&join.RightConditions),
        OtherConditions: clone_exprs(&join.OtherConditions),
        LeftProperties: join.LeftProperties.clone(),
        RightProperties: join.RightProperties.clone(),
        FullSchema: join.FullSchema.as_ref().map(expression::Schema::Clone),
        FullNames: join.FullNames.Shallow(),
        RedundantColsToOutputIdx: join.RedundantColsToOutputIdx.clone(),
        PreferCorrelate: join.PreferCorrelate,
        EqualCondOutCnt: join.EqualCondOutCnt,
        FromDecorrelatedApply: join.FromDecorrelatedApply,
        ..Default::default()
    }
    .Init(required_context(join.SCtx()), join.QueryBlockOffset());
    cloned.SetSchema(join.Schema().Clone());
    cloned.SetOutputNames(join.OutputNames().Shallow());
    cloned
}

/// 克隆相关列列表。
fn clone_correlated_columns(
    columns: &[expression::CorrelatedColumn],
) -> Vec<expression::CorrelatedColumn> {
    columns
        .iter()
        .map(expression::CorrelatedColumn::Clone)
        .collect()
}
#[allow(non_snake_case)]
/// 构造 TransformApplyToJoin 规则。
pub fn NewRuleTransformApplyToJoin() -> Box<dyn Transformation> {
    Box::new(TransformApplyToJoin {
        base: BaseRule {
            pattern: pattern::NewPattern(pattern::OperandApply, pattern::EngineTiDBOnly),
        },
    })
}
impl TransformApplyToJoin {
    fn extract_cor_columns_from_group(
        &self,
        group: &memo::Group,
    ) -> Vec<expression::CorrelatedColumn> {
        let mut columns = Vec::new();
        for expression in &group.Equivalents {
            let expression = expression.borrow();
            columns.extend(coreusage::ExtractCorrelatedCols4LogicalPlan(
                expression.ExprNode.as_ref(),
            ));
            for child in &expression.Children {
                columns
                    .extend(self.extract_cor_columns_from_group(&memo::GroupHandle(child.clone())));
            }
        }
        columns
    }
    fn extract_cor_columns_by_schema(
        &self,
        inner: &memo::Group,
        outer: &expression::Schema,
    ) -> Vec<expression::CorrelatedColumn> {
        let mut columns = self.extract_cor_columns_from_group(inner);
        coreusage::ExtractCorColumnsBySchema(&mut columns, outer, false)
    }
}
impl Transformation for TransformApplyToJoin {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let source = old.logical_apply();
        let mut apply = logicalop::LogicalApply {
            LogicalJoin: clone_logical_join(&source.LogicalJoin),
            CorCols: clone_correlated_columns(&source.CorCols),
            NoDecorrelate: source.NoDecorrelate,
            IsLateral: source.IsLateral,
            PrunedToLeft: source.PrunedToLeft,
        }
        .Init(required_context(source.SCtx()), source.QueryBlockOffset());
        apply.SetSchema(source.Schema().Clone());
        apply.CorCols = self.extract_cor_columns_by_schema(
            &old.expr().Children[1],
            old.expr().Children[0]
                .Prop
                .Schema
                .as_deref()
                .expect("memo outer group must carry a schema"),
        );
        if !apply.CorCols.is_empty() {
            return unchanged();
        }
        rewritten(vec![memo::NewGroupExprWithChildren(
            apply.LogicalJoin,
            old.expr().Children.clone(),
        )])
    }
}

// PullSelectionUpApply 将内侧 Selection 条件按外侧 Schema 去关联，再并入 Apply 的 Join 条件。
/// 把 Apply 内层 Selection 上拉以便后续转 Join。
pub struct PullSelectionUpApply {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 PullSelectionUpApply 规则。
pub fn NewRulePullSelectionUpApply() -> Box<dyn Transformation> {
    let outer = pattern::NewPattern(pattern::OperandAny, pattern::EngineTiDBOnly);
    let inner = pattern::NewPattern(pattern::OperandSelection, pattern::EngineTiDBOnly);
    Box::new(PullSelectionUpApply {
        base: BaseRule {
            pattern: pattern::BuildPattern(
                pattern::OperandApply,
                pattern::EngineTiDBOnly,
                vec![outer, inner],
            ),
        },
    })
}
impl Transformation for PullSelectionUpApply {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        self.base.matches(expr)
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let apply = old.logical_apply();
        let outer = old.child(0).group();
        let inner = old.child(1).group();
        let outer_schema = outer
            .Prop
            .Schema
            .as_deref()
            .expect("memo outer group must carry a schema");
        let inner_schema = inner
            .Prop
            .Schema
            .as_deref()
            .expect("memo inner group must carry a schema");
        let selection = old.child(1).logical_selection();
        let conditions = selection
            .Conditions
            .iter()
            .map(|condition| condition.Decorrelate(outer_schema))
            .collect::<Vec<_>>();
        let mut new_apply = logicalop::LogicalApply::new(
            clone_logical_join(&apply.LogicalJoin),
            clone_correlated_columns(&apply.CorCols),
            apply.SCtx(),
            apply.QueryBlockOffset(),
        );
        new_apply.SetSchema(apply.Schema().Clone());
        for condition in conditions {
            let columns = expression::ExtractColumns(condition.as_ref());
            let from_left =
                !columns.is_empty() && columns.iter().all(|column| outer_schema.Contains(column));
            let from_right =
                !columns.is_empty() && columns.iter().all(|column| inner_schema.Contains(column));
            let is_cross_equality = condition
                .as_scalar_function()
                .filter(|function| {
                    matches!(function.FuncName.L.as_str(), ast::EQ | ast::NullEQ)
                        && function.GetArgs().len() == 2
                })
                .is_some_and(|function| {
                    let left = function.GetArgs()[0].as_column();
                    let right = function.GetArgs()[1].as_column();
                    matches!((left, right), (Some(left), Some(right))
                        if (outer_schema.Contains(left) && inner_schema.Contains(right))
                            || (inner_schema.Contains(left) && outer_schema.Contains(right)))
                });
            if is_cross_equality {
                new_apply.LogicalJoin.EqualConditions.push(condition);
            } else if from_left {
                new_apply.LogicalJoin.LeftConditions.push(condition);
            } else if from_right {
                new_apply.LogicalJoin.RightConditions.push(condition);
            } else {
                new_apply.LogicalJoin.OtherConditions.push(condition);
            }
        }
        // 内侧 Selection 被移除，Apply 直接连接它原来的孩子。
        Ok((
            vec![memo::NewGroupExprWithChildren(
                new_apply,
                vec![outer, old.child(1).expr().Children[0].clone()],
            )],
            false,
            false,
        ))
    }
}

// MergeAdjacentWindow 要求 Partition/Order/Frame 完全相同，且上层窗口函数不引用下层尚未产出的列。
/// 合并可融合的相邻 Window 算子。
pub struct MergeAdjacentWindow {
    base: BaseRule,
}
#[allow(non_snake_case)]
/// 构造 MergeAdjacentWindow 规则。
pub fn NewRuleMergeAdjacentWindow() -> Box<dyn Transformation> {
    Box::new(MergeAdjacentWindow {
        base: BaseRule {
            pattern: unary_pattern(
                pattern::OperandWindow,
                pattern::OperandWindow,
                pattern::EngineAll,
            ),
        },
    })
}
impl Transformation for MergeAdjacentWindow {
    fn get_pattern(&self) -> &pattern::Pattern {
        self.base.get_pattern()
    }
    fn matches(&self, expr: &memo::ExprIter) -> bool {
        let current = expr.logical_window();
        let next_expr = expr.child(0).expr();
        let next = expr.child(0).logical_window();
        if !current.equalPartitionBy(&next)
            || !current.equalOrderBy(&next)
            || !current.equalFrame(&next)
        {
            return false;
        }
        let existing: HashSet<_> = next_expr
            .Children
            .iter()
            .flat_map(|group| {
                group
                    .Prop
                    .Schema
                    .as_deref()
                    .into_iter()
                    .flat_map(|schema| schema.Columns.iter())
            })
            .map(|col| col.UniqueID)
            .collect();
        current
            .WindowFuncDescs
            .iter()
            .flat_map(|f| &f.Args)
            .flat_map(|argument| expression::ExtractColumns(argument.as_ref()))
            .all(|col| existing.contains(&col.UniqueID))
    }
    fn on_transform(&self, old: &memo::ExprIter) -> TransformResult {
        let current = old.logical_window();
        let next = old.child(0).logical_window();
        let mut funcs = current.WindowFuncDescs.clone();
        funcs.extend(next.WindowFuncDescs.clone());
        let merged = logicalop::LogicalWindow::new(
            funcs,
            current.PartitionBy.clone(),
            current.OrderBy.clone(),
            current.Frame.clone(),
            current.SCtx(),
            current.QueryBlockOffset(),
        );
        rewritten(vec![memo::NewGroupExprWithChildren(
            merged,
            old.child(0).expr().Children.clone(),
        )])
    }
}
