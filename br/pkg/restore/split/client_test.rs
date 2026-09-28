// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Equivalents of `br/pkg/restore/split/client_test.go`.
//! PD/TiKV boundaries are mocked via `MockPDClientForSplit` (no kvproto/grpcio).
//! split client 测试：codec PD、批量分裂、scatter、空结果与重试。
//! check_regions_boundaries 校验分裂后边界连续性。
//! 错误注入用例锁定 retryable 判定，避免无限重试或过早失败。
//! 使用 mock PD，不依赖真实集群。
//! 对齐 Go `client_test.go` 主场景。
//! check_regions_boundaries 验证分裂后区域边界闭合连续。
//! encode_row_key 与实现共用编码，避免测试私有编码器分叉。
//! pd_client_raw 区分 RawKV 客户端行为。
//! test_get_codec_pd_client 覆盖编解码包装是否正确委托。
//! test_batch_split/test_split_scatter 是主成功路径。
//! test_split_scatter_raw_kv 与 txn 路径对照键编码差异。
//! test_split_scatter_empty_end_key 锁定开放上界语义。
//! test_scan_region_empty_result 覆盖空扫描的错误/重试策略。
//! test_split_meet_error_and_retry 与 PD 可重试错误分类。
//! test_pd_error_can_retry 单元级验证错误判定函数。
//! mock 注入的错误次数用尽后应上浮原始错误。
//! 不依赖真实 PD 进程，计时断言保持宽松。
//! 补充要点1：check_regions_boundaries 验证分裂后区域边界闭合连续。
//! 补充要点2：encode_row_key 与实现共用编码，避免测试私有编码器分叉。
//! 补充要点3：pd_client_raw 区分 RawKV 客户端行为。
//! 补充要点4：test_get_codec_pd_client 覆盖编解码包装是否正确委托。
//! 补充要点5：test_batch_split/test_split_scatter 是主成功路径。
//! 补充要点6：test_split_scatter_raw_kv 与 txn 路径对照键编码差异。
//! 补充要点7：test_split_scatter_empty_end_key 锁定开放上界语义。
//! 补充要点8：test_scan_region_empty_result 覆盖空扫描的错误/重试策略。
//! 补充要点9：test_split_meet_error_and_retry 与 PD 可重试错误分类。
//! 补充要点10：test_pd_error_can_retry 单元级验证错误判定函数。
//! 补充要点11：mock 注入的错误次数用尽后应上浮原始错误。
//! 补充要点12：不依赖真实 PD 进程，计时断言保持宽松。
//! 补充要点13：check_regions_boundaries 验证分裂后区域边界闭合连续。
//! 补充要点14：encode_row_key 与实现共用编码，避免测试私有编码器分叉。
//! 补充要点15：pd_client_raw 区分 RawKV 客户端行为。
//! 补充要点16：test_get_codec_pd_client 覆盖编解码包装是否正确委托。
//! 补充要点17：test_batch_split/test_split_scatter 是主成功路径。
//! 补充要点18：test_split_scatter_raw_kv 与 txn 路径对照键编码差异。
//! 补充要点19：test_split_scatter_empty_end_key 锁定开放上界语义。
//! 补充要点20：test_scan_region_empty_result 覆盖空扫描的错误/重试策略。
//! 补充要点21：test_split_meet_error_and_retry 与 PD 可重试错误分类。
//! 补充要点22：test_pd_error_can_retry 单元级验证错误判定函数。
//! 补充要点23：mock 注入的错误次数用尽后应上浮原始错误。
//! 补充要点24：不依赖真实 PD 进程，计时断言保持宽松。
//! 补充要点25：check_regions_boundaries 验证分裂后区域边界闭合连续。
//! 补充要点26：encode_row_key 与实现共用编码，避免测试私有编码器分叉。
//! 补充要点27：pd_client_raw 区分 RawKV 客户端行为。
//! 补充要点28：test_get_codec_pd_client 覆盖编解码包装是否正确委托。
//! 补充要点29：test_batch_split/test_split_scatter 是主成功路径。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use astersql_errors::New;

