// Copyright 2016 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Codec 包日常性能基准（benchdaily）用例。
//
// 对应 Go `pkg/util/codec/bench_test.go`：准备编码输入，测量 Decode / EncodeInt /
// DecodeDecimal / DecodeOneToChunk 等路径，并由 `TestBenchDaily` 注册到 benchdaily。

// 本文件由 pkg/util/codec/bench_test.go 迁移而来，保留 benchmark 输入准备和 benchdaily 注册。
//

use super::*;

/// 基准默认构造的 Datum 个数。
const VALUE_COUNT: usize = 100;

// composeEncodedData 对应 Go 的同名辅助：构造 size 个 Datum 后用 UTC 时区 EncodeValue。
/// 构造 `size` 个整型 Datum 并编码为字节，供 Decode 基准复用。
fn composeEncodedData(size: usize) -> Vec<u8> {
    let mut values = Vec::with_capacity(size);
    for i in 0..size {
        values.push(types::NewIntDatum(i as i64));
    }
    EncodeValue(time::UTC, Vec::new(), values).expect("benchmark values must encode")
}

/// 已知 value 数量时 Decode 的基准。
pub fn BenchmarkDecodeWithSize(b: &mut benchdaily::Benchmark) {
    let bs = composeEncodedData(VALUE_COUNT);
    b.iter(|| Decode(bs.clone(), VALUE_COUNT).expect("encoded benchmark values must decode"));
}

/// 未知完整 value 数量（传入 1）时 Decode 的基准。
pub fn BenchmarkDecodeWithOutSize(b: &mut benchdaily::Benchmark) {
    let bs = composeEncodedData(VALUE_COUNT);
    // Go 源码传入 1 表示不知道完整 value 数量。
    b.iter(|| Decode(bs.clone(), 1).expect("encoded benchmark values must decode"));
}

/// 预分配容量后 EncodeInt 的基准。
pub fn BenchmarkEncodeIntWithSize(b: &mut benchdaily::Benchmark) {
    b.iter(|| EncodeInt(Vec::with_capacity(8), 10));
}

/// 空 Vec 上 EncodeInt（依赖扩容）的基准。
pub fn BenchmarkEncodeIntWithOutSize(b: &mut benchdaily::Benchmark) {
    b.iter(|| EncodeInt(Vec::new(), 10));
}

/// Decimal 编解码往返中的 DecodeDecimal 基准。
pub fn BenchmarkDecodeDecimal(b: &mut benchdaily::Benchmark) {
    let dec = types::NewDecFromFloatForTest(1211.1211113);
    let (precision, frac) = dec.PrecisionAndFrac();
    let raw = EncodeDecimal(Vec::new(), &dec, precision as i32, frac as i32)
        .expect("benchmark decimal must encode");
    b.iter(|| DecodeDecimal(&raw).expect("encoded benchmark decimal must decode"));
}

/// Decoder::DecodeOne 写入 Chunk 的基准。
pub fn BenchmarkDecodeOneToChunk(b: &mut benchdaily::Benchmark) {
    let string = types::NewStringDatum("a".to_owned());
    let mut raw = vec![1_u8]; // bytesFlag
    raw = EncodeBytes(raw, &string.GetBytes());
    let mut int_type = types::NewFieldType(mysql::TypeLonglong);
    let mut chunk = chunk::New(vec![(*int_type).clone()], 32, 32);
    let mut decoder = NewDecoder(&mut *chunk, time::UTC);
    b.iter(|| {
        decoder
            .DecodeOne(raw.clone(), 0, &mut *int_type)
            .expect("encoded benchmark value must decode")
    });
}

// TestBenchDaily 对应 Go 的 benchdaily 注册测试，把本文件所有 benchmark 交给日常性能框架。
/// 将本文件全部 benchmark 交给 benchdaily 日常性能框架执行。
#[test]
pub fn TestBenchDaily() {
    benchdaily::Run(vec![
        BenchmarkDecodeWithSize,
        BenchmarkDecodeWithOutSize,
        BenchmarkEncodeIntWithSize,
        BenchmarkEncodeIntWithOutSize,
        BenchmarkDecodeDecimal,
        BenchmarkDecodeOneToChunk,
    ]);
}
