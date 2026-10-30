// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::{Range, types};
use std::collections::HashSet;

/// Proves at most one row per range using a full, public, unconditional unique key.
pub fn RowCountUpperBoundByUniqueKey(
    tc: types::Context,
    table: Option<&model::TableInfo>,
    columns: &[expression::Column],
    lengths: &[isize],
    ranges: &[Range],
) -> (u64, bool) {
    let Some(table) = table else {
        return (0, false);
    };
    if ranges.is_empty() || columns.is_empty() || columns.len() != lengths.len() {
        return (0, false);
    }
    let mut keys = Vec::new();
    if table.PKIsHandle {
        if let Some(pk) = table.GetPkColInfo() {
            keys.push(vec![pk.ID]);
        }
    }
    for index in &table.Indices {
        if !(index.Unique || index.Primary)
            || index.State != model::StatePublic
            || index.MVIndex
            || index.HasCondition()
            || index.HasPrefixIndex()
        {
            continue;
        }
        let ids = index
            .Columns
            .iter()
            .map(|column| table.Columns[column.Offset as usize].ID)
            .collect::<Vec<_>>();
        keys.push(ids);
    }
    if keys.is_empty() {
        return (0, false);
    }
    let mut fixed = HashSet::with_capacity(columns.len());
    for range in ranges {
        fixed.clear();
        let n = range
            .LowVal
            .len()
            .min(range.HighVal.len())
            .min(columns.len())
            .min(range.Collators.len());
        let mut all_fixed = true;
        for i in 0..n {
            let low = &range.LowVal[i];
            let high = &range.HighVal[i];
            if [low.Kind(), high.Kind()]
                .iter()
                .any(|kind| *kind == types::KindMinNotNull || *kind == types::KindMaxValue)
                || !matches!(
                    low.Compare(tc.clone(), high, range.Collators[i].as_ref()),
                    Ok(0)
                )
            {
                all_fixed = false;
                break;
            }
            if !low.IsNull() && lengths[i] == types::UnspecifiedLength as isize {
                fixed.insert(columns[i].ID);
            }
        }
        if all_fixed
            && range.LowVal.len() == range.HighVal.len()
            && n == range.LowVal.len()
            && (range.LowExclude || range.HighExclude)
        {
            return (0, false);
        }
        if !keys
            .iter()
            .any(|key| key.iter().all(|id| fixed.contains(id)))
        {
            return (0, false);
        }
    }
    (ranges.len() as u64, true)
}
