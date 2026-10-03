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

// 逻辑数据源（DataSource）算子：查询计划中未解析的表叶节点。
//
// 承载表元数据、谓词、访问路径（AccessPath）与统计信息；谓词下推后
// 由 Ranger 推导 Range，并可转换为 Table/Index Gather 物理候选。

use crate::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// DataSource 的引用计数可变句柄，便于 Gather 与路径枚举共享同一叶节点。
pub type DataSourceRef = Rc<RefCell<DataSource>>;

/// Build the pseudo histogram collection that Go derives from a pseudo table.
///
/// A bare `PseudoHistColl` has no columns, which makes `Selectivity` fall
/// back to one coarse factor for every predicate.  Go's `PseudoTable` records
/// metadata for every public column before remapping it to plan column IDs.
/// Keep that metadata here so pseudo range/IN/DNF estimates follow the same
/// code path even before real statistics are available.
/// 构造与 Go 伪表一致的列元数据直方图，供伪统计选择率估算使用。
pub fn BuildPseudoHistColl(
    table_info: &model::TableInfo,
    physical_table_id: i64,
    columns: &[Column],
) -> statistics::HistColl {
    let mut histogram = statistics::PseudoHistColl(physical_table_id, true);
    for table_column in &table_info.Columns {
        let Some(column) = columns.iter().find(|column| column.ID == table_column.ID) else {
            continue;
        };
        let info = statistics::ColumnInfo {
            ID: table_column.ID,
            Name: table_column.Name.O.clone(),
            FieldType: table_column.FieldType.clone(),
            IsPrimaryKey: expression::mysql::HasPriKeyFlag(table_column.GetFlag()),
        };
        let mut column_stats =
            statistics::EmptyColumn(physical_table_id, table_info.PKIsHandle, info);
        column_stats.Histogram =
            statistics::NewPseudoHistogram(table_column.ID, &table_column.FieldType);
        histogram
            .Columns
            .insert(column.UniqueID, Box::new(column_stats));
        histogram
            .UniqueID2colInfoID
            .insert(column.UniqueID, table_column.ID);
    }
    histogram
}

/// Estimate pseudo-table filters with the same per-column range factors used
/// by Go's ranger/cardinality path. Multiple bounds on one column form one
/// range; independent columns multiply.
fn pseudo_filter_selectivity(
    histogram: &statistics::HistColl,
    conditions: &[expression::ExprBox],
    default_factor: f64,
) -> f64 {
    #[derive(Default)]
    struct Constraint {
        factor: Option<f64>,
        lower: bool,
        upper: bool,
        excludes_null: bool,
    }

    let mut constraints = HashMap::<i64, Constraint>::new();
    for condition in conditions {
        let Some(function) = condition.as_scalar_function() else {
            continue;
        };
        // Go's pseudoSelectivity recognizes only a direct Column/Constant
        // constraint. A column nested under SUBSTRING (or another scalar
        // function) is not an indexable point/range and falls back to the
        // session SelectionFactor instead of being charged as `IN/1000`.
        let column = match function.FuncName.L.as_str() {
            parser_ast::In => function.GetArgs().first().and_then(|arg| arg.as_column()),
            parser_ast::EQ
            | parser_ast::NullEQ
            | parser_ast::GE
            | parser_ast::GT
            | parser_ast::LE
            | parser_ast::LT
                if function
                    .GetArgs()
                    .iter()
                    .any(|arg| arg.as_constant().is_some()) =>
            {
                function.GetArgs().iter().find_map(|arg| arg.as_column())
            }
            _ => None,
        };
        let Some(column) = column else {
            continue;
        };
        let entry = constraints.entry(column.UniqueID).or_default();
        let factor = match function.FuncName.L.as_str() {
            parser_ast::EQ | parser_ast::NullEQ => Some(1.0 / 1_000.0),
            parser_ast::In if function.GetArgs().len() > 1 => {
                Some((function.GetArgs().len() - 1) as f64 / 1_000.0)
            }
            "or" => Some(function.GetArgs().len() as f64 / 1_000.0),
            parser_ast::GE | parser_ast::GT => {
                entry.lower = true;
                entry.excludes_null |= function
                    .GetArgs()
                    .first()
                    .is_some_and(|argument| argument.as_constant().is_some());
                None
            }
            parser_ast::LE | parser_ast::LT => {
                entry.upper = true;
                entry.excludes_null |= function
                    .GetArgs()
                    .first()
                    .is_some_and(|argument| argument.as_column().is_some());
                None
            }
            // NOT LIKE and other negated expressions are not point ranges.
            // Go falls back to SelectionFactor for these pseudo predicates.
            "not" => None,
            _ => None,
        };
        if let Some(factor) = factor {
            entry.factor = Some(entry.factor.map_or(factor, |current| current.min(factor)));
        }
    }
    let mut recognized = false;
    let selectivity = constraints.values().fold(1.0, |selectivity, constraint| {
        let range_factor = match (constraint.lower, constraint.upper) {
            (true, true) => Some(1.0 / 40.0),
            (true, false) | (false, true) => Some(
                1.0 / 3.0
                    - if constraint.excludes_null {
                        1.0 / 1_000.0
                    } else {
                        0.0
                    },
            ),
            _ => None,
        };
        let factor = match (constraint.factor, range_factor) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        if let Some(factor) = factor {
            recognized = true;
            selectivity * factor
        } else {
            selectivity
        }
    });
    if recognized {
        selectivity
    } else {
        default_factor
    }
}

/// Installs the runtime constant-folding boundary used by predicate pushdown.
/// Full equivalence-class simplification remains rule-owned, while this hook
/// preserves Go's required constant folding and true-predicate elimination.
/// 安装谓词下推使用的运行时常量折叠边界；完整等价类简化仍由规则拥有，
/// 本钩子保留 Go 所需的常量折叠与真谓词消除。
pub fn InstallPredicateSimplificationPassthrough() {
    let _ = rule_util::RegisterApplyPredicateSimplification(
        |context, predicates, propagate_constant, filter| {
            let predicates = if propagate_constant {
                expression::PropagateConstantRef(context.GetExprCtx(), filter, predicates)
            } else {
                predicates
            };
            let folded = predicates
                .into_iter()
                .flat_map(|predicate| expression::SplitCNFItems(predicate.as_ref()))
                .map(|predicate| {
                    fold_runtime_predicate(expression::FoldConstant(
                        context.GetExprCtx(),
                        predicate,
                    ))
                })
                .filter(|predicate| !constant_is_true(predicate.as_ref()))
                .collect::<Vec<_>>();
            let mut equalities = HashMap::<i64, types::datum::Datum>::new();
            for predicate in &folded {
                if constant_is_false(predicate.as_ref()) {
                    return vec![Box::new(expression::NewZero()) as Expression];
                }
                if let Some((column_id, value)) = equality_column_constant(predicate.as_ref())
                    && equalities
                        .insert(column_id, value.clone())
                        .is_some_and(|previous| !previous.Equals(value))
                {
                    return vec![Box::new(expression::NewZero()) as Expression];
                }
            }
            // CNF absorption: `A AND (A OR B)` is equivalent to `A`. Go's
            // predicate simplification applies this before DataSource range
            // derivation, so the redundant DNF must not survive as an extra
            // pushed-down Selection condition.
            let evaluation_context = context.GetExprCtx().GetEvalCtx();
            let redundant = folded
                .iter()
                .enumerate()
                .map(|(candidate_index, candidate)| {
                    candidate
                        .as_scalar_function()
                        .filter(|function| function.FuncName.L == expression::ast::LogicOr)
                        .is_some_and(|function| {
                            folded
                                .iter()
                                .enumerate()
                                .any(|(predicate_index, predicate)| {
                                    predicate_index != candidate_index
                                        && function.GetArgs().iter().any(|branch| {
                                            branch.Equal(evaluation_context, predicate.as_ref())
                                        })
                                })
                        })
                })
                .collect::<Vec<_>>();
            folded
                .into_iter()
                .enumerate()
                .filter_map(|(index, predicate)| (!redundant[index]).then_some(predicate))
                .collect()
        },
    );
    let _ = rule_util::RegisterApplyPredicateSimplificationForJoin(
        |context, predicates, schema1, schema2, propagate_constant, filter| {
            let predicates = if propagate_constant {
                let keep = context
                    .GetSessionVars()
                    .GetSystemVar(vardef::TiDBOptAlwaysKeepJoinKey)
                    .map_or(vardef::DefOptAlwaysKeepJoinKey, |value| {
                        matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                    });
                expression::PropagateConstantForJoinRef(
                    context.GetExprCtx(),
                    keep,
                    schema1.Clone(),
                    schema2.Clone(),
                    filter,
                    predicates,
                )
            } else {
                predicates
            };
            rule_util::ApplyPredicateSimplification(context, predicates, false, filter)
        },
    );
}

