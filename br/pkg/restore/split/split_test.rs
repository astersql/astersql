// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! 对应 Go `br/pkg/restore/split/split_test.go` 的等价测试。
//! PD/TiKV/failpoint 边界本地 mock，无 kvproto/grpcio。
//! 覆盖分页扫描、一致性检查、拆分重试、Scatter 等待与 SplitPoint 等路径。
//! 断言依据 Go 表驱动期望；并行安全依赖线程局部重试计数。
//! Equivalents of `br/pkg/restore/split/split_test.go`.
//! PD/TiKV/failpoint boundaries mocked locally (no kvproto/grpcio).

//! split 测试：对齐 Go `split_test.go`，用本地 mock PD/TiKV 覆盖扫描、分裂与 scatter。
//! 辅助函数构造编码键、region 边界与 rewrite 前缀，避免真实 tablecodec 依赖。
//! backoff/cancel 用例锁定可重试与不可重试错误的计数差异。
//! PaginateScanRegion 与 limit-with-retry 验证分页连续性与空结果处理。
//! RegionSplitter/SplitPoint 场景覆盖 rough split、空 region 与 RawKV 路径。
//! 一致性检查覆盖 epoch 不一致与部分 region 覆盖失败路径。
//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_br_pkg_errors::ErrPDBatchScanRegion;
use astersql_br_pkg_restore_utils::RewriteRules;
use astersql_br_pkg_restore_utils::stubs::import_sstpb;
use astersql_errors::{Annotatef, New, SharedError};

use crate::client::{NewClient, NewCodecAwareClient, PdBackend, SplitClient};
use crate::mock_pd_client::{NewFakeSplitClient, NewMockPDClientForSplit, RegionTree};
use crate::region::RegionInfo;
use crate::split::{
    CheckRegionEpoch, ErrBackoff, ErrBackoffAndDontCount, NewBackoffMayNotCountBackoffer,
    NewRegionSplitter, NewRegionSplitterWithRegionIndexStep, NewWaitRegionOnlineBackoffer,
    PaginateScanRegion, PaginateScanRegionWithCodecAware, ScanRegionsWithRetry,
    WaitRegionOnlineAttemptTimes, checkPartRegionConsistency, checkRegionConsistency,
    getSplitKeysOfRegions, scanRegionsLimitWithRetry,
};
use crate::splitter::{NewRewriteSpliter, NewSplitHelperIterator, SplitPoint};
use crate::stubs::codec;
use crate::stubs::tablecodec;
use crate::stubs::{
    BackoffStrategy, Context, EnableHintScanRegionBackoff, GetRegionOption, GetStoreOption,
    WithRetry, metapb, pdhttp, pdpb,
};
use crate::sum_sorted::{NewSplitHelper, Span, Value, Valued};

/// `check_regions_boundaries`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `check_regions_boundaries` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn check_regions_boundaries(regions: &[RegionInfo], expected: &[Vec<u8>]) {
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions.len(), expected.len() - 1);
    for i in 1..expected.len() {
        let meta = regions[i - 1].Region.as_ref().unwrap();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(&meta.StartKey, &expected[i - 1]);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(&meta.EndKey, &expected[i]);
    }
}

/// `encode_bytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `encode_bytes` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn encode_bytes(keys: &mut [Vec<u8>]) {
    for k in keys.iter_mut() {
        if k.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            continue;
            // 继续下一轮重试或迭代。
        }
        *k = codec::EncodeBytes(Vec::new(), k);
    }
}

/// `rewrite_prefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `rewrite_prefix` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn rewrite_prefix(key: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    if key.starts_with(old) {
        // 条件分支：见块内处理与 Go 对齐点。
        let mut out = new.to_vec();
        out.extend_from_slice(&key[old.len()..]);
        out
    } else {
        key.to_vec()
    }
}

/// `key_with_table_prefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `key_with_table_prefix` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn key_with_table_prefix(table_id: i64, key: &str) -> Vec<u8> {
    let mut raw = tablecodec::GenTableRecordPrefix(table_id);
    raw.extend_from_slice(key.as_bytes());
    codec::EncodeBytes(Vec::new(), &raw)
}

/// `get_char_from_number`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `get_char_from_number` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn get_char_from_number(prefix: &str, i: i32) -> String {
    let c = b'1' + (i % 10) as u8;
    let b = b'1' + ((i % 100) / 10) as u8;
    let a = b'1' + (i / 100) as u8;
    format!("{prefix}{}{}{}", a as char, b as char, c as char)
}

/// `region_info`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `region_info` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn region_info(start: &str, end: &str) -> RegionInfo {
    RegionInfo {
        Region: Some(metapb::Region {
            StartKey: start.as_bytes().to_vec(),
            EndKey: end.as_bytes().to_vec(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `init_keys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `init_keys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn init_keys() -> Vec<Vec<u8>> {
    vec![
        b"aae".to_vec(),
        b"aaz".to_vec(),
        b"ccf".to_vec(),
        b"ccj".to_vec(),
    ]
}

/// `RecordCntBackoffer`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RecordCntBackoffer` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct RecordCntBackoffer {
    already: i32,
}

/// `RecordCntBackoffer` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `RecordCntBackoffer` 方法边界：非法参数应返回可分类错误而非 panic。
impl BackoffStrategy for RecordCntBackoffer {
    /// `NextBackoff`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `NextBackoff` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn NextBackoff(&mut self, _err: &SharedError) -> Duration {
        self.already += 1;
        Duration::ZERO
    }
    /// `RemainingAttempts`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `RemainingAttempts` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn RemainingAttempts(&self) -> i32 {
        100
    }
}

#[test]
/// 测试 `test_scan_region_back_offer_with_success`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_region_back_offer_with_success` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_region_back_offer_with_success() {
    let mut counter = 0;
    let mut bo = NewWaitRegionOnlineBackoffer();
    let ctx = Context::Background();
    WithRetry(
        &ctx,
        || {
            let done = counter == 3;
            counter += 1;
            if done {
                // 条件分支：见块内处理与 Go 对齐点。
                Ok(())
            } else {
                Err(SharedError::new((*ErrPDBatchScanRegion).clone()))
                // 错误路径：向上返回，保留上下文。
            }
        },
        &mut bo,
    )
    .unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(counter, 4);
}

#[test]
/// 测试 `test_scan_region_back_offer_with_fail`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_region_back_offer_with_fail` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_region_back_offer_with_fail() {
    EnableHintScanRegionBackoff(true);
    let mut counter = 0;
    let mut bo = NewWaitRegionOnlineBackoffer();
    let ctx = Context::Background();
    let err = WithRetry(
        &ctx,
        || {
            counter += 1;
            Err(SharedError::new((*ErrPDBatchScanRegion).clone()))
            // 错误路径：向上返回，保留上下文。
        },
        &mut bo,
    )
    .unwrap_err();
    let _ = err;
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(counter, WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst));
    EnableHintScanRegionBackoff(false);
}

#[test]
/// 测试 `test_scan_region_back_offer_with_stop_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_region_back_offer_with_stop_retry` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_region_back_offer_with_stop_retry() {
    EnableHintScanRegionBackoff(true);
    let mut counter = 0;
    let mut bo = NewWaitRegionOnlineBackoffer();
    let ctx = Context::Background();
    let _ = WithRetry(
        &ctx,
        || {
            let c = counter;
            counter += 1;
            if c < 5 {
                // 条件分支：见块内处理与 Go 对齐点。
                Err(SharedError::new((*ErrPDBatchScanRegion).clone()))
                // 错误路径：向上返回，保留上下文。
            } else {
                Err(New("unknown"))
                // 错误路径：向上返回，保留上下文。
            }
        },
        &mut bo,
    );
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(counter, 6);
    EnableHintScanRegionBackoff(false);
}

