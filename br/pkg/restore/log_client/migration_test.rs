// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go `migration_test.go` equivalents — pure migration filter + ingested SST filtering.
//!
//! 本文件对齐 Go `migration_test.go`，覆盖 migration 过滤与 ingested SST 筛选：
//! - 辅助函数用确定性 ID（gfl/lfl）构造 meta/物理/逻辑三层数据；
//! - `test_migrations` 验证 EditMeta 删除 + 粗时间窗过滤后的迭代结果；
//! - `test_filter_out` 验证 compaction Artifacts 目录的粗过滤保留集；
//! - `test_retain_latest_mvcc_compaction_coverage` 校验分片/空洞/配置门槛；
//! - 后半组测试用 MemStorage 写入 extbackupmeta，验证 unfinished / 分段 / 时间窗外过滤。
//! 夹具只解释意图与断言依据，不改任何测试行为或期望值。
//!
//! 数据流（过滤侧）：`NewMigrationBuilder` → `Build` → `Metas/Physicals/Logicals`，
//! 断言通过 Length/StoreId 编码反查身份，避免依赖真实路径字符串比较。
//! 数据流（ingested SST）：`pef` 写 JSON → `AddIngestedSSTs` → `IngestedSSTsFiltered`，
//! 按 BackupUuid 分组的 Finished/AsIfTs 决定整组去留。
//! Go 对照点：粗过滤窗口、DestructSelf 优先、分片覆盖算法、未完成分组丢弃。
//! 本文件不启动真实对象存储；MemStorage 仅模拟元数据文件读写边界。
//! 若某断言失败，优先核对：窗口是否让 EditMeta 生效、期望矩阵空槽是否与过滤层一致、
//! ingested 分组的 Finished/AsIfTs 是否被 SetStartTS/SetRestoredTS 正确夹逼。
//! retain-latest-mvcc 失败用例依赖注释字段，而非 CompactionFrom/Until  alone。
//! 辅助函数保持纯函数：无全局可变状态，可在多 case 间重复调用。
//! check_* 系列失败时打印迭代 Err，便于区分「过滤结果不对」与「迭代器故障」。
//! 与 export_test 夹具（NewMigrationBuilder/NewMetaName）协作，不直接构造私有字段。
//! 密度目标服务于审查可读性：注释解释「为何如此期望」，而非复述断言 API。

use astersql_br_pkg_utils_iter::{CollectAll, Enumerate, FromSlice, Map, TryNextor};

use crate::export_test::{IngestedSSTRec, NewMetaName, NewMigrationBuilder};
use crate::log_file_manager::{FileIndex, FileIndexIter, MetaNameIter};
use crate::migration::{MetaWithMigrations, PhysicalWithMigrations, WithMigrations};
use crate::stubs::Context;
use crate::stubs::backuppb::{
    DataFileGroup, DataFileInfo, DeleteFilesInPhysical, LogFileCompaction, MetaEdit, Metadata,
    Migration, Span,
};
use crate::stubs::storeapi::{MemStorage, Storage};

// ---------------------------------------------------------------------------
// 夹具约定（与 Go 对齐）：
// 1) meta/物理/逻辑三层 ID 用算术编码，断言只看编码后的 Length/StoreId；
// 2) generate_meta_name_iter 固定产出 meta_0/1/2，各 3 物理 × 3 逻辑；
// 3) NewMigrationBuilder(shift,start,restored) 的粗过滤下界是 shiftStartTS；
// 4) ingested SST 测试依赖同 UUID 多段 JSON，Finished/AsIfTs 决定整组去留；
// 5) 期望矩阵中的空 Vec 表示「该层已被过滤，不会出现在迭代中」。
// ---------------------------------------------------------------------------

