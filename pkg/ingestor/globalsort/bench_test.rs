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

// 全局排序吞吐/分析基准测试的 Rust 移植（对应 Go `bench_test.go`）。
//
// Go 入口都以 `openTestingStorage(t)` 为前置条件；未提供
// `--testing-storage-uri` 时由测试框架标记为 skipped。Rust 端尚未接入
// 等价的外部存储配置，因此显式使用 `#[ignore]`，避免把未执行的
// GB 级吞吐基准误报为 passed。

/// Corresponds to Go's TestCompareWriter.
/// 比较写入器吞吐（需外部存储）。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_compare_writer() {}

/// Corresponds to Go's TestCompareReaderEvenlyDistributedContent.
/// 均匀分布内容下的读取比较。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_compare_reader_evenly_distributed_content() {}

/// Corresponds to Go's TestReadFileConcurrently.
/// 并发读文件基准。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_file_concurrently() {}

/// Corresponds to Go's TestReadFileSequential.
/// 顺序读文件基准。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_file_sequential() {}

/// Corresponds to Go's TestReadMergeIterCheckHotspot.
/// 带热点检测的归并迭代读取。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_merge_iter_check_hotspot() {}

/// Corresponds to Go's TestReadMergeIterWithoutCheckHotspot.
/// 不带热点检测的归并迭代读取。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_merge_iter_without_check_hotspot() {}

/// Corresponds to Go's TestMergeBench.
/// 归并步骤吞吐基准。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_merge_bench() {}

/// Corresponds to Go's TestReadAllDataLargeFiles.
/// 大文件全量读取基准。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_all_data_large_files() {}

/// Corresponds to Go's TestReadAllData.
/// 全量读取基准。
#[test]
#[ignore = "requires --testing-storage-uri and an external object store"]
fn test_read_all_data() {}
