# `br/pkg/trace/tracing.rs`

## 文件定位

`tracing.rs` 是独立 crate `astersql-br-pkg-trace` 的实际实现文件，由同目录 [`lib.rs`](./lib.rs) 以 `#[path = "tracing.rs"] pub mod tracing` 装配并整体再导出。该 crate 的 [`Cargo.toml`](./Cargo.toml) 将它标记为对齐 Go 包 `br/pkg/trace` 的 library；manifest 没有声明第三方依赖，当前实现只使用 Rust 标准库。

它位于 BR 命令执行链的可选追踪边界。直接 Rust 上游 [`br/cmd/br/cmd.rs`](../../cmd/br/cmd.rs) 的 `with_tracing` 在开关启用时调用 `TracerStartSpan` 和 `TracerFinishSpan`。本文件不是通用或生产级分布式 tracing 后端：文件注释明确说明 `MemoryStore`、`Tracer`、`Span` 只是复刻 Go appdash/opentracing 中 BR 用到的最小子集，并把已结束的 span 格式化为本地文本文件。

## 核心职责

1. `TracerStartSpan` 创建共享的 `MemoryStore`、绑定它的 `Tracer` 和名为 `trace` 的根 `Span`，再把根 span 放入轻量 `Context`。
2. `Span::Finish` 以幂等方式把已结束 span 收集到内存；`MemoryStore::build_traces` 根据 `parent_id` 把扁平记录重建为一棵或多棵 `Trace` 树。
3. `TracerFinishSpan` 查询当前已收集的 trace，结束上下文中的根 span，并在查询结果非空时把第一棵树写到 trace 文件。
4. `dfsTree` 对子节点按开始时间排序，生成树形前缀、当地墙钟时间和 Go 风格耗时；`Tabby` 负责三列对齐输出。
5. `timestampTraceFileName`、时间戳/时长格式化函数和测试文件名 hook 保持输出形状与 Go `tracing.go` 一致。

## 主要符号

- `Context { span: Option<Arc<Span>> }`：本地追踪上下文。`Context::Background` 创建空上下文；`ContextWithSpan` 和 `SpanFromContext` 分别写入、读取当前 span。
- `TracesOpts`、`Queryer::Traces`：对齐 `appdash.TracesOpts` 与 `appdash.Queryer` 的最小查询接口。选项当前为空，`MemoryStore` 实现会忽略它。
- `SpanRec`、`Trace { Span, Sub }`、`TimespanEvent`：可输出的树节点及其起止时间视图。`Trace::TimespanEvent` 当前总是返回 `Ok`，但调用方仍保留失败回退路径以对齐 Go 接口形状。
- `CollectedSpan`：未公开的扁平存储记录，保存 `id`、可选 `parent_id`、名称和起止时刻。
- `MemoryStore`：用 `Mutex<Vec<CollectedSpan>>` 保存已结束 span；`NewMemoryStore` 返回 `Arc<Self>`，`collect` 追加记录，`build_traces` 重建森林。
- `Tracer`：持有共享 store 和 `AtomicU64 next_id`。`StartSpan` 创建根节点，`StartSpanChildOf` 读取父 span id，二者都进入 `start_span_inner`。
- `Span`：持有 tracer、id、父 id、名称、开始时间和 `AtomicBool finished`；`Tracer` 返回共享 tracer，`Finish` 仅在第一次调用时收集记录。
- `Tabby`：缓存三列字符串并向 `Box<dyn Write + Send>` 输出。`Print` 按 Unicode 字符数而非 UTF-8 字节数计算列宽，采用两空格 padding。
- `TracerStartSpan`、`TracerFinishSpan`：BR 对外生命周期入口。
- `dfsTree`：递归渲染一棵 `Trace`，同时原地排序 `Trace::Sub`。
- `timestampTraceFileName`、`format_clock_micros`、`format_go_duration`：生成默认路径、墙钟列和耗时列。`format_go_trace_stamp_with_offset_for_test`、`format_clock_micros_with_offset_for_test` 仅在 `cfg(test)` 下暴露。
- `set_get_trace_file_name_for_test`：设置当前测试线程的文件名函数覆盖；其参数类型 `TraceFileNameFn` 本身不公开，因此实际用途限于 crate 内测试/可推断类型的调用场景。