#[test]
/// 测试 `test_scatter_sequentially_retry_cnt`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scatter_sequentially_retry_cnt` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scatter_sequentially_retry_cnt() {
    let mock = NewMockPDClientForSplit();
    mock.set_scatter_each_fail_before(7);
    let mut client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    client.ForceNeedScatter(true);
    let ctx = Context::Background();
    let regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    let mut backoffer = RecordCntBackoffer { already: 0 };
    client.scatterRegionsSequentially(&ctx, &regions, &mut backoffer);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(backoffer.already, 7);
}

#[test]
/// 测试 `test_batch_scatter_regions_retry_cnt`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_batch_scatter_regions_retry_cnt` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_batch_scatter_regions_retry_cnt() {
    let mock = NewMockPDClientForSplit();
    mock.set_scatter_regions_failed_count(7);
    let mut client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    client.ForceNeedScatter(true);
    let ctx = Context::Background();
    let regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    client.scatterRegions(&ctx, &regions).unwrap();
}

#[test]
/// 测试 `test_scatter_backward_compatibility`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scatter_backward_compatibility` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scatter_backward_compatibility() {
    let mock = NewMockPDClientForSplit();
    mock.set_scatter_regions_not_implemented(true);
    let mut client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);
    client.ForceNeedScatter(true);
    let ctx = Context::Background();
    let regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    client.scatterRegions(&ctx, &regions).unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(mock.scatter_region_count(), HashMap::from([(1, 1), (2, 1)]));
}

