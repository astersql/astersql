# `pkg/util/tracing/util.rs`

## 文件定位

本文件是 `astersql-util-tracing` crate 的通用追踪实现，crate 入口 `pkg/util/tracing/lib.rs` 通过 `pub mod util` 加载它并以 `pub use util::*` 重导出。仓库根门面 `pkg/lib.rs` 又在 `util::tracing` 下重导出 `facade_util_tracing`，因此这里定义的是 Rust 侧与 Go `pkg/util/tracing/util.go` 对齐的公共追踪基础类型，而不是某个单一 SQL 算子的私有辅助代码。

它同时承载四组能力：可回调录制的 span 树、不可变派生的追踪 `Context`、进程级事件类别位图，以及把 runtime span、回调 span 和 flight-recorder begin/end 事件组合起来的 `Region`。`Region` 在这里表示一段待计时的代码区间，不是 TiKV 的数据 Region。

`pkg/util/tracing/Cargo.toml` 将该 crate 的库入口指定为 `lib.rs`，运行时依赖只有 `tracing = "0.1"`（本文件用它建立 runtime span）和 crate 其他模块使用的 `serde`；测试另依赖 `serde_json`。manifest 的 `package.metadata.porting.go-package` 明确指向 `pkg/util/tracing`。

## 核心职责

1. `NewRecordedTrace` 安装进程级回调录制器，创建带 `TiDBTrace = "tr"` baggage 的根 span；`Span::finish` 将完成快照转换成 `RawSpan` 并交给回调。
2. `Context` 以 `Arc<ContextData>` 保存 span、事件 sink、独立 trace ID 和 SQL `TraceInfo`，所有 `with_*`/`ContextWith*` API 都通过复制快照派生新实例，不就地修改旧上下文。
3. `TraceCategory` 和 `Enable`、`Disable`、`SetCategories`、`IsEnabled` 管理进程内原子类别掩码；`ParseTraceCategory` 提供稳定名称到单一类别位的反向解析。
4. `StartRegion`、`StartRegionWithNewRootSpan`、`StartRegionEx` 和 `Region::end` 统一管理 runtime span、可录制 span 以及 `General` 类别的 begin/end 事件。
5. `TraceInfoFromContext`/`ContextWithTraceInfo` 传递 SQL 语句关联信息；`WithFlightRecorder`/`GetSink` 和 `ExtractTraceID` 为结构化事件提供接收端与标识。

## 主要符号

- `TiDBTrace: &str`：记录型根 span 的 baggage 键；`NewRecordedTrace` 将其值设为 `"1"`。
- `RawSpan`：回调可见的完成快照，包含操作名、当前/父 span ID、baggage 和按写入顺序保存的日志键值。
- `CallbackRecorder<F>`：轻量回调适配器；`RecordSpan` 直接调用所持闭包。实际 span 使用的擦除类型是私有 `Recorder = Arc<dyn Fn(RawSpan) + Send + Sync>`。
- `Span`/`SpanInner`：`Span` 是可克隆的 `Arc` 句柄；内部保存不可变身份字段，以及受 `Mutex` 保护的 baggage/log、可选 recorder、`AtomicBool` 完成标记和 noop 标记。`finish`、`log_kv`、baggage 读写是主要操作。
- `TraceInfo`：SQL 语句级元数据，字段为 `SessionAlias`、`TraceID`、`ConnectionID`。它与 `ContextData.trace_id` 是两份不同数据：前者是业务关联结构，后者由 `ExtractTraceID` 返回并写入 `Event`。
- `Context`：不可变上下文句柄。`background` 创建空上下文；`with_span`、`with_trace_id` 派生新快照；`same_instance` 只判断两个句柄是否仍共享同一 `Arc`。
- `Sink`/`FlightRecorder`：线程安全事件接收接口；`FlightRecorder` 当前是 `Sink` 的标记型扩展，没有新增方法。
- `TraceCategory(u64)`：类别位图。14 个单类别常量从 `TxnLifecycle` 到 `RegionCache` 占据 bit 0..13，`AllCategories` 是这些位的并集，`TraceCategory::NONE` 为零。
- `Phase` 与 `PhaseBegin`、`PhaseEnd`、异步/流/瞬时常量：沿用 Chrome Trace Event 的相位字符。
- `EventField`/`Event`：结构化事件及其字段；事件保存时间、名称、相位、trace ID、字段和类别。
- `Region`：持有 `tracing::span::EnteredSpan`、可选回调 `Span`、可选 begin 事件及其 sink/context。公开 `Span` 字段保留 Go API 的可观察形态；`End` 是 `end` 的 Go 风格别名。

