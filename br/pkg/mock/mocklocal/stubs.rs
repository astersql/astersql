// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Local stand-ins for lightning backend / import_sstpb / tikv Codec
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! mocklocal 测试桩的轻量类型替身：在 darwin/arm64 等不便链接 kvproto、grpcio、
//! lightning backend 原生依赖的环境中，用本地结构体对齐 Go mockgen 产物所依赖的
//! `backend.EngineFileSize`、`import_sstpb.Range`、`tikv.Codec` 外形。
//! 这些类型仅承载 mock 调用中的字段形状与 `take_ts` 解包，不提供真实编码、
//! 导入或 TiKV RPC 能力；完整行为仍以 Go `local.go`（MockGen）为准。

use std::any::Any;

/// Go: `backend.EngineFileSize`.
///
/// Lightning 引擎文件占用摘要的本地替身：UUID/磁盘/内存与导入中标志，
/// 供 DiskUsage 类 mock 返回值使用，不访问真实引擎目录。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineFileSize {
    pub UUID: [u8; 16],
    pub DiskSize: i64,
    pub MemSize: i64,
    pub IsImporting: bool,
}

/// Go: `import_sstpb.Range`.
///
/// SST 键范围的字节区间替身；仅保存 start/end，不解析 protobuf 或校验有序性。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}

/// Go: `tikv.Codec` interface stand-in.
///
/// TiKV Codec 接口的最小占位：真实 Codec 负责 key 编解码，此处仅用 id 区分实例，
/// 避免在 mock 路径上拉入 client-go Codec 实现。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Codec {
    pub id: i64,
}

/// Extract `(T0, T1, Option<Error>)` from gomock returns (Go GetTS shape).
///
/// 将 gomock 风格的 `Vec<Box<dyn Any>>` 解包为 GetTS 三元组 (physical, logical, err)。
/// 缺省槽位回落为 0 / None；错误槽同时接受 `Option<Error>` 与裸 `Error`，
/// 以兼容不同录制方式；类型不符时视为无错误而非 panic。
pub fn take_ts(
    mut rets: Vec<Box<dyn Any + Send>>,
) -> (i64, i64, Option<astersql_br_pkg_mock::Error>) {
    // 物理时间戳：空向量或 downcast 失败时置 0，避免 mock 未设返回值时崩溃。
    let physical = if rets.is_empty() {
        0i64
    } else {
        let r = rets.remove(0);
        r.downcast::<i64>().map(|b| *b).unwrap_or(0)
    };
    // 逻辑时间戳与物理槽位相同的容错策略。
    let logical = if rets.is_empty() {
        0i64
    } else {
        let r = rets.remove(0);
        r.downcast::<i64>().map(|b| *b).unwrap_or(0)
    };
    // 第三槽：优先 Option<Error>，其次包装裸 Error；未知类型忽略为 None。
    let err = if rets.is_empty() {
        None
    } else {
        let r = rets.remove(0);
        if r.is::<Option<astersql_br_pkg_mock::Error>>() {
            *r.downcast::<Option<astersql_br_pkg_mock::Error>>().unwrap()
        } else if r.is::<astersql_br_pkg_mock::Error>() {
            Some(*r.downcast::<astersql_br_pkg_mock::Error>().unwrap())
        } else {
            None
        }
    };
    (physical, logical, err)
}
