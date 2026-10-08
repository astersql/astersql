# `pkg/util/mock/fortest.rs`

## 文件定位

[`fortest.rs`](./fortest.rs) 是 `astersql-util-mock` crate 的测试上下文工厂门面。crate 根模块 [`lib.rs`](./lib.rs) 通过 `mod fortest; pub use fortest::*;` 将本文件的 `NewContext` 公开到 crate 根，使调用方可以写 `mock_crate::NewContext()`，而不必依赖内部的 `context::newContext`。

文件名、注释和 Go 对照都把该 API 定位为测试辅助设施。不过 Rust 模块当前没有 `#[cfg(test)]`：只要依赖 `astersql-util-mock`，`NewContext` 就会被编译并公开。因此“仅测试使用”是 API 约定，不是 Rust 编译器强制的可见性边界。

## 核心职责

本文件只承担一项职责：把 crate 内部构造器 `context.rs::newContext` 包装为公开的 `NewContext() -> Box<Context>`。它不自行配置会话变量、不创建 Store，也不实现事务、值存取或取消逻辑；这些行为都属于 [`context.rs`](./context.rs) 中的 `Context`、`MockSessionVars::default` 和 `newContext`。

这个薄门面的价值是稳定测试调用入口，同时把实际初始化保持在 `context.rs`。同文件中的 `NewContextDeprecated` 是面向遗留调用方的另一入口，不应与本文件的测试工厂混为一谈。

## 主要符号

- `use crate::{Context, newContext}`：引入公开返回类型 `Context` 和 crate 内可见的真实构造器 `newContext`。`newContext` 使用 `pub(crate)`，外部 crate 不能绕过本门面直接调用。
- `pub fn NewContext() -> Box<Context>`：本文件唯一的函数和公开 API。名称保留 Go 风格，crate 根的 `#![allow(non_snake_case)]` 允许这种命名。函数没有参数，直接返回堆分配且由调用方独占的 mock 会话上下文。
- 本文件没有常量、结构体、枚举、trait、`impl` 或条件编译项。

## 执行流程

1. 测试或测试辅助代码从 `astersql-util-mock` 的 crate 根导入 `NewContext`；同 crate 测试也可通过 `crate::NewContext` 使用再导出。
2. `fortest.rs::NewContext` 不做分支或转换，直接调用 `context.rs::newContext`。
3. `newContext` 构造 `Context`：创建默认 `wrapTxn`、新的 `sessionctx::ExecutionContext`、空 `HashMap` 和 `MockSessionVars::default`，并把 Store、Domain、InfoSchema、session manager、plan cache 等可选依赖初始化为 `None`；DDL owner 与沙箱标志初始化为 `false`。
4. `MockSessionVars::default` 进一步建立正式 `SessionVars`，写入 `max_allowed_packet=67108864`、`character_set_connection=utf8mb4`，并设置 chunk、时区、分页、Chunk RPC、TiFlash 和查询限流等 mock 默认值。
5. 返回的 `Box<Context>` 由调用方按测试场景继续变更，例如写入本地值、绑定 Store、创建假事务或读取 DistSQL 上下文。

## 数据与状态

`NewContext` 自身没有全局状态，也不缓存实例；每次调用都会获得新的 `Box<Context>`。构造结果中的本地值表为空，事务包装器为默认值，多数外部服务句柄为 `None`。`MockSessionVars` 的已验证默认值包括 `InitChunkSize=2`、`MaxChunkSize=32`、UTC 时区、`EnableChunkRPC=true` 和 `QueryCopStoreLimit=15`。

实例隔离是重要不变量：本工厂没有复用静态 `Context`。不过 `Context` 的后续方法可能访问 `context.rs` 内的共享类型或调用方注入的 `Arc`；这不改变工厂每次新建顶层对象的事实。

## 依赖与调用关系

- 上游装配：[`lib.rs`](./lib.rs) 声明并公开再导出 `fortest`，因此公开路径是 crate 根的 `NewContext`。
- 已索引的直接使用者：[`go_merge_30_test.rs`](./go_merge_30_test.rs) 通过 `crate::NewContext` 验证查询级 Store limiter；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 用它覆盖默认值、本地值、事务、取消、错误和锁接口。
- 额外测试使用者：[`mock_test.rs`](./mock_test.rs) 通过 `mock_crate::NewContext` 验证键值生命周期，并提供对应 Go 基准语义的构造循环。
- 唯一下游调用：`context.rs::newContext`。该构造器依赖 `Context`、`wrapTxn`、`sessionctx::ExecutionContext`、`HashMap` 和 `MockSessionVars` 的默认初始化。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 将该包定义为 `astersql-util-mock`，库入口为 `lib.rs`；会话上下文与变量能力来自 `astersql-sessionctx`、`astersql-sessionctx-vardef` 和 `astersql-sessionctx-variable` 等路径依赖。本文件没有直接引入外部 crate。

RustCodeGraph 对 `fortest.rs` 的文件节点给出了“used by”边到 `go_merge_30_test.rs` 和 `migration_aster_unit_test.rs`；精确 `callers/callees --file` 查询本次没有输出，因此这里没有把名称相同的其他 `NewContext` 错认成调用者。

## 错误处理与边界

`NewContext` 的签名不返回 `Result`，门面内部也没有显式错误分支。初始化中固定系统变量和 UTC 偏移使用 `expect`，若这些仓库内常量或解析契约被破坏会 panic，而不是从 `NewContext` 返回错误。

