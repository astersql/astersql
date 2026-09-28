// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `tablecodec` 编解码微基准（对应 Go `bench_test.go`）。
//
// 覆盖行键（Row Key）、结束键、前缀下一键（PrefixNext）、索引值中 handle 的编解码吞吐路径。

use super::*;

/// 基准：按表 ID 与整型 handle 编码行键。
pub fn BenchmarkEncodeRowKeyWithHandle(iterations: usize) {
    for _ in 0..iterations {
        let _ = EncodeRowKeyWithHandle(100, Box::new(kv::IntHandle(100)));
    }
}

/// 基准：编码相邻 handle 的行键，模拟结束键构造。
pub fn BenchmarkEncodeEndKey(iterations: usize) {
    for _ in 0..iterations {
        let _ = EncodeRowKeyWithHandle(100, Box::new(kv::IntHandle(100)));
        let _ = EncodeRowKeyWithHandle(100, Box::new(kv::IntHandle(101)));
    }
}

// PrefixNext 比直接编码结束 key 慢；保留 Go benchmark 的比较说明和调用顺序。
/// 基准：先编码行键再 `PrefixNext`（字典序下一前缀），对比直接编码结束键。
pub fn BenchmarkEncodeRowKeyWithPrefixNex(iterations: usize) {
    for _ in 0..iterations {
        let sk = EncodeRowKeyWithHandle(100, Box::new(kv::IntHandle(100)));
        let _ = sk.PrefixNext();
    }
}

/// 基准：从行键解码出 handle。
pub fn BenchmarkDecodeRowKey(iterations: usize) {
    let row_key = EncodeRowKeyWithHandle(100, Box::new(kv::IntHandle(100)));
    for _ in 0..iterations {
        DecodeRowKey(row_key.clone()).unwrap();
    }
}

/// 基准：从唯一索引值解码整型 handle。
pub fn BenchmarkDecodeIndexKeyIntHandle(iterations: usize) {
    // handle >255 的 Go fixture 用于覆盖额外内存分配路径。
    let idx_val = EncodeHandleInUniqueIndexValue(Box::new(kv::IntHandle(256)), false);
    for _ in 0..iterations {
        let _ = DecodeHandleInIndexValue(idx_val.clone());
    }
}

/// 基准：从索引值解码 Common Handle（聚簇索引多列主键编码）。
pub fn BenchmarkDecodeIndexKeyCommonHandle(iterations: usize) {
    // 构造带版本标志的 Common Handle 索引值夹具。
    let mut idx_val = vec![0, IndexVersionFlag, 1];
    let encoded = codec::EncodeKey(
        time::UTC,
        Vec::new(),
        vec![types::NewIntDatum(1), types::NewIntDatum(2)],
    )
    .unwrap();
    let handle = kv::NewCommonHandle(encoded).unwrap();
    idx_val = encodeCommonHandle(idx_val, Box::new(handle));
    for _ in 0..iterations {
        let _ = DecodeHandleInIndexValue(idx_val.clone());
    }
}

#[test]
/// 冒烟：对齐 Go `benchdaily.Run` 的 benchmark 集合。
pub fn TestBenchDaily() {
    BenchmarkEncodeRowKeyWithHandle(1);
    BenchmarkEncodeEndKey(1);
    BenchmarkEncodeRowKeyWithPrefixNex(1);
    super::tablecodec_test::BenchmarkHasTablePrefix();
    super::tablecodec_test::BenchmarkHasTablePrefixBuiltin();
    super::tablecodec_test::BenchmarkEncodeValue();
}
