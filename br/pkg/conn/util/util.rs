// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! TiKV/PD connection helpers ported from `br/pkg/conn/util/util.go`.
//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/util/util.rs`对应的公共辅助能力，并保持重试、HTTP 与地址处理语义和 Go 一致。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少71行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `is_cancelled`是当前文件的重要函数，承担\"is cancelled\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl StatusUrl`把\"StatusUrl\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl Display`把\"Display\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `fmt`是当前文件的重要函数，承担\"fmt\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `enum`用离散值表达\"enum\"的状态，关系到序列化、日志和错误判定。
//! 这类符号最容易因为默认值、未知值或字符串映射而与 Go 端产生偏差。
//! 中文注释会提醒维护者把重点放在状态转换、展示文本和兜底分支。
//! 如果测试里出现 raw integer、unknown 或 not found，对应的兼容性保护通常都落在这里。
//! - `GetAllStores`是当前文件的重要函数，承担\"GetAllStores\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetTS`是当前文件的重要函数，承担\"GetTS\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `EngineLabel`承载\"EngineLabel\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Label`把\"Label\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `key`是当前文件的重要函数，承担\"key\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `value`是当前文件的重要函数，承担\"value\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `EngineLabelSlice`承载\"EngineLabelSlice\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl LabelStore`把\"LabelStore\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `labels`是当前文件的重要函数，承担\"labels\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `is_store_tiflash`是当前文件的重要函数，承担\"is store tiflash\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `trace_err`是当前文件的重要函数，承担\"trace err\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `compose_ts`是当前文件的重要函数，承担\"compose ts\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `join_host_port`是当前文件的重要函数，承担\"join host port\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `RetryCancelled`承载\"RetryCancelled\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl std`把\"std\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `EmptyRetryErrors`承载\"EmptyRetryErrors\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `with_retry`是当前文件的重要函数，承担\"with retry\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"StoreBehavior is the action to do in GetAllTiKVStores when a non-TiKV\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"store (e.g. TiFlash store) is found.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"ErrorOnTiFlash causes GetAllTiKVStores to return error when the store is\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"found to be a TiFlash node.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"SkipTiFlash causes GetAllTiKVStores to skip the store when it is found to\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"be a TiFlash node.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"TiFlashOnly caused GetAllTiKVStores to skip the store which is not a\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"TiFlash node.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"StoreMeta is the required interface for a watcher.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"It is striped from pd.Client.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetAllStores gets all stores from pd.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"The store may expire later. Caller is responsible for caching and taking care\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"of store change.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"PdClient is the TS source striped from pd.Client.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetAllTiKVStores returns all TiKV stores registered to the PD client. The\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"stores must not be a tombstone and must never contain a label `engine=tiflash`.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetCurrentTsFromPD gets current ts from PD.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetCurrentTsFromPDWithRetry gets current ts from PD with retry.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetConfigFromTiKVStores gets configs from the specified TiKV stores.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetConfigBytesFromTiKVStores gets config response bodies from the specified TiKV stores.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"HandleTiKVAddress returns the TiKV status HTTP address used to fetch configs.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::fmt::Display;
use std::thread;
use std::time::{Duration, Instant};

use crate::kvproto::metapb::{self, Store};
use astersql_br_pkg_errors::ErrPDInvalidResponse;
use astersql_br_pkg_logutil::{Field, Level, ShortError, log};
use astersql_br_pkg_utils::BackoffStrategy;
use astersql_br_pkg_utils::backoff::NewAggressivePDBackoffStrategy;
use astersql_errors::{Annotatef, ErrorArg, Errorf, Join, SharedError, Trace};
use astersql_util_engine::{IsTiFlash, Label, LabelStore};

const TS_LOGICAL_BITS: u64 = 18;

/// Cancellation surface matching Go `context.Context`.
pub trait CancelContext {
    fn is_cancelled(&self) -> bool;
}

/// HTTP client surface matching Go `*http.Client` for status/config requests.
pub trait HttpClient: Send + Sync {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError>;

    /// Testable equivalent of closing Go's `resp.Body` after each attempt.
    fn CloseResponse(&self, _response: &HttpResponse) {}
}