## 执行流程

### 记录 span

1. `NewRecordedTrace(opName, callback)` 将回调包装成线程安全 `Recorder`，写入 `global_recorder()` 的 `RwLock<Option<Recorder>>`。
2. `Span::new` 用 `NEXT_SPAN_ID.fetch_add(1, Relaxed)` 分配 ID；根的 `parent_span_id` 为 0，子 span 从父句柄复制 baggage，并复用父 recorder。
3. 根 span 写入 `TiDBTrace -> "1"`。调用者可用 `Context::with_span` 挂入上下文，再由 `ChildSpanFromContxt` 或 `StartRegion` 建立子级。
4. `Span::finish` 先以 `finished.swap(true, AcqRel)` 抢占唯一完成权；只有第一次完成且不是 noop 时，才锁住 baggage/log 生成 `RawSpan` 并同步调用 recorder。

### 从 Context 派生 span

- `SpanFromContext` 返回上下文里的克隆句柄；缺失时返回 ID 为 0、无 recorder 的 noop span。
- `ChildSpanFromContxt` 只有在上下文含真实父 span 时才创建子 span并返回写入该子 span 的新 `Context`。父 span 缺失或为 noop 时，它返回 noop 和原 `Context`；`util_test.rs::test_child_span_from_context` 用 `same_instance` 验证这一不派生分支。
- `start_global_span` 读取当前全局 recorder；存在时创建新的根 span，不存在时退化为 noop。它只供 `StartRegionWithNewRootSpan` 使用。

### 类别开关

- `Enable` 用 `fetch_or` 加位，`Disable` 用 `fetch_and(!mask)` 清位，`SetCategories` 整体覆盖，`GetEnabledCategories` 读取快照。
- `IsEnabled` 先检查环境变量 `TIDB_KERNEL_TYPE`：值忽略大小写等于 `classic` 且当前不是 `cfg(test)` 时恒为 false；否则检查目标掩码与全局掩码是否有交集。
- `ParseTraceCategory` 只在 `TraceCategory::KNOWN` 的 14 个单 bit 值中匹配名称；未知名称返回 `NONE`。组合值和未知值的 `Display` 形态是 `unknown(<数值>)`。

### Region 生命周期

1. `StartRegion` 立即调用 `start_runtime_region`，进入名为 `tidb_region`、带 `region_type` 字段的 TRACE 级 `tracing` span。
2. 若上下文含真实父 span，则创建同名可录制子 span；若明确含 noop 父 span，则保留 `Some(noop)` 的 API 形态；若完全没有父 span，则 `Region.Span` 为 `None`。
3. 仅当 `IsEnabled(General)` 且上下文含 sink 时，函数创建并立即记录 `PhaseBegin` 事件，同时在 `Region` 内保存事件副本、sink 和 context，供结束阶段配对。
4. `Region::end(&mut self)` 依次取出并完成回调 span、取出 `EnteredSpan` 以退出 runtime 区间，最后把保存事件的相位改为 `PhaseEnd`、刷新时间并再次交给同一 sink。
5. `StartRegionEx` 在 `StartRegion` 基础上把 `Region.Span`（包括 noop）写回派生上下文。`StartRegionWithNewRootSpan` 则直接把全局 recorder 创建的根 span同时放入 `Region` 和新上下文，不再创建同名子 span，也不产生 flight-recorder begin/end 事件。

## 数据与状态

