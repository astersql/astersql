// Copyright 2026 AsterSQL.

// Traversal of the real parser AST, including statement and expression children.
// Field lists follow the AST definitions; containers preserve source order.
use super::*;

pub(crate) trait Children {
    fn visit_children(&self, visitor: &mut dyn Visitor) -> bool;
}
pub(crate) trait MutChildren {
    fn visit_children_mut(&mut self, visitor: &mut dyn InPlaceVisitor) -> bool;
}
trait Visit {
    fn visit(&self, visitor: &mut dyn Visitor) -> bool;
}
trait VisitMut {
    fn visit_mut(&mut self, visitor: &mut dyn InPlaceVisitor) -> bool;
}
macro_rules! node_visit {
    ($($name:ty),* $(,)?) => {$ (
        impl Visit for $name {
            fn visit(&self, v: &mut dyn Visitor) -> bool { self.accept(v) }
        }
        impl VisitMut for $name {
            fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool { self.accept_in_place(v) }
        }
    )*};
}
node_visit!(
    AddQueryWatchStmt,
    AdminStmt,
    AlterDatabaseStmt,
    AlterInstanceStmt,
    AlterPlacementPolicyStmt,
    AlterRangeStmt,
    AlterResourceGroupStmt,
    AlterSequenceStmt,
    AlterTableStmt,
    AlterUserStmt,
    AnalyzeTableStmt,
    BRIEStmt,
    BeginStmt,
    BinlogStmt,
    CalibrateResourceStmt,
    CallStmt,
    CancelDistributionJobStmt,
    CleanupTableLockStmt,
    CommitStmt,
    CompactTableStmt,
    CreateBindingStmt,
    CreateDatabaseStmt,
    CreateIndexStmt,
    CreateMaskingPolicyStmt,
    CreatePlacementPolicyStmt,
    CreateResourceGroupStmt,
    CreateSequenceStmt,
    CreateStatisticsStmt,
    CreateTableStmt,
    CreateUserStmt,
    CreateViewStmt,
    CreateMaterializedViewStmt,
    CreateMaterializedViewLogStmt,
    AlterMaterializedViewAction,
    AlterMaterializedViewStmt,
    AlterMaterializedViewLogAction,
    AlterMaterializedViewLogStmt,
    DropMaterializedViewStmt,
    DropMaterializedViewLogStmt,
    PurgeMaterializedViewLogStmt,
    CancelMaterializedViewJobStmt,
    RefreshMaterializedViewStmt,
    RefreshMaterializedViewImplementStmt,
    DeallocateStmt,
    DeleteStmt,
    DistributeTableStmt,
    DoStmt,
    DropBindingStmt,
    DropDatabaseStmt,
    DropIndexStmt,
    DropPlacementPolicyStmt,
    DropProcedureStmt,
    DropQueryWatchStmt,
    DropResourceGroupStmt,
    DropSequenceStmt,
    DropStatisticsStmt,
    DropStatsStmt,
    DropTableStmt,
    DropUserStmt,
    ExecuteStmt,
    ExplainForStmt,
    ExplainStmt,
    ExprNode,
    FlashBackDatabaseStmt,
    FlashBackTableStmt,
    FlashBackToTimestampStmt,
    FlushStmt,
    GrantProxyStmt,
    GrantRoleStmt,
    GrantStmt,
    HelpStmt,
    ImportIntoActionStmt,
    ImportIntoStmt,
    InsertStmt,
    KillStmt,
    LoadDataStmt,
    LoadStatsStmt,
    LockStatsStmt,
    LockTablesStmt,
    NonTransactionalDMLStmt,
    OptimizeTableStmt,
    PlanReplayerStmt,
    PrepareStmt,
    ProcedureBlock,
    ProcedureCloseCur,
    ProcedureCursor,
    ProcedureElseBlock,
    ProcedureElseIfBlock,
    ProcedureErrorCon,
    ProcedureErrorControl,
    ProcedureErrorState,
    ProcedureErrorVal,
    ProcedureFetchInto,
    ProcedureIfBlock,
    ProcedureIfInfo,
    ProcedureInfo,
    ProcedureJump,
    ProcedureLabelBlock,
    ProcedureLabelLoop,
    ProcedureOpenCur,
    ProcedureRepeatStmt,
    ProcedureWhileStmt,
    RecommendIndexStmt,
    RecoverTableStmt,
    RefreshStatsStmt,
    ReleaseSavepointStmt,
    RenameTableStmt,
    RenameUserStmt,
    RepairTableStmt,
    RestartStmt,
    RevokeRoleStmt,
    RevokeStmt,
    RollbackStmt,
    SavepointStmt,
    SearchCaseStmt,
    SearchWhenThenStmt,
    SelectStmt,
    SetBindingStmt,
    SetConfigStmt,
    SetDefaultRoleStmt,
    SetOprSelectList,
    SetOprStmt,
    SetPwdStmt,
    SetResourceGroupStmt,
    SetRoleStmt,
    SetSessionStatesStmt,
    SetStmt,
    ShowStmt,
    ShutdownStmt,
    SimpleCaseStmt,
    SimpleWhenThenStmt,
    SplitRegionStmt,
    TraceStmt,
    TrafficStmt,
    TruncateTableStmt,
    UnlockStatsStmt,
    UnlockTablesStmt,
    UpdateStmt,
    UseStmt,
    VariableExpr
);
impl Visit for dyn Node {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.accept(v)
    }
}
impl VisitMut for dyn Node {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.accept_in_place(v)
    }
}
impl<T: Visit> Visit for Option<T> {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.as_ref().is_none_or(|x| x.visit(v))
    }
}
impl<T: VisitMut> VisitMut for Option<T> {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.as_mut().is_none_or(|x| x.visit_mut(v))
    }
}
impl<T: Visit> Visit for Vec<T> {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.iter().all(|x| x.visit(v))
    }
}
impl<T: VisitMut> VisitMut for Vec<T> {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.iter_mut().all(|x| x.visit_mut(v))
    }
}
impl<T: Visit + ?Sized> Visit for Box<T> {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        (**self).visit(v)
    }
}
impl<T: VisitMut + ?Sized> VisitMut for Box<T> {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        (**self).visit_mut(v)
    }
}
impl Visit for WithClauseRef {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.borrow().visit(v)
    }
}
impl VisitMut for WithClauseRef {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.borrow_mut().visit_mut(v)
    }
}
impl Visit for NodeRef {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.with_node(|x| x.accept(v)).unwrap_or(true)
    }
}
impl VisitMut for NodeRef {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.with_node_mut(|x| x.accept_in_place(v)).unwrap_or(true)
    }
}
impl Visit for TableName {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        if v.enter_table_name(self) {
            return v.leave_table_name(self);
        }
        v.leave_table_name(self)
    }
}
impl VisitMut for TableName {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        if v.enter_table_name(self) {
            return v.leave_table_name(self);
        }
        v.leave_table_name(self)
    }
}
impl Visit for ColumnName {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        let _skip = v.enter_column_name(self);
        v.leave_column_name(self)
    }
}
impl VisitMut for ColumnName {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        let _skip = v.enter_column_name(self);
        v.leave_column_name(self)
    }
}
impl Visit for OnDeleteOpt {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        let _skip = v.enter_on_delete(self);
        v.leave_on_delete(self)
    }
}
impl VisitMut for OnDeleteOpt {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        let _skip = v.enter_on_delete(self);
        v.leave_on_delete(self)
    }
}
impl Visit for OnUpdateOpt {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        let _skip = v.enter_on_update(self);
        v.leave_on_update(self)
    }
}
impl VisitMut for OnUpdateOpt {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        let _skip = v.enter_on_update(self);
        v.leave_on_update(self)
    }
}
macro_rules! children {
    ($name:ty => $($field:ident),* $(,)?) => {
        impl Children for $name {
            fn visit_children(&self, _v: &mut dyn Visitor) -> bool {
                true $(&& self.$field.visit(_v))*
            }
        }
        impl MutChildren for $name {
            fn visit_children_mut(&mut self, _v: &mut dyn InPlaceVisitor) -> bool {
                true $(&& self.$field.visit_mut(_v))*
            }
        }
    };
}
macro_rules! embedded {
    ($($name:ty),* $(,)?) => {$ (
        impl Visit for $name {
            fn visit(&self, v: &mut dyn Visitor) -> bool {
                if v.enter_embedded(self) { return v.leave_embedded(self); }
                self.visit_children(v) && v.leave_embedded(self)
            }
        }
        impl VisitMut for $name {
            fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
                if v.enter_embedded(self) { return v.leave_embedded(self); }
                if !self.visit_children_mut(v) { return false; }
                v.leave_embedded(self)
            }
        }
    )*};
}