#[test]
/// 测试 `test_wait_for_scatter_regions`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_wait_for_scatter_regions` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_wait_for_scatter_regions() {
    let mock = NewMockPDClientForSplit();
    mock.set_scatter_regions_not_implemented(true);
    let mut client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);
    client.ForceNeedScatter(true);
    let region_cnt = 6usize;
    let ctx = Context::Background();
    let regions: Vec<_> = (1..=region_cnt)
        .map(|i| RegionInfo {
            Region: Some(metapb::Region {
                Id: i as u64,
                ..Default::default()
            }),
            ..Default::default()
        })
        .collect();

    let mut responses = HashMap::new();
    responses.insert(
        1,
        vec![pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader {
                Error: Some(pdpb::Error {
                    Type: pdpb::ErrorType::REGION_NOT_FOUND,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        }],
    );
    responses.insert(
        2,
        vec![pdpb::GetOperatorResponse {
            Desc: b"not-scatter-region".to_vec(),
            ..Default::default()
        }],
    );
    responses.insert(
        3,
        vec![pdpb::GetOperatorResponse {
            Desc: b"scatter-region".to_vec(),
            Status: pdpb::OperatorStatus::SUCCESS,
            ..Default::default()
        }],
    );
    responses.insert(
        4,
        vec![
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::TIMEOUT,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::SUCCESS,
                ..Default::default()
            },
        ],
    );
    responses.insert(
        5,
        vec![
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::CANCEL,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::CANCEL,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::CANCEL,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"not-scatter-region".to_vec(),
                ..Default::default()
            },
        ],
    );
    responses.insert(
        6,
        vec![
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::REPLACE,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::SUCCESS,
                ..Default::default()
            },
        ],
    );
    mock.set_get_operator_responses(responses);

    let (left, err) = client.WaitRegionsScattered(&ctx, &regions).unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().is_empty() || left == 0, "{err}");
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(left, 0);
    for i in 1..=3u64 {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(mock.scatter_region_count().get(&i).copied().unwrap_or(0), 0);
    }
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(mock.scatter_region_count().get(&4).copied().unwrap_or(0), 1);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(mock.scatter_region_count().get(&5).copied().unwrap_or(0), 3);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(mock.scatter_region_count().get(&6).copied().unwrap_or(0), 1);
    for i in 1..=region_cnt as u64 {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(mock.get_operator_len(i), 0);
    }

    // non-retryable error
    mock.reset_scatter_region_count();
    let mut responses = HashMap::new();
    responses.insert(
        1,
        vec![pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader {
                Error: Some(pdpb::Error {
                    Type: pdpb::ErrorType::REGION_NOT_FOUND,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        }],
    );
    responses.insert(
        2,
        vec![pdpb::GetOperatorResponse {
            Desc: b"not-scatter-region".to_vec(),
            ..Default::default()
        }],
    );
    responses.insert(
        3,
        vec![pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader {
                Error: Some(pdpb::Error {
                    Type: pdpb::ErrorType::DATA_COMPACTED,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        }],
    );
    mock.set_get_operator_responses(responses);
    let (left, err) = client.WaitRegionsScattered(&ctx, &regions).unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().contains("DATA_COMPACTED"), "{err}");
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(left, 4);

    // backoff timeout
    let backup = WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst);
    WaitRegionOnlineAttemptTimes.store(2, Ordering::SeqCst);
    mock.reset_scatter_region_count();
    let mut responses = HashMap::new();
    responses.insert(
        1,
        vec![pdpb::GetOperatorResponse {
            Header: Some(pdpb::ResponseHeader {
                Error: Some(pdpb::Error {
                    Type: pdpb::ErrorType::REGION_NOT_FOUND,
                    ..Default::default()
                }),
            }),
            ..Default::default()
        }],
    );
    responses.insert(
        2,
        vec![pdpb::GetOperatorResponse {
            Desc: b"not-scatter-region".to_vec(),
            ..Default::default()
        }],
    );
    responses.insert(
        3,
        vec![pdpb::GetOperatorResponse {
            Desc: b"scatter-region".to_vec(),
            Status: pdpb::OperatorStatus::SUCCESS,
            ..Default::default()
        }],
    );
    responses.insert(
        4,
        vec![
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
            pdpb::GetOperatorResponse {
                Desc: b"scatter-region".to_vec(),
                Status: pdpb::OperatorStatus::RUNNING,
                ..Default::default()
            },
        ],
    );
    responses.insert(
        5,
        vec![pdpb::GetOperatorResponse {
            Desc: b"not-scatter-region".to_vec(),
            ..Default::default()
        }],
    );
    responses.insert(
        6,
        vec![pdpb::GetOperatorResponse {
            Desc: b"scatter-region".to_vec(),
            Status: pdpb::OperatorStatus::SUCCESS,
            ..Default::default()
        }],
    );
    mock.set_get_operator_responses(responses);
    let (left, err) = client.WaitRegionsScattered(&ctx, &regions).unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        err.to_string()
            .contains("the first unfinished region: id:4"),
        "{err}"
    );
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(left, 1);
    WaitRegionOnlineAttemptTimes.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_backoff_may_not_count_backoffer`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_backoff_may_not_count_backoffer` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_backoff_may_not_count_backoffer() {
    let mut b = NewBackoffMayNotCountBackoffer();
    let init = b.RemainingAttempts();
    b.NextBackoff(&ErrBackoffAndDontCount());
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(b.RemainingAttempts(), init);
    let annotated = Annotatef(Some(ErrBackoffAndDontCount()), "caller message", &[]).unwrap();
    b.NextBackoff(&annotated);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(b.RemainingAttempts(), init);
    b.NextBackoff(&ErrBackoff());
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(b.RemainingAttempts(), init - 1);

    let mut lookalike = NewBackoffMayNotCountBackoffer();
    lookalike.NextBackoff(&New("unrelated found backoff error suffix"));
    // Go errors.ErrorEqual compares the unwrapped cause exactly, so merely
    // containing the sentinel text must not make an unrelated error retryable.
    assert_eq!(lookalike.RemainingAttempts(), 0);

    b.NextBackoff(&New("test"));
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(b.RemainingAttempts(), 0);
}

#[test]
/// 测试 `test_split_ctx_cancel`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_ctx_cancel` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_ctx_cancel() {
    let (ctx, cancel) = Context::WithCancel(&Context::Background());
    let cancel = std::sync::Mutex::new(Some(cancel));
    let mock = NewMockPDClientForSplit();
    mock.set_split_hijack(Some(Box::new(move || {
        if let Some(c) = cancel.lock().unwrap().take() {
            // 条件分支：见块内处理与 Go 对齐点。
            c();
        }
        Ok((
            RegionInfo {
                Region: Some(metapb::Region {
                    Id: 1,
                    ..Default::default()
                }),
                ..Default::default()
            },
            vec![
                RegionInfo {
                    Region: Some(metapb::Region {
                        Id: 1,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                RegionInfo {
                    Region: Some(metapb::Region {
                        Id: 2,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        ))
    })));
    let client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    let err = client
        .SplitWaitAndScatter(&ctx, &RegionInfo::default(), &[vec![1]])
        .unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        err.to_string().contains("canceled") || err.to_string().contains("cancel"),
        "{err}"
    );
}

#[test]
/// 测试 `test_get_split_key_per_region`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_get_split_key_per_region` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_get_split_key_per_region() {
    let sorted_keys = vec![
        b"b".to_vec(),
        b"d".to_vec(),
        b"g".to_vec(),
        b"j".to_vec(),
        b"l".to_vec(),
        b"m".to_vec(),
    ];
    let sorted_regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                StartKey: b"a".to_vec(),
                EndKey: b"g".to_vec(),
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                StartKey: b"g".to_vec(),
                EndKey: b"k".to_vec(),
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 3,
                StartKey: b"k".to_vec(),
                EndKey: b"m".to_vec(),
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    let result = getSplitKeysOfRegions(&sorted_keys, &sorted_regions, false);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(result.len(), 3);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(result.get(&1).unwrap(), &vec![b"b".to_vec(), b"d".to_vec()]);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(result.get(&2).unwrap(), &vec![b"g".to_vec(), b"j".to_vec()]);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(result.get(&3).unwrap(), &vec![b"l".to_vec()]);

    let table_id: i64 = 1;
    let keys_ends = [1i64, 10, 100, 1000, 10000, -1];
    let mut sorted_regions = Vec::new();
    let mut start = tablecodec::EncodeRowKeyWithHandle(table_id, &tablecodec::IntHandle(0));
    let mut region_start = codec::EncodeBytes(Vec::new(), &start);
    for (i, end) in keys_ends.iter().enumerate() {
        let region_end_key = if *end >= 0 {
            let end_key =
                tablecodec::EncodeRowKeyWithHandle(table_id, &tablecodec::IntHandle(*end));
            codec::EncodeBytes(Vec::new(), &end_key)
        } else {
            vec![]
        };
        sorted_regions.push(RegionInfo {
            Region: Some(metapb::Region {
                Id: i as u64,
                StartKey: region_start.clone(),
                EndKey: region_end_key.clone(),
                ..Default::default()
            }),
            ..Default::default()
        });
        region_start = region_end_key;
        let _ = start;
    }

    let check_keys = HashMap::from([
        (0i64, -1i64),
        (5, 1),
        (6, 1),
        (7, 1),
        (50, 2),
        (60, 2),
        (70, 2),
        (100, -1),
        (50000, 5),
    ]);
    let mut expected: HashMap<u64, Vec<Vec<u8>>> = HashMap::new();
    let mut sorted_keys = Vec::new();
    for (hdl, idx) in &check_keys {
        let key = tablecodec::EncodeRowKeyWithHandle(table_id, &tablecodec::IntHandle(*hdl));
        sorted_keys.push(key.clone());
        if *idx >= 0 {
            // 条件分支：见块内处理与 Go 对齐点。
            expected.entry(*idx as u64).or_default().push(key);
        }
    }
    sorted_keys.sort();
    for v in expected.values_mut() {
        v.sort();
    }
    let got = getSplitKeysOfRegions(&sorted_keys, &sorted_regions, false);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(got.len(), expected.len());
    for (region_id, keys) in got {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(&keys, expected.get(&region_id).unwrap());
    }
}

#[test]
/// 测试 `test_paginate_scan_region_with_codec_aware_codec_pd_client`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_paginate_scan_region_with_codec_aware_codec_pd_client` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_paginate_scan_region_with_codec_aware_codec_pd_client() {
    let ctx = Context::Background();
    {
        let mock = NewMockPDClientForSplit();
        let physical = vec![
            codec::EncodeBytes(Vec::new(), b"a"),
            codec::EncodeBytes(Vec::new(), b"b"),
            codec::EncodeBytes(Vec::new(), b"d"),
        ];
        mock.SetRegions(&physical);
        let client = NewCodecAwareClient(Box::new(mock), None, 100, 4, vec![]);
        let regions = PaginateScanRegionWithCodecAware(
            &ctx,
            &client,
            codec::EncodeBytes(Vec::new(), b"a"),
            codec::EncodeBytes(Vec::new(), b"d"),
            2,
        )
        .unwrap();
        check_regions_boundaries(&regions, &physical);
    }
    {
        let mock = NewMockPDClientForSplit();
        let physical = vec![
            codec::EncodeBytes(Vec::new(), b"a"),
            codec::EncodeBytes(Vec::new(), b"b"),
            vec![],
        ];
        mock.SetRegions(&physical);
        let client = NewCodecAwareClient(Box::new(mock), None, 100, 4, vec![]);
        // Empty logical end stays unbounded (Go EncodeRange(..., nil)).
        let regions = PaginateScanRegionWithCodecAware(
            &ctx,
            &client,
            codec::EncodeBytes(Vec::new(), b"a"),
            vec![],
            2,
        )
        .unwrap();
        check_regions_boundaries(&regions, &physical);
    }
}

#[test]
/// 测试 `test_paginate_scan_region`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_paginate_scan_region` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_paginate_scan_region() {
    let ctx = Context::Background();
    let mock = NewMockPDClientForSplit();
    let client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);

    let backup = WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst);
    WaitRegionOnlineAttemptTimes.store(3, Ordering::SeqCst);

    let err = PaginateScanRegion(&ctx, &client, &[], &[], 3).unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(ErrPDBatchScanRegion.Equal(Some(&err)));
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().contains("scan region return empty result"));

    mock.push_scan_error(Some(New("not leader")));
    mock.SetRegions(&[vec![], vec![]]);
    let got = PaginateScanRegion(&ctx, &client, &[], &[], 3).unwrap();
    check_regions_boundaries(&got, &[vec![], vec![]]);

    let boundaries = vec![
        vec![],
        vec![1],
        vec![2],
        vec![3],
        vec![4],
        vec![5],
        vec![6],
        vec![7],
        vec![8],
        vec![],
    ];
    mock.SetRegions(&boundaries);
    let got = PaginateScanRegion(&ctx, &client, &[], &[], 3).unwrap();
    check_regions_boundaries(&got, &boundaries);
    let got = PaginateScanRegion(&ctx, &client, &[1], &[], 3).unwrap();
    check_regions_boundaries(&got, &boundaries[1..]);
    let got = PaginateScanRegion(&ctx, &client, &[], &[2], 8).unwrap();
    check_regions_boundaries(&got, &boundaries[..3]);
    let got = PaginateScanRegion(&ctx, &client, &[4], &[5], 1).unwrap();
    check_regions_boundaries(&got, &[vec![4], vec![5]]);

    let err = PaginateScanRegion(&ctx, &client, &[4], &[4], 1).unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().contains("scan region return empty result"));
    let err = PaginateScanRegion(&ctx, &client, &[5], &[4], 5).unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().contains("startKey > endKey"));

    mock.push_scan_error(Some(New("not leader")));
    mock.push_scan_error(Some(New("not leader")));
    mock.push_scan_error(Some(New("not leader")));
    let err = PaginateScanRegion(&ctx, &client, &[4], &[5], 1).unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(err.to_string().contains("not leader"));

    mock.replace_regions_tree(RegionTree::new());
    mock.set_region_info(RegionInfo {
        Region: Some(metapb::Region {
            Id: 1,
            StartKey: vec![1],
            EndKey: vec![2],
            ..Default::default()
        }),
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
        ..Default::default()
    });
    mock.set_region_info(RegionInfo {
        Region: Some(metapb::Region {
            Id: 4,
            StartKey: vec![4],
            EndKey: vec![5],
            ..Default::default()
        }),
        Leader: Some(metapb::Peer { Id: 4, StoreId: 1 }),
        ..Default::default()
    });
    let err = PaginateScanRegion(&ctx, &client, &[1], &[5], 3).unwrap_err();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(ErrPDBatchScanRegion.Equal(Some(&err)));
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        err.to_string()
            .contains("region 1's endKey not equal to next region 4's startKey")
    );

    let to_add = Mutex::new(vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                StartKey: vec![2],
                EndKey: vec![3],
                ..Default::default()
            }),
            Leader: Some(metapb::Peer { Id: 2, StoreId: 1 }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 3,
                StartKey: vec![3],
                EndKey: vec![4],
                ..Default::default()
            }),
            Leader: Some(metapb::Peer { Id: 3, StoreId: 1 }),
            ..Default::default()
        },
    ]);
    let mock2 = mock.clone();
    mock.set_scan_before_hook(move || {
        let mut g = to_add.lock().unwrap();
        if !g.is_empty() {
            // 条件分支：见块内处理与 Go 对齐点。
            let r = g.remove(0);
            mock2.set_region_info(r);
        }
    });
    let got = PaginateScanRegion(&ctx, &client, &[1], &[5], 100).unwrap();
    check_regions_boundaries(&got, &[vec![1], vec![2], vec![3], vec![4], vec![5]]);

    WaitRegionOnlineAttemptTimes.store(backup, Ordering::SeqCst);
}

