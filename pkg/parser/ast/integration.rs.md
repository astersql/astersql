# `pkg/parser/ast/integration.rs`

## 文件定位

`integration.rs` 是 `astersql-parser-ast` crate 的历史兼容视图，而不是一套独立的 AST 实现。crate 入口在 [`pkg/parser/ast/lib.rs`](lib.rs) 中通过 `pub mod integration;` 暴露该模块；本文件唯一的生产代码 `pub use crate::*;` 将 crate 根部的公开 AST 类型、trait、函数、常量和公开子模块重新导出到 `parser_ast::integration` 路径下。

该文件存在的目的，是让旧的集成代码可以继续从 `integration::...` 取用 AST API，同时所有路径仍指向 `lib.rs` 中的规范定义。它不参与 SQL 文本解析、AST 遍历或 SQL 恢复的运行时步骤，也不会生成第二份 AST 类型。

## 核心职责

本文件只有一项职责：为 crate 根公开 API 提供源码兼容的命名空间别名。

- `pub use crate::*` 是公开的 glob re-export；例如根部的 `CIStr`、`NewCIStr` 和 `Node` 同时可写成 `integration::CIStr`、`integration::NewCIStr` 和 `integration::Node`。
- 类型身份保持不变。`pkg/parser/ast/canonical_contract_test.rs` 用同一泛型参数检查根部与 `integration` 路径的 `CIStr`，并比较 `dyn Node` 的 `TypeId`，防止兼容路径演变成独立定义。
- 导出面随 crate 根公开面同步变化。根部新增或移除 `pub` 项会自动反映到该兼容模块；本文件没有手工白名单。

## 主要符号

- `pub use crate::*`（`integration.rs:21`）：模块中唯一的 Rust item。`pub` 让导入项继续对外可见，`crate::*` 指向当前 `astersql-parser-ast` crate 根的全部可公开导入项。
- 本文件没有常量、类型、trait、函数、`impl` 或条件编译项，也没有私有辅助实现。
- `pub mod integration`（`lib.rs:34`）是模块接线点。若缺少该声明，文件不会进入 crate 的公共模块树。

兼容路径所指向的代表性规范符号均定义在 `lib.rs`，例如 `Node`（访问者协议）、`CIStr`/`NewCIStr`（大小写不敏感标识符）、`LeadingList`/`FlattenLeadingList`、掩码策略位常量以及索引提示类型与作用域；它们不是在本文件中实现的。

## 执行流程

该文件没有运行时控制流，实际过程发生在编译期名称解析阶段：

1. Rust 从 `lib.rs` 的 `pub mod integration;` 装载本模块。
2. `pub use crate::*` 将 crate 根公开项绑定到 `integration` 命名空间。
3. 下游代码解析 `parser_ast::integration::X` 时，得到的仍是 crate 根的规范项 `X`。
4. 对函数的实际调用、对 AST 节点的构造或访问者遍历，直接进入 `lib.rs` 中对应符号的实现；本模块没有包装调用、转换或分派。

`canonical_contract_test.rs` 以 `integration::NewCIStr` 和 `integration::Node` 覆盖第 3 步的类型/协议一致性。RustCodeGraph 将本文件识别为单独文件节点，但文件中没有可执行函数，所以不存在可对本文件自身执行的 caller/callee 函数边。

## 数据与状态

本文件不拥有数据结构、静态变量、缓存或可变状态。通过兼容路径创建的值就是根部类型的值，不需要跨边界复制或转换。

例如 `integration::NewCIStr("MiXeD")` 返回根部 `CIStr`；其 `O`/`L` 语义来自 `lib.rs` 的规范实现。类似地，`integration::Node` 是同一个 trait，而不是结构相似但类型身份不同的协议。`canonical_contract_test.rs` 对这两项不变量提供了直接证据。

## 依赖与调用关系

- 上游接线：`lib.rs:34` 的 `pub mod integration`。
- 已确认的直接使用者：`pkg/parser/ast/canonical_contract_test.rs` 使用 `integration::NewCIStr` 与 `integration::Node`，验证兼容路径。
- 下游依赖：名称解析直接落到 crate 根公开项；本文件本身没有函数调用，也没有额外的 crate 依赖。
- crate 边界：`pkg/parser/ast/Cargo.toml` 将库命名为 `astersql-parser-ast`，入口为 `lib.rs`，并声明 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json`、`url`。这些依赖由根部 AST 实现使用，不是 `integration.rs` 单独引入的。
- feature/条件编译：该 `Cargo.toml` 没有定义 feature，本模块也没有 `#[cfg(...)]`；只有相关测试模块在 `lib.rs` 中受 `#[cfg(test)]` 控制。

RustCodeGraph 的 `node --file pkg/parser/ast/integration.rs` 显示文件完整内容仅 21 行，并确认唯一 item 为重导出。由于 glob re-export 是名称关系而非函数调用，调用图不能把所有潜在的根符号使用列成 `integration.rs` 的 callees；兼容关系应以重导出语句和契约测试为准。

