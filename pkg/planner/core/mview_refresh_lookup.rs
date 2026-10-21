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

use std::collections::HashSet;

use crate::{PlanKind, PlanNode};

/// Index metadata needed by the MIN/MAX full-update lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MViewIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub visible: bool,
}

/// Return every visible index whose leading columns cover all group keys,
/// independent of group-key order, matching TiDB's mview utility contract.
pub fn FindVisibleIndexesWithPrefixCoveringColumns(
    indexes: &[MViewIndex],
    group_by_columns: &[String],
) -> Vec<String> {
    let required = group_by_columns
        .iter()
        .map(|column| column.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    indexes
        .iter()
        .filter(|index| index.visible && index.columns.len() >= required.len())
        .filter(|index| {
            index
                .columns
                .iter()
                .take(required.len())
                .map(|column| column.to_ascii_lowercase())
                .collect::<HashSet<_>>()
                == required
        })
        .map(|index| index.name.clone())
        .collect()
}

/// Optimizer output needed by the executor after discarding the synthetic
/// outer probe side of the lookup template.
#[derive(Clone, Debug, PartialEq)]
pub struct MViewFullUpdateLookupTemplate {
    pub inner_source: PlanNode,
    pub inner_column_count: usize,
    pub index_ranges: Vec<Vec<String>>,
    pub key_off_to_index_off: Vec<usize>,
    pub key_result_col_indexes: Vec<usize>,
    pub output_mv_offsets: Vec<usize>,
}

/// Convert lookup-template output order to the optimized inner schema order.
pub fn DeriveMViewFullUpdateOutputMVOffsetsByInnerSchema(
    lookup_output_mv_offsets: &[usize],
    group_key_mv_offsets: &[usize],
    key_result_col_indexes: &[usize],
    inner_column_count: usize,
) -> Result<Vec<usize>, String> {
    if lookup_output_mv_offsets.len() != inner_column_count {
        return Err(format!(
            "mview full-update lookup template: unexpected lookup output mv-offset length: got {}, expected {}",
            lookup_output_mv_offsets.len(),
            inner_column_count
        ));
    }
    if group_key_mv_offsets.len() != key_result_col_indexes.len() {
        return Err(format!(
            "mview full-update lookup template: group key mapping length mismatch: group key mv offsets={}, key result indexes={}",
            group_key_mv_offsets.len(),
            key_result_col_indexes.len()
        ));
    }

    let mut result = vec![usize::MAX; inner_column_count];
    let mut group_keys = HashSet::new();
    for (&mv_offset, &result_index) in group_key_mv_offsets
        .iter()
        .zip(key_result_col_indexes.iter())
    {
        if !group_keys.insert(mv_offset) {
            return Err(format!(
                "mview full-update lookup template: duplicate group key mv offset {mv_offset}"
            ));
        }
        if result_index >= inner_column_count {
            return Err(format!(
                "mview full-update lookup template: key result col idx {result_index} out of range [0,{inner_column_count})"
            ));
        }
        if result[result_index] != usize::MAX {
            return Err(format!(
                "mview full-update lookup template: duplicate key result col idx {result_index}"
            ));
        }
        result[result_index] = mv_offset;
    }

    let non_keys = lookup_output_mv_offsets
        .iter()
        .copied()
        .filter(|offset| !group_keys.contains(offset))
        .collect::<Vec<_>>();
    let unassigned = result
        .iter()
        .enumerate()
        .filter_map(|(index, offset)| (*offset == usize::MAX).then_some(index))
        .collect::<Vec<_>>();
    if non_keys.len() != unassigned.len() {
        return Err(format!(
            "mview full-update lookup template: non-key column mapping mismatch: unassigned result columns={}, non-key mv offsets={}",
            unassigned.len(),
            non_keys.len()
        ));
    }
    for (result_index, mv_offset) in unassigned.into_iter().zip(non_keys) {
        result[result_index] = mv_offset;
    }
    Ok(result)
}

/// Validate and extract the optimizer-facing lookup metadata.  The generic
/// Rust plan represents the inner child explicitly, so extraction does not
/// depend on a concrete IndexJoin implementation.
pub fn ExtractMViewFullUpdateLookupTemplate(
    inner_source: PlanNode,
    inner_column_count: usize,
    index_ranges: Vec<Vec<String>>,
    key_off_to_index_off: Vec<usize>,
    key_result_col_indexes: Vec<usize>,
    lookup_output_mv_offsets: &[usize],
    group_key_mv_offsets: &[usize],
) -> Result<MViewFullUpdateLookupTemplate, String> {
    if inner_column_count == 0 {
        return Err("mview full-update lookup template: inner schema is empty".to_owned());
    }
    if index_ranges.is_empty() || index_ranges[0].is_empty() {
        return Err("mview full-update lookup template: index join ranges are empty".to_owned());
    }
    if key_off_to_index_off.len() != group_key_mv_offsets.len()
        || key_result_col_indexes.len() != group_key_mv_offsets.len()
    {
        return Err(
            "mview full-update lookup template: group-key mapping length mismatch".to_owned(),
        );
    }
    let range_width = index_ranges[0].len();
    if let Some((position, offset)) = key_off_to_index_off
        .iter()
        .enumerate()
        .find(|(_, offset)| **offset >= range_width)
    {
        return Err(format!(
            "mview full-update lookup template: invalid keyOff2IdxOff[{position}]={offset} for range width {range_width}"
        ));
    }
    let output_mv_offsets = DeriveMViewFullUpdateOutputMVOffsetsByInnerSchema(
        lookup_output_mv_offsets,
        group_key_mv_offsets,
        &key_result_col_indexes,
        inner_column_count,
    )?;
    let inner_source = if matches!(inner_source.kind, PlanKind::IndexJoin { .. }) {
        inner_source
    } else {
        return Err(
            "mview full-update lookup template: expected index join plan but not found".to_owned(),
        );
    };
    Ok(MViewFullUpdateLookupTemplate {
        inner_source,
        inner_column_count,
        index_ranges,
        key_off_to_index_off,
        key_result_col_indexes,
        output_mv_offsets,
    })
}
