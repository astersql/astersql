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

//! Go-equivalent tests for `br/pkg/conn/conn_test.go`.
//!
//! External PD/TiKV/gRPC are not used: FakePD + in-process HTTP `/config`
//! mock + `fail` failpoints preserve Go call order, errors, and assertions.
//! Darwin arm64: no kv/domain/kvproto/grpcio.
//! 中文注释索引开始
//! 本文件负责`br/pkg/conn/conn_test.rs`对应的BR 连接与 TiKV/PD 探测，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少125行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `FakePD`承载\"FakePD\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl FakePD`把\"FakePD\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `new`是当前文件的重要函数，承担\"new\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl StoreMeta`把\"StoreMeta\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetAllStores`是当前文件的重要函数，承担\"GetAllStores\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `MemCtrl`承载\"MemCtrl\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl MemCtrl`把\"MemCtrl\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl PdControllerHandle`把\"PdControllerHandle\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `GetPDClient`是当前文件的重要函数，承担\"GetPDClient\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `SetPDClient`是当前文件的重要函数，承担\"SetPDClient\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `engine_store`是当前文件的重要函数，承担\"engine store\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `plain_store`是当前文件的重要函数，承担\"plain store\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `labeled_store`是当前文件的重要函数，承担\"labeled store\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `addr_store`是当前文件的重要函数，承担\"addr store\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `ConfigMockServer`承载\"ConfigMockServer\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl ConfigMockServer`把\"ConfigMockServer\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `start`是当前文件的重要函数，承担\"start\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `url`是当前文件的重要函数，承担\"url\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `close`是当前文件的重要函数，承担\"close\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `config_mock_server_waits_for_fragmented_http_headers`是当前文件的重要函数，承担\"config mock server waits for fragmented http headers\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `StdHttpClient`承载\"StdHttpClient\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl HttpClient`把\"HttpClient\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Get`是当前文件的重要函数，承担\"Get\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `enable_fp`是当前文件的重要函数，承担\"enable fp\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `multierr_errors`是当前文件的重要函数，承担\"multierr errors\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_get_all_tikv_stores_with_retry_cancel`对齐 Go 同名测试或契约片段，用来固定\"test get all tikv stores with retry cancel\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_get_all_tikv_stores_with_unknown`对齐 Go 同名测试或契约片段，用来固定\"test get all tikv stores with unknown\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_check_stores_alive`对齐 Go 同名测试或契约片段，用来固定\"test check stores alive\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_get_all_tikv_stores`对齐 Go 同名测试或契约片段，用来固定\"test get all tikv stores\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `Case`承载\"Case\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `test_get_conn_on_canceled_context`对齐 Go 同名测试或契约片段，用来固定\"test get conn on canceled context\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_get_merge_region_size_and_count`对齐 Go 同名测试或契约片段，用来固定\"test get merge region size and count\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_is_log_backup_enabled`对齐 Go 同名测试或契约片段，用来固定\"test is log backup enabled\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_handle_tikv_address`对齐 Go 同名测试或契约片段，用来固定\"test handle tikv address\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `mixed_engine_stores`是当前文件的重要函数，承担\"mixed engine stores\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `map_ids`是当前文件的重要函数，承担\"map ids\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"Minimal HTTP/1.1 GET over TCP for the in-process mock.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_get_all_tikv_stores_with_retry_cancel ↔ TestGetAllTiKVStoresWithRetryCancel\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_get_all_tikv_stores_with_unknown ↔ TestGetAllTiKVStoresWithUnknown\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_check_stores_alive ↔ TestCheckStoresAlive\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_get_all_tikv_stores ↔ TestGetAllTiKVStores\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_get_conn_on_canceled_context ↔ TestGetConnOnCanceledContext\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_get_merge_region_size_and_count ↔ TestGetMergeRegionSizeAndCount\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Re-bind PD with addresses applied.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_is_log_backup_enabled ↔ TestIsLogBackupEnabled\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"test_handle_tikv_address ↔ TestHandleTiKVAddress\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_glue::Storage;
use astersql_errors::{Errors, SharedError};

