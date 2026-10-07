# `br/pkg/gluetidb/mock/lib.rs`

## 文件定位

本文件是 Cargo crate `astersql-br-pkg-gluetidb-mock` 的唯一库入口；[`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 指向它，并把 Go 包映射记录为 `br/pkg/gluetidb/mock`。它位于 BR 通用 [`Glue`](../../glue/glue.rs) 契约与测试用 TiDB 替身之间：入口本身不实现会话或存储行为，只负责装配 [`mock.rs`](mock.rs)、将其公开项提升到 crate 根，并在测试构建中挂载独立的 [`parity_test.rs`](parity_test.rs)。

该 crate 不是生产 TiDB Glue；它为 BR 的备份、恢复和测试套件提供轻量替身。Rust 中已确认的直接使用者是 [`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs)，该文件从 crate 根导入 `MockGlue`。因此 `lib.rs` 是稳定的导入门面，而真实行为和迁移边界位于 `mock.rs`。

## 核心职责

- 用 `#[path = "mock.rs"] mod mock` 把同目录实现文件声明为私有子模块；显式 `path` 使 Rust 模块布局与 Go 包的单目录布局保持直观对应。
- 用 `pub use mock::*` 扁平再导出实现模块的所有公开项，使调用方写 `astersql_br_pkg_gluetidb_mock::MockGlue`，无需依赖私有的 `mock` 模块路径。
- 仅在 `cfg(test)` 下用 `#[path = "parity_test.rs"] mod parity_test` 挂载独立测试，满足生产逻辑与测试逻辑分文件的仓库约束。
- 在 crate 根统一放宽迁移代码会触发的命名及未使用告警；这些 `allow` 只影响编译告警，不改变运行时行为。

## 主要符号

- `mod mock`：私有模块声明，源码固定为 [`mock.rs`](mock.rs)。它包含 `MockGlue`、`MockSession`、`SessionAPI`、`RecordSet`、`Chunk`、`InternalTxnBR` 和测试观测辅助函数等实现。
- `pub use mock::*`：本文件唯一的公开 API 动作。由于是通配再导出，`mock.rs` 新增的任何 `pub` 项都会自动成为 crate 根 API；私有项如 `CloseRecordSet`、`NilStorage`、`NopProgress` 不会泄露。
- `mod parity_test`：只在当前 crate 的测试构建中出现的私有模块，源码为 [`parity_test.rs`](parity_test.rs)；生产依赖编译不会包含它。
- crate 级 `#![allow(...)]`：允许 `dead_code`、Go 风格的大小写命名、未使用导入和未使用变量。它服务于逐步移植及 Go API 对齐，但也可能掩盖新增代码的无效接线，因此扩展时仍需人工审查。

本文件没有函数、类型、trait、常量、`impl` 或异步入口，也没有 feature 条件；唯一条件编译项是 `cfg(test)` 测试模块。

## 执行流程

