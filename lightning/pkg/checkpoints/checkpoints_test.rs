// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Go-equivalent unit tests for `lightning/pkg/checkpoints/checkpoints_test.go`
//! (package `checkpoints`).
//!
//! 这组测试与 Go 版本逐项对齐，主要覆盖 checkpoint 增量合并、
//! 基值回写、文件持久化恢复以及路径拆分等辅助逻辑。
//! 这里验证的是测试夹具中的纯内存状态变化，而不是完整导入流程，
//! 因此断言会直接比对 `TableCheckpointDiff` 与 `TableCheckpoint` 的结构体内容。
//! 对迁移版 Rust 而言，这些测试同时承担“语义护栏”的作用：
//! 一旦某个 merger 改写了不该触碰的字段，或者恢复流程丢失了 Go 中依赖的空映射语义，
//! 就会在这里暴露为结构不一致。
//! 注释重点说明每个测试想保护的约束，以及为什么这些断言必须保持与 Go 一致。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::mydump;
use crate::storeapi::StorageHandle;
use crate::verify::MakeKVChecksum;
use crate::*;

/// 这个用例验证状态 merger 的作用域。
/// 当 `EngineID` 是普通引擎编号时，只应更新对应引擎的状态槽位。
/// 当 `EngineID` 是 `WholeTableEngineID` 时，才允许写入表级状态。
/// 测试分四步累积写入，故意混合普通引擎、索引引擎和整表引擎。
/// 断言不仅检查新状态值，还检查先前已经写入的其他引擎状态没有被覆盖。
/// 这与 Go 版本的语义一致：diff 负责记录“局部更新”，
/// 而不是在每次 merge 时重建整张表的 checkpoint 快照。
/// 这里顺带验证普通引擎和特殊引擎都共享同一种 merge 接口，
/// 但最终落点会依据 `EngineID` 分流到不同层级。
/// 最后一段再把 `-1` 引擎推进到 `AllWritten`，
/// 用来确认整表状态一旦被设置为 `Closed`，不会因为子引擎后续推进而被回退或抬升。
/// TestMergeStatusCheckpoint
#[test]
fn test_merge_status_checkpoint() {
    let mut cpd = NewTableCheckpointDiff();

    // 普通数据引擎首次写入时，会为该引擎创建独立 diff，并保留空 chunk 映射。
    let m = StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusImported,
    };
    m.MergeInto(&mut cpd);
    assert_eq!(
        cpd,
        TableCheckpointDiff {
            hasStatus: false,
            engines: HashMap::from([(
                0,
                engineCheckpointDiff {
                    hasStatus: true,
                    status: CheckpointStatusImported,
                    chunks: HashMap::new(),
                },
            )]),
            ..Default::default()
        }
    );

    // `-1` 代表与整表并列存在的特殊引擎槽位，不应误写到表级状态。
    let m = StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusLoaded,
    };
    m.MergeInto(&mut cpd);
    assert_eq!(
        cpd,
        TableCheckpointDiff {
            hasStatus: false,
            engines: HashMap::from([
                (
                    0,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusImported,
                        chunks: HashMap::new(),
                    },
                ),
                (
                    -1,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusLoaded,
                        chunks: HashMap::new(),
                    },
                ),
            ]),
            ..Default::default()
        }
    );

    // 只有整表引擎编号才允许改动 `TableCheckpointDiff.status`。
    let m = StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusClosed,
    };
    m.MergeInto(&mut cpd);
    assert_eq!(
        cpd,
        TableCheckpointDiff {
            hasStatus: true,
            status: CheckpointStatusClosed,
            engines: HashMap::from([
                (
                    0,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusImported,
                        chunks: HashMap::new(),
                    },
                ),
                (
                    -1,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusLoaded,
                        chunks: HashMap::new(),
                    },
                ),
            ]),
            ..Default::default()
        }
    );

    // 子引擎继续推进后，整表状态仍维持此前结果，避免 merge 过程中出现跨层级污染。
    let m = StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusAllWritten,
    };
    m.MergeInto(&mut cpd);
    assert_eq!(
        cpd,
        TableCheckpointDiff {
            hasStatus: true,
            status: CheckpointStatusClosed,
            engines: HashMap::from([
                (
                    0,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusImported,
                        chunks: HashMap::new(),
                    },
                ),
                (
                    -1,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusAllWritten,
                        chunks: HashMap::new(),
                    },
                ),
            ]),
            ..Default::default()
        }
    );
}