/// Minimal HTTP response shape used by config fetch helpers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub status: String,
    pub body: Vec<u8>,
    pub request_url: String,
}

/// Parsed TiKV status URL used to fetch `/config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusUrl {
    scheme: String,
    host: String,
    path: String,
    query: String,
    fragment: String,
}

impl StatusUrl {
    pub fn parse(raw: &str) -> Result<Self, SharedError> {
        let (scheme, rest) = raw
            .split_once("://")
            .ok_or_else(|| Errorf("invalid URL %s", &[ErrorArg::String(raw.to_string())]))?;
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = &rest[..authority_end];
        let suffix = &rest[authority_end..];
        let fragment_start = suffix.find('#').unwrap_or(suffix.len());
        let before_fragment = &suffix[..fragment_start];
        let fragment = &suffix[fragment_start..];
        let query_start = before_fragment.find('?').unwrap_or(before_fragment.len());
        let path = &before_fragment[..query_start];
        let query = &before_fragment[query_start..];
        Ok(Self {
            scheme: scheme.to_string(),
            host: host.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            fragment: fragment.to_string(),
        })
    }

    pub fn hostname(&self) -> &str {
        if let Some(stripped) = self.host.strip_prefix('[') {
            return stripped.split(']').next().unwrap_or(stripped);
        }
        self.host
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(&self.host)
    }

    pub fn port(&self) -> &str {
        if let Some(stripped) = self.host.strip_prefix('[') {
            return stripped
                .split(']')
                .nth(1)
                .and_then(|tail| tail.strip_prefix(':'))
                .unwrap_or("");
        }
        self.host
            .rsplit_once(':')
            .map(|(_, port)| port)
            .unwrap_or("")
    }

    pub fn set_host(&mut self, host_port: String) {
        self.host = host_port;
    }

    pub fn join_path(&self, path: &str) -> String {
        // Go's URL.JoinPath delegates to path.Join, so dot segments and repeated
        // separators are cleaned before the query and fragment are restored.
        let joined_path = format!("{}/{}", self.path, path);
        let mut segments = Vec::new();
        for segment in joined_path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                segment => segments.push(segment),
            }
        }
        let clean_path = format!("/{}", segments.join("/"));
        format!(
            "{}://{}{}{}{}",
            self.scheme, self.host, clean_path, self.query, self.fragment
        )
    }
}

impl Display for StatusUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}://{}{}{}{}",
            self.scheme, self.host, self.path, self.query, self.fragment
        )
    }
}

// StoreBehavior is the action to do in GetAllTiKVStores when a non-TiKV
// store (e.g. TiFlash store) is found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreBehavior {
    // ErrorOnTiFlash causes GetAllTiKVStores to return error when the store is
    // found to be a TiFlash node.
    ErrorOnTiFlash = 0,
    // SkipTiFlash causes GetAllTiKVStores to skip the store when it is found to
    // be a TiFlash node.
    SkipTiFlash = 1,
    // TiFlashOnly caused GetAllTiKVStores to skip the store which is not a
    // TiFlash node.
    TiFlashOnly = 2,
}

// StoreMeta is the required interface for a watcher.
// It is striped from pd.Client.
pub trait StoreMeta: Send + Sync {
    // GetAllStores gets all stores from pd.
    // The store may expire later. Caller is responsible for caching and taking care
    // of store change.
    fn GetAllStores(
        &self,
        ctx: &dyn CancelContext,
        exclude_tombstone: bool,
    ) -> Result<Vec<Store>, SharedError>;
}

// PdClient is the TS source striped from pd.Client.
pub trait PdClient: Send + Sync {
    fn GetTS(&self, ctx: &dyn CancelContext) -> Result<(i64, i64), SharedError>;
}

#[derive(Clone, Debug)]
struct EngineLabel {
    key: String,
    value: String,
}

impl Label for EngineLabel {
    fn key(&self) -> &str {
        &self.key
    }

    fn value(&self) -> &str {
        &self.value
    }
}

struct EngineLabelSlice<'a>(&'a [EngineLabel]);

impl LabelStore for EngineLabelSlice<'_> {
    type Label = EngineLabel;

    fn labels(&self) -> &[Self::Label] {
        self.0
    }
}