/// 识别 `列 = 非常量NULL常量` 形式，返回 (列 UniqueID, 常量 Datum)。
fn equality_column_constant(
    expression: &dyn expression::Expression,
) -> Option<(i64, &types::datum::Datum)> {
    let function = expression.as_scalar_function()?;
    if function.FuncName.L != parser_ast::EQ || function.GetArgs().len() != 2 {
        return None;
    }
    let args = function.GetArgs();
    if let (Some(column), Some(constant)) = (args[0].as_column(), args[1].as_constant()) {
        return (!constant.Value.IsNull()).then_some((column.UniqueID, &constant.Value));
    }
    let (Some(constant), Some(column)) = (args[0].as_constant(), args[1].as_column()) else {
        return None;
    };
    (!constant.Value.IsNull()).then_some((column.UniqueID, &constant.Value))
}

/// 将两侧均为非常量 NULL 常量的 EQ/NE 折叠为 true/false 常量。
fn fold_runtime_predicate(predicate: Expression) -> Expression {
    let Some(function) = predicate.as_scalar_function() else {
        return predicate;
    };
    if !matches!(
        function.FuncName.L.as_str(),
        parser_ast::EQ | parser_ast::NE
    ) || function.GetArgs().len() != 2
    {
        return predicate;
    }
    let (Some(left), Some(right)) = (
        function.GetArgs()[0].as_constant(),
        function.GetArgs()[1].as_constant(),
    ) else {
        return predicate;
    };
    if left.Value.IsNull() || right.Value.IsNull() {
        return predicate;
    }
    let equal = left.Value.Equals(&right.Value);
    if equal == (function.FuncName.L == parser_ast::EQ) {
        Box::new(expression::NewOne())
    } else {
        Box::new(expression::NewZero())
    }
}

/// 判断表达式是否为可求值的真常量（排除延迟表达式与参数标记）。
fn constant_is_true(expression: &dyn expression::Expression) -> bool {
    let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() else {
        return false;
    };
    if constant.DeferredExpr.is_some() || constant.ParamMarker.is_some() || constant.Value.IsNull()
    {
        return false;
    }
    match constant.Value.Kind() {
        types::datum::KindInt64 => constant.Value.GetInt64() != 0,
        types::datum::KindUint64 => constant.Value.GetUint64() != 0,
        types::datum::KindFloat32 | types::datum::KindFloat64 => constant.Value.GetFloat64() != 0.0,
        _ => false,
    }
}

/// 判断表达式是否为可求值的假常量。
fn constant_is_false(expression: &dyn expression::Expression) -> bool {
    let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() else {
        return false;
    };
    if constant.DeferredExpr.is_some() || constant.ParamMarker.is_some() || constant.Value.IsNull()
    {
        return false;
    }
    match constant.Value.Kind() {
        types::datum::KindInt64 => constant.Value.GetInt64() == 0,
        types::datum::KindUint64 => constant.Value.GetUint64() == 0,
        types::datum::KindFloat32 | types::datum::KindFloat64 => constant.Value.GetFloat64() == 0.0,
        _ => false,
    }
}

/// 将 PlanContext 适配为基数估计（Cardinality）所需的上下文接口。
struct CardinalityContextAdapter<'a>(&'a dyn base::PlanContext);

impl cardinality::CardinalityContext for CardinalityContextAdapter<'_> {
    fn GetSessionVars(&self) -> &cardinality::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn cardinality::expression::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

#[derive(Clone, Default)]
/// 可能的物理有序性及是否具备 TiFlash 副本。
pub struct PossiblePropertiesInfo {
    pub Orders: Vec<Vec<Column>>,
    pub HasTiFlash: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 句柄列被索引覆盖的程度。
pub enum HandleCoverState {
    NotCovered,
    Covered,
    CoveredByPrefix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 全文检索（FTS）查询是否需要相关性分数。
pub enum FTSQueryType {
    NoScore,
    WithScore,
}

#[derive(Clone, Debug)]
/// 全文检索下推查询描述。
pub struct FTSQueryInfo {
    pub IndexID: i64,
    pub ColumnID: i64,
    pub ColumnName: String,
    pub QueryText: String,
    pub QueryTokenizer: String,
    pub QueryType: FTSQueryType,
    pub TopK: u32,
}

#[derive(Clone, Debug)]
/// 绑定到具体全文索引的 FTS 下推信息。
pub struct FTSPushDown {
    pub IndexInfo: model::IndexInfo,
    pub QueryInfo: FTSQueryInfo,
}

/// Unresolved table leaf.  Storage/statistics services populate this object;
/// logical transformations only consume the owned metadata below.
/// 未解析的表叶节点。存储/统计服务填充本对象；逻辑变换只消费下方元数据。
pub struct DataSource {
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    pub TableInfo: model::TableInfo,
    pub Columns: Vec<model::ColumnInfo>,
    pub DBName: parser_ast::CIStr,
    pub TableAsName: Option<parser_ast::CIStr>,
    pub PushedDownConds: Vec<Expression>,
    pub AllConds: Vec<Expression>,
    pub TableStats: StatsInfo,
    pub AllPossibleAccessPaths: Vec<planner_util::AccessPath>,
    pub PossibleAccessPaths: Vec<planner_util::AccessPath>,
    pub PartitionDefIdx: Option<usize>,
    pub PhysicalTableID: i64,
    pub PartitionNames: Vec<parser_ast::CIStr>,
    pub HandleCols: Option<Box<dyn HandleCols>>,
    pub UnMutableHandleCols: Option<Box<dyn HandleCols>>,
    pub TblCols: Vec<Column>,
    pub TblColsByID: HashMap<i64, Column>,
    pub CommonHandleCols: Vec<Column>,
    pub CommonHandleLens: Vec<isize>,
    pub PreferStoreType: i32,
    pub IsForUpdateRead: bool,
    pub ContainExprPrefixUk: bool,
    pub ColsRequiringFullLen: Option<Vec<Column>>,
    /// MPP 列裁剪后由数据源顶层暴露的父级列顺序（允许重复列）。
    pub PrunedOutputColumns: Option<Vec<Column>>,
    pub AccessPathMinSelectivity: f64,
    pub AskedColumnGroup: Vec<Vec<Column>>,
    pub InterestingColumns: Vec<Column>,
    /// Parsed USE_INDEX_MERGE names; an empty entry denotes a general hint.
    pub IndexMergeHints: Vec<Vec<String>>,
    pub FtsPushDown: Option<FTSPushDown>,
}

/// 默认空数据源。
impl Default for DataSource {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            TableInfo: model::TableInfo::default(),
            Columns: Vec::new(),
            DBName: parser_ast::CIStr::default(),
            TableAsName: None,
            PushedDownConds: Vec::new(),
            AllConds: Vec::new(),
            TableStats: StatsInfo::default(),
            AllPossibleAccessPaths: Vec::new(),
            PossibleAccessPaths: Vec::new(),
            PartitionDefIdx: None,
            PhysicalTableID: 0,
            PartitionNames: Vec::new(),
            HandleCols: None,
            UnMutableHandleCols: None,
            TblCols: Vec::new(),
            TblColsByID: HashMap::new(),
            CommonHandleCols: Vec::new(),
            CommonHandleLens: Vec::new(),
            PreferStoreType: 0,
            IsForUpdateRead: false,
            ContainExprPrefixUk: false,
            ColsRequiringFullLen: None,
            PrunedOutputColumns: None,
            AccessPathMinSelectivity: 0.0,
            AskedColumnGroup: Vec::new(),
            InterestingColumns: Vec::new(),
            IndexMergeHints: Vec::new(),
            FtsPushDown: None,
        }
    }
}

impl DataSource {
    /// Apply Go READ_FROM_STORAGE(TIFLASH[...]) semantics after provider path
    /// population so the forced engine reaches physical enumeration.
    /// 在路径填充后强制 READ_FROM_STORAGE(TIFLASH)：仅保留表路径并设 StoreType。
    pub fn ForceTiFlashPath(&mut self) {
        for paths in [
            &mut self.AllPossibleAccessPaths,
            &mut self.PossibleAccessPaths,
        ] {
            paths.retain(|path| path.IsTablePath());
            for path in paths.iter_mut() {
                path.StoreType = kv::StoreType::TiFlash;
            }
        }
    }

    /// Apply Go READ_FROM_STORAGE(TIKV[...]) semantics after provider path
    /// population by excluding TiFlash alternatives from physical enumeration.
    pub fn ForceTiKVPath(&mut self) {
        for paths in [
            &mut self.AllPossibleAccessPaths,
            &mut self.PossibleAccessPaths,
        ] {
            paths.retain(|path| path.StoreType != kv::StoreType::TiFlash);
        }
    }

    /// Whether READ_FROM_STORAGE selects TiKV without also selecting TiFlash.
    /// READ_FROM_STORAGE 是否仅选择 TiKV、未同时选择 TiFlash。
    pub fn PrefersTiKVOnly(&self) -> bool {
        self.PreferStoreType & hint::PreferTiKV as i32 != 0
            && self.PreferStoreType & hint::PreferTiFlash as i32 == 0
    }

