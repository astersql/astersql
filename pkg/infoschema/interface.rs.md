# [`pkg/infoschema/interface.rs`](interface.rs)

## 文件定位

本文件位于 `astersql-infoschema` crate 内，定义一个不依赖 session、planner 或异步运行时的只读请求上下文抽象。模块本身在 `pkg/infoschema/lib.rs` 中以私有 `mod interface` 装配，但六个公开项 `RequestContext`、`ContextError`、`Background`、`BackgroundArc`、`TODO`、`TODOArc` 均由 crate 根重新导出，因此外部 crate 不需要访问私有模块名。

当前直接接线点是规划器兼容层：`pkg/planner/core/lib.rs::context` 通过 `infoschema_dependency` 再导出这些项，并把 `RequestContext` 改名为 `Context`；`pkg/planner/core/Cargo.toml` 则声明了对 `astersql-infoschema` 的路径依赖。需要注意，本文件不是 `pkg/infoschema/infoschema.rs` 中 `InfoSchema` trait 的定义位置，也不负责实际 schema 缓存或元数据加载。

## 核心职责

1. 用 `RequestContext` 描述 Go `context.Context` 的四类只读观察能力：截止时间、是否完成、终态错误、请求作用域值。
2. 用 `ContextError::{Cancelled, DeadlineExceeded}` 区分取消与超时两种终态，而不是把它们压缩成布尔值。
3. 提供与 Go `context.Background()` / `context.TODO()` 对应的两个空根上下文；它们永不到期、永不完成、没有错误，也不保存值。
4. 同时提供借用形式与共享所有权形式：`Background` / `TODO` 返回进程期静态引用，`BackgroundArc` / `TODOArc` 返回可跨 API 保存和克隆的 trait object。

本文件只提供接口与空实现，不包含取消触发器、deadline 计时器、父子上下文传播或键值存储容器。源码注释说明该抽象是为 InfoSchema V2 惰性加载和跨库取消检查保留的表面；但仓库当前的 Rust `InfoSchema` trait 方法没有接收 `RequestContext`，因此不能把该设计意图表述成已经接入的 InfoSchema 运行路径。

## 主要符号

- `pub enum ContextError`：可复制、可比较的终态枚举。`Cancelled` 对应 Go `context.Canceled`，`DeadlineExceeded` 对应 `context.DeadlineExceeded`。
- `pub trait RequestContext: Send + Sync`：对象安全的只读接口。`Send + Sync` 允许实现及其 `dyn RequestContext` 引用跨线程传递和共享。
  - `deadline(&self) -> Option<Instant>`：返回单调时钟上的可选截止点。
  - `is_done(&self) -> bool`：表示上下文已经取消或超时。
  - `error(&self) -> Option<ContextError>`：未完成时应为 `None`，完成时给出终态原因。
  - `value(&self, key: &(dyn Any + Send + Sync)) -> Option<&(dyn Any + Send + Sync)>`：按动态类型擦除的键查询值；返回值生命周期受 `self` 约束。
- `struct EmptyContext`：私有、零字段、`Copy` 的空实现；四个 trait 方法分别固定返回 `None`、`false`、`None`、`None`。
- `static BACKGROUND` 与 `static TODO_CONTEXT`：类型相同但语义命名不同的两个 `EmptyContext` 单例。
- `Background() -> &'static dyn RequestContext` 与 `TODO() -> &'static dyn RequestContext`：不分配内存，返回对应单例的静态 trait-object 引用。
- `BackgroundArc() -> Arc<dyn RequestContext>` 与 `TODOArc() -> Arc<dyn RequestContext>`：每次调用创建一个新的 `Arc<EmptyContext>` 并擦除为 `Arc<dyn RequestContext>`；它们不复用两个静态单例。

## 执行流程

借用形式的调用流程很短：调用 `Background` 或 `TODO`，函数取得对应静态 `EmptyContext`，转换为 `&'static dyn RequestContext`；调用方再通过动态分派观察四个属性，得到空上下文的固定结果。

共享形式的调用流程为：调用 `BackgroundArc` 或 `TODOArc`，构造新的零字段 `EmptyContext`，由 `Arc::new` 建立共享所有权，再转换为 `Arc<dyn RequestContext>`。规划器中的构建路径（例如 `pkg/planner/core/expression_rewriter.rs` 与 `logical_plan_builder_runtime.rs`）使用 `TODOArc`，测试和部分执行器路径使用经规划器重导出的 `BackgroundArc`。这些上下文目前充当调用边界的根/占位对象，不会主动驱动取消逻辑。

