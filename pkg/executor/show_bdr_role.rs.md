# `pkg/executor/show_bdr_role.rs`

## 文件定位

本文件属于 `astersql-executor` crate；[`pkg/executor/Cargo.toml`](Cargo.toml) 以 `lib.rs` 为 crate 根，[`pkg/executor/lib.rs`](lib.rs) 通过 `pub mod show_bdr_role` 公开此模块，并将独立测试文件 `show_bdr_role_test.rs` 置于 `#[cfg(test)]` 下。它表达 `ADMIN SHOW BDR ROLE` 的 Rust 侧单行执行控制流：每次先清空输出 `Chunk`，首次调用在新管理员事务中读取角色，后续调用不再产出行。

本文件是“执行器状态机 + 运行时边界”，而不是完整的 KV/元数据实现。Rust builder 在 [`pkg/executor/builder.rs`](builder.rs) 中把计划转成通用 `ExecutorKind::AdminShowBdrRole` 叶节点，但当前生产代码搜索未发现 `ShowBdrRoleRuntime` 的实现，也未发现 builder 直接构造本文件的泛型 `AdminShowBDRRoleExec`。因此，已有证据只能证明核心流程已移植并受单元测试约束，不能声称它已直接接入完整 Rust SQL 主链。

## 核心职责

- `ShowBdrRoleRuntime` 抽象输出块重置以及“新管理员事务中读角色并写回结果”的环境能力，使状态机不依赖具体会话、KV 事务或 meta mutator 类型。
- `AdminShowBDRRoleExec::Next` 管理单行结果的生命周期：首次执行运行时回调，`done` 为真后所有后续调用均返回空块。
- trait 文档明确保留 Go `kv.RunInNewTxn` 的一个细节：事务回调内已写行并设置 `done`，但后续提交失败时，这两个内存侧作用不回滚。
- 文件不负责 SQL 解析、计划 schema 生成、BDR 角色值的合法性、存储键编码或真实事务提交；这些要么在 parser/planner/meta/KV 层，要么属于待接入的运行时实现。

## 主要符号

- `pub trait ShowBdrRoleRuntime`：运行时契约。`Context` 是调用环境关联类型，`Error` 是统一错误类型。trait 本身没有生产实现证据。
- `ShowBdrRoleRuntime::reset_chunk(&self, request: &mut Chunk)`：必须在每次 `Next` 入口清空上一批结果，包括执行器已完成的快速返回路径。
- `ShowBdrRoleRuntime::run_in_new_admin_transaction(...) -> Result<(), Self::Error>`：受托管的副作用边界。实现者应在新管理员事务中读取角色、向 `request` 追加一行，并将 `done` 设为 `true`。
- `pub struct AdminShowBDRRoleExec<R: ShowBdrRoleRuntime>`：持有公开的 `runtime: R` 和 `done: bool`。它没有构造函数、`Default` 或条件编译分支，调用方必须显式初始化两个字段。
- `AdminShowBDRRoleExec::Next(&mut self, context, request) -> Result<(), R::Error>`：唯一执行入口。大写命名受文件级 `#![allow(non_snake_case)]` 允许，用于保留 Go `Next` 形状。

文件没有模块常量、枚举、宏、自由函数或 `unsafe` 代码；直接 import 只有 `astersql_util_chunk::Chunk`。

## 执行流程

1. 调用方以可变上下文和可变 `Chunk` 调用 `AdminShowBDRRoleExec::Next`。
2. `Next` 无条件调用 `runtime.reset_chunk(request)`，因此新一批不会混入旧行；即使 `done == true` 也会先重置。
3. 若 `done` 已为真，立即返回 `Ok(())`，不再打开事务，输出块保持为空。
4. 若未完成，把 `context`、`request` 和 `&mut self.done` 同时交给 `run_in_new_admin_transaction`。真实实现应对应 Go 路径的 `RunInNewTxn(InternalTxnAdmin) -> meta.NewMutator(txn).GetBDRRole() -> AppendString(0, role) -> done = true`。
5. `Next` 原样返回运行时的 `Result`，没有额外映射、重试或清理。若回调已写行和设置 `done` 后返回提交错误，错误向上传播，但该行和 `done` 保留；下次 `Next` 会清空它后直接成功返回。

