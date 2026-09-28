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

//! Go `import_test.go` — ImportKVFiles / FilterFilesByRegion / FileImporter.
//! 对应 Go 侧日志文件导入测试：校验参数错误路径、按 Region 过滤文件，
//! 以及 FileImporter 在 Mem 桩上的 Clear/Import/Close 生命周期。
//! 边界依赖（PD/TiKV/importer gRPC）全部用本地 Mem* 替代，避免真实集群。
//! 过滤用例把 files 与 ranges 下标对齐，断言路径列表顺序与 Go 一致。
//! FakeImportClient 用于观测 ClearFiles 前缀回写与空 Metas 拒绝路径。
//! prepare_data 故意保留 bakcup 拼写，避免与 Go 夹具字符串漂移。
//! ImportKVFiles 的批量与非批量路径都必须成功，与 Go 的 NoError 断言一致。

use std::sync::Arc;

use astersql_br_pkg_restore_utils::stubs::{codec, tablecodec};
use astersql_br_pkg_restore_utils::{GetRewriteRuleOfTable, RewriteRules};

use crate::export_test::FilterFilesByRegion;
use crate::import::NewLogFileImporter;
use crate::log_file_manager::LogDataFileInfo;
use crate::stubs::berrors;
use crate::stubs::import_sstpb;
use crate::stubs::importclient::{ImporterClient, MemImporterClient};
use crate::stubs::kv::KeyRange;
use crate::stubs::metapb;
use crate::stubs::pd::MemPdClient;
use crate::stubs::split_client::{MemSplitClient, RegionInfo};
use crate::stubs::{Context, Result};

/// Go `TestImportKVFiles`：空规则 + 乱序 path 应被拒绝。
/// 断言错误码与 Go `BR:Common:ErrInvalidArgument` 对齐，证明参数校验先于导入。
/// TS 参数 100/200/300 仅为占位，真正触发点是空 RewriteRules。
#[test]
fn test_import_kv_files() {
    // 使用默认 MemSplit/MemImporter，不注入真实存储，专注参数校验分支。
    let importer = NewLogFileImporter(
        Arc::new(MemSplitClient::default()),
        Arc::new(MemImporterClient::default()),
        None,
    );
    let ctx = Context::Background();
    // 刻意提供无有效 rewrite/range 的文件列表，触发 InvalidArgument。
    // 路径 log3/log1 与 Filter 用例共用命名习惯，便于阅读对照。
    let err = importer
        .ImportKVFiles(
            &ctx,
            &[
                LogDataFileInfo {
                    Path: "log3".into(),
                    ..Default::default()
                },
                LogDataFileInfo {
                    Path: "log1".into(),
                    ..Default::default()
                },
            ],
            &RewriteRules::default(),
            100,
            200,
            300,
            false,
            None,
            &[],
        )
        .unwrap_err();
    // 与 Go berrors 错误码字符串保持一致，便于跨语言对照回归。
    assert_eq!(err.code, Some("BR:Common:ErrInvalidArgument"));
    // 额外触达错误构造函数，防止桩侧常量漂移。
    let _ = berrors::ErrInvalidArgument("x");
}

/// Go `TestFilterFilesByRegion`：验证文件 range 与 Region 边界相交规则。
/// files[i] 与 ranges[i] 一一对应；cases 覆盖左开右闭、空 EndKey（开区间）等边界。
/// 不相交 Region 期望空列表；半开边界与 Go kv.KeyRange 相交判定对齐。
#[test]
fn test_filter_files_by_region() {
    // log3 ↔ [1111,2222)，log1 ↔ [3333,4444)；路径名仅作断言标识。
    // LogDataFileInfo 其它字段默认即可，过滤只依赖与 ranges 的配对关系。
    let files = vec![
        LogDataFileInfo {
            Path: "log3".into(),
            ..Default::default()
        },
        LogDataFileInfo {
            Path: "log1".into(),
            ..Default::default()
        },
    ];
    // 字节串用十进制可读键，避免二进制难对照。
    let ranges = vec![
        KeyRange {
            StartKey: b"1111".to_vec(),
            EndKey: b"2222".to_vec(),
        },
        KeyRange {
            StartKey: b"3333".to_vec(),
            EndKey: b"4444".to_vec(),
        },
    ];
    // 期望路径顺序与 Go 用例一致：仅保留与 Region 相交的文件。
    // 前几案覆盖左侧空隙、贴边、跨越；后几案覆盖右侧与开区间。
    let cases: Vec<(RegionInfo, Vec<&str>)> = vec![
        (region(b"0000", b"1110"), vec![]),
        (region(b"0000", b"1111"), vec!["log3"]),
        (region(b"0000", b"2222"), vec!["log3"]),
        (region(b"2222", b"3332"), vec!["log3"]),
        (region(b"2223", b"3332"), vec![]),
        (region(b"3332", b"3333"), vec!["log1"]),
        (region(b"4444", b"5555"), vec!["log1"]),
        // 空 EndKey 表示到正无穷，应覆盖右侧文件。
        (region_open(b"4444"), vec!["log1"]),
        // 从最小键开区间应同时命中两个文件。
        (region_open(b"0000"), vec!["log3", "log1"]),
    ];
    for (r, expect_paths) in cases {
        let sub = FilterFilesByRegion(&files, &ranges, &r).unwrap();
        let paths: Vec<_> = sub.iter().map(|f| f.Path.as_str()).collect();
        assert_eq!(paths, expect_paths);
    }
}