// 命名夹具：与 Go nameFromID / phyNameFromID 保持同一字符串形态，便于对照期望。
fn name_from_id(prefix: &str, id: u64) -> String {
    format!("{prefix}_{id}")
}
// 物理文件路径编码 meta 与物理序号，供 DeletePhysicalFiles / DataFileInfo.Path 共用。
fn phy_name_from_id(metaid: u64, phy_len: u64) -> String {
    format!("meta_{metaid}_phy_{phy_len}")
}
// 物理组 Length 编码：store*1e5 + glen*100，断言时用 Length 反查身份。
fn gfl(store_id: u64, length: u64) -> u64 {
    store_id * 100000 + length * 100
}
// 逻辑文件 Length/Offset 编码：再叠加 plen，保证三层 ID 互不冲突。
fn lfl(store_id: u64, glen: u64, plen: u64) -> u64 {
    store_id * 100000 + glen * 100 + plen
}
// 将「按 store 的物理序号矩阵」展开为期望的 PhysicalLength 列表；空 store 跳过。
fn gfls(matrix: Vec<Vec<u64>>) -> Vec<Vec<u64>> {
    let mut out = Vec::new();
    for (store_id, groups) in matrix.into_iter().enumerate() {
        // 空 store 对应 meta 被 DestructSelf 后不再出现在迭代中。
        if groups.is_empty() {
            continue;
        }
        // 将逻辑物理序号映射为 gfl 编码，与 generate_group_files 的 Length 对齐。
        out.push(
            groups
                .into_iter()
                .map(|glen| gfl(store_id as u64, glen))
                .collect(),
        );
    }
    out
}
// 三层矩阵 → 期望逻辑 Length；空 store/空组跳过，对齐过滤后迭代形状。
fn lfls(matrix: Vec<Vec<Vec<u64>>>) -> Vec<Vec<Vec<u64>>> {
    let mut out = Vec::new();
    for (store_id, groups) in matrix.into_iter().enumerate() {
        // 空向量表示该 store 的 meta 已被整份跳过。
        if groups.is_empty() {
            continue;
        }
        let mut group_out = Vec::new();
        for (glen, files) in groups.into_iter().enumerate() {
            // 空物理组表示该物理文件被 skipPhysical 删除。
            if files.is_empty() {
                continue;
            }
            // flen 是逻辑序号，编码后应等于 DataFileInfo.Length。
            group_out.push(
                files
                    .into_iter()
                    .map(|flen| lfl(store_id as u64, glen as u64, flen))
                    .collect(),
            );
        }
        out.push(group_out);
    }
    out
}

// 构造 DeleteLogicalFiles 用的 Span：Offset 用 lfl，Length 固定 1。
fn generate_spans(metaid: u64, physical_length: u64, span_length: u64) -> Vec<Span> {
    // Offset 必须等于对应 DataFileInfo.RangeOffset，NeedSkip 才能命中。
    (0..span_length)
        .map(|i| Span {
            Offset: lfl(metaid, physical_length, i),
            // Length=1 仅占位；跳过判定只看 Offset。
            Length: 1,
        })
        .collect()
}
// 单个物理文件上删除前 logical_length 个 span。
fn generate_delete_logical_files(
    metaid: u64,
    physical_length: u64,
    logical_length: u64,
) -> Vec<DeleteFilesInPhysical> {
    // 单元素列表：Go 同样一次删除一个物理上的多 span。
    vec![DeleteFilesInPhysical {
        Path: phy_name_from_id(metaid, physical_length),
        Spans: generate_spans(metaid, physical_length, logical_length),
    }]
}
// 删除前 physical_length 个物理文件路径。
fn generate_delete_physical_files(metaid: u64, physical_length: u64) -> Vec<String> {
    // 路径形态必须与 generate_group_files 一致，否则 skipmap 键对不上。
    (0..physical_length)
        .map(|i| phy_name_from_id(metaid, i))
        .collect()
}
// DestructSelf=true：整份 meta 删除（对应 skipMeta）。
fn generate_migration_meta(metaid: u64) -> MetaEdit {
    // Path 用 meta_{id}，与 NewMetaName 第二参数一致。
    MetaEdit {
        Path: name_from_id("meta", metaid),
        DestructSelf: true,
        ..Default::default()
    }
}
// 混合删除：前 physical_length 个物理文件 + 指定物理上的逻辑 span。
fn generate_migration_file(
    metaid: u64,
    physical_length: u64,
    physical_offset: u64,
    logical_length: u64,
) -> MetaEdit {
    // physical_offset 指向「被删逻辑 span 所在物理文件」序号，可与物理删除列表重叠。
    MetaEdit {
        Path: name_from_id("meta", metaid),
        // 删除 [0, physical_length) 物理文件。
        DeletePhysicalFiles: generate_delete_physical_files(metaid, physical_length),
        // 在 physical_offset 物理文件上删除前 logical_length 个逻辑 span。
        DeleteLogicalFiles: generate_delete_logical_files(metaid, physical_offset, logical_length),
        // 非 DestructSelf：允许物理/逻辑细删共存。
        DestructSelf: false,
    }
}
// 每个物理组固定 3 个逻辑文件，RangeOffset/Length 均用 lfl 编码。
fn generate_data_files(meta_id: u64, glen: u64, plen: u64) -> Vec<DataFileInfo> {
    // Path 与物理组 Path 一致；RangeOffset/Length 同值便于断言。
    (0..plen)
        .map(|i| DataFileInfo {
            Path: phy_name_from_id(meta_id, glen),
            RangeOffset: lfl(meta_id, glen, i),
            Length: lfl(meta_id, glen, i),
            ..Default::default()
        })
        .collect()
}
// 生成 length 个物理组，供固定夹具 meta_0/1/2 使用。
fn generate_group_files(meta_id: u64, length: u64) -> Vec<DataFileGroup> {
    // 每组固定 3 个逻辑文件，与 case 期望矩阵宽度对齐。
    (0..length)
        .map(|i| DataFileGroup {
            Path: phy_name_from_id(meta_id, i),
            Length: gfl(meta_id, i),
            DataFilesInfo: generate_data_files(meta_id, i, 3),
            ..Default::default()
        })
        .collect()
}
// 固定 3 个 meta（StoreId 0/1/2，各 3 物理组），作为所有 case 的输入迭代源。
fn generate_meta_name_iter() -> MetaNameIter {
    FromSlice(vec![
        // meta_0：将被部分 case 的 DestructSelf 删除。
        NewMetaName(
            Metadata {
                StoreId: 0,
                FileGroups: generate_group_files(0, 3),
                ..Default::default()
            },
            name_from_id("meta", 0),
        ),
        // meta_1：通常不编辑，作「全保留」对照。
        NewMetaName(
            Metadata {
                StoreId: 1,
                FileGroups: generate_group_files(1, 3),
                ..Default::default()
            },
            name_from_id("meta", 1),
        ),
        // meta_2：常被删物理/逻辑，验证细粒度 skip。
        NewMetaName(
            Metadata {
                StoreId: 2,
                FileGroups: generate_group_files(2, 3),
                ..Default::default()
            },
            name_from_id("meta", 2),
        ),
    ])
}