若未来提供非空实现，调用者预期先通过 `deadline` 或 `is_done` 观察状态，再用 `error` 区分取消与超时，并通过 `value` 读取请求级附加数据；本文件没有规定轮询频率或状态更新机制，这些属于具体实现的责任。

## 数据与状态

`EmptyContext` 没有可变字段，`BACKGROUND` 与 `TODO_CONTEXT` 也没有运行时状态。二者在行为上完全相同，只通过入口名称表达调用者意图：已明确选择后台根上下文，或尚未决定应传入何种上下文。

`ContextError` 是值类型，不携带文本、来源或错误链。`Instant` 使用单调时间语义，适合进程内 deadline 比较，但不是可序列化的墙上时钟时间。`value` 使用 `Any` 做类型擦除；键和值都要求 `Send + Sync`，但接口不定义键的相等规则，具体实现必须自行设计并保证返回引用在 `self` 有效期内可用。

两个 `Arc` 构造器每次都创建独立控制块。由于 `EmptyContext` 是零大小类型，业务状态成本为零，但仍存在 `Arc` 分配和原子引用计数成本；需要静态借用且 API 不要求所有权时应优先使用 `Background` / `TODO`。

## 依赖与调用关系

本文件只依赖标准库：`std::any::Any` 用于类型擦除值，`std::sync::Arc` 用于共享所有权，`std::time::Instant` 用于 deadline。`pkg/infoschema/Cargo.toml` 没有为本文件引入专属第三方依赖；crate 的其他元数据依赖与本接口实现无直接调用关系。

向上游的装配链为 `pkg/infoschema/interface.rs` → `pkg/infoschema/lib.rs` 公开再导出 → `pkg/planner/core/lib.rs::context` 再导出。已检索到的实际使用包括 `pkg/planner/core/expression_rewriter.rs`、`logical_plan_builder_runtime.rs`、`planbuilder_runtime.rs` 对 `TODOArc` 的调用，以及 `pkg/session/runtime/explain_query.rs`、`pkg/executor/statement_ru_plan_walk_test.rs` 等通过 planner context 使用根上下文。

RustCodeGraph 对目标文件中精确的 `RequestContext`、`BackgroundArc`、`TODOArc` 节点执行 callers/callees 查询时未返回跨文件边；因此上述跨 crate 关系由 crate 根再导出、Cargo 依赖和 `rg` 的限定引用共同核验。文档不据此声称 `RequestContext` 已被 Rust InfoSchema V2 的查表实现消费。

## 错误处理与边界

空上下文自身没有失败路径：`deadline`、`error`、`value` 都返回 `None`，`is_done` 返回 `false`。`ContextError` 不是 `std::error::Error`，也不带错误消息；它表达的是可观察状态，不是通用错误传播载体。

trait 依赖实现者维护一致性不变量：`is_done() == false` 时 `error()` 应为 `None`；完成后应返回与实际原因相符的 `ContextError`。编译器不会强制这一关系，本文件也没有默认方法验证它。接口同样没有 `cancel()`，所以观察者不能通过 trait 触发取消。

`value` 的动态类型 API要求调用方和实现方约定键类型并自行 `downcast_ref`；错误类型的键或不存在的键都只能表现为 `None`。因为返回的是借用值，不能在不复制或另行共享的情况下越过上下文本身的生命周期。

## 并发与资源生命周期

`RequestContext: Send + Sync` 是主要并发保证：实现必须能够安全地在线程间移动并由多个线程同时读取。`EmptyContext` 无状态，因此天然满足要求；静态根上下文存活到进程结束，不需要清理。

`Arc<dyn RequestContext>` 允许规划器节点等长生命周期对象持有上下文，克隆 `Arc` 只增加原子引用计数，最后一个所有者释放时销毁具体实现。当前 `EmptyContext` 没有线程、锁、通道、计时器或析构副作用。未来的可取消实现若使用原子变量、锁或唤醒机制，必须自行保证 `deadline`、`is_done` 与 `error` 的并发观察一致，并把资源释放规则写入独立实现及测试，而不是改变空根上下文的语义。

## 与 Go 版本的对应关系

