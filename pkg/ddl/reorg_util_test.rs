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

// DDL 重组（reorg）工具相关的单元测试。
//
// 覆盖通过 PD（Placement Driver，负责 Region 调度与元信息）的 Region
// 近似统计估算表体积与平均行大小的逻辑：对每个 Region 取
// `approximate_size` 与 `approximate_kv_size` 的较大值再累加，
// 以贴近 TiDB reorg 回填（backfill）前的容量评估行为。

use crate::reorg_util::{
    InitializedReorgMeta, RegionSize, ReorgMetaError, ReorgVariables,
    estimate_table_size_by_regions, get_table_size_by_id, init_job_reorg_meta_from_variables,
};

/// 一兆字节（MiB）对应的字节数，用于把 PD 返回的 MiB 近似值换算成字节。
const MIB: i64 = 1024 * 1024;

/// 模拟单个 Region（键空间分片）的近似统计信息。
#[derive(Clone, Debug, Default)]
struct RegionInfo {
    id: u64,
    approximate_size_mib: i64,
    approximate_kv_size_mib: i64,
    approximate_keys: i64,
}

/// 一次 PD 分页查询返回的 Region 列表。
#[derive(Clone, Debug, Default)]
struct RegionsInfo {
    regions: Vec<RegionInfo>,
}

/// 键范围 `[start_key, end_key)`，对应表在 KV 层的扫描区间。
#[derive(Clone, Debug, PartialEq, Eq)]
struct KeyRange {
    start_key: Vec<u8>,
    end_key: Vec<u8>,
}

/// Mock PD 客户端：按预设分页返回 Region，并记录首次查询参数供断言。
#[derive(Default)]
struct MockPdClient {
    pages: Vec<RegionsInfo>,
    call_count: usize,
    first_range: Option<KeyRange>,
    first_limit: usize,
}

impl MockPdClient {
    /// 按键范围分页拉取 Region；空页表示分页结束。
    fn get_regions_by_key_range(&mut self, key_range: &KeyRange, limit: usize) -> RegionsInfo {
        // 仅记录第一次调用的参数，用于校验估算函数传入的范围与 limit。
        if self.call_count == 0 {
            self.first_range = Some(key_range.clone());
            self.first_limit = limit;
        }
        let result = self.pages.get(self.call_count).cloned().unwrap_or_default();
        self.call_count += 1;
        result
    }
}

/// 构造表 `table_id` 对应的行数据键范围（前缀 `k:t{id}_r`）。
fn expected_region_range(table_id: i64) -> KeyRange {
    let mut start_key = b"k:t".to_vec();
    start_key.extend_from_slice(&table_id.to_be_bytes());
    start_key.extend_from_slice(b"_r");
    let mut end_key = start_key.clone();
    end_key.push(0xff);
    KeyRange { start_key, end_key }
}

/// 分页累加各 Region 的 max(近似大小, 近似 KV 大小)，得到表体积估算（字节）。
fn estimate_table_size_by_id(client: &mut MockPdClient, table_id: i64) -> i64 {
    let key_range = expected_region_range(table_id);
    let mut total_mib = 0;
    // 以固定 page size 128 翻页，直到 PD 返回空页。
    loop {
        let page = client.get_regions_by_key_range(&key_range, 128);
        if page.regions.is_empty() {
            break;
        }
        total_mib += page
            .regions
            .iter()
            .map(|region| {
                region
                    .approximate_size_mib
                    .max(region.approximate_kv_size_mib)
            })
            .sum::<i64>();
    }
    total_mib * MIB
}

/// 从少量采样 Region 估算平均行大小：max(大小) * MiB / keys。
fn estimate_row_size_from_region(client: &mut MockPdClient, table_id: i64) -> i64 {
    // 只拉取前 3 个 Region 作为样本，与生产估算侧采样策略一致。
    let regions = client.get_regions_by_key_range(&expected_region_range(table_id), 3);
    let sample = regions
        .regions
        .iter()
        .find(|region| region.id != 0 && region.approximate_keys > 0)
        .expect("a sample region is required");
    sample
        .approximate_size_mib
        .max(sample.approximate_kv_size_mib)
        * MIB
        / sample.approximate_keys
}