// 收集 MetaWithMigrations 的 StoreId，与期望 store 列表比对。
fn check_meta_name_iter(expect: &[i64], actual: Box<dyn TryNextor<MetaWithMigrations>>) {
    // StoreId 与 meta 下标一致（0/1/2），便于阅读期望列表。
    let mut mapped = Map(actual, |m: MetaWithMigrations| m.StoreId());
    let res = CollectAll(
        &astersql_br_pkg_utils_iter::Context::background(),
        &mut *mapped,
    );
    // 迭代本身不应失败；失败说明夹具或 skipmap 构造异常。
    assert!(res.Err.is_none(), "{:?}", res.Err);
    // 顺序与 FromSlice 输入顺序一致（过滤后保持相对序）。
    assert_eq!(expect, res.Item.unwrap_or_default().as_slice());
}

// 收集物理层 Length（即 gfl 编码），验证 skipPhysical 效果。
fn check_physical_iter(expect: &[u64], actual: Box<dyn TryNextor<PhysicalWithMigrations>>) {
    // PhysicalLength 即 DataFileGroup.Length（gfl），不是文件字节数。
    let mut mapped = Map(actual, |p: PhysicalWithMigrations| p.PhysicalLength());
    let res = CollectAll(
        &astersql_br_pkg_utils_iter::Context::background(),
        &mut *mapped,
    );
    assert!(res.Err.is_none(), "{:?}", res.Err);
    // 被 skipPhysical 删除的物理组不应出现。
    assert_eq!(expect, res.Item.unwrap_or_default().as_slice());
}

// 收集逻辑文件 Length（lfl 编码），验证 skipLogical 效果。
fn check_logical_iter(expect: &[u64], actual: FileIndexIter) {
    // 用 Length（lfl）而非 Path，避免字符串比较掩盖 offset 过滤错误。
    let mut mapped = Map(actual, |l: FileIndex| l.Item.Length);
    let res = CollectAll(
        &astersql_br_pkg_utils_iter::Context::background(),
        &mut *mapped,
    );
    assert!(res.Err.is_none(), "{:?}", res.Err);
    // skipLogical 按 RangeOffset 剔除后，剩余 Length 序列须匹配。
    assert_eq!(expect, res.Item.unwrap_or_default().as_slice());
}

// 从 meta 展开 Physicals 迭代；Enumerate 提供 GroupIndex。
fn generate_physical_iter(meta: &MetaWithMigrations) -> Box<dyn TryNextor<PhysicalWithMigrations>> {
    // FileGroups 克隆后 Enumerate，模拟生产路径的 GroupIndex 输入。
    let group_iter = FromSlice(meta.Meta().FileGroups.clone());
    let group_index_iter = Enumerate(group_iter);
    // Physicals 内部按 skipmap 决定 FilterOut 或下传逻辑 skipmap。
    meta.Physicals(group_index_iter)
}

// 从物理组展开 Logicals 迭代。
fn generate_logical_iter(phy: &PhysicalWithMigrations) -> FileIndexIter {
    // DataFilesInfo 同样经 Enumerate 得到 FileIndex。
    let file_iter = FromSlice(phy.Physical().DataFilesInfo.clone());
    let file_index_iter = Enumerate(file_iter);
    // Logicals 按 RangeOffset 查逻辑 skipmap。
    phy.Logicals(file_index_iter)
}