## 执行流程

启用追踪时，调用方先以 `Context::Background()` 调用 `TracerStartSpan`。函数创建空 store，构造从 1 开始分配 id 的 tracer，再建立尚未进入 store 的根 span `trace`，最后返回携带该 span 的 context 与 store。

业务代码若从 context 取得 span，可通过 `span.Tracer().StartSpanChildOf(name, &span)` 创建子节点，并用 `ContextWithSpan` 把子节点继续向下传递。创建动作只记录 `SystemTime::now()`；只有 `Finish` 才记录结束时间并调用 `MemoryStore::collect`。重复 `Finish` 被原子状态拒绝，不会产生重复节点。

结束时，`TracerFinishSpan` 先执行 `store.Traces(TracesOpts {})`，随后无条件要求 context 中存在 active span 并结束它。这个顺序很重要：根 span 在查询之后才被收集，所以输出通常从查询时已结束的业务子 span 开始，而不包含根 `trace`；若此前没有任何已结束子 span，查询结果为空，结束根 span 后直接返回且不创建文件。若已有数据，则仅取 `traces[0]`，创建文件，递归执行 `dfsTree`，最后由 `Tabby::Print` 写入并刷新。

`MemoryStore::build_traces` 在持锁快照上先建立 id 集合，再按有效 `parent_id` 分组；父 id 缺失（包括父 span 尚未结束）的节点会提升为森林根。`build_tree` 从各根递归复制名称和时间，构造输出树。`dfsTree` 为每个节点生成 `├─`/`└─` 前缀，提取起止时间，把负时长回退为零，然后按子节点开始时间升序原地排序并深度优先输出。

## 数据与状态

- 文件名覆盖保存在 `TRACE_FILE_NAME_OVERRIDE` 的 `thread_local RefCell<Option<Arc<dyn Fn() -> String + Send + Sync>>>` 中。默认路径是系统临时目录下的 `br.trace.<本地时间及数字时区>`；非 UTF-8 路径通过 `to_string_lossy` 转为字符串。
- `Tracer::next_id` 使用 relaxed 原子递增，只要求在该 tracer 内获得唯一、单调分配的 id，不用它同步其他内存状态。
- `Span::finished` 是一次性提交闸门。首次 `Finish` 记录当前时间并写 store，之后调用无效。
- `MemoryStore::spans` 是唯一共享可变集合；中毒的 mutex 不会使流程失败，而是通过 `PoisonError::into_inner` 继续使用数据。
- `Trace` 是为查询/渲染构造的拥有型副本；`dfsTree` 会改变其中 `Sub` 的顺序，但不会回写 `MemoryStore`。
- `Tabby::rows` 在 `Print` 前保存完整输出，因此内存占用与本次输出的节点数和文本总量线性相关；实现不会自动清空 rows。

## 依赖与调用关系

上游主链由 RustCodeGraph 和源码共同确认：`br/cmd/br/cmd.rs::with_tracing` 调用 `TracerStartSpan`/`TracerFinishSpan`；`br/cmd/br/parity_test.rs::contract_resource_cleanup` 覆盖这条命令层资源清理契约。crate 入口 `br/pkg/trace/lib.rs` 将本文件全部公开导出。

内部调用主链为：

`TracerStartSpan` → `MemoryStore::NewMemoryStore` → `Tracer::NewTracer` → `Tracer::StartSpan` → `ContextWithSpan`；结束链为 `TracerFinishSpan` → `Queryer::Traces`/`MemoryStore::build_traces` → `SpanFromContext` → `Span::Finish` → 文件创建 → `dfsTree` → `Tabby::AddLine`/`Print`。子 span 路径则是 `Span::Tracer` → `Tracer::StartSpanChildOf` → `Span::Finish` → `MemoryStore::collect`。

