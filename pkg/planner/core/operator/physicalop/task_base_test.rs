// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode, Stats};
use crate::task_base::{CopTask, SimpleWarnings};

fn plan(row_count: f64) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id: row_count as i64,
        kind: PhysicalKind::Other("scan".into()),
        schema: Vec::new(),
        children: Vec::new(),
        stats: Stats {
            row_count,
            version: 0,
        },
        required_properties: Vec::new(),
    }
}

#[test]
fn cop_task_count_follows_the_active_plan_like_go() {
    let mut task = CopTask {
        index_plan: Some(plan(11.0)),
        table_plan: Some(plan(29.0)),
        ..CopTask::default()
    };

    assert_eq!(task.count(), 11.0);
    task.index_plan_finished = true;
    assert_eq!(task.count(), 29.0);
}

#[test]
fn copying_warning_sources_does_not_apply_the_append_limit_again() {
    let mut first = SimpleWarnings::default();
    let mut second = SimpleWarnings::default();
    for _ in 0..u16::MAX {
        first.append_warning("first");
        second.append_note("second");
    }

    let mut combined = SimpleWarnings::default();
    combined.copy_from(&[&first, &second]);

    assert_eq!(combined.warning_count(), usize::from(u16::MAX) * 2);
}

#[test]
fn cop_conversion_finishes_index_stats_before_building_the_reader() {
    let task = CopTask {
        index_plan: Some(PhysicalPlanNode {
            stats: Stats {
                row_count: 11.0,
                version: 7,
            },
            ..plan(11.0)
        }),
        table_plan: Some(PhysicalPlanNode {
            stats: Stats {
                row_count: 29.0,
                version: 9,
            },
            ..plan(29.0)
        }),
        ..CopTask::default()
    };

    let root = task.convert_to_root_task();
    let reader = root.plan.expect("valid reader");
    assert_eq!(reader.stats.row_count, 11.0);
    assert_eq!(reader.stats.version, 9);
    assert!(
        !task.index_plan_finished,
        "conversion must mutate only its copy"
    );
}
