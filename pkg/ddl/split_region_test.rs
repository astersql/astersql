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

use super::split_region::{
    SplitError, SplitTableInfo, encode_record_key, normalize_split_policy, pre_split_record_keys,
    wait_scatter_finished,
};
use std::cell::Cell;

#[test]
fn normalize_policy_accepts_one_region_like_go() {
    let policy = normalize_split_policy(
        vec!["0".to_owned()],
        vec!["10".to_owned()],
        1,
        Vec::new(),
        None,
    )
    .expect("Go accepts every positive region count");

    assert_eq!(policy.num, 1);
}

#[test]
fn normalize_policy_does_not_compare_expression_text_lexically() {
    normalize_split_policy(
        vec!["10".to_owned()],
        vec!["2".to_owned()],
        2,
        Vec::new(),
        None,
    )
    .expect("Go validates expressions here, not their restored string ordering");
}

#[test]
fn shard_pre_split_uses_table_prefix_and_non_negative_handle_space() {
    let info = SplitTableInfo {
        table_id: 42,
        partition_ids: Vec::new(),
        shard_row_id_bits: 4,
        pre_split_regions: 2,
        index_ids: Vec::new(),
    };

    let keys = pre_split_record_keys(&info).expect("valid shard split settings");
    let mut table_prefix = b"t".to_vec();
    table_prefix.extend_from_slice(&42_i64.to_be_bytes());

    assert_eq!(
        keys,
        vec![
            table_prefix,
            encode_record_key(42, 1_i64 << 61),
            encode_record_key(42, 1_i64 << 62),
            encode_record_key(42, 3_i64 << 61),
        ]
    );
}

#[test]
fn shard_pre_split_preserves_physical_table_order_like_go() {
    let info = SplitTableInfo {
        table_id: 42,
        partition_ids: vec![9, 3],
        shard_row_id_bits: 1,
        pre_split_regions: 0,
        index_ids: Vec::new(),
    };

    let keys = pre_split_record_keys(&info).expect("valid shard split settings");
    assert_eq!(&keys[0][1..9], &9_i64.to_be_bytes());
    assert_eq!(&keys[1][1..9], &3_i64.to_be_bytes());
}

#[test]
fn scatter_wait_stops_after_the_first_non_pd_error_like_go() {
    let visited = Cell::new(0_u64);
    let results = [Ok(()), Err(()), Ok(())].into_iter().map(|result| {
        visited.set(visited.get() + 1);
        result
    });

    assert_eq!(
        wait_scatter_finished(results),
        Err(SplitError::ScatterFailed(1))
    );
    assert_eq!(visited.get(), 2);
}

#[derive(Default)]
struct RecordingStore {
    calls: std::cell::RefCell<Vec<(Vec<Vec<u8>>, bool, Option<i64>)>>,
    waits: std::cell::RefCell<Vec<u64>>,
    fail_call: Option<usize>,
    wait_failure: Option<bool>,
}
impl astersql_kv::SplittableStore for RecordingStore {
    fn SplitRegions(
        &self,
        _: &astersql_kv::Context,
        keys: &[Vec<u8>],
        scatter: bool,
        group: Option<i64>,
    ) -> Result<Vec<u64>, astersql_kv::errors::SharedError> {
        self.calls
            .borrow_mut()
            .push((keys.to_vec(), scatter, group));
        let call = self.calls.borrow().len();
        if self.fail_call == Some(call) {
            return Err(astersql_kv::errors::New("split unavailable"));
        }
        Ok(vec![call as u64])
    }
    fn WaitScatterRegionFinish(
        &self,
        _: &astersql_kv::Context,
        region: u64,
        _: i32,
    ) -> Result<(), astersql_kv::errors::SharedError> {
        self.waits.borrow_mut().push(region);
        if region == 1 {
            if let Some(pd) = self.wait_failure {
                return Err(if pd {
                    astersql_kv::errors::SharedError::new(
                        astersql_store_driver_error::PdError::Other(
                            "PD scatter request failed".into(),
                        ),
                    )
                } else {
                    astersql_kv::errors::New("context canceled")
                });
            }
        }
        Ok(())
    }
    fn CheckRegionInScattering(&self, _: u64) -> Result<bool, astersql_kv::errors::SharedError> {
        Ok(false)
    }
}