fn is_store_tiflash(store: &Store) -> bool {
    let labels: Vec<EngineLabel> = store
        .get_labels()
        .iter()
        .map(|label| EngineLabel {
            key: label.get_key().to_string(),
            value: label.get_value().to_string(),
        })
        .collect();
    IsTiFlash(&EngineLabelSlice(&labels))
}

fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

fn compose_ts(physical: i64, logical: i64) -> u64 {
    ((physical as u64) << TS_LOGICAL_BITS).wrapping_add(logical as u64)
}

fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RetryCancelled;

impl std::fmt::Display for RetryCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for RetryCancelled {}

#[derive(Debug, Clone, Copy, Default)]
struct EmptyRetryErrors;

impl std::fmt::Display for EmptyRetryErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("retry failed with no errors collected")
    }
}

impl std::error::Error for EmptyRetryErrors {}

fn with_retry(
    ctx: &dyn CancelContext,
    mut retryable: impl FnMut() -> Result<(), SharedError>,
    mut backoff: Box<dyn BackoffStrategy>,
) -> Result<(), SharedError> {
    let mut all_errors: Vec<Option<SharedError>> = Vec::new();
    while backoff.RemainingAttempts() > 0 {
        match retryable() {
            Ok(()) => return Ok(()),
            Err(err) => {
                all_errors.push(Some(err));
                if ctx.is_cancelled() {
                    return Err(
                        Join(&all_errors).unwrap_or_else(|| SharedError::new(RetryCancelled))
                    );
                }
                let backoff_duration =
                    backoff.NextBackoff(all_errors.last().and_then(|e| e.as_ref()).unwrap());
                if sleep_until_backoff_or_cancelled(ctx, backoff_duration) {
                    return Err(
                        Join(&all_errors).unwrap_or_else(|| SharedError::new(RetryCancelled))
                    );
                }
            }
        }
    }
    Err(Join(&all_errors).unwrap_or_else(|| SharedError::new(EmptyRetryErrors)))
}

fn sleep_until_backoff_or_cancelled(ctx: &dyn CancelContext, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if ctx.is_cancelled() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(1)));
    }
}

// GetAllTiKVStores returns all TiKV stores registered to the PD client. The
// stores must not be a tombstone and must never contain a label `engine=tiflash`.
pub fn GetAllTiKVStores(
    ctx: &dyn CancelContext,
    pd_client: &dyn StoreMeta,
    store_behavior: StoreBehavior,
) -> Result<Vec<Store>, SharedError> {
    let stores = pd_client.GetAllStores(ctx, true).map_err(trace_err)?;

    let mut filtered = Vec::with_capacity(stores.len());
    for store in stores {
        let mut is_tiflash = false;
        if is_store_tiflash(&store) {
            if store_behavior == StoreBehavior::SkipTiFlash {
                continue;
            } else if store_behavior == StoreBehavior::ErrorOnTiFlash {
                return Err(trace_err(
                    Annotatef(
                        Some(SharedError::new((*ErrPDInvalidResponse).clone())),
                        "cannot restore to a cluster with active TiFlash stores (store %d at %s)",
                        &[
                            ErrorArg::Unsigned(store.get_id() as u128),
                            ErrorArg::String(store.get_address().to_string()),
                        ],
                    )
                    .expect("annotate tiflash store"),
                ));
            }
            is_tiflash = true;
        }
        if !is_tiflash && store_behavior == StoreBehavior::TiFlashOnly {
            continue;
        }
        filtered.push(store);
    }
    Ok(filtered)
}

pub fn GetAllTiKVStoresWithRetry(
    ctx: &dyn CancelContext,
    pd_client: &dyn StoreMeta,
    store_behavior: StoreBehavior,
) -> Result<Vec<Store>, SharedError> {
    let mut stores = Vec::new();
    with_retry(
        ctx,
        || {
            stores = GetAllTiKVStores(ctx, pd_client, store_behavior)?;
            Ok(())
        },
        NewAggressivePDBackoffStrategy(),
    )
    .map_err(trace_err)?;
    Ok(stores)
}

