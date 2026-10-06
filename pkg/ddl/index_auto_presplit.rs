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

//! `PRE_SPLIT_REGIONS AUTO` planning for add-index.
//!
//! The statistics handle owns loading and decoding column statistics.  This
//! module owns the deterministic part of the Go algorithm: eligibility gates,
//! merging TopN and histogram masses, quantile sampling, key construction, and
//! the strict-manual/best-effort-AUTO execution distinction.

use crate::backfilling::Key;
use crate::index_cop::Datum;
use crate::index_presplit::{SplitError, get_split_keys_from_value_list};

/// Production defaults from Go `getAutoPreSplitConfig`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoPreSplitConfig {
    pub min_table_rows: i64,
    pub min_stats_healthy: i64,
    pub boundary_ratio_step: f64,
}

impl Default for AutoPreSplitConfig {
    fn default() -> Self {
        Self {
            min_table_rows: 1_000_000,
            min_stats_healthy: 80,
            boundary_ratio_step: 0.02,
        }
    }
}

/// Facts that the caller derives from table/index metadata and table stats.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutoPreSplitEligibility {
    pub table_id: i64,
    pub index_id: i64,
    pub partitioned: bool,
    pub partial_index: bool,
    pub has_leading_column: bool,
    pub leading_string_prefix: bool,
    pub row_count: i64,
    pub stats_healthy: Option<i64>,
}

/// One histogram upper bound and its cumulative count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistogramPoint {
    pub upper: Datum,
    pub cumulative_count: i64,
}

impl HistogramPoint {
    pub fn new(upper: Datum, cumulative_count: i64) -> Self {
        Self {
            upper,
            cumulative_count,
        }
    }
}