标准库依赖分别承担共享所有权与并发（`Arc`、`Mutex`、原子类型）、时间（`SystemTime`、`Duration`）、文件/输出（`File`、`Write`）以及树重建（`HashMap`、`HashSet`）。Unix 上 `local_utc_offset_seconds` 通过 `localtime_r` FFI 读取 `tm_gmtoff`；非 Unix 构建固定回退到 UTC。Go 的 Bazel 目标仍依赖 appdash、opentracing、tabby、PingCAP log 和 zap，这些依赖没有进入 Rust crate。

## 错误处理与边界

- `Queryer::Traces` 返回错误时，`TracerFinishSpan` 向 stderr 输出 `fail to get traces` 并返回，不结束 active span；这是当前控制流的直接结果。
- 查询成功但 context 无 span 时，`expect("TracerFinishSpan requires an active span")` 会 panic，对齐 Go 对 nil span 调用 `Finish` 的失败语义；`tracing_test.rs::finish_without_active_span_matches_go_nil_span_panic` 固定了该边界。
- 文件创建失败只记录 stderr 并返回，不 panic；`parity_test.rs::go_rust_public_contract_matches` 用不可写路径覆盖此行为。
- `Tabby::Print` 明确忽略 `write_all` 与 `flush` 的错误，因此调用者无法从 API 得知磁盘写入中途失败或 flush 失败。这是安全扩展时需要优先评估的可观测性缺口。
- `TracerFinishSpan` 只输出森林的第一棵树；其他根节点会被忽略。无已结束 span 时不落盘，根 span 虽随后进入 store，但该函数不会二次查询。
- 缺失父节点会被提升为根，不会报错；时间区间读取失败或结束早于开始时分别使用 Unix epoch/零时长回退。当前 `TimespanEvent` 实现本身不会返回错误。
- Unix `localtime_r` 失败、时区偏移无法转为 `i32` 时回退 UTC；非 Unix 平台也始终按 UTC 格式化，因此与 Go 本地时区表现可能不同。
- `format_go_duration` 接收无符号 `Duration`，不覆盖 Go `time.Duration` 的负值；`SystemTime` 早于 epoch 时只按整秒处理文件名，丢弃亚秒误差，但正常运行路径使用当前时间。

## 并发与资源生命周期

`Arc<MemoryStore>` 同时由 tracer、span 和结束调用方持有，使 store 至少存活到所有相关 span 释放。`Mutex` 串行化并发 `Finish` 的追加与查询/建树；建树期间一直持锁，因此节点很多或递归较深时会暂时阻塞其他 span 完成。`AtomicU64` 允许并发创建 span，`AtomicBool` 的 compare-exchange 保证同一 span 至多收集一次。

测试文件名 hook 特意做成 thread-local：Go 测试串行修改包变量，而 Rust libtest 默认并行，这种隔离避免测试相互覆盖路径。它不提供跨线程继承；若一个测试把写文件动作移到新线程，新线程会看到默认文件名。测试必须调用 `set_get_trace_file_name_for_test(None)` 清理本线程覆盖。

文件由 `File::create` 获得所有权后装入 `Tabby` 的 writer；`TracerFinishSpan` 返回时 `Tabby` 和 `File` 自动 drop，等价于 Go 的 deferred close。`Print` 在 drop 前主动 flush，但错误被忽略。实现没有后台任务、通道或网络连接，也不会跨进程持久化 store。

## 与 Go 版本的对应关系

Go 对照文件是 [`tracing.go`](./tracing.go)。公开流程保持一致：默认临时文件命名、创建根 span、从 context 取 span、先查询后结束根 span、空结果返回、只打印第一棵树、子节点按开始时间排序，以及三列树形文本。`tracing_serial_test.go::TestSpan` 与 Rust `tracing_serial_test.rs::test_span` 都构造 `jobA → jobB`，断言约 200ms/100ms 的两行输出。