Go `pkg/infoschema/interface.go` 不重新定义上下文，而是在 `InfoSchema` 的 `TableByName(ctx context.Context, ...)` 和 `TableByID(ctx context.Context, ...)` 中直接使用标准库 `context.Context`。Go `pkg/infoschema/infoschema_v2.go::SchemaTableInfos` 将该上下文传入元数据读取，并在 flashback 重试等待时监听 `ctx.Done()`、返回 `ctx.Err()`；这证明取消会影响真实的惰性元数据加载流程。

Rust 文件把 Go 标准接口的只读部分显式映射为 `RequestContext`：`Deadline` 对应 `deadline`，`Done` 的可观察完成状态对应 `is_done`，`Err` 对应 `error`，`Value` 对应 `value`；`ContextError` 映射两个标准终态。与 Go 不同，Rust 没有暴露完成 channel，而是同步布尔查询；值查询使用 `Any` 而非 Go 的任意 `interface{}`。

迁移尚未完全对齐：当前 `pkg/infoschema/infoschema.rs::InfoSchema` 的 `TableByName`、`TableByID`、`SchemaTableInfos` 等签名均不接收 `RequestContext`，`pkg/infoschema/infoschema_v2.rs` 的对应实现也未通过本接口传播取消。因此本文件目前兼具 planner 上下文兼容层和未来 InfoSchema 上下文边界的角色，但不能替代 Go V2 已有的取消传播行为。

## 扩展指南

- 新增真正可取消/可超时的上下文时，实现应放在独立生产文件，并把测试放在独立 `*_test.rs` 文件；至少覆盖 deadline 到期、显式取消、两种 `ContextError`、并发读取和值的类型/作用域。
- 若将上下文接入 Rust InfoSchema，应从 `InfoSchema` trait 与 v1/v2 实现的相关查询签名统一设计，沿调用链传递同一上下文，并针对 Go `SchemaTableInfos` 的 flashback 重试取消行为增加回归测试；不能只修改本文件或用固定空上下文掩盖缺失传播。
- 扩展 `ContextError` 会影响穷举匹配和 Go 兼容语义；应先确认 Go 标准 context 是否存在对应稳定终态。
- 修改根上下文时必须保持永不到期、永不完成、无错误、无值的不变量，并同步 `pkg/infoschema/interface_aster_unit_test.rs::root_contexts_never_finish_or_carry_values`。
- 如调用 API 仅需要借用，使用静态入口可避免 `Arc` 分配；需要存储、克隆或跨所有权边界时再使用 `BackgroundArc` / `TODOArc`。不要依赖 Background 与 TODO 的对象地址相同：源码刻意使用两个静态项表达不同语义。

## 验证依据

- 目标源码：`pkg/infoschema/interface.rs`，核对了枚举、trait、私有空实现、两个静态项及四个构造函数的完整定义。
- crate 装配：`pkg/infoschema/lib.rs` 的 `mod interface` 与公开再导出；`pkg/infoschema/Cargo.toml` 的 crate 边界和依赖。
- 直接上游：`pkg/planner/core/lib.rs::context`、`pkg/planner/core/Cargo.toml`，以及规划器/会话/执行器中对重导出构造器的限定引用。
- Rust 测试：`pkg/infoschema/interface_aster_unit_test.rs::root_contexts_never_finish_or_carry_values` 验证两个静态根上下文的四项空语义；该文件也表明测试逻辑与生产源文件分离。当前没有覆盖两个 `Arc` 构造器或自定义 `RequestContext` 实现的同模块测试。
- Go 对照：`pkg/infoschema/interface.go::InfoSchema` 的上下文参数；`pkg/infoschema/infoschema_v2.go::SchemaTableInfos` 的 `ctx.Done()` / `ctx.Err()` 取消分支，以及 `TableByName`、`TableByID` 的上下文签名。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/infoschema/interface.rs` 确认目标含 16 个符号；`query` 精确定位 `RequestContext`（第 45 行）、`ContextError`（第 29 行）、`BackgroundArc`（第 90 行）、`TODOArc`（第 101 行）；对精确节点执行 callers/callees 未得到跨文件边，因此再导出与调用点使用源码/Cargo/限定搜索补证。
- 本任务只生成说明文档，未运行 Cargo；最终以固定十一章节结构检查和人工事实复核作为验证。
