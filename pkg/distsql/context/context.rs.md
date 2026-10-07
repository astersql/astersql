# `pkg/distsql/context/context.rs`

源文件：[`context.rs`](./context.rs)

## 文件定位

本文件实现独立 crate `astersql-distsql-context` 的核心数据结构 `DistSQLContext<'a>`。它把一次 DistSQL/Coprocessor 请求需要的会话、语句、KV、TiFlash、资源控制和观测信息收拢为一个可传递的上下文，并提供默认构造、告警转发和脱离会话后的复制语义。crate 入口 [`lib.rs`](./lib.rs) 将本文件声明为私有模块后公开重导出全部符号；[`Cargo.toml`](./Cargo.toml) 以 `pkg/distsql/context` 为 Go 对照包，并声明 KV、错误上下文、执行统计、内存追踪、CPU 统计、SQLKiller 和 TiFlash 等依赖。

当前 Rust 接线并不等同于 Go 的完整应用范围。明确的生产消费者是 [`pkg/ddl/backfilling_txn_executor.rs`](../../ddl/backfilling_txn_executor.rs) 中的 `new_default_reorg_dist_sql_context` 与 `new_reorg_dist_sql_context_with_reorg_meta`，它们为 DDL 回填扫描设置 Chunk RPC、跳过块缓存和资源组。主 `astersql-distsql` crate 仅在 Windows 条件依赖中引用本 crate，其 [`pkg/distsql/distsql.rs`](../distsql.rs) 仍有一套不同的本地 `DistSQLContext`；因此不能把本文件描述成所有 Rust DistSQL 请求已经统一使用的唯一上下文。

## 核心职责

1. `DistSQLContext<'a>` 保存请求发送和结果处理所需的配置与共享服务，包括 KV client、副本读、一致性、分页、TiFlash 配额、资源组、超时、最大读键数、告警、错误策略、内存/CPU/运行时统计和 kill 信号（`DistSQLContext`）。
2. `Default::default` 提供可安全构造和逐字段覆盖的零值基线，并保证 `WarnHandler` 与 `ErrCtx` 共享同一个静态告警接收器（`default`）。
3. `AppendWarning` 把下游产生的共享错误原样转发给 `WarnHandler`，不在本层分类、改写或吞掉（`AppendWarning`）。
4. 手写 `Clone` 明确标量按值复制、字符串拥有式复制、`Arc` 增加引用计数，以及 `KVVars`、`TryCopLiteWorker` 的特殊重建规则（`Clone::clone`）。
5. `Detach` 为可能越过当前语句生命周期继续工作的执行上下文建立副本：大部分字段保留 Go 的浅复制关系，CPU 与 KV 变量独立，最大读键计数器重新清零（`Detach`）。

## 主要符号

- `WarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>`：线程安全的共享告警接收器。`AppendWarning` 和默认错误上下文都围绕它汇聚告警。
- `SharedContextValue = Arc<dyn Any + Send + Sync>`：对本 crate 不拥有接口定义的 Go 字段做类型擦除。当前用于 `KvExecCounter`、`RunawayChecker`、`RUConsumptionReporter`；本文件只保持对象身份并透传，不能在此调用领域方法。
- `DistSQLContext<'a>`：公开上下文。`'a` 主要约束借用的 `SQLKiller`，以及 `tikvstore::Variables<'a>` 内的 `Killed` 原子信号引用。
- `impl Clone for DistSQLContext`：逐字段实现浅/深复制边界。`Arc` 字段共享；`String` 独立拥有相同文本；`KVVars` 新建结构并复制两个退避值和原 `Killed` 引用；`TryCopLiteWorker` 读取当前原子值后建立新原子。
- `impl Default for DistSQLContext`：建立静态告警器和与之绑定的 `ErrCtx`，副本读默认为 `ReplicaReadLeader`，优先级为 `NoPriority`，时长为零，其余布尔、数值、字符串和 `Option` 均取关闭/零/空。
- `AppendWarning(&self, errors::SharedError)`：单步调用 `WarnHandler.AppendWarning`。
- `Detach(&self) -> Box<DistSQLContext<'a>>`：在 `clone` 结果上重建需要独立的字段，返回堆分配上下文。它要求 `SQLKiller`、`CPUUsage`、`KVVars` 已存在，否则通过 `expect` panic。

