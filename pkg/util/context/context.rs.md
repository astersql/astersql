# `pkg/util/context/context.rs`

## 文件定位

本文件属于 `astersql-util-context` crate。crate 入口 `pkg/util/context/lib.rs` 以 `pub mod context` 暴露本模块；`pkg/util/context/Cargo.toml` 将 `lib.rs` 指定为库入口，并以 `package.metadata.porting.go-package = "pkg/util/context"` 记录对应 Go 包。

它包含两类基础能力：一是定义可由会话/规划上下文实现的动态值存取契约 `ValueStoreContext`，二是通过进程内全局原子计数器生成上下文 ID。当前文件不保存任何会话值，也不创建 Domain；具体存储和 Domain 生命周期由 trait 的实现者负责。

在已接线的 Rust 主链中，`GenContextID` 被 `pkg/sessionctx/stmtctx/stmtctx.rs::StatementContext::build`、`pkg/expression/exprstatic/evalctx.rs::NewEvalContext` 和 `EvalContext::Apply` 使用，分别给语句上下文和静态表达式求值上下文分配标识。`ValueStoreContext` 还由 `pkg/planner/planctx/lib.rs` 重导出，供规划上下文接口组合使用；`pkg/util/breakpoint/breakpoint.rs::Inject` 则以它约束会话参数并读取动态断点回调。

## 核心职责

- `ValueStoreContext`：规定按可显示 key 设置、查询、清除任意类型值，以及查询 Domain 动态对象的四个操作。它只是对象安全的借用式接口，不规定容器、key 的规范化方式、同步策略或具体 Domain 类型。
- `contextIDGenerator`：保存整个进程内共享的下一个上下文编号状态，初值为 `0`。
- `GenContextID`：对全局计数器执行一次原子加一，并返回加一后的 Go 等价结果；正常初始调用返回 `1`。

该文件不负责错误封装、ID 回收、持久化、跨进程协调或上下文销毁。因而这里的 ID 只在同一进程、同一计数器实例和未发生 `u64` 回绕的区间内提供递增且不重复的分配结果。

## 主要符号

### `pub trait ValueStoreContext`

- `fn SetValue(&mut self, key: &dyn fmt::Display, value: Box<dyn Any>)`：把动态类型值的所有权交给实现者。`&mut self` 要求调用点独占可变借用，但 trait 本身没有要求 `Send`、`Sync` 或内部并发能力。
- `fn Value(&self, key: &dyn fmt::Display) -> Option<&dyn Any>`：借用已保存值；`None` 对应 Go 返回 `nil`。返回引用的生命周期受 `self` 借用约束，调用者若要具体类型须使用 `Any` 的向下转型。
- `fn ClearValue(&mut self, key: &dyn fmt::Display)`：请求实现者移除对应值；接口未规定 key 不存在时的额外行为。
- `fn GetDomain(&self) -> Option<&dyn Any>`：借用动态 Domain；没有 Domain 时返回 `None`，调用者仍需向下转型验证具体类型。

key 参数只有 `fmt::Display` 能力，而不是 `Eq + Hash`。因此 key 如何转换、比较以及是否可能发生相同显示文本冲突，均是实现者的责任，不能从此 trait 推断。

### `pub static contextIDGenerator: AtomicU64`

包级、公开的原子计数器，初值为零。公开性使迁移测试可用 `swap`/`store` 构造回绕边界；生产调用应通过 `GenContextID`，否则直接修改静态状态会破坏分配序列。

### `pub fn GenContextID() -> u64`

执行 `contextIDGenerator.fetch_add(1, Ordering::SeqCst).wrapping_add(1)`。`fetch_add` 修改原子值但返回旧值，随后的 `wrapping_add(1)` 把返回语义对齐 Go `atomic.Uint64.Add(1)`；显式 wrapping 还确保旧值为 `u64::MAX` 时返回 `0`，而不会在 debug 构建中因整数溢出 panic。

## 执行流程

ID 分配流程如下：

1. 调用者创建新的语句或求值上下文。
2. `GenContextID` 以 `SeqCst` 对 `contextIDGenerator` 原子加一；并发调用在该原子操作上形成一个全序。
3. 原子操作返回修改前的值，函数以 wrapping 方式加一后返回。
4. 调用者把结果写入自身 `ctxID`/`id` 字段；本模块不再跟踪该上下文，也没有释放或复用步骤。

具体接线包括：

- `pkg/sessionctx/stmtctx/stmtctx.rs::StatementContext::build` 在构造 `StatementContext` 时写入 `ctxID`。
- `pkg/expression/exprstatic/evalctx.rs::NewEvalContext` 为新建 `EvalContext` 分配 ID。
- `pkg/expression/exprstatic/evalctx.rs::EvalContext::Apply` 在复制状态并应用新选项时创建新的 `EvalContext`，因此重新分配 ID，而不是沿用原对象 ID。

值存取没有可在本文件内展开的执行体：调用 `SetValue`、`Value`、`ClearValue` 或 `GetDomain` 后，控制权立即进入具体 trait 实现。