use crate::client::{
    NewClient, NewCodecAwareClient, PdClient, PdErrorCanRetry, PdHttpBackend, SplitClient,
    isUnsupportedError, maxBatchSplitSize,
};
use crate::mock_pd_client::{
    MockPDClientForSplit, NewFakePDHTTPClient, NewFakeSplitClient, NewMockPDClientForSplit,
    NewTestClient,
};
use crate::region::RegionInfo;
use crate::split::{PaginateScanRegion, SplitRetryTimes, WaitRegionOnlineAttemptTimes};
use crate::stubs::codec;
use crate::stubs::tablecodec;
use crate::stubs::{CodecPDClient, Context, metapb, pdhttp};
use std::collections::HashMap;

/// `check_regions_boundaries`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn check_regions_boundaries(regions: &[RegionInfo], expected: &[Vec<u8>]) {
    assert_eq!(
        regions.len(),
        expected.len() - 1,
        "region count mismatch: first_start={:?} last_end={:?} expected_first={:?} expected_last={:?}",
        regions
            .first()
            .and_then(|r| r.Region.as_ref())
            .map(|r| &r.StartKey),
        regions
            .last()
            .and_then(|r| r.Region.as_ref())
            .map(|r| &r.EndKey),
        expected.first(),
        expected.last()
    );
    for i in 1..expected.len() {
        let meta = regions[i - 1].Region.as_ref().unwrap();
        assert_eq!(&meta.StartKey, &expected[i - 1], "start at {i}");
        assert_eq!(&meta.EndKey, &expected[i], "end at {i}");
    }
}

/// `encode_row_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn encode_row_key(table_id: i64, datums: &[i64]) -> Vec<u8> {
    let handle = tablecodec::EncodeCommonHandle(datums);
    tablecodec::EncodeRowKeyWithHandle(table_id, &handle)
}

/// `pd_client_raw`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn pd_client_raw(mock: MockPDClientForSplit, concurrency: i32, batch: i32) -> PdClient {
    let mut c = NewClient(Box::new(mock), None, batch, concurrency, vec![]);
    c.isRawKv = true;
    c.ForceNeedScatter(true);
    c
}

#[test]
/// 测试 `test_get_codec_pd_client`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_get_codec_pd_client() {
    let plain = NewClient(Box::new(NewMockPDClientForSplit()), None, 0, 0, vec![]);
    assert!(plain.GetCodecPDClient().is_none());

    let with_mock = NewClient(Box::new(NewMockPDClientForSplit()), None, 0, 0, vec![]);
    assert!(with_mock.GetCodecPDClient().is_none());

    let mut with_codec_flag = NewClient(Box::new(NewMockPDClientForSplit()), None, 0, 0, vec![]);
    with_codec_flag.codecClient = Some(CodecPDClient);
    assert!(with_codec_flag.GetCodecPDClient().is_none());

    let aware = NewCodecAwareClient(Box::new(NewMockPDClientForSplit()), None, 0, 0, vec![]);
    assert!(aware.GetCodecPDClient().is_some());

    assert!(NewFakeSplitClient().GetCodecPDClient().is_none());
    assert!(
        NewTestClient(HashMap::new(), HashMap::new(), 0)
            .GetCodecPDClient()
            .is_none()
    );
}

#[derive(Clone, Default)]
struct RecordingHttpClient {
    calls: Arc<Mutex<Vec<(u64, String, String)>>>,
}

impl PdHttpBackend for RecordingHttpClient {
    fn GetReplicateConfig(&self) -> crate::stubs::Result<HashMap<String, f64>> {
        Ok(HashMap::new())
    }

    fn GetPlacementRule(
        &self,
        _group_id: &str,
        _rule_id: &str,
    ) -> crate::stubs::Result<pdhttp::Rule> {
        Ok(pdhttp::Rule::default())
    }

    fn SetPlacementRule(&self, _rule: &pdhttp::Rule) -> crate::stubs::Result<()> {
        Ok(())
    }

    fn DeletePlacementRule(&self, _group_id: &str, _rule_id: &str) -> crate::stubs::Result<()> {
        Ok(())
    }

    fn SetStoreLabels(
        &self,
        store_id: u64,
        label_key: &str,
        label_value: &str,
    ) -> crate::stubs::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((store_id, label_key.to_owned(), label_value.to_owned()));
        Ok(())
    }
}