字段可按用途归组：请求入口为 `Client`、`KVVars`、`DistSQLConcurrency`、`StoreBatchSize`；读语义为 `ReplicaReadType`、`WeakConsistency`、`RCCheckTS`、`NotFillCache`；分页为 `EnablePaging` 和三个 paging size；TiFlash 为 `TiFlashReplicaRead`、线程/内存/落盘阈值及 hash join 版本；资源控制为 limiter、tagger、资源组、runaway checker 和 RU reporter；观测与控制为 warning/error、内存、CPU、运行时统计、执行细节、SQLKiller、超时和读键预算；诊断身份为 SQL、task、connection、session alias 和 request source。

## 执行流程

默认构造流程：`DistSQLContext::default` 先创建 `NewStaticWarnHandler(0)`，再把其克隆传给 `errctx::NewContext`，随后填入默认副本读和优先级，其他字段归零。调用者通常使用结构体更新语法覆盖所需字段；DDL 回填构造器就是这一模式。

告警流程：调用方把 `errors::SharedError` 传给 `AppendWarning`，方法直接调用共享 `WarnHandler`。同一个 handler 被其他上下文或 `ErrCtx` 持有时，追加结果对所有持有者可见。独立 Rust 测试先追加告警，再以 `WarningCount()` 验证转发完成。

分离流程：

1. `Detach` 先调用手写 `clone`，得到保留所有配置值和大部分共享对象身份的副本。
2. 从原上下文取出非空 `SQLKiller`，继续共享同一对象，使分离任务仍能观察同一个 kill signal。
3. 从原 `CPUUsage` 读取累计值，创建新的 `SQLCPUUsages` 并写入该快照；后续两边的 CPU 更新互不影响。
4. 从原 `KVVars` 复制 `BackoffLockFast`、`BackOffWeight`，同时把 `Killed` 明确指回共享 `SQLKiller.Signal`。
5. 原 `MaxKeysReadCounter` 存在时，副本获得新的 `AtomicU64(0)`；不存在时继续为 `None`。
6. 返回 `Box<DistSQLContext>`。除上述例外外，标量配置保持值相等，`Arc` 领域对象保持共享。

## 数据与状态

本文件本身不发送 RPC，也不实现分页、限流或 TiFlash 算法；它保存这些机制的输入和共享对象。布尔/数值/枚举字段是请求策略快照，`String` 字段是拥有式元数据，`Option<Arc<_>>` 字段表达“能力可能未安装”并允许并发共享。`Client`、tracker、stats、limiter、tagger 等对象在 `Clone`/`Detach` 后仍共享，因此内部可变状态也可能共同可见。

需要特别区分三个原子状态：`SQLKiller.Signal` 在分离前后共享；`MaxKeysReadCounter` 在普通 `Clone` 中共享，但在 `Detach` 中若存在则换成零值独立计数器；`TryCopLiteWorker` 在每次 `Clone` 时按当前值建立独立 `AtomicU32`。后者的注释规定同一上下文同时只能有一个 cop-reader 使用 lite worker，但本文件只提供标志，不实现抢占协议。

`ErrCtx` 通过自身的 `Clone` 复制；默认值与 `WarnHandler` 指向同一告警汇聚点。`SharedContextValue` 的类型擦除意味着类型安全的具体操作必须留在拥有真实接口的下游包中，本文件只能复制 `Arc`。

## 依赖与调用关系