/// 验证表体积估算对每个 Region 取两种近似大小的较大值后再累加。
#[test]
fn test_estimate_table_size_by_id_uses_max_approximate_sizes() {
    let mut client = MockPdClient {
        pages: vec![
            RegionsInfo {
                regions: vec![
                    RegionInfo {
                        id: 1,
                        approximate_size_mib: 5,
                        approximate_kv_size_mib: 64,
                        ..RegionInfo::default()
                    },
                    RegionInfo {
                        id: 2,
                        approximate_size_mib: 16,
                        approximate_kv_size_mib: 7,
                        ..RegionInfo::default()
                    },
                    RegionInfo {
                        id: 3,
                        approximate_size_mib: 0,
                        approximate_kv_size_mib: 9,
                        ..RegionInfo::default()
                    },
                ],
            },
            RegionsInfo::default(),
        ],
        ..MockPdClient::default()
    };

    assert_eq!(89 * MIB, estimate_table_size_by_id(&mut client, 42));
    assert_eq!(2, client.call_count);
    assert_eq!(128, client.first_limit);
    assert_eq!(Some(expected_region_range(42)), client.first_range);
}

/// 验证行大小估算同样取 max(近似大小, 近似 KV 大小) 再除以 keys。
#[test]
fn test_estimate_row_size_from_region_uses_max_approximate_sizes() {
    let table_id = 1024;
    // (approximate_size, approximate_kv_size, keys, 期望平均行字节数)
    let cases = [(4, 10, 2, 5 * MIB), (12, 3, 3, 4 * MIB), (9, 0, 3, 3 * MIB)];
    for (size_mib, kv_size_mib, keys, expected) in cases {
        let mut client = MockPdClient {
            pages: vec![RegionsInfo {
                regions: vec![RegionInfo {
                    id: 2,
                    approximate_size_mib: size_mib,
                    approximate_kv_size_mib: kv_size_mib,
                    approximate_keys: keys,
                }],
            }],
            ..MockPdClient::default()
        };
        assert_eq!(
            expected,
            estimate_row_size_from_region(&mut client, table_id)
        );
        assert_eq!(1, client.call_count);
        assert_eq!(3, client.first_limit);
        assert_eq!(Some(expected_region_range(table_id)), client.first_range);
    }
}

#[test]
fn production_size_aggregation_preserves_go_signed_arithmetic() {
    assert_eq!(-3, get_table_size_by_id(&[5, -10, 2]));

    let pages = vec![vec![RegionSize {
        approximate_size_mib: i64::MAX,
        approximate_kv_size_mib: 0,
    }]];
    assert_eq!(
        i64::MAX.wrapping_mul(MIB),
        estimate_table_size_by_regions(&pages)
    );
}

#[test]
fn production_region_aggregation_uses_each_regions_larger_estimate() {
    let pages = vec![
        vec![
            RegionSize {
                approximate_size_mib: 5,
                approximate_kv_size_mib: 64,
            },
            RegionSize {
                approximate_size_mib: 16,
                approximate_kv_size_mib: 7,
            },
        ],
        vec![RegionSize {
            approximate_size_mib: 0,
            approximate_kv_size_mib: 9,
        }],
    ];
    assert_eq!(89 * MIB, estimate_table_size_by_regions(&pages));
}

#[test]
fn reorg_meta_initialization_validates_and_snapshots_every_variable() {
    assert_eq!(
        Err(ReorgMetaError::InvalidConcurrency),
        init_job_reorg_meta_from_variables(&ReorgVariables {
            worker_count: 0,
            ..ReorgVariables::default()
        })
    );
    assert_eq!(
        Err(ReorgMetaError::InvalidBatchSize),
        init_job_reorg_meta_from_variables(&ReorgVariables {
            batch_size: 0,
            ..ReorgVariables::default()
        })
    );
    assert_eq!(
        Err(ReorgMetaError::CloudStorageUriMissing),
        init_job_reorg_meta_from_variables(&ReorgVariables {
            use_cloud_storage: true,
            ..ReorgVariables::default()
        })
    );

    let variables = ReorgVariables {
        worker_count: 8,
        batch_size: 512,
        max_write_speed: 4096,
        use_cloud_storage: true,
        cloud_storage_uri: "s3://bucket/prefix".into(),
        distributed: true,
    };
    assert_eq!(
        Ok(InitializedReorgMeta {
            concurrency: 8,
            batch_size: 512,
            max_write_speed: 4096,
            cloud_storage_uri: "s3://bucket/prefix".into(),
            distributed: true,
            version: 1,
        }),
        init_job_reorg_meta_from_variables(&variables)
    );

    let local = init_job_reorg_meta_from_variables(&ReorgVariables {
        cloud_storage_uri: "must-not-leak".into(),
        ..ReorgVariables::default()
    })
    .unwrap();
    assert!(local.cloud_storage_uri.is_empty());
}
