# `pkg/executor/internal/exec/executor.rs`

## 文件定位

该文件是 `astersql-executor-internal-exec` crate 的执行器公共契约与基础实现，模块由同目录 `lib.rs` 以 `pub mod executor` 导出。它把批量 Volcano 执行模型抽象为 `Executor::Open`、`Executor::Next`、`Executor::Close`，并集中提供结果块分配、执行树元数据、运行时统计、查询中断、TopSQL 登记门闩和系统会话借还等通用能力。

当前 Rust 实现是自包含的迁移边界：`Cargo.toml` 把完整 TiDB 子系统依赖全部放在 `cfg(windows)` 下，而本文件本身只依赖标准库。生产接线可见于 `pkg/executor/staticrecordset/recordset.rs`（消费 `Executor`、`Chunk`、`ExecContext`、`NewFirstChunk`、`Open`、`Next`、`Close`）以及 `pkg/executor/internal/testutil/testutil.rs`（构造实现该 trait 的测试执行器）。它并不代表仓库内所有 Rust 执行器已经统一迁移到这套 trait；大量执行器仍有各自的泛型运行时接口。

## 核心职责

- 定义执行器及其错误、schema、字段类型、结果块和调用上下文的最小公共模型：`Error`、`Result<T>`、`FieldType`、`Schema`、`Chunk`、`ExecContext`、`Executor`。
- 将 schema、子节点、返回类型和 plan id 收拢到 `ExecutorMeta`，将 Chunk 尺寸策略收拢到 `ExecutorChunkAllocator`。
- 由 `BaseExecutorV2` 实现默认的递归打开、空结果 `Next`、尽力关闭所有子节点，以及统计和 SQL Killer 挂钩；由 `BaseExecutor` 再绑定 `SessionContext`。
- 用自由函数 `Open`、`Next`、`Close` 包装具体执行器调用，统一捕获 panic、计时，并在 `Next` 前后检查查询中断。
- 提供 `TryNewCacheChunk`、`RetTypes`、`NewFirstChunk` 等调用侧辅助函数，以及系统会话的借用、restricted 标记、回滚和归还策略。

## 主要符号

- `Error::{Killed, Panic, Session, Other}`：本模块统一错误。`Display` 将前两者规范化为 `query interrupted` 和 `executor panic`，后两者保留消息。
- `Chunk`：只保存行数、列数、初始容量和最大行数；`SetNumRows` 用 `min(max_size)` 强制上限，`Reset` 仅清零行数。它是迁移期模型，并不存放真实列数据。
- `Executor: Send`：公开生命周期和结构接口。实现者必须提供类型名、生命周期方法、schema/返回类型、Chunk 尺寸和子节点管理；统计、killer、TopSQL、`Detach` 有无操作默认值。
- `ExecutorMeta::new`：从可选 `Schema` 克隆 `FieldType` 列表，保存 `id` 与拥有所有权的 `Vec<Box<dyn Executor>>`。`Schema` 在缺失时返回空 schema，`GetSchema` 则保留“缺失”语义。
- `ExecutorChunkAllocator`：在构造时快照 `SessionVars` 的 `init_chunk_size`、`max_chunk_size`，并以返回类型创建 `Chunk`；两个 setter 只影响后续分配。
- `StatementContext` / `SessionVars`：分别承载语句摘要、TopSQL 开关、按 plan id 的统计表，以及会话级 Chunk 配置和共享 killed 标志。默认 Chunk 尺寸为 32/1024。
- `ExecutorStats::new`：仅在 `id > 0` 时从 `StatementContext::runtime_stats` 取得或创建共享统计项；互斥锁中毒时不产生统计句柄。`RegisterSQLAndPlanInExecForTopProfiling` 只在启用时原子置位 `registered`。
- `BaseExecutorV2::{NewBaseExecutorV2, BuildNewBaseExecutorV2}`：前者从 `SessionVars` 建立元数据、分配器、统计和 killed 句柄；后者复用现有实例的会话侧共享状态和尺寸配置，但替换 schema、子树、返回类型与 plan id 统计项。
- `BaseExecutor`：在 `BaseExecutorV2` 外绑定 `Arc<dyn SessionContext>`；提供 `Ctx`、`UpdateDeltaForTableID`、`GetSysSession`、`ReleaseSysSession`。
- `Open` / `Next` / `Close`：应优先于直接调用 trait 方法的公共包装入口；三者均用 `catch_unwind(AssertUnwindSafe(...))` 把 unwind 映射为 `Error::Panic`。