1. Cargo 依据 [`Cargo.toml`](Cargo.toml) 把 `lib.rs` 编译为 `astersql-br-pkg-gluetidb-mock` 库。
2. 编译器按显式路径解析 [`mock.rs`](mock.rs)，其私有项留在模块内，公开项经 `pub use mock::*` 加入 crate 根命名空间。
3. 上游如 [`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs) 从 crate 根构造 `MockGlue`；后续 `CreateSession`、`Execute`、`Open` 等调用直接进入 `mock.rs` 的 trait 实现，运行时不再经过 `lib.rs` 的额外分支。
4. 当编译该 crate 自身的测试目标时，`cfg(test)` 变真，编译器再载入 [`parity_test.rs`](parity_test.rs)。测试通过 `use crate::{...}` 验证的正是本文件形成的根级再导出 API。
5. 普通依赖构建中 `parity_test` 不存在，因此测试替身 `FakeSession`、`FakeRecordSet` 和两个 `#[test]` 不进入库产物。

## 数据与状态

`lib.rs` 自身不分配也不持有运行时状态。它只定义静态模块关系与编译配置。经它再导出的状态都属于 [`mock.rs`](mock.rs)：`MockGlue` 保存可选的 `Arc<Mutex<dyn SessionAPI>>` 与 `GlobalVars`；`MockSession` 克隆这些会话/变量配置；线程局部的 `LAST_INTERNAL_SOURCE` 记录最近一次内部事务来源；`NopProgress` 用原子值记录进度和关闭状态。

再导出不会复制这些对象，也不会引入第二套单例或缓存。特别是 `pub use` 只是名称解析层接线；`MockGlue` 与 `MockSession` 的所有权、克隆和锁语义完全由 `mock.rs` 决定。

## 依赖与调用关系

向上，已验证的 Rust 依赖链是 [`br/pkg/utiltest/Cargo.toml`](../../utiltest/Cargo.toml) 以路径依赖引入本 crate，随后 [`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs) 使用 `astersql_br_pkg_gluetidb_mock::MockGlue` 组装恢复测试套件。Go 侧还有 `br/pkg/backup/client_test.go`、`br/pkg/restore/data/data_test.go`、`br/pkg/task/restore_test.go` 和 `br/pkg/utiltest/suite.go` 引用同路径 Go 包，说明该模块的角色是跨多条 BR 测试链复用 Glue 替身，而不是应用主链入口。

向下，`lib.rs -> mock.rs` 是唯一生产模块边。[`Cargo.toml`](Cargo.toml) 的直接 crate 依赖只有 `astersql-br-pkg-glue` 和 `astersql-errors`，实际由 `mock.rs` 使用：前者提供 `Glue`、`Session`、`Storage`、`Progress` 等接口与数据类型，后者提供 `SharedError`。`lib.rs -> parity_test.rs` 仅存在于测试构建。

RustCodeGraph 将目标文件识别为 1 个符号的 crate 入口，并列出同目录 `lib.rs`、`mock.rs`、`mock.go`、`parity_test.rs` 四个已索引文件。对 `MockGlue`、`MockSession`、`WithInternalSourceType` 的精确查询定位到 `mock.rs` 实现及 `mock.go` 对照；由于 `lib.rs` 没有可调用函数，调用者/被调用者图不应被解读为业务调用链，真实上游以 Cargo 依赖和导入点为准。

## 错误处理与边界

本文件没有 `Result`、panic 分支或错误包装。错误语义来自再导出的实现：`MockSession::ExecuteInternal` 传播底层 `SessionAPI::ExecuteInternal` 错误，却按 Go 行为吞掉结果集第一次 `Next` 的错误；未注入 session 时会 panic；未实现的 DDL/元数据方法也 panic。`CloseRecordSet` 的 `Drop` 确保结果集在普通返回、错误和 panic 展开时关闭。

门面的主要边界是 API 暴露范围。`pub use mock::*` 会自动公开未来所有 `pub` 项，新增辅助符号若不应成为外部契约，必须留为 `pub(crate)` 或私有。反过来，把现有公开项改成私有会直接破坏 crate 根导入。`#![allow(...)]` 可能让遗漏接线或拼写风格问题不产生告警，不能把“无编译告警”当作功能已被使用的证据。

`cfg(test)` 只覆盖该 crate 自身测试；依赖者编译本库时不会得到 `parity_test` 模块。需要供其他 crate 测试复用的替身必须放在生产模块并有意公开，不能只写进 `parity_test.rs`。

## 并发与资源生命周期

`lib.rs` 没有线程、任务、通道、锁或资源清理逻辑，也不改变再导出类型的 `Send`/`Sync` 属性。并发与生命周期证据来自 [`mock.rs`](mock.rs)：注入的会话以 `Arc<Mutex<dyn SessionAPI>>` 共享并串行访问；内部事务来源用线程局部存储隔离线程；`NopProgress` 使用原子计数；结果集由 RAII 包装器在离开作用域时调用 `Close`。

[`parity_test.rs`](parity_test.rs) 的 `result_set_closes_when_next_panics` 验证 `Next` panic 展开时仍关闭结果集，`go_rust_public_contract_matches` 验证 SQL 转发、错误传播、一次 `Next`、关闭、全局变量和未实现路径等契约。入口把测试保留在独立文件中，不在生产源文件内嵌测试代码。

## 与 Go 版本的对应关系

Go 的 [`mock.go`](mock.go) 直接在 `package mock` 中声明 `mockSession` 和公开 `MockGlue`，Go 包系统天然把同目录文件合并为一个包；Rust 需要 `lib.rs` 显式声明子模块并再导出，才能给调用方提供近似的包级 API。因此 `mod mock` 加 `pub use mock::*` 是结构层面的 Go 包对应物，不是 Go 中某个函数的翻译。

行为对应由 [`mock.rs`](mock.rs) 完成：`MockGlue` 实现 Glue trait，`MockSession` 执行内部 SQL并标记 `InternalTxnBR`，全局变量缺省为 `"True"`，版本固定为 `"mock glue"`，`OwnsStorage` 为真，一次性会话不继承 `GlobalVars`。Rust 为 Go 的 `nil` Domain/Storage/Progress 结果提供可满足 trait 的空对象，并用 panic 对应 Go 未实现路径中的 `log.Fatal`；这些适配是实现文件的差异，不是 `lib.rs` 的行为。

Rust 额外把 parity 测试通过 `cfg(test)` 和显式路径接入，Go 对照目录没有同名 `mock_test.go`；Go 的实际使用证据分散在备份、恢复、任务和 utiltest 测试中。本文只据此认定公共测试替身契约，不宣称它替代完整 TiDB session/domain/storage。

## 扩展指南

- 新增 mock 行为应修改 [`mock.rs`](mock.rs) 中最接近的 trait 实现，并同步扩展独立的 [`parity_test.rs`](parity_test.rs)；不要把实现或测试塞进 `lib.rs`。
- 新增公开类型或函数前，确认它确实应从 crate 根稳定导出；通配再导出会立即放大 API 面。只供内部组合的辅助类型优先保持私有。
- 若拆分实现为多个源文件，应在本入口或 `mock.rs` 明确声明模块，并决定是否逐项再导出；同时检查 [`Cargo.toml`](Cargo.toml)、`BUILD.bazel` 和上游导入路径是否需要同步。
- 改变 `MockGlue`/`MockSession` 契约时，以 [`mock.go`](mock.go) 为语义基线，继续覆盖底层执行错误、`Next` 错误吞掉、panic 时资源关闭、未注入 session、全局变量覆盖、一次性会话和 DDL 未实现边界。
- 并发相关扩展要保持锁粒度与线程局部观测的意图；新增测试继续放在独立测试文件，避免污染生产模块。
- 不要仅因 crate 级 `allow` 使代码通过检查就认定接线完成；应同时搜索上游实际导入和调用点，并评估公开符号是否被测试覆盖。

## 验证依据

- 入口源码：[`lib.rs`](lib.rs) 全部 25 行；确认 crate 级告警配置、`mock` 私有模块、通配再导出和仅测试挂载的 `parity_test`，且无其他符号或条件编译项。
- crate 边界：[`Cargo.toml`](Cargo.toml) 的包名、`[lib]` 路径、Go 包元数据及 `astersql-br-pkg-glue`、`astersql-errors` 两项直接依赖；[`BUILD.bazel`](BUILD.bazel) 仅作为同目录构建元数据存在，本任务未修改。
- 实现与 Go 对照：[`mock.rs`](mock.rs) 的公开符号和 Glue/Session 实现；[`mock.go`](mock.go) 的 `mockSession`、`MockGlue` 及其方法，用于核对门面所暴露能力的来源与移植差异。
- 独立测试：[`parity_test.rs`](parity_test.rs) 的 `result_set_closes_when_next_panics` 与 `go_rust_public_contract_matches`；上游 [`br/pkg/utiltest/parity_test.rs`](../../utiltest/parity_test.rs) 还检查测试套件中的默认 `MockGlue` 状态。
- 上游证据：[`br/pkg/utiltest/Cargo.toml`](../../utiltest/Cargo.toml) 的路径依赖和 [`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs) 的 crate 根导入；Go 使用点包括 `br/pkg/backup/client_test.go`、`br/pkg/restore/data/data_test.go`、`br/pkg/task/restore_test.go`、`br/pkg/utiltest/suite.go`。
- RustCodeGraph：`status` 报告索引含 7032 个 Rust 文件；`files --filter br/pkg/gluetidb/mock` 返回四个源码文件；`node --file br/pkg/gluetidb/mock/lib.rs` 读取全部入口并标为 1 个符号；`query` 分别定位 `MockGlue`、`MockSession` 与 `WithInternalSourceType` 的 Rust/Go 定义。自然语言 `explore` 与针对歧义类型名的调用图未提供可靠附加边，故外部接线改以 Cargo 与精确 `rg` 结果为证据。
- 验证限制：依任务约束未运行 Cargo；本文证明源码结构、再导出关系、对照实现和测试覆盖位置，不证明完整 TiDB 运行时集成。交付前另运行任务规定的 11 章节结构校验。