/// Scripted ScanRegions backend for limit/retry tests.
/// `MockPDErrorClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `MockPDErrorClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct MockPDErrorClient {
    cases: Mutex<Vec<ScanRegionTestCase>>,
}

/// `ScanRegionTestCase`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `ScanRegionTestCase` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct ScanRegionTestCase {
    allow_follower_handle: bool,
    case_error: Option<SharedError>,
    case_region: Vec<RegionInfo>,
}

/// `MockPDErrorClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `MockPDErrorClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl PdBackend for MockPDErrorClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(&self, _storeID: u64) -> Result<metapb::Store, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, _key: &[u8]) -> Result<RegionInfo, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, _regionID: u64) -> Result<RegionInfo, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _key: &[u8],
        _endKey: &[u8],
        _limit: i32,
        allow_follower: bool,
    ) -> Result<Vec<RegionInfo>, SharedError> {
        let mut g = self.cases.lock().unwrap();
        let case = g.remove(0);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(case.allow_follower_handle, allow_follower);
        match case.case_error {
            // 分支匹配：各臂处理不同结果/错误。
            Some(e) => Err(e),
            None => Ok(case.case_region),
        }
    }
    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _regionID: u64) -> Result<pdpb::GetOperatorResponse, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegions(
        &self,
        _regionIDs: &[u64],
    ) -> Result<pdpb::ScatterRegionResponse, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScatterRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScatterRegion(&self, _regionID: u64) -> Result<(), SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>, SharedError> {
        Ok(vec![])
    }
    /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitRegion(
        &self,
        region: &RegionInfo,
        _keys: &[Vec<u8>],
        _is_raw_kv: bool,
    ) -> Result<(RegionInfo, Vec<RegionInfo>), SharedError> {
        Ok((region.clone(), vec![region.clone()]))
    }
}

/// `new_case_region`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `new_case_region` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn new_case_region(kids: &[u64]) -> Vec<RegionInfo> {
    kids.iter()
        .map(|kid| RegionInfo {
            Region: Some(metapb::Region {
                Id: *kid,
                StartKey: format!("{kid:03}").into_bytes(),
                EndKey: format!("{:03}", kid + 1).into_bytes(),
                ..Default::default()
            }),
            Leader: Some(metapb::Peer {
                Id: *kid,
                StoreId: *kid,
            }),
            ..Default::default()
        })
        .collect()
}

/// `rk`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `rk` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn rk(kid: i32) -> Vec<u8> {
    format!("{kid:03}5").into_bytes()
}

/// `check_regions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
/// `check_regions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
fn check_regions(start_kid: u64, end_kid: u64, regions: &[RegionInfo]) {
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions.len() as u64, end_kid - start_kid + 1);
    let mut i = 0;
    for kid in start_kid..=end_kid {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(regions[i].Leader.as_ref().unwrap().Id, kid);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(regions[i].Leader.as_ref().unwrap().StoreId, kid);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(regions[i].Region.as_ref().unwrap().Id, kid);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            regions[i].Region.as_ref().unwrap().StartKey,
            format!("{kid:03}").into_bytes()
        );
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            regions[i].Region.as_ref().unwrap().EndKey,
            format!("{:03}", kid + 1).into_bytes()
        );
        i += 1;
    }
}