## 执行流程

1. 构建计划节点时，调用 `BaseExecutorV2::NewBaseExecutorV2`（需要完整会话能力则调用 `BaseExecutor::NewBaseExecutor`）。schema 被转换为返回字段类型，子节点所有权进入 `ExecutorMeta`；正 plan id 对应的统计句柄在此登记。
2. 调用侧通过自由函数 `Open(ctx, executor)` 进入。包装层开始计时并捕获 panic；`BaseExecutorV2::Open` 按子节点存储顺序递归调用同一个包装函数，遇到第一个错误立即返回，因此后续子节点不会打开。
3. 调用侧通常通过 `Executor::NewChunk` 或 `NewFirstChunk` 按当前返回类型、初始容量和最大尺寸创建输出块。具体执行器每轮负责写入结果；接口注释和 Go 契约要求复用前先重置 Chunk，但 Rust 包装函数不会代替实现者调用 `Reset`。
4. 每轮 `Next` 先计时，然后在 unwind 边界内先执行 `HandleSQLKillerSignal`，再尝试登记 TopSQL，调用具体 `Executor::Next`，成功后再次检查 killed 标志。任何一步出错都会成为本轮结果；即使失败或 panic，统计仍按调用结束时的 `req.NumRows()` 累加。
5. 收尾通过自由函数 `Close(executor)`。`BaseExecutorV2::Close` 会遍历并尝试关闭全部子节点，保留第一个错误但不因它跳过后续清理；包装层再记录关闭耗时并转换 panic。
6. 需要内部 SQL 会话时，`BaseExecutor::GetSysSession` 从池取出对象并设为 restricted SQL。`ReleaseSysSession` 先回滚：成功才放回池，失败则直接关闭，避免污染会话重新入池。

## 数据与状态

- `ExecutorMeta` 拥有子执行器，执行树因而通过 `Box<dyn Executor>` 形成。`SetChildren(index, ...)` 依赖有效下标，越界会 panic；公共包装入口可在生命周期调用栈中将该 panic 转成 `Error::Panic`，直接调用则不会。
- `Schema`、`FieldType` 和 `Chunk` 都是轻量值类型。返回 schema/字段类型时发生克隆；`BuildNewBaseExecutorV2` 也会把新 schema 的字段类型复制到克隆后的分配器。
- `StatementContext::runtime_stats` 是 `Mutex<HashMap<i32, Arc<Mutex<BasicRuntimeStats>>>>`。同一正 plan id 的执行器共享同一统计对象；计数采用 `saturating_add`，避免行数溢出回绕。
- `SessionVars::killed`、语句上下文、单项统计均通过 `Arc` 共享。killer 使用 Acquire 读取；TopSQL 门闩以 `compare_exchange(false, true, AcqRel, Acquire)` 保证一次性状态转换。
- `ExecutorStats` 保存共享语句上下文，因此由 `BuildNewBaseExecutorV2` 派生的节点会共享 TopSQL 门闩和统计表，但为新 id 获取单独统计项。

## 依赖与调用关系

- crate 边界：`pkg/executor/internal/exec/Cargo.toml` 声明库入口为 `lib.rs`，端口元数据指向 Go 包 `pkg/executor/internal/exec`。当前非 Windows 构建下本文件仅使用 `std::{collections, fmt, panic, sync, time}`；清单中的完整领域依赖均为 Windows 条件依赖。
- 上游调用：`pkg/executor/staticrecordset/recordset.rs` 持有 `Box<dyn Executor>`，首次取数时调用 `exec::Open` 并用 `exec::NewFirstChunk` 分配缓冲，随后经 `exec::Next` 拉取批次、经 `exec::Close` 释放；这是目前最直接的生产消费证据。
- 测试/构造上游：`pkg/executor/internal/testutil/testutil.rs` 使用本模块的 `BaseExecutorV2`、`Executor`、schema 和 Chunk 类型构造模拟物理计划；`pkg/executor/staticrecordset/integration_test.rs` 以实现该 trait 的执行器验证 recordset 接线。
- 下游调用：本文件没有外部函数依赖；生命周期递归通过自身的自由函数 `Open`/`Close`，会话资源则通过 `SessionContext -> SessionPool -> SystemSession` 三层 trait 反转控制。
- RustCodeGraph 将本文件标记为被 16 个文件使用，并能精确解析 Rust/Go 两侧的 `Executor`、`NewBaseExecutorV2`、`BuildNewBaseExecutorV2`、`NewBaseExecutor`；对 `Open`/`Next`/`Close` 这类通用名称，图查询存在跨仓库同名噪声，因此具体接线由上述精确 import 和源码调用补充核验。