    /// 按 AllConds 为每条访问路径拆分 Access/Filter，构建 Range 并估计行数。
    pub(crate) fn deriveAccessPathsFromPredicates(&mut self) -> Result<()> {
        let Some(context) = self.SCtx().cloned() else {
            return Ok(());
        };
        let cardinality_context = CardinalityContextAdapter(context.as_ref());
        let conditions = self.AllConds.clone();
        let eval = context.GetExprCtx().GetEvalCtx();
        let range_conditions = conditions
            .iter()
            .map(|condition| {
                let Some(function) = condition.as_scalar_function().filter(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast::EQ
                            | parser_ast::NullEQ
                            | parser_ast::LT
                            | parser_ast::LE
                            | parser_ast::GT
                            | parser_ast::GE
                    ) && function.GetArgs().len() == 2
                }) else {
                    return condition.CloneExpr();
                };
                let mut normalized = function.clone_scalar();
                let mut changed = false;
                for argument in normalized.GetArgsMut() {
                    let Some(cast) = argument.as_scalar_function().filter(|cast| {
                        cast.FuncName.L == expression::ast::Cast
                            && cast.GetType(eval).GetType() == expression::mysql::TypeDatetime
                            && cast.GetArgs().len() == 1
                    }) else {
                        continue;
                    };
                    let source = &cast.GetArgs()[0];
                    if source.as_column().is_some()
                        && source.GetType(eval).GetType() == expression::mysql::TypeTimestamp
                    {
                        *argument = source.CloneExpr();
                        changed = true;
                    }
                }
                if !changed {
                    return condition.CloneExpr();
                }
                normalized.CleanHashCode();
                Box::new(normalized) as Expression
            })
            .collect::<Vec<_>>();
        self.AllConds = range_conditions.clone();
        let histogram = (self.TableStats.StatsVersion > 0)
            .then(|| self.TableStats.HistColl.as_deref())
            .flatten()
            .and_then(|histogram| histogram.downcast_ref::<statistics::HistColl>());
        let schema_columns = self.Schema().Columns.clone();
        let all_table_schema_columns = self.TblCols.clone();
        let table_columns = self.TableInfo.Columns.clone();
        let required_full_length = self.ColsRequiringFullLen.clone().unwrap_or_default();
        let base_row_count = histogram
            .map_or(self.TableStats.RowCount, |histogram| {
                histogram.RealtimeCount as f64
            })
            .max(1.0);
        let handle_column = self
            .HandleCols
            .as_ref()
            .and_then(|handle| handle.GetCol(0))
            .cloned();
        let handle_columns = self
            .HandleCols
            .as_ref()
            .map(|handle| handle.IterColumns().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let integer_handle = self.GetPKIsHandleCol();
        let mut appended_handles = Vec::new();
        for path in &mut self.PossibleAccessPaths {
            if path.IsTablePath() {
                let Some(handle) = handle_column.as_ref() else {
                    path.TableFilters = conditions.clone();
                    continue;
                };
                let (mut access, mut residual) = ranger::DetachCondsForColumn(
                    context.GetRangerCtx(),
                    range_conditions.clone(),
                    handle.Clone(),
                );
                let Some(field_type) = handle.RetType.as_ref() else {
                    continue;
                };
                // Go's deriveTablePathStats treats `pk = correlated-column`
                // as a parameterized point access even though the ordinary
                // ranger leaves it in TableFilters.
                // Go：`pk = 关联列` 视为参数化点查，即便 Ranger 常留在 TableFilters。
                if access.is_empty() {
                    let eval = context.GetExprCtx().GetEvalCtx();
                    let correlated_offset = residual.iter().position(|condition| {
                        let Some(function) = condition.as_scalar_function().filter(|function| {
                            function.FuncName.L == parser_ast::EQ && function.GetArgs().len() == 2
                        }) else {
                            return false;
                        };
                        let arguments = function.GetArgs();
                        arguments[0]
                            .as_column()
                            .is_some_and(|column| column.Equal(eval, handle))
                            && arguments[1].as_correlated_column().is_some()
                            || arguments[1]
                                .as_column()
                                .is_some_and(|column| column.Equal(eval, handle))
                                && arguments[0].as_correlated_column().is_some()
                    });
                    if let Some(offset) = correlated_offset {
                        access.push(residual.remove(offset));
                        path.Ranges = ranger::FullIntRange(expression::mysql::HasUnsignedFlag(
                            field_type.GetFlag(),
                        ))
                        .0;
                        path.AccessConds = access;
                        path.TableFilters = residual;
                        path.CountAfterAccess = 1.0;
                        path.MinCountAfterAccess = 1.0;
                        path.MaxCountAfterAccess = 1.0;
                        path.CountAfterIndex = 1.0;
                        continue;
                    }
                }
                if access.is_empty() {
                    path.AccessConds.clear();
                    path.TableFilters = residual;
                    path.CountAfterAccess = base_row_count;
                    path.MinCountAfterAccess = base_row_count;
                    path.MaxCountAfterAccess = base_row_count;
                    path.CountAfterIndex = base_row_count;
                    continue;
                }
                let mut ranger_context = context.GetRangerCtx().clone();
                let (ranges, access, range_residual) =
                    ranger::BuildTableRange(access, &mut ranger_context, field_type, 0)
                        .map_err(|error| PlannerError(error.to_string()))?;
                residual.extend(range_residual);
                path.Ranges = ranges.0;
                path.AccessConds = access;
                path.TableFilters = residual;
                if let Some(histogram) = histogram {
                    let range_refs = path.Ranges.iter().collect::<Vec<_>>();
                    let estimate = cardinality::GetRowCountByColumnRanges(
                        &cardinality_context,
                        histogram,
                        handle.UniqueID,
                        &range_refs,
                        self.TableInfo.PKIsHandle,
                    )
                    .map_err(|error| PlannerError(error.to_string()))?;
                    path.CountAfterAccess = estimate.Est;
                    path.MinCountAfterAccess = estimate.MinEst;
                    path.MaxCountAfterAccess = estimate.MaxEst;
                    path.CountAfterIndex = estimate.Est;
                } else if !path.AccessConds.is_empty() {
                    let equality = path.AccessConds.iter().any(|condition| {
                        condition.as_scalar_function().is_some_and(|function| {
                            matches!(
                                function.FuncName.L.as_str(),
                                parser_ast::EQ
                                    | parser_ast::NullEQ
                                    | parser_ast::In
                                    | parser_ast::IsNull
                            )
                        })
                    });
                    let estimate = if equality {
                        (base_row_count / 1_000.0).max(1.0)
                    } else {
                        (base_row_count / 3.0).max(1.0)
                    };
                    path.CountAfterAccess = estimate;
                    path.MinCountAfterAccess = estimate;
                    path.MaxCountAfterAccess = estimate;
                    path.CountAfterIndex = estimate;
                }
                continue;
            }

            // 索引路径：按索引列拆条件、建 Range，并区分 IndexFilters/TableFilters。
            let Some(index) = path.Index.as_ref() else {
                continue;
            };
            let mut index_columns = index
                .Columns
                .iter()
                .filter_map(|index_column| {
                    table_columns.get(index_column.Offset as usize).map(|info| {
                        schema_columns
                            .iter()
                            .find(|column| column.ID == info.ID)
                            .or_else(|| {
                                all_table_schema_columns
                                    .iter()
                                    .find(|column| column.ID == info.ID)
                            })
                            .cloned()
                            .unwrap_or_else(|| {
                                let mut column = Column::new(
                                    info.FieldType.clone(),
                                    info.ID,
                                    context.GetSessionVars().AllocPlanColumnID(),
                                    index_column.Offset,
                                );
                                column.OrigName = info.Name.O.clone();
                                column
                            })
                    })
                })
                .collect::<Vec<_>>();
            if index_columns.is_empty() {
                path.TableFilters = conditions.clone();
                continue;
            }
            let declared_col_count = index_columns.len();
            let mut index_lengths = index
                .Columns
                .iter()
                .take(index_columns.len())
                .map(|column| column.Length as i32)
                .collect::<Vec<_>>();
            let declared_columns = index_columns.iter().cloned().map(Some).collect::<Vec<_>>();
            let (append_columns, append_lengths) = handle_cols_to_append(
                &self.TableInfo,
                &self.CommonHandleCols,
                &self.CommonHandleLens,
                integer_handle.as_ref(),
                path,
                &declared_columns,
            );
            if !append_columns.is_empty() {
                appended_handles.push((
                    index.ID,
                    index.Columns.len(),
                    append_columns
                        .iter()
                        .map(|column| column.UniqueID)
                        .collect::<Vec<_>>(),
                ));
                index_columns.extend(append_columns);
                index_lengths.extend(append_lengths.into_iter().map(|length| length as i32));
            }
            path.IdxCols = index_columns.clone();
            path.IdxColLens = index_lengths
                .iter()
                .map(|length| *length as isize)
                .collect();
            path.FullIdxCols = index_columns.iter().cloned().map(Some).collect();
            path.FullIdxColLens = path.IdxColLens.clone();
            let detached = ranger::DetachCondAndBuildRangeForIndex(
                context.GetRangerCtx(),
                range_conditions.clone(),
                index_columns.clone(),
                index_lengths.clone(),
                0,
            )
            .map_err(|error| PlannerError(error.to_string()))?;
            path.Ranges = detached.Ranges.0;
            path.AccessConds = detached.AccessConds;
            let (index_filters, table_filters): (Vec<_>, Vec<_>) =
                detached.RemainedConds.into_iter().partition(|condition| {
                    expression::ExtractColumns(condition.as_ref())
                        .into_iter()
                        .all(|column| {
                            isIndexColsCoveringCol(column, &index_columns, &path.IdxColLens, false)
                                || handle_columns
                                    .iter()
                                    .any(|handle| handle.UniqueID == column.UniqueID)
                        })
                });
            path.IndexFilters = index_filters;
            path.TableFilters = table_filters;
            path.EqCondCount = detached.EqCondCount;
            path.EqOrInCondCount = detached.EqOrInCount;
            if path.AccessConds.is_empty() {
                let first_unique_id = index_columns[0].UniqueID;
                let access = range_conditions
                    .iter()
                    .filter(|condition| {
                        let columns = expression::ExtractColumns(condition.as_ref());
                        !columns.is_empty()
                            && columns
                                .iter()
                                .all(|column| column.UniqueID == first_unique_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !access.is_empty() {
                    let mut ranger_context = context.GetRangerCtx().clone();
                    let (ranges, built_access, remained) = ranger::BuildColumnRange(
                        access,
                        &mut ranger_context,
                        index_columns[0].RetType.as_ref().ok_or_else(|| {
                            PlannerError("index column type is required".to_owned())
                        })?,
                        index_lengths[0],
                        0,
                    )
                    .map_err(|error| PlannerError(error.to_string()))?;
                    path.EqCondCount = built_access
                        .iter()
                        .filter(|condition| {
                            condition
                                .as_scalar_function()
                                .is_some_and(|function| function.FuncName.L == parser_ast::EQ)
                        })
                        .count();
                    path.EqOrInCondCount = path.EqCondCount;
                    path.Ranges = ranges.0;
                    path.AccessConds = built_access;
                    // BuildColumnRange sees only the leading-column candidates.
                    // Keep every other predicate from the index detacher, and
                    // drop an access predicate only when a full-length range
                    // absorbed it without returning it as a residual.
                    let mut filters = path
                        .IndexFilters
                        .iter()
                        .chain(path.TableFilters.iter())
                        .cloned()
                        .collect::<Vec<_>>();
                    filters.extend(remained.iter().cloned());
                    if index_lengths[0] == expression::types::UnspecifiedLength {
                        filters.retain(|filter| {
                            !path.AccessConds.iter().any(|access| {
                                access.Equal(eval, filter.as_ref())
                                    && !remained
                                        .iter()
                                        .any(|residual| residual.Equal(eval, filter.as_ref()))
                            })
                        });
                    }
                    let mut unique_filters = Vec::<Expression>::new();
                    for filter in filters {
                        if !unique_filters
                            .iter()
                            .any(|existing| existing.Equal(eval, filter.as_ref()))
                        {
                            unique_filters.push(filter);
                        }
                    }
                    let (index_filters, table_filters): (Vec<_>, Vec<_>) =
                        unique_filters.into_iter().partition(|condition| {
                            expression::ExtractColumns(condition.as_ref())
                                .into_iter()
                                .all(|column| {
                                    isIndexColsCoveringCol(
                                        column,
                                        &index_columns,
                                        &path.IdxColLens,
                                        false,
                                    ) || handle_columns
                                        .iter()
                                        .any(|handle| handle.UniqueID == column.UniqueID)
                                })
                        });
                    path.IndexFilters = index_filters;
                    path.TableFilters = table_filters;
                }
            }
            if let Some(is_null) = range_conditions.iter().find(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    function.FuncName.L == parser_ast::IsNull
                        && function.GetArgs().len() == 1
                        && function.GetArgs()[0]
                            .as_column()
                            .is_some_and(|column| column.UniqueID == index_columns[0].UniqueID)
                })
            }) {
                if path.AccessConds.is_empty() {
                    path.Ranges = ranger::NullRange().0;
                    path.AccessConds = vec![is_null.CloneExpr()];
                    path.EqCondCount = 1;
                    path.EqOrInCondCount = 1;
                }
                let eval = context.GetExprCtx().GetEvalCtx();
                if path
                    .AccessConds
                    .iter()
                    .any(|access| access.Equal(eval, is_null.as_ref()))
                {
                    // NULL has a dedicated index encoding, so even a prefix
                    // index can verify IS NULL without a second table filter.
                    // NULL 使用独立索引编码，前缀索引也无需回表重复校验。
                    path.TableFilters
                        .retain(|filter| !filter.Equal(eval, is_null.as_ref()));
                    if index_lengths[0] == expression::types::UnspecifiedLength {
                        path.IndexFilters
                            .retain(|filter| !filter.Equal(eval, is_null.as_ref()));
                    } else if !path
                        .IndexFilters
                        .iter()
                        .any(|filter| filter.Equal(eval, is_null.as_ref()))
                    {
                        path.IndexFilters.push(is_null.CloneExpr());
                    }
                }
            }
            if path.EqOrInCondCount == path.AccessConds.len() {
                let (correlated_access, remained) =
                    path.SplitCorColAccessCondFromFilters(context.as_ref(), path.EqOrInCondCount);
                path.AccessConds.extend(correlated_access);
                path.TableFilters = remained;
            }
            path.IsSingleScan = required_full_length.iter().all(|required| {
                index_columns
                    .iter()
                    .any(|index_column| index_column.UniqueID == required.UniqueID)
            });
            if let Some(histogram) = histogram {
                let need_prune = index_columns.len() > declared_col_count
                    && path.Ranges.iter().any(|range| {
                        range.LowVal.len() > declared_col_count
                            || range.HighVal.len() > declared_col_count
                    });
                let estimate_ranges = if need_prune {
                    let truncated = path
                        .Ranges
                        .iter()
                        .map(|range| ranger::Range {
                            LowVal: range
                                .LowVal
                                .iter()
                                .take(declared_col_count)
                                .cloned()
                                .collect(),
                            HighVal: range
                                .HighVal
                                .iter()
                                .take(declared_col_count)
                                .cloned()
                                .collect(),
                            Collators: range
                                .Collators
                                .iter()
                                .take(declared_col_count)
                                .map(|collator| collator.Clone())
                                .collect(),
                            LowExclude: range.LowExclude
                                && range.LowVal.len() <= declared_col_count,
                            HighExclude: range.HighExclude
                                && range.HighVal.len() <= declared_col_count,
                            ..Default::default()
                        })
                        .collect();
                    ranger::UnionRanges(context.GetRangerCtx(), ranger::Ranges(truncated), false)
                        .map_err(|error| PlannerError(error.to_string()))?
                        .0
                } else {
                    path.Ranges.clone()
                };
                let range_refs = estimate_ranges.iter().collect::<Vec<_>>();
                let column_refs = index_columns[..declared_col_count]
                    .iter()
                    .collect::<Vec<_>>();
                let mut estimate = cardinality::GetRowCountByIndexRanges(
                    &cardinality_context,
                    histogram,
                    index.ID,
                    &range_refs,
                    &column_refs,
                )
                .map_err(|error| PlannerError(error.to_string()))?;
                if need_prune && self.TableInfo.PKIsHandle {
                    let full_range_refs = path.Ranges.iter().collect::<Vec<_>>();
                    let full_column_refs = index_columns.iter().collect::<Vec<_>>();
                    estimate = cardinality::AdjustRowCountForAppendedHandleColumns(
                        &cardinality_context,
                        histogram,
                        &full_range_refs,
                        &full_column_refs,
                        declared_col_count,
                        estimate,
                    );
                }
                path.CountAfterAccess = estimate.Est;
                path.MinCountAfterAccess = estimate.MinEst;
                path.MaxCountAfterAccess = estimate.MaxEst;
                path.CountAfterIndex = estimate.Est;
            } else if !path.AccessConds.is_empty() {
                // Go's pseudo cardinality uses 1/1000 for equality and 1/3
                // for a one-sided range when no histogram is available.
                // 无直方图时的伪基数：等值约 1/1000，单边范围约 1/3。
                let equality = path.AccessConds.iter().any(|condition| {
                    condition.as_scalar_function().is_some_and(|function| {
                        matches!(
                            function.FuncName.L.as_str(),
                            parser_ast::EQ
                                | parser_ast::NullEQ
                                | parser_ast::In
                                | parser_ast::IsNull
                        )
                    })
                });
                let range_comparison = path.AccessConds.iter().any(|condition| {
                    condition.as_scalar_function().is_some_and(|function| {
                        matches!(
                            function.FuncName.L.as_str(),
                            parser_ast::LT | parser_ast::LE | parser_ast::GT | parser_ast::GE
                        )
                    })
                });
                let estimate = if equality {
                    (base_row_count / 1_000.0).max(1.0)
                } else if range_comparison {
                    (base_row_count / 3.0).max(1.0)
                } else {
                    base_row_count
                };
                path.CountAfterAccess = estimate;
                path.MinCountAfterAccess = estimate;
                path.MaxCountAfterAccess = estimate;
                path.CountAfterIndex = estimate;
            }
            if path.AccessConds.is_empty() {
                path.CountAfterAccess = base_row_count;
                path.MinCountAfterAccess = base_row_count;
                path.MaxCountAfterAccess = base_row_count;
                path.CountAfterIndex = base_row_count;
            }
        }

        // Go generateIndexMergePath builds one union candidate for every
        // indexable top-level DNF item. Each OR branch keeps all indexes whose
        // leading column can satisfy that branch; the remaining CNF items stay
        // as table filters on that candidate.
        fn predicate_column(expression: &dyn expression::Expression) -> Option<i64> {
            if let Some(column) = expression.as_column() {
                return Some(column.UniqueID);
            }
            expression
                .as_scalar_function()?
                .GetArgs()
                .iter()
                .find_map(|argument| predicate_column(argument.as_ref()))
        }

        if !self
            .PossibleAccessPaths
            .iter()
            .any(|path| !path.PartialAlternativeIndexPaths.is_empty())
        {
            let regular_paths = self.PossibleAccessPaths.clone();
            let mut index_merge_paths = Vec::new();
            for (condition_index, condition) in conditions.iter().enumerate() {
                let branches = expression::SplitDNFItems(condition.as_ref());
                if branches.len() < 2 {
                    continue;
                }
                let mut alternatives = Vec::with_capacity(branches.len());
                let mut complete = true;
                for branch in branches {
                    let Some(column_id) = predicate_column(branch.as_ref()) else {
                        complete = false;
                        break;
                    };
                    let branch_paths = regular_paths
                        .iter()
                        .filter(|path| {
                            !path.IsTablePath()
                                && path
                                    .IdxCols
                                    .first()
                                    .is_some_and(|column| column.UniqueID == column_id)
                        })
                        .map(|path| vec![path.clone()])
                        .collect::<Vec<_>>();
                    if branch_paths.is_empty() {
                        complete = false;
                        break;
                    }
                    alternatives.push(branch_paths);
                }
                if !complete {
                    continue;
                }
                let mut path = planner_util::AccessPath::default();
                path.PartialAlternativeIndexPaths = alternatives;
                path.TableFilters = conditions
                    .iter()
                    .enumerate()
                    .filter_map(|(index, filter)| {
                        (index != condition_index).then(|| filter.CloneExpr())
                    })
                    .collect();
                path.CountAfterAccess = regular_paths
                    .iter()
                    .map(|path| path.CountAfterAccess)
                    .fold(base_row_count, f64::min);
                path.MinCountAfterAccess = path.CountAfterAccess;
                path.MaxCountAfterAccess = path.CountAfterAccess;
                path.CountAfterIndex = path.CountAfterAccess;
                index_merge_paths.push(path);
            }
            self.PossibleAccessPaths.extend(index_merge_paths);
        }
        if !appended_handles.is_empty()
            && let Some(histogram) = self
                .TableStats
                .HistColl
                .as_deref()
                .and_then(|histogram| histogram.downcast_ref::<statistics::HistColl>())
        {
            appended_handles.retain(|(id, count, _)| {
                histogram
                    .Idx2ColUniqueIDs
                    .get(id)
                    .is_some_and(|mapped| mapped.len() == *count)
            });
            if !appended_handles.is_empty() {
                // The histogram is shared as an opaque Arc; publish a new
                // snapshot rather than mutate another plan's statistics.
                let mut updated = histogram.Copy();
                for (index_id, _, handle_ids) in appended_handles {
                    updated
                        .Idx2ColUniqueIDs
                        .get_mut(&index_id)
                        .unwrap()
                        .extend(handle_ids);
                }
                self.TableStats.HistColl = Some(std::sync::Arc::new(updated));
            }
        }
        self.AllPossibleAccessPaths = self.PossibleAccessPaths.clone();
        Ok(())
    }

    /// 初始化基类逻辑计划，算子名为 DataSource。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "DataSource", offset);
        self
    }

    /// 生成 EXPLAIN：表名与可选分区名。
    pub fn ExplainInfo(&self) -> String {
        let table = self
            .TableAsName
            .as_ref()
            .filter(|name| !name.O.is_empty())
            .unwrap_or(&self.TableInfo.Name);
        let mut result = format!("table:{}", table.O);
        if let (Some(offset), Some(partition)) =
            (self.PartitionDefIdx, self.TableInfo.GetPartitionInfo())
            && let Some(definition) = partition.Definitions.get(offset)
        {
            result.push_str(&format!(", partition:{}", definition.Name.O));
        }
        result
    }

    /// 谓词下推：简化后写入 AllConds/PushedDownConds，再推导访问路径。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let predicates = self.SCtx().cloned().map_or(predicates.clone(), |context| {
            rule_util::ApplyPredicateSimplification(context, predicates, true, None)
        });
        let all_conditions = predicates
            .iter()
            .map(|predicate| predicate.CloneExpr())
            .collect::<Vec<_>>();
        fn can_encode_for_storage(expression: &dyn expression::Expression) -> bool {
            if expression.as_column().is_some() || expression.as_correlated_column().is_some() {
                return true;
            }
            if let Some(constant) = expression.as_constant() {
                // Scalar subqueries are represented as deferred runtime
                // constants. TiDB fills them before execution; storage nodes
                // cannot serialize or evaluate that reference.
                return constant.SubqueryRefID == 0;
            }
            expression.as_scalar_function().is_some_and(|function| {
                function
                    .GetArgs()
                    .iter()
                    .all(|argument| can_encode_for_storage(argument.as_ref()))
            })
        }
        let (resolvable, retained): (Vec<_>, Vec<_>) =
            predicates.into_iter().partition(|predicate| {
                expression::ExtractColumns(predicate.as_ref())
                    .iter()
                    .all(|column| self.Schema().Contains(column))
                    && can_encode_for_storage(predicate.as_ref())
            });
        // Go keeps every predicate in AllConds for range derivation and
        // statistics, while expressions whose ordinary columns are local to
        // the source enter PushedDownConds. Like Go's PushDownExprs, this
        // includes correlated predicates: their outer operand is represented
        // separately and can drive a decided-by access range at execution.
        // Go：AllConds 保留全部谓词用于 Range/统计；本地列谓词进入 PushedDownConds（含关联谓词）。
        self.AllConds = all_conditions;
        self.PushedDownConds = resolvable
            .iter()
            .map(|predicate| predicate.CloneExpr())
            .collect();
        // Constant-false folding is represented by zero cardinality.  The
        // physical implementation still receives the original condition list.
        // 恒假折叠用零基数表示；物理层仍收到原始条件列表。
        if crate::Conds2TableDual(&self.AllConds) {
            self.TableStats.RowCount = 0.0;
            for ndv in self.TableStats.ColNDVs.values_mut() {
                *ndv = 0.0;
            }
        }
        self.deriveAccessPathsFromPredicates()?;
        // Go filters partial-index paths after predicates have been pushed
        // down, before the optimizer can choose a range on an unusable index.
        self.CheckPartialIndexes();
        Ok(retained)
    }

    /// 列裁剪：保留父用列与条件引用列；若全空则优先保留键列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let mut used: HashSet<i64> = parent_used_cols.iter().map(|c| c.UniqueID).collect();
        for condition in &self.AllConds {
            used.extend(
                expression::ExtractColumns(condition.as_ref())
                    .into_iter()
                    .map(|column| column.UniqueID),
            );
        }
        self.ColsRequiringFullLen = Some(
            self.Schema()
                .Columns
                .iter()
                .filter(|column| {
                    parent_used_cols
                        .iter()
                        .any(|used| used.UniqueID == column.UniqueID)
                })
                .cloned()
                .collect(),
        );

        let old_schema = self.Schema().Columns.clone();
        let old_columns = self.Columns.clone();
        let mut keep = self
            .Schema()
            .Columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| used.contains(&column.UniqueID).then_some(index))
            .collect::<Vec<_>>();
        let add_one_handle = keep.is_empty() && !old_schema.is_empty();
        if add_one_handle {
            keep.push(preferKeyColumnFromTable(self, &old_schema, &old_columns));
        }
        self.LogicalSchemaProducer.Schema_mut().Columns = keep
            .iter()
            .map(|index| old_schema[*index].clone())
            .collect();
        self.Columns = keep
            .iter()
            .filter_map(|index| old_columns.get(*index).cloned())
            .collect();
        if self.HandleCols.as_ref().is_some_and(|handle| {
            handle.IsInt() && handle.GetCol(0).is_some_and(|c| !self.Schema().Contains(c))
        }) {
            self.HandleCols = None;
        }
        // Go wraps an MPP DataSource in a projection when filter-only columns
        // remain below it. Preserve the parent's exact column list here,
        // including duplicate references, so physical construction can expose
        // that projection schema without dropping columns needed by filters.
        self.PrunedOutputColumns = (!add_one_handle
            && self.Schema().Len() > parent_used_cols.len()
            && !parent_used_cols.is_empty())
        .then(|| parent_used_cols.to_vec());
        Ok(())
    }

    /// 表是否有可用的 TiFlash 副本。
    pub fn HasTiFlash(&self) -> bool {
        self.TableInfo
            .TiFlashReplica
            .as_ref()
            .is_some_and(|replica| replica.Available && replica.Count > 0)
    }

    /// 根据唯一索引与句柄主键填充 Schema 的 PKOrUK / NullableUK。
    pub fn BuildKeyInfo(&mut self) {
        let mut strong = Vec::new();
        let mut nullable = Vec::new();
        {
            let schema = self.LogicalSchemaProducer.Schema();
            for index in self
                .TableInfo
                .Indices
                .iter()
                .filter(|index| index.IsPublic())
            {
                let (unique_key, new_key) =
                    rule_util::CheckIndexCanBeKey(index, &self.Columns, schema);
                if let Some(new_key) = new_key {
                    strong.push(new_key);
                } else if let Some(unique_key) = unique_key {
                    nullable.push(unique_key);
                }
            }
            if self.TableInfo.PKIsHandle {
                for (i, col) in self.Columns.iter().enumerate() {
                    if mysql::r#type::HasPriKeyFlag(col.GetFlag()) {
                        if let Some(column) = schema.Columns.get(i) {
                            strong.push(vec![column.clone()]);
                        }
                        break;
                    }
                }
            }
        }
        let schema = self.LogicalSchemaProducer.Schema_mut();
        schema.PKOrUK = strong;
        schema.NullableUK = nullable;
    }

    /// 谓词简化占位：在规则安装前保持两份条件列表同步。
    pub fn PredicateSimplification(&mut self) {
        // Expression canonicalization is owned by expression/ruleutil.  Keep
        // both condition lists synchronized until that rule is installed.
        // 表达式规范化由 expression/ruleutil 拥有；规则安装前保持两列表同步。
        self.PushedDownConds = self.AllConds.clone();
    }

    // Index histograms describe the declared key, even when range construction
    // has appended several common-handle columns to its column-ID mapping.
    fn getGroupNDVs(&self) -> Vec<property::GroupNDV> {
        let mut ndvs = Vec::new();
        if self.AskedColumnGroup.is_empty() {
            return ndvs;
        }
        let Some(histogram) = self
            .TableStats
            .HistColl
            .as_deref()
            .and_then(|histogram| histogram.downcast_ref::<statistics::HistColl>())
        else {
            return ndvs;
        };
        histogram.ForEachIndexImmutable(|id, index| {
            let Some(info) = index.Info.as_ref() else {
                return false;
            };
            let mapped = histogram
                .Idx2ColUniqueIDs
                .get(&id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let count = info.Columns.len();
            if mapped.len() < count {
                return false;
            }
            let mut columns = mapped[..count].to_vec();
            columns.sort_unstable();
            for group in &self.AskedColumnGroup {
                if group.len() != count {
                    return false;
                }
                if group
                    .iter()
                    .map(|column| column.UniqueID)
                    .eq(columns.iter().copied())
                    && index.IsEssentialStatsLoaded()
                {
                    ndvs.push(property::GroupNDV {
                        Cols: columns,
                        NDV: index.NDV as f64,
                    });
                    return true;
                }
            }
            false
        });
        ndvs
    }

    /// 推导统计：以 Cardinality 的直方图/伪统计选择率缩放行数与 NDV。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        self.deriveAccessPathsFromPredicates()?;
        // Predicate pushdown mutates AllConds after the builder has cached the
        // unfiltered table statistics.  Reuse that cache only for a source
        // without predicates; otherwise derive selectivity from the current
        // ranges/histograms.
        if !reload
            && self.AllConds.is_empty()
            && let Some(stats) = self.StatsInfo()
        {
            return Ok((stats.clone(), false));
        }
        self.TableStats.GroupNDVs = self.getGroupNDVs();
        let mut stats = self.TableStats.clone();
        if !self.AllConds.is_empty() && stats.RowCount > 0.0 {
            let context = self
                .SCtx()
                .ok_or_else(|| PlannerError("DataSource has no plan context".to_owned()))?;
            let fallback_histogram =
                BuildPseudoHistColl(&self.TableInfo, self.PhysicalTableID, &self.TblCols);
            let histogram = self
                .TableStats
                .HistColl
                .as_deref()
                .and_then(|histogram| histogram.downcast_ref::<statistics::HistColl>())
                .filter(|histogram| {
                    self.TableStats.StatsVersion != statistics::PseudoVersion
                        || histogram.ColNum() > 0
                });
            let histogram = histogram.unwrap_or(&fallback_histogram);
            let paths = self.PossibleAccessPaths.iter().collect::<Vec<_>>();
            let selectivity = cardinality::Selectivity(
                &CardinalityContextAdapter(context.as_ref()),
                histogram,
                &self.AllConds,
                &paths,
            )
            .map_err(|error| PlannerError(error.to_string()))?;
            stats = stats.Scale(context.GetSessionVars(), selectivity);
        }
        for path in &mut self.PossibleAccessPaths {
            let appended_handle_range = path.Index.as_ref().is_some_and(|index| {
                path.IdxCols.len() > index.Columns.len()
                    && path.Ranges.iter().any(|range| {
                        range.LowVal.len() > index.Columns.len()
                            || range.HighVal.len() > index.Columns.len()
                    })
            });
            // Handle selectivity is deliberately damped. Align every appended
            // handle path, including points, without the SelectionFactor penalty.
            if appended_handle_range
                && path.CountAfterAccess + cost::factors_thresholds::ToleranceFactor
                    < stats.RowCount
            {
                path.MinCountAfterAccess = if path.MinCountAfterAccess > 0.0 {
                    path.MinCountAfterAccess.min(path.CountAfterAccess)
                } else {
                    path.CountAfterAccess
                };
                path.CountAfterAccess = stats.RowCount;
                path.MaxCountAfterAccess = path.MaxCountAfterAccess.max(stats.RowCount);
            }
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 枚举句柄/索引路径可能提供的有序性。
    pub fn PreparePossibleProperties(&self) -> PossiblePropertiesInfo {
        let mut orders = Vec::new();
        for path in &self.AllPossibleAccessPaths {
            if path.IsIntHandlePath {
                if let Some(column) = self.GetPKIsHandleCol() {
                    orders.push(vec![column]);
                }
            } else if !path.IdxCols.is_empty() {
                for offset in 0..=path.EqCondCount.min(path.IdxCols.len() - 1) {
                    orders.push(path.IdxCols[offset..].to_vec());
                }
            }
        }
        PossiblePropertiesInfo {
            Orders: orders,
            HasTiFlash: self.HasTiFlash() && !self.PrefersTiKVOnly(),
        }
    }

    /// 从已下推条件中提取关联列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.PushedDownConds
            .iter()
            .flat_map(|condition| expression::ExtractCorColumns(condition.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 先 BuildKeyInfo，再提取函数依赖集。
    pub fn ExtractFD(&mut self) -> &fd::FDSet {
        self.BuildKeyInfo();
        self.LogicalSchemaProducer.BaseLogicalPlan.ExtractFD()
    }

    /// 由表路径构造 LogicalTableScan + TiKVSingleGather。
    pub fn buildTableGather(
        source: DataSourceRef,
        path: &planner_util::AccessPath,
    ) -> LogicalPlanRef {
        let (ctx, offset, schema, names, handle) = {
            let source_ref = source.borrow();
            (
                source_ref.SCtx().cloned(),
                source_ref.QueryBlockOffset(),
                source_ref.Schema().Clone(),
                source_ref.OutputNames().Shallow(),
                source_ref
                    .HandleCols
                    .as_ref()
                    .map(|value| value.CloneHandleCols()),
            )
        };
        let mut scan = LogicalTableScan::default();
        if let Some(ctx) = ctx.clone() {
            scan = scan.Init(ctx, offset);
        }
        scan.Source = Some(source.clone());
        scan.HandleCols = handle;
        scan.AccessConds = path.AccessConds.clone();
        scan.TableFilters = path.TableFilters.clone();
        scan.Ranges = path.Ranges.clone();
        scan.StoreType = path.StoreType;
        scan.LogicalSchemaProducer
            .SetSchemaAndNames(schema.Clone(), names.Shallow());
        let mut gather = TiKVSingleGather::default();
        if let Some(ctx) = ctx {
            gather = gather.Init(ctx, offset);
        }
        gather.Source = Some(source);
        gather.StoreType = path.StoreType;
        gather.TableFilters = path.TableFilters.clone();
        gather
            .LogicalSchemaProducer
            .SetSchemaAndNames(schema, names);
        gather.SetChildren(vec![Box::new(scan)]);
        Box::new(gather)
    }

    /// 由索引路径构造 LogicalIndexScan + TiKVSingleGather（可能双读）。
    pub fn buildIndexGather(
        source: DataSourceRef,
        path: &planner_util::AccessPath,
    ) -> Option<LogicalPlanRef> {
        let index = path.Index.as_ref()?.Clone();
        let (ctx, offset, schema, names, columns, is_single_scan) = {
            let source_ref = source.borrow();
            let single = source_ref.IsSingleScan(&path.IdxCols, &path.IdxColLens);
            (
                source_ref.SCtx().cloned(),
                source_ref.QueryBlockOffset(),
                source_ref.Schema().Clone(),
                source_ref.OutputNames().Shallow(),
                source_ref.Columns.clone(),
                single,
            )
        };
        let mut scan = LogicalIndexScan::default();
        if let Some(ctx) = ctx.clone() {
            scan = scan.Init(ctx, offset);
        }
        scan.Source = Some(source.clone());
        scan.Index = index.Clone();
        scan.Columns = columns;
        scan.FullIdxCols = path.FullIdxCols.iter().flatten().cloned().collect();
        scan.FullIdxColLens = path.FullIdxColLens.clone();
        scan.IdxCols = path.IdxCols.clone();
        scan.IdxColLens = path.IdxColLens.clone();
        scan.NoncacheableReason = path.NoncacheableReason.clone();
        scan.EqCondCount = path.EqCondCount;
        scan.AccessConds = path.AccessConds.clone();
        scan.IndexFilters = path.IndexFilters.clone();
        scan.Ranges = path.Ranges.clone();
        scan.IsDoubleRead = !is_single_scan;
        scan.LogicalSchemaProducer
            .SetSchemaAndNames(schema.Clone(), names.Shallow());
        let mut gather = TiKVSingleGather::default();
        if let Some(ctx) = ctx {
            gather = gather.Init(ctx, offset);
        }
        gather.Source = Some(source);
        gather.IsIndexGather = true;
        gather.Index = Some(index);
        gather.StoreType = path.StoreType;
        gather.IsDoubleRead = !is_single_scan;
        gather.TableFilters = path.TableFilters.clone();
        gather
            .LogicalSchemaProducer
            .SetSchemaAndNames(schema, names);
        gather.SetChildren(vec![Box::new(scan)]);
        Some(Box::new(gather))
    }

    /// 将所有 PossibleAccessPaths 转换为 Gather 候选计划。
    pub fn Convert2Gathers(source: DataSourceRef) -> Vec<LogicalPlanRef> {
        let paths = source.borrow().PossibleAccessPaths.clone();
        paths
            .iter()
            .filter_map(|path| {
                if path.IsTablePath() {
                    Some(Self::buildTableGather(source.clone(), path))
                } else {
                    Self::buildIndexGather(source.clone(), path)
                }
            })
            .collect()
    }

    /// 若 PK 为整数句柄，返回对应 Schema 列。
    pub fn GetPKIsHandleCol(&self) -> Option<Column> {
        getPKIsHandleColFromSchema(&self.Columns, self.Schema(), self.TableInfo.PKIsHandle)
    }

    /// Shared physical-key layout used by pruning and range construction.
    pub fn HandleColsToAppend(
        &self,
        path: &planner_util::AccessPath,
        declared: &[Option<Column>],
    ) -> (Vec<Column>, Vec<isize>) {
        handle_cols_to_append(
            &self.TableInfo,
            &self.CommonHandleCols,
            &self.CommonHandleLens,
            self.GetPKIsHandleCol().as_ref(),
            path,
            declared,
        )
    }

    /// Whether v0 stores non-binary string handle columns as collation weights.
    pub fn HasV0NewCollationStringHandle(&self) -> bool {
        has_v0_new_collation_string_handle(&self.TableInfo, &self.CommonHandleCols)
    }

    /// 创建隐藏的额外句柄列（_tidb_rowid 语义）。
    pub fn NewExtraHandleSchemaCol(&self) -> Column {
        let mut field_type = expression::types::NewFieldType(mysql::r#type::TypeLonglong);
        field_type.AddFlag(mysql::r#type::NotNullFlag | mysql::r#type::PriKeyFlag);
        let mut column = Column::new(
            *field_type,
            model::ExtraHandleID,
            self.SCtx()
                .expect("initialized DataSource has a plan context")
                .GetExprCtx()
                .AllocPlanColumnID(),
            self.Schema().Len() as isize,
        );
        column.OrigName = format!(
            "{}.{}.{}",
            self.DBName.O,
            self.TableInfo.Name.O,
            model::ExtraHandleName.O
        );
        column.IsHidden = true;
        column
    }

    /// 创建隐藏的提交时间戳列。
    pub fn NewExtraCommitTSSchemaCol(&self) -> Column {
        let mut field_type = expression::types::NewFieldType(mysql::r#type::TypeLonglong);
        field_type.AddFlag(mysql::r#type::UnsignedFlag);
        let mut column = Column::new(
            *field_type,
            model::ExtraCommitTSID,
            self.SCtx()
                .expect("initialized DataSource has a plan context")
                .GetExprCtx()
                .AllocPlanColumnID(),
            self.Schema().Len() as isize,
        );
        column.OrigName = format!(
            "{}.{}.{}",
            self.DBName.O,
            self.TableInfo.Name.O,
            model::ExtraCommitTSName.O
        );
        column.IsHidden = true;
        column
    }

    /// 判断索引是否覆盖所需列/条件（可单扫，无需回表）。
    pub fn IsSingleScan(&self, index_columns: &[Column], index_lens: &[isize]) -> bool {
        let use_pruned_columns = self
            .SCtx()
            .is_some_and(|context| context.GetRangerCtx().OptPrefixIndexSingleScan)
            && self.ColsRequiringFullLen.is_some();
        if !use_pruned_columns {
            return self.IsIndexCoveringColumns(&self.Schema().Columns, index_columns, index_lens);
        }
        self.IsIndexCoveringColumns(
            self.ColsRequiringFullLen.as_deref().unwrap_or_default(),
            index_columns,
            index_lens,
        ) && self
            .AllConds
            .iter()
            .all(|condition| self.IsIndexCoveringCondition(condition, index_columns, index_lens))
    }

    /// 索引列（含句柄）是否覆盖给定列集合。
    pub fn IsIndexCoveringColumns(
        &self,
        columns: &[Column],
        index_columns: &[Column],
        lens: &[isize],
    ) -> bool {
        columns
            .iter()
            .all(|column| self.indexCoveringColumn(column, index_columns, lens, false))
    }

    /// 索引是否覆盖条件中引用的全部列。
    pub fn IsIndexCoveringCondition(
        &self,
        condition: &Expression,
        index_columns: &[Column],
        lens: &[isize],
    ) -> bool {
        if let Some(function) = condition.as_scalar_function() {
            // A prefix index retains enough information to evaluate `col IS NULL`.
            if function.FuncName.L == parser_ast::IsNull
                && let Some(column) = function.GetArgs().first().and_then(|arg| arg.as_column())
            {
                return self.indexCoveringColumn(column, index_columns, lens, true);
            }
            return function
                .GetArgs()
                .iter()
                .all(|argument| self.IsIndexCoveringCondition(argument, index_columns, lens));
        }
        expression::ExtractColumns(condition.as_ref())
            .into_iter()
            .all(|column| self.indexCoveringColumn(column, index_columns, lens, false))
    }

    /// 单列是否被索引列或句柄覆盖。
    fn indexCoveringColumn(
        &self,
        column: &Column,
        index_columns: &[Column],
        lens: &[isize],
        ignore_len: bool,
    ) -> bool {
        isIndexColsCoveringCol(column, index_columns, lens, ignore_len)
            || self.handleCoveringColumn(column, ignore_len) != HandleCoverState::NotCovered
    }

    /// 判断句柄/公共句柄/额外句柄列对目标列的覆盖状态。
    fn handleCoveringColumn(&self, column: &Column, ignore_len: bool) -> HandleCoverState {
        if self.TableInfo.PKIsHandle
            && column
                .RetType
                .as_ref()
                .is_some_and(|field_type| mysql::r#type::HasPriKeyFlag(field_type.GetFlag()))
        {
            return HandleCoverState::Covered;
        }
        if column.ID == model::ExtraHandleID || column.ID == model::ExtraPhysTblID {
            return HandleCoverState::Covered;
        }
        if isIndexColsCoveringCol(
            column,
            &self.CommonHandleCols,
            &self.CommonHandleLens,
            ignore_len,
        ) {
            return HandleCoverState::Covered;
        }
        HandleCoverState::NotCovered
    }

    /// 登记表全列映射（按 ID 与顺序）。
    pub fn AppendTableCol(&mut self, column: Column) {
        self.TblColsByID.insert(column.ID, column.clone());
        self.TblCols.push(column);
    }

    /// 丢弃无访问条件的部分索引路径（条件表达式非空却未匹配）。
    pub fn CheckPartialIndexes(&mut self) {
        let schema = self.Schema().Clone();
        let names = self.OutputNames().Shallow();
        let context = self.SCtx().cloned();
        let mut partial_index_is_usable = |path: &planner_util::AccessPath| {
            let Some(index) = path.Index.as_ref().filter(|index| {
                !index.ConditionExprString.is_empty()
                    && !index
                        .ConditionExprString
                        .to_ascii_lowercase()
                        .contains("vec_")
                    && index.VectorInfo.is_none()
                    && index.InvertedInfo.is_none()
            }) else {
                return true;
            };
            if path.AccessConds.is_empty() {
                return false;
            }
            let normalized = index.ConditionExprString.to_ascii_lowercase();
            if normalized.contains("is not null") || normalized.contains("not(isnull") {
                let Some(context) = context.as_ref() else {
                    return false;
                };
                partial_index_always_meets_constraints(
                    context.as_ref(),
                    &index.ConditionExprString,
                    &schema,
                    &names,
                    &self.TableInfo,
                    &self.PushedDownConds,
                )
            } else {
                true
            }
        };
        if let Some(context) = self.SCtx().cloned()
            && context.GetSessionVars().StmtCtx.PlanCacheTracker.UseCache()
        {
            let schema = self.Schema().Clone();
            let names = self.OutputNames().Shallow();
            for path in &mut self.PossibleAccessPaths {
                let Some(index) = path.Index.as_ref().filter(|index| {
                    !index.ConditionExprString.is_empty() && !path.AccessConds.is_empty()
                }) else {
                    continue;
                };
                if !partial_index_always_meets_constraints(
                    context.as_ref(),
                    &index.ConditionExprString,
                    &schema,
                    &names,
                    &self.TableInfo,
                    &self.PushedDownConds,
                ) {
                    path.NoncacheableReason =
                        "IndexScan of partial index is uncacheable".to_owned();
                }
            }
        }
        self.AllPossibleAccessPaths
            .retain(&mut partial_index_is_usable);
        self.PossibleAccessPaths.retain(partial_index_is_usable);
    }
}

fn has_v0_new_collation_string_handle(table: &model::TableInfo, handles: &[Column]) -> bool {
    table.CommonHandleVersion == 0
        && expression::collate::NewCollationEnabled()
        && handles.iter().any(|column| {
            column.RetType.as_ref().is_some_and(|field| {
                field.EvalType() == expression::types::ETString
                    && !expression::mysql::HasBinaryFlag(field.GetFlag())
            })
        })
}

fn handle_cols_to_append(
    table: &model::TableInfo,
    handles: &[Column],
    lengths: &[isize],
    integer_handle: Option<&Column>,
    path: &planner_util::AccessPath,
    declared: &[Option<Column>],
) -> (Vec<Column>, Vec<isize>) {
    let empty = || (Vec::new(), Vec::new());
    let Some(index) = path.Index.as_ref() else {
        return empty();
    };
    if index.Unique
        || index.Primary
        || index.Columns.len() != declared.len()
        || declared.iter().any(Option::is_none)
    {
        return empty();
    }
    if table.IsCommonHandle {
        if handles.is_empty()
            || lengths.len() != handles.len()
            || index.Global
            || index.MVIndex
            || index.IsColumnarIndex()
            || has_v0_new_collation_string_handle(table, handles)
            || handles.iter().any(|handle| {
                declared
                    .iter()
                    .flatten()
                    .any(|column| column.UniqueID == handle.UniqueID)
            })
        {
            return empty();
        }
        return (handles.to_vec(), lengths.to_vec());
    }
    let Some(handle) = integer_handle else {
        return empty();
    };
    if handle
        .RetType
        .as_ref()
        .is_none_or(|field| expression::mysql::HasUnsignedFlag(field.GetFlag()))
        || declared
            .iter()
            .flatten()
            .any(|column| column.ID == model::ExtraHandleID || column.UniqueID == handle.UniqueID)
    {
        return empty();
    }
    (
        vec![handle.Clone()],
        vec![expression::types::UnspecifiedLength as isize],
    )
}

/// Go only treats a single `IS NOT NULL` partial-index predicate as invariant
/// across prepared parameters, and only when a pushed-down filter null-rejects
/// that same column. Other implication proofs may depend on parameter values.
fn partial_index_always_meets_constraints(
    context: &dyn base::PlanContext,
    condition: &str,
    schema: &Schema,
    names: &NameSlice,
    table: &model::TableInfo,
    filters: &[Expression],
) -> bool {
    let Ok(predicate) = expression::ParseSimpleExpr(
        context.GetExprCtx(),
        condition,
        vec![expression::WithInputSchemaAndNames(
            schema,
            names.Shallow(),
            Some(table),
        )],
    ) else {
        return false;
    };
    let predicates = expression::SplitCNFItems(predicate.as_ref());
    let Some(outer) = (predicates.len() == 1)
        .then(|| predicates[0].as_scalar_function())
        .flatten()
        .filter(|function| function.FuncName.L == parser_ast::UnaryNot)
    else {
        return false;
    };
    let Some(inner) = outer
        .GetArgs()
        .first()
        .and_then(|argument| argument.as_scalar_function())
        .filter(|function| function.FuncName.L == parser_ast::IsNull)
    else {
        return false;
    };
    let Some(column) = inner
        .GetArgs()
        .first()
        .and_then(|argument| argument.as_column())
    else {
        return false;
    };
    filters
        .iter()
        .any(|filter| partial_index_filter_null_rejects(column, filter.as_ref()))
}

fn partial_index_filter_null_rejects(column: &Column, filter: &dyn expression::Expression) -> bool {
    let Some(function) = filter.as_scalar_function() else {
        return false;
    };
    if function.FuncName.L == expression::ast::LogicOr {
        return function
            .GetArgs()
            .iter()
            .all(|argument| partial_index_filter_null_rejects(column, argument.as_ref()));
    }
    if function.FuncName.L == expression::ast::LogicAnd {
        return function
            .GetArgs()
            .iter()
            .any(|argument| partial_index_filter_null_rejects(column, argument.as_ref()));
    }
    if function.FuncName.L == parser_ast::IsNull {
        return false;
    }
    if function.FuncName.L == parser_ast::NullEQ
        || !expression::CompareOpMap.contains(function.FuncName.L.as_str())
    {
        return false;
    }
    function
        .GetArgs()
        .iter()
        .filter_map(|argument| argument.as_column())
        .any(|candidate| candidate.UniqueID == column.UniqueID)
}

/// LogicalPlan trait 委托；恒假条件时可替换为 TableDual。
impl LogicalPlan for DataSource {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn ExplainInfo(&self) -> String {
        DataSource::ExplainInfo(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        DataSource::PredicatePushDown(self, predicates)
    }
    fn PredicatePushDownRoot(
        &mut self,
        predicates: Vec<Expression>,
    ) -> Result<(Vec<Expression>, Option<LogicalPlanRef>)> {
        let retained = DataSource::PredicatePushDown(self, predicates)?;
        if crate::Conds2TableDual(&self.AllConds) {
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| PlannerError("DataSource has no plan context".into()))?;
            let mut dual = LogicalTableDual {
                RowCount: 0,
                ..LogicalTableDual::default()
            }
            .Init(context, self.QueryBlockOffset());
            dual.SetSchema(self.Schema().Clone());
            dual.SetOutputNames(self.OutputNames().Shallow());
            return Ok((retained, Some(Box::new(dual))));
        }
        Ok((retained, None))
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        DataSource::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        DataSource::BuildKeyInfo(self)
    }
    fn PredicateSimplification(&mut self) {
        DataSource::PredicateSimplification(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        DataSource::DeriveStats(self, reload)
    }
}

/// 从列元数据与 Schema 中查找整数句柄主键列。
fn getPKIsHandleColFromSchema(
    columns: &[model::ColumnInfo],
    schema: &Schema,
    pk_is_handle: bool,
) -> Option<Column> {
    if !pk_is_handle {
        return columns.iter().enumerate().find_map(|(index, info)| {
            (info.ID == model::ExtraHandleID)
                .then(|| schema.Columns.get(index).cloned())
                .flatten()
        });
    }
    columns.iter().find_map(|info| {
        mysql::r#type::HasPriKeyFlag(info.GetFlag())
            .then(|| {
                schema
                    .Columns
                    .iter()
                    .find(|column| column.ID == info.ID)
                    .cloned()
            })
            .flatten()
    })
}

/// 列裁剪兜底：优先保留主键或首个公开索引列的偏移。
fn preferKeyColumnFromTable(
    ds: &DataSource,
    schema: &[Column],
    columns: &[model::ColumnInfo],
) -> usize {
    // Go prefers the table handle when pruning would otherwise remove every
    // column. For a non-clustered table this is `_tidb_rowid`, not the first
    // user column or a secondary index column.
    if let Some(handle) = ds.HandleCols.as_ref().and_then(|handle| handle.GetCol(0))
        && let Some(index) = schema
            .iter()
            .position(|column| column.UniqueID == handle.UniqueID)
    {
        return index;
    }
    if ds.TableInfo.PKIsHandle
        && let Some(index) = columns
            .iter()
            .position(|column| mysql::r#type::HasPriKeyFlag(column.GetFlag()))
    {
        return index.min(schema.len() - 1);
    }
    columns
        .iter()
        .position(|column| column.ID == model::ExtraHandleID)
        .unwrap_or(0)
}

/// 判断列是否出现在索引列中，且（除非 ignore_len）为全长前缀。
fn isIndexColsCoveringCol(
    column: &Column,
    index_columns: &[Column],
    lens: &[isize],
    ignore_len: bool,
) -> bool {
    index_columns.iter().enumerate().any(|(index, candidate)| {
        (candidate.UniqueID == column.UniqueID || candidate.ID == column.ID)
            && (ignore_len
                || lens
                    .get(index)
                    .copied()
                    .unwrap_or(expression::types::UnspecifiedLength as isize)
                    == expression::types::UnspecifiedLength as isize)
    })
}

/// 表是否声明了可用的（含假设）TiFlash 副本。
pub fn UsedHypoTiFlashReplicas(_db_name: &parser_ast::CIStr, table: &model::TableInfo) -> bool {
    table
        .TiFlashReplica
        .as_ref()
        .is_some_and(|replica| replica.Available)
}
