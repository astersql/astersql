// Copyright 2026 AsterSQL.
// Copyright 2016-present, PingCAP, Inc.
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

// mockstore 集群拆分（split）集成测试。
//
// 向嵌入式 UniStore 写入表记录与索引键，再按 Region（键空间分片）边界拆分，
// 验证拆分后各 Region 内扫描行数与键全集完整性。

use crate::embedded_unistore::tikv::mvcc::{Mutation, MutationOp, MvccStore, PrewriteRequest};
use crate::embedded_unistore::{Cluster, NULL_KEYSPACE_ID, New, encode_bytes};
use astersql_tablecodec as tablecodec;
use std::collections::HashMap;

/// 写入 1000 行记录与索引，拆分表/索引区间为 10 个 Region，并校验每片约 100 键。
#[test]
fn test_cluster_split() {
    let (rpc, _pd, cluster) =
        New("", Vec::new(), NULL_KEYSPACE_ID, Vec::new()).expect("create embedded unistore");
    let mvcc_store = rpc.mvcc_store();

    let tbl_id: i64 = 1;
    let idx_id: i64 = 2;
    let col_id: i64 = 3;
    let mut record_keys = Vec::with_capacity(1000);
    let mut index_keys = Vec::with_capacity(1000);

    // 预写并提交 1000 行：每行一条记录键 + 一条索引键（两阶段提交的 start_ts 递增）。
    for handle in 1..=1000_i64 {
        let row_key =
            tablecodec::EncodeRowKeyWithHandle(tbl_id, Box::new(tablecodec::kv::IntHandle(handle)));
        let col_value = tablecodec::types::NewStringDatum(handle.to_string());
        let row_value = tablecodec::EncodeRow(
            tablecodec::codec::NewEncoder(tablecodec::collate::NewCollationEnabled()),
            Some(tablecodec::time::UTC),
            vec![col_value.clone()],
            vec![col_id],
            Vec::new(),
            None,
            None,
            tablecodec::rowcodec::Encoder::new(true),
        )
        .expect("encode row");
        let encoded_index = tablecodec::codec::EncodeKey(
            tablecodec::codec::time::UTC,
            Vec::new(),
            vec![col_value, tablecodec::types::NewIntDatum(handle)],
        )
        .expect("encode index");
        let idx_key = tablecodec::EncodeIndexSeekKey(tbl_id, idx_id, Some(encoded_index));

        commit_put(&mvcc_store, &row_key.0, &row_value, handle as u64 * 2);
        commit_put(&mvcc_store, &idx_key.0, b"0", handle as u64 * 2 + 1);
        record_keys.push(row_key.0);
        index_keys.push(idx_key.0);
    }
    // SplitKeys assumes memcomparable order; string index encodings are not insertion-ordered.
    record_keys.sort();
    index_keys.sort();

    let table_start = tablecodec::GenTableRecordPrefix(tbl_id);
    let table_end = table_start.PrefixNext();
    split_range_into(
        &cluster,
        table_start.0.as_slice(),
        table_end.0.as_slice(),
        10,
        &record_keys,
    );

    let regions = cluster.region_manager().scan_regions(&[], &[], 0);
    // before-table + 10 table regions + after-table
    assert_eq!(regions.len(), 12);

    let mut all_keys = HashMap::new();
    let record_prefix = tablecodec::GenTableRecordPrefix(tbl_id);
    for region in &regions {
        let start_key = to_raw_key(&region.meta.start_key);
        let end_key = to_raw_key(&region.meta.end_key);
        if !start_key.starts_with(&record_prefix.0) {
            continue;
        }
        let end = if end_key.is_empty() {
            &[][..]
        } else {
            end_key.as_slice()
        };
        let pairs = mvcc_store.scan(&start_key, end, u64::MAX, usize::MAX, false, false, &[]);
        if !pairs.is_empty() {
            assert_eq!(pairs.len(), 100);
        }
        for pair in pairs {
            all_keys.insert(pair.key, true);
        }
    }
    assert_eq!(all_keys.len(), 1000);

    // 再拆分索引键区间，同样期望每 Region 约 100 条索引。
    let index_start = tablecodec::EncodeTableIndexPrefix(tbl_id, idx_id);
    let index_end = index_start.PrefixNext();
    split_range_into(
        &cluster,
        index_start.0.as_slice(),
        index_end.0.as_slice(),
        10,
        &index_keys,
    );

    let mut all_index = HashMap::new();
    let index_prefix = tablecodec::EncodeTableIndexPrefix(tbl_id, idx_id);
    let regions = cluster.region_manager().scan_regions(&[], &[], 0);
    for region in regions {
        let start_key = to_raw_key(&region.meta.start_key);
        let end_key = to_raw_key(&region.meta.end_key);
        if !start_key.starts_with(&index_prefix.0) {
            continue;
        }
        let end = if end_key.is_empty() {
            &[][..]
        } else {
            end_key.as_slice()
        };
        let pairs = mvcc_store.scan(&start_key, end, u64::MAX, usize::MAX, false, false, &[]);
        if !pairs.is_empty() {
            assert_eq!(pairs.len(), 100);
        }
        for pair in pairs {
            all_index.insert(pair.key, true);
        }
    }
    assert_eq!(all_index.len(), 1000);

    let _ = rpc.close();
    cluster.close();
}