新建上下文故意没有 Store。普通的假事务路径可以工作，但等待需要真实 Store 的 pending transaction 会返回 `MockError::MissingStore`；`Execute`、`ParseWithParams` 等未实现能力返回 `MockError::NotSupported`。未绑定 Domain 时访问要求 Domain 的接口也可能 panic。以上边界来自 `context.rs` 及 `migration_aster_unit_test.rs`，不是 `fortest.rs` 自己新增的策略。

调用方不应把该 mock 当作带真实 `kv::Storage` 的生产会话。Go 文件还明确建议：测试若要访问 Store 中的数据，应优先创建绑定真实测试 Store 的 session，而不是使用此空 Store 上下文。

## 并发与资源生命周期

工厂不加锁、不启动线程、不创建异步任务或通道。`Box<Context>` 的所有权直接转移给调用方，离开作用域后按 Rust 所有权规则释放。每次调用创建独立的 `ExecutionContext`，`Context::Cancel` 可以取消该实例的执行上下文；`migration_aster_unit_test.rs::context_preserves_pending_future_and_cancel_semantics` 验证了取消状态的变化。

本文件不承诺 `Context` 整体可在线程间共享。若测试需要并发共享，应依据具体字段和 trait 边界显式使用 `Arc`/锁，不能因为工厂返回 `Box` 就推断线程安全。资源型依赖（Store、Domain 等）也必须由测试显式绑定并管理生命周期。

## 与 Go 版本的对应关系

[`fortest.go`](./fortest.go) 与本文件具有相同的核心调用链：公开 `NewContext` 直接返回私有 `newContext` 的结果。Go 返回 `*Context`，Rust 用 `Box<Context>` 表达同样的独占堆对象意图；两边都不在门面中复制初始化逻辑。

关键差异是可用范围。Go 文件带 `//go:build !codes`，并明确排除生产构建；Rust 的 `lib.rs` 当前无条件包含和再导出 `fortest`。此外，Go `context.go::newContext` 初始化更完整的正式会话设施，包括可取消的标准 context、表达式/表变更上下文以及内存和磁盘 tracker；当前 Rust `context.rs::newContext` 是按已移植 mock 能力构造的对应实现，不能据 Go 行为推断 Rust 已具备所有相同子系统。

Go [`mock_test.go`](./mock_test.go) 的 `TestContext` 和 `BenchmarkNewContext` 分别对应 Rust `mock_test.rs` 的键值生命周期测试与 `benchmark_new_context` 辅助函数。Rust 还有 `migration_aster_unit_test.rs` 和 `go_merge_30_test.rs`，用于固定更多迁移后的行为。

## 扩展指南

- 若只是新增 `Context` 的默认字段或初始化规则，应修改 `context.rs::newContext` 或相应类型的 `Default`，不要把初始化复制到 `fortest.rs::NewContext`。
- 若新增公开测试工厂语义，先判断是否应给 `NewContext` 增加参数、增加独立工厂，或让测试在返回对象上显式注入依赖；保持空 Store 默认值可避免无意改变现有测试。
- 修改默认值时，至少同步独立测试 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；修改键值或构造入口时同步 [`mock_test.rs`](./mock_test.rs)；修改 DistSQL 限流默认值时同步 [`go_merge_30_test.rs`](./go_merge_30_test.rs)。Rust 单元测试应继续保留在独立测试文件中，不嵌入 `fortest.rs`。
- 若要真正强制“仅测试可见”，需要评估所有下游 crate 的使用方式后再考虑 feature 或条件编译；直接添加 `#[cfg(test)]` 会使依赖 crate 的测试也看不到该 API，因为依赖库通常不会以自身 `cfg(test)` 编译。
- 兼容风险主要是公开函数路径、返回所有权和默认会话状态；性能风险主要来自每次调用都重新分配并初始化上下文。修改后应保留 Go 对照语义，不能用空桩替代实际构造。

## 验证依据

- RustCodeGraph `status`：索引包含 `pkg/util/mock/fortest.rs`；文件节点显示 26 行、2 个符号，并给出到 `go_merge_30_test.rs`、`migration_aster_unit_test.rs` 的使用关系。
- RustCodeGraph `node --file`：读取了 `fortest.rs` 全文、`context.rs` 的 `newContext` 与相邻公开入口、三个相关 Rust 测试的使用场景。`query NewContext/newContext` 的结果包含定义文件路径，据此排除了仓库内其他同名符号。
- 源码与配置：读取了 `pkg/util/mock/{fortest.rs,context.rs,lib.rs,Cargo.toml}`，确认公开再导出、真实初始化和 crate 依赖边界。
- Go 对照：读取了 `pkg/util/mock/{fortest.go,context.go,mock_test.go}`，确认门面调用链、构建标签、初始化差异和测试意图。
- 相关 Rust 测试：`mock_test.rs`、`go_merge_30_test.rs`、`migration_aster_unit_test.rs`。本任务为纯文档分析，按计划不运行 Cargo；测试文件仅作为现有行为证据。
- 人工复核结论：本文件存在的原因是提供稳定公开的测试构造入口；运行时只委托真实构造器；安全扩展应优先修改 `context.rs` 并在独立测试文件中锁定默认值与边界。
