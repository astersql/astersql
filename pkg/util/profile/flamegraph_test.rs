// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 火焰图展开结果单测：对照 Go `TestProfileToDatum` 的 fixture 行序与六列内容。

use std::fs::File;
use std::path::PathBuf;

use types::datum::{Datum, KindInt64, KindString, NewIntDatum, NewStringDatum};

use crate::Collector;
use crate::flamegraph::percentage;

/// 返回包内 `testdata/test.pprof` 的绝对路径。
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/test.pprof")
}

/// 构造期望火焰图行（标识、自占比、父占比、根子序号、深度、文件行）。
fn datum(
    name: &str,
    self_percent: &str,
    total_percent: &str,
    kind: i64,
    depth: i64,
    location: &str,
) -> Vec<Datum> {
    vec![
        NewStringDatum(name.to_owned()),
        NewStringDatum(self_percent.to_owned()),
        NewStringDatum(total_percent.to_owned()),
        NewIntDatum(kind),
        NewIntDatum(depth),
        NewStringDatum(location.to_owned()),
    ]
}

/// 将一行 Datum 转为可比较的字符串向量（字符串列与 int64 列）。
fn row_strings(row: &[Datum]) -> Vec<String> {
    row.iter()
        .map(|d| {
            let kind = d.Kind();
            if kind == KindString {
                d.GetString()
            } else if kind == KindInt64 {
                d.GetInt64().to_string()
            } else {
                panic!("unexpected datum kind {kind}")
            }
        })
        .collect()
}

/// Go `fmt.Sprintf("%.2g", ratio)` rounds before choosing fixed/scientific notation.
#[test]
fn percentage_rounding_crosses_fixed_notation_boundary_like_go() {
    assert_eq!(percentage(9_999, 10_000_000_000), "0.0001%");
    assert_eq!(percentage(99, 100_000_000), "9.9e-05%");
    assert_eq!(percentage(0, 0), "0%");
}

