// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::{AnalyzeResult, NewFMSketch};

// Mirrors AnalyzeResult.DestroyAndPutToPool in analyze.go: assigning nil to
// Fms releases the slice backing storage instead of retaining it for reuse.
#[test]
fn destroy_analyze_result_releases_fm_sketch_storage() {
    let mut sketches = Vec::with_capacity(8);
    sketches.push(NewFMSketch(1));
    let mut result = AnalyzeResult {
        Hist: Vec::new(),
        Cms: Vec::new(),
        TopNs: Vec::new(),
        Fms: sketches,
        IsIndex: 0,
    };

    result.DestroyAndPutToPool();

    assert!(result.Fms.is_empty());
    assert_eq!(result.Fms.capacity(), 0);
}
