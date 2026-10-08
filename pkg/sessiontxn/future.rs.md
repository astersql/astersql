# `pkg/sessiontxn/future.rs`

## 文件定位

`future.rs` 属于 `astersql-sessiontxn` crate。crate 根模块 [`lib.rs`](lib.rs) 以私有模块 `mod future` 装配它，再通过 `pub use future::*` 公开导出其中的 `ConstantFuture`。[`Cargo.toml`](Cargo.toml) 将该 crate 对应到 Go 包 `pkg/sessiontxn`，并以 `astersql-sessionctx` 作为普通依赖；本文件正是通过该依赖实现会话上下文定义的 Oracle Future 契约。

这个文件只提供“已经得到时间戳”的立即完成 Future，不负责向 PD/TSO 请求时间戳，也不负责创建、激活或提交事务。当前 Rust 仓库中，它的直接引用只出现在 [`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs)；Rust 隔离层另有自己的 `TimestampFuture` 和同名 `ConstantFuture`（[`isolation/base.rs`](isolation/base.rs)），两者的 trait、错误类型及接线位置不同，不能视为同一个实现。

## 核心职责

- 用 `ConstantFuture(pub u64)` 保存一个预先确定的时间戳。
- 通过固有方法 `ConstantFuture::Wait(&self)` 为直接调用者提供不消耗对象的读取入口。
- 通过 `impl sessionctx::OracleFuture` 使其能装箱为 `Box<dyn OracleFuture>`，满足 [`sessionctx::Context::PrepareTSFuture`](../sessionctx/context.rs) 的参数契约。
- 每次等待都原样返回保存的 `u64`，不访问网络、时钟、存储或全局状态。

## 主要符号

- `ConstantFuture(pub u64)`：公开元组结构体；字段也公开，调用者可用 `ConstantFuture(ts)` 构造并直接读取 `.0`。它派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`PartialEq`；默认值因此是 `0`，相等性只比较所存时间戳。
- `ConstantFuture::Wait(&self) -> Result<u64, sessionctx::GoError>`：公开固有方法。它借用 `self`，返回 `Ok(self.0)`，所以同一值可重复等待且对象仍可继续使用。
- `sessionctx::OracleFuture for ConstantFuture`：实现 [`OracleFuture`](../sessionctx/context.rs) 的唯一方法 `Wait(self: Box<Self>) -> Result<u64, GoError>`。该入口消费装箱对象并返回相同值，使 `ConstantFuture` 可经 trait object 交给会话接口。trait 自身要求 `Send`；本类型只含 `u64`，自动满足该约束。
- `#![allow(non_snake_case)]`：允许两个 Rust 方法沿用 Go 风格名称 `Wait`，保持跨语言接口命名一致。

## 执行流程

直接调用路径如下：调用者构造 `ConstantFuture(ts)`，调用固有 `Wait(&future)`，方法立即读取元组字段并返回 `Ok(ts)`。这里没有延迟初始化或状态迁移。

trait-object 路径如下：调用者将值装箱为 `Box<dyn sessionctx::OracleFuture>`，会话侧接口可以把它传给 `PrepareTSFuture`，最终通过 `OracleFuture::Wait` 消费该盒子并得到 `Ok(ts)`。本文件只证明类型适配和取值步骤；当前 Rust 仓库没有找到把这个 `sessiontxn::ConstantFuture` 传入生产会话实现的调用边，因此不能据此断言完整事务链已经采用该实现。

[`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs) 的 `constant_future_implements_session_oracle_contract` 分别覆盖两条入口：固有方法以 `u64::MAX` 验证值不被截断，trait 方法以 `42` 验证装箱动态分派返回相同值。

## 数据与状态

唯一运行时状态是元组字段中的一个 `u64`。`Wait` 不修改它，也没有缓存标记、完成标记或重试计数。`Clone`/`Copy` 会复制该数值而非共享状态；`Default` 产生 `ConstantFuture(0)`，但本文件不解释 `0` 是否是业务上有效的事务时间戳，调用者必须依据自己的事务语义决定是否允许它。

本类型没有保存 scope、执行上下文、取消信号或错误。它也不会更新会话中的事务上下文；把返回值如何写入 start TS、for-update TS 或 prepared transaction，属于上层会话/隔离实现的职责。

## 依赖与调用关系

下游依赖只有 `astersql_sessionctx as sessionctx`：使用其中的 `GoError` 返回类型和 `OracleFuture` trait。[`pkg/sessionctx/context.rs`](../sessionctx/context.rs) 定义 `OracleFuture: Send`，并让 `Context::PrepareTSFuture` 接收 `Box<dyn OracleFuture>`，这是本实现可供会话层消费的接口边界。

上游装配来自 [`pkg/sessiontxn/lib.rs`](lib.rs) 的 `pub use future::*`。RustCodeGraph 对 `pkg/sessiontxn/future.rs` 的文件关系只报告 [`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs) 使用它，且对目标 `ConstantFuture` 没有生产 callers/callees；精确仓库搜索同样只发现该契约测试引用此定义。隔离 crate 中出现的 `ConstantFuture` 均解析到 [`pkg/sessiontxn/isolation/base.rs`](isolation/base.rs) 的局部类型，它实现的是 `TimestampFuture::wait(&mut self) -> Result<u64, TxnError>`，并非这里的 `sessionctx::OracleFuture`。

`Cargo.toml` 没有为本文件设置 feature 或条件编译开关。本类型随 `astersql-sessiontxn` 库正常编译；只有验证它的 `sessiontxn_aster_unit_test` 受 `#[cfg(test)]` 控制。

## 错误处理与边界

两个 `Wait` 的签名都保留 `Result<_, sessionctx::GoError>`，以符合会话 Oracle 接口，但当前实现没有错误分支，恒为 `Ok(self.0)`。它不会把无效值转换成错误，不校验时间戳是否为零、是否单调、是否早于上次提交时间，也不会响应取消或超时。

因此，调用者不应把成功返回理解为时间戳已经通过业务校验；本类型只能表达“按原样交付一个预置值”。若未来增加校验或可失败行为，必须同时审视直接 `Wait` 与 trait `Wait`，避免两条入口产生不同语义，并更新独立测试而不是把测试嵌入本源文件。

## 并发与资源生命周期

`ConstantFuture` 没有内部可变性、锁、通道、任务或堆资源。`u64` 值使它天然可复制，`OracleFuture: Send` 允许装箱值跨线程转移；不过 trait 没有要求 `Sync`，本文件也不建立共享访问协议。

固有 `Wait(&self)` 仅借用对象，生命周期由调用者管理，可重复调用。trait 的 `Wait(self: Box<Self>)` 获取盒子所有权；返回后盒子被释放，符合一次性 Future 的消费模型。由于没有阻塞操作，“Wait”只是接口兼容名称，并不发生线程等待或异步轮询。

## 与 Go 版本的对应关系

Go 对照文件 [`future.go`](future.go) 定义 `type ConstantFuture uint64`，其 `Wait() (uint64, error)` 直接返回转换后的数值和 `nil`。Rust 的元组结构体、固有 `Wait` 与之保持“常量值、立即成功、不产生副作用”的核心语义；Rust 额外显式实现 `sessionctx::OracleFuture`，并用 `Box<Self>` 表达 trait-object 的所有权消费。

Go 版本已经在 [`isolation/base.go`](isolation/base.go) 中为预置 start TS 和 snapshot TS 替换事务 Future，在 [`isolation/readcommitted.go`](isolation/readcommitted.go) 中复用最新 Oracle TS，并在 [`staleread/provider.go`](staleread/provider.go) 中准备 stale-read 时间戳；[`session/syssession/session_integration_test.go`](../session/syssession/session_integration_test.go) 也直接把它交给 `PrepareTSFuture`。当前 Rust 同路径调用尚未使用本文件的类型，而是在 isolation crate 内维护独立适配类型，所以 Rust 的类型契约已具备，但与这些 Go 生产调用点的对齐仍未由当前调用图证明。

## 扩展指南

- 若只需新增构造辅助或只读查询，优先围绕 `ConstantFuture` 增加无状态方法，保持两个 `Wait` 均原样返回同一字段。
- 若要把该类型接入 Rust 事务链，应从接收 `Box<dyn sessionctx::OracleFuture>` 的会话实现和 `PrepareTSFuture` 调用点开始，而不是把 isolation crate 的 `TimestampFuture` 当作同一 trait；必要的适配应明确错误类型与所有权转换。
- 若要消除 isolation 层的重复 `ConstantFuture`，先验证两个 trait 的 `Wait` 接收者（`Box<Self>` 与 `&mut self`）、错误类型（`GoError` 与 `TxnError`）及 crate 依赖方向，避免为复用制造循环依赖或改变消费语义。
- 任何行为变化都应同步修改独立测试 [`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs)；至少覆盖固有方法、trait-object 方法、边界值和新增错误分支。若新增生产接线，还应在对应 isolation/session 独立测试中验证预置 TS、snapshot/stale-read 以及重复等待策略。
- 性能风险很低，但不应在常量路径中引入网络等待、锁或不必要分配；兼容性重点是保持 Go 风格 `Wait`、公开构造形式以及 trait-object 可用性。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被完整读取为 41 行。
- RustCodeGraph `query/node`：确认目标符号为 `pkg/sessiontxn/future.rs::ConstantFuture`，确认 [`OracleFuture`](../sessionctx/context.rs) 的签名为 `trait OracleFuture: Send { fn Wait(self: Box<Self>) -> Result<u64, GoError>; }`。
- RustCodeGraph `callers/callees`：目标 `ConstantFuture` 均未返回生产调用边；文件关系只列出 [`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs)。因此本文将 Rust 生产接线明确标为未由当前代码证明。
- 已核对源码与装配：[`future.rs`](future.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`sessionctx/context.rs`](../sessionctx/context.rs)。
- 已核对 Rust 测试与相邻实现：[`sessiontxn_aster_unit_test.rs`](sessiontxn_aster_unit_test.rs)、[`isolation/base.rs`](isolation/base.rs)、[`isolation/readcommitted.rs`](isolation/readcommitted.rs)、[`isolation/main_test.rs`](isolation/main_test.rs)。
- 已核对 Go 对照与实际调用：[`future.go`](future.go)、[`isolation/base.go`](isolation/base.go)、[`isolation/readcommitted.go`](isolation/readcommitted.go)、[`staleread/provider.go`](staleread/provider.go)、[`session/syssession/session_integration_test.go`](../session/syssession/session_integration_test.go)。
- 精确 `rg` 搜索用于区分两个 Rust 同名类型并补足图未表达的 Go 调用；未运行 Cargo，符合本纯文档任务的验证计划。