- 全局 recorder 位于 `OnceLock<RwLock<Option<Recorder>>>`。第一次访问时初始化为空；每次 `NewRecordedTrace` 会替换它，既有 `Span` 已持有自己的 `Arc<Recorder>`，不会因替换而改变回调目标。
- `NEXT_SPAN_ID` 是进程级 `AtomicU64`，从 1 开始单调分配；使用 `Relaxed` 是因为它只要求 ID 唯一分配，不承担其他内存同步。
- `SpanInner.baggage` 与 `logs` 分别由 `Mutex` 保护；子 span 创建时复制父 baggage，此后父子各自修改，不共享同一 map。
- `enabledCategories` 是进程级 `AtomicU64`，默认静态初始化为 0。类别测试会修改共享状态，因此 `lib.rs` 为测试提供 `CATEGORY_TEST_LOCK` 串行化相关用例。
- `Context` 的派生是浅层快照：`ContextData` 被克隆，内部的 `Span`、sink 和 `TraceInfo` 仍通过 `Arc` 共享，`Vec<u8>` trace ID 则按值克隆。
- `Region` 保存 begin 事件的克隆；结束时只修改自己的副本并发出 end 事件，所以 sink 已收到的 begin 值不会被反向改变。

## 依赖与调用关系

- 上游装配：`pkg/util/tracing/lib.rs` 重导出本文件全部公共符号；`pkg/lib.rs::util::tracing` 再通过 `facade_util_tracing` 暴露 crate。多个 workspace crate 的 `Cargo.toml`（包括 `pkg/session`、`pkg/executor`、`pkg/ddl`、`pkg/distsql`、`pkg/server` 和 DXF 组件）声明了 `astersql-util-tracing` 路径依赖。
- 生产调用现状：本次对 `astersql_util_tracing::`、`facade_util_tracing::` 及 `util::tracing::` 的 Rust 源码检索，只确认了 `pkg/lib.rs` 的门面重导出，没有确认到仓库生产 Rust 文件直接调用本文件 API。若其他模块通过本地 `tracing` 模块调用同名 `StartRegion`，不能据此归到本文件；例如 DXF storage 当前使用的是 `pkg/dxf/framework/storage/lib.rs` 内的占位 `tracing` 模块。
- 下游标准库依赖：集合使用 `HashMap`/`Vec`，共享所有权与同步使用 `Arc`、`Mutex`、`RwLock`、`OnceLock` 和原子类型，事件时间使用 `SystemTime`。
- 下游外部依赖：`start_runtime_region` 使用 `tracing::span!` 和 `EnteredSpan`；回调 span 是本文件自有的兼容实现，不是 `tracing` crate 的 `Span`。
- RustCodeGraph 将 `util.rs` 标为被 32 个文件使用，并能定位 `NewRecordedTrace`、`StartRegion` 和 `TraceCategory` 的 Rust/Go 双版本；但精确 `callers/callees` 子命令本次未输出边，且同名符号会混入其他模块，因此不把这 32 个文件解释为已确认的本文件直接调用者。

## 错误处理与边界

- 这些 API 不返回 `Result`。同步锁均以 `unwrap()` 获取；若 recorder、baggage、logs 或测试 sink 的锁被 poison，调用线程会 panic，而不是吞掉错误或返回降级值。
- recorder 回调在 `finish` 调用栈内同步执行；回调 panic 会向调用者传播。由于 `finished` 在回调前已经设为 true，随后再次 `finish` 不会重试该回调。
- noop span 丢弃 `log_kv`，`finish` 无动作，ID 与父 ID 都为 0；它为“没有 tracer”提供非空句柄，但不能被误当作已录制 span。
- `Span` 没有 `Drop` 自动完成逻辑，`Region` 也没有 `Drop` 自动调用 `end`；调用者必须显式调用 `finish`/`end`（或 Go 风格 `End`），否则不会生成完成快照、end 事件，runtime entered span 只会在值最终 drop 时退出。
- `StartRegion` 的 begin/end 事件只受 `General` 类别和 sink 是否存在控制；没有父回调 span并不妨碍事件记录。反之，即使有父 span，只要类别未启用或没有 sink，就不会发事件。
- `ContextWithTraceInfo(ctx, None)` 返回原实例；`ParseTraceCategory` 对未知文本返回零值。调用者若需区分“未知”与显式空类别，必须在调用前自行保留输入。
- `IsEnabled` 的 classic 判断完全取决于 `TIDB_KERNEL_TYPE` 环境变量；这与 Go 通过 `kerneltype.IsClassic()` 查询内核类型的实现机制不同，调用环境应确保变量语义一致。
- `Event.Fields` 是拥有所有权的 `Vec<EventField>`；本文件创建 Region 事件时总是空字段。若未来把事件发给多个接收者，应继续通过克隆/新分配维持 Go 注释要求的发布后不可变语义。