## 错误处理与边界

- `Open`、`Next`、`Close` 只捕获 unwind panic，不捕获进程 abort；panic payload 也不会保留，统一降格为 `Error::Panic`。
- `BaseExecutorV2::Open` 是“首错停止”，且不会自动关闭已成功打开的较早子节点；调用方必须在失败路径决定是否调用 `Close`。`Close` 则是“全部尝试、首错返回”。
- `Next` 在具体执行前后都检查 killed，覆盖调用前已取消和执行期间被取消两种情况。如果具体 `Next` 已返回错误，则后置 killer 检查不会执行，保留原始执行错误。
- 统计锁或统计表锁中毒时，代码选择跳过创建/写入统计而不让查询失败；这使可观测性降级与查询行为解耦。
- `Chunk::SetNumRows` 静默截断到 `max_size`，不会报告实现者尝试写入过多行；同时它不验证 `capacity`。真实数据容量和类型正确性仍需具体 Chunk 实现迁移后保证。
- `GetSysSession` 的取池错误原样传播；`ReleaseSysSession` 不返回回滚/关闭错误，并且 Rust 签名要求实际 `Box<dyn SystemSession>`，不存在 Go 版的 nil 早返回分支。

## 并发与资源生命周期

`Executor` 要求 `Send`，但生命周期方法接收 `&mut self`；安全 Rust 调用侧不能在没有额外同步封装时并发执行同一个节点的 `Next` 与 `Close`。这比 Go 接口注释所承诺的“`Close` 可与 `Next` 同时调用”更窄，当前文件没有提供该并发保证。

共享状态的并发边界明确：killed 与 TopSQL 门闩使用原子变量，统计表及每项统计使用互斥锁，执行树和 Chunk 则依靠独占可变借用。`Arc<dyn SessionContext>`、`SessionPool: Send + Sync` 允许跨线程共享上下文和池；取出的 `SystemSession: Send` 在释放前由调用者独占。资源回收的关键不变量是：子执行器关闭时不能因某个兄弟失败而跳过其余兄弟；系统会话只有成功回滚后才能回池。

## 与 Go 版本的对应关系

Rust 的 `Executor`、`ExecutorMeta`、`ExecutorChunkAllocator`、`ExecutorStats`、`BaseExecutorV2`、`BaseExecutor` 以及辅助自由函数，逐一对应同路径 `executor.go` 中的同名或小写辅助类型。核心顺序保持一致：递归打开子树；关闭全部子节点并返回首错；`Next` 前后检查 killer；运行时统计包围实际调用；系统会话回滚成功才归还。

需要明确的迁移差异如下：

- Go `chunk.Chunk`、`expression.Schema`、`types.FieldType` 是完整实现；Rust 当前类型只保存本文件测试所需的尺寸和 `type_code`。
- Go Chunk 由会话 allocator pool 分配；Rust `ExecutorChunkAllocator` 直接构造值，没有池与缓存复用，所以 `TryNewCacheChunk` 名称中的“Cache”目前只保留 API 意图。
- Go `Next` 创建按具体执行器类型命名的 tracing region；Rust 没有 tracing region。
- Go TopSQL 路径实际调用 `topsql.RegisterSQL/RegisterPlan` 并持有 digest；Rust 只设置一次性 `registered` 标志，字符串和 digest 字段未被注册逻辑消费。
- Go killer 通过通用 `signalHandler`；Rust 直接读取共享 `AtomicBool`。Go 的 panic 转换保留 `util.GetRecoverError` 信息，Rust 统一为 `Error::Panic`。
- Go `BuildNewBaseExecutorV2` 可显式接收新的 `RuntimeStatsColl`；Rust 始终从共享 `StatementContext` 按新 id 取统计项。Go allocator 支持 failpoint 覆盖尺寸，Rust 无对应注入点。
- Go `BaseExecutor` 嵌入 `BaseExecutorV2` 并自然实现接口；Rust 使用 `base` 字段且没有为 `BaseExecutor` 实现 `Executor`，调用者需显式转发或使用内部 `base`。
- Go `Detach` 的契约要求返回前后对象都可继续使用；Rust 默认恒为 `(None, false)`，本文件没有真正 detach 实现。