## 数据与状态

`AdminShowBDRRoleExec` 唯一持久的流程状态是 `done`。它是单向完成位：本文件只读取它，转为 `true` 的责任由运行时回调承担，没有重置为 `false` 的路径。这保证一个执行器实例最多调用一次读取事务。

`Chunk` 是调用方拥有的列式结果缓冲，本文件不保存其引用。结果的列数、列类型和 schema 由规划/构建层确定；Go 实现把角色字符串写入第 0 列。元数据层 `Mutator::GetBDRRole` 在未设置角色时返回空字符串；`pkg/meta/meta_test.go` 还验证了设置 primary 和清除后再读取的循环。

## 依赖与调用关系

本文件的直接 Rust 依赖只有 `astersql-util-chunk`，它在 [`pkg/executor/Cargo.toml`](Cargo.toml) 中以路径 `../util/chunk` 声明。`Context`、`Error` 以及事务/meta 操作被 trait 隔离，所以文件没有直接 import `kv` 或 `meta`。

文件内的确定调用边是 `AdminShowBDRRoleExec::Next -> ShowBdrRoleRuntime::reset_chunk` 以及未完成分支上的 `Next -> ShowBdrRoleRuntime::run_in_new_admin_transaction`。RustCodeGraph 已返回目标文件全文和这两个 trait 方法节点，但对泛型 `impl` 中的大写 `Next` 未生成可直接定址节点，因此未伪造模块外图边。

计划构建侧的间接路径是 `Plan::AdminShowBdrRole -> executorBuilder::buildAdminShowBDRRole -> build_leaf(ExecutorKind::AdminShowBdrRole)`。这是 Rust 管理命令计划的通用分发证据，不是对本文件 `AdminShowBDRRoleExec<R>` 的直接构造边。当前唯一直接构造者在独立测试 `pkg/executor/show_bdr_role_test.rs`。

## 错误处理与边界

- `reset_chunk` 无返回值；该契约无法表达重置失败。
- 事务创建、元数据读取和提交错误均应收敛为 `R::Error`，`Next` 不包装、不记录且不重试。
- 运行时契约依赖实现者正确追加一行并设置 `done`。若实现返回 `Ok(())` 却忘记设置 `done`，后续 `Next` 会重复打开事务；类型系统不能阻止此类契约违反。
- 回调内的行/`done` 副作用先于最终事务成功，提交错误不回滚这些内存修改。`commit_failure_preserves_callback_side_effects_like_go` 精确覆盖了这一边界。
- 本文件不验证返回的角色是 `primary`/`secondary`/空值之一，也不负责权限检查。这些行为不应在扩展本状态机时凭假设添加。

## 并发与资源生命周期

本文件是同步、单次可变借用流程：`Next` 需要 `&mut self`，所以安全 Rust 不允许同一执行器实例被并发可变调用。它不创建线程、异步任务、通道或锁，也不持有 `Chunk`、上下文或事务跨调用生存。

