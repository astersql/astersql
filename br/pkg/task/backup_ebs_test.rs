// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/task/backup_ebs_test.go`.
//!
//! 对齐 Go `TestIsRegionsHasHole`：用表驱动覆盖连续覆盖、空洞与异常多终点。
//! 空 StartKey/EndKey 表示键空间两端（与 TiKV region 约定一致）。
//! 被测函数会原地按 StartKey 排序，用例无需预排序。

use crate::backup_ebs::{
    DefineBackupEBSFlags, flagBackupVolumeFile, flagProgressFile, getMockSleepTimeFromEnv,
    isRegionsHasHole,
};
use crate::common::{
    defaultCloudAPIConcurrency, flagCloudAPIConcurrency, flagFullBackupType,
    flagOperatorPausedGCAndSchedulers, flagSkipAWS,
};
use crate::stubs::FlagSet;
use crate::stubs::metapb::Region;
use std::time::Duration;

#[test]
fn test_define_backup_ebs_flags_matches_go_defaults() {
    let mut flags = FlagSet::new();
    DefineBackupEBSFlags(&mut flags);

    assert_eq!(flags.GetString(flagFullBackupType).unwrap(), "kv");
    assert_eq!(
        flags.GetString(flagBackupVolumeFile).unwrap(),
        "./backup.json"
    );
    assert!(!flags.GetBool(flagSkipAWS).unwrap());
    assert_eq!(
        flags.GetUint(flagCloudAPIConcurrency).unwrap(),
        defaultCloudAPIConcurrency
    );
    assert_eq!(flags.GetString(flagProgressFile).unwrap(), "progress.txt");
    assert!(!flags.GetBool(flagOperatorPausedGCAndSchedulers).unwrap());
}

#[test]
fn test_get_mock_sleep_time_matches_go_fallbacks() {
    assert_eq!(getMockSleepTimeFromEnv(None), Duration::from_millis(800));
    assert_eq!(
        getMockSleepTimeFromEnv(Some("not-a-duration")),
        Duration::from_millis(800)
    );
    assert_eq!(
        getMockSleepTimeFromEnv(Some("25ms")),
        Duration::from_millis(25)
    );
    assert_eq!(getMockSleepTimeFromEnv(Some("2s")), Duration::from_secs(2));
    assert_eq!(
        getMockSleepTimeFromEnv(Some("1m500ms")),
        Duration::from_millis(60_500)
    );
    assert_eq!(getMockSleepTimeFromEnv(Some("0")), Duration::ZERO);
    assert_eq!(getMockSleepTimeFromEnv(Some("-1s")), Duration::ZERO);
}

// 测试草稿：仅关心起止键，其它 Region 字段用 Default。
struct RegionDraft {
    // 区间左闭端点；空切片表示最小键。
    start_key: &'static [u8],
    // 区间右开端点；空切片表示最大键（无穷尾）。
    end_key: &'static [u8],
}

// 单用例：名称便于失败定位，want 为是否检测到空洞。
struct IsRegionsHasHoleCase {
    // 与 Go 表项 Name 对应，出现在 assert 消息中。
    name: &'static str,
    all_regions: Vec<RegionDraft>,
    // true=存在空洞或非法边界；false=键空间连续覆盖。
    want: bool,
}

/// Corresponds to Go `TestIsRegionsHasHole`.
/// 表驱动：无空洞场景 want=false；间隙/重叠边界 want=true。
#[test]
fn test_is_regions_has_hole() {
    // 用例顺序与 Go 测试表保持一致，便于逐项 diff。
    let tests = vec![
        // 单 region 覆盖全键空间：无相邻对可比较 → false。
        IsRegionsHasHoleCase {
            name: "one region",
            all_regions: vec![RegionDraft {
                start_key: b"",
                end_key: b"",
            }],
            want: false,
        },
        // 两段在 "a" 处首尾相接。
        // 左段 EndKey 等于右段 StartKey，判定无洞。
        IsRegionsHasHoleCase {
            name: "2 region",
            all_regions: vec![
                RegionDraft {
                    start_key: b"",
                    end_key: b"a",
                },
                RegionDraft {
                    start_key: b"a",
                    end_key: b"",
                },
            ],
            want: false,
        },
        // 多段链连续：""→a→c→f→g→""。
        // 覆盖中间多跳，防止只测两端相接的假阴性。
        IsRegionsHasHoleCase {
            name: "many regions",
            all_regions: vec![
                RegionDraft {
                    start_key: b"",
                    end_key: b"a",
                },
                RegionDraft {
                    start_key: b"a",
                    end_key: b"c",
                },
                RegionDraft {
                    start_key: b"c",
                    end_key: b"f",
                },
                RegionDraft {
                    start_key: b"f",
                    end_key: b"g",
                },
                RegionDraft {
                    start_key: b"g",
                    end_key: b"",
                },
            ],
            // 全链相接，快照键空间完整。
            want: false,
        },
        // f 的下一期望 StartKey=f，实际为 e → 空洞/错乱边界。
        // 排序后 c..f 与 e..g 的 End/Start 不相等。
        IsRegionsHasHoleCase {
            name: "region hole 1",
            all_regions: vec![
                RegionDraft {
                    start_key: b"",
                    end_key: b"a",
                },
                RegionDraft {
                    start_key: b"a",
                    end_key: b"c",
                },
                RegionDraft {
                    start_key: b"c",
                    end_key: b"f",
                },
                // 故意从 e 起，制造与上一 EndKey=f 的间隙。
                RegionDraft {
                    start_key: b"e",
                    end_key: b"g",
                },
                RegionDraft {
                    start_key: b"g",
                    end_key: b"",
                },
            ],
            want: true,
        },
        // 中段 EndKey 已是 ""（全尾），后面仍有 region → 异常多终点。
        // Go 注释标明正常集群不应出现，仍需检出为 hole。
        IsRegionsHasHoleCase {
            name: "multiple end keys(should not happen normally)",
            all_regions: vec![
                RegionDraft {
                    start_key: b"",
                    end_key: b"a",
                },
                RegionDraft {
                    start_key: b"a",
                    end_key: b"c",
                },
                // 过早以空 EndKey 收尾。
                RegionDraft {
                    start_key: b"c",
                    end_key: b"",
                },
                RegionDraft {
                    start_key: b"e",
                    end_key: b"g",
                },
                RegionDraft {
                    start_key: b"g",
                    end_key: b"",
                },
            ],
            // 空 EndKey 后仍有后续 StartKey，必然不相等。
            want: true,
        },
        // 经典空洞：""..a 与 b.."" 之间缺 a..b。
        // 最小复现两段间隙，断言依据最直观。
        IsRegionsHasHoleCase {
            name: "region hole",
            all_regions: vec![
                RegionDraft {
                    start_key: b"",
                    end_key: b"a",
                },
                RegionDraft {
                    start_key: b"b",
                    end_key: b"",
                },
            ],
            want: true,
        },
    ];

    for tt in tests {
        // 转为 stubs Region；isRegionsHasHole 会原地排序，故用 mut。
        let mut regions: Vec<Region> = tt
            .all_regions
            .iter()
            .map(|r| Region {
                StartKey: r.start_key.to_vec(),
                EndKey: r.end_key.to_vec(),
                ..Default::default()
            })
            .collect();
        // 调用与 Go isRegionsHasHole 同名语义函数。
        let got = isRegionsHasHole(&mut regions);
        // 失败信息带 case name，便于与 Go 表项对照。
        assert_eq!(got, tt.want, "isRegionsHasHole() case {}", tt.name);
    }
}