fn scatter_table() -> astersql_meta_model::TableInfo {
    use astersql_meta_model::{IndexInfo, PartitionDefinition, PartitionInfo, TableInfo};
    TableInfo {
        ID: 100,
        ShardRowIDBits: 2,
        PreSplitRegions: 1,
        Partition: Some(PartitionInfo {
            Enable: true,
            Definitions: vec![
                PartitionDefinition {
                    ID: 101,
                    ..Default::default()
                },
                PartitionDefinition {
                    ID: 102,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        Indices: vec![
            IndexInfo {
                ID: 11,
                ..Default::default()
            },
            IndexInfo {
                ID: 22,
                Global: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[test]
fn partition_presplit_separates_physical_keys_from_scatter_groups() {
    for (scope, group) in [("table", 100), ("global", -1), ("off", 100)] {
        let store = RecordingStore::default();
        let expr = astersql_expression_exprstatic::NewExprContext(Vec::new());
        super::split_region::split_table_regions(
            &Default::default(),
            &expr,
            &store,
            &scatter_table(),
            scope,
        );
        let calls = store.calls.borrow();
        assert_eq!(calls.len(), 5);
        for (_, scatter, id) in calls.iter() {
            assert_eq!(*scatter, scope != "off");
            assert_eq!(*id, Some(group));
        }
        assert_eq!(
            calls[0].0,
            vec![astersql_tablecodec::EncodeTableIndexPrefix(100, 22).0]
        );
        for (offset, physical) in [(1, 101), (3, 102)] {
            assert_eq!(
                calls[offset].0,
                vec![
                    astersql_tablecodec::GenTablePrefix(physical).0,
                    astersql_tablecodec::EncodeRecordKey(
                        astersql_tablecodec::GenTableRecordPrefix(physical),
                        Box::new(astersql_kv::IntHandle(1_i64 << 62))
                    )
                    .0
                ]
            );
            assert_eq!(
                calls[offset + 1].0,
                vec![astersql_tablecodec::EncodeTableIndexPrefix(physical, 12).0]
            );
        }
        assert_eq!(
            store.waits.borrow().len(),
            if scope == "off" { 0 } else { 5 }
        );
    }
}

#[test]
fn partitioned_split_policies_keep_record_keys_on_physical_ids() {
    astersql_planner_core::InstallPlannerExpressionFactory().unwrap();
    use astersql_meta_model::{IndexInfo, RegionSplitPolicy};
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(
            "create table t (id bigint primary key, val bigint, index idx_local(val)) \
             partition by range (id) (partition p0 values less than (100), \
             partition p1 values less than (200))",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<astersql_parser_ast::CreateTableStmt>()
        .unwrap();
    let context = astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let mut table = crate::BuildTableInfoFromAST(&context, create).unwrap();
    table.ID = 100;
    table.Partition.as_mut().unwrap().Definitions[0].ID = 101;
    table.Partition.as_mut().unwrap().Definitions[1].ID = 102;

    let policy = RegionSplitPolicy {
        Lower: vec!["0".into()],
        Upper: vec!["10000".into()],
        Regions: 2,
        ..Default::default()
    };
    table.TableSplitPolicy = Some(policy.clone());
    let local_index = table
        .Indices
        .iter()
        .position(|index| index.Name.L == "idx_local")
        .expect("expected idx_local in table info");
    table.Indices[local_index].ID = 11;
    table.Indices[local_index].RegionSplitPolicy = Some(policy.clone());
    table.Indices.push(IndexInfo {
        ID: 22,
        Global: true,
        Columns: table.Indices[local_index].Columns.clone(),
        RegionSplitPolicy: Some(policy),
        ..Default::default()
    });

    let store = RecordingStore::default();
    let expr = astersql_expression_exprstatic::NewExprContext(Vec::new());
    super::split_region::split_table_regions(&Default::default(), &expr, &store, &table, "off");

    let calls = store.calls.borrow();
    assert_eq!(calls.len(), 5);
    assert!(calls.iter().all(|(keys, _, _)| !keys.is_empty()));
    assert!(calls[0].0.iter().all(|key| {
        matches!(
            astersql_tablecodec::DecodeKeyHead(astersql_kv::Key(key.clone())),
            Ok((100, 22 | 23, false))
        )
    }));
    for (table_call, index_call, physical) in [(1, 2, 101), (3, 4, 102)] {
        assert!(calls[table_call].0.iter().all(|key| {
            matches!(
                astersql_tablecodec::DecodeKeyHead(astersql_kv::Key(key.clone())),
                Ok((id, _, true)) if id == physical
            )
        }));
        assert!(calls[index_call].0.iter().all(|key| {
            matches!(
                astersql_tablecodec::DecodeKeyHead(astersql_kv::Key(key.clone())),
                Ok((id, 11 | 12, false)) if id == physical
            )
        }));
    }
    assert!(
        calls
            .iter()
            .flat_map(|(keys, _, _)| keys)
            .all(|key| !matches!(
                astersql_tablecodec::DecodeKeyHead(astersql_kv::Key(key.clone())),
                Ok((100, _, true))
            ))
    );
}

#[test]
fn policies_use_persisted_scope_and_continue_after_storage_failure() {
    astersql_planner_core::InstallPlannerExpressionFactory().unwrap();
    use astersql_meta_model::RegionSplitPolicy;
    let mut parser = astersql_parser::New();
    let statement = parser
        .ParseOneStmt(
            "create table t (id bigint primary key, index idx (id))",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<astersql_parser_ast::CreateTableStmt>()
        .unwrap();
    let context = astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let mut table = crate::BuildTableInfoFromAST(&context, create).unwrap();
    table.ID = 200;
    table.TableSplitPolicy = Some(RegionSplitPolicy {
        Lower: vec!["0".into()],
        Upper: vec!["10000".into()],
        Regions: 2,
        ..Default::default()
    });
    // Original Go regression: table policy only, while the applying worker's
    // scatter setting is irrelevant to this persisted scope argument.
    for (scope, group) in [("table", 200), ("global", -1)] {
        let store = RecordingStore::default();
        let expr = astersql_expression_exprstatic::NewExprContext(Vec::new());
        super::split_region::split_table_regions(&Default::default(), &expr, &store, &table, scope);
        let calls = store.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1);
        assert_eq!(calls[0].2, Some(group));
        assert!(!calls[0].0.is_empty());
    }
    table.Indices[0].RegionSplitPolicy = Some(RegionSplitPolicy {
        Lower: vec!["0".into()],
        Upper: vec!["10000".into()],
        Regions: 2,
        ..Default::default()
    });
    for (scope, group) in [("table", 200), ("global", -1), ("off", 200)] {
        for failure in [None, Some(1)] {
            let store = RecordingStore {
                fail_call: failure,
                ..Default::default()
            };
            let expr = astersql_expression_exprstatic::NewExprContext(Vec::new());
            let ids = super::split_region::split_table_regions(
                &Default::default(),
                &expr,
                &store,
                &table,
                scope,
            );
            assert_eq!(store.calls.borrow().len(), 2);
            for (keys, scatter, id) in store.calls.borrow().iter() {
                assert!(!keys.is_empty());
                assert_eq!(*scatter, scope != "off");
                assert_eq!(*id, Some(group));
                for key in keys {
                    assert_eq!(&key[..9], &astersql_tablecodec::GenTablePrefix(200).0);
                }
            }
            assert_eq!(
                ids,
                if failure.is_some() {
                    vec![2]
                } else {
                    vec![1, 2]
                }
            );
        }
    }
}

#[test]
fn presplit_wait_continues_for_pd_errors_but_stops_for_other_errors() {
    for (pd, waits) in [(true, vec![1, 2, 3, 4, 5]), (false, vec![1])] {
        let store = RecordingStore {
            wait_failure: Some(pd),
            ..Default::default()
        };
        let expr = astersql_expression_exprstatic::NewExprContext(Vec::new());
        super::split_region::split_table_regions(
            &Default::default(),
            &expr,
            &store,
            &scatter_table(),
            "global",
        );
        assert_eq!(*store.waits.borrow(), waits);
    }
}