Rust 的主要替换是把 appdash/opentracing/tabby 依赖内置为本地类型，并用 stderr 代替 PingCAP 结构化日志。Rust `MemoryStore` 只保存已结束 span，`TracesOpts` 为空，不能视为完整 appdash 实现。Go 的 `getTraceFileName` 是进程级可变函数，Rust 测试覆盖是 thread-local；Go `tabwriter` 的 rune 宽度由 Rust `chars().count()` 模拟。

格式兼容由额外 Rust 回归测试加强：`parity_test.rs` 检查本地数字时区、公开 start/finish 契约、空森林和不可写路径；`tracing_test.rs` 检查小时/分钟零分量、Unicode 树形列宽、原地排序、当地墙钟偏移与无 active span panic。Go `main_test.go` 还用 goleak 审计包测试进程；Rust 本地实现没有 goroutine 对应物。

## 扩展指南

- 增加 span 元数据、tag 或跨进程上下文时，应同时扩展 `CollectedSpan`、`Span`、`Trace`/`SpanRec`、`start_span_inner` 和 `Finish`，并先明确是否仍要维持“最小 BR 子集”定位；不要只改输出层伪造后端能力。
- 改变输出树或排序时，入口是 `MemoryStore::build_traces`、`build_tree` 和 `dfsTree`。必须保留缺失父节点策略、同开始时间排序语义和“只输出第一棵树”行为，除非 Go 版本及兼容契约同步改变。
- 改变文件名、时区或耗时格式时，同步检查 `timestampTraceFileName`、`format_go_trace_stamp_with_offset`、`format_clock_micros_with_offset`、`format_go_duration`，并更新独立的 `parity_test.rs`、`tracing_test.rs` 和 `tracing_serial_test.rs`；测试逻辑不要放回生产文件。
- 若要传播写入错误，应为 `Tabby::Print` 和 `TracerFinishSpan` 设计 `Result`，并评估现有“记录错误后返回”的 Go 兼容性，而不是只在内部开始 panic。
- 若需减少锁持有时间，可在 `build_traces` 中先复制必要记录再解锁，但要验证并发 `Finish` 相对于查询快照的边界。若允许极深 span 树，还应评估 `build_tree`/`dfsTree` 递归导致的栈风险。
- 新增平台支持时应为非 Unix 的本地 UTC offset 提供可靠实现，并加入固定 offset 测试；不要依赖运行测试机器的真实时区。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/trace` 确认本 crate 的实现、Go 对照和三份 Rust 测试均已索引。
- RustCodeGraph `node --file br/pkg/trace/tracing.rs`：核对了本文件 699 行源码及全部类型、函数、条件编译分支；`query TracerStartSpan` 区分了 Go/Rust 两个同名定义。
- RustCodeGraph `callees TracerStartSpan`：确认 Rust 入口调用 `NewMemoryStore`、`NewTracer`、`StartSpan`、`ContextWithSpan`；`callees TracerFinishSpan` 确认查询、结束、文件名、渲染与打印链；`callees dfsTree` 确认时间读取、格式化和行输出链。图的无路径限定 `callers` 查询未返回结果，因此直接上游另由 `br/cmd/br/cmd.rs::with_tracing` 与其 parity 测试源码核验。
- crate/装配证据：`br/pkg/trace/Cargo.toml`、`br/pkg/trace/lib.rs`、`br/pkg/trace/BUILD.bazel`。
- Go 语义证据：`br/pkg/trace/tracing.go`、`br/pkg/trace/tracing_serial_test.go`、`br/pkg/trace/main_test.go`。
- Rust 回归证据：`br/pkg/trace/tracing_test.rs`、`br/pkg/trace/parity_test.rs`、`br/pkg/trace/tracing_serial_test.rs`，以及命令层调用/清理证据 `br/cmd/br/cmd.rs`、`br/cmd/br/parity_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的结构命令验证文档存在且恰好包含 11 个固定二级章节，并人工复核链接、符号名、当前限制和扩展测试位置。