#[test]
/// 测试 `test_scan_regions_limit_with_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_regions_limit_with_retry` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_regions_limit_with_retry() {
    let ctx = Context::Background();
    let mock = MockPDErrorClient {
        cases: Mutex::new(Vec::new()),
    };
    let client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    // Re-bind mock via reconstructing — use shared Arc style instead.
    let _ = client;
    let mock = std::sync::Arc::new(Mutex::new(Vec::<ScanRegionTestCase>::new()));
    /// `SharedMock`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `SharedMock` 生命周期：构造后是否可变、是否跨线程共享需明确。
    struct SharedMock(std::sync::Arc<Mutex<Vec<ScanRegionTestCase>>>);
    /// `SharedMock` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SharedMock` 方法边界：非法参数应返回可分类错误而非 panic。
    impl PdBackend for SharedMock {
        /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetStore(&self, _: u64) -> Result<metapb::Store, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegion(&self, _: &[u8]) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegionByID(&self, _: u64) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScanRegions(
            &self,
            _: &[u8],
            _: &[u8],
            _: i32,
            allow_follower: bool,
        ) -> Result<Vec<RegionInfo>, SharedError> {
            let mut g = self.0.lock().unwrap();
            let case = g.remove(0);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(case.allow_follower_handle, allow_follower);
            match case.case_error {
                // 分支匹配：各臂处理不同结果/错误。
                Some(e) => Err(e),
                None => Ok(case.case_region),
            }
        }
        /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetOperator(&self, _: u64) -> Result<pdpb::GetOperatorResponse, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScatterRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScatterRegions(&self, _: &[u64]) -> Result<pdpb::ScatterRegionResponse, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScatterRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScatterRegion(&self, _: u64) -> Result<(), SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetAllStores(&self) -> Result<Vec<metapb::Store>, SharedError> {
            Ok(vec![])
        }
        /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SplitRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SplitRegion(
            &self,
            region: &RegionInfo,
            _: &[Vec<u8>],
            _: bool,
        ) -> Result<(RegionInfo, Vec<RegionInfo>), SharedError> {
            Ok((region.clone(), vec![region.clone()]))
        }
    }

    let backup = WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst);
    WaitRegionOnlineAttemptTimes.store(3, Ordering::SeqCst);
    let case_error = Annotatef(
        Some(SharedError::new((*ErrPDBatchScanRegion).clone())),
        "case error",
        &[],
    )
    .unwrap();

    {
        let cases = mock.clone();
        *cases.lock().unwrap() = vec![
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: Some(case_error.clone()),
                case_region: vec![],
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
        ];
        let client = NewClient(Box::new(SharedMock(cases.clone())), None, 100, 4, vec![]);
        let (_, must_leader) =
            scanRegionsLimitWithRetry(&ctx, &client, &rk(1), &rk(2), 128, false).unwrap();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(must_leader);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(cases.lock().unwrap().is_empty());
    }
    {
        let cases = mock.clone();
        *cases.lock().unwrap() = vec![ScanRegionTestCase {
            allow_follower_handle: true,
            case_error: None,
            case_region: new_case_region(&[1, 2, 3]),
        }];
        let client = NewClient(Box::new(SharedMock(cases.clone())), None, 100, 4, vec![]);
        let (_, must_leader) =
            scanRegionsLimitWithRetry(&ctx, &client, &rk(1), &rk(2), 128, false).unwrap();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(!must_leader);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(cases.lock().unwrap().is_empty());
    }
    {
        let cases = mock.clone();
        *cases.lock().unwrap() = vec![
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: Some(case_error.clone()),
                case_region: vec![],
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
        ];
        let client = NewClient(Box::new(SharedMock(cases.clone())), None, 100, 4, vec![]);
        let (_, must_leader) =
            scanRegionsLimitWithRetry(&ctx, &client, &rk(1), &rk(2), 128, true).unwrap();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(must_leader);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(cases.lock().unwrap().is_empty());
    }
    {
        let cases = mock.clone();
        *cases.lock().unwrap() = vec![ScanRegionTestCase {
            allow_follower_handle: false,
            case_error: None,
            case_region: new_case_region(&[1, 2, 3]),
        }];
        let client = NewClient(Box::new(SharedMock(cases.clone())), None, 100, 4, vec![]);
        let (_, must_leader) =
            scanRegionsLimitWithRetry(&ctx, &client, &rk(1), &rk(2), 128, true).unwrap();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(must_leader);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(cases.lock().unwrap().is_empty());
    }

    WaitRegionOnlineAttemptTimes.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_paginate_scan_region2`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_paginate_scan_region2` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_paginate_scan_region2() {
    let ctx = Context::Background();
    let cases_store = std::sync::Arc::new(Mutex::new(Vec::<ScanRegionTestCase>::new()));
    /// `SharedMock`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `SharedMock` 生命周期：构造后是否可变、是否跨线程共享需明确。
    struct SharedMock(std::sync::Arc<Mutex<Vec<ScanRegionTestCase>>>);
    /// `SharedMock` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `SharedMock` 方法边界：非法参数应返回可分类错误而非 panic。
    impl PdBackend for SharedMock {
        /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetStore(&self, _: u64) -> Result<metapb::Store, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegion(&self, _: &[u8]) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegionByID(&self, _: u64) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScanRegions(
            &self,
            _: &[u8],
            _: &[u8],
            _: i32,
            allow_follower: bool,
        ) -> Result<Vec<RegionInfo>, SharedError> {
            let mut g = self.0.lock().unwrap();
            let case = g.remove(0);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(case.allow_follower_handle, allow_follower);
            match case.case_error {
                // 分支匹配：各臂处理不同结果/错误。
                Some(e) => Err(e),
                None => Ok(case.case_region),
            }
        }
        /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetOperator(&self, _: u64) -> Result<pdpb::GetOperatorResponse, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScatterRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScatterRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScatterRegions(&self, _: &[u64]) -> Result<pdpb::ScatterRegionResponse, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScatterRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScatterRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScatterRegion(&self, _: u64) -> Result<(), SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetAllStores` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetAllStores(&self) -> Result<Vec<metapb::Store>, SharedError> {
            Ok(vec![])
        }
        /// `SplitRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SplitRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SplitRegion(
            &self,
            region: &RegionInfo,
            _: &[Vec<u8>],
            _: bool,
        ) -> Result<(RegionInfo, Vec<RegionInfo>), SharedError> {
            Ok((region.clone(), vec![region.clone()]))
        }
    }

    let backup = WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst);
    WaitRegionOnlineAttemptTimes.store(3, Ordering::SeqCst);
    let case_error = Annotatef(
        Some(SharedError::new((*ErrPDBatchScanRegion).clone())),
        "case error",
        &[],
    )
    .unwrap();

    {
        *cases_store.lock().unwrap() = vec![
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: Some(case_error.clone()),
                case_region: vec![],
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[4, 5]),
            },
        ];
        let client = NewClient(
            Box::new(SharedMock(cases_store.clone())),
            None,
            100,
            4,
            vec![],
        );
        let regions = PaginateScanRegion(&ctx, &client, &rk(1), &rk(5), 3).unwrap();
        check_regions(1, 5, &regions);
    }
    {
        *cases_store.lock().unwrap() = vec![
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: Some(case_error.clone()),
                case_region: vec![],
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: Some(case_error.clone()),
                case_region: vec![],
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[4, 5, 6]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[7, 8, 9]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[10]),
            },
        ];
        let client = NewClient(
            Box::new(SharedMock(cases_store.clone())),
            None,
            100,
            4,
            vec![],
        );
        let regions = PaginateScanRegion(&ctx, &client, &rk(1), &rk(10), 3).unwrap();
        check_regions(1, 10, &regions);
    }
    {
        *cases_store.lock().unwrap() = vec![
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: true,
                case_error: None,
                case_region: new_case_region(&[4, 5]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[1, 2, 3]),
            },
            ScanRegionTestCase {
                allow_follower_handle: false,
                case_error: None,
                case_region: new_case_region(&[4, 5, 6]),
            },
        ];
        let client = NewClient(
            Box::new(SharedMock(cases_store.clone())),
            None,
            100,
            4,
            vec![],
        );
        let regions = PaginateScanRegion(&ctx, &client, &rk(1), &rk(6), 3).unwrap();
        check_regions(1, 6, &regions);
    }

    WaitRegionOnlineAttemptTimes.store(backup, Ordering::SeqCst);
}