## 数据与状态

唯一由本文件持有的运行时状态是 `AtomicU64`。它是进程级全局状态，不属于某个 session、statement 或线程；所有链接到同一静态实例并调用 `GenContextID` 的消费者共享一个序列。

`ValueStoreContext` 的 `Box<dyn Any>` 表示设置时转移独占所有权，`Option<&dyn Any>` 表示查询时只借用。与 Go 的 `any` 相比，这个 Rust 签名没有要求值可跨线程，也没有提供共享所有权克隆；实现若需要并发共享，应在所存值或实现内部显式采用 `Arc`、锁等机制。

计数器的边界行为是模 `2^64` 回绕：初始序列为 `1, 2, ...`，原子值达到 `u64::MAX` 后下一次返回 `0`，再下一次返回 `1`。因此“正数”和“不重复”是常规未回绕区间的性质，不是无限期不变量。`pkg/util/context/migration_aster_unit_test.rs::migration_context_id_wraps_like_go_atomic_uint64` 固定了这一兼容行为。

## 依赖与调用关系

本文件只直接依赖 Rust 标准库：

- `std::any::Any` 提供运行时类型擦除与向下转型基础；
- `std::fmt::Display` 近似 Go `fmt.Stringer` 的 key 约束；
- `std::sync::atomic::{AtomicU64, Ordering}` 提供无锁 ID 状态更新。

crate 层面还声明了 errors、parser terror、warning、plan cache 等依赖，但这些服务于 `pkg/util/context` 的其他模块，不是 `context.rs` 的直接依赖。

已核实的上游关系：

- `pkg/util/context/lib.rs` 声明并公开本模块；
- `pkg/sessionctx/stmtctx/stmtctx.rs` 通过依赖别名 `task_context as util_context` 调用 `context::GenContextID`；
- `pkg/expression/exprstatic/lib.rs` 将 `contextutil_crate` 重导出为 `contextutil`，其 `evalctx.rs` 两处调用 `context::GenContextID`；
- `pkg/planner/planctx/Cargo.toml` 以 `contextutil-crate = { package = "astersql-util-context", ... }` 依赖本 crate，`pkg/planner/planctx/lib.rs` 重导出本文件的 `ValueStoreContext`。
- `pkg/util/breakpoint/Cargo.toml` 直接依赖本 crate；`breakpoint.rs::Inject` 通过 `ValueStoreContext::Value` 读取并向下转型断点通知函数，独立测试中的 `MockSessionContext` 展示了一种以 `key.to_string()` 为键的具体实现。

RustCodeGraph 的文件节点确认 `context.rs` 有 7 个索引符号，并给出了 `stmtctx.rs`、`migration_aster_unit_test.rs` 等使用边；精确 `callers GenContextID` 查询未在 30 秒内返回，所以表达式侧两个调用点以直接源码引用核验，不把不完整图结果解释成“没有调用者”。

## 错误处理与边界

本文件没有 `Result`、panic 分支或日志路径：trait 方法也没有错误返回通道。实现者若遇到存储失败、非法 key 或类型不匹配，只能在自己的 API/实现策略中处理；调用者对 `Value`/`GetDomain` 的 `None` 以及 `Any` 向下转型失败必须自行分支。

主要边界如下：

- 不存在的 value 或 Domain 由 `None` 表达；接口不区分“未设置”和“已清理”。
- key 只保证可格式化，不保证稳定、唯一或可哈希；此文件未实现 key 比较。
- `GenContextID` 不接受输入且不会报告失败，但在 `u64` 边界按 Go 语义回绕到零。
- ID 不持久化，进程重启后从零重新开始；它也不是分布式唯一 ID。
- `contextIDGenerator` 为公开静态量，外部直接 `store`、`swap` 或 `fetch_add` 会改变后续结果；当前测试用全局互斥锁隔离其边界修改，但生产代码不应绕过生成函数。

## 并发与资源生命周期

`GenContextID` 使用 `Ordering::SeqCst`，是本文件最强的内存序选择。它保证所有相关顺序一致原子操作可观察为同一个全局顺序，并使并发 `fetch_add` 获得不同旧值；`migration_context_ids_are_unique_across_threads` 用 8 个线程各生成 128 个 ID，验证 1024 个结果在未回绕条件下全部为正且互不重复。

ID 生成不使用锁、不分配堆内存，也没有显式资源清理；静态计数器与进程同寿命。上下文销毁不会回收编号。

