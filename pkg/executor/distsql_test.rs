// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DistSQL（分布式 SQL 下推）辅助函数的单元测试。
//
// 覆盖批大小计算、多 range 归并排序判定，以及 IndexLookUp 路径上
// 「期望 handle 集合相对已获得集合」的差集规则。

use crate::distsql::{
    ByItem, CalculateBatchSize, Datum, GetLackHandles, Handle, getDatumRow, needMergeSort,
};
use std::collections::BTreeSet;

/// 批大小、merge-sort 判定与缺失 handle 差集与读路径语义一致。
#[test]
fn distsql_batch_merge_and_missing_handle_rules_match_read_path() {
    // estimated 较大时按 2 次幂增长；estimated 小于初始批大小时仍保留 initial。
    assert_eq!(CalculateBatchSize(100, 32, 64), 64);
    assert_eq!(CalculateBatchSize(2, 32, 64), 32);
    // 有排序项且 range 数 > 1 才需要 merge sort。
    assert!(needMergeSort(
        &[ByItem {
            offset: 0,
            descending: false
        }],
        2
    ));
    assert!(!needMergeSort(&[], 2));
    let mut obtained = BTreeSet::from([Handle::Int(2)]);
    assert_eq!(
        GetLackHandles(&[Handle::Int(1), Handle::Int(2)], &mut obtained),
        vec![Handle::Int(1)]
    );
    assert!(obtained.is_empty());
}

/// Go consumes each obtained handle once, so duplicate index handles remain visible as missing.
#[test]
fn get_lack_handles_consumes_obtained_handles_and_reports_duplicates() {
    let mut obtained = BTreeSet::from([Handle::Int(1), Handle::Int(2)]);
    assert_eq!(
        GetLackHandles(
            &[Handle::Int(1), Handle::Int(1), Handle::Int(2)],
            &mut obtained,
        ),
        vec![Handle::Int(1)]
    );
    assert!(obtained.is_empty());
}

/// Go copies only columns that have matching field metadata.
#[test]
fn get_datum_row_stops_at_the_available_field_metadata() {
    let row = vec![Datum::Signed(1), Datum::Signed(2), Datum::Signed(3)];
    assert_eq!(getDatumRow(&row, &[0, 1]), row[..2]);
}

fn lookup_workers(
    check: bool,
) -> (
    crate::distsql::indexWorker<crate::table_readers_required_rows_test::RequiredRowsBackend>,
    crate::distsql::tableWorker<crate::table_readers_required_rows_test::RequiredRowsBackend>,
) {
    lookup_workers_with_rows(check, vec![vec![Datum::Signed(0)]])
}

fn lookup_workers_with_rows(
    check: bool,
    rows: Vec<crate::distsql::Row>,
) -> (
    crate::distsql::indexWorker<crate::table_readers_required_rows_test::RequiredRowsBackend>,
    crate::distsql::tableWorker<crate::table_readers_required_rows_test::RequiredRowsBackend>,
) {
    use crate::distsql::*;
    use std::sync::{Arc, Mutex, mpsc};
    let backend = Arc::new(
        crate::table_readers_required_rows_test::RequiredRowsBackend::new(
            rows,
            std::time::Duration::ZERO,
        ),
    );
    let stats = Arc::new(Mutex::new(IndexLookUpRunTimeStats::default()));
    let request = Request {
        ranges: vec![],
        descending: false,
        keep_order: true,
        concurrency: 1,
        plan_ids: vec![],
        collect_stats: false,
    };
    (
        indexWorker {
            backend: backend.clone(),
            table_tx: mpsc::sync_channel(1).0,
            ranges: vec![],
            request: request.clone(),
            handle_offsets: vec![0],
            common_handle: false,
            partition_mode: false,
            keep_order: true,
            check_index_value: check,
            pushed_limit: None,
            batch_size: 100,
            max_batch_size: 100,
            scanned_keys: 0,
            stats: stats.clone(),
            adaptive_limit_controller: None,
        },
        tableWorker {
            backend,
            table_id: 1,
            request,
            handle_idx: vec![0],
            common_handle: false,
            keep_order: true,
            weak_consistency: false,
            check_index_value: check,
            stats,
        },
    )
}

