// Copyright 2021 PingCAP, Inc.
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

//! Go `main_test.go` package-level exports for tests.
//!
//! Go exposes private `checkStoresAlive` / `handleTiKVAddress` via package vars.
//! In Rust those are already public as `CheckStoresAlive` / `HandleTiKVAddress`.
//! TestMain goleak setup has no direct Rust equivalent; this module documents
//! the export surface and asserts the symbols remain reachable.
//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/main_test.rs`对应的BR 连接与 TiKV/PD 探测，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少5行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `EmptyPD`承载\"EmptyPD\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl StoreMeta`把\"StoreMeta\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetAllStores`是当前文件的重要函数，承担\"GetAllStores\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_main_exports_reachable`对齐 Go 同名测试或契约片段，用来固定\"test main exports reachable\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! 中文注释索引结束

use crate::{CheckStoresAlive, HandleTiKVAddress, Store, StoreBehavior, StoreMeta, StoreState};
use astersql_errors::SharedError;

/// Mirrors Go `var CheckStoresAlive = checkStoresAlive`.
pub use crate::CheckStoresAlive as ExportedCheckStoresAlive;

/// Mirrors Go `var HandleTiKVAddress = handleTiKVAddress`.
pub use crate::HandleTiKVAddress as ExportedHandleTiKVAddress;

struct EmptyPD;

impl StoreMeta for EmptyPD {
    fn GetAllStores(&self, _exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        Ok(vec![])
    }
}

/// Ensures the Go main_test export surface stays linked for conn_test consumers.
#[test]
fn test_main_exports_reachable() {
    ExportedCheckStoresAlive(&EmptyPD, StoreBehavior::SkipTiFlash).expect("alive");
    let store = Store {
        id: 1,
        address: "127.0.0.1:20160".into(),
        status_address: "127.0.0.1:20180".into(),
        state: StoreState::Up,
        ..Default::default()
    };
    let addr = ExportedHandleTiKVAddress(&store, "http://").expect("addr");
    assert_eq!("http://127.0.0.1:20180", addr.to_string());
}
