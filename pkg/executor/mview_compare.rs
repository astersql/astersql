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

use astersql_types::datum::Datum;

pub const DIFFER: &str = "mview_differ";
pub const VACUUM: &str = "mview_vacuum";
pub const EXCESSIVE: &str = "mview_excessive";

/// Full-join layout uses a visible NOT NULL column to distinguish absent rows.
/// Payload equality uses Datum's DECIMAL value comparison and binary strings.
pub struct CompareLayout {
    pub columns: usize,
    pub marker: usize,
    pub group_keys: Vec<usize>,
}
impl CompareLayout {
    pub fn classify<'a>(
        &self,
        joined: &'a [Datum],
    ) -> Result<Option<(&'a [Datum], &'static str)>, String> {
        if joined.len() != 2 * self.columns || self.marker >= self.columns {
            return Err("compare materialized view: invalid joined row layout".into());
        }
        let (current, recomputed) = joined.split_at(self.columns);
        if recomputed[self.marker].IsNull() {
            return Ok(Some((current, EXCESSIVE)));
        }
        if current[self.marker].IsNull() {
            return Ok(Some((recomputed, VACUUM)));
        }
        let changed = current
            .iter()
            .zip(recomputed)
            .enumerate()
            .any(|(offset, (a, b))| !self.group_keys.contains(&offset) && !payload_equal(a, b));
        Ok(changed.then_some((current, DIFFER)))
    }
}

pub fn differ_type_column_name(names: impl IntoIterator<Item = String>) -> String {
    let used = names
        .into_iter()
        .map(|name| name.to_lowercase())
        .collect::<std::collections::HashSet<_>>();
    let base = "_Differ_type_";
    let mut name = base.to_owned();
    let mut suffix = 1;
    while used.contains(&name.to_lowercase()) {
        name = format!("{base}{suffix}");
        suffix += 1;
    }
    name
}

pub fn output_batch_bytes(txn_limit: u64) -> usize {
    (txn_limit / 4).clamp(1, 16 * 1024 * 1024) as usize
}

fn payload_equal(a: &Datum, b: &Datum) -> bool {
    use astersql_types::datum::KindMysqlDecimal;
    if a.Kind() == KindMysqlDecimal && b.Kind() == KindMysqlDecimal {
        a.GetMysqlDecimal().Compare(&b.GetMysqlDecimal()) == 0
    } else {
        a.Equals(b)
    }
}
