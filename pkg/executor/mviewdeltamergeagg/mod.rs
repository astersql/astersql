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

use astersql_types::datum::{Datum, KindFloat32, KindFloat64, KindInt64, KindNull, KindUint64};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggKind {
    Count,
    Sum,
    Min,
    Max,
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub kind: AggKind,
    pub output_col: usize,
    pub delta_col: usize,
    pub removed_col: Option<usize>,
    pub count_dependency: Option<usize>,
    pub nullable: bool,
    pub unsigned: bool,
}

pub trait MinMaxRecompute: Send + Sync {
    fn recompute(&self, row: usize, mapping: usize) -> Result<Option<Datum>>;
}

pub fn validate(mappings: &[Mapping], delta_cols: usize) -> Result<()> {
    for (idx, m) in mappings.iter().enumerate() {
        if m.output_col < delta_cols {
            return Err(format!("mapping {idx} output depends on delta side"));
        }
        if m.delta_col >= delta_cols {
            return Err(format!("mapping {idx} delta depends on computed side"));
        }
        if m.kind == AggKind::Count && m.unsigned {
            return Err("COUNT output must be signed".into());
        }
        if m.kind == AggKind::Sum && m.unsigned {
            return Err("SUM delta must be signed".into());
        }
        if matches!(m.kind, AggKind::Min | AggKind::Max) && m.removed_col.is_none() {
            return Err(format!("mapping {idx} MIN/MAX removed delta is absent"));
        }
        if m.nullable
            && matches!(m.kind, AggKind::Sum | AggKind::Min | AggKind::Max)
            && m.count_dependency.is_none()
        {
            return Err(format!(
                "mapping {idx} nullable aggregate requires COUNT dependency"
            ));
        }
        if let Some(dep) = m.count_dependency
            && dep < delta_cols
        {
            return Err(format!("mapping {idx} COUNT dependency comes from delta"));
        }
    }
    Ok(())
}

fn numeric_add(current: &Datum, delta: &Datum) -> Result<Datum> {
    let mut out = Datum::default();
    match (current.Kind(), delta.Kind()) {
        (KindInt64, KindInt64) => out.SetInt64(
            current
                .GetInt64()
                .checked_add(delta.GetInt64())
                .ok_or("integer SUM overflow")?,
        ),
        (KindUint64, KindInt64) => {
            let value = current.GetUint64() as i128 + delta.GetInt64() as i128;
            if !(0..=u64::MAX as i128).contains(&value) {
                return Err("unsigned SUM overflow".into());
            }
            out.SetUint64(value as u64);
        }
        (KindFloat32 | KindFloat64, KindFloat32 | KindFloat64) => {
            out.SetFloat64(current.GetFloat64() + delta.GetFloat64())
        }
        _ => return Err("SUM type mismatch".into()),
    }
    Ok(out)
}

pub fn merge_rows(
    rows: &mut [Vec<Datum>],
    mappings: &[Mapping],
    delta_cols: usize,
    recompute: Option<&dyn MinMaxRecompute>,
) -> Result<Vec<Vec<bool>>> {
    validate(mappings, delta_cols)?;
    let mut touched = vec![vec![false; rows.first().map_or(0, Vec::len)]; rows.len()];
    for (row_idx, row) in rows.iter_mut().enumerate() {
        for (mapping_idx, m) in mappings.iter().enumerate() {
            if m.output_col >= row.len() || m.delta_col >= row.len() {
                return Err("mapping column out of range".into());
            }
            let before = row[m.output_col].clone();
            let delta = row[m.delta_col].clone();
            let next = match m.kind {
                AggKind::Count => numeric_add(&before, &delta)?,
                AggKind::Sum => {
                    if delta.IsNull() {
                        before.clone()
                    } else if before.IsNull() {
                        delta.clone()
                    } else {
                        numeric_add(&before, &delta)?
                    }
                }
                AggKind::Min | AggKind::Max => {
                    let removed = m
                        .removed_col
                        .and_then(|col| row.get(col))
                        .cloned()
                        .unwrap_or_default();
                    if removed.Kind() != KindNull && removed.Equals(&before) {
                        recompute
                            .ok_or("MIN/MAX recompute executor is absent")?
                            .recompute(row_idx, mapping_idx)?
                            .unwrap_or_default()
                    } else if delta.Kind() == KindNull {
                        before.clone()
                    } else if before.Kind() == KindNull {
                        delta.clone()
                    } else {
                        let take_delta = match (before.Kind(), delta.Kind()) {
                            (KindInt64, KindInt64) => {
                                if m.kind == AggKind::Min {
                                    delta.GetInt64() < before.GetInt64()
                                } else {
                                    delta.GetInt64() > before.GetInt64()
                                }
                            }
                            (KindUint64, KindUint64) => {
                                if m.kind == AggKind::Min {
                                    delta.GetUint64() < before.GetUint64()
                                } else {
                                    delta.GetUint64() > before.GetUint64()
                                }
                            }
                            (KindFloat32 | KindFloat64, KindFloat32 | KindFloat64) => {
                                if m.kind == AggKind::Min {
                                    delta.GetFloat64() < before.GetFloat64()
                                } else {
                                    delta.GetFloat64() > before.GetFloat64()
                                }
                            }
                            _ => return Err("MIN/MAX type mismatch".into()),
                        };
                        if take_delta {
                            delta.clone()
                        } else {
                            before.clone()
                        }
                    }
                }
            };
            if !next.Equals(&before) {
                row[m.output_col] = next;
                touched[row_idx][m.output_col] = true;
            }
        }
    }
    Ok(touched)
}
