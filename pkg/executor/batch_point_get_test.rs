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
// 批量点查模块中悲观锁缓存 Getter 的单元测试。
//
// 验证 `PessimisticLockCacheGetter::Get`：正常返回缓存值，
// 并在请求 `return_commit_ts` 时拒绝（该选项不被锁缓存路径支持）。

use crate::batch_point_get::{
    BatchPointGetExec, BatchPointGetIndexInfo, BatchPointGetRuntime, BatchPointGetTableInfo,
    PessimisticLockCacheGetter, PointGetKey, PointGetOptions, PointGetValue,
};
use astersql_util_chunk::Chunk;
use std::cmp::Ordering;
use std::collections::BTreeMap;

#[derive(Default)]
struct LocalIndexRuntime {
    batch_get_calls: usize,
}

impl BatchPointGetRuntime for LocalIndexRuntime {
    type Context = ();
    type Handle = i64;
    type IndexValue = i64;
    type FieldType = ();
    type ColumnInfo = ();
    type RowDecoder = ();
    type Error = String;

    fn build_virtual_column_info(&self, _: &[()]) -> (Vec<usize>, Vec<()>) {
        (Vec::new(), Vec::new())
    }
    fn open_batch_getter(&mut self, _: bool, _: bool, _: i64) -> Result<(), String> {
        Ok(())
    }
    fn close_runtime_stats(
        &mut self,
        _: &BatchPointGetTableInfo,
        _: Option<&BatchPointGetIndexInfo>,
    ) {
    }
    fn reset_snapshot_runtime_stats(&mut self) {}
    fn maximum_execution_time_ms(&self) -> u64 {
        0
    }
    fn pessimistic_read_consistency(&self) -> bool {
        false
    }
    fn encode_unique_index_key(
        &self,
        _: &BatchPointGetTableInfo,
        _: &BatchPointGetIndexInfo,
        _: &[i64],
        _: i64,
    ) -> Result<Option<PointGetKey>, String> {
        Ok(Some(b"index".to_vec()))
    }
    fn batch_get(
        &mut self,
        _: &mut (),
        _: &[PointGetKey],
        _: u64,
    ) -> Result<BTreeMap<PointGetKey, PointGetValue>, String> {
        self.batch_get_calls += 1;
        Ok(if self.batch_get_calls == 1 {
            BTreeMap::from([(b"index".to_vec(), b"handle".to_vec())])
        } else {
            BTreeMap::from([(b"row".to_vec(), b"value".to_vec())])
        })
    }
    fn decode_index_handle(&self, _: &[u8]) -> Result<i64, String> {
        Ok(7)
    }
    fn global_index_partition_id(&self, _: &[u8]) -> Result<i64, String> {
        unreachable!("local indexes decode their partition from the key")
    }
    fn table_id_from_index_key(&self, _: &[u8]) -> i64 {
        42
    }
    fn partition_matches(&self, _: i64, _: &[String]) -> bool {
        false
    }
    fn compare_handles(&self, left: &i64, right: &i64, _: bool) -> Ordering {
        left.cmp(right)
    }
    fn encode_row_key(&self, _: i64, _: &i64) -> PointGetKey {
        b"row".to_vec()
    }
    fn lock_keys(&mut self, _: &mut (), _: i64, _: &[PointGetKey]) -> Result<(), String> {
        Ok(())
    }
    fn weak_consistency(&self) -> bool {
        false
    }
    fn report_lookup_inconsistent(
        &mut self,
        _: &mut (),
        _: &BatchPointGetTableInfo,
        _: &BatchPointGetIndexInfo,
        _: &[u8],
        _: &[u8],
        _: &i64,
    ) -> Result<(), String> {
        Ok(())
    }
    fn update_delta_for_table_id(&mut self, _: i64) {}
    fn reset_output_chunk(&self, _: &mut Chunk) {}
    fn output_chunk_is_full(&self, _: &Chunk) -> bool {
        false
    }
    fn decode_row(&mut self, _: &i64, _: &[u8], _: &mut Chunk, _: &mut ()) -> Result<(), String> {
        Ok(())
    }
    fn fill_row_checksum(
        &mut self,
        _: usize,
        _: usize,
        _: &[PointGetValue],
        _: &[i64],
        _: &mut Chunk,
    ) -> Result<(), String> {
        Ok(())
    }
    fn fill_virtual_columns(
        &mut self,
        _: &[()],
        _: &[usize],
        _: &[()],
        _: &mut Chunk,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[test]
/// 命中缓存返回值；开启 return_commit_ts 时返回错误。
fn batch_point_get_lock_cache_returns_values_and_rejects_commit_ts_option() {
    let getter = PessimisticLockCacheGetter {
        values: BTreeMap::from([(b"k1".to_vec(), b"v1".to_vec())]),
    };
    assert_eq!(
        getter
            .Get(
                b"k1",
                PointGetOptions {
                    return_commit_ts: false
                }
            )
            .unwrap(),
        Some(b"v1".to_vec())
    );
    assert!(
        getter
            .Get(
                b"k1",
                PointGetOptions {
                    return_commit_ts: true
                }
            )
            .is_err()
    );
}

#[test]
fn batch_point_get_local_index_does_not_apply_global_partition_name_filter() {
    let mut executor = BatchPointGetExec {
        runtime: LocalIndexRuntime::default(),
        table_info: BatchPointGetTableInfo {
            id: 1,
            partitioned: true,
            ..Default::default()
        },
        index_info: Some(BatchPointGetIndexInfo {
            id: 2,
            global: false,
            primary: false,
        }),
        handles: Vec::new(),
        plan_physical_ids: vec![42],
        single_partition_id: 0,
        partition_names: vec!["other_partition".to_owned()],
        index_values: vec![vec![1]],
        lock: false,
        wait_time_ms: 0,
        initialized: false,
        values: Vec::new(),
        cursor: 0,
        row_decoder: (),
        keep_order: false,
        descending: false,
        columns: Vec::new(),
        virtual_column_indices: Vec::new(),
        virtual_column_field_types: Vec::new(),
    };

    executor.initialize(&mut ()).unwrap();

    assert_eq!(executor.handles, vec![7]);
    assert_eq!(executor.values, vec![b"value".to_vec()]);
}
