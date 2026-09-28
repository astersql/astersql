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

// Ingestor 包文档与 Go 职责对齐的迁移单元测试。
//
// 编译期与运行期检查 `doc.rs` 是否仍描述 SST 直灌、KV 排序与导入环境准备等职责。

/// 嵌入的包级文档源文本（`doc.rs`）。
const DOCUMENTATION: &str = include_str!("doc.rs");

/// 编译期可用的朴素子串查找，供 `const` 断言复用。
const fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        let mut offset = 0;
        while offset < needle.len() && haystack[start + offset] == needle[offset] {
            offset += 1;
        }
        if offset == needle.len() {
            return true;
        }
        start += 1;
    }
    false
}

// 以下 const 断言锁定与 Go 包文档对应的关键职责句子
const _: () = assert!(contains(
    DOCUMENTATION.as_bytes(),
    b"//! Package ingestor provides interfaces for ingesting SSTs directly",
));
const _: () = assert!(contains(
    DOCUMENTATION.as_bytes(),
    b"//! - Sort encoded KVs locally or globally",
));
const _: () = assert!(contains(
    DOCUMENTATION.as_bytes(),
    b"//! - Prepare the environment for writing KVs and ingesting SSTs",
));
const _: () = assert!(contains(
    DOCUMENTATION.as_bytes(),
    b"//! Most implementations currently live in `pkg/lightning/backend`",
));

/// 运行期再确认版权头存在，且不含机械翻译残留措辞。
#[test]
fn package_documentation_matches_go_responsibilities() {
    assert!(DOCUMENTATION.starts_with("// Copyright 2026 AsterSQL.\n"));

    for responsibility in [
        "underlying storage layer",
        "local disk for partial sorting",
        "external storage for intermediate sorted files before merge sorting",
        "pausing PD schedulers",
        "splitting and scattering regions based on the sorted",
        "switching TiKV to import mode",
        "moved into this package gradually",
    ] {
        assert!(
            DOCUMENTATION.contains(responsibility),
            "package documentation omitted Go responsibility: {responsibility}",
        );
    }

    assert!(!DOCUMENTATION.contains("mechanically"));
    assert!(!DOCUMENTATION.contains("guaranteed to compile"));
}
