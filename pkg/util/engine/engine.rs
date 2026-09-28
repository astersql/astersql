// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 存储引擎标签识别：根据 store label 判断是否为 TiFlash 节点。
//
// 对应 Go `pkg/util/engine`。TiFlash 是列存分析引擎；PD（Placement Driver）
// 通过 store 上的 key/value 标签描述引擎类型。本模块用 `Label`/`LabelStore`
// 抽象，兼容 kvproto 与 PD HTTP 两种数据源。

/// A store label as exposed by kvproto and PD HTTP responses.
///
/// The trait keeps this package independent of either transport model. Concrete
/// protobuf and HTTP response types can implement it when their crates are wired
/// into the package-level module.
///
/// 存储节点标签（store label），来自 kvproto 或 PD HTTP 响应。
/// 通过 trait 抽象，使本包不依赖具体传输模型。
pub trait Label {
    /// 标签键，例如 `"engine"`、`"engine_role"`。
    fn key(&self) -> &str;
    /// 标签值，例如 `"tiflash"`、`"tiflash_compute"`。
    fn value(&self) -> &str;
}

/// A store containing the labels inspected by this package.
///
/// 带有可检查标签列表的存储节点抽象。
pub trait LabelStore {
    type Label: Label;

    /// 返回该 store 上的全部标签。
    fn labels(&self) -> &[Self::Label];
}

/// 判断单个标签是否标识 TiFlash（含 Classic 与 NextGen compute）。
fn is_tiflash_label(label: &impl Label) -> bool {
    label.key() == "engine" && matches!(label.value(), "tiflash_compute" | "tiflash")
}

// Under Classic kernel, TiFlash nodes have the label {"engine":"tiflash"}.
// Under NextGen kernel,
// - TiFlash write nodes have the label {"engine":"tiflash", "engine_role":"write"},
// - TiFlash compute nodes have the label {"engine":"tiflash_compute", "exclusive":"no-data"}, and no Region is stored on them.
// Classic：{"engine":"tiflash"}；NextGen 写节点另有 engine_role=write；
// compute 节点 engine=tiflash_compute，且不存放 Region（数据分片）。

/// Checks whether a kvproto store is based on the TiFlash engine.
///
/// 判断 kvproto store 是否基于 TiFlash 引擎（含 compute）。
#[allow(non_snake_case)]
pub fn IsTiFlash(store: &(impl LabelStore + ?Sized)) -> bool {
    store.labels().iter().any(is_tiflash_label)
}

/// Checks whether a store from a PD HTTP response is based on the TiFlash engine.
///
/// 判断 PD HTTP 响应中的 store 是否为 TiFlash（含 compute）。
#[allow(non_snake_case)]
pub fn IsTiFlashHTTPResp(store: &(impl LabelStore + ?Sized)) -> bool {
    store.labels().iter().any(is_tiflash_label)
}

/// Checks whether a PD HTTP store is a Classic TiFlash node or a NextGen
/// TiFlash write node. NextGen compute nodes are excluded.
///
/// 判断是否为 Classic TiFlash 或 NextGen 写节点；排除 `tiflash_compute`。
#[allow(non_snake_case)]
pub fn IsTiFlashWriteHTTPResp(store: &(impl LabelStore + ?Sized)) -> bool {
    // 仅匹配 engine=tiflash，不把 compute 节点算作写节点。
    store
        .labels()
        .iter()
        .any(|label| label.key() == "engine" && label.value() == "tiflash")
}