/// Go `TestMigrations`.
///
/// 用 builder(shift=10,start=100,restored=200) 构建跳过表，再对固定 3-meta 迭代
/// 校验 store / 物理 Length / 逻辑 Length。关键点：粗过滤窗外的 EditMeta 不生效。
#[test]
fn test_migrations() {
    // 每 case：migrations 输入 + 三层期望（storeId / phy Length / log Length）。
    struct Case {
        migrations: Vec<Migration>,
        expect_store_ids: Vec<i64>,
        expect_phy_lengths: Vec<Vec<u64>>,
        expect_log_lengths: Vec<Vec<Vec<u64>>>,
    }
    let cases = vec![
        // case0：compaction [1,9] 完全在窗前 → 粗过滤丢弃整条，删除不生效，三层全保留。
        Case {
            migrations: vec![Migration {
                EditMeta: vec![
                    generate_migration_meta(0),
                    generate_migration_file(2, 1, 2, 2),
                ],
                Compactions: vec![LogFileCompaction {
                    InputMinTs: 1,
                    InputMaxTs: 9,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            expect_store_ids: vec![0, 1, 2],
            expect_phy_lengths: gfls(vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]]),
            expect_log_lengths: lfls(vec![
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
            ]),
        },
        // case1：compaction [50,52] 落窗内 → meta0 DestructSelf；meta2 删 phy0 + phy2 上两逻辑 span。
        Case {
            migrations: vec![Migration {
                EditMeta: vec![
                    generate_migration_meta(0),
                    generate_migration_file(2, 1, 2, 2),
                ],
                Compactions: vec![LogFileCompaction {
                    InputMinTs: 50,
                    InputMaxTs: 52,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            expect_store_ids: vec![1, 2],
            expect_phy_lengths: gfls(vec![vec![], vec![0, 1, 2], vec![1, 2]]),
            expect_log_lengths: lfls(vec![
                vec![],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
                vec![vec![], vec![0, 1, 2], vec![2]],
            ]),
        },
        // case2：两条 migration 均在窗内，效果与 case1 合并相同。
        Case {
            migrations: vec![
                Migration {
                    EditMeta: vec![generate_migration_meta(0)],
                    Compactions: vec![LogFileCompaction {
                        InputMinTs: 50,
                        InputMaxTs: 52,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Migration {
                    EditMeta: vec![generate_migration_file(2, 1, 2, 2)],
                    Compactions: vec![LogFileCompaction {
                        InputMinTs: 120,
                        InputMaxTs: 140,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            expect_store_ids: vec![1, 2],
            expect_phy_lengths: gfls(vec![vec![], vec![0, 1, 2], vec![1, 2]]),
            expect_log_lengths: lfls(vec![
                vec![],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
                vec![vec![], vec![0, 1, 2], vec![2]],
            ]),
        },
        // case3：第二条 compaction [1200,1400] 在窗外 → 其 EditMeta 被丢弃，仅 meta0 删除生效。
        Case {
            migrations: vec![
                Migration {
                    EditMeta: vec![generate_migration_meta(0)],
                    Compactions: vec![LogFileCompaction {
                        InputMinTs: 50,
                        InputMaxTs: 52,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Migration {
                    EditMeta: vec![generate_migration_file(2, 1, 2, 2)],
                    Compactions: vec![LogFileCompaction {
                        InputMinTs: 1200,
                        InputMaxTs: 1400,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            expect_store_ids: vec![1, 2],
            expect_phy_lengths: gfls(vec![vec![], vec![0, 1, 2], vec![0, 1, 2]]),
            expect_log_lengths: lfls(vec![
                vec![],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
                vec![vec![0, 1, 2], vec![0, 1, 2], vec![0, 1, 2]],
            ]),
        },
    ];

    let ictx = astersql_br_pkg_utils_iter::Context::background();
    for (i, cs) in cases.into_iter().enumerate() {
        // shiftStartTS=10,startTS=100,restoredTS=200：与 Go TestMigrations 窗口一致。
        let builder = NewMigrationBuilder(10, 100, 200);
        // Build 合并所有未粗过滤 migration 的 EditMeta / Artifacts。
        let with_migrations = builder.Build(&cs.migrations);
        // 第一遍：只校验 meta 层 StoreId 序列。
        let it = with_migrations.Metas(generate_meta_name_iter());
        check_meta_name_iter(&cs.expect_store_ids, it);
        // 第二遍：物化 metas，再逐层下钻（迭代器单次消费）。
        let mut it2 = with_migrations.Metas(generate_meta_name_iter());
        let collect = CollectAll(&ictx, &mut *it2);
        assert!(collect.Err.is_none(), "case {i}: {:?}", collect.Err);
        let metas = collect.Item.unwrap_or_default();
        // 对每个幸存 meta 再下钻物理/逻辑层，形状必须与期望矩阵一致。
        for (j, meta) in metas.iter().enumerate() {
            // 先用独立迭代做物理断言。
            let physical_iter = generate_physical_iter(meta);
            check_physical_iter(&cs.expect_phy_lengths[j], physical_iter);
            // 再收集物理列表以展开逻辑层。
            let mut physical_iter = generate_physical_iter(meta);
            let collect = CollectAll(&ictx, &mut *physical_iter);
            assert!(collect.Err.is_none());
            let phys = collect.Item.unwrap_or_default();
            for (k, phy) in phys.iter().enumerate() {
                let logical_iter = generate_logical_iter(phy);
                check_logical_iter(&cs.expect_log_lengths[j][k], logical_iter);
            }
        }
    }
}

/// Go `TestFilterOut`.
///
/// 只关心 Build 后 CompactionDirs 集合：粗过滤应丢掉完全越界的 Artifacts，
/// InputMin/Max=0 的 compaction 视为「区间无效」而不被粗过滤剔除。
#[test]
fn test_filter_out() {
    // 仅填 Input 区间与 Artifacts 名，用于粗过滤判定。
    fn simple(i_min: u64, i_max: u64, name: &str) -> LogFileCompaction {
        LogFileCompaction {
            InputMinTs: i_min,
            InputMaxTs: i_max,
            Artifacts: name.into(),
            ..Default::default()
        }
    }
    // 额外带 CompactionFrom/Until，验证细字段不影响粗过滤目录收集。
    fn with_compact(
        i_min: u64,
        i_max: u64,
        c_from: u64,
        c_until: u64,
        name: &str,
    ) -> LogFileCompaction {
        LogFileCompaction {
            InputMinTs: i_min,
            InputMaxTs: i_max,
            CompactionFromTs: c_from,
            CompactionUntilTs: c_until,
            Artifacts: name.into(),
            ..Default::default()
        }
    }
    // (shift, restored, migrations, 期望保留的 Artifacts 名)。
    let cases: Vec<(u64, u64, Vec<Migration>, Vec<&str>)> = vec![
        // [50,60]：a 重叠保留，b 完全在窗外丢弃。
        (
            50,
            60,
            vec![
                Migration {
                    Compactions: vec![simple(49, 61, "a")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![simple(61, 80, "b")],
                    ..Default::default()
                },
            ],
            vec!["a"],
        ),
        // [30,50]：1b 完全在窗前丢弃；其余与窗有交集保留。
        (
            30,
            50,
            vec![
                Migration {
                    Compactions: vec![simple(40, 60, "1a")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![simple(10, 20, "1b")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![simple(31, 50, "2a")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![simple(50, 80, "2b")],
                    ..Default::default()
                },
            ],
            vec!["1a", "2a", "2b"],
        ),
        // with_compact：c 的 Input 完全在窗前 → 丢弃；a/b 保留。
        (
            30,
            50,
            vec![
                Migration {
                    Compactions: vec![with_compact(49, 100, 50, 99, "a")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![with_compact(10, 30, 15, 29, "b")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![with_compact(8, 29, 10, 20, "c")],
                    ..Default::default()
                },
            ],
            vec!["a", "b"],
        ),
        // InputMin/Max=0 视为无效区间，粗过滤不剔除 b/c。
        (
            100,
            120,
            vec![
                Migration {
                    Compactions: vec![with_compact(49, 100, 50, 99, "a")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![with_compact(0, 0, 15, 29, "b")],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![with_compact(0, 0, 10, 20, "c")],
                    ..Default::default()
                },
            ],
            vec!["a", "b", "c"],
        ),
    ];
    for (i, (shift, restored, migs, expect)) in cases.into_iter().enumerate() {
        // startTS=shift，与 Go 用例一致。
        let b = NewMigrationBuilder(shift, shift, restored);
        let built = b.Build(&migs);
        // CompactionDirs 顺序不稳定，排序后比较集合。
        let mut dirs = built.CompactionDirs();
        dirs.sort();
        let mut exp: Vec<String> = expect.iter().map(|s| (*s).to_string()).collect();
        exp.sort();
        // 名称即 Artifacts 字段，代表保留的 compaction 产物目录。
        assert_eq!(dirs, exp, "case {i}");
    }
}

#[test]
fn compactions_loads_storage_artifacts_and_filters_by_input_ts() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    strg
        .WriteFile(
            &ctx,
            "compact/0001.meta",
            br#"{"Subcompactions":[{"Meta":{"TableId":1,"InputMinTs":20,"InputMaxTs":30},"SstOutputs":[]},{"Meta":{"TableId":2,"InputMinTs":5,"InputMaxTs":9},"SstOutputs":[]}]}"#,
        )
        .unwrap();
    let wm = WithMigrations {
        skipmap: Default::default(),
        compactionDirs: vec!["compact".into()],
        fullBackups: vec![],
        shiftStartTS: 10,
        startTS: 10,
        restoredTS: 40,
    };

    let mut iter = wm.Compactions(&ctx, &strg);
    let collected = CollectAll(
        &astersql_br_pkg_utils_iter::Context::background(),
        iter.as_mut(),
    );
    assert_eq!(collected.Err, None);
    let items = collected.Item.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].Meta.TableId, 1);
}

#[test]
fn compactions_propagates_artifact_decode_errors() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    strg.WriteFile(&ctx, "compact/broken.meta", b"not-json")
        .unwrap();
    let wm = WithMigrations {
        skipmap: Default::default(),
        compactionDirs: vec!["compact".into()],
        fullBackups: vec![],
        shiftStartTS: 10,
        startTS: 10,
        restoredTS: 40,
    };

    let mut iter = wm.Compactions(&ctx, &strg);
    let result = iter.TryNext(&astersql_br_pkg_utils_iter::Context::background());
    assert!(
        result
            .Err
            .unwrap()
            .contains("failed to decode subcompactions")
    );
}

/// Go `TestRetainLatestMVCCCompactionCoverage`.
///
/// 窗口固定 [100,200]：成功案须完整覆盖且分片齐全；失败案覆盖空洞、缺分片、
/// minimal≠0、未开 cal-shift-ts。错误文案须含 retain-latest-mvcc-version。
#[test]
fn test_retain_latest_mvcc_compaction_coverage() {
    // 生成 compact-log-backup 注释 JSON；shard_total<=1 时写 null。
    fn comment(
        from: u64,
        until: u64,
        shard_index: u64,
        shard_total: u64,
        minimal: u64,
        cal_shift: bool,
    ) -> String {
        // total>1 时写入 shard 对象；否则 null（解析后视为 1/1）。
        let shard = if shard_total > 1 {
            format!(r#","shard":{{"index":{shard_index},"total":{shard_total}}}"#)
        } else {
            r#","shard":null"#.to_string()
        };
        format!(
            r#"{{"config":{{"from-ts":{from},"until-ts":{until},"cal-shift-ts":{cal_shift},"minimal-compaction-size":{minimal}{shard}}}}}"#
        )
    }
    // 把注释挂到 LogFileCompaction，供解析路径读取。
    fn compaction(
        from: u64,
        until: u64,
        shard_index: u64,
        shard_total: u64,
        minimal: u64,
        cal_shift: bool,
    ) -> LogFileCompaction {
        LogFileCompaction {
            CompactionFromTs: from,
            CompactionUntilTs: until,
            Comments: comment(from, until, shard_index, shard_total, minimal, cal_shift),
            ..Default::default()
        }
    }
    // (名称, migrations, want_err)。
    let cases: Vec<(&str, Vec<Migration>, bool)> = vec![
        // 单分片完整覆盖 [100,200]。
        (
            "unsharded complete",
            vec![Migration {
                Compactions: vec![compaction(100, 200, 1, 1, 0, true)],
                ..Default::default()
            }],
            false,
        ),
        // 2 分片均到齐。
        (
            "sharded complete",
            vec![Migration {
                Compactions: vec![
                    compaction(100, 200, 1, 2, 0, true),
                    compaction(100, 200, 2, 2, 0, true),
                ],
                ..Default::default()
            }],
            false,
        ),
        // 分段拼接：前半 1/1 + 后半 2 分片齐全。
        (
            "segmented complete",
            vec![
                Migration {
                    Compactions: vec![compaction(100, 150, 1, 1, 0, true)],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![
                        compaction(150, 200, 1, 2, 0, true),
                        compaction(150, 200, 2, 2, 0, true),
                    ],
                    ..Default::default()
                },
            ],
            false,
        ),
        // 150→151 空洞，覆盖失败。
        (
            "ts gap",
            vec![
                Migration {
                    Compactions: vec![compaction(100, 150, 1, 1, 0, true)],
                    ..Default::default()
                },
                Migration {
                    Compactions: vec![compaction(151, 200, 1, 1, 0, true)],
                    ..Default::default()
                },
            ],
            true,
        ),
        // 仅有 1/2 分片。
        (
            "incomplete shard",
            vec![Migration {
                Compactions: vec![compaction(100, 200, 1, 2, 0, true)],
                ..Default::default()
            }],
            true,
        ),
        // minimal≠0 → 不参与覆盖 → 失败。
        (
            "minimal compaction size is not zero",
            vec![Migration {
                Compactions: vec![compaction(100, 200, 1, 1, 1, true)],
                ..Default::default()
            }],
            true,
        ),
        // 未开 cal-shift-ts → 不参与覆盖 → 失败。
        (
            "cal shift ts is not enabled",
            vec![Migration {
                Compactions: vec![compaction(100, 200, 1, 1, 0, false)],
                ..Default::default()
            }],
            true,
        ),
    ];
    for (name, migs, want_err) in cases {
        // 与覆盖窗口对齐：start=100, restored=200。
        let builder = NewMigrationBuilder(100, 100, 200);
        let err = builder.ValidateRetainLatestMVCCCompactionCoverage(&migs);
        if want_err {
            let e = err.expect_err(name);
            // 失败路径必须带业务关键字，防止误匹配其它 InvalidArgument。
            assert!(
                e.to_string().contains("retain-latest-mvcc-version"),
                "{name}: {e}"
            );
        } else {
            // 成功路径：Ok(())，名称仅用于定位失败 case。
            err.expect(name);
        }
    }
}

// 将 IngestedSSTRec 写成 extbackupmeta_XXXXXXXX，返回路径供 AddIngestedSSTs。
fn pef(s: &dyn Storage, fb: &IngestedSSTRec, sn: i32) -> String {
    // 路径序号左填充 8 位，对齐 Go 外部备份元数据命名。
    let path = format!("extbackupmeta_{sn:08}");
    let bs = serde_json::to_vec(fb).unwrap();
    // 写入失败会直接 unwrap；夹具阶段不应失败。
    s.WriteFile(&Context::Background(), &path, &bs).unwrap();
    path
}

// 按 FilesPrefixHint 排序后比对，忽略返回顺序。
fn assert_full_backup_pfxs(got: &[IngestedSSTRec], expect: &[&str]) {
    // 只比对 prefix hint，不比对 UUID/TS，聚焦过滤集合正确性。
    let mut act: Vec<_> = got.iter().map(|i| i.FilesPrefixHint.clone()).collect();
    let mut exp: Vec<_> = expect.iter().map(|s| (*s).to_string()).collect();
    act.sort();
    exp.sort();
    assert_eq!(act, exp);
}

// 用重复字节构造 16B UUID，区分不同 BackupUuid 分组。
fn uuid_bytes(n: u8) -> Vec<u8> {
    vec![n; 16]
}

// 空 WithMigrations；测试中再 AddIngestedSSTs / Set*TS。
fn new_wm() -> WithMigrations {
    // 零时间窗默认；各测试按需 SetStartTS/SetRestoredTS。
    WithMigrations {
        skipmap: Default::default(),
        compactionDirs: vec![],
        fullBackups: vec![],
        shiftStartTS: 0,
        startTS: 0,
        restoredTS: 0,
    }
}

/// Go `TestNotRestoreIncomplete`.
///
/// Finished=false 的分组即使 AsIfTs 落窗也应整组丢弃。
#[test]
fn test_not_restore_incomplete() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    // 单段未完成：GroupFinished=false。
    let ebk = IngestedSSTRec {
        FilesPrefixHint: "001".into(),
        AsIfTs: 90,
        BackupUuid: uuid_bytes(1),
        Finished: false,
    };
    let mut wm = new_wm();
    wm.AddIngestedSSTs(pef(&strg, &ebk, 0));
    // restoredTS 足以覆盖 AsIfTs，但仍因未完成而丢弃。
    wm.SetRestoredTS(91);
    let got = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap();
    // 未完成分组：期望空结果。
    assert!(got.is_empty());
}

/// Go `TestRestoreSegmented`.
///
/// 同 UUID 分段：前段 Finished=false，末段 Finished=true 且 AsIfTs 落窗 → 两段都恢复。
#[test]
fn test_restore_segmented() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    // 共享 UUID：模拟一次外部备份的多段元数据。
    let id = uuid_bytes(2);
    // 分段前半：尚未完成。
    let ebk1 = IngestedSSTRec {
        FilesPrefixHint: "001".into(),
        BackupUuid: id.clone(),
        Finished: false,
        AsIfTs: 0,
    };
    // 分段末段：完成且时间戳在 restoredTS 内。
    let ebk2 = IngestedSSTRec {
        FilesPrefixHint: "002".into(),
        AsIfTs: 90,
        Finished: true,
        BackupUuid: id,
    };
    let mut wm = new_wm();
    wm.AddIngestedSSTs(pef(&strg, &ebk1, 0));
    wm.AddIngestedSSTs(pef(&strg, &ebk2, 1));
    // GroupFinished 由末段 Finished=true 决定；GroupTS 取末段 AsIfTs。
    wm.SetRestoredTS(91);
    let got = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap();
    // 前半虽 Finished=false，但同组完成后两段 prefix 都应出现。
    assert_full_backup_pfxs(&got, &["001", "002"]);
}

/// Go `TestFilteredOut`.
///
/// restoredTS=89 / startTS=42：完成组 AsIfTs=90 超上界，AsIfTs=10 低于下界，均过滤。
#[test]
fn test_filtered_out() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    let id = uuid_bytes(3);
    let mut wm = new_wm();
    // 未完成段：与完成段同 UUID，整组 AsIfTs 由完成段决定。
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "001".into(),
            BackupUuid: id.clone(),
            ..Default::default()
        },
        0,
    ));
    // 完成但 AsIfTs=90 > restoredTS=89。
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "002".into(),
            AsIfTs: 90,
            Finished: true,
            BackupUuid: id,
        },
        1,
    ));
    // 另一 UUID，AsIfTs=10 < startTS=42。
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "003".into(),
            AsIfTs: 10,
            Finished: true,
            BackupUuid: uuid_bytes(9),
        },
        2,
    ));
    // 双端收紧：上界 89、下界 42。
    wm.SetRestoredTS(89);
    wm.SetStartTS(42);
    let got = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap();
    // 两组均越界 → 空。
    assert!(got.is_empty());
}

/// Go `TestMultiRestores`.
///
/// 两组 UUID 均完成且落窗 → 四个 prefix 全部保留。
#[test]
fn test_multi_restores() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    // id：001(未完成段) + 002(完成 AsIfTs=90)
    let id = uuid_bytes(4);
    // id2：101(未完成段) + 102(完成 AsIfTs=88)
    let id2 = uuid_bytes(5);
    let mut wm = new_wm();
    // 注意 Add 顺序与 sn 编号不必连续，过滤按 UUID 聚合。
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "001".into(),
            BackupUuid: id.clone(),
            ..Default::default()
        },
        0,
    ));
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "101".into(),
            BackupUuid: id2.clone(),
            ..Default::default()
        },
        2,
    ));
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "102".into(),
            AsIfTs: 88,
            Finished: true,
            BackupUuid: id2,
        },
        3,
    ));
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "002".into(),
            AsIfTs: 90,
            Finished: true,
            BackupUuid: id,
        },
        4,
    ));
    // restoredTS=91 覆盖 88 与 90。
    wm.SetRestoredTS(91);
    let got = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap();
    // 排序后集合应含两组全部 prefix。
    assert_full_backup_pfxs(&got, &["101", "102", "001", "002"]);
}