真实事务的创建、回调、提交与释放全在 `run_in_new_admin_transaction` 运行时边界内，本文件无法保证其锁粒度、取消语义或超时。Go 对照使用 `kv.RunInNewTxn(..., true, callback)`，其回调完成后由帮助函数提交；Rust 未来的生产实现需保持这个事务生命周期和 `InternalTxnAdmin` 来源标记。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/executor/show_bdr_role.go`](show_bdr_role.go)。两版的主干顺序相同：重置请求块，已完成则返回，否则在新管理事务中读 BDR role，写第 0 列并设置完成位。两版都保留回调内副作用先于最终 commit 结果的时序。

主要差异是具体能力所在位置：Go `AdminShowBDRRoleExec` 嵌入 `exec.BaseExecutor`，从 `e.Ctx().GetStore()` 取 store，直接调用 `kv.WithInternalSourceType`、`kv.RunInNewTxn`、`meta.NewMutator(txn).GetBDRRole` 和 `req.AppendString`；Rust 类型把这些全部放入 `ShowBdrRoleRuntime`。Go builder 直接返回 `&AdminShowBDRRoleExec{BaseExecutor: ...}`，Rust builder 目前只通过通用 `build_leaf` 路径生成执行器盒，没有将该路径与本文件的泛型类型绑定的直接证据。

Go 未找到专门针对 `show_bdr_role.go` 执行器的独立测试；相关 Go 证据是 `pkg/meta/meta_test.go` 对 `GetBDRRole` 的空值、设置和清除行为。Rust 则在 [`pkg/executor/show_bdr_role_test.rs`](show_bdr_role_test.rs) 用可控运行时专门锁定 commit 失败后的副作用语义。

## 扩展指南

- 接入完整 Rust 执行链时，应在 executor builder 的具体依赖实现中为 `ExecutorKind::AdminShowBdrRole` 构造本类型及真实 `ShowBdrRoleRuntime`；实现必须使用新事务、标记管理员内部来源、读 meta role，并保留回调/提交时序。
- 修改单行/完成语义时，主要修改点是 `AdminShowBDRRoleExec::Next` 和 `run_in_new_admin_transaction` 契约。必须同步更新独立的 `pkg/executor/show_bdr_role_test.rs`，不应把 `#[cfg(test)]` 测试嵌入生产文件。
- 扩展错误行为时应分别覆盖：读角色失败前不写行/不设 `done`，回调成功但 commit 失败后保留行/保留 `done`，以及成功后第二次 `Next` 重置为空且不再开事务。
- 若增加角色列、状态或诊断信息，需同时核对 planner 的 `buildAdminShowBDRRoleFields`、Go/Rust 输出顺序和 `Chunk` 列型，避免只改运行时而破坏 schema 契约。
- 兼容性风险是改变空值、角色文本或错误后重入语义；正确性风险是错用业务会话事务或忘记 `done`；性能风险很低，但必须保持每个执行器实例最多一次元数据事务。

## 验证依据

- RustCodeGraph `status` 确认当前索引可用（11,467 个文件、307,296 个节点、1,848,419 条边）；`explore "pkg/executor/show_bdr_role.rs ShowBDRRole"` 返回目标 Rust 文件和独立测试全文，`query AdminShowBDRRoleExec` / `query ShowBdrRoleRuntime` 同时定位 Rust 与 Go 对照符号。
- RustCodeGraph `node buildAdminShowBDRRole` 确认 Go builder 直接构造 `AdminShowBDRRoleExec`，而 Rust `buildAdminShowBDRRole` 调用 `build_leaf`；对泛型 Rust `Next` 的精确 `node/callers/callees` 定位未成功，所以调用关系仅使用已返回的源码和全仓引用搜索交叉验证。
- 已读 Rust 证据：[`pkg/executor/show_bdr_role.rs`](show_bdr_role.rs)、[`pkg/executor/show_bdr_role_test.rs`](show_bdr_role_test.rs)、[`pkg/executor/lib.rs`](lib.rs)、[`pkg/executor/builder.rs`](builder.rs) 及 [`pkg/executor/Cargo.toml`](Cargo.toml)。`pkg/executor` 下不存在 `doc.go`。
- 已读 Go 证据：[`pkg/executor/show_bdr_role.go`](show_bdr_role.go)、[`pkg/executor/builder.go`](builder.go)、[`pkg/meta/meta.go`](../meta/meta.go) 与 [`pkg/meta/meta_test.go`](../meta/meta_test.go)。全仓搜索未发现专门调用 `AdminShowBDRRoleExec` 的 Go 测试。
- 全仓 Rust 引用搜索只发现模块声明、目标文件、独立测试和 builder 的通用 kind 分发，未发现生产 `impl ShowBdrRoleRuntime` 或对本泛型执行器的直接构造；因此文档将生产接线标为未验证/未显示接入。
- 本任务只生成文档，按计划不运行 Cargo。交付前使用任务指定命令验证恰好 11 个固定章节，并人工复核只有本说明文档和完成后删除的编号任务文件发生变化。