children!(DoStmt => Exprs);
children!(CallStmt => Procedure);
children!(ShowStmt => Table, Column, Pattern, Where, ShowProfileLimit);
impl Children for TableSource {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        let source_ok = if let Some(query) = &self.QuerySource {
            query.visit(v)
        } else {
            self.Source.visit(v)
        };
        source_ok && self.TableSample.visit(v) && self.AsOf.visit(v)
    }
}
impl MutChildren for TableSource {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        let source_ok = if let Some(query) = &mut self.QuerySource {
            query.visit_mut(v)
        } else {
            self.Source.visit_mut(v)
        };
        source_ok && self.TableSample.visit_mut(v) && self.AsOf.visit_mut(v)
    }
}
children!(TableSample => Expr, RepeatableSeed);
children!(Join => Left, Right, On, Using);
children!(TableRefsClause => TableRefs);
children!(WhenClause => Expr, Result);
children!(ExprNode => Kind);
children!(SelectField => Expr);
children!(FieldList => Fields);
children!(ByItem => Expr);
children!(Limit => Count, Offset);
children!(Assignment => Column, Expr);
children!(InsertStmt => Select, Table, Columns, Lists, OnDuplicate, Returning);
children!(UpdateStmt => With, TableRefs, List, Where, Order, Limit, Returning);
children!(DeleteStmt => With, TableRefs, Tables, Where, Order, Limit, Returning);
children!(ColumnOption => Expr);
children!(ColumnDef => Name, Options);
children!(ColumnPosition => RelativeColumn);
children!(IndexLockAndAlgorithm => );
children!(ResourceGroupRunawayActionOption => );
children!(AttributesSpec => );
children!(StatsOptionsSpec => );
children!(WildCardField => );
children!(SelectIntoOption => );
children!(PrivElem => Cols);
children!(UserToUser => );
children!(TableOptimizerHint => );
children!(StoreParameter => );
children!(ColumnNameOrUserVar => ColumnName, UserVar);
impl Visit for TimeUnitType {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        let _skip = v.enter_embedded(self);
        v.leave_embedded(self)
    }
}
impl VisitMut for TimeUnitType {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        let _skip = v.enter_embedded(self);
        v.leave_embedded(self)
    }
}
children!(TableOption => Value, TimeUnitValue);
children!(PartitionIntervalExpr => Expr);
children!(PartitionInterval => IntervalExpr, FirstRangeEnd, LastRangeEnd);
children!(PartitionMethod => Expr, ColumnNames);
children!(SubPartitionDefinition => Options);
children!(PartitionDefinition => Clause);
children!(PartitionOptions => PartitionMethod, Sub, Definitions);
children!(CreateTableStmt => Table, ReferTable, Cols, Constraints, SplitIndex, Select, Partition, Options);
children!(CreateViewStmt => ViewName, Select);
children!(MViewRefreshClause => StartWith, Next);
children!(MLogPurgeClause => StartWith, Next);
children!(CreateMaterializedViewStmt => ViewName, Options, Refresh, Select);
children!(CreateMaterializedViewLogStmt => Table, Options, Purge);
children!(AlterMaterializedViewAction => Refresh);
children!(AlterMaterializedViewStmt => ViewName, Actions);
children!(AlterMaterializedViewLogAction => Purge);
children!(AlterMaterializedViewLogStmt => Table, Actions);
children!(DropMaterializedViewStmt => ViewName);
children!(DropMaterializedViewLogStmt => Table);
children!(PurgeMaterializedViewLogStmt => Table);
children!(CancelMaterializedViewJobStmt => );
children!(RefreshMaterializedViewStmt => ViewName, AsOf);
children!(RefreshMaterializedViewImplementStmt => RefreshStmt);
children!(AlterTableSpec => Constraint, NewTable, SplitIndex, NewColumns, OldColumnName, Position, MaskingPolicyColumn, MaskingPolicyExpr, Partition, Options, PartDefinitions);
children!(ReferenceDef => Table, IndexPartSpecifications, OnDelete, OnUpdate);
children!(Constraint => Keys, Refer, Option, Expr);
children!(AlterTableStmt => Table, Specs);
children!(DropTableStmt => Tables);
children!(CreateDatabaseStmt => );
children!(DropDatabaseStmt => );
children!(AlterDatabaseStmt => );
children!(AlterInstanceStmt => );
children!(AlterRangeStmt => );
children!(StringOrUserVar => UserVar);
impl Children for CreateBindingStmt {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        if let Some(origin) = &self.OriginNode {
            origin.visit(v) && self.HintedNode.visit(v)
        } else {
            self.PlanDigests.visit(v)
        }
    }
}
impl MutChildren for CreateBindingStmt {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        if let Some(origin) = &mut self.OriginNode {
            origin.visit_mut(v) && self.HintedNode.visit_mut(v)
        } else {
            self.PlanDigests.visit_mut(v)
        }
    }
}
impl Children for DropBindingStmt {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        if let Some(origin) = &self.OriginNode {
            origin.visit(v) && self.HintedNode.visit(v)
        } else {
            self.SQLDigests.visit(v)
        }
    }
}
impl MutChildren for DropBindingStmt {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        if let Some(origin) = &mut self.OriginNode {
            origin.visit_mut(v) && self.HintedNode.visit_mut(v)
        } else {
            self.SQLDigests.visit_mut(v)
        }
    }
}
impl Children for SetBindingStmt {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        self.OriginNode
            .as_ref()
            .is_none_or(|origin| origin.visit(v) && self.HintedNode.visit(v))
    }
}
impl MutChildren for SetBindingStmt {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.OriginNode
            .as_mut()
            .is_none_or(|origin| origin.visit_mut(v) && self.HintedNode.visit_mut(v))
    }
}
children!(DistributeTableStmt => Table);
children!(CancelDistributionJobStmt => );
children!(RenameUserStmt => UserToUsers);
children!(DropUserStmt => );
children!(DropProcedureStmt => );
children!(DropPlacementPolicyStmt => );
children!(DropResourceGroupStmt => );
children!(CreateResourceGroupStmt => );
children!(AlterResourceGroupStmt => );
children!(CreatePlacementPolicyStmt => );
children!(AlterPlacementPolicyStmt => );
children!(DropQueryWatchStmt => );
children!(ImportIntoActionStmt => );
children!(CreateSequenceStmt => Name);
children!(AlterSequenceStmt => Name);
children!(DropSequenceStmt => Sequences);
children!(TruncateTableStmt => Table);
children!(RecoverTableStmt => Table);
impl Children for FlashBackToTimestampStmt {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        self.Tables.visit(v) && (self.FlashbackTSO != 0 || self.FlashbackTS.visit(v))
    }
}
impl MutChildren for FlashBackToTimestampStmt {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.Tables.visit_mut(v) && (self.FlashbackTSO != 0 || self.FlashbackTS.visit_mut(v))
    }
}
children!(FlashBackTableStmt => Table);
children!(FlashBackDatabaseStmt => );
impl Children for IndexPartSpecification {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        if let Some(expr) = &self.Expr {
            expr.visit(v)
        } else {
            self.Column.visit(v)
        }
    }
}
impl MutChildren for IndexPartSpecification {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        if let Some(expr) = &mut self.Expr {
            expr.visit_mut(v)
        } else {
            self.Column.visit_mut(v)
        }
    }
}
children!(SplitOption => Lower, Upper, ValueLists);
children!(SplitRegionStmt => Table, SplitOpt);
children!(SplitIndexOption => SplitOpt);
children!(IndexOption => SplitOpt);
children!(CreateIndexStmt => Table, IndexPartSpecifications, Option, LockAlg);
children!(DropIndexStmt => Table, LockAlg);
children!(RenameTableStmt => TableToTables);
children!(TableToTable => OldTable, NewTable);
children!(AnalyzeOpt => Value);
children!(AnalyzeTableStmt => TableNames);
children!(CompactTableStmt => Table);
children!(OptimizeTableStmt => );
children!(KillStmt => );
children!(LoadStatsStmt => );
children!(LockStatsStmt => Tables);
children!(UnlockStatsStmt => Tables);
children!(DropStatsStmt => Tables);
children!(RefreshStatsStmt => );
children!(FlushStmt => Tables);
children!(LockTablesStmt => TableLocks);
children!(TableLock => Table);
children!(UseStmt => );
children!(SetStmt => Variables);
children!(VariableAssignment => Value);
children!(BeginStmt => AsOf);
children!(BinlogStmt => );
children!(DeallocateStmt => );
children!(PrepareStmt => );
children!(ExecuteStmt => UsingVars);
children!(HelpStmt => );
children!(SavepointStmt => );
children!(ReleaseSavepointStmt => );
children!(RecommendIndexStmt => );
children!(RecommendIndexOption => Value);
children!(AsOfClause => TsExpr);
impl Children for PlanReplayerStmt {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        if self.Load {
            return true;
        }
        if !self.HistoricalStatsInfo.visit(v) {
            return false;
        }
        if let Some(statement) = &self.Stmt {
            statement.visit(v)
        } else {
            self.Where.visit(v) && self.OrderBy.visit(v) && self.Limit.visit(v)
        }
    }
}
impl MutChildren for PlanReplayerStmt {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        if self.Load {
            return true;
        }
        if !self.HistoricalStatsInfo.visit_mut(v) {
            return false;
        }
        if let Some(statement) = &mut self.Stmt {
            statement.visit_mut(v)
        } else {
            self.Where.visit_mut(v) && self.OrderBy.visit_mut(v) && self.Limit.visit_mut(v)
        }
    }
}
children!(TrafficOption => FloatValue);
children!(TrafficStmt => );
children!(DropStatisticsStmt => );
children!(CreateStatisticsStmt => Table, Columns);
children!(SetPwdStmt => );
children!(SetSessionStatesStmt => );
children!(SetConfigStmt => Value);
children!(SetResourceGroupStmt => );
children!(SetRoleStmt => );
children!(SetDefaultRoleStmt => );
children!(CreateUserStmt => );
children!(AlterUserStmt => );
children!(GrantStmt => Privs);
children!(GrantProxyStmt => );
children!(GrantRoleStmt => );
children!(RevokeStmt => Privs);
children!(RevokeRoleStmt => );
children!(LoadDataOpt => Value);
children!(LoadDataStmt => Table, Columns, ColumnAssignments, ColumnsAndUserVars);
children!(ImportIntoStmt => Table, ColumnsAndUserVars, ColumnAssignments, Select);
children!(NonTransactionalDMLStmt => ShardColumn, DMLStmt);
children!(CreateMaskingPolicyStmt => Table, Column, Expr);
children!(DynamicCalibrateResourceOption => Ts);
children!(CalibrateResourceStmt => DynamicCalibrateResourceOptionList);
children!(QueryWatchResourceGroupOption => GroupNameExpr);
children!(QueryWatchTextOption => PatternExpr);
children!(QueryWatchOption => ResourceGroupOption, ActionOption, TextOption);
children!(AddQueryWatchStmt => QueryWatchOptionList);
children!(ProcedureDecl => DeclDefault);
children!(ProcedureOpenCur => );
children!(ProcedureCloseCur => );
children!(ProcedureFetchInto => );
children!(ProcedureErrorCon => );
children!(ProcedureErrorVal => );
children!(ProcedureErrorState => );
children!(ProcedureCursor => );
macro_rules! procedure_any_visitors {
    ($($name:ty),* $(,)?) => {
        fn visit_procedure_any(value: &dyn std::any::Any, visitor: &mut dyn Visitor) -> bool {
            if let Some(node) = value.downcast_ref::<Box<dyn Node>>() { return node.accept(visitor); }
            $(if let Some(node) = value.downcast_ref::<$name>() { return node.visit(visitor); })*
            true
        }
        fn visit_procedure_any_mut(value: &mut dyn std::any::Any, visitor: &mut dyn InPlaceVisitor) -> bool {
            if let Some(node) = value.downcast_mut::<Box<dyn Node>>() { return node.accept_in_place(visitor); }
            $(if let Some(node) = value.downcast_mut::<$name>() { return node.visit_mut(visitor); })*
            true
        }
    };
}
procedure_any_visitors!(
    ProcedureDecl,
    ExprNode,
    ProcedureBlock,
    ProcedureCursor,
    ProcedureErrorControl,
    ProcedureErrorCon,
    ProcedureErrorVal,
    ProcedureErrorState,
    ProcedureIfBlock,
    ProcedureElseIfBlock,
    ProcedureElseBlock,
    ProcedureWhileStmt,
    ProcedureRepeatStmt,
    ProcedureOpenCur,
    ProcedureCloseCur,
    ProcedureFetchInto,
    ProcedureLabelBlock,
    ProcedureLabelLoop,
    ProcedureJump,
    SimpleCaseStmt,
    SearchCaseStmt,
);
impl Children for ProcedureBlock {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        self.ProcedureVars
            .iter()
            .all(|value| visit_procedure_any(value.as_ref(), v))
    }
}
impl MutChildren for ProcedureBlock {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.ProcedureVars
            .iter_mut()
            .all(|value| visit_procedure_any_mut(value.as_mut(), v))
    }
}
impl Children for ProcedureErrorControl {
    fn visit_children(&self, v: &mut dyn Visitor) -> bool {
        self.ErrorCon
            .iter()
            .all(|value| visit_procedure_any(value.as_ref(), v))
    }
}
impl MutChildren for ProcedureErrorControl {
    fn visit_children_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.ErrorCon
            .iter_mut()
            .all(|value| visit_procedure_any_mut(value.as_mut(), v))
    }
}
children!(ProcedureIfInfo => IfBody);
children!(ProcedureIfBlock => IfExpr, ProcedureElseStmt);
children!(ProcedureElseIfBlock => ProcedureIfStmt);
children!(ProcedureElseBlock => );
children!(SimpleWhenThenStmt => Expr);
children!(SearchWhenThenStmt => Expr);
children!(SimpleCaseStmt => Condition, WhenCases, ElseCases);
children!(SearchCaseStmt => WhenCases);
children!(ProcedureWhileStmt => Condition, Body);
children!(ProcedureRepeatStmt => Body, Condition);
children!(ProcedureLabelBlock => Block);
children!(ProcedureLabelLoop => Block);
children!(ProcedureJump => );
children!(ProcedureInfo => ProcedureParam, ProcedureBody);
children!(CommitStmt => );
children!(RollbackStmt => );
children!(RowExpr => Values);
children!(FrameBound => Expr);
children!(FrameExtent => Start, End);
children!(FrameClause => Extent);
children!(WindowSpec => PartitionBy, OrderBy, Frame);
children!(SelectStmt => With, TableHints, Fields, From, Where, GroupBy, Having, Lists, WindowSpecs, OrderBy, Limit, lock_info, children);
impl Visit for SelectLockInfo {
    fn visit(&self, v: &mut dyn Visitor) -> bool {
        self.Tables.visit(v)
    }
}
impl VisitMut for SelectLockInfo {
    fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool {
        self.Tables.visit_mut(v)
    }
}
children!(ExplainStmt => stmt);
children!(ExplainForStmt => );
children!(CommonTableExpression => Query);
children!(WithClause => CTEs);
children!(SetOprSelectList => With, selects, OrderBy, Limit);
children!(SetOprStmt => With, select_list, OrderBy, Limit);
children!(AlterJobOption => Value);
children!(AdminStmt => tables, where_expr);
children!(CleanupTableLockStmt => Tables);
children!(RepairTableStmt => Table, CreateStmt);
children!(TraceStmt => Stmt);
children!(BRIEStmt => Tables);
children!(VariableExpr => );
impl Children for ResultSetNode {
    fn visit_children(&self, _v: &mut dyn Visitor) -> bool {
        match self {
            Self::TableSource(x0) => x0.visit(_v),
            Self::Join(x0) => x0.visit(_v),
        }
    }
}
impl Children for ExprKind {
    fn visit_children(&self, _v: &mut dyn Visitor) -> bool {
        match self {
            Self::Value(_) => true,
            Self::IntroducedValue { .. } => true,
            Self::Column(column) => column.visit(_v),
            Self::Variable { Value, .. } => Value.visit(_v),
            Self::Function { Args, .. } => Args.visit(_v),
            Self::AggregateFunction { Args, Order, .. } => Args.visit(_v) && Order.visit(_v),
            Self::Binary { L, R, .. } => L.visit(_v) && R.visit(_v),
            Self::Unary { V, .. } => V.visit(_v),
            Self::IsTruth { Expr, .. } => Expr.visit(_v),
            Self::IsNull { Expr, .. } => Expr.visit(_v),
            Self::InList { Expr, List, .. } => Expr.visit(_v) && List.visit(_v),
            Self::Between {
                Expr, Left, Right, ..
            } => Expr.visit(_v) && Left.visit(_v) && Right.visit(_v),
            Self::Like { Expr, Pattern, .. } => Expr.visit(_v) && Pattern.visit(_v),
            Self::Regexp { Expr, Pattern, .. } => Expr.visit(_v) && Pattern.visit(_v),
            Self::Row(x0) => x0.visit(_v),
            Self::Collate { Expr, .. } => Expr.visit(_v),
            Self::NamedDefault(_) => true,
            Self::MaxValue => true,
            Self::MatchAgainst {
                ColumnNames,
                Against,
                ..
            } => ColumnNames.visit(_v) && Against.visit(_v),
            Self::Case {
                Value,
                WhenClauses,
                ElseClause,
                ..
            } => Value.visit(_v) && WhenClauses.visit(_v) && ElseClause.visit(_v),
            Self::WindowFunction { Args, Spec, .. } => Args.visit(_v) && Spec.visit(_v),
            Self::TimeUnit(_) => true,
            Self::GetFormatSelector(_) => true,
            Self::TrimDirection(_) => true,
            Self::TableName(table) => table.visit(_v),
            Self::Parentheses(x0) => x0.visit(_v),
            Self::ParamMarker { .. } => true,
            Self::DefaultValue => true,
            Self::Subquery { Query, .. } => Query.visit(_v),
            Self::CompareSubquery { L, R, .. } => L.visit(_v) && R.visit(_v),
            Self::InSubquery { Expr, Sel, .. } => Expr.visit(_v) && Sel.visit(_v),
            Self::ExistsSubquery { Sel, .. } => Sel.visit(_v),
            Self::Cast { Expr, .. } => Expr.visit(_v),
            Self::JSONSumCrc32 { Expr, .. } => Expr.visit(_v),
        }
    }
}
impl Children for PartitionDefinitionClause {
    fn visit_children(&self, _v: &mut dyn Visitor) -> bool {
        match self {
            Self::None => true,
            Self::LessThan(x0) => x0.visit(_v),
            Self::In(x0) => x0.visit(_v),
            Self::History { .. } => true,
        }
    }
}
impl MutChildren for ResultSetNode {
    fn visit_children_mut(&mut self, _v: &mut dyn InPlaceVisitor) -> bool {
        match self {
            Self::TableSource(x0) => x0.visit_mut(_v),
            Self::Join(x0) => x0.visit_mut(_v),
        }
    }
}
impl MutChildren for ExprKind {
    fn visit_children_mut(&mut self, _v: &mut dyn InPlaceVisitor) -> bool {
        match self {
            Self::Value(_) => true,
            Self::IntroducedValue { .. } => true,
            Self::Column(column) => column.visit_mut(_v),
            Self::Variable { Value, .. } => Value.visit_mut(_v),
            Self::Function { Args, .. } => Args.visit_mut(_v),
            Self::AggregateFunction { Args, Order, .. } => {
                Args.visit_mut(_v) && Order.visit_mut(_v)
            }
            Self::Binary { L, R, .. } => L.visit_mut(_v) && R.visit_mut(_v),
            Self::Unary { V, .. } => V.visit_mut(_v),
            Self::IsTruth { Expr, .. } => Expr.visit_mut(_v),
            Self::IsNull { Expr, .. } => Expr.visit_mut(_v),
            Self::InList { Expr, List, .. } => Expr.visit_mut(_v) && List.visit_mut(_v),
            Self::Between {
                Expr, Left, Right, ..
            } => Expr.visit_mut(_v) && Left.visit_mut(_v) && Right.visit_mut(_v),
            Self::Like { Expr, Pattern, .. } => Expr.visit_mut(_v) && Pattern.visit_mut(_v),
            Self::Regexp { Expr, Pattern, .. } => Expr.visit_mut(_v) && Pattern.visit_mut(_v),
            Self::Row(x0) => x0.visit_mut(_v),
            Self::Collate { Expr, .. } => Expr.visit_mut(_v),
            Self::NamedDefault(_) => true,
            Self::MaxValue => true,
            Self::MatchAgainst {
                ColumnNames,
                Against,
                ..
            } => ColumnNames.visit_mut(_v) && Against.visit_mut(_v),
            Self::Case {
                Value,
                WhenClauses,
                ElseClause,
                ..
            } => Value.visit_mut(_v) && WhenClauses.visit_mut(_v) && ElseClause.visit_mut(_v),
            Self::WindowFunction { Args, Spec, .. } => Args.visit_mut(_v) && Spec.visit_mut(_v),
            Self::TimeUnit(_) => true,
            Self::GetFormatSelector(_) => true,
            Self::TrimDirection(_) => true,
            Self::TableName(table) => table.visit_mut(_v),
            Self::Parentheses(x0) => x0.visit_mut(_v),
            Self::ParamMarker { .. } => true,
            Self::DefaultValue => true,
            Self::Subquery { Query, .. } => Query.visit_mut(_v),
            Self::CompareSubquery { L, R, .. } => L.visit_mut(_v) && R.visit_mut(_v),
            Self::InSubquery { Expr, Sel, .. } => Expr.visit_mut(_v) && Sel.visit_mut(_v),
            Self::ExistsSubquery { Sel, .. } => Sel.visit_mut(_v),
            Self::Cast { Expr, .. } => Expr.visit_mut(_v),
            Self::JSONSumCrc32 { Expr, .. } => Expr.visit_mut(_v),
        }
    }
}
impl MutChildren for PartitionDefinitionClause {
    fn visit_children_mut(&mut self, _v: &mut dyn InPlaceVisitor) -> bool {
        match self {
            Self::None => true,
            Self::LessThan(x0) => x0.visit_mut(_v),
            Self::In(x0) => x0.visit_mut(_v),
            Self::History { .. } => true,
        }
    }
}
embedded!(
    MViewRefreshClause,
    MLogPurgeClause,
    AlterJobOption,
    AlterTableSpec,
    AnalyzeOpt,
    AsOfClause,
    Assignment,
    AttributesSpec,
    ByItem,
    ColumnDef,
    ColumnPosition,
    ColumnNameOrUserVar,
    ColumnOption,
    CommonTableExpression,
    Constraint,
    DynamicCalibrateResourceOption,
    ExprKind,
    FieldList,
    FrameBound,
    FrameClause,
    FrameExtent,
    IndexLockAndAlgorithm,
    IndexOption,
    IndexPartSpecification,
    Join,
    Limit,
    LoadDataOpt,
    PartitionDefinition,
    PartitionDefinitionClause,
    PartitionInterval,
    PartitionIntervalExpr,
    PartitionMethod,
    PartitionOptions,
    PrivElem,
    ProcedureDecl,
    QueryWatchOption,
    QueryWatchResourceGroupOption,
    QueryWatchTextOption,
    ResourceGroupRunawayActionOption,
    RecommendIndexOption,
    ReferenceDef,
    ResultSetNode,
    RowExpr,
    SelectField,
    SelectIntoOption,
    SplitIndexOption,
    SplitOption,
    StatsOptionsSpec,
    StoreParameter,
    StringOrUserVar,
    SubPartitionDefinition,
    TableOption,
    TableLock,
    TableOptimizerHint,
    TableRefsClause,
    TableSample,
    TableSource,
    TableToTable,
    TrafficOption,
    UserToUser,
    VariableAssignment,
    WildCardField,
    WhenClause,
    WindowSpec,
    WithClause
);

children!(RestartStmt => );

children!(ShutdownStmt => );

children!(UnlockTablesStmt => );
