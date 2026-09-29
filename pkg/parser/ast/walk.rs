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
            fn visit(&self, v: &mut dyn Visitor) -> bool { self.visit_children(v) }
        }
        impl VisitMut for $name {
            fn visit_mut(&mut self, v: &mut dyn InPlaceVisitor) -> bool { self.visit_children_mut(v) }
        }
    )*};
}

children!(DoStmt => Exprs);
children!(CallStmt => Procedure);
children!(ShowStmt => Pattern, Where, ShowProfileLimit);
children!(TableSource => QuerySource, TableSample, AsOf);
children!(TableSample => Expr, RepeatableSeed);
children!(Join => Left, Right, On);
children!(TableRefsClause => TableRefs);
children!(WhenClause => Expr, Result);
children!(ExprNode => Kind);
children!(SelectField => Expr);
children!(FieldList => Fields);
children!(ByItem => Expr);
children!(Limit => Count, Offset);
children!(Assignment => Expr);
children!(InsertStmt => Table, Lists, OnDuplicate, Select, Returning);
children!(UpdateStmt => TableRefs, List, Where, Order, Limit, Returning, With);
children!(DeleteStmt => TableRefs, Where, Order, Limit, Returning, With);
children!(ColumnOption => Expr, Refer);
children!(ColumnDef => Options);
children!(ColumnNameOrUserVar => UserVar);
children!(TableOption => Value);
children!(PartitionIntervalExpr => Expr);
children!(PartitionInterval => IntervalExpr, FirstRangeEnd, LastRangeEnd);
children!(PartitionMethod => Expr, ColumnNames);
children!(SubPartitionDefinition => Options);
children!(PartitionDefinition => Clause);
children!(PartitionOptions => PartitionMethod, Sub, Definitions);
children!(CreateTableStmt => Cols, Constraints, Options, Partition, SplitIndex, Select);
children!(CreateViewStmt => Select);
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
children!(AlterTableSpec => NewColumns, SplitIndex, PartitionExpr, Options, Constraint, MaskingPolicyExpr, PartDefinitions, Partition);
children!(ReferenceDef => IndexPartSpecifications);
children!(Constraint => Keys, Option, Refer, Expr);
children!(AlterTableStmt => Specs);
children!(DropTableStmt => );
children!(CreateDatabaseStmt => );
children!(DropDatabaseStmt => );
children!(AlterDatabaseStmt => );
children!(AlterInstanceStmt => );
children!(AlterRangeStmt => );
children!(StringOrUserVar => UserVar);
children!(CreateBindingStmt => OriginNode, HintedNode, PlanDigests);
children!(DropBindingStmt => OriginNode, HintedNode, SQLDigests);
children!(SetBindingStmt => OriginNode, HintedNode);
children!(DistributeTableStmt => );
children!(CancelDistributionJobStmt => );
children!(RenameUserStmt => );
children!(DropUserStmt => );
children!(DropProcedureStmt => );
children!(DropPlacementPolicyStmt => );
children!(DropResourceGroupStmt => );
children!(CreateResourceGroupStmt => );
children!(AlterResourceGroupStmt => );
children!(CreatePlacementPolicyStmt => );
children!(AlterPlacementPolicyStmt => );
children!(DropQueryWatchStmt => GroupNameExpr);
children!(ImportIntoActionStmt => );
children!(CreateSequenceStmt => );
children!(AlterSequenceStmt => );
children!(DropSequenceStmt => );
children!(TruncateTableStmt => );
children!(RecoverTableStmt => );
children!(FlashBackToTimestampStmt => FlashbackTS);
children!(FlashBackTableStmt => );
children!(FlashBackDatabaseStmt => );
children!(IndexPartSpecification => Expr);
children!(SplitOption => Lower, Upper, ValueLists);
children!(SplitRegionStmt => SplitOpt);
children!(SplitIndexOption => SplitOpt);
children!(IndexOption => SplitOpt, Condition);
children!(CreateIndexStmt => IndexPartSpecifications, Option);
children!(DropIndexStmt => );
children!(RenameTableStmt => );
children!(AnalyzeOpt => Value);
children!(AnalyzeTableStmt => AnalyzeOpts);
children!(CompactTableStmt => );
children!(OptimizeTableStmt => );
children!(KillStmt => Expr);
children!(LoadStatsStmt => );
children!(LockStatsStmt => );
children!(UnlockStatsStmt => );
children!(DropStatsStmt => );
children!(RefreshStatsStmt => );
children!(FlushStmt => );
children!(LockTablesStmt => );
children!(UseStmt => );
children!(SetStmt => Variables);
children!(VariableAssignment => Value, ExtendValue);
children!(BeginStmt => AsOf);
children!(BinlogStmt => );
children!(DeallocateStmt => );
children!(PrepareStmt => );
children!(ExecuteStmt => UsingVars);
children!(HelpStmt => );
children!(SavepointStmt => );
children!(ReleaseSavepointStmt => );
children!(RecommendIndexStmt => Options);
children!(RecommendIndexOption => Value);
children!(AsOfClause => TsExpr);
children!(PlanReplayerStmt => Stmt, Where, OrderBy, Limit, HistoricalStatsInfo);
children!(TrafficOption => FloatValue);
children!(TrafficStmt => Options);
children!(DropStatisticsStmt => );
children!(CreateStatisticsStmt => );
children!(SetPwdStmt => );
children!(SetSessionStatesStmt => );
children!(SetConfigStmt => Value);
children!(SetResourceGroupStmt => );
children!(SetRoleStmt => );
children!(SetDefaultRoleStmt => );
children!(CreateUserStmt => );
children!(AlterUserStmt => );
children!(GrantStmt => );
children!(GrantProxyStmt => );
children!(GrantRoleStmt => );
children!(RevokeStmt => );
children!(RevokeRoleStmt => );
children!(LoadDataOpt => Value);
children!(LoadDataStmt => ColumnsAndUserVars, ColumnAssignments, Options);
children!(ImportIntoStmt => ColumnsAndUserVars, ColumnAssignments, Select, Options);
children!(NonTransactionalDMLStmt => DMLStmt);
children!(CreateMaskingPolicyStmt => Expr);
children!(DynamicCalibrateResourceOption => Ts);
children!(CalibrateResourceStmt => DynamicCalibrateResourceOptionList);
children!(QueryWatchResourceGroupOption => GroupNameExpr);
children!(QueryWatchTextOption => PatternExpr);
children!(QueryWatchOption => ResourceGroupOption, TextOption);
children!(AddQueryWatchStmt => QueryWatchOptionList);
children!(ProcedureDecl => DeclDefault);
children!(ProcedureOpenCur => );
children!(ProcedureCloseCur => );
children!(ProcedureFetchInto => );
children!(ProcedureErrorCon => );
children!(ProcedureErrorVal => );
children!(ProcedureErrorState => );
children!(ProcedureCursor => Selectstring);
children!(ProcedureErrorControl => Operate);
children!(ProcedureBlock => ProcedureProcStmts);
children!(ProcedureIfInfo => IfBody);
children!(ProcedureIfBlock => IfExpr, ProcedureIfStmts, ProcedureElseStmt);
children!(ProcedureElseIfBlock => ProcedureIfStmt);
children!(ProcedureElseBlock => ProcedureIfStmts);
children!(SimpleWhenThenStmt => Expr, ProcedureStmts);
children!(SearchWhenThenStmt => Expr, ProcedureStmts);
children!(SimpleCaseStmt => Condition, WhenCases, ElseCases);
children!(SearchCaseStmt => WhenCases, ElseCases);
children!(ProcedureWhileStmt => Condition, Body);
children!(ProcedureRepeatStmt => Body, Condition);
children!(ProcedureLabelBlock => Block);
children!(ProcedureLabelLoop => Block);
children!(ProcedureJump => );
children!(ProcedureInfo => ProcedureBody);
children!(CommitStmt => );
children!(RollbackStmt => );
children!(RowExpr => Values);
children!(FrameBound => Expr);
children!(FrameExtent => Start, End);
children!(FrameClause => Extent);
children!(WindowSpec => PartitionBy, OrderBy, Frame);
children!(SelectStmt => From, Where, Fields, GroupBy, Having, OrderBy, Limit, With, children, Lists, WindowSpecs);
children!(ExplainStmt => stmt);
children!(ExplainForStmt => );
children!(CommonTableExpression => Query);
children!(WithClause => CTEs);
children!(SetOprSelectList => selects, With, OrderBy, Limit);
children!(SetOprStmt => select_list, OrderBy, Limit, With);
children!(AlterJobOption => Value);
children!(AdminStmt => where_expr, alter_job_options);
children!(CleanupTableLockStmt => );
children!(RepairTableStmt => CreateStmt);
children!(TraceStmt => Stmt);
children!(BRIEStmt => );
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
    ByItem,
    ColumnDef,
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
    ProcedureDecl,
    QueryWatchOption,
    QueryWatchResourceGroupOption,
    QueryWatchTextOption,
    RecommendIndexOption,
    ReferenceDef,
    ResultSetNode,
    RowExpr,
    SelectField,
    SplitIndexOption,
    SplitOption,
    StringOrUserVar,
    SubPartitionDefinition,
    TableOption,
    TableRefsClause,
    TableSample,
    TableSource,
    TrafficOption,
    VariableAssignment,
    WhenClause,
    WindowSpec,
    WithClause
);

children!(RestartStmt => );

children!(ShutdownStmt => );

children!(UnlockTablesStmt => );