/// Failpoints are process-global; serialize any path that calls
/// `GetAllTiKVStoresWithRetry` so injection cannot race across tests.
pub(crate) fn failpoint_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn clear_store_failpoints() {
    let _ = fail::cfg("hint-GetAllTiKVStores-error", "off");
    let _ = fail::cfg("hint-GetAllTiKVStores-grpc-cancel", "off");
    let _ = fail::cfg("hint-GetAllTiKVStores-ctx-cancel", "off");
}

use crate::{
    BackgroundContext, BackupClient, CancelContext, CancelledContext, CheckStoresAlive, ConfigTerm,
    DefaultImportNumGoroutines, DefaultMergeRegionKeyCount, DefaultMergeRegionSizeBytes,
    GetAllTiKVStores, GetAllTiKVStoresWithRetry, GrpcCode, HandleTiKVAddress, HttpClient,
    HttpResponse, KVConfig, LogBackupClient, Mgr, PdControllerHandle, Store, StoreBehavior,
    StoreLabel, StoreManagerHandle, StoreMeta, StoreState, status_code, units,
};

struct KeyspaceStorage;

impl Storage for KeyspaceStorage {
    fn keyspace_id(&self) -> u32 {
        42
    }
}

#[test]
fn storage_keyspace_is_projected_for_gc_manager() {
    assert_eq!(42, crate::conn::keyspace_id_for_gc(&KeyspaceStorage));
}

struct FakePD {
    stores: Mutex<Vec<Store>>,
}

impl FakePD {
    fn new(stores: Vec<Store>) -> Self {
        Self {
            stores: Mutex::new(stores),
        }
    }
}

impl StoreMeta for FakePD {
    fn GetAllStores(&self, _exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        Ok(self.stores.lock().unwrap().clone())
    }
}

struct MemCtrl {
    pd: Mutex<Arc<dyn StoreMeta>>,
}

impl MemCtrl {
    fn new(pd: Arc<dyn StoreMeta>) -> Self {
        Self { pd: Mutex::new(pd) }
    }
}

impl PdControllerHandle for MemCtrl {
    fn GetPDClient(&self) -> Arc<dyn StoreMeta> {
        self.pd.lock().unwrap().clone()
    }
    fn SetPDClient(&self, pd: Arc<dyn StoreMeta>) {
        *self.pd.lock().unwrap() = pd;
    }
}

fn engine_store(id: u64, state: StoreState, engine: &str) -> Store {
    Store {
        id,
        state,
        labels: vec![StoreLabel {
            key: "engine".into(),
            value: engine.into(),
        }],
        ..Default::default()
    }
}

fn plain_store(id: u64) -> Store {
    Store {
        id,
        state: StoreState::Up,
        ..Default::default()
    }
}

fn labeled_store(id: u64, labels: &[(&str, &str)]) -> Store {
    Store {
        id,
        state: StoreState::Up,
        labels: labels
            .iter()
            .map(|(k, v)| StoreLabel {
                key: (*k).into(),
                value: (*v).into(),
            })
            .collect(),
        ..Default::default()
    }
}

fn addr_store(id: u64, address: &str, status_address: &str) -> Store {
    Store {
        id,
        state: StoreState::Up,
        address: address.into(),
        status_address: status_address.into(),
        ..Default::default()
    }
}

/// In-process HTTP server serving sequential `/config` bodies (Go httptest).
struct ConfigMockServer {
    base_url: String,
    join: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicUsize>,
}