/// 对单键执行 prewrite + commit（两阶段提交的简化路径）。
fn commit_put(store: &MvccStore, key: &[u8], value: &[u8], start_ts: u64) {
    let req = PrewriteRequest {
        mutations: vec![Mutation {
            op: MutationOp::Put,
            key: key.to_vec(),
            value: value.to_vec(),
            is_pessimistic_lock: false,
        }],
        primary_lock: key.to_vec(),
        start_ts,
        lock_ttl: 3000,
        min_commit_ts: start_ts + 1,
        ..PrewriteRequest::default()
    };
    store.prewrite(&req).expect("prewrite");
    store
        .commit(&[key.to_vec()], start_ts, start_ts + 1)
        .expect("commit");
}

/// 先保证 `[start, end)` 边界各有一次 split，再按 `count` 与数据键均匀拆分内部 Region。
fn split_range_into(
    cluster: &Cluster,
    start: &[u8],
    end: &[u8],
    count: usize,
    data_keys: &[Vec<u8>],
) {
    let manager = cluster.region_manager();
    // Ensure boundaries around [start, end) so total regions become 12 like Go.
    // 若当前 Region 起点不是目标 start，先在 start 处切开。
    let region = manager
        .get_region_by_key(&encode_bytes(start))
        .or_else(|| manager.scan_regions(&[], &[], 1).into_iter().next())
        .expect("bootstrapped region");
    if region.meta.start_key != encode_bytes(start) {
        let new_region = manager.alloc_id();
        let peer = manager.alloc_id();
        cluster
            .split(region.meta.id, new_region, start, &[peer], peer)
            .expect("split at range start");
    }
    let region = manager
        .get_region_by_key(&encode_bytes(start))
        .expect("table region");
    let encoded_end = encode_bytes(end);
    // 若终点边界未对齐，在 end 处再切一刀。
    if region.meta.end_key != encoded_end {
        let new_region = manager.alloc_id();
        let peer = manager.alloc_id();
        cluster
            .split(region.meta.id, new_region, end, &[peer], peer)
            .expect("split at range end");
    }
    cluster
        .split_keys(start, end, count, data_keys)
        .expect("split keys");
}

/// 将 Region meta 中的编码键解码为原始键字节。
fn to_raw_key(k: &[u8]) -> Vec<u8> {
    if k.is_empty() {
        return Vec::new();
    }
    let (_, raw) = tablecodec::codec::DecodeBytes(k, None).unwrap_or_else(|err| panic!("{err:?}"));
    raw
}