`ValueStoreContext` 则没有并发保证。`SetValue`/`ClearValue` 的 `&mut self` 避免同一安全 Rust 借用范围内的并发可变访问，但 trait 未声明 `Send + Sync`，且 `Box<dyn Any>` 也未附带这些界限。实现者需要依据自身共享方式决定是否加锁及如何管理被装箱值和 Domain 的生命周期。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/context/context.go`：

- Go `ValueStoreContext` 的四个方法与 Rust trait 一一对应；`fmt.Stringer` 映射为 `dyn fmt::Display`，`any` 映射为 `dyn Any`，Go `nil` 映射为 `Option::None`。
- Rust `SetValue` 使用 `Box<dyn Any>` 明确转移所有权，`Value`/`GetDomain` 返回受上下文借用约束的引用；这比 Go interface value 的复制/共享语义更严格。
- Go 包级 `atomic.Uint64` 映射为 Rust `AtomicU64`。Go `Add(1)` 返回新值，Rust `fetch_add(1)` 返回旧值，所以 Rust 还要 `wrapping_add(1)`。
- Go 无符号原子加法会回绕；Rust 显式 wrapping 保持相同边界行为。`migration_context_id_wraps_like_go_atomic_uint64` 验证最大值后的返回值为零。

Go 调用面还包括 `pkg/expression/exprstatic/evalctx.go` 的创建与 `Apply`、`pkg/sessionctx/stmtctx/stmtctx.go` 的创建与 `Reset`。Rust 已对齐静态求值上下文的创建/Apply，以及语句上下文的构造；本文件只提供生成器，是否在其他生命周期点重新分配由各消费者决定。

Go `pkg/domain/domainctx.go::GetDomain` 会对 `ValueStoreContext::GetDomain` 返回值做 `*Domain` 类型断言；这印证了 trait 的动态返回值用途。Rust 侧当前未在本文件提供等价的强类型辅助函数。

## 扩展指南

- 新增值存取能力时，应优先扩展 `ValueStoreContext`，并同步所有实现者和组合该 trait 的规划/会话接口；不得在本文件假定某一种 map 或锁实现。测试应放在独立 `*_test.rs` 文件，而不是内嵌到生产源文件。
- 若改变 key 类型，需要评估与 Go `fmt.Stringer` 的兼容性、对象安全性，以及现有实现对显示文本的处理；尤其不要在没有实现证据时把 `Display` 自动等同于唯一字符串 key。
- 若改变 value/Domain 的所有权或线程界限，需要同时检查 `Box<dyn Any>`、借用返回值、`Send + Sync` 需求和 planner 的重导出 API，避免无意破坏对象安全或调用者生命周期。
- 若改变 ID 算法或内存序，必须同步核对 `StatementContext::build`、`NewEvalContext`、`EvalContext::Apply`，并保留并发唯一性与 Go 回绕语义。把 ID 扩展为跨进程唯一标识属于新设计，不能仅替换这个原子计数器后继续声称 Go 等价。
- ID 相关回归应扩展 `pkg/util/context/migration_aster_unit_test.rs`；消费者何时分配新 ID 的语义应在各自独立测试（如 expression/stmtctx 测试）中验证。

兼容风险主要是公开 trait 签名和回绕语义；正确性风险主要来自外部直接修改公开计数器、错误假设 ID 永不回绕，以及对 `Any` 做未检查的具体类型转换。当前原子路径成本固定且很小，但 `SeqCst` 是全局同步热点；若因性能改用更弱内存序，必须先证明调用者只依赖原子唯一分配而不依赖全序，并增加相应并发验证。

## 验证依据

- 生产源码：`pkg/util/context/context.rs`，核对 `ValueStoreContext`、`contextIDGenerator`、`GenContextID` 的完整定义。
- crate 边界：`pkg/util/context/lib.rs`、`pkg/util/context/Cargo.toml`，核对模块公开方式、库入口、依赖和 Go 包迁移元数据。
- Rust 调用点：`pkg/sessionctx/stmtctx/stmtctx.rs::StatementContext::build`；`pkg/expression/exprstatic/evalctx.rs::NewEvalContext`、`EvalContext::Apply`。
- Rust trait 接线：`pkg/planner/planctx/lib.rs` 与 `pkg/planner/planctx/Cargo.toml`，核对 `ValueStoreContext` 的依赖和重导出。
- Rust 独立测试：`pkg/util/context/migration_aster_unit_test.rs::migration_context_ids_are_unique_across_threads`、`migration_context_id_wraps_like_go_atomic_uint64`；`pkg/util/breakpoint/migration_aster_unit_test.rs::MockSessionContext` 及其断点测试覆盖 trait 的具体存取/向下转型用法。同目录 `warn_test.rs` 不测试本文件。
- Go 对照：`pkg/util/context/context.go`；调用/语义证据来自 `pkg/expression/exprstatic/evalctx.go`、`evalctx_test.go`、`exprctx_test.go`、`pkg/sessionctx/stmtctx/stmtctx.go`、`pkg/domain/domainctx.go` 和 `pkg/planner/planctx/context.go`。
- RustCodeGraph：运行 `status`（索引包含 11,467 文件、307,296 节点和 1,848,419 边）、`files --filter pkg/util/context`、目标文件 `node --file`、`query ValueStoreContext`、`query GenContextID` 及限定 `callers`。调用者查询超时的部分由上述直接 Rust 引用补证。
- 文档结构按任务命令验证：目标文件存在，且固定二级标题恰好 11 个。纯文档任务未运行 Cargo。