## 并发与资源生命周期

- `Span`、`Context`、sink 和 recorder 都使用 `Arc`，可跨线程共享；`Sink` 和 recorder trait object 显式要求 `Send + Sync`。
- 多个 `Span` 克隆并发 `finish` 时，`AtomicBool::swap(AcqRel)` 保证只有一个调用生成 `RawSpan`。baggage/log 的互斥锁保证快照和写入之间没有数据竞争，但完成与并发写入的先后仍由抢锁顺序决定。
- 全局 recorder 的读写由 `RwLock` 串行化。创建普通根 span时会克隆当前 recorder 后释放读锁，不在用户回调执行期间持有全局锁。
- 类别位图操作是无锁原子操作；`Acquire`/`Release`/`AcqRel` 使开关发布和读取有明确同步语义。复合类别传给 `IsEnabled` 时采用“任一位启用即为真”，不是要求所有位都启用。
- `EnteredSpan` 从创建线程进入 tracing subscriber 的当前 span，并在 `Region::end` 的 `take()`/drop 时退出。`Region::end` 通过 `Option::take` 使 span 完成与 runtime 退出幂等；事件字段未被 `take`，重复调用会再次发出 end 事件，因此公共契约应把 `end`/`End` 视为只调用一次。
- `NewRecordedTrace` 改写进程全局 recorder，彼此并发或测试并行时存在“最后一次写入决定后续根 span”的全局语义；已创建的 span 不受影响。测试若涉及这一状态，应与类别测试一样避免并行污染。

## 与 Go 版本的对应关系

- `CallbackRecorder`、`NewRecordedTrace`、noop、`SpanFromContext`、`ChildSpanFromContxt` 对齐 Go 的 opentracing/basictracer 行为；Rust 用自有 `SpanInner`、数字 ID 和回调快照替代第三方 tracer 对象，但保留根 ID、父子链、baggage 和缺失 tracer 时 noop 的可观察契约。
- Go `context.Context` 能携带任意键值、取消和 deadline；Rust `Context` 只包含本模块声明的四类追踪字段，不提供通用值、取消或 deadline 传播。文档和调用方不应把它描述成完整的 Go context 移植。
- Go `ExtractTraceID` 委托 client-go 的 `TraceIDFromContext`；Rust 直接读取 `ContextData.trace_id`，必须先通过 `with_trace_id` 写入。`TraceInfo.TraceID` 不会自动成为事件的 `TraceID`。
- Go 类别开关使用 `atomic.Uint64` 的 CAS/Store/Load；Rust 以等价原子位操作实现。14 个名称、未知解析为 0、组合值显示为 `unknown(n)` 与 Go 保持一致。
- Go `runtime/trace.StartRegion` 对应 Rust `tracing` crate 的 TRACE 级 entered span。Go `Region.End` 顺序为完成 opentracing span、结束 runtime region、发送 end 事件；Rust `Region::end` 保留同一顺序。
- `StartRegionWithNewRootSpan` 在两个版本中都把新根 span直接放进返回 `Region` 和派生 context，不额外建子 span，也不走 `StartRegion` 的 flight-recorder 事件路径；`util_test.rs::test_start_region_with_new_root_span_reuses_root` 专门锁定此差异。
- Go `Event.Fields` 是 `[]zap.Field`，Rust 是字符串化的 `Vec<EventField>`；Rust 当前 Region 路径只创建空字段，因此未覆盖 zap 的类型化值语义。
- Rust 的 `TraceInfo` 当前只派生 `Clone/Debug/Eq/PartialEq`，而 Go 字段带 JSON tag；本文件本身不实现 serde。若未来把它直接接入持久化模型，必须另行设计并验证线形兼容性，不能仅凭同名 Go 类型推断已支持序列化。

