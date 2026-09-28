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

//! Canonical integer-handle point plans from resolved logical SELECTs.
//! Eligibility follows Go's tryPointGetPlan; unsupported shapes retain the
//! ordinary optimizer path, including predicates that require residual filters.

use base_dependency::{ContextRef, PhysicalPlan, Plan};
use expression_dependency as expression;
use logicalop_dependency::{DataSource, LogicalPlan, LogicalProjection, LogicalSelection};
use physicalop_dependency::PointGetPlan;

/// Build a real PointGet operator without enumerating general physical plans.
/// The caller owns statement eligibility (fix controls, hints, and read engines).
/// Logical construction has already resolved names and checked column access.
pub fn TryFastIntegerPointGet(
    context: &ContextRef,
    logical: &dyn LogicalPlan,
) -> Option<Box<dyn PhysicalPlan>> {
    let projection = logical.as_any().downcast_ref::<LogicalProjection>()?;
    let [selection] = projection.Children() else {
        return None;
    };
    let selection = selection.as_any().downcast_ref::<LogicalSelection>()?;
    let [source] = selection.Children() else {
        return None;
    };
    let source = source.as_any().downcast_ref::<DataSource>()?;
    let table = &source.TableInfo;
    if !table.PKIsHandle
        || table.Partition.is_some()
        || !source.AllConds.is_empty()
        || !source.PushedDownConds.is_empty()
        || table
            .Columns
            .iter()
            .any(|column| column.IsGenerated() || column.State != expression::model::StatePublic)
        || matches!(
            source.DBName.L.as_str(),
            "information_schema" | "performance_schema" | "metrics_schema"
        )
    {
        return None;
    }
    let primary = table
        .Columns
        .iter()
        .find(|column| expression::mysql::HasPriKeyFlag(column.GetFlag()))?;
    // An extra condition must never disappear when replacing the selection.
    let [condition] = selection.Conditions.as_slice() else {
        return None;
    };
    let equality = condition
        .as_any()
        .downcast_ref::<expression::ScalarFunction>()?;
    if equality.FuncName.L != expression::ast::EQ {
        return None;
    }
    let [left, right] = equality.GetArgs() else {
        return None;
    };
    let (column, constant) = left
        .as_any()
        .downcast_ref::<expression::Column>()
        .zip(right.as_any().downcast_ref::<expression::Constant>())
        .or_else(|| {
            right
                .as_any()
                .downcast_ref::<expression::Column>()
                .zip(left.as_any().downcast_ref::<expression::Constant>())
        })?;
    if column.ID != primary.ID || constant.DeferredExpr.is_some() || constant.ParamMarker.is_some()
    {
        return None;
    }
    // Only lossless integer bindings are eligible here. Other coercions retain
    // the general range builder's overflow, truncation and NULL semantics.
    let value = &constant.Value;
    let unsigned = expression::mysql::HasUnsignedFlag(primary.GetFlag());
    let handle = match value.Kind() {
        expression::types::KindInt64 if !unsigned || value.GetInt64() >= 0 => value.GetInt64(),
        expression::types::KindUint64 if unsigned || value.GetUint64() <= i64::MAX as u64 => {
            value.GetUint64() as i64
        }
        _ => return None,
    };
    let columns = projection
        .Exprs
        .iter()
        .map(|expr| {
            let column = expr.as_any().downcast_ref::<expression::Column>()?;
            table
                .Columns
                .iter()
                .find(|info| info.ID == column.ID)
                .cloned()
        })
        .collect::<Option<Vec<_>>>()?;
    let access_columns = projection
        .Exprs
        .iter()
        .map(|expr| {
            expr.as_any()
                .downcast_ref::<expression::Column>()
                .map(expression::Column::Clone)
        })
        .collect::<Option<Vec<_>>>()?;
    // Match Go TryFastPlan: construction allocates the first ID of this
    // optimization, rather than renumbering a general plan after the fact.
    context.reset_plan_id();
    let mut point = PointGetPlan::New(context.clone());
    point.DBName = source.DBName.O.clone();
    point.TblInfo = Some(table.Clone());
    point.Handle = Some(handle);
    point.UnsignedHandle = unsigned;
    point.Columns = columns;
    point.AccessColumns = access_columns;
    point.AccessConditions = selection.Conditions.clone();
    point.CostValue = 1.0;
    point.SetSchema(projection.Schema().Clone());
    point.set_output_names(projection.OutputNames().Shallow());
    Some(Box::new(point))
}