/// 这个用例覆盖“无效状态”的降级编码。
/// Go 实现中，`SetInvalid()` 会把状态缩小到原值的十分之一，
/// 以便在 checkpoint 中保留“曾推进到这里，但结果无效”的信息。
/// 这里先写入一个正常引擎状态，再让另一个引擎以 invalid 形式合并，
/// 用来确认两层状态都会使用降级值，而不是只在引擎层生效。
/// 如果 Rust 迁移时漏掉整表状态同步，这个断言会立刻失败。
/// TestMergeInvalidStatusCheckpoint
#[test]
fn test_merge_invalid_status_checkpoint() {
    let mut cpd = NewTableCheckpointDiff();

    StatusCheckpointMerger {
        EngineID: 0,
        Status: CheckpointStatusLoaded,
    }
    .MergeInto(&mut cpd);

    let mut m = StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusAllWritten,
    };
    m.SetInvalid();
    m.MergeInto(&mut cpd);

    assert_eq!(
        cpd,
        TableCheckpointDiff {
            hasStatus: true,
            status: CheckpointStatusAllWritten / 10,
            engines: HashMap::from([
                (
                    0,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusLoaded,
                        chunks: HashMap::new(),
                    },
                ),
                (
                    -1,
                    engineCheckpointDiff {
                        hasStatus: true,
                        status: CheckpointStatusAllWritten / 10,
                        chunks: HashMap::new(),
                    },
                ),
            ]),
            ..Default::default()
        }
    );
}

