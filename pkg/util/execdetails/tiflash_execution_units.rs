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

/// Presence bits describe fields carried by summaries, not task coverage.
pub type TiFlashUnitFields = u8;
pub const TiFlashUnitRows: TiFlashUnitFields = 1;
pub const TiFlashUnitHash: TiFlashUnitFields = 2;
pub const TiFlashUnitScan: TiFlashUnitFields = 4;
pub const TiFlashUnitNetwork: TiFlashUnitFields = 8;
const ALL_FIELDS: TiFlashUnitFields =
    TiFlashUnitRows | TiFlashUnitHash | TiFlashUnitScan | TiFlashUnitNetwork;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TiFlashExecutionUnits {
    pub Rows: u64,
    pub HashDistinctEntries: u64,
    pub HashBuildRows: u64,
    pub UserReadBytes: u64,
    pub InnerZoneSendBytes: u64,
    pub InterZoneSendBytes: u64,
    pub Observed: TiFlashUnitFields,
    pub Missing: TiFlashUnitFields,
    pub Invalid: bool,
}

fn tiFlashExecutionUnits(summary: &tipb::ExecutorExecutionSummary) -> TiFlashExecutionUnits {
    let mut units = TiFlashExecutionUnits::default();
    if let Some(rows) = summary.NumProducedRows {
        units.Rows = rows;
        units.Observed |= TiFlashUnitRows;
    }
    if let Some(stats) = summary.GetTiflashHashTableStats() {
        if let Some(size) = stats.Size_ {
            match stats.SizeKind {
                tipb::TiFlashHashTableSizeKind::DistinctKeyCount => {
                    units.HashDistinctEntries = size;
                    units.Observed |= TiFlashUnitHash;
                }
                tipb::TiFlashHashTableSizeKind::BuildRowCount => {
                    units.HashBuildRows = size;
                    units.Observed |= TiFlashUnitHash;
                }
                tipb::TiFlashHashTableSizeKind::Unknown(_) => {}
            }
        }
    }
    if let Some(bytes) = summary
        .GetTiflashScanContext()
        .and_then(|scan| scan.UserReadBytes)
        .or_else(|| {
            summary
                .GetColumnarScanContext()
                .and_then(|scan| scan.UserReadBytes)
        })
    {
        units.UserReadBytes = bytes;
        units.Observed |= TiFlashUnitScan;
    }
    if let Some(network) = summary.GetTiflashNetworkSummary() {
        units.InnerZoneSendBytes = network.InnerZoneSendBytes.unwrap_or(0);
        units.InterZoneSendBytes = network.InterZoneSendBytes.unwrap_or(0);
        if network.InnerZoneSendBytes.is_some() && network.InterZoneSendBytes.is_some() {
            units.Observed |= TiFlashUnitNetwork;
        }
    }
    units.Missing = ALL_FIELDS & !units.Observed;
    units
}

impl TiFlashExecutionUnits {
    fn merge(&mut self, other: Self) {
        self.Observed |= other.Observed;
        self.Missing |= other.Missing;
        self.Invalid |= other.Invalid;
        fn add(dst: &mut u64, value: u64, invalid: &mut bool) {
            if let Some(sum) = dst.checked_add(value) {
                *dst = sum;
            } else {
                *invalid = true;
            }
        }
        add(&mut self.Rows, other.Rows, &mut self.Invalid);
        add(
            &mut self.HashDistinctEntries,
            other.HashDistinctEntries,
            &mut self.Invalid,
        );
        add(
            &mut self.HashBuildRows,
            other.HashBuildRows,
            &mut self.Invalid,
        );
        add(
            &mut self.UserReadBytes,
            other.UserReadBytes,
            &mut self.Invalid,
        );
        add(
            &mut self.InnerZoneSendBytes,
            other.InnerZoneSendBytes,
            &mut self.Invalid,
        );
        add(
            &mut self.InterZoneSendBytes,
            other.InterZoneSendBytes,
            &mut self.Invalid,
        );
        self.Invalid |= self.Rows > i64::MAX as u64;
    }
}

impl RuntimeStatsColl {
    /// Records one consumed response or direct task report. The caller owns route deduplication.
    pub fn RecordTiFlashExecutionSummaries(
        &self,
        planIDs: &[i32],
        summaries: &[Option<tipb::ExecutorExecutionSummary>],
    ) {
        if summaries.is_empty() {
            return;
        }
        let mut map = self
            .tiFlashExecutionUnits
            .lock()
            .expect("TiFlash units lock poisoned");
        let mut seen = HashSet::new();
        for summary in summaries.iter().flatten() {
            let (id, ok) = getPlanIDFromExecutionSummary(summary);
            if !ok || id <= 0 || !planIDs.contains(&id) {
                continue;
            }
            let units = map.entry(id).or_default();
            if !seen.insert(id) {
                units.Invalid = true;
            } else {
                units.merge(tiFlashExecutionUnits(summary));
            }
        }
    }

    /// Returns an immutable value snapshot, keeping missing evidence separate from observed zero.
    pub fn GetTiFlashExecutionUnits(&self, planID: i32) -> (TiFlashExecutionUnits, bool) {
        let map = self
            .tiFlashExecutionUnits
            .lock()
            .expect("TiFlash units lock poisoned");
        map.get(&planID)
            .copied()
            .map_or((TiFlashExecutionUnits::default(), false), |units| {
                (units, true)
            })
    }
}