/// Go `TestMultiFilteredOutOne`.
///
/// restoredTS=89：id 组 AsIfTs=90 超界整组丢弃；id2 组 AsIfTs=88 保留。
#[test]
fn test_multi_filtered_out_one() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    // 与 multi_restores 相同结构，仅收紧 restoredTS。
    let id = uuid_bytes(6);
    let id2 = uuid_bytes(7);
    let mut wm = new_wm();
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "001".into(),
            BackupUuid: id.clone(),
            ..Default::default()
        },
        0,
    ));
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "101".into(),
            BackupUuid: id2.clone(),
            ..Default::default()
        },
        2,
    ));
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "102".into(),
            AsIfTs: 88,
            Finished: true,
            BackupUuid: id2,
        },
        3,
    ));
    // 该完成段将整组 id 推到 AsIfTs=90，超出 restoredTS=89。
    wm.AddIngestedSSTs(pef(
        &strg,
        &IngestedSSTRec {
            FilesPrefixHint: "002".into(),
            AsIfTs: 90,
            Finished: true,
            BackupUuid: id,
        },
        4,
    ));
    // 89 < 90 → id 整组过滤；88 ≤ 89 → id2 保留。
    wm.SetRestoredTS(89);
    let got = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap();
    assert_full_backup_pfxs(&got, &["101", "102"]);
}

/// Go `TestError` — failpoint path; Mem load error injected via malformed JSON.
///
/// 写入非法 JSON，期望 IngestedSSTsFiltered 返回非空错误（对齐 Go failpoint 错误路径）。
#[test]
fn test_error() {
    let ctx = Context::Background();
    let strg = MemStorage::new();
    let path = "extbackupmeta_00000000";
    // 非法 JSON：触发解析/加载错误。
    strg.WriteFile(&ctx, path, b"not-json").unwrap();
    let mut wm = new_wm();
    // 路径加入 fullBackups，过滤时强制读取该坏文件。
    wm.AddIngestedSSTs(path.into());
    wm.SetRestoredTS(91);
    // 与 Go failpoint 类似：错误必须向上返回，不能静默跳过。
    let err = wm.IngestedSSTsFiltered(&ctx, &strg).unwrap_err();
    assert!(!err.to_string().is_empty());
}