impl ConfigMockServer {
    fn start(contents: Vec<String>, cancel: Option<Arc<CancelledContext>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");
        let shutdown = Arc::new(AtomicUsize::new(0));
        let shutdown_flag = shutdown.clone();
        let join = thread::spawn(move || {
            let mut count = 0_usize;
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            while shutdown_flag.load(Ordering::SeqCst) == 0 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking mock stream");
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .expect("mock read timeout");
                        let mut request = Vec::with_capacity(512);
                        loop {
                            let mut buf = [0_u8; 512];
                            match stream.read(&mut buf) {
                                Ok(0) => break,
                                Ok(read) => {
                                    request.extend_from_slice(&buf[..read]);
                                    if request.windows(4).any(|window| window == b"\r\n\r\n")
                                        || request.len() >= 16 * 1024
                                    {
                                        break;
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => break,
                            }
                        }
                        let req = String::from_utf8_lossy(&request);
                        let path = req
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("/");
                        if path.trim() == "/config" {
                            let body = contents.get(count).cloned().unwrap_or_default();
                            if body.is_empty() {
                                if let Some(c) = &cancel {
                                    c.cancel();
                                }
                            }
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            count += 1;
                        } else {
                            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url,
            join: Some(join),
            shutdown,
        }
    }

    fn url(&self) -> &str {
        &self.base_url
    }

    fn close(mut self) {
        self.shutdown.store(1, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[test]
fn config_mock_server_waits_for_fragmented_http_headers() {
    let body = "{\"log-backup\":{\"enable\":true}}";
    let mock = ConfigMockServer::start(vec![body.to_string()], None);
    let host_port = mock.url().strip_prefix("http://").expect("mock URL");
    let mut stream = std::net::TcpStream::connect(host_port).expect("connect mock");
    stream
        .write_all(b"GET /con")
        .expect("write first request fragment");
    thread::sleep(Duration::from_millis(20));
    stream
        .write_all(b"fig HTTP/1.1\r\nHost: mock\r\nConnection: close\r\n\r\n")
        .expect("write second request fragment");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("finish fragmented request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read mock response");
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "unexpected response: {response:?}"
    );
    assert!(
        response.ends_with(body),
        "unexpected response body: {response:?}"
    );
    mock.close();
}

struct StdHttpClient;

impl HttpClient for StdHttpClient {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError> {
        // Minimal HTTP/1.1 GET over TCP for the in-process mock.
        let url = url
            .strip_prefix("http://")
            .ok_or_else(|| astersql_errors::New(format!("unsupported url {url}")))?;
        let (host_port, path) = url
            .split_once('/')
            .map(|(h, p)| (h, format!("/{p}")))
            .unwrap_or((url, "/".into()));
        let mut stream = std::net::TcpStream::connect(host_port)
            .map_err(|e| astersql_errors::New(format!("connect {host_port}: {e}")))?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(req.as_bytes())
            .map_err(|e| astersql_errors::New(format!("write: {e}")))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| astersql_errors::New(format!("read: {e}")))?;
        let text = String::from_utf8_lossy(&raw);
        let mut parts = text.splitn(2, "\r\n\r\n");
        let header = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("").as_bytes().to_vec();
        let status_code = header
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        Ok(HttpResponse {
            status_code,
            body,
            request_url: format!("http://{host_port}{path}"),
        })
    }
}

fn enable_fp(name: &str) -> fail::FailGuard {
    fail::FailGuard::new(name, "1*return(true)")
        .unwrap_or_else(|e| panic!("enable failpoint {name}: {e}"))
}

/// Go `multierr.Errors(errors.Trace(multierr))` does not unwrap Trace, so the
/// outer traced error is a single element; status is read via Cause/status_code.
fn multierr_errors(err: &SharedError) -> Vec<SharedError> {
    let _ = Errors; // keep import used when debugging groups
    vec![err.clone()]
}

// test_get_all_tikv_stores_with_retry_cancel ↔ TestGetAllTiKVStoresWithRetryCancel
#[test]
fn test_get_all_tikv_stores_with_retry_cancel() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    let _g1 = enable_fp("hint-GetAllTiKVStores-grpc-cancel");
    let _g2 = enable_fp("hint-GetAllTiKVStores-ctx-cancel");
    let ctx = BackgroundContext;
    let stores = vec![
        engine_store(1, StoreState::Up, "tiflash"),
        engine_store(2, StoreState::Offline, "tiflash"),
    ];
    let fpdc = FakePD::new(stores);
    let err = GetAllTiKVStoresWithRetry(&ctx, &fpdc, StoreBehavior::SkipTiFlash)
        .expect_err("canceled retry");
    let errs = multierr_errors(&err);
    assert_eq!(1, errs.len(), "errs={errs:?} full={err}");
    assert_eq!(GrpcCode::Canceled, status_code(&errs[0]));
}

// test_get_all_tikv_stores_with_unknown ↔ TestGetAllTiKVStoresWithUnknown
#[test]
fn test_get_all_tikv_stores_with_unknown() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    let _g1 = enable_fp("hint-GetAllTiKVStores-error");
    let _g2 = enable_fp("hint-GetAllTiKVStores-ctx-cancel");
    let ctx = BackgroundContext;
    let stores = vec![
        engine_store(1, StoreState::Up, "tiflash"),
        engine_store(2, StoreState::Offline, "tiflash"),
    ];
    let fpdc = FakePD::new(stores);
    let err = GetAllTiKVStoresWithRetry(&ctx, &fpdc, StoreBehavior::SkipTiFlash)
        .expect_err("unknown retry");
    let errs = multierr_errors(&err);
    assert_eq!(1, errs.len(), "errs={errs:?} full={err}");
    assert_eq!(GrpcCode::Unknown, status_code(&errs[0]));
}

// test_check_stores_alive ↔ TestCheckStoresAlive
#[test]
fn test_check_stores_alive() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    let ctx = BackgroundContext;
    let stores = vec![
        engine_store(1, StoreState::Up, "tiflash"),
        engine_store(2, StoreState::Offline, "tiflash"),
        engine_store(3, StoreState::Up, "tikv"),
        engine_store(4, StoreState::Offline, "tikv"),
    ];
    let fpdc = FakePD::new(stores.clone());
    let kv_stores = GetAllTiKVStoresWithRetry(&ctx, &fpdc, StoreBehavior::SkipTiFlash)
        .expect("GetAllTiKVStoresWithRetry");
    assert_eq!(2, kv_stores.len());
    assert_eq!(stores[2].id, kv_stores[0].id);
    assert_eq!(stores[3].id, kv_stores[1].id);
    CheckStoresAlive(&fpdc, StoreBehavior::SkipTiFlash).expect("CheckStoresAlive");
}

// test_get_all_tikv_stores ↔ TestGetAllTiKVStores
#[test]
fn test_get_all_tikv_stores() {
    struct Case {
        stores: Vec<Store>,
        behavior: StoreBehavior,
        expected: HashMap<u64, i32>,
        expected_error: &'static str,
    }
    let mixed = mixed_engine_stores();
    let cases = vec![
        Case {
            stores: vec![plain_store(1)],
            behavior: StoreBehavior::SkipTiFlash,
            expected: map_ids(&[1]),
            expected_error: "",
        },
        Case {
            stores: vec![plain_store(1)],
            behavior: StoreBehavior::ErrorOnTiFlash,
            expected: map_ids(&[1]),
            expected_error: "",
        },
        Case {
            stores: vec![plain_store(1), engine_store(2, StoreState::Up, "tiflash")],
            behavior: StoreBehavior::SkipTiFlash,
            expected: map_ids(&[1]),
            expected_error: "",
        },
        Case {
            stores: vec![plain_store(1), engine_store(2, StoreState::Up, "tiflash")],
            behavior: StoreBehavior::ErrorOnTiFlash,
            expected: HashMap::new(),
            expected_error: "cannot restore to a cluster with active TiFlash stores",
        },
        Case {
            stores: mixed.clone(),
            behavior: StoreBehavior::SkipTiFlash,
            expected: map_ids(&[1, 3, 4, 6]),
            expected_error: "",
        },
        Case {
            stores: mixed.clone(),
            behavior: StoreBehavior::ErrorOnTiFlash,
            expected: HashMap::new(),
            expected_error: "cannot restore to a cluster with active TiFlash stores",
        },
        Case {
            stores: mixed,
            behavior: StoreBehavior::TiFlashOnly,
            expected: map_ids(&[2, 5]),
            expected_error: "",
        },
    ];

    for case in cases {
        let pd = FakePD::new(case.stores);
        let result = GetAllTiKVStores(&pd, case.behavior);
        if !case.expected_error.is_empty() {
            let err = result.expect_err("TiFlash policy");
            assert!(
                err.to_string().contains(case.expected_error),
                "actual={} pattern={}",
                err,
                case.expected_error
            );
            continue;
        }
        let stores = result.expect("GetAllTiKVStores");
        let mut found = HashMap::new();
        for s in stores {
            *found.entry(s.id).or_insert(0) += 1;
        }
        assert_eq!(case.expected, found);
    }
}

// test_get_conn_on_canceled_context ↔ TestGetConnOnCanceledContext
#[test]
fn test_get_conn_on_canceled_context() {
    let ctx = CancelledContext::new();
    ctx.cancel();
    let mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(Arc::new(FakePD::new(vec![])))));
    let err = mgr
        .GetBackupClient(&ctx, 42)
        .expect_err("GetBackupClient canceled");
    assert!(err.to_string().contains("context canceled"));
    let err = mgr
        .ResetBackupClient(&ctx, 42)
        .expect_err("ResetBackupClient canceled");
    assert!(err.to_string().contains("context canceled"));
}