#[test]
fn distsql_admin_deduplicates_handles_before_split_point_routing() {
    let (index, mut table) = lookup_workers(true);
    let mut task =
        index.buildTableTask(0, vec![Handle::Int(0); 3], vec![vec![Datum::Signed(0)]; 3]);
    assert_eq!(
        task.handles,
        vec![Handle::Int(0)],
        "one point request per handle"
    );
    table
        .executeTask(&mut task)
        .expect("all repeated index rows match");
    assert_eq!(task.rows, vec![vec![Datum::Signed(0)]]);
}

#[test]
fn distsql_admin_checks_every_original_offset_and_restores_success() {
    let (index, mut table) = lookup_workers(true);
    for corrupt in [1, 3] {
        let mut index_rows = vec![vec![Datum::Signed(0)]; 4];
        index_rows[corrupt].push(Datum::Signed(99));
        let mut task = index.buildTableTask(0, vec![Handle::Int(0); 4], index_rows);
        assert!(table.executeTask(&mut task).is_err());
        let mut repaired =
            index.buildTableTask(1, vec![Handle::Int(0); 4], vec![vec![Datum::Signed(0)]; 4]);
        table
            .executeTask(&mut repaired)
            .expect("repaired duplicate entries");
    }
}

#[test]
fn distsql_normal_lookup_preserves_duplicates_and_admin_keep_order() {
    let (index, _) = lookup_workers(false);
    let handles = vec![Handle::Int(1), Handle::Int(0), Handle::Int(1)];
    assert_eq!(
        index.buildTableTask(0, handles.clone(), vec![]).handles,
        handles
    );
    // Distinct checksums make a positional comparison fail when storage
    // returns handle order rather than index order.
    let row0 = vec![Datum::Signed(0)];
    let row1 = vec![Datum::Signed(1), Datum::Signed(99)];
    let (index, mut table) = lookup_workers_with_rows(true, vec![row0.clone(), row1.clone()]);
    let mut task = index.buildTableTask(
        0,
        vec![Handle::Int(1), Handle::Int(0), Handle::Int(1)],
        vec![row1.clone(), row0.clone(), row1.clone()],
    );
    table
        .executeTask(&mut task)
        .expect("compare by handle and restore index order");
    assert_eq!(task.rows, vec![row1, row0]);
    assert_eq!(task.row_idx, vec![0, 1]);
}

#[test]
fn distsql_admin_rejects_invalid_offsets_without_panicking() {
    use crate::distsql::DistSqlError;
    let (index, table) = lookup_workers(true);
    let task = index.buildTableTask(0, vec![Handle::Int(0)], vec![vec![Datum::Signed(0)]]);
    for offsets in [vec![], vec![10], vec![0, 0]] {
        let mut invalid = task.clone();
        invalid.index_order.insert(Handle::Int(0), offsets);
        assert!(matches!(
            table.compareData(&invalid, &[vec![Datum::Signed(0)]]),
            Err(DistSqlError::InvalidPlan(_))
        ));
    }
    let mut unmapped = task.clone();
    unmapped.index_rows.push(vec![Datum::Signed(0)]);
    assert!(matches!(
        table.compareData(&unmapped, &[vec![Datum::Signed(0)]]),
        Err(DistSqlError::InvalidPlan(_))
    ));
    assert!(matches!(
        table.compareData(&task, &[]),
        Err(DistSqlError::Inconsistent { .. })
    ));
    assert!(matches!(
        table.compareData(&task, &[vec![Datum::Signed(1)]]),
        Err(DistSqlError::InvalidPlan(_))
    ));
}