/// 这个用例验证 chunk merger 的覆盖语义。
/// chunk diff 以 `(Path, Offset)` 作为键，因此同一个文件块再次 merge 时，
/// 应该替换旧位置、行号和校验和，而不是累计出多个条目。
/// 这与导入恢复逻辑直接相关：重跑同一 chunk 时，
/// checkpoint 只应保留最后一次成功推进到的位置。
/// 测试先写入一组初始位点，再用相同 key 写入更新后的位点，
/// 最终断言映射里仍只有一条记录，但内容已经变成第二次 merge 的值。
/// TestMergeChunkCheckpoint
#[test]
fn test_merge_chunk_checkpoint() {
    let mut cpd = NewTableCheckpointDiff();
    // 选择固定 key，便于证明第二次 merge 是“覆盖同键”而不是“新增另一键”。
    let key = ChunkCheckpointKey {
        Path: "/tmp/path/1.sql".into(),
        Offset: 0,
    };

    ChunkCheckpointMerger {
        EngineID: 2,
        Key: key.clone(),
        Checksum: MakeKVChecksum(700, 15, 1234567890),
        Pos: 1055,
        RealPos: 1053,
        RowID: 31,
        ..Default::default()
    }
    .MergeInto(&mut cpd);

    assert_eq!(
        cpd,
        TableCheckpointDiff {
            engines: HashMap::from([(
                2,
                engineCheckpointDiff {
                    chunks: HashMap::from([(
                        key.clone(),
                        chunkCheckpointDiff {
                            pos: 1055,
                            realPos: 1053,
                            rowID: 31,
                            checksum: MakeKVChecksum(700, 15, 1234567890),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }
    );

    // 相同 key 的第二次写入必须覆盖旧位点，模拟恢复过程中 checkpoint 的前移。
    ChunkCheckpointMerger {
        EngineID: 2,
        Key: key.clone(),
        Checksum: MakeKVChecksum(800, 20, 1357924680),
        Pos: 1080,
        RealPos: 1070,
        RowID: 42,
        ..Default::default()
    }
    .MergeInto(&mut cpd);

    assert_eq!(
        cpd,
        TableCheckpointDiff {
            engines: HashMap::from([(
                2,
                engineCheckpointDiff {
                    chunks: HashMap::from([(
                        key,
                        chunkCheckpointDiff {
                            pos: 1080,
                            realPos: 1070,
                            rowID: 42,
                            checksum: MakeKVChecksum(800, 20, 1357924680),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }
    );
}

/// 这个用例保护自增基值的“只增不减”规则。
/// rebase diff 代表导入过程中观察到的更大基值，
/// 因而 apply/merge 时只能把 checkpoint 往前推进，不能回退到更小值。
/// 第一轮 merge 写入三个基值并形成期望快照。
/// 第二轮故意提供更小的数值，
/// 断言结果保持不变，证明 Rust 版本保留了 Go 中的单调性约束。
/// 若这里允许回退，恢复后生成的 row id 可能与已导入数据重叠。
/// TestRebaseCheckpoint
#[test]
fn test_rebase_checkpoint() {
    let mut cpd = NewTableCheckpointDiff();
    RebaseCheckpointMerger {
        AutoRandBase: 132861,
        AutoIncrBase: 132862,
        AutoRowIDBase: 132863,
    }
    .MergeInto(&mut cpd);

    let expected = TableCheckpointDiff {
        hasRebase: true,
        autoRandBase: 132861,
        autoIncrBase: 132862,
        autoRowIDBase: 132863,
        engines: HashMap::new(),
        ..Default::default()
    };
    assert_eq!(cpd, expected);

    // 第二次 merge 提供更小基值，预期被忽略而非覆盖。
    RebaseCheckpointMerger {
        AutoRandBase: 131,
        AutoIncrBase: 132,
        AutoRowIDBase: 133,
    }
    .MergeInto(&mut cpd);
    assert_eq!(cpd, expected);
}

/// 这个用例把多种 merger 组合起来，验证 `TableCheckpoint::Apply` 的整体效果。
/// 初始 checkpoint 只含两个引擎和两段 chunk，用来模拟导入中途的持久化状态。
/// 随后构造的 diff 同时包含：
/// 表级状态推进、
/// 特殊引擎状态推进、
/// 一个不存在引擎的状态写入、
/// 三种自增基值的 rebase、
/// 已有 chunk 的位点覆盖、
/// 不存在引擎上的 chunk 更新、
/// 以及不存在 chunk key 的写入。
/// 这组输入刻意覆盖“应生效”和“应忽略”两类分支。
/// Go 版本的关键约束是：
/// `Apply` 只更新已有引擎和已有 chunk，
/// 不能因为 diff 里出现额外键就把运行时结构扩展出新实体。
/// 因此最终断言里看不到 `1234`、`5678`、`/tmp/03.sql` 或 `/tmp/10.sql`，
/// 但已有的 `-1` 引擎、表级状态和 `/tmp/01.sql` chunk 会被正确推进。
/// 这个测试是整份文件里最接近真实恢复路径的案例，
/// 一旦 `Apply` 对过滤条件、覆盖顺序或基值更新规则处理错误，
/// 这里就会出现大面积结构体差异。
/// 也是因为断言对象很完整，它能同时证明“未命中的 diff 被忽略”这一负向语义。
/// TestApplyDiff
#[test]
fn test_apply_diff() {
    // 初始快照故意只放入一部分引擎和 chunk，便于验证“只更新已存在项”的策略。
    let mut cp = TableCheckpoint {
        Status: CheckpointStatusLoaded,
        AutoRandBase: 131,
        AutoIncrBase: 132,
        AutoRowIDBase: 133,
        Engines: HashMap::from([
            (
                -1,
                EngineCheckpoint {
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![],
                },
            ),
            (
                0,
                EngineCheckpoint {
                    Status: CheckpointStatusLoaded,
                    Chunks: vec![
                        ChunkCheckpoint {
                            Key: ChunkCheckpointKey {
                                Path: "/tmp/01.sql".into(),
                                Offset: 0,
                            },
                            Chunk: mydump::Chunk {
                                Offset: 0,
                                RealOffset: 0,
                                EndOffset: 20000,
                                PrevRowIDMax: 0,
                                RowIDMax: 1000,
                            },
                            ..Default::default()
                        },
                        ChunkCheckpoint {
                            Key: ChunkCheckpointKey {
                                Path: "/tmp/04.sql".into(),
                                Offset: 0,
                            },
                            Chunk: mydump::Chunk {
                                Offset: 0,
                                RealOffset: 0,
                                EndOffset: 15000,
                                PrevRowIDMax: 1000,
                                RowIDMax: 1300,
                            },
                            ..Default::default()
                        },
                    ],
                },
            ),
        ]),
        ..Default::default()
    };

    let mut cpd = NewTableCheckpointDiff();
    // 特殊引擎状态推进，应命中已存在的 `-1` 引擎。
    StatusCheckpointMerger {
        EngineID: -1,
        Status: CheckpointStatusImported,
    }
    .MergeInto(&mut cpd);
    // 整表状态推进，最终应覆盖 `cp.Status`。
    StatusCheckpointMerger {
        EngineID: WholeTableEngineID,
        Status: CheckpointStatusAllWritten,
    }
    .MergeInto(&mut cpd);
    // 不存在的引擎状态写入会留在 diff 中，但 apply 到现有 checkpoint 时应被忽略。
    StatusCheckpointMerger {
        EngineID: 1234,
        Status: CheckpointStatusAnalyzeSkipped,
    }
    .MergeInto(&mut cpd);
    // 三种基值统一前推，验证 apply 会同步更新表级元信息。
    RebaseCheckpointMerger {
        AutoRandBase: 1131,
        AutoIncrBase: 1132,
        AutoRowIDBase: 1133,
    }
    .MergeInto(&mut cpd);
    // 命中已有 chunk key，应覆盖位点与 checksum。
    ChunkCheckpointMerger {
        EngineID: 0,
        Key: ChunkCheckpointKey {
            Path: "/tmp/01.sql".into(),
            Offset: 0,
        },
        Checksum: MakeKVChecksum(3333, 4444, 5555),
        Pos: 6666,
        RealPos: 6565,
        RowID: 777,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    // 命中不存在的引擎，应证明 apply 不会凭空创建新引擎。
    ChunkCheckpointMerger {
        EngineID: 5678,
        Key: ChunkCheckpointKey {
            Path: "/tmp/04.sql".into(),
            Offset: 0,
        },
        Pos: 9999,
        RealPos: 9888,
        RowID: 888,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    // 命中已存在引擎但不存在的 chunk key，同样不应新增 chunk。
    ChunkCheckpointMerger {
        EngineID: 0,
        Key: ChunkCheckpointKey {
            Path: "/tmp/03.sql".into(),
            Offset: 0,
        },
        Pos: 3636,
        RealPos: 3535,
        RowID: 2222,
        ..Default::default()
    }
    .MergeInto(&mut cpd);
    // 再给同一引擎一个额外未知 key，覆盖“多个未知项同时存在”场景。
    ChunkCheckpointMerger {
        EngineID: 0,
        Key: ChunkCheckpointKey {
            Path: "/tmp/10.sql".into(),
            Offset: 0,
        },
        Pos: 4949,
        RealPos: 4848,
        RowID: 444,
        ..Default::default()
    }
    .MergeInto(&mut cpd);

    // 真正执行 apply 后，最终结构只应体现合法命中的那部分 diff。
    cp.Apply(&cpd);

    assert_eq!(
        cp,
        TableCheckpoint {
            Status: CheckpointStatusAllWritten,
            AutoRandBase: 1131,
            AutoIncrBase: 1132,
            AutoRowIDBase: 1133,
            Engines: HashMap::from([
                (
                    -1,
                    EngineCheckpoint {
                        Status: CheckpointStatusImported,
                        Chunks: vec![]
                    },
                ),
                (
                    0,
                    EngineCheckpoint {
                        Status: CheckpointStatusLoaded,
                        Chunks: vec![
                            ChunkCheckpoint {
                                Key: ChunkCheckpointKey {
                                    Path: "/tmp/01.sql".into(),
                                    Offset: 0
                                },
                                Chunk: mydump::Chunk {
                                    Offset: 6666,
                                    RealOffset: 6565,
                                    EndOffset: 20000,
                                    PrevRowIDMax: 777,
                                    RowIDMax: 1000,
                                },
                                Checksum: MakeKVChecksum(3333, 4444, 5555),
                                ..Default::default()
                            },
                            ChunkCheckpoint {
                                Key: ChunkCheckpointKey {
                                    Path: "/tmp/04.sql".into(),
                                    Offset: 0
                                },
                                Chunk: mydump::Chunk {
                                    Offset: 0,
                                    RealOffset: 0,
                                    EndOffset: 15000,
                                    PrevRowIDMax: 1000,
                                    RowIDMax: 1300,
                                },
                                ..Default::default()
                            },
                        ],
                    },
                ),
            ]),
            ..Default::default()
        }
    );
}

/// 这个用例验证文件 checkpoint 的序列化与反序列化不会丢失空映射。
/// Go 中空 map 与 nil map 的语义不同，恢复时若把空映射省略成 nil，
/// 后续代码可能因为直接写入或遍历而表现出不同分支。
/// Rust 版本虽然没有 nil map，但 protobuf/持久化恢复后仍要显式补齐空容器，
/// 才能保持与 Go 对照测试的结构语义一致。
/// 这里先写入一个含空 `Engines` 映射的模型，再重新打开文件，
/// 最终断言读取出的条目仍然可安全访问该映射字段。
/// TestCheckpointMarshallUnmarshall
#[test]
fn test_checkpoint_marshall_unmarshall() {
    // 使用临时目录隔离文件系统副作用，避免测试间互相污染。
    let dir = tempfile_dir();
    let path = dir.join("filecheckpoint");
    let path_str = path.to_string_lossy().to_string();
    let ctx = context::Background();

    let mut file_chkp = NewFileCheckpointsDB(ctx, &path_str).unwrap();
    file_chkp.checkpoints.Checkpoints.insert(
        "a".into(),
        astersql_lightning_pkg_checkpoints_checkpointspb::TableCheckpointModel {
            Status: CheckpointStatusLoaded as u32,
            Engines: HashMap::new(),
            ..Default::default()
        },
    );
    file_chkp.Close().unwrap();

    // 重新打开文件，验证持久化层不会把空 map 恢复成“缺失字段”。
    let file_chkp2 = NewFileCheckpointsDB(ctx, &path_str).unwrap();
    // if not recover empty map explicitly, it will become nil (Go)
    assert!(
        file_chkp2
            .checkpoints
            .Checkpoints
            .get("a")
            .map(|t| {
                let _ = &t.Engines;
                true
            })
            .unwrap_or(false)
    );
}

/// 这个用例覆盖 `separateCompletePath` 的路径拆分规则。
/// 输入既包含本地路径，也包含 `file://` 与 `s3://` URL，
/// 同时混入百分号编码、问号、Unicode 字符与查询参数。
/// 断言目标有两个：
/// 一是返回的文件名必须符合恢复逻辑后续使用方式，
/// 二是“父路径”必须保留协议、前缀和必要编码，
/// 这样下游重新拼接对象路径时才不会偏离 Go 行为。
/// 这类字符串处理最容易在看似无害的重构中产生回归，
/// 因为错误通常只出现在少量特殊字符组合上。
/// 其中 `file://` 的几组案例故意保留与 Go 兼容的编码差异说明，
/// 因为本地对象存储桩当前会输出小写十六进制编码。
/// 这些测试的价值在于把 URI 规范化中的细碎边界固定下来，
/// 避免未来重构 `separateCompletePath` 时破坏看似不起眼但恢复流程依赖的字符串格式。
/// TestSeparateCompletePath
#[test]
fn test_separate_complete_path() {
    let cases = [
        // 先覆盖空串、纯目录和普通相对/绝对文件路径。
        ("", "", ""),
        ("/a/", "", "/a/"),
        ("test.log", "test.log", "."),
        ("./test.log", "test.log", "."),
        ("./tmp/test.log", "test.log", "tmp"),
        ("tmp/test.log", "test.log", "tmp"),
        ("/test.log", "test.log", "/"),
        ("/tmp/test.log", "test.log", "/tmp"),
        ("/a%3F%2Fbc/a%3F%2Fbc.log", "a%3F%2Fbc.log", "/a%3F%2Fbc"),
        ("/a??bc/a??bc.log", "a??bc.log", "/a??bc"),
        (
            "/t-%C3%8B%21s%60t/t-%C3%8B%21s%60t.log",
            "t-%C3%8B%21s%60t.log",
            "/t-%C3%8B%21s%60t",
        ),
        ("/t-Ë!s`t/t-Ë!s`t.log", "t-Ë!s`t.log", "/t-Ë!s`t"),
        // `url.URL.String` 使用大写十六进制输出百分号编码；Rust 也必须保持一致。
        (
            "file:///a%3F%2Fbc/a%3F%2Fcd.log",
            "cd.log",
            "file:///a%3F/bc/a%3F",
        ),
        ("file:///a?/bc/a?/cd.log", "a", "file:///?/bc/a?/cd.log"),
        ("file:///a/?/bc/a?/cd.log", "", "file:///a/?/bc/a?/cd.log"),
        (
            "file:///t-%C3%8B%21s%60t/t-%C3%8B%21s%60t.log",
            "t-Ë!s`t.log",
            "file:///t-%C3%8B!s%60t",
        ),
        (
            "file:///t-Ë!s`t/t-Ë!s`t.log",
            "t-Ë!s`t.log",
            "file:///t-%C3%8B!s%60t",
        ),
        ("s3://bucket2/test.log", "test.log", "s3://bucket2/"),
        (
            "s3://bucket2/test/test.log",
            "test.log",
            "s3://bucket2/test",
        ),
        (
            "s3://bucket3/prefix/test.log?access-key=NXN7IPIOSAAKDEEOLMAF&secret-access-key=nREY/7Dt+PaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
            "test.log",
            "s3://bucket3/prefix?access-key=NXN7IPIOSAAKDEEOLMAF&secret-access-key=nREY/7Dt%2BPaIbYKrKlEEMMF/ExCiJEX=XMLPUANw",
        ),
    ];

    // 逐项遍历而不是拆成多个独立测试，便于与 Go 的表驱动写法保持一致。
    for (complete, expect_file_name, expect_path) in cases {
        let (file_name, new_path) = separateCompletePath(complete).unwrap();
        assert_eq!(
            expect_file_name, file_name,
            "file for complete={complete:?}"
        );
        assert_eq!(expect_path, new_path, "path for complete={complete:?}");
    }
}

/// 这个用例专门验证 `Apply` 对基值字段采用“取较大值”策略。
/// 初始表级 checkpoint 已经持有较大的三类基值，
/// 后续 diff 即使声明了更小的新值，也不应把已记录的上界回退。
/// 这是恢复后继续分配 row id 的安全前提，
/// 否则导入器可能重用已经消费过的编号区间。
/// 断言拆成三条单独比较，便于在失败时直接看出是哪一种基值被错误覆盖。
/// TestTableCheckpointApplyBases
#[test]
fn test_table_checkpoint_apply_bases() {
    let mut tbl_cp = TableCheckpoint {
        AutoRowIDBase: 11,
        AutoIncrBase: 12,
        AutoRandBase: 13,
        ..Default::default()
    };
    tbl_cp.Apply(&TableCheckpointDiff {
        hasRebase: true,
        autoRowIDBase: 1,
        autoIncrBase: 2,
        autoRandBase: 3,
        ..Default::default()
    });
    assert_eq!(tbl_cp.AutoRowIDBase, 11);
    assert_eq!(tbl_cp.AutoIncrBase, 12);
    assert_eq!(tbl_cp.AutoRandBase, 13);
}

// 这个辅助函数为文件 checkpoint 测试生成唯一临时目录。
// 目录名同时包含进程号和纳秒时间戳，
// 以降低并发测试或重复运行时发生路径冲突的概率。
// 它不参与业务逻辑，只负责让持久化测试具备稳定的文件系统隔离性。
// 这里不用测试框架自带目录，是为了保持与当前 Rust 迁移夹具的使用方式一致。
fn tempfile_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "lightning-checkpoints-unit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[allow(dead_code)]
// 这里保留一个对 `StorageHandle` 的最小引用，
// 目的是让迁移后的测试文件维持与主模块相同的导入集合，
// 同时避免编译器把相关 `use` 当成未使用导入报错。
// 该辅助函数本身不会被调用，因此显式标注 `dead_code`。
fn _keep_storage_handle_import() {
    let _ = StorageHandle::memory();
}