因此，新增依赖真实列数据、池化分配、完整 TopSQL、tracing 或 `Next`/`Close` 并发语义的功能时，不能把当前占位抽象误判为已经与 Go 完全等价。

## 扩展指南

- 新增通用执行器能力时，先判断它属于所有实现者必须提供的 `Executor` 方法，还是可以由 `BaseExecutorV2`/包装自由函数统一实现。新增必选 trait 方法会影响所有实现者，优先提供有安全默认值的方法以控制迁移范围。
- 修改生命周期顺序时，应同步检查 `Open`、`Next`、`Close` 自由函数和 `BaseExecutorV2` 的递归实现；特别保留 `Next` 双重 killer 检查、`Close` 全量清理/首错返回和失败调用仍计时的行为。
- 扩展 schema 或 Chunk 前，应替换/增强 `FieldType`、`Schema`、`Chunk` 及 `ExecutorChunkAllocator`，并检查 `ExecutorMeta::new`、`BuildNewBaseExecutorV2`、`NewFirstChunk` 的派生规则；注意性能风险来自频繁克隆字段类型和丢失池化复用。
- 扩展统计或 TopSQL 时，接入点是 `StatementContext`、`ExecutorStats::new` 与 `RegisterSQLAndPlanInExecForTopProfiling`。必须保持同一 plan id 的共享、锁中毒不改变查询结果，以及一次性登记语义。
- 扩展系统会话时，接入 `SessionContext`、`SessionPool`、`SystemSession` 和 `BaseExecutor::{GetSysSession, ReleaseSysSession}`；新路径必须证明异常会话不会回池。
- 回归测试继续放在独立的 `pkg/executor/internal/exec/executor_test.rs`，不要内嵌到生产文件。至少覆盖成功、具体错误、panic、kill 前后置检查、统计、子节点局部打开失败、多个 close 错误、统计锁异常和系统会话回滚失败；跨 crate 接线可同步扩充 `pkg/executor/staticrecordset/integration_test.rs`。
- 若追求 Go 等价，优先补齐真实 Chunk/schema 类型、tracing/TopSQL 行为和 `BaseExecutor` trait 转发；并单独评估 Go 声明的并发 Close 契约，因为仅增加方法体不能绕过 Rust 的 `&mut self` 独占模型。

## 验证依据

- 目标源码：`pkg/executor/internal/exec/executor.rs`，完整核对 516 行；主要符号包括 `Executor`、`ExecutorMeta`、`ExecutorChunkAllocator`、`StatementContext`、`ExecutorStats`、`BaseExecutorV2`、`BaseExecutor`、`Open`、`Next`、`Close`。
- RustCodeGraph：运行 `status`、`files --filter pkg/executor/internal/exec`、针对目标文件的 `node --file ... --offset ... --limit ...`、`explore`，以及 `query BuildNewBaseExecutorV2`、`query NewBaseExecutor`、`query NewBaseExecutorV2`、`query Executor --kind trait`；图确认 Rust/Go 对应符号并报告目标文件的 16 个使用文件。
- crate 与模块边界：`pkg/executor/internal/exec/Cargo.toml`、`pkg/executor/internal/exec/lib.rs`。
- Go 对照：`pkg/executor/internal/exec/executor.go`，核对接口、基类、统计、killer、Chunk 分配、系统会话和三个生命周期包装函数。
- Rust 独立测试：`pkg/executor/internal/exec/executor_test.rs`，覆盖递归 Open/Close、Open 耗时、字段列数与 Chunk 尺寸、最大行数截断、kill 前置短路和 panic 转换。该文件当前未覆盖 `Next` 成功统计、后置 kill、TopSQL、派生构造器、系统会话及多子节点错误组合，这些不能据现有测试宣称已验证。
- 直接调用证据：`pkg/executor/staticrecordset/recordset.rs`、`pkg/executor/staticrecordset/integration_test.rs`、`pkg/executor/internal/testutil/testutil.rs`。任务为纯文档分析，按计划未运行 Cargo 或代码测试；最终仅执行固定十一章节的结构验证并人工复核结论来源。
