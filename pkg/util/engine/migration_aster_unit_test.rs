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

// engine 迁移回归测试：Classic/NextGen TiFlash 标签识别。
//
// 覆盖 `IsTiFlash`（metapb/kvproto）、`IsTiFlashHTTPResp` 与
// `IsTiFlashWriteHTTPResp`（排除 NextGen compute）与 Go 行为对齐。

use super::{IsTiFlash, IsTiFlashHTTPResp, IsTiFlashWriteHTTPResp, Label, LabelStore};

/// 测试用标签，实现 `Label`。
#[derive(Debug)]
struct TestLabel {
    key: &'static str,
    value: &'static str,
}

impl Label for TestLabel {
    fn key(&self) -> &str {
        self.key
    }

    fn value(&self) -> &str {
        self.value
    }
}

/// 测试用 store，内部为标签向量。
#[derive(Debug)]
struct TestStore(Vec<TestLabel>);

impl LabelStore for TestStore {
    type Label = TestLabel;

    fn labels(&self) -> &[Self::Label] {
        &self.0
    }
}

/// 由 (key, value) 切片快速构造 `TestStore`。
fn store(labels: &[(&'static str, &'static str)]) -> TestStore {
    TestStore(
        labels
            .iter()
            .map(|&(key, value)| TestLabel { key, value })
            .collect(),
    )
}

/// 校验 `IsTiFlash`：Classic/compute 为真，无关标签或空列表为假。
#[test]
fn metapb_store_recognizes_classic_and_nextgen_tiflash() {
    assert!(IsTiFlash(&store(&[("engine", "tiflash")])));
    assert!(IsTiFlash(&store(&[("engine", "tiflash_compute")])));
    assert!(IsTiFlash(&store(&[
        ("zone", "engine"),
        ("engine", "tiflash"),
    ])));
    assert!(!IsTiFlash(&store(&[("engine_role", "write")])));
    assert!(!IsTiFlash(&store(&[("engine", "not_tiflash")])));
    assert!(!IsTiFlash(&store(&[])));
}

/// 校验 HTTP 路径下 Classic、写节点与 compute 均被识别为 TiFlash。
#[test]
fn http_store_recognizes_classic_write_and_compute_nodes() {
    assert!(IsTiFlashHTTPResp(&store(&[("engine", "tiflash")])));
    assert!(IsTiFlashHTTPResp(&store(&[
        ("engine", "tiflash"),
        ("engine_role", "write"),
    ])));
    assert!(IsTiFlashHTTPResp(&store(&[("engine", "tiflash_compute",)])));
    assert!(!IsTiFlashHTTPResp(&store(&[("engine", "not_tiflash",)])));
    assert!(!IsTiFlashHTTPResp(&store(&[])));
}

/// 校验写节点过滤器排除 `tiflash_compute`。
#[test]
fn http_write_filter_excludes_nextgen_compute_nodes() {
    assert!(IsTiFlashWriteHTTPResp(&store(&[("engine", "tiflash")])));
    assert!(IsTiFlashWriteHTTPResp(&store(&[
        ("engine", "tiflash"),
        ("engine_role", "write"),
    ])));
    assert!(!IsTiFlashWriteHTTPResp(&store(&[(
        "engine",
        "tiflash_compute",
    )])));
    assert!(!IsTiFlashWriteHTTPResp(&store(&[(
        "engine",
        "not_tiflash",
    )])));
    assert!(!IsTiFlashWriteHTTPResp(&store(&[])));
}