下游依赖由 [`Cargo.toml`](./Cargo.toml) 和 [`lib.rs`](./lib.rs) 的重导出共同界定：`astersql-errctx` 提供错误和告警接口，`astersql-kv` 提供 client、变量、副本读、limiter 和 tagger，`astersql-util-memory`、`astersql-util-execdetails`、`astersql-util-ppcpuusage` 提供资源与观测对象，`astersql-util-sqlkiller` 提供终止信号，`astersql-util-tiflash` 与 `astersql-parser-mysql` 提供枚举/常量，`chrono-tz` 提供时区。

RustCodeGraph 将 `context.rs` 标记为被 12 个文件使用，并识别 `DistSQLContext`、`Detach`、`AppendWarning` 及对应测试符号；但对两个方法执行精确 callers/callees 查询没有返回静态调用边。因此直接接线以源码引用核验：

- [`pkg/ddl/backfilling_txn_executor.rs`](../../ddl/backfilling_txn_executor.rs) 直接导入本 crate，构造 DDL reorg 扫描上下文。
- [`pkg/distsql/context_test.rs`](../context_test.rs) 在 Windows 条件下为主 DistSQL 测试创建默认上下文。
- [`context_test.rs`](./context_test.rs) 与 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接调用 `AppendWarning`、`Detach` 并验证复制关系。
- `pkg/distsql/Cargo.toml` 仅在 `cfg(target_os = "windows")` 下声明本 crate；`pkg/ddl/Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/session/Cargo.toml` 则声明直接依赖，但当前源码搜索只确认 DDL 有生产导入。

## 错误处理与边界

`AppendWarning` 不返回 `Result`，其行为取决于注入的 `WarnAppender`；本层没有恢复或降级逻辑。`Default` 允许许多能力为空或为零，因此默认上下文是构造基线，不代表可直接执行真实 KV 请求：例如 `Client`、`SQLKiller`、`CPUUsage`、`KVVars` 默认均为 `None`。

`Detach` 的前置条件是 `SQLKiller`、`CPUUsage`、`KVVars` 全部为 `Some`。任何一个缺失都会以固定 `expect` 消息 panic；方法不以类型或 `Result` 编码该前置条件。调用者在引入新的分离路径时必须先填齐三者，不能直接对 `default()` 调用 `Detach`。`MaxKeysReadCounter` 是可选例外，缺失不会报错。

数值字段在本层不校验范围或组合关系，例如分页上下限、并发度、spill ratio、超时和内存阈值均可为零或不合理值；有效性由构造者或消费方负责。`SharedContextValue` 也不验证具体类型。Rust 独立测试验证 Go `int` 对应字段使用 `isize`，避免在不同指针宽度上无意缩窄。

## 并发与资源生命周期

所有跨线程共享别名都要求 `Send + Sync` 并放入 `Arc`。`Clone` 和 `Detach` 对这些字段通常只增加引用计数，因此 tracker、warning、runtime stats、exec details、request limiter、runaway checker 与 RU reporter 的生命周期可以超过原上下文，但它们的共享可变行为由各自实现保证。

`DistSQLContext<'a>` 并不是完全 `'static` 的所有权容器：它借用 `SQLKiller`，`KVVars.Killed` 也借用原子信号。`Detach` 返回的 `Box` 仍带同一生命周期 `'a`，并没有通过不安全代码延长引用；调用方必须保证 killer 活得足够久。CPU 快照和最大读键计数器独立，防止后台执行继续修改原语句的这两类统计；kill signal 刻意共享，便于停止后台 cursor，但 Go 源码注释也指出语句间重置会使该终止语义不完全稳定。

`Ordering::Relaxed` 仅用于复制 `TryCopLiteWorker` 当前数值；这里不建立跨线程 happens-before 关系。字段注释规定它是 lite-worker 占用标志，真正的 compare/exchange 或释放协议应由使用者实现。`Detach` 没有启动线程、创建 channel、持锁或执行 I/O。

## 与 Go 版本的对应关系