/// Complete leading-column distribution loaded from one statistics snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DistributionStats {
    pub stats_version: i64,
    pub null_count: i64,
    pub top_n: Vec<(Datum, u64)>,
    pub histogram: Vec<HistogramPoint>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AutoPreSplitPlanState {
    #[default]
    Invalid,
    Planned,
    Skipped,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutoPreSplitPlan {
    pub state: AutoPreSplitPlanState,
    pub split_keys: Vec<Key>,
    pub boundary_rows: Vec<Vec<Datum>>,
    pub skip_reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WeightedValue {
    value: Datum,
    encoded: Vec<u8>,
    count: u64,
}

fn skipped(reason: impl Into<String>) -> AutoPreSplitPlan {
    AutoPreSplitPlan {
        state: AutoPreSplitPlanState::Skipped,
        skip_reason: Some(reason.into()),
        ..Default::default()
    }
}

/// Plans AUTO split keys from a complete Analyze V2 leading-column snapshot.
pub fn plan_auto_pre_split(
    eligibility: &AutoPreSplitEligibility,
    stats: &DistributionStats,
    config: AutoPreSplitConfig,
) -> Result<AutoPreSplitPlan, String> {
    if eligibility.partitioned {
        return Ok(skipped("partitioned table"));
    }
    if eligibility.partial_index {
        return Ok(skipped("partial index"));
    }
    if !eligibility.has_leading_column {
        return Ok(skipped("index has no columns"));
    }
    if eligibility.leading_string_prefix {
        return Ok(skipped("leading string column uses prefix index"));
    }
    let Some(healthy) = eligibility.stats_healthy else {
        return Ok(skipped("stats health unavailable"));
    };
    if healthy < config.min_stats_healthy {
        return Ok(skipped(format!(
            "stats health {healthy} below threshold {}",
            config.min_stats_healthy
        )));
    }
    if eligibility.row_count < config.min_table_rows {
        return Ok(skipped(format!(
            "row count {} below threshold {}",
            eligibility.row_count, config.min_table_rows
        )));
    }
    if stats.stats_version != 2 {
        return Ok(skipped(format!(
            "leading column stats version {} is not Analyze V2",
            stats.stats_version
        )));
    }
    if stats.null_count < 0 {
        return Err(format!(
            "leading column statistics have negative null count {}",
            stats.null_count
        ));
    }
    if !(0.0..1.0).contains(&config.boundary_ratio_step) || config.boundary_ratio_step == 0.0 {
        return Err("AUTO pre-split boundary ratio step must be between 0 and 1".into());
    }

    let mut values = Vec::with_capacity(stats.top_n.len() + stats.histogram.len() + 1);
    if stats.null_count > 0 {
        values.push(weighted(Datum::Null, stats.null_count as u64));
    }
    values.extend(
        stats
            .top_n
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(value, count)| weighted(value.clone(), *count)),
    );

    let mut previous = 0_i64;
    for (index, bucket) in stats.histogram.iter().enumerate() {
        if bucket.cumulative_count < previous {
            return Err(format!(
                "histogram bucket {index} cumulative count {} is below previous count {previous}",
                bucket.cumulative_count
            ));
        }
        let delta = bucket.cumulative_count - previous;
        previous = bucket.cumulative_count;
        if delta > 0 {
            values.push(weighted(bucket.upper.clone(), delta as u64));
        }
    }

    values.sort_by(|left, right| left.encoded.cmp(&right.encoded));
    let mut merged: Vec<WeightedValue> = Vec::with_capacity(values.len());
    let mut total_count = 0_u64;
    for value in values {
        total_count = total_count.saturating_add(value.count);
        if let Some(previous) = merged
            .last_mut()
            .filter(|item| item.encoded == value.encoded)
        {
            previous.count = previous.count.saturating_add(value.count);
        } else {
            merged.push(value);
        }
    }
    if total_count == 0 {
        return Ok(skipped("no usable leading column distribution"));
    }

    let boundary_rows = sample_boundaries(&merged, total_count, config.boundary_ratio_step);
    if boundary_rows.is_empty() {
        return Ok(skipped("no internal distribution boundary"));
    }
    let mut split_keys =
        get_split_keys_from_value_list(eligibility.table_id, eligibility.index_id, &boundary_rows)
            .map_err(|error| format!("failed to build auto presplit keys: {error:?}"))?;
    split_keys.retain(|key| !key.is_empty());
    split_keys.sort();
    split_keys.dedup();
    if split_keys.is_empty() {
        return Err("planned auto pre-split has no split keys".into());
    }
    Ok(AutoPreSplitPlan {
        state: AutoPreSplitPlanState::Planned,
        split_keys,
        boundary_rows,
        skip_reason: None,
    })
}

fn weighted(value: Datum, count: u64) -> WeightedValue {
    let encoded = encode_comparison_value(&value);
    WeightedValue {
        value,
        encoded,
        count,
    }
}

fn encode_comparison_value(value: &Datum) -> Vec<u8> {
    match value {
        Datum::Null => vec![0],
        Datum::Int(value) => {
            let mut encoded = vec![1];
            encoded.extend_from_slice(&((*value as u64) ^ (1 << 63)).to_be_bytes());
            encoded
        }
        Datum::UInt(value) => {
            let mut encoded = vec![2];
            encoded.extend_from_slice(&value.to_be_bytes());
            encoded
        }
        Datum::Bytes(value) => {
            let mut encoded = vec![3];
            encoded.extend_from_slice(value);
            encoded
        }
        Datum::Text(value) => {
            let mut encoded = vec![3];
            encoded.extend_from_slice(value.as_bytes());
            encoded
        }
    }
}

fn sample_boundaries(
    values: &[WeightedValue],
    total_count: u64,
    boundary_ratio_step: f64,
) -> Vec<Vec<Datum>> {
    let mut next_threshold_index = 1_usize;
    let mut cumulative = 0_u64;
    let mut rows = Vec::new();
    for value in values {
        cumulative = cumulative.saturating_add(value.count);
        let next_threshold = next_threshold_index as f64 * boundary_ratio_step;
        if next_threshold >= 1.0 {
            break;
        }
        let cumulative_ratio = cumulative as f64 / total_count as f64;
        if cumulative_ratio < next_threshold {
            continue;
        }
        rows.push(vec![value.value.clone()]);
        let crossed = (cumulative_ratio / boundary_ratio_step).floor() as usize;
        next_threshold_index = (next_threshold_index + 1).max(crossed + 1);
    }
    rows
}

/// Manual options always take precedence when both fields appear in decoded
/// legacy metadata.  Manual failures remain fatal; AUTO planning/split failures
/// are optional optimization failures and do not fail add-index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreSplitMode {
    None,
    Manual(Vec<Vec<Datum>>),
    Auto,
}

/// Converts persisted add-index arguments into the execution mode.  The Go
/// builder clears AUTO when an explicit option is present; preferring manual
/// here also preserves that rule for legacy or hand-crafted job payloads.
pub fn select_pre_split_mode(
    manual_rows: Option<Vec<Vec<Datum>>>,
    auto_pre_split: bool,
) -> PreSplitMode {
    match manual_rows {
        Some(rows) => PreSplitMode::Manual(rows),
        None if auto_pre_split => PreSplitMode::Auto,
        None => PreSplitMode::None,
    }
}

pub fn run_pre_split(
    mode: PreSplitMode,
    plan_auto: impl FnOnce() -> Result<AutoPreSplitPlan, String>,
    mut split: impl FnMut(&[Key]) -> Result<usize, SplitError>,
) -> Result<Option<usize>, SplitError> {
    match mode {
        PreSplitMode::None => Ok(None),
        PreSplitMode::Manual(rows) => {
            let keys = get_split_keys_from_value_list(0, 0, &rows)?;
            split(&keys).map(Some)
        }
        PreSplitMode::Auto => {
            let Ok(plan) = plan_auto() else {
                return Ok(None);
            };
            if plan.state != AutoPreSplitPlanState::Planned {
                return Ok(None);
            }
            Ok(split(&plan.split_keys).ok())
        }
    }
}
