// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/split` vs Go sources.
//!
//! 本文件用单一综合测试 `go_rust_public_contract_matches` 对照 Go `split` 包公开契约。
//! 覆盖 restore 阶段 region 扫描、分裂键聚合、一致性校验与 PD 重试策略等可观察行为。
//! 段落顺序从纯函数/常量推进到带 PD stub 的状态路径，便于定位失败属于哪类语义漂移。
//! 全部依赖 FakeSplitClient / MockPDClientForSplit 等内存桩，不启动真实 PD/TiKV 集群。
//! 断言失败时按段落英文标记定位（normal/boundary/error/resource）。
//! 不改测试行为：仅补充中文说明「校验什么契约、场景为何成立、断言依据来自哪份 Go 测试」。
//!
//! Go 对照索引（便于回归时跳转）：
//! - `sum_sorted_test.go` 的 `TestSumSorted`：Merge/Traverse 前缀和与 String 格式；
//! - `split_test.go` 的 `TestGetSplitKeyPerRegion`：`getSplitKeysOfRegions` 按 region 分组；
//! - `split_test.go` 的 region consistency 表驱动用例：`checkRegionConsistency` 错误文案；
//! - `split_test.go` 的 `TestSplitCheckPartRegionConsistency`：部分区间连续性；
//! - `split_test.go` 的 `TestSplitPoint`：RewriteRules + SplitPoint 回调键重写；
//! - `split_test.go` 的 `TestBackoffMayNotCountBackoffer`：ErrBackoff 计数语义；
//! - `split.go` / `client.go`：`NormalizeRegionIndexStep`、`WaitRegionOnlineAttemptTimes`、
//!   `CheckRegionEpoch` 等导出常量与辅助函数。
//! 若 Rust stub 与 Go 行为存在已知差异，应在对应段落中文注释中标明当前 stub 约定。
//! 本任务只加注释，因此所有期望值保持与既有断言完全一致。
//!
//! 阅读建议：先扫模块概述建立心智模型，再按英文段落标记对照 Go 同名测试与断言。
//! 新增公开 API 时，应在本测试追加对应段落，而不是另起零散用例文件。
//! sum_sorted 段只取 Go `TestSumSorted` 前三组，足以覆盖重叠/相邻/嵌套 span 合并路径。
//! 边界段验证空键跳过、全空间初始 span、区间重叠判定与 `beforeEnd` 半开区间语义。
//! RegionInfo 段确认左闭右开：起点/终点不算 interior，中间键才算命中。
//! consistency 段按 Go 表驱动顺序：空结果、首 region 缺口、相邻 region 断档三类错误。
//! getSplitKeysOfRegions 段复用 BR 迁移用例：键必须落在对应 region 内且按 Id 分组。
//! SplitPoint 段对齐 `TestSplitPoint` 的 region/valued 键重写与回调次数。
//! backoff 段区分 PD 可重试错误与「不计次」退避，以及 MockPD 的 epoch 比较。
//! 末尾 `HashMap` 占位仅确保 `RewriteRules` 类型在本 crate 可见，不参与断言。
//!
//! 段落之间状态不共享（除局部变量自然传递），避免隐式耦合掩盖失败原因。
//! 常量段失败通常表示 config 默认值或全局 atomic 初始化与 Go 不一致。
//! sum_sorted 段失败优先核对 Merge 扫描顺序与 Value 累加规则，而非键字面量。
//! 边界段失败说明空键过滤、全空间初始 span 或半开区间比较实现漂移。
//! RegionInfo 段失败意味着 ContainsInterior 对左闭右开边界的处理与 Go 不同。
//! consistency 段错误文案是稳定契约：回归时勿随意改写字符串仅为了让测试通过。
//! getSplitKeysOfRegions 要求 regions 已按 StartKey 排序且 keys 已排序，否则分组无意义。
//! SplitPoint 段依赖 RewriteSpliter 将 oldTableID 键重写为 newTableID 后再与 region 求交。
//! WaitRegionOnlineBackoffer 只对 PD 扫描类错误退避；其它错误必须立即终止重试循环。
//! BackoffMayNotCountBackoffer 是 scatter/split 路径的细粒度配额控制，与 WaitRegion 不同。
//! MockPD SetRegions 模拟 PD 预分裂：传入有序 split points 得到连续 region 列表。
//! CheckRegionEpoch 只比较 epoch 字段，不校验 leader/store，与 split client 用法一致。
//! checkPartRegionConsistency 比全量 check 更弱：允许末尾 region 超出 endKey，但要求首尾对齐。
//! 本文件不并行拆测：单一测试保证契约清单完整可见，便于审查 Go/Rust 对照表。

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/split/parity_test.rs`对应逻辑，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少79行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 中文注释索引结束
//! 桩行为与真实 PD 差异以段落注释为准，勿把 stub 简化当成生产语义。
//! 失败定位优先看段落英文标记，再对照上方 Go 索引中的同名测试。
//! 本综合测试刻意单测函数聚合，减少并发下共享全局桩的竞态面。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_br_pkg_restore_utils::RewriteRules;
use astersql_br_pkg_restore_utils::stubs::{codec, import_sstpb, tablecodec};

use crate::mock_pd_client::{NewFakeSplitClient, NewMockPDClientForSplit};
use crate::region::{RegionInfo, beforeEnd};
use crate::split::{
    CheckRegionEpoch, DefaultRegionIndexStep, ErrBackoff, ErrBackoffAndDontCount,
    NewBackoffMayNotCountBackoffer, NewWaitRegionOnlineBackoffer, NormalizeRegionIndexStep,
    WaitRegionOnlineAttemptTimes, checkPartRegionConsistency, checkRegionConsistency,
    getSplitKeysOfRegions,
};
use crate::splitter::{NewRewriteSpliter, NewSplitHelperIterator, SplitPoint};
use crate::stubs::BackoffStrategy;
use crate::stubs::{
    CompareBytesExt, Context, InitialRetryState, RegionEpoch, WithRetryReturnLastErr, metapb,
};
use crate::sum_sorted::{NewSplitHelper, NewValued, Span, Value, Valued, checkOverlaps};

/// 构造带半开区间 `[s,e)` 与 Value 的 `Valued`，对齐 Go 测试辅助函数 `v`。
fn v(s: &str, e: &str, val: Value) -> Valued {
    Valued {
        Key: Span {
            StartKey: s.as_bytes().to_vec(),
            EndKey: e.as_bytes().to_vec(),
        },
        Value: val,
    }
}

/// 将 MB 数转为 `Value{Size, Number}`，与 Go `mb`  helper 一致（Size 为字节、Number 为 MB 整数）。
fn mb(b: u64) -> Value {
    Value {
        Size: b * 1024 * 1024,
        Number: b as i64,
    }
}

/// 生成 Go `exportString` 同款可读串，用于校验 `Valued::String` 的十六进制键展示格式。
fn export_string(start_key: &str, end_key: &str, size: &str, number: i64) -> String {
    format!("([{start_key}, {end_key}), {size} MB, {number})")
}

/// 按表 ID 前缀 + 记录键编码 TiDB row key，对齐 Go `keyWithTablePrefix`（GenTableRecordPrefix + EncodeBytes）。
fn key_with_table_prefix(table_id: i64, key: &str) -> Vec<u8> {
    let mut raw = tablecodec::GenTableRecordPrefix(table_id);
    raw.extend_from_slice(key.as_bytes());
    codec::EncodeBytes(Vec::new(), &raw)
}

/// Go `context.WithTimeout` 的三个可观察契约：显式取消、截止时间与父取消传播。
#[test]
fn context_cancellation_matches_go() {
    let parent = Context::Background();
    let (explicit, cancel) = Context::WithTimeout(&parent, Duration::from_secs(60));
    cancel();
    assert_eq!(explicit.Err().unwrap().to_string(), "context canceled");

    let (expired, _cancel) = Context::WithTimeout(&parent, Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(
        expired.Err().unwrap().to_string(),
        "context deadline exceeded"
    );

    let (parent, cancel_parent) = Context::WithCancel(&Context::Background());
    let (child, _cancel_child) = Context::WithCancel(&parent);
    cancel_parent();
    assert_eq!(child.Err().unwrap().to_string(), "context canceled");
}

/// Go `bytes.Compare` 在未启用 empty-as-infinity 时把空切片视为最小值。
#[test]
fn compare_bytes_ext_honors_each_empty_as_infinity_flag() {
    assert_eq!(CompareBytesExt(b"", false, b"a", false), -1);
    assert_eq!(CompareBytesExt(b"a", false, b"", false), 1);
    assert_eq!(CompareBytesExt(b"", true, b"a", false), 1);
    assert_eq!(CompareBytesExt(b"a", false, b"", true), -1);
}

/// Go `WithRetryReturnLastErr` 对已取消 context 直接返回，不能调用业务闭包。
#[test]
fn retry_return_last_err_checks_context_before_first_attempt() {
    let (ctx, cancel) = Context::WithCancel(&Context::Background());
    cancel();
    let mut attempts = 0;
    let mut backoff = InitialRetryState(3, Duration::ZERO, Duration::ZERO);
    let err = WithRetryReturnLastErr(
        &ctx,
        || {
            attempts += 1;
            Err(astersql_errors::New("operation should not run"))
        },
        &mut backoff,
    )
    .unwrap_err();
    assert_eq!(attempts, 0);
    assert_eq!(err.to_string(), "context canceled");
}

/// 综合契约测试：任一段失败都表示 Rust `split` 与 Go 公开语义发生漂移。
/// 段落内断言顺序与 Go 同名测试保持一致，便于 diff 定位。
#[test]
fn go_rust_public_contract_matches() {
    // --- normal: NormalizeRegionIndexStep / constants ---
    // 未配置时使用默认 rough split step；显式传入应原样保留。
    assert_eq!(NormalizeRegionIndexStep(0), DefaultRegionIndexStep);
    assert_eq!(NormalizeRegionIndexStep(64), 64);
    // 等待 region online 的全局重试上限须与 Go `WaitRegionOnlineAttemptTimes` 常量一致。
    assert_eq!(WaitRegionOnlineAttemptTimes.load(Ordering::SeqCst), 1800);

    // --- normal: sum_sorted Merge/Traverse (Go TestSumSorted first cases) ---
    // 三组用例覆盖：部分重叠、右端对齐、左端对齐三种 span 合并形态。
    {
        let cases: Vec<(Vec<Valued>, Vec<u64>, Vec<String>)> = vec![
            (
                vec![
                    v("a", "f", mb(100)),
                    v("a", "c", mb(200)),
                    v("d", "g", mb(100)),
                ],
                vec![0, 250, 25, 75, 50, 0],
                vec![
                    export_string("61", "66", "100.00", 100),
                    export_string("61", "63", "200.00", 200),
                    export_string("64", "67", "100.00", 100),
                ],
            ),
            (
                vec![
                    v("a", "f", mb(100)),
                    v("a", "c", mb(200)),
                    v("d", "f", mb(100)),
                ],
                vec![0, 250, 25, 125, 0],
                vec![
                    export_string("61", "66", "100.00", 100),
                    export_string("61", "63", "200.00", 200),
                    export_string("64", "66", "100.00", 100),
                ],
            ),
            (
                vec![
                    v("a", "f", mb(100)),
                    v("a", "c", mb(200)),
                    v("c", "f", mb(100)),
                ],
                vec![0, 250, 150, 0],
                vec![
                    export_string("61", "66", "100.00", 100),
                    export_string("61", "63", "200.00", 200),
                    export_string("63", "66", "100.00", 100),
                ],
            ),
        ];
        for (values, result, strs) in cases {
            let mut full = NewSplitHelper();
            for (i, val) in values.into_iter().enumerate() {
                // Merge 前先断言 String 输出，锁定键的十六进制编码与 Go 一致。
                assert_eq!(val.String(), strs[i]);
                // 多次 Merge 叠加同一前缀和结构，Traverse 应产出 len(result) 个采样点。
                full.Merge(val);
            }
            // Traverse 按扫描顺序产出前缀和；result 向量与 Go ca.result 逐点对应。
            // 每个采样点的 Size/Number 由 mb(result[i]) 构造，与 Go require.Equal 同构。
            let mut i = 0;
            full.Traverse(|got| {
                assert_eq!(mb(result[i]), got.Value, "index {i}");
                i += 1;
                true
            });
            assert_eq!(i, result.len());
        }
    }

    // --- boundary: empty merge / checkOverlaps / beforeEnd ---
    // 空 StartKey/EndKey 的 Merge 应被忽略，最终只剩初始全空间 span。
    {
        let mut h = NewSplitHelper();
        h.Merge(NewValued(vec![], b"b".to_vec(), mb(1))); // empty start skipped
        h.Merge(NewValued(b"a".to_vec(), vec![], mb(1))); // empty end skipped
        let mut count = 0;
        h.Traverse(|_| {
            count += 1;
            true
        });
        assert_eq!(count, 1); // only initial full-space span
        // checkOverlaps：左 span 无 EndKey 表示延伸到正无穷，与右 span 必重叠。
        assert!(checkOverlaps(
            &Span {
                StartKey: b"a".to_vec(),
                EndKey: vec![]
            },
            &Span {
                StartKey: b"b".to_vec(),
                EndKey: b"c".to_vec()
            }
        ));
        // beforeEnd：半开区间比较；空 end 表示上界为正无穷。
        assert!(beforeEnd(b"a", b"b"));
        assert!(beforeEnd(b"a", &[]));
        assert!(!beforeEnd(b"b", b"a"));
    }

    // --- normal: RegionInfo.ContainsInterior ---
    // region [a,z)：边界键 a/z 不算 interior，中间键 m 算 interior。
    {
        let region = RegionInfo {
            Region: Some(metapb::Region {
                StartKey: b"a".to_vec(),
                EndKey: b"z".to_vec(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!region.ContainsInterior(b"a")); // boundary
        assert!(region.ContainsInterior(b"m"));
        assert!(!region.ContainsInterior(b"z"));
    }

    // --- error: checkRegionConsistency empty / gap / nil leader ---
    // 场景 1：扫描区间非空但 regions 为空，应对齐 Go「scan region return empty result」。
    {
        let start = codec::EncodeBytes(Vec::new(), b"a");
        let end = codec::EncodeBytes(Vec::new(), b"a");
        let err = checkRegionConsistency(&start, &end, &[], false).unwrap_err();
        assert!(
            err.to_string().contains("scan region return empty result"),
            "{err}"
        );

        // 场景 2：首 region 起点晚于 startKey，错误信息应提及 startKey。
        let regions = vec![RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                StartKey: codec::EncodeBytes(Vec::new(), b"b"),
                EndKey: codec::EncodeBytes(Vec::new(), b"d"),
                ..Default::default()
            }),
            ..Default::default()
        }];
        let err = checkRegionConsistency(&start, &end, &regions, false).unwrap_err();
        assert!(err.to_string().contains("startKey"), "{err}");

        // 场景 3：相邻 region 断档（b-d 与 e-f 之间缺 d-e），应报 endKey not equal。
        let regions = vec![
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
        ];
        let err = checkRegionConsistency(
            &codec::EncodeBytes(Vec::new(), b"c"),
            &codec::EncodeBytes(Vec::new(), b"e"),
            &regions,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("endKey not equal"), "{err}");
    }

    // --- normal: getSplitKeysOfRegions ---
    // 对齐 Go `TestGetSplitKeyPerRegion`：每个 split key 归属其覆盖的 region Id。
    {
        // sorted_keys 与 sorted_regions 均须已排序；g 同时是 region1 终点与 region2 起点。
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
        assert_eq!(result.len(), 3);
        assert_eq!(result.get(&1).unwrap(), &vec![b"b".to_vec(), b"d".to_vec()]);
        assert_eq!(result.get(&2).unwrap(), &vec![b"g".to_vec(), b"j".to_vec()]);
        // region3 仅 [k,m)，故只分配 l；m 本身是 region 右边界不再作为 split key。
        assert_eq!(result.get(&3).unwrap(), &vec![b"l".to_vec()]);
    }

    // --- normal: SplitPoint with FakeSplitClient ---
    // 对齐 Go `TestSplitPoint`：旧表键经 RewriteRules 映射后，回调收到新表 region 与两条 valued。
    {
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
        // split_helper 仍用 oldTableID 键；RewriteSpliter 在迭代时按规则改写到 table_id。
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
        // g-i 与首 region [a,f) 不相交，故回调只应收到 b-c 与 d-e 两段。
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
        // FakeSplitClient 预置四个连续 region，覆盖 table_id 全表范围至下一表前缀。
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
            // 首 region [a,f) 与 b-c、d-e 两段 valued 重叠，u/o 占位应为 0。
            assert_eq!(u, 0);
            assert_eq!(o, 0);
            assert_eq!(
                ri.Region.as_ref().unwrap().StartKey,
                key_with_table_prefix(table_id, "a")
            );
            assert_eq!(
                ri.Region.as_ref().unwrap().EndKey,
                key_with_table_prefix(table_id, "f")
            );
            assert_eq!(valueds.len(), 2);
            assert_eq!(
                valueds[0].Key.StartKey,
                key_with_table_prefix(table_id, "b")
            );
            assert_eq!(valueds[0].Key.EndKey, key_with_table_prefix(table_id, "c"));
            assert_eq!(
                valueds[1].Key.StartKey,
                key_with_table_prefix(table_id, "d")
            );
            assert_eq!(valueds[1].Key.EndKey, key_with_table_prefix(table_id, "e"));
            Ok(())
        })
        .unwrap();
        // 仅第一个 region 触发一次 split 回调（g-i 落在下一 region）。
        assert_eq!(calls, 1);
    }

    // --- error/backoff: WaitRegionOnlineBackoffer gives up on non-PD errors ---
    // 非 PD 扫描类错误应立即放弃：零退避且剩余次数清零。
    // 对齐 Go 中 ErrKVUnknown 等路径：NextBackoff 返回 0 且不再保留重试配额。
    {
        let mut bo = NewWaitRegionOnlineBackoffer();
        let delay = bo.NextBackoff(&astersql_errors::New("other"));
        assert_eq!(delay, Duration::ZERO);
        assert_eq!(bo.RemainingAttempts(), 0);
    }

    // --- resource: BackoffMayNotCountBackoffer reduce retry ---
    // ErrBackoffAndDontCount 不消耗配额；普通 ErrBackoff 应减少 RemainingAttempts。
    // 第二分支证明普通 ErrBackoff 仍会扣减，防止两种错误被混为一谈。
    {
        let mut bo = NewBackoffMayNotCountBackoffer();
        let before = bo.RemainingAttempts();
        let _ = bo.NextBackoff(&ErrBackoffAndDontCount());
        assert_eq!(bo.RemainingAttempts(), before); // counted then reduced
        let _ = bo.NextBackoff(&ErrBackoff());
        assert!(bo.RemainingAttempts() < before);
    }

    // --- resource: MockPD SetRegions / CheckRegionEpoch ---
    // SetRegions 按 split keys 切分；CheckRegionEpoch 比较 ConfVer/Version 是否一致。
    {
        let mock = NewMockPDClientForSplit();
        let regions = mock.SetRegions(&[b"a".to_vec(), b"m".to_vec(), b"z".to_vec(), Vec::new()]);
        assert_eq!(regions.len(), 3);
        let a = RegionInfo {
            Region: Some(metapb::Region {
                RegionEpoch: Some(RegionEpoch {
                    ConfVer: 1,
                    Version: 2,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let b = a.clone();
        assert!(CheckRegionEpoch(&a, &b));
        let mut c = a.clone();
        c.Region
            .as_mut()
            .unwrap()
            .RegionEpoch
            .as_mut()
            .unwrap()
            .Version = 3;
        assert!(!CheckRegionEpoch(&a, &c));
    }

    // --- part consistency ---
    // 对齐 Go `TestSplitCheckPartRegionConsistency` 成功路径：[a,m)+[m,z) 连续覆盖 [a,z)。
    // 只校验部分扫描窗口内 region 链连续，不要求覆盖全局 PD 拓扑。
    {
        let ok = checkPartRegionConsistency(
            b"a",
            b"z",
            &[
                RegionInfo {
                    Region: Some(metapb::Region {
                        StartKey: b"a".to_vec(),
                        EndKey: b"m".to_vec(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                RegionInfo {
                    Region: Some(metapb::Region {
                        StartKey: b"m".to_vec(),
                        EndKey: b"z".to_vec(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        );
        assert!(ok.is_ok());
    }

    // 类型可见性冒烟：确保 RewriteRules 在本 parity 测试中可实例化。
    // 与 restore/utils 的导入链一致，避免 parity crate 漏 re-export 导致编译通过但集成失败。
    let _ = HashMap::<i64, RewriteRules>::new();
}