// 封闭区间 Region 构造：EndKey 非空，与 Go metapb.Region 测试夹具一致。
fn region(start: &[u8], end: &[u8]) -> RegionInfo {
    RegionInfo {
        Region: Some(metapb::Region {
            StartKey: start.to_vec(),
            EndKey: end.to_vec(),
            ..Default::default()
        }),
        Leader: None,
    }
}
// 开右界 Region：EndKey 为空表示无穷大，用于覆盖末尾 keyspace。
fn region_open(start: &[u8]) -> RegionInfo {
    RegionInfo {
        Region: Some(metapb::Region {
            StartKey: start.to_vec(),
            EndKey: vec![],
            ..Default::default()
        }),
        Leader: None,
    }
}

// FakeImportClient：在 ClearFiles 响应中回写 Prefix，便于观测调用是否到达。
// ApplyKVFile 对空 Metas 返回 ErrKVRangeIsEmpty，对齐 Go 空 range 防护。
// 其它方法委托 MemImporterClient，保持最小可注入面。
struct FakeImportClient {
    inner: MemImporterClient,
}

impl ImporterClient for FakeImportClient {
    fn CloseGrpcClient(&self) -> Result<()> {
        // 关闭 gRPC 通道；Mem 实现为空操作。
        self.inner.CloseGrpcClient()
    }
    fn ClearFiles(
        &self,
        ctx: &Context,
        store_id: u64,
        req: &import_sstpb::ClearRequest,
    ) -> Result<import_sstpb::ClearResponse> {
        // 先走真实 Mem 清理，再把 Prefix 塞进 Error.Message 供上层感知。
        // 注意：此处 Error 字段被用作观测通道，并非表示清理失败。
        let _ = self.inner.ClearFiles(ctx, store_id, req)?;
        Ok(import_sstpb::ClearResponse {
            Error: Some(import_sstpb::Error {
                Message: req.Prefix.clone(),
                ..Default::default()
            }),
        })
    }
    fn ApplyKVFile(
        &self,
        ctx: &Context,
        store_id: u64,
        req: &import_sstpb::ApplyRequest,
    ) -> Result<import_sstpb::ApplyResponse> {
        // 空 metas 在真正 apply 前拦截，避免 Mem 桩吞掉非法请求。
        if req.Metas.is_empty() {
            return Err(berrors::ErrKVRangeIsEmpty("empty metas"));
        }
        self.inner.ApplyKVFile(ctx, store_id, req)
    }
}

// 准备表 1→2 与 767→511 两套 rewrite，以及表 1 记录前缀编码后的日志文件。
// 路径拼写 bakcup 与 Go 测试保持一致，避免因更正拼写导致对照失败。
// 第二套规则覆盖非连续表 ID，防止导入只处理单一 rewrite。
fn prepare_data() -> (RewriteRules, Vec<LogDataFileInfo>) {
    let mut rules = GetRewriteRuleOfTable(1, 2, Default::default(), false);
    let extra = GetRewriteRuleOfTable(767, 511, Default::default(), false);
    // 合并 Data 规则切片，模拟多表恢复场景。
    rules.Data.extend(extra.Data);
    let encode_key_files = vec![LogDataFileInfo {
        Path: "bakcup.log".into(),
        // Start/End 用 tablecodec 编码，模拟真实备份键空间。
        // PrefixNext 得到半开区间上界，与 TiDB 表前缀惯例一致。
        StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::GenTableRecordPrefix(1)),
        EndKey: codec::EncodeBytes(
            Vec::new(),
            &tablecodec::PrefixNext(&tablecodec::GenTableRecordPrefix(1)),
        ),
        ..Default::default()
    }];
    (rules, encode_key_files)
}

/// Go `TestFileImporter`：全链路烟雾——ClearFiles、批/非批 Import、Close。
/// 批量与非批量 Import 都必须成功；非批量桩返回 ErrKVRangeIsEmpty，导入器应按 Go 语义跳过 region。
/// PD 仅提供 store 1 Up，保证 ClearFiles 能解析到目标节点。
#[test]
fn test_file_importer() {
    let ctx = Context::Background();
    // 单 Region 覆盖全 keyspace，Leader 指向 store 1，满足 Clear/Import 路由。
    // by_id 与 regions 双索引保持一致，避免按 ID 查找失败。
    let meta = Arc::new(MemSplitClient {
        regions: std::sync::Mutex::new(vec![RegionInfo {
            Region: Some(metapb::Region {
                Id: 1,
                StartKey: vec![],
                EndKey: vec![],
                ..Default::default()
            }),
            Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
        }]),
        by_id: std::sync::Mutex::new(std::collections::HashMap::from([(
            1u64,
            RegionInfo {
                Region: Some(metapb::Region {
                    Id: 1,
                    ..Default::default()
                }),
                Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
            },
        )])),
    });
    let import = Arc::new(FakeImportClient {
        inner: MemImporterClient::default(),
    });
    let importer = NewLogFileImporter(meta.clone(), import, None);
    let pd = MemPdClient {
        cluster_id: 1,
        stores: vec![metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            ..Default::default()
        }],
    };
    // ClearFiles 成功说明 FakeImportClient 已被正确注入。
    importer.ClearFiles(&ctx, &pd, "test").unwrap();
    let (rewrite_rules, encode_key_files) = prepare_data();
    // 批处理与非批各跑一次，并保持 Go require.NoError 的测试强度。
    importer
        .ImportKVFiles(
            &ctx,
            &encode_key_files,
            &rewrite_rules,
            1,
            1,
            1,
            true,
            None,
            &[],
        )
        .unwrap();
    importer
        .ImportKVFiles(
            &ctx,
            &encode_key_files,
            &rewrite_rules,
            1,
            1,
            1,
            false,
            None,
            &[],
        )
        .unwrap();
    // Close 必须成功，对应 Go 测试对资源释放的硬性要求。
    importer.Close().unwrap();
}