## 错误处理与边界

本文件没有 `Result`、错误类型、`panic!`、输入校验或恢复逻辑，因此不会自行产生或转换错误。通过 `integration` 路径调用某个根函数时，其错误行为完全由那个根符号决定。

主要边界是公开面控制：只有 crate 根可公开导出的项才会进入兼容视图，私有项不会因 glob re-export 变成外部 API。另一方面，glob 会使根部公开 API 的变动自动扩散到 `integration`；若需要稳定的受限 API 清单，应先评估是否会破坏当前“完整兼容视图”契约，不能悄然改成选择性导出。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源，也没有初始化/销毁阶段。重导出是编译期机制，不增加运行时分配、同步、复制或析构成本。

通过该路径取得的具体 AST 值，其所有权、借用、`Send`/`Sync` 能力和资源生命周期均与根部原类型完全相同。本文件不会增强或削弱这些属性。

## 与 Go 版本的对应关系

Go 的 `pkg/parser/ast` 天然只有一个包级公开命名空间，没有与 `integration.rs` 一一对应的 `integration.go`。Rust 为迁移期/历史调用方额外提供 `integration` 子模块，再把同一套根类型重导出；因此该文件对应的是 Go 单一 `ast` 包公开面的兼容适配，而不是某个 Go 算法文件。

相关语义的直接 Go 证据包括：

- `pkg/parser/ast/model.go` 定义 `CIStr` 与 `NewCIStr`；Rust 的兼容路径仍指向根部同名实现。
- `pkg/parser/ast/dml.go` 定义 `IndexHintType`、`IndexHintScope` 及其常量。
- `pkg/parser/ast/ddl.go` 定义 `MaskingPolicyRestrictOps` 位掩码常量。
- `pkg/parser/ast/misc.go` 定义 `LeadingList` 与 `FlattenLeadingList`。

`pkg/parser/ast/integration_9_aster_unit_test.rs` 验证上述代表性 Rust 根符号的 Go 字段/常量语义；它通过 `use super::*` 测试规范实现，而 `canonical_contract_test.rs` 另外证明 `integration` 路径引用的正是该规范实现。两类测试合起来分别覆盖“实现与 Go 对齐”和“兼容路径不复制类型”。

## 扩展指南

- 新增 AST 类型或函数时，应在实际负责语义的根部实现文件/`lib.rs` 中实现，而不是在 `integration.rs` 复制定义；公开根符号会被本文件自动重导出。
- 若新增 API 必须能从历史路径访问，应扩展独立的 `pkg/parser/ast/canonical_contract_test.rs`，同时比较根路径和 `integration` 路径的具体类型或 trait 身份。
- 若新增逻辑复刻 Go 行为，应把语义测试放在独立的 `*_test.rs` 文件，并在 `lib.rs` 的 `#[cfg(test)]` 区域接线；不要把测试内嵌到本生产文件。Go 对照测试应优先来自相应的 `model_test.go`、`dml_test.go`、`ddl_test.go`、`misc_test.go` 等同职责文件。
- 修改 `pub use crate::*` 为选择性导出，或在 `integration` 中增加同名包装类型，可能造成源码兼容破坏、类型身份分裂以及后续 API 漂移；必须同步更新契约测试并审计所有 `integration::` 使用者。
- 当前重导出没有运行时性能成本。扩展时加入转换、克隆或包装层会改变这一特性，应单独评估兼容性和性能。

## 验证依据

事实核验使用了以下证据：

- RustCodeGraph：`status`（索引包含 11,467 个文件，目标文件已索引）、`files --filter pkg/parser/ast/integration.rs`、`node --file pkg/parser/ast/integration.rs --offset 1 --limit 500`、`query integration --kind file --limit 20`。文件节点显示完整 21 行源码及唯一重导出；因为没有函数符号，本文件无可用的函数 callers/callees。
- Rust 源与接线：`pkg/parser/ast/integration.rs`、`pkg/parser/ast/lib.rs`。
- crate 声明：`pkg/parser/ast/Cargo.toml`。
- 独立 Rust 测试：`pkg/parser/ast/canonical_contract_test.rs`、`pkg/parser/ast/integration_9_aster_unit_test.rs`；二者均由 `lib.rs` 的 `#[cfg(test)]` 模块声明接线。
- Go 对照：`pkg/parser/ast/model.go`、`pkg/parser/ast/dml.go`、`pkg/parser/ast/ddl.go`、`pkg/parser/ast/misc.go`。
- 仓库引用搜索：除契约测试外，未发现其他 Rust 文件直接使用 `parser_ast::integration`、`astersql_parser_ast::integration` 或 `integration::NewCIStr`/`integration::Node`；这说明当前可见直接消费者主要是兼容契约测试，不代表外部仓库没有依赖该公开路径。

本任务是纯文档分析，按计划不运行 Cargo 或 parser 构建测试。交付前以任务指定命令验证文档存在且恰含十一个固定二级章节，并人工复核“为何存在、如何运行、如何安全扩展”均有源码或测试依据。
