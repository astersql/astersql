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

// `fetchShowAffinity` 单元测试：验证亲和组状态映射为展示行。

use std::collections::HashMap;

use crate::show_affinity::{
    AffinityLevel, AffinityPartition, AffinitySchemaTables, AffinityState, AffinityTable,
    ShowAffinityRuntime, ShowAffinityValue, fetchShowAffinity,
};

/// 测试用运行时：可组合过滤器、库表清单与 PD 状态。
struct AffinityRuntime {
    field_filter: Option<String>,
    pattern_result: bool,
    tables: Vec<AffinitySchemaTables>,
    states: Result<HashMap<String, AffinityState>, &'static str>,
    rows: Vec<Vec<ShowAffinityValue>>,
}

impl Default for AffinityRuntime {
    fn default() -> Self {
        Self {
            field_filter: None,
            pattern_result: true,
            tables: vec![AffinitySchemaTables {
                database_name: "test".into(),
                tables: vec![AffinityTable {
                    id: 11,
                    name: "T".into(),
                    lowercase_name: "t".into(),
                    level: Some(AffinityLevel::Table),
                    partitions: Vec::new(),
                }],
            }],
            states: Ok(HashMap::from([(
                "table-11".into(),
                AffinityState {
                    leader_store_id: 1,
                    voter_store_ids: vec![2, 3],
                    phase: "stable".into(),
                    region_count: 8,
                    affinity_region_count: 7,
                },
            )])),
            rows: Vec::new(),
        }
    }
}

impl ShowAffinityRuntime for AffinityRuntime {
    type Context = ();
    type Error = &'static str;

    fn field_filter(&self) -> Option<String> {
        self.field_filter.clone()
    }
    fn field_pattern_matches(&self, _: &str) -> bool {
        self.pattern_result
    }
    fn tables_with_affinity(&self) -> Vec<AffinitySchemaTables> {
        self.tables.clone()
    }
    fn table_group_id(&self, table_id: i64) -> String {
        format!("table-{table_id}")
    }
    fn partition_group_id(&self, table_id: i64, partition_id: i64) -> String {
        format!("partition-{table_id}-{partition_id}")
    }
    fn all_group_states(
        &mut self,
        _: &mut (),
    ) -> Result<HashMap<String, AffinityState>, &'static str> {
        self.states.clone()
    }
    fn append_row(&mut self, row: Vec<ShowAffinityValue>) {
        self.rows.push(row);
    }
}

/// Go 在精确过滤命中后仍会应用同时存在的 LIKE 模式。
#[test]
fn exact_filter_does_not_bypass_like_pattern() {
    let mut runtime = AffinityRuntime {
        field_filter: Some("t".into()),
        pattern_result: false,
        ..AffinityRuntime::default()
    };

    fetchShowAffinity(&mut runtime, &mut ()).unwrap();

    assert!(runtime.rows.is_empty());
}

/// 分区级亲和按每个分区展开；无状态的组保持原生 NULL。
#[test]
fn partition_affinity_and_missing_state_match_go_rows() {
    let mut runtime = AffinityRuntime {
        field_filter: Some("parts".into()),
        tables: vec![AffinitySchemaTables {
            database_name: "db2".into(),
            tables: vec![
                AffinityTable {
                    id: 10,
                    name: "plain".into(),
                    lowercase_name: "plain".into(),
                    level: None,
                    partitions: Vec::new(),
                },
                AffinityTable {
                    id: 20,
                    name: "Parts".into(),
                    lowercase_name: "parts".into(),
                    level: Some(AffinityLevel::Partition),
                    partitions: vec![
                        AffinityPartition {
                            id: 21,
                            name: "p0".into(),
                        },
                        AffinityPartition {
                            id: 22,
                            name: "p1".into(),
                        },
                    ],
                },
                AffinityTable {
                    id: 30,
                    name: "Other".into(),
                    lowercase_name: "other".into(),
                    level: Some(AffinityLevel::Other),
                    partitions: Vec::new(),
                },
            ],
        }],
        states: Ok(HashMap::from([(
            "partition-20-21".into(),
            AffinityState {
                leader_store_id: 0,
                voter_store_ids: Vec::new(),
                phase: "pending".into(),
                region_count: 3,
                affinity_region_count: 1,
            },
        )])),
        ..AffinityRuntime::default()
    };

    fetchShowAffinity(&mut runtime, &mut ()).unwrap();

    assert_eq!(
        runtime.rows,
        vec![
            vec![
                ShowAffinityValue::String("db2".into()),
                ShowAffinityValue::String("Parts".into()),
                ShowAffinityValue::String("p0".into()),
                ShowAffinityValue::Null,
                ShowAffinityValue::Null,
                ShowAffinityValue::String("Pending".into()),
                ShowAffinityValue::U64(3),
                ShowAffinityValue::U64(1),
            ],
            vec![
                ShowAffinityValue::String("db2".into()),
                ShowAffinityValue::String("Parts".into()),
                ShowAffinityValue::String("p1".into()),
                ShowAffinityValue::Null,
                ShowAffinityValue::Null,
                ShowAffinityValue::Null,
                ShowAffinityValue::Null,
                ShowAffinityValue::Null,
            ],
        ]
    );
}

/// 阶段映射保留 Go 的三个已知文案和未知值透传分支。
#[test]
fn preparing_and_unknown_phases_match_go_status_mapping() {
    let table = |id, name: &str| AffinityTable {
        id,
        name: name.into(),
        lowercase_name: name.into(),
        level: Some(AffinityLevel::Table),
        partitions: Vec::new(),
    };
    let state = |phase: &str| AffinityState {
        leader_store_id: 0,
        voter_store_ids: Vec::new(),
        phase: phase.into(),
        region_count: 0,
        affinity_region_count: 0,
    };
    let mut runtime = AffinityRuntime {
        tables: vec![AffinitySchemaTables {
            database_name: "test".into(),
            tables: vec![table(1, "a"), table(2, "b")],
        }],
        states: Ok(HashMap::from([
            ("table-1".into(), state("preparing")),
            ("table-2".into(), state("custom")),
        ])),
        ..AffinityRuntime::default()
    };

    fetchShowAffinity(&mut runtime, &mut ()).unwrap();

    assert_eq!(
        runtime.rows[0][5],
        ShowAffinityValue::String("Preparing".into())
    );
    assert_eq!(
        runtime.rows[1][5],
        ShowAffinityValue::String("custom".into())
    );
}

/// PD 查询错误必须原样向上传播，且不能输出半成品行。
#[test]
fn group_state_error_is_propagated_without_rows() {
    let mut runtime = AffinityRuntime {
        states: Err("pd unavailable"),
        ..AffinityRuntime::default()
    };

    assert_eq!(
        fetchShowAffinity(&mut runtime, &mut ()),
        Err("pd unavailable")
    );
    assert!(runtime.rows.is_empty());
}

/// 核对 leader / voters / Stable 阶段与 Region 计数是否正确写入结果行。
#[test]
fn show_affinity_maps_group_state_to_visible_row() {
    let mut runtime = AffinityRuntime::default();
    fetchShowAffinity(&mut runtime, &mut ()).unwrap();
    assert_eq!(
        runtime.rows,
        vec![vec![
            ShowAffinityValue::String("test".into()),
            ShowAffinityValue::String("T".into()),
            ShowAffinityValue::Null,
            ShowAffinityValue::U64(1),
            ShowAffinityValue::String("2,3".into()),
            ShowAffinityValue::String("Stable".into()),
            ShowAffinityValue::U64(8),
            ShowAffinityValue::U64(7),
        ]]
    );
}