mod split_region_routing {
    use super::*;
    use astersql_store_copr::{
        BatchError, BatchResult, KeyLocation, KeyRange, RegionCache, RegionCacheBackend,
        RegionMeta, RegionStore, RegionVerId, ReplicaReadType, RpcContext,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    #[derive(Default)]
    struct SplitRegionBackend {
        locations: Vec<KeyLocation>,
        rpc_context: Option<RpcContext>,
        send_fail_count: Arc<AtomicUsize>,
    }

    impl RegionCacheBackend for SplitRegionBackend {
        fn batch_locate_key_ranges(
            &self,
            _ranges: &[KeyRange],
            _need_leader: bool,
            _need_buckets: bool,
        ) -> BatchResult<Vec<KeyLocation>> {
            Ok(self.locations.clone())
        }

        fn locate_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
            self.locations
                .iter()
                .find(|location| location.contains_start(key))
                .cloned()
                .ok_or_else(|| BatchError::OtherResponse("location not found".to_owned()))
        }

        fn locate_end_key(&self, key: &[u8]) -> BatchResult<KeyLocation> {
            self.locate_key(key)
        }

        fn locate_region_from_pd(&self, region_id: u64) -> BatchResult<KeyLocation> {
            self.locations
                .iter()
                .find(|location| location.region.id == region_id)
                .cloned()
                .ok_or_else(|| BatchError::OtherResponse("region not found".to_owned()))
        }

        fn invalidate_region(&self, _region: RegionVerId) {}
        fn update_buckets(&self, _region: RegionVerId, _old_version: u64, _new_version: u64) {}
        fn on_send_fail_tiflash(
            &self,
            _store: &RegionStore,
            _region: RegionVerId,
            _meta: &RegionMeta,
            _schedule_reload: bool,
            _error: &BatchError,
        ) {
            self.send_fail_count.fetch_add(1, Ordering::Relaxed);
        }

        fn tikv_rpc_context(
            &self,
            _region: RegionVerId,
            _replica_read: ReplicaReadType,
        ) -> BatchResult<Option<RpcContext>> {
            Ok(self.rpc_context.clone())
        }

        fn tiflash_rpc_context(
            &self,
            _region: RegionVerId,
            _is_mpp: bool,
        ) -> BatchResult<Option<RpcContext>> {
            Ok(None)
        }

        fn all_valid_tiflash_store_ids(
            &self,
            _region: RegionVerId,
            _primary_store_id: u64,
        ) -> Vec<u64> {
            Vec::new()
        }

        fn all_tiflash_stores(&self) -> Vec<RegionStore> {
            Vec::new()
        }

        fn compute_stores(&self) -> BatchResult<Vec<RegionStore>> {
            Ok(Vec::new())
        }

        fn fetch_topology(&self) -> BatchResult<Vec<String>> {
            Ok(Vec::new())
        }

        fn is_store_alive(&self, _address: &str, _ttl: Duration) -> bool {
            false
        }

        fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn distsql_admin_point_ranges_do_not_overlap_across_handle_zero_region_split() {
        let (index, _) = lookup_workers(true);
        let handles = [0, 1, 0, 2, 1, 0, 3, 4, 2, 3, 4]
            .into_iter()
            .map(Handle::Int)
            .collect::<Vec<_>>();
        let rows = handles
            .iter()
            .map(|h| match h {
                Handle::Int(v) => vec![Datum::Signed(*v)],
                _ => unreachable!(),
            })
            .collect();
        let task = index.buildTableTask(0, handles, rows);
        let record = |value| {
            astersql_kv::Key(
                astersql_tablecodec::EncodeRowKeyWithHandle(
                    100,
                    Box::new(astersql_tablecodec::kv::IntHandle(value)),
                )
                .0,
            )
        };
        let split = record(0).Next().0;
        let cache = RegionCache::new(Arc::new(SplitRegionBackend {
            locations: vec![
                KeyLocation {
                    region: RegionVerId::new(1, 1, 1),
                    start_key: vec![],
                    end_key: split.clone(),
                    ..Default::default()
                },
                KeyLocation {
                    region: RegionVerId::new(2, 1, 1),
                    start_key: split.clone(),
                    end_key: vec![],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }));
        let mut ranges = task
            .handles
            .iter()
            .map(|handle| {
                let Handle::Int(value) = handle else {
                    unreachable!()
                };
                let start = record(*value);
                KeyRange {
                    end: start.PrefixNext().0,
                    start: start.0,
                }
            })
            .collect::<Vec<_>>();
        ranges.sort_by(|a, b| a.start.cmp(&b.start));
        assert!(
            ranges.windows(2).all(|p| p[0].end <= p[1].start),
            "table requests must not overlap before region routing"
        );
        let routed = cache
            .split_region_ranges(ranges, -1)
            .expect("split inside handle 0 point range");
        assert_eq!(
            routed.len(),
            6,
            "five point ranges with handle 0 split in two"
        );
        assert!(routed.windows(2).all(|p| p[0].end <= p[1].start));
        assert_eq!(routed[0].end, split);
        assert_eq!(routed[1].start, split);
    }
}

#[test]
fn port_e48ac5fef1_index_reader_request_uses_unique_key_bound() {
    use crate::distsql::IndexReaderScan;
    use crate::table_readers_required_rows_test::{RequiredRowsBackend, required_rows_reader};
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, StatePublic, TableInfo};
    use astersql_util_ranger::{Range, collate, types};
    use protobuf::Message;
    let mut scan = tipb::Executor::new();
    scan.set_tp(tipb::ExecType::TypeIndexScan);
    let mut limit = tipb::Executor::new();
    limit.set_tp(tipb::ExecType::TypeLimit);
    let mut n = tipb::Limit::new();
    n.set_limit(100);
    limit.set_limit(n);
    let mut dag = tipb::DagRequest::new();
    dag.set_executors(vec![scan, limit].into());
    for (unique, null, points, partition_map, expected) in [
        (true, false, 35, false, 4),
        (true, false, 100, false, 4),
        (true, false, 101, false, 1),
        (false, false, 35, false, 1),
        (true, true, 1, false, 1),
        (true, false, 35, true, 1),
    ] {
        let mut reader = required_rows_reader(RequiredRowsBackend::new(
            Vec::new(),
            std::time::Duration::ZERO,
        ));
        let mut column = astersql_expression::Column::default();
        column.ID = 1;
        reader.scan = Some(IndexReaderScan {
            type_context: types::DefaultStmtNoWarningContext.clone(),
            dag: dag.write_to_bytes().unwrap(),
            table: Some(TableInfo {
                Columns: vec![ColumnInfo {
                    ID: 1,
                    ..Default::default()
                }],
                Indices: vec![IndexInfo {
                    Unique: unique,
                    State: StatePublic,
                    Columns: vec![IndexColumn {
                        Offset: 0,
                        Length: -1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }),
            columns: vec![column],
            lengths: vec![-1],
            ranges: (0..points)
                .map(|i| {
                    let value = if null {
                        types::Datum::default()
                    } else {
                        types::NewIntDatum(i)
                    };
                    Range {
                        LowVal: vec![value.clone()],
                        HighVal: vec![value],
                        Collators: collate::GetBinaryCollatorSlice(1),
                        ..Default::default()
                    }
                })
                .collect(),
            partition_range_map: partition_map,
        });
        assert_eq!(
            reader
                .buildKVReq(reader.ranges.clone())
                .unwrap()
                .concurrency,
            expected
        );
        if points == 35 {
            reader.ranges.push(reader.ranges[0].clone());
            reader.by_items.push(crate::distsql::ByItem {
                offset: 0,
                descending: false,
            });
        }
        reader.Open().unwrap();
        assert!(reader.Next(100).unwrap().is_empty());
        reader.Close().unwrap();
    }
}