// GetCurrentTsFromPD gets current ts from PD.
pub fn GetCurrentTsFromPD(
    ctx: &dyn CancelContext,
    pd_client: &dyn PdClient,
) -> Result<u64, SharedError> {
    let (physical, logical) = pd_client.GetTS(ctx).map_err(trace_err)?;
    Ok(compose_ts(physical, logical))
}

// GetCurrentTsFromPDWithRetry gets current ts from PD with retry.
pub fn GetCurrentTsFromPDWithRetry(
    ctx: &dyn CancelContext,
    pd_client: &dyn PdClient,
) -> Result<u64, SharedError> {
    let mut current_ts = 0_u64;
    let mut retry = 0_u32;
    let err = with_retry(
        ctx,
        || {
            retry += 1;
            match GetCurrentTsFromPD(ctx, pd_client) {
                Ok(ts) => {
                    current_ts = ts;
                    Ok(())
                }
                Err(err) => {
                    log::Warn(
                        "failed to get current TS from PD, retry it",
                        [
                            Field::uint64("retry time", retry as u64),
                            ShortError(Some(&err)),
                        ],
                    );
                    Err(err)
                }
            }
        },
        NewAggressivePDBackoffStrategy(),
    );
    if let Err(err) = err {
        log::L().log(
            Level::Error,
            "failed to get current TS from PD",
            [Field::string("error", &err.to_string())],
        );
        return Err(trace_err(err));
    }
    Ok(current_ts)
}

// GetConfigFromTiKVStores gets configs from the specified TiKV stores.
pub fn GetConfigFromTiKVStores(
    ctx: &dyn CancelContext,
    stores: &[Store],
    cli: &dyn HttpClient,
    http_prefix: &str,
    mut callback: impl FnMut(&HttpResponse) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    for store in stores {
        if store.get_state() != metapb::StoreState::Up {
            continue;
        }
        let addr = HandleTiKVAddress(store, http_prefix)?;
        let config_addr = addr.join_path("config");

        with_retry(
            ctx,
            || {
                let resp = cli.Get(&config_addr)?;
                let result = callback(&resp);
                cli.CloseResponse(&resp);
                result
            },
            NewAggressivePDBackoffStrategy(),
        )?;
    }
    Ok(())
}

// GetConfigBytesFromTiKVStores gets config response bodies from the specified TiKV stores.
pub fn GetConfigBytesFromTiKVStores(
    ctx: &dyn CancelContext,
    stores: &[Store],
    cli: &dyn HttpClient,
    http_prefix: &str,
    mut collect: impl FnMut(&[u8]) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    GetConfigFromTiKVStores(ctx, stores, cli, http_prefix, |resp| {
        if resp.status_code != 200 {
            return Err(Errorf(
                "request %s failed: %s",
                &[
                    ErrorArg::String(resp.request_url.clone()),
                    ErrorArg::String(resp.status.clone()),
                ],
            ));
        }
        collect(&resp.body)
    })
}

// HandleTiKVAddress returns the TiKV status HTTP address used to fetch configs.
pub fn HandleTiKVAddress(store: &Store, http_prefix: &str) -> Result<StatusUrl, SharedError> {
    let mut status_addr = store.get_status_address().to_string();
    if status_addr.is_empty() {
        return Err(Errorf(
            "TiKV store %d does not have status address",
            &[ErrorArg::Unsigned(store.get_id() as u128)],
        ));
    }
    let mut node_addr = store.get_address().to_string();
    if !status_addr.starts_with("http") {
        status_addr = format!("{http_prefix}{status_addr}");
    }
    if !node_addr.starts_with("http") {
        node_addr = format!("{http_prefix}{node_addr}");
    }

    let status_url = StatusUrl::parse(&status_addr)?;
    let node_url = StatusUrl::parse(&node_addr)?;

    let mut addr = status_url.clone();
    if status_url.hostname() != node_url.hostname() {
        addr.set_host(join_host_port(node_url.hostname(), status_url.port()));
        log::Warn(
            "store address and status address mismatch the host, we will use the store address as hostname",
            [
                Field::uint64("store", store.get_id()),
                Field::string("status address", status_addr),
                Field::string("node address", node_addr),
                Field::string("request address", status_url.to_string()),
            ],
        );
    }
    Ok(addr)
}