#[test]
fn test_set_stores_label_calls_pd_http_for_each_store() {
    let http = RecordingHttpClient::default();
    let calls = http.calls.clone();
    let client = NewClient(
        Box::new(NewMockPDClientForSplit()),
        Some(Box::new(http)),
        100,
        20,
        vec![],
    );

    client
        .SetStoresLabel(&Context::Background(), &[7, 11], "zone", "east")
        .expect("set labels");

    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (7, "zone".to_owned(), "east".to_owned()),
            (11, "zone".to_owned(), "east".to_owned()),
        ]
    );
}

#[test]
/// 测试 `test_batch_split`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_batch_split() {
    let backup = maxBatchSplitSize.load(Ordering::SeqCst);
    maxBatchSplitSize.store(7, Ordering::SeqCst);

    let mock = NewMockPDClientForSplit();
    let keys = vec![vec![], vec![]];
    let set_regions = mock.SetRegions(&keys);
    assert_eq!(set_regions.len(), 1);
    let split_region = RegionInfo {
        Region: Some(set_regions[0].clone()),
        Leader: Some(metapb::Peer {
            Id: set_regions[0].Id,
            StoreId: 1,
        }),
        ..Default::default()
    };
    let mock_client = pd_client_raw(mock.clone(), 0, 100);
    let ctx = Context::Background();

    let split_keys = vec![
        b"ba".to_vec(),
        b"bb".to_vec(),
        b"bc".to_vec(),
        b"bd".to_vec(),
        b"be".to_vec(),
        b"bf".to_vec(),
        b"bg".to_vec(),
        b"bh".to_vec(),
    ];
    let expected_batch_split_cnt = 3;

    mock_client
        .SplitWaitAndScatter(&ctx, &split_region, &split_keys)
        .expect("split");

    let regions = PaginateScanRegion(&ctx, &mock_client, b"b", b"c", 5).expect("scan");
    let mut expected = vec![vec![]];
    expected.extend(split_keys.iter().cloned());
    expected.push(vec![]);
    check_regions_boundaries(&regions, &expected);

    assert_eq!(mock.split_count(), expected_batch_split_cnt);
    assert_eq!(
        mock.scatter_regions_region_count() as usize,
        split_keys.len()
    );

    maxBatchSplitSize.store(backup, Ordering::SeqCst);
}

#[test]
fn test_need_scatter_probe_error_uses_permissive_strategy() {
    let mock = NewMockPDClientForSplit();
    let client = NewClient(Box::new(mock.clone()), None, 100, 20, vec![]);
    let region = RegionInfo {
        Region: Some(metapb::Region {
            Id: 42,
            ..Default::default()
        }),
        ..Default::default()
    };

    client
        .scatterRegions(&Context::Background(), &[region])
        .expect("scatter should remain enabled when the probe fails");

    assert_eq!(mock.scatter_regions_region_count(), 1);
}

#[test]
fn test_need_scatter_when_store_count_equals_replica_count() {
    let mock = NewMockPDClientForSplit();
    mock.SetStores(HashMap::from([
        (
            1,
            metapb::Store {
                Id: 1,
                ..Default::default()
            },
        ),
        (
            2,
            metapb::Store {
                Id: 2,
                ..Default::default()
            },
        ),
        (
            3,
            metapb::Store {
                Id: 3,
                ..Default::default()
            },
        ),
    ]));
    let client = NewClient(
        Box::new(mock.clone()),
        Some(Box::new(NewFakePDHTTPClient())),
        100,
        20,
        vec![],
    );
    let region = RegionInfo {
        Region: Some(metapb::Region {
            Id: 43,
            ..Default::default()
        }),
        ..Default::default()
    };

    client
        .scatterRegions(&Context::Background(), &[region])
        .expect("equal store and replica counts should still scatter");

    assert_eq!(mock.scatter_regions_region_count(), 1);
}