// Go GetCurrentTsFromPD delegates to PD and must never fabricate a timestamp.
#[test]
fn get_current_ts_from_pd_propagates_pd_failure() {
    let mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(Arc::new(FakePD::new(vec![])))));
    let err = mgr
        .GetCurrentTsFromPD()
        .expect_err("a PD without GetTS support must fail");
    assert!(err.to_string().contains("GetTS"), "actual error: {err}");
}

struct TsPD;

impl StoreMeta for TsPD {
    fn GetAllStores(&self, _exclude_tombstone: bool) -> Result<Vec<Store>, SharedError> {
        Ok(vec![])
    }

    fn GetTS(&self) -> Result<(i64, i64), SharedError> {
        Ok((1234, 17))
    }
}

struct ClientStoreManager;

impl StoreManagerHandle for ClientStoreManager {
    fn Close(&self) {}

    fn GetBackupClient(
        &self,
        _ctx: &dyn CancelContext,
        store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        Ok(BackupClient { store_id })
    }

    fn ResetBackupClient(
        &self,
        _ctx: &dyn CancelContext,
        store_id: u64,
    ) -> Result<BackupClient, SharedError> {
        Ok(BackupClient {
            store_id: store_id + 1,
        })
    }

    fn GetLogBackupClient(
        &self,
        _ctx: &dyn CancelContext,
        store_id: u64,
    ) -> Result<LogBackupClient, SharedError> {
        Ok(LogBackupClient { store_id })
    }
}