#[test]
/// 测试 `test_region_consistency`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_region_consistency` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_region_consistency() {
    let cases = vec![
        (
            codec::EncodeBytes(Vec::new(), b"a"),
            codec::EncodeBytes(Vec::new(), b"a"),
            "scan region return empty result",
            vec![],
        ),
        (
            codec::EncodeBytes(Vec::new(), b"a"),
            codec::EncodeBytes(Vec::new(), b"a"),
            "startKey",
            vec![RegionInfo {
                Region: Some(metapb::Region {
                    Id: 1,
                    StartKey: codec::EncodeBytes(Vec::new(), b"b"),
                    EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        ),
        (
            codec::EncodeBytes(Vec::new(), b"b"),
            codec::EncodeBytes(Vec::new(), b"e"),
            "endKey",
            vec![RegionInfo {
                Region: Some(metapb::Region {
                    Id: 100,
                    StartKey: codec::EncodeBytes(Vec::new(), b"b"),
                    EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        ),
        (
            codec::EncodeBytes(Vec::new(), b"c"),
            codec::EncodeBytes(Vec::new(), b"e"),
            "endKey not equal",
            vec![
                RegionInfo {
                    Leader: Some(metapb::Peer { Id: 6, StoreId: 1 }),
                    Region: Some(metapb::Region {
                        Id: 6,
                        StartKey: codec::EncodeBytes(Vec::new(), b"b"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                RegionInfo {
                    Leader: Some(metapb::Peer { Id: 8, StoreId: 1 }),
                    Region: Some(metapb::Region {
                        Id: 8,
                        StartKey: codec::EncodeBytes(Vec::new(), b"e"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"f"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        ),
        (
            codec::EncodeBytes(Vec::new(), b"c"),
            codec::EncodeBytes(Vec::new(), b"e"),
            "leader is nil",
            vec![
                RegionInfo {
                    Region: Some(metapb::Region {
                        Id: 6,
                        StartKey: codec::EncodeBytes(Vec::new(), b"c"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                RegionInfo {
                    Region: Some(metapb::Region {
                        Id: 8,
                        StartKey: codec::EncodeBytes(Vec::new(), b"d"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"e"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        ),
        (
            codec::EncodeBytes(Vec::new(), b"c"),
            codec::EncodeBytes(Vec::new(), b"e"),
            "store id is 0",
            vec![
                RegionInfo {
                    Leader: Some(metapb::Peer { Id: 6, StoreId: 0 }),
                    Region: Some(metapb::Region {
                        Id: 6,
                        StartKey: codec::EncodeBytes(Vec::new(), b"c"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                RegionInfo {
                    Leader: Some(metapb::Peer { Id: 6, StoreId: 0 }),
                    Region: Some(metapb::Region {
                        Id: 8,
                        StartKey: codec::EncodeBytes(Vec::new(), b"d"),
                        EndKey: codec::EncodeBytes(Vec::new(), b"e"),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        ),
    ];
    for (start, end, needle, regions) in cases {
        let err = checkRegionConsistency(&start, &end, &regions, false).unwrap_err();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert!(err.to_string().contains(needle), "{err} vs {needle}");
    }
}

#[test]
/// 测试 `test_split_check_part_region_consistency`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_check_part_region_consistency` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_check_part_region_consistency() {
    let start = b"a";
    let end = b"f";
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(checkPartRegionConsistency(start, end, &[]).is_err());
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(checkPartRegionConsistency(start, end, &[region_info("b", "c")]).is_err());
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        checkPartRegionConsistency(start, end, &[region_info("a", "c"), region_info("d", "e")])
            .is_err()
    );
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        checkPartRegionConsistency(start, end, &[region_info("a", "c"), region_info("c", "d")])
            .is_ok()
    );
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        checkPartRegionConsistency(
            start,
            end,
            &[
                region_info("a", "c"),
                region_info("c", "d"),
                region_info("d", "f")
            ]
        )
        .is_ok()
    );
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(
        checkPartRegionConsistency(start, end, &[region_info("a", "c"), region_info("c", "z")])
            .is_ok()
    );
}

#[test]
/// 测试 `test_scan_regions_with_retry`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_regions_with_retry` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_regions_with_retry() {
    let ctx = Context::Background();
    let mock = NewMockPDClientForSplit();
    let client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert!(ScanRegionsWithRetry(&ctx, &client, b"1", b"0", 0).is_err());

    mock.SetRegions(&[
        vec![],
        b"1".to_vec(),
        b"2".to_vec(),
        b"3".to_vec(),
        b"4".to_vec(),
        vec![],
    ]);
    let regions = ScanRegionsWithRetry(&ctx, &client, b"1", b"3", 0).unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions.len(), 2);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions[0].Region.as_ref().unwrap().StartKey, b"1");
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions[1].Region.as_ref().unwrap().StartKey, b"2");
}

#[test]
/// 测试 `test_scan_empty_region`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_scan_empty_region` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_scan_empty_region() {
    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&[vec![], vec![12], vec![34], vec![]]);
    let client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    let keys = init_keys()[..1].to_vec();
    let splitter = NewRegionSplitter(Box::new(client));
    splitter
        .ExecuteSortedKeys(&Context::Background(), &keys)
        .unwrap();
}

/// `RecordingSplitClient`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
/// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
/// `RecordingSplitClient` 生命周期：构造后是否可变、是否跨线程共享需明确。
struct RecordingSplitClient {
    split_calls: Mutex<Vec<Vec<Vec<u8>>>>,
    scatter_by_calls: Mutex<Vec<bool>>,
}

/// `RecordingSplitClient` 的 impl：方法语义、错误传播与并发约束对齐 Go。
/// 桩实现仅服务测试，不可当作生产路径完备性证明。
/// `RecordingSplitClient` 方法边界：非法参数应返回可分类错误而非 panic。
impl SplitClient for RecordingSplitClient {
    /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetStore(
        &self,
        _: &Context,
        _: u64,
        _: &[GetStoreOption],
    ) -> Result<metapb::Store, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegion(&self, _: &Context, _: &[u8]) -> Result<RegionInfo, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetRegionByID(&self, _: &Context, _: u64) -> Result<RegionInfo, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeysAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeysAndScatter(
        &self,
        _: &Context,
        keys: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>, SharedError> {
        self.split_calls.lock().unwrap().push(keys.to_vec());
        self.scatter_by_calls.lock().unwrap().push(true);
        Ok(vec![])
    }
    /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitKeys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitKeys(&self, _: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>, SharedError> {
        self.split_calls.lock().unwrap().push(keys.to_vec());
        self.scatter_by_calls.lock().unwrap().push(false);
        Ok(vec![])
    }
    /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SplitWaitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SplitWaitAndScatter(
        &self,
        _: &Context,
        _: &RegionInfo,
        _: &[Vec<u8>],
    ) -> Result<Vec<RegionInfo>, SharedError> {
        Ok(vec![])
    }
    /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetOperator(&self, _: &Context, _: u64) -> Result<pdpb::GetOperatorResponse, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn ScanRegions(
        &self,
        _: &Context,
        _: &[u8],
        _: &[u8],
        _: i32,
        _: &[GetRegionOption],
    ) -> Result<Vec<RegionInfo>, SharedError> {
        Ok(vec![])
    }
    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn GetPlacementRule(&self, _: &Context, _: &str, _: &str) -> Result<pdhttp::Rule, SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetPlacementRule(&self, _: &Context, _: &pdhttp::Rule) -> Result<(), SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn DeletePlacementRule(&self, _: &Context, _: &str, _: &str) -> Result<(), SharedError> {
        Err(New("n/a"))
        // 错误路径：向上返回，保留上下文。
    }
    /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    /// `SetStoresLabel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
    fn SetStoresLabel(&self, _: &Context, _: &[u64], _: &str, _: &str) -> Result<(), SharedError> {
        Ok(())
    }
}

#[test]
/// 测试 `test_region_splitter_rough_split_uses_configured_region_index_step`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_region_splitter_rough_split_uses_configured_region_index_step` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_region_splitter_rough_split_uses_configured_region_index_step() {
    let keys = vec![
        b"a".to_vec(),
        b"b".to_vec(),
        b"c".to_vec(),
        b"d".to_vec(),
        b"e".to_vec(),
        b"f".to_vec(),
    ];
    for (coarse, expected_scatter) in [(false, vec![true, true]), (true, vec![true, false])] {
        let client = RecordingSplitClient {
            split_calls: Mutex::new(Vec::new()),
            scatter_by_calls: Mutex::new(Vec::new()),
        };
        // Need to leak or wrap — RegionSplitter takes Box<dyn SplitClient>
        // Use a shared recording via Arc.
        let _ = (coarse, expected_scatter, client, &keys);
    }

    // Arc-backed recorder
    #[derive(Clone)]
    /// `Rec`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    /// 字段语义与 Go 对照，避免测试桩字段被误读为协议扩展。
    /// `Rec` 生命周期：构造后是否可变、是否跨线程共享需明确。
    struct Rec {
        split_calls: std::sync::Arc<Mutex<Vec<Vec<Vec<u8>>>>>,
        scatter_by_calls: std::sync::Arc<Mutex<Vec<bool>>>,
    }
    /// `Rec` 的 impl：方法语义、错误传播与并发约束对齐 Go。
    /// 桩实现仅服务测试，不可当作生产路径完备性证明。
    /// `Rec` 方法边界：非法参数应返回可分类错误而非 panic。
    impl SplitClient for Rec {
        /// `GetStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetStore` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetStore(
            &self,
            _: &Context,
            _: u64,
            _: &[GetStoreOption],
        ) -> Result<metapb::Store, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegion` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegion(&self, _: &Context, _: &[u8]) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `GetRegionByID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetRegionByID` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetRegionByID(&self, _: &Context, _: u64) -> Result<RegionInfo, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `SplitKeysAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SplitKeysAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SplitKeysAndScatter(
            &self,
            _: &Context,
            keys: &[Vec<u8>],
        ) -> Result<Vec<RegionInfo>, SharedError> {
            self.split_calls.lock().unwrap().push(keys.to_vec());
            self.scatter_by_calls.lock().unwrap().push(true);
            Ok(vec![])
        }
        /// `SplitKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SplitKeys` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SplitKeys(&self, _: &Context, keys: &[Vec<u8>]) -> Result<Vec<RegionInfo>, SharedError> {
            self.split_calls.lock().unwrap().push(keys.to_vec());
            self.scatter_by_calls.lock().unwrap().push(false);
            Ok(vec![])
        }
        /// `SplitWaitAndScatter`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SplitWaitAndScatter` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SplitWaitAndScatter(
            &self,
            _: &Context,
            _: &RegionInfo,
            _: &[Vec<u8>],
        ) -> Result<Vec<RegionInfo>, SharedError> {
            Ok(vec![])
        }
        /// `GetOperator`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetOperator` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetOperator(
            &self,
            _: &Context,
            _: u64,
        ) -> Result<pdpb::GetOperatorResponse, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `ScanRegions` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn ScanRegions(
            &self,
            _: &Context,
            _: &[u8],
            _: &[u8],
            _: i32,
            _: &[GetRegionOption],
        ) -> Result<Vec<RegionInfo>, SharedError> {
            Ok(vec![])
        }
        /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `GetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn GetPlacementRule(
            &self,
            _: &Context,
            _: &str,
            _: &str,
        ) -> Result<pdhttp::Rule, SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SetPlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SetPlacementRule(&self, _: &Context, _: &pdhttp::Rule) -> Result<(), SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `DeletePlacementRule` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn DeletePlacementRule(&self, _: &Context, _: &str, _: &str) -> Result<(), SharedError> {
            Err(New("n/a"))
            // 错误路径：向上返回，保留上下文。
        }
        /// `SetStoresLabel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        /// `SetStoresLabel` 数据流：调用方准备输入，本函数产出可断言结果或错误。
        fn SetStoresLabel(
            &self,
            _: &Context,
            _: &[u64],
            _: &str,
            _: &str,
        ) -> Result<(), SharedError> {
            Ok(())
        }
    }

    for (coarse, expected_scatter) in [(false, vec![true, true]), (true, vec![true, false])] {
        let rec = Rec {
            split_calls: std::sync::Arc::new(Mutex::new(Vec::new())),
            scatter_by_calls: std::sync::Arc::new(Mutex::new(Vec::new())),
        };
        let mut splitter = NewRegionSplitterWithRegionIndexStep(Box::new(rec.clone()), 2);
        splitter.SetCoarseScatter(coarse);
        splitter
            .ExecuteSortedKeys(&Context::Background(), &keys)
            .unwrap();
        let calls = rec.split_calls.lock().unwrap().clone();
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(calls.len(), 2);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(calls[0], vec![b"c".to_vec(), b"e".to_vec()]);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(calls[1], keys);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(*rec.scatter_by_calls.lock().unwrap(), expected_scatter);
    }
}

#[test]
/// 测试 `test_split_empty_region`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_empty_region` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_empty_region() {
    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&[vec![], vec![12], vec![34], vec![]]);
    let client = NewClient(Box::new(mock), None, 100, 4, vec![]);
    let splitter = NewRegionSplitter(Box::new(client));
    splitter
        .ExecuteSortedKeys(&Context::Background(), &[])
        .unwrap();
}

#[test]
/// 测试 `test_split_and_scatter`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_and_scatter` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_and_scatter() {
    let mut range_boundaries = vec![
        vec![],
        b"aay".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"cca".to_vec(),
        vec![],
    ];
    encode_bytes(&mut range_boundaries);
    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&range_boundaries);
    let client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);
    let splitter = NewRegionSplitter(Box::new(client));
    let ctx = Context::Background();

    // Go initRanges end keys rewritten with aa->xx, cc->bb.
    let mut split_keys = vec![
        rewrite_prefix(b"aae", b"aa", b"xx"),
        rewrite_prefix(b"aaz", b"aa", b"xx"),
        rewrite_prefix(b"ccf", b"cc", b"bb"),
        rewrite_prefix(b"ccj", b"cc", b"bb"),
    ];
    split_keys.sort();
    splitter.ExecuteSortedKeys(&ctx, &split_keys).unwrap();
    let regions = mock.scan_regions_tree().ScanRange(&[], &[], 100);
    let mut expected = vec![
        vec![],
        b"aay".to_vec(),
        b"bba".to_vec(),
        b"bbf".to_vec(),
        b"bbh".to_vec(),
        b"bbj".to_vec(),
        b"cca".to_vec(),
        b"xxe".to_vec(),
        b"xxz".to_vec(),
        vec![],
    ];
    encode_bytes(&mut expected);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions.len(), expected.len() - 1);
    for (i, region) in regions.iter().enumerate() {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(region.Region.as_ref().unwrap().StartKey, expected[i]);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(region.Region.as_ref().unwrap().EndKey, expected[i + 1]);
    }
}

#[test]
/// 测试 `test_raw_split`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_raw_split` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_raw_split() {
    let split_keys = vec![vec![]];
    let ctx = Context::Background();
    let range_boundaries = vec![
        vec![],
        b"aay".to_vec(),
        b"bba".to_vec(),
        b"bbh".to_vec(),
        b"cca".to_vec(),
        vec![],
    ];
    let mock = NewMockPDClientForSplit();
    mock.SetRegions(&range_boundaries);
    let client = NewClient(
        Box::new(mock.clone()),
        None,
        100,
        4,
        vec![crate::client::WithRawKV()],
    );
    let splitter = NewRegionSplitter(Box::new(client));
    splitter.ExecuteSortedKeys(&ctx, &split_keys).unwrap();
    let regions = mock.scan_regions_tree().ScanRange(&[], &[], 100);
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(regions.len(), range_boundaries.len() - 1);
    for (i, region) in regions.iter().enumerate() {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            region.Region.as_ref().unwrap().StartKey,
            range_boundaries[i]
        );
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            region.Region.as_ref().unwrap().EndKey,
            range_boundaries[i + 1]
        );
    }
}

#[test]
/// 测试 `test_split_point`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_point` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_point() {
    let ctx = Context::Background();
    let old_table_id: i64 = 50;
    let table_id: i64 = 100;
    let rewrite_rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(old_table_id),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(table_id),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut split_helper = NewSplitHelper();
    split_helper.Merge(Valued {
        Key: Span {
            StartKey: key_with_table_prefix(old_table_id, "b"),
            EndKey: key_with_table_prefix(old_table_id, "c"),
        },
        Value: Value {
            Size: 100,
            Number: 100,
        },
    });
    split_helper.Merge(Valued {
        Key: Span {
            StartKey: key_with_table_prefix(old_table_id, "d"),
            EndKey: key_with_table_prefix(old_table_id, "e"),
        },
        Value: Value {
            Size: 200,
            Number: 200,
        },
    });
    split_helper.Merge(Valued {
        Key: Span {
            StartKey: key_with_table_prefix(old_table_id, "g"),
            EndKey: key_with_table_prefix(old_table_id, "i"),
        },
        Value: Value {
            Size: 300,
            Number: 300,
        },
    });
    let client = NewFakeSplitClient();
    client.AppendRegion(
        key_with_table_prefix(table_id, "a"),
        key_with_table_prefix(table_id, "f"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "f"),
        key_with_table_prefix(table_id, "h"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "h"),
        key_with_table_prefix(table_id, "j"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "j"),
        key_with_table_prefix(table_id + 1, "a"),
    );
    let iter = NewSplitHelperIterator(vec![NewRewriteSpliter(
        Vec::new(),
        table_id,
        rewrite_rules,
        split_helper,
    )]);
    let mut calls = 0;
    SplitPoint(&ctx, &iter, &client, |_ctx, u, o, ri, valueds| {
        calls += 1;
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(u, 0);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(o, 0);
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            ri.Region.as_ref().unwrap().StartKey,
            key_with_table_prefix(table_id, "a")
        );
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(
            ri.Region.as_ref().unwrap().EndKey,
            key_with_table_prefix(table_id, "f")
        );
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        assert_eq!(valueds.len(), 2);
        Ok(())
    })
    .unwrap();
    // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
    assert_eq!(calls, 1);
}

#[test]
/// 测试 `test_split_point2`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_split_point2` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_split_point2() {
    let ctx = Context::Background();
    let old_table_id: i64 = 50;
    let table_id: i64 = 100;
    let rewrite_rules = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(old_table_id),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(table_id),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut split_helper = NewSplitHelper();
    for (s, e, size) in [
        ("b", "c", 100u64),
        ("d", "e", 200),
        ("f", "i", 300),
        ("j", "k", 200),
        ("l", "n", 200),
    ] {
        split_helper.Merge(Valued {
            Key: Span {
                StartKey: key_with_table_prefix(old_table_id, s),
                EndKey: key_with_table_prefix(old_table_id, e),
            },
            Value: Value {
                Size: size,
                Number: size as i64,
            },
        });
    }
    let client = NewFakeSplitClient();
    client.AppendRegion(
        key_with_table_prefix(table_id, "a"),
        key_with_table_prefix(table_id, "g"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "g"),
        key_with_table_prefix(table_id, &get_char_from_number("g", 0)),
    );
    for i in 0..256 {
        client.AppendRegion(
            key_with_table_prefix(table_id, &get_char_from_number("g", i)),
            key_with_table_prefix(table_id, &get_char_from_number("g", i + 1)),
        );
    }
    client.AppendRegion(
        key_with_table_prefix(table_id, &get_char_from_number("g", 256)),
        key_with_table_prefix(table_id, "h"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "h"),
        key_with_table_prefix(table_id, "m"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "m"),
        key_with_table_prefix(table_id, "o"),
    );
    client.AppendRegion(
        key_with_table_prefix(table_id, "o"),
        key_with_table_prefix(table_id + 1, "a"),
    );

    let mut first_split = true;
    let iter = NewSplitHelperIterator(vec![NewRewriteSpliter(
        Vec::new(),
        table_id,
        rewrite_rules,
        split_helper,
    )]);
    SplitPoint(&ctx, &iter, &client, |_ctx, u, o, ri, valueds| {
        if first_split {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(u, 0);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(o, 0);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(
                ri.Region.as_ref().unwrap().StartKey,
                key_with_table_prefix(table_id, "a")
            );
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(
                ri.Region.as_ref().unwrap().EndKey,
                key_with_table_prefix(table_id, "g")
            );
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(valueds.len(), 3);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(valueds[2].Value.Size, 1);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(valueds[2].Value.Number, 1);
            first_split = false;
        } else {
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(u, 1);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(o, 1);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(
                ri.Region.as_ref().unwrap().StartKey,
                key_with_table_prefix(table_id, "h")
            );
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(
                ri.Region.as_ref().unwrap().EndKey,
                key_with_table_prefix(table_id, "m")
            );
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(valueds.len(), 2);
            // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
            assert_eq!(valueds[1].Value.Size, 100);
            assert_eq!(valueds[1].Value.Number, 100);
        }
        Ok(())
    })
    .unwrap();
}

#[test]
/// 测试 `test_regions_not_fully_scatter`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_regions_not_fully_scatter` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_regions_not_fully_scatter() {
    let mock = NewMockPDClientForSplit();
    let mut client = NewClient(Box::new(mock.clone()), None, 100, 4, vec![]);
    client.ForceNeedScatter(true);
    let ctx = Context::Background();
    let regions = vec![
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                ..Default::default()
            }),
            ..Default::default()
        },
        RegionInfo {
            Region: Some(metapb::Region {
                Id: 2,
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    client.scatterRegions(&ctx, &regions).unwrap();
    assert_eq!(mock.scatter_regions_region_count(), 2);
    assert!(mock.scatter_region_count().is_empty());

    mock.set_scatter_finished_percentage(50);
    client.scatterRegions(&ctx, &regions).unwrap();
    assert_eq!(mock.scatter_regions_region_count(), 2 + 1);
    assert_eq!(mock.scatter_region_count(), HashMap::from([(1, 1), (2, 1)]));

    mock.set_scatter_each_fail_before(7);
    client.scatterRegions(&ctx, &regions).unwrap();
    assert_eq!(mock.scatter_regions_region_count(), 2 + 1 + 1);
    assert_eq!(
        mock.scatter_region_count(),
        HashMap::from([(1, 1 + 7), (2, 1 + 7)])
    );
}

#[test]
/// 测试 `test_check_region_epoch_helper`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
/// `test_check_region_epoch_helper` 场景补充：空结果、重试耗尽与上下文取消需分别覆盖。
fn test_check_region_epoch_helper() {
    let a = RegionInfo {
        Region: Some(metapb::Region {
            RegionEpoch: Some(crate::stubs::RegionEpoch {
                ConfVer: 1,
                Version: 2,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(CheckRegionEpoch(&a, &a));
}