Go 基准实现是 [`context.go`](./context.go)，测试是 [`context_test.go`](./context_test.go)。Rust 保留了字段名和总体顺序，`[package.metadata.porting]` 也明确记录 `go-package = "pkg/distsql/context"`。主要类型映射为：Go 接口/指针转为 `Option<Arc<dyn ...>>`、`Arc<具体类型>` 或借用；Go `int` 转为 `isize`；`atomic.Uint64/Uint32` 转为标准库原子类型；Go `time.Location` 转为 `chrono_tz::Tz`；尚未在本 crate 建模的具体接口转为 `SharedContextValue`。

`Detach` 与 Go 一致地先浅复制全部字段，再单独处理 `SQLKiller`、`CPUUsage`、`KVVars`、`MaxKeysReadCounter`：killer 共享，CPU 与 KV 容器独立，KV 的 killed signal 指向共享 killer，读键计数器重新置零。Rust 额外通过 `Option::expect` 明确了 Go 中对 nil 解引用会失败的隐含前置条件，并通过生命周期防止引用悬垂。

差异和迁移限制包括：Rust 的 `KvExecCounter`、`RunawayChecker`、`RUConsumptionReporter` 被类型擦除，无法在本 crate 调用 Go 接口能力；`Client` 是 `Option<Arc<dyn kv::Client>>`；默认实现是 Rust 为便于构造补充的 API；主 Rust DistSQL 尚未在所有目标和执行路径统一采用本 crate。Go 测试用递归深拷贝断言覆盖大量字段，Rust [`context_test.rs`](./context_test.rs) 和迁移测试用显式值与 `Arc::ptr_eq`/原始指针比较表达同一不变量。

## 扩展指南

新增上下文字段时至少同步四处：`DistSQLContext` 定义、手写 `Clone`、`Default`、`Detach` 的共享/独立策略；遗漏 `Clone` 会直接导致编译失败，但选择错误的复制方式仍可能通过编译并造成跨语句状态污染。若字段来自 Go，还应同步核对 [`context.go`](./context.go) 和 [`context_test.go`](./context_test.go)，明确 Go 是值复制、指针共享还是 `Detach` 特例。

新增行为测试应放在独立 [`context_test.rs`](./context_test.rs) 或迁移测试文件中，不要内嵌进生产文件。测试至少覆盖：默认值是否可用、普通 `Clone` 的对象身份、`Detach` 后共享与独立状态、原对象变更是否影响副本、可选字段的 `None` 分支，以及必需字段缺失时的 panic 合约。涉及主 DistSQL 接线时还要核对 Windows 条件依赖和 [`pkg/distsql/context_test.rs`](../context_test.rs)；涉及 DDL 则同步检查回填构造器及其独立测试。

性能上应避免把大对象从 `Arc` 浅复制改为深复制；正确性上不要让分离后的计数器意外共享或让 kill signal 意外断开；兼容性上保持 Go `int`/时长/枚举的宽度与零值语义。若要用具体 trait 替换 `SharedContextValue`，需要先在实际拥有接口语义的 crate 建立稳定边界，而不是在本文件猜造缩减版接口。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标目录 6 个文件；`files --filter pkg/distsql/context`；`node --file` 完整读取 [`context.rs`](./context.rs)、[`context_test.rs`](./context_test.rs)、[`lib.rs`](./lib.rs)、[`context.go`](./context.go)、[`context_test.go`](./context_test.go)；`query` 定位 Rust `DistSQLContext`、`Detach`、`AppendWarning`；精确 `callers`/`callees` 未返回边，已用直接引用搜索补证。
- crate 与入口：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`pkg/distsql/Cargo.toml`](../Cargo.toml)、[`pkg/distsql/lib.rs`](../lib.rs)。目标包及父包均无 `doc.go`。
- Rust 调用与测试：[`pkg/ddl/backfilling_txn_executor.rs`](../../ddl/backfilling_txn_executor.rs)、[`pkg/distsql/context_test.rs`](../context_test.rs)、[`context_test.rs`](./context_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。
- Go 对照：[`context.go`](./context.go)、[`context_test.go`](./context_test.go)。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证要求本文恰好包含上述 11 个固定二级标题。