#[test]
/// 测试 `test_split_scatter`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_split_scatter() {
    let backup = maxBatchSplitSize.load(Ordering::SeqCst);
    maxBatchSplitSize.store(100, Ordering::SeqCst);

    let table_id: i64 = 1;
    let table_start_key = codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(table_id));
    let table_end_key =
        codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(table_id + 1));
    let mut keys = vec![vec![], table_start_key.clone()];
    for i in 0..2 {
        let key = encode_row_key(table_id, &[i]);
        keys.push(codec::EncodeBytes(Vec::new(), &key));
    }
    keys.push(table_end_key.clone());
    keys.push(vec![]);

    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&keys);
    let mut mock_client = NewClient(Box::new(mock.clone()), None, 100, 20, vec![]);
    mock_client.ForceNeedScatter(true);
    let ctx = Context::Background();

    let mut split_keys = Vec::with_capacity(20);
    for i in 0..2 {
        for j in 0..10 {
            split_keys.push(encode_row_key(table_id, &[i, j * 10000]));
        }
    }

    let no_scatter = NewMockPDClientForSplit();
    no_scatter.SetRegions(&keys);
    let mut no_scatter_client = NewClient(Box::new(no_scatter.clone()), None, 100, 20, vec![]);
    // leave needScatter false
    let no_scatter_keys: Vec<Vec<u8>> = split_keys.iter().map(|k| k.clone()).collect();
    no_scatter_client
        .SplitKeys(&ctx, &no_scatter_keys)
        .expect("split keys");
    assert!(no_scatter.split_count() > 0);
    assert_eq!(no_scatter.scatter_regions_region_count(), 0);

    mock_client
        .SplitKeysAndScatter(&ctx, &split_keys)
        .expect("split scatter");

    let regions =
        PaginateScanRegion(&ctx, &mock_client, &table_start_key, &table_end_key, 5).expect("scan");
    let mut expected = Vec::with_capacity(24);
    expected.push(table_start_key);
    expected.push(keys[2].clone());
    for k in &split_keys[..10] {
        expected.push(codec::EncodeBytes(Vec::new(), k));
    }
    expected.push(keys[3].clone());
    for k in &split_keys[10..] {
        expected.push(codec::EncodeBytes(Vec::new(), k));
    }
    expected.push(table_end_key);
    check_regions_boundaries(&regions, &expected);

    maxBatchSplitSize.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_split_scatter_raw_kv`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_split_scatter_raw_kv() {
    let backup = maxBatchSplitSize.load(Ordering::SeqCst);
    maxBatchSplitSize.store(7, Ordering::SeqCst);

    let mock = NewMockPDClientForSplit();
    let keys = vec![
        vec![],
        b"aay".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"cca".to_vec(),
        vec![],
    ];
    mock.SetRegions(&keys);
    let mock_client = pd_client_raw(mock.clone(), 10, 100);
    let ctx = Context::Background();

    let mut split_keys = vec![b"b".to_vec()];
    for i in b'a'..=b'z' {
        split_keys.push(vec![b'b', i]);
    }

    mock_client
        .SplitKeysAndScatter(&ctx, &split_keys)
        .expect("split scatter");

    let regions = PaginateScanRegion(&ctx, &mock_client, b"b", b"c", 5).expect("scan");
    let result = vec![
        b"b".to_vec(),
        b"ba".to_vec(),
        b"bb".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"bc".to_vec(),
        b"bd".to_vec(),
        b"be".to_vec(),
        b"bf".to_vec(),
        b"bg".to_vec(),
        b"bh".to_vec(),
        b"bi".to_vec(),
        b"bj".to_vec(),
        b"bk".to_vec(),
        b"bl".to_vec(),
        b"bm".to_vec(),
        b"bn".to_vec(),
        b"bo".to_vec(),
        b"bp".to_vec(),
        b"bq".to_vec(),
        b"br".to_vec(),
        b"bs".to_vec(),
        b"bt".to_vec(),
        b"bu".to_vec(),
        b"bv".to_vec(),
        b"bw".to_vec(),
        b"bx".to_vec(),
        b"by".to_vec(),
        b"bz".to_vec(),
        b"cca".to_vec(),
    ];
    check_regions_boundaries(&regions, &result);

    assert_eq!(mock.split_count(), 9);
    assert_eq!(
        mock.scatter_regions_region_count() as usize,
        result.len() - 3
    );

    maxBatchSplitSize.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_split_scatter_empty_end_key`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_split_scatter_empty_end_key() {
    let mock = NewMockPDClientForSplit();
    let keys = vec![
        vec![],
        b"aay".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"cca".to_vec(),
        vec![],
    ];
    mock.SetRegions(&keys);
    let mock_client = pd_client_raw(mock.clone(), 10, 100);
    let ctx = Context::Background();

    let split_keys = vec![b"b".to_vec(), b"c".to_vec(), vec![]];
    mock_client
        .SplitKeysAndScatter(&ctx, &split_keys)
        .expect("split");

    let regions = PaginateScanRegion(&ctx, &mock_client, &[], &[], 5).expect("scan");
    let result = vec![
        vec![],
        b"aay".to_vec(),
        b"b".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"c".to_vec(),
        b"cca".to_vec(),
        vec![],
    ];
    check_regions_boundaries(&regions, &result);

    mock_client
        .SplitKeysAndScatter(&ctx, &[vec![]])
        .expect("empty key");
    let regions = PaginateScanRegion(&ctx, &mock_client, &[], &[], 5).expect("scan2");
    check_regions_boundaries(&regions, &result);
}