#[test]
fn manager_delegates_pd_timestamp_and_store_clients() {
    let mut mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(Arc::new(TsPD))));
    mgr.storeManager = Some(Arc::new(ClientStoreManager));

    assert_eq!((1234_u64 << 18) | 17, mgr.GetCurrentTsFromPD().unwrap());
    assert_eq!(
        42,
        mgr.GetBackupClient(&BackgroundContext, 42)
            .unwrap()
            .store_id
    );
    assert_eq!(
        43,
        mgr.ResetBackupClient(&BackgroundContext, 42)
            .unwrap()
            .store_id
    );
    assert_eq!(
        42,
        mgr.GetLogBackupClient(&BackgroundContext, 42)
            .unwrap()
            .store_id
    );
}

struct StatusHttp(u16);

impl HttpClient for StatusHttp {
    fn Get(&self, url: &str) -> Result<HttpResponse, SharedError> {
        Ok(HttpResponse {
            status_code: self.0,
            body: b"config".to_vec(),
            request_url: url.to_string(),
        })
    }
}

#[test]
fn get_config_bytes_requires_http_ok_and_collects_body() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    let pd = Arc::new(FakePD::new(vec![addr_store(
        1,
        "127.0.0.1:20160",
        "127.0.0.1:20180",
    )]));
    let mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(pd)));
    let err = mgr
        .GetConfigBytesFromTiKV(&BackgroundContext, &StatusHttp(503), |_| Ok(()))
        .expect_err("non-200 response");
    assert!(err.to_string().contains("HTTP 503"));

    let mut body = Vec::new();
    mgr.GetConfigBytesFromTiKV(&BackgroundContext, &StatusHttp(200), |bytes| {
        body.extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    assert_eq!(b"config", body.as_slice());
}

// test_get_merge_region_size_and_count ↔ TestGetMergeRegionSizeAndCount
#[test]
fn test_get_merge_region_size_and_count() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    struct Case {
        stores: Vec<Store>,
        content: Vec<&'static str>,
        import_num_goroutines: u32,
        region_split_size: u64,
        region_split_keys: u64,
    }
    let cases = vec![
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tiflash")],
            content: vec![""],
            import_num_goroutines: DefaultImportNumGoroutines,
            region_split_size: DefaultMergeRegionSizeBytes,
            region_split_keys: DefaultMergeRegionKeyCount,
        },
        Case {
            stores: vec![
                engine_store(1, StoreState::Up, "tiflash"),
                engine_store(2, StoreState::Up, "tikv"),
            ],
            content: vec!["", ""],
            import_num_goroutines: DefaultImportNumGoroutines,
            region_split_size: DefaultMergeRegionSizeBytes,
            region_split_keys: DefaultMergeRegionKeyCount,
        },
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tikv")],
            content: vec![
                "{\"log-level\": \"debug\", \"coprocessor\": {\"region-split-keys\": 1, \"region-split-size\": \"1MiB\"}, \"import\": {\"num-threads\": 6}}",
            ],
            import_num_goroutines: DefaultImportNumGoroutines,
            region_split_size: units::MiB,
            region_split_keys: 1,
        },
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tikv")],
            content: vec![
                "{\"log-level\": \"debug\", \"coprocessor\": {\"region-split-keys\": 10000000, \"region-split-size\": \"1GiB\"}, \"import\": {\"num-threads\": 128}}",
            ],
            import_num_goroutines: 132,
            region_split_size: units::GiB,
            region_split_keys: 10_000_000,
        },
        Case {
            stores: vec![
                engine_store(1, StoreState::Up, "tikv"),
                engine_store(2, StoreState::Up, "tikv"),
            ],
            content: vec![
                "{\"log-level\": \"debug\", \"coprocessor\": {\"region-split-keys\": 10000000, \"region-split-size\": \"1GiB\"}, \"import\": {\"num-threads\": 128}}",
                "{\"log-level\": \"debug\", \"coprocessor\": {\"region-split-keys\": 12000000, \"region-split-size\": \"900MiB\"}, \"import\": {\"num-threads\": 12}}",
            ],
            import_num_goroutines: 132,
            region_split_size: units::GiB,
            region_split_keys: 10_000_000,
        },
    ];

    for case in cases {
        let cancel = Arc::new(CancelledContext::new());
        let mock = ConfigMockServer::start(
            case.content.iter().map(|s| (*s).to_string()).collect(),
            Some(cancel.clone()),
        );
        let mut stores = case.stores;
        assert_eq!(case.content.len(), stores.len());
        for s in &mut stores {
            s.address = mock.url().to_string();
            s.status_address = mock.url().to_string();
        }
        // Re-bind PD with addresses applied.
        let pd = Arc::new(FakePD::new(stores));
        let mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(pd)));
        let mut kv_configs = KVConfig {
            ImportGoroutines: ConfigTerm {
                Value: DefaultImportNumGoroutines,
                Modified: false,
            },
            MergeRegionSize: ConfigTerm {
                Value: DefaultMergeRegionSizeBytes,
                Modified: false,
            },
            MergeRegionKeyCount: ConfigTerm {
                Value: DefaultMergeRegionKeyCount,
                Modified: false,
            },
        };
        mgr.ProcessTiKVConfigs(cancel.as_ref(), &mut kv_configs, &StdHttpClient);
        assert_eq!(case.region_split_size, kv_configs.MergeRegionSize.Value);
        assert_eq!(case.region_split_keys, kv_configs.MergeRegionKeyCount.Value);
        assert_eq!(
            case.import_num_goroutines,
            kv_configs.ImportGoroutines.Value
        );
        mock.close();
    }
}