// test_profile_to_datum 对应 Go 的 TestProfileToDatum：读取 fixture 并逐行比较。
// Go uses Datum.Compare with the binary collator; the Rust port compares the
// same six columns via their string forms (identical for these fixtures).
/// 读取 test.pprof，断言火焰图行数与每行六列字符串与 Go 期望一致。
#[test]
fn test_profile_to_datum() {
    // Go defer view.Stop() cleans OpenCensus global view state. The Rust port
    // does not register OpenCensus views; keep the cleanup boundary explicit.
    let file = File::open(fixture_path()).expect("open testdata/test.pprof");
    let data = Collector::default()
        .ProfileReaderToDatums(file)
        .expect("ProfileReaderToDatums");

    let datums = vec![
        datum("root", "100%", "100%", 0, 0, "root"),
        datum(
            "├─runtime.main",
            "87.50%",
            "87.50%",
            1,
            1,
            "c:/go/src/runtime/proc.go:203",
        ),
        datum("│ └─main.main", "87.50%", "100%", 1, 2, "Z:/main.go:46"),
        datum(
            "│   ├─main.collatz",
            "68.75%",
            "78.57%",
            1,
            3,
            "Z:/main.go:22",
        ),
        datum(
            "│   │ └─crypto/cipher.(*ctr).XORKeyStream",
            "68.75%",
            "100%",
            1,
            4,
            "c:/go/src/crypto/cipher/ctr.go:84",
        ),
        datum(
            "│   │   ├─crypto/cipher.(*ctr).refill",
            "62.50%",
            "90.91%",
            1,
            5,
            "c:/go/src/crypto/cipher/ctr.go:60",
        ),
        datum(
            "│   │   │ ├─crypto/aes.(*aesCipherAsm).Encrypt",
            "56.25%",
            "90.00%",
            1,
            6,
            "c:/go/src/crypto/aes/cipher_asm.go:68",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "12.50%",
            "22.22%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:49",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:45",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:39",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:37",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:43",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:41",
        ),
        datum(
            "│   │   │ │ ├─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:51",
        ),
        datum(
            "│   │   │ │ └─crypto/aes.encryptBlockAsm",
            "6.25%",
            "11.11%",
            1,
            7,
            "c:/go/src/crypto/aes/asm_amd64.s:11",
        ),
        datum(
            "│   │   │ └─crypto/aes.(*aesCipherAsm).Encrypt",
            "6.25%",
            "10.00%",
            1,
            6,
            "c:/go/src/crypto/aes/cipher_asm.go:58",
        ),
        datum(
            "│   │   └─crypto/cipher.(*ctr).refill",
            "6.25%",
            "9.09%",
            1,
            5,
            "c:/go/src/crypto/cipher/ctr.go:60",
        ),
        datum(
            "│   ├─main.collatz",
            "12.50%",
            "14.29%",
            1,
            3,
            "Z:/main.go:30",
        ),
        datum(
            "│   │ └─main.collatz",
            "12.50%",
            "100%",
            1,
            4,
            "Z:/main.go:22",
        ),
        datum(
            "│   │   └─crypto/cipher.(*ctr).XORKeyStream",
            "12.50%",
            "100%",
            1,
            5,
            "c:/go/src/crypto/cipher/ctr.go:84",
        ),
        datum(
            "│   │     └─crypto/cipher.(*ctr).refill",
            "12.50%",
            "100%",
            1,
            6,
            "c:/go/src/crypto/cipher/ctr.go:60",
        ),
        datum(
            "│   │       ├─crypto/aes.(*aesCipherAsm).Encrypt",
            "6.25%",
            "50.00%",
            1,
            7,
            "c:/go/src/crypto/aes/cipher_asm.go:68",
        ),
        datum(
            "│   │       │ └─crypto/aes.encryptBlockAsm",
            "6.25%",
            "100%",
            1,
            8,
            "c:/go/src/crypto/aes/asm_amd64.s:45",
        ),
        datum(
            "│   │       └─crypto/aes.(*aesCipherAsm).Encrypt",
            "6.25%",
            "50.00%",
            1,
            7,
            "c:/go/src/crypto/aes/cipher_asm.go:65",
        ),
        datum(
            "│   │         └─crypto/internal/subtle.InexactOverlap",
            "6.25%",
            "100%",
            1,
            8,
            "c:/go/src/crypto/internal/subtle/aliasing.go:33",
        ),
        datum(
            "│   │           └─crypto/internal/subtle.AnyOverlap",
            "6.25%",
            "100%",
            1,
            9,
            "c:/go/src/crypto/internal/subtle/aliasing.go:20",
        ),
        datum(
            "│   └─main.collatz",
            "6.25%",
            "7.14%",
            1,
            3,
            "Z:/main.go:20",
        ),
        datum(
            "│     └─runtime.memmove",
            "6.25%",
            "100%",
            1,
            4,
            "c:/go/src/runtime/memmove_amd64.s:362",
        ),
        datum(
            "├─runtime.mstart",
            "6.25%",
            "6.25%",
            2,
            1,
            "c:/go/src/runtime/proc.go:1146",
        ),
        datum(
            "│ └─runtime.systemstack",
            "6.25%",
            "100%",
            2,
            2,
            "c:/go/src/runtime/asm_amd64.s:370",
        ),
        datum(
            "│   └─runtime.bgscavenge.func2",
            "6.25%",
            "100%",
            2,
            3,
            "c:/go/src/runtime/mgcscavenge.go:315",
        ),
        datum(
            "│     └─runtime.(*mheap).scavengeLocked",
            "6.25%",
            "100%",
            2,
            4,
            "c:/go/src/runtime/mheap.go:1446",
        ),
        datum(
            "│       └─runtime.(*mspan).scavenge",
            "6.25%",
            "100%",
            2,
            5,
            "c:/go/src/runtime/mheap.go:589",
        ),
        datum(
            "│         └─runtime.sysUnused",
            "6.25%",
            "100%",
            2,
            6,
            "c:/go/src/runtime/mem_windows.go:33",
        ),
        datum(
            "│           └─runtime.stdcall3",
            "6.25%",
            "100%",
            2,
            7,
            "c:/go/src/runtime/os_windows.go:837",
        ),
        datum(
            "└─runtime.morestack",
            "6.25%",
            "6.25%",
            3,
            1,
            "c:/go/src/runtime/asm_amd64.s:449",
        ),
        datum(
            "  └─runtime.newstack",
            "6.25%",
            "100%",
            3,
            2,
            "c:/go/src/runtime/stack.go:1038",
        ),
        datum(
            "    └─runtime.gopreempt_m",
            "6.25%",
            "100%",
            3,
            3,
            "c:/go/src/runtime/proc.go:2653",
        ),
        datum(
            "      └─runtime.goschedImpl",
            "6.25%",
            "100%",
            3,
            4,
            "c:/go/src/runtime/proc.go:2625",
        ),
        datum(
            "        └─runtime.schedule",
            "6.25%",
            "100%",
            3,
            5,
            "c:/go/src/runtime/proc.go:2524",
        ),
        datum(
            "          └─runtime.findrunnable",
            "6.25%",
            "100%",
            3,
            6,
            "c:/go/src/runtime/proc.go:2170",
        ),
    ];

    assert_eq!(data.len(), datums.len(), "row count mismatch");
    for (i, row) in data.iter().enumerate() {
        let actual = row_strings(row);
        let expected = row_strings(&datums[i]);
        assert_eq!(
            actual, expected,
            "row {i:2}, actual ({actual:?}), expected ({expected:?})"
        );
    }
}
