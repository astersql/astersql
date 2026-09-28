// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

use crate::*;
use astersql_lightning_pkg_precheck::{Critical, Warn};

#[test]
fn simple_template_matches_go_collection_contract() {
    let mut template = NewSimpleTemplate();
    template.Collect(Critical, true, "critical pass".into());
    template.Collect(Warn, false, "warning failure".into());
    template.Collect(Critical, false, "first critical failure".into());
    template.Collect(Critical, false, "second critical failure".into());

    assert!(!template.Success());
    assert_eq!(template.FailedCount(Warn), 1);
    assert_eq!(template.FailedCount(Critical), 2);
    assert_eq!(
        template.FailedMsg(),
        "first critical failure;\nsecond critical failure"
    );
}

#[test]
fn simple_template_matches_go_render_limits() {
    let mut template = NewSimpleTemplate();
    template.Collect(Warn, false, "x".repeat(300));

    let output = template.Output();
    assert!(
        output.contains("CHECK ITEM"),
        "Go's default table style uppercases headers"
    );
    assert!(
        output.lines().all(|line| visible_width(line) <= 170),
        "Go configures table.Writer with SetAllowedRowLength(170)"
    );
}

fn visible_width(line: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for ch in line.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            width += 1;
        }
    }
    width
}