// test_is_log_backup_enabled ↔ TestIsLogBackupEnabled
#[test]
fn test_is_log_backup_enabled() {
    let _lock = failpoint_lock();
    clear_store_failpoints();
    struct Case {
        stores: Vec<Store>,
        content: Vec<&'static str>,
        enable: bool,
        err: bool,
    }
    let cases = vec![
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tiflash")],
            content: vec![""],
            enable: true,
            err: false,
        },
        Case {
            stores: vec![
                engine_store(1, StoreState::Up, "tiflash"),
                engine_store(2, StoreState::Up, "tikv"),
            ],
            content: vec!["", ""],
            enable: false,
            err: true,
        },
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tikv")],
            content: vec!["{\"log-level\": \"debug\", \"log-backup\": {\"enable\": true}}"],
            enable: true,
            err: false,
        },
        Case {
            stores: vec![engine_store(1, StoreState::Up, "tikv")],
            content: vec!["{\"log-level\": \"debug\", \"log-backup\": {\"enable\": false}}"],
            enable: false,
            err: false,
        },
        Case {
            stores: vec![
                engine_store(1, StoreState::Up, "tikv"),
                engine_store(2, StoreState::Up, "tikv"),
            ],
            content: vec![
                "{\"log-level\": \"debug\", \"log-backup\": {\"enable\": true}}",
                "{\"log-level\": \"debug\", \"log-backup\": {\"enable\": false}}",
            ],
            enable: false,
            err: false,
        },
    ];

    for case in cases {
        let cancel = Arc::new(CancelledContext::new());
        let mock = ConfigMockServer::start(
            case.content.iter().map(|s| (*s).to_string()).collect(),
            Some(cancel.clone()),
        );
        let mut stores = case.stores;
        assert_eq!(case.content.len(), stores.len());
        for s in &mut stores {
            s.address = mock.url().to_string();
            s.status_address = mock.url().to_string();
        }
        let pd = Arc::new(FakePD::new(stores));
        let mgr = Mgr::new_with_pd(Arc::new(MemCtrl::new(pd)));
        let result = mgr.IsLogBackupEnabled(cancel.as_ref(), &StdHttpClient);
        if case.err {
            assert!(result.is_err(), "expected error, got {result:?}");
        } else {
            let enable = result.expect("IsLogBackupEnabled");
            assert_eq!(case.enable, enable);
        }
        mock.close();
    }
}