## 扩展指南

- 新增追踪类别时，应在哨兵位之前添加单 bit 常量，同时更新 `traceCategorySentinel`、`TraceCategory::KNOWN`、`as_str`、Go `util.go` 的常量与 `getCategoryName`，并扩展 `migration_aster_unit_test.rs::categories_match_go_names_and_atomic_mask_operations`。不得让 `AllCategories` 漏掉新位。
- 修改 span 录制字段时，应同步更新 `RawSpan`、`SpanInner`、`Span::finish` 和父子继承规则，并在独立的 `util_test.rs` 或 `migration_aster_unit_test.rs` 增加回归；不要把测试嵌回 `util.rs`。
- 扩展 `Context` 字段时，在 `ContextData` 中增加可克隆状态并提供派生/读取 API，确保现有 `with_span`、`with_trace_id`、`WithFlightRecorder`、`ContextWithTraceInfo` 仍保留其他字段。需要“无变化”快速路径时，应像 `ContextWithTraceInfo(None)` 一样返回原实例。
- 修改 Region 生命周期时，应保持 runtime span、回调 span、事件三条资源链的明确结束顺序，并覆盖无父 span、noop 父 span、无 sink、`General` 关闭以及 begin/end 成对场景。若要使重复 `end` 完全幂等，还需一并消费或标记 `event`，并补回归测试。
- 若要增加事件字段，需明确 Rust 字符串字段与 Go `zap.Field` 的类型差异；对发布给 sink 的 `Event.Fields` 继续采用拥有所有权的值或深克隆，避免共享可变底层存储。
- 变更全局 recorder 或类别状态时要评估测试并行污染和生产热路径开销；锁内不得执行用户回调，类别检查应保持原子快速路径。
- 对齐 Go 行为时优先更新同路径 `util_test.rs`，并参照 `util_test.go`；类别和 Region 事件的迁移回归位于 `migration_aster_unit_test.rs`。源文件与测试必须继续分文件存放。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点、1,848,419 条边，其中 Rust 文件 7,032 个。
- RustCodeGraph `files --filter pkg/util/tracing`：确认 crate 内 `lib.rs`、`util.rs`、`util_test.rs`、`migration_aster_unit_test.rs` 以及 Go 对照文件集合。
- RustCodeGraph `node --file pkg/util/tracing/util.rs`：逐行核对 619 行实现；`explore "pkg/util/tracing/util.rs symbols callers callees tracing utility"` 及 `query NewRecordedTrace`、`query StartRegion`、`query TraceCategory` 用于核对主要符号与 Rust/Go 双版本。精确 `callers/callees` 查询本次没有产生输出，未将其当作调用边证据。
- 已读生产/装配/配置：`pkg/util/tracing/util.rs`、`pkg/util/tracing/lib.rs`、`pkg/util/tracing/Cargo.toml`、`pkg/lib.rs`。另核对 `pkg/dxf/framework/storage/lib.rs`，确认 DXF 同名 `StartRegion` 来自其本地占位模块，未误记为本文件调用边。
- 已读 Go 对照：`pkg/util/tracing/util.go`；已读独立测试：`pkg/util/tracing/util_test.rs`、`pkg/util/tracing/migration_aster_unit_test.rs`、`pkg/util/tracing/util_test.go`。
- 测试事实：Rust 用例验证缺失 span 得 noop、父子 ID 链、完成回调、`TraceInfo` 挂载、新根 Region 不创建子 span、noop Region 形态、类别原子开关，以及 `General` begin/end 事件携带同一 trace ID。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证目标文档存在且恰有 11 个固定二级标题，并人工复核无运行时行为修改。