#[test]
/// 测试 `test_scan_region_empty_result`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_scan_region_empty_result() {
    let backup = WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst);
    let backup2 = SplitRetryTimes.load(Ordering::SeqCst);
    WaitRegionOnlineAttemptTimes.store(2, Ordering::SeqCst);
    SplitRetryTimes.store(2, Ordering::SeqCst);

    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&[vec![], vec![]]);
    for _ in 0..8 {
        mock.push_scan_error(None);
    }
    let mock_client = pd_client_raw(mock, 4, 100);
    let ctx = Context::Background();

    let err = mock_client
        .SplitKeysAndScatter(&ctx, &[b"ba".to_vec(), b"bb".to_vec()])
        .unwrap_err();
    assert!(
        err.to_string().contains("scan region return empty result"),
        "{err}"
    );

    WaitRegionOnlineAttemptTimes.store(backup, Ordering::SeqCst);
    SplitRetryTimes.store(backup2, Ordering::SeqCst);
}

#[test]
/// 测试 `test_split_meet_error_and_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_split_meet_error_and_retry() {
    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&[vec![], b"a".to_vec(), vec![]]);
    let mock_client = pd_client_raw(mock.clone(), 1, 100);
    let ctx = Context::Background();

    mock.set_split_hijack(Some(Box::new({
        let mock = mock.clone();
        move || {
            mock.set_split_hijack(None);
            Err(New("epoch not match"))
        }
    })));

    mock_client
        .SplitKeysAndScatter(&ctx, &[b"b".to_vec()])
        .expect("retry ok");
    let regions = PaginateScanRegion(&ctx, &mock_client, b"a", &[], 5).expect("scan");
    check_regions_boundaries(&regions, &[b"a".to_vec(), b"b".to_vec(), vec![]]);
    assert_eq!(mock.split_count(), 2);

    mock.set_split_hijack(Some(Box::new({
        let mock = mock.clone();
        move || {
            mock.set_split_hijack(None);
            Err(New("no valid key"))
        }
    })));
    mock.reset_split_count();

    mock_client
        .SplitKeysAndScatter(&ctx, &[b"c".to_vec()])
        .expect("retry ok 2");
    let regions = PaginateScanRegion(&ctx, &mock_client, b"b", &[], 5).expect("scan2");
    check_regions_boundaries(&regions, &[b"b".to_vec(), b"c".to_vec(), vec![]]);
    assert_eq!(mock.split_count(), 2);

    let backup = SplitRetryTimes.load(Ordering::SeqCst);
    SplitRetryTimes.store(2, Ordering::SeqCst);
    mock.set_split_hijack_persistent(Box::new(|| Err(New("no valid key"))));
    let err = mock_client
        .SplitKeysAndScatter(&ctx, &[b"d".to_vec()])
        .unwrap_err();
    assert!(err.to_string().contains("no valid key"), "{err}");
    mock.set_split_hijack(None);
    SplitRetryTimes.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_pd_error_can_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_pd_error_can_retry() {
    assert!(!PdErrorCanRetry(&New("random failure")));
    assert!(PdErrorCanRetry(&New("region 42 is not fully replicated")));
    assert!(PdErrorCanRetry(&New(
        "operator canceled because cannot add an operator to the execute queue"
    )));
    assert!(PdErrorCanRetry(&New(
        "unable to create operator, failed to create scatter region operator for region 13813282"
    )));
    assert!(!PdErrorCanRetry(&New("should be false")));
}

#[test]
fn test_unsupported_batch_scatter_error_matches_go_fallbacks() {
    assert!(isUnsupportedError(&New("rpc error: code = Unimplemented")));
    assert!(isUnsupportedError(&New("rpc error: region 0 not found")));
    assert!(!isUnsupportedError(&New("feature not supported locally")));
}