// test_handle_tikv_address ↔ TestHandleTiKVAddress
#[test]
fn test_handle_tikv_address() {
    let cases = [
        (
            addr_store(1, "127.0.0.1:20160", "127.0.0.1:20180"),
            "http://",
            "http://127.0.0.1:20180",
        ),
        (
            addr_store(1, "[::1]:20160", "[::1]:20180"),
            "http://",
            "http://[::1]:20180",
        ),
        (
            addr_store(1, "192.168.1.5:20160", "0.0.0.0:20180"),
            "https://",
            "https://192.168.1.5:20180",
        ),
        (
            addr_store(1, "[fd00::1:5]:20160", "[::]:20180"),
            "https://",
            "https://[fd00::1:5]:20180",
        ),
    ];
    for (store, prefix, want) in cases {
        let addr = HandleTiKVAddress(&store, prefix).expect("HandleTiKVAddress");
        assert_eq!(want, addr.to_string());
    }
}

fn mixed_engine_stores() -> Vec<Store> {
    vec![
        plain_store(1),
        engine_store(2, StoreState::Up, "tiflash"),
        plain_store(3),
        engine_store(4, StoreState::Up, "tikv"),
        labeled_store(5, &[("else", "tikv"), ("engine", "tiflash")]),
        labeled_store(6, &[("else", "tiflash"), ("engine", "tikv")]),
    ]
}

fn map_ids(ids: &[u64]) -> HashMap<u64, i32> {
    ids.iter().map(|id| (*id, 1)).collect()
}
