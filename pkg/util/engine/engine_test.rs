// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// engine 包单元测试：`IsTiFlashHTTPResp` / `IsTiFlashWriteHTTPResp` 表驱动用例。
//
// 用简化的 label/store 草稿结构模拟 PD HTTP MetaStore，覆盖 Classic TiFlash、
// NextGen 写/算节点、非 TiFlash 与空标签等场景。

use super::engine::{IsTiFlashHTTPResp, IsTiFlashWriteHTTPResp, Label, LabelStore};

// StoreLabelDraft 对应 Go 的 pdhttp.StoreLabel。
// 字段名保持 key/value 语义，用于表达测试表中的 label 数据。
/// 测试用 store 标签草稿，实现 `Label`。
#[derive(Clone, Debug)]
struct StoreLabelDraft {
    key: &'static str,
    value: &'static str,
}

impl Label for StoreLabelDraft {
    fn key(&self) -> &str {
        self.key
    }

    fn value(&self) -> &str {
        self.value
    }
}

// MetaStoreDraft 对应 Go 的 pdhttp.MetaStore，只保留本测试读取的 Labels 字段。
/// 测试用 MetaStore 草稿，仅持有 labels。
#[derive(Clone, Debug)]
struct MetaStoreDraft {
    labels: Vec<StoreLabelDraft>,
}

impl LabelStore for MetaStoreDraft {
    type Label = StoreLabelDraft;

    fn labels(&self) -> &[Self::Label] {
        &self.labels
    }
}

/// 单条引擎识别用例：名称、输入 store、期望布尔结果。
struct EngineCase {
    name: &'static str,
    store: MetaStoreDraft,
    want: bool,
}

/// 构造一条静态生命周期的标签草稿。
fn label(key: &'static str, value: &'static str) -> StoreLabelDraft {
    StoreLabelDraft { key, value }
}

// TestIsTiFlashHTTPResp 对应 Go 的 TestIsTiFlashHTTPResp。
// 它验证 classic TiFlash、NextGen write/compute、非 TiFlash 和空 label 的返回值。
/// 表驱动校验 `IsTiFlashHTTPResp`（compute 节点也应为 true）。
#[test]
fn test_is_tiflash_http_resp() {
    let tests = vec![
        EngineCase {
            name: "Test with TiFlash label",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash")],
            },
            want: true,
        },
        EngineCase {
            name: "Test with TiFlash write label under NextGen kernel",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash"), label("engine_role", "write")],
            },
            want: true,
        },
        EngineCase {
            name: "Test with TiFlash compute label under NextGen kernel",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash_compute")],
            },
            want: true,
        },
        EngineCase {
            name: "Test without TiFlash label",
            store: MetaStoreDraft {
                labels: vec![label("engine", "not_tiflash")],
            },
            want: false,
        },
        EngineCase {
            name: "Test with no labels",
            store: MetaStoreDraft { labels: vec![] },
            want: false,
        },
    ];

    for tt in tests {
        assert_eq!(tt.want, IsTiFlashHTTPResp(&tt.store), "{}", tt.name);
    }
}

// TestIsTiFlashWriteHTTPResp 对应 Go 的 TestIsTiFlashWriteHTTPResp。
// 与上一个测试的关键差异是 tiflash_compute 必须返回 false。
/// 表驱动校验 `IsTiFlashWriteHTTPResp`（`tiflash_compute` 必须为 false）。
#[test]
fn test_is_tiflash_write_http_resp() {
    let tests = vec![
        EngineCase {
            name: "Test with TiFlash label",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash")],
            },
            want: true,
        },
        EngineCase {
            name: "Test with TiFlash write label under NextGen kernel",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash"), label("engine_role", "write")],
            },
            want: true,
        },
        EngineCase {
            name: "Test with TiFlash compute label under NextGen kernel",
            store: MetaStoreDraft {
                labels: vec![label("engine", "tiflash_compute")],
            },
            want: false,
        },
        EngineCase {
            name: "Test without TiFlash label",
            store: MetaStoreDraft {
                labels: vec![label("engine", "not_tiflash")],
            },
            want: false,
        },
        EngineCase {
            name: "Test with no labels",
            store: MetaStoreDraft { labels: vec![] },
            want: false,
        },
    ];

    for tt in tests {
        assert_eq!(tt.want, IsTiFlashWriteHTTPResp(&tt.store), "{}", tt.name);
    }
}
