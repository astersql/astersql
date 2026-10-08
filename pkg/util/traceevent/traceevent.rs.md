# `pkg/util/traceevent/traceevent.rs`

## 文件定位

本文件是 `astersql-util-traceevent` crate 的结构化追踪事件核心，定义事件类别、事件与字段模型、进程级模式和 sink、内存环形缓冲，以及面向 Chrome Trace Event/Perfetto 的渲染结构。crate 入口 `pkg/util/traceevent/lib.rs` 公开 `traceevent` 模块并重导出本文件的 API；`pkg/util/traceevent/Cargo.toml` 表明该 crate 只直接依赖 `log`、`rand`、`serde` 和 `serde_json`。

它位于两条链路的交汇处：上游生产者通过 `trace_event` 写事件，例如 `pkg/session/runtime/dispatch.rs` 在每条可执行语句开始时调用 `generate_trace_id`，再发出 `STMT_LIFECYCLE` 类别的 `stmt.start`；`pkg/util/traceevent/adapter.rs::handle_client_go_trace_event` 则把 client-go 类别映射为内部类别后调用同一入口。下游由进程级 `RingBufferSink`、`Context` 携带的会话 `Trace`（定义于 `flightrecorder.rs`）和全局 `Sink` 共同承接事件。

## 核心职责

- 用 `TraceCategory(u64)` 和 14 个单比特常量表达事务、语句、KV、DDL、TiKV 详情等类别；`ALL_CATEGORIES` 是低 14 位的并集，`parse_trace_category` 完成配置字符串到位图的映射。
- 用 `Field`、`Phase`、`Event` 和 `Context` 建模一条 Instant 事件及其 trace ID、JSON 字段和可选会话 sink。
- 用 `normalize_mode`、`set_mode`、`current_mode` 管理 `off`、`base`、`full` 三种进程级模式；默认原子状态为 base（recorder 开、logging 关）。
- 由 `trace_event` 先做类别过滤，再一次构造事件并按需扇出到进程环形缓冲、会话 `Trace` 和当前全局 sink。
- 提供 `MultiSink` 与线程安全的 `RingBufferSink`，支持多目标扇出、定容覆盖、清空和按旧到新快照。
- 用 `dump_flight_recorder_to_logger` 做带 10 秒冷却的进程缓冲转储，用 `convert_events_for_rendering` 生成 `RenderEvent` 供 Trace Event 可视化。
- 通过文件末尾的 PascalCase `pub use` 别名保持与 Go 公共 API 的命名兼容。

## 主要符号

- `TraceCategory(pub u64)`：可复制的类别位图。`contains` 采用“任一重叠位即为真”，不是集合完整包含；`name`/`Display` 仅为单个已知常量给出稳定名称，组合位或未知值显示为 `unknown(N)`。位运算 trait 允许类别组合。
- `TXN_LIFECYCLE` 至 `REGION_CACHE`、`ALL_CATEGORIES`：当前定义的 14 个类别位。`TxnLifecycle`、`Txn2PC` 等仅是 Go 风格别名，不产生不同值。
- `parse_trace_category(&str) -> TraceCategory`：精确、区分大小写地解析类别名；未知字符串返回零位图，不返回错误。
- `Field`：键加 `serde_json::Value`；`string`、`u32`、`u64`、`i64`、`boolean` 是类型安全构造器。
- `Phase::Instant`：当前唯一相位，序列化为 `"i"`。
- `Event`：包含 `category`、`name`、`phase`、微秒 Unix 时间戳、原始 `trace_id` 字节和字段列表。
- `Context`：不可变构建风格的上下文；`with_trace_id` 和 `with_sink` 消费并返回自身，`sink()` 克隆其中的 `Arc<Trace>`。
- `Sink: Send + Sync`：统一接收接口 `record(&Context, Event)`；具体实现有 `LogSink`、`MultiSink`、`RingBufferSink`，另有 `flightrecorder.rs::Trace`。
- `set_mode` / `current_mode`：以两个 `AtomicBool` 更新或推导模式。非法输入在更新前由 `normalize_mode` 拒绝。
- `set_sink` / `current_sink`：通过 `OnceLock<RwLock<Arc<dyn Sink>>>` 替换或克隆进程级 sink；传 `None` 恢复 `LogSink`。
- `flight_recorder`：惰性创建容量为 1024 的进程级 `RingBufferSink` 单例。
- `trace_event`：核心写入入口。
- `generate_trace_id`：按 `[start_ts: 8 字节大端][stmt_count: 8 字节大端][random: 4 字节大端]` 生成 20 字节 ID；优先复用上下文 `Trace::random()`，其值为零时再随机生成。
- `dump_flight_recorder_to_logger`：快照非空且不在冷却期时逐条调用 `log_event`，返回实际转储数量；`reason` 当前未用于日志内容。
- `RenderEvent` / `convert_events_for_rendering`：生成 `name/ph/ts/pid/tid/id/cat/args` 结构。`tid` 取所有事件中第一个合法且非零的 trace ID 后缀，并应用于全部结果。

## 执行流程

1. 配置层通过 `flightrecorder.rs` 建立全局 flight recorder 并设置启用类别；模式入口把用户值经 `normalize_mode` 归一化，再原子更新 recorder/logging 开关。
2. 调用者准备 `Context`。标准会话路径在 `pkg/session/runtime/dispatch.rs` 读取事务 start TS、递增语句序号，调用 `generate_trace_id`，把 ID 同时写入 session 的 previous-trace 状态和新的 `Context`。
3. `trace_event` 首先调用 `is_enabled`。该函数读取 `flightrecorder.rs::get_flight_recorder()` 的启用位图；记录器不存在或类别无重叠时立即返回，连事件分配和时间读取都不发生。
4. 类别启用时构造 `Event`：名称复制为 `String`，相位固定为 `Instant`，时间来自 `SystemTime` 到 Unix epoch 的微秒数，trace ID 从上下文复制，字段所有权直接移入事件。
5. `RECORDER_ENABLED` 为真时，事件先克隆给进程级 `flight_recorder()`；若上下文含 `Trace`，再克隆给会话 sink。会话 `Trace` 后续由 `flightrecorder.rs::Trace::discard_or_flush` 按触发器位图决定收集或丢弃。
6. 原始事件最后交给 `current_sink()`。默认 `LogSink` 只在 `LOGGING_ENABLED` 为真时输出；自定义 sink 的行为由其自身决定，因此模式开关并不会自动屏蔽任意自定义实现。
7. 崩溃恢复路径 `pkg/util/util.rs::GetRecoverError` 调用 `DumpFlightRecorderToLogger`。函数取得一致快照，检查全局冷却时间，随后逐条同步提交给 `log_event`。
8. 导出可视化时，`convert_events_for_rendering` 先扫描首个有效非零随机后缀作为公共 `tid`，再逐条复制名称、时间、类别与字段；字段非空时以 JSON 对象构建 `args`，其中还会附加十六进制 trace ID。

## 数据与状态

进程级共享状态包括：`RECORDER_ENABLED`、`LOGGING_ENABLED` 两个顺序一致性原子布尔值，`LAST_DUMP_TIME` 原子秒时间戳，`EVENT_SINK` 的惰性 `RwLock<Arc<dyn Sink>>`，以及 `FLIGHT_RECORDER` 的惰性 `Arc<RingBufferSink>`。这些状态会跨请求和测试保留；独立 Rust 测试通过 `lib.rs::test_support::test_guard` 串行化并显式重置相关状态。

`RingBufferSink` 将 `capacity` 固定为至少 1。未写满时 `buf` 按写入顺序增长；写满后 `next` 指向下一覆盖位置。`snapshot` 在未满时直接克隆，在已满时从 `next` 到尾、再从头到 `next` 拼接，因此结果始终从最旧到最新。`discard_or_flush` 清空数组并把 `next` 归零，但保留已分配容量。

`Context` 的 trace ID 和 sink 均按拥有关系保存；读取 trace ID 时复制字节，读取 sink 时克隆 `Arc`。`Event` 在多路扇出和快照时整体克隆，字段中的 JSON 值也随之克隆，因此后续调用者不能通过原输入修改已记录事件。

时间源异常时，`now_micros`/`now_seconds` 对早于 epoch 的 `SystemTime` 使用默认零时长，而不是传播错误。微秒值最终强制转换为 `i64`。

## 依赖与调用关系

- crate 内下游：`flightrecorder.rs::get_flight_recorder` 提供启用类别；`flightrecorder.rs::Trace` 作为 `Context` 的会话 sink，并向 `generate_trace_id` 提供随机后缀。该依赖形成核心事件层与保留策略层之间的双向模块协作，但 Rust 类型引用通过 crate 模块边界解析。
- crate 内上游：`adapter.rs::handle_client_go_trace_event` 调用 `is_enabled` 和 `trace_event`；`handle_trace_control_extractor` 调用 `get_enabled_categories`。
- 应用上游：`pkg/session/runtime/dispatch.rs` 调用 `generate_trace_id` 和 `trace_event` 建立 `stmt.start` 事件；`pkg/sessionctx/variable/sysvar_builtins.rs` 通过 flight recorder 配置接线 SQL 全局变量，并在 classic kernel 拒绝该变量。
- 故障诊断上游：`pkg/util/util.rs::GetRecoverError` 在转换 panic 载荷前调用 `DumpFlightRecorderToLogger`。
- 外部库：`rand::random` 仅负责 trace ID 后缀；`serde`/`serde_json` 负责字段及渲染结构；`log` 负责默认日志输出；并发原语均来自标准库。
- RustCodeGraph 对目标文件给出了完整符号源码并显示其被 crate/仓库广泛引用，但对所查 `trace_event`、`generate_trace_id`、`convert_events_for_rendering`、`dump_flight_recorder_to_logger` 的精确 `callers/callees` 命令未返回静态边。因此上述调用关系以已索引调用点源码为准，不依据笼统的 “used by” 列表推断。

## 错误处理与边界

- `normalize_mode` 是本文件唯一显式返回错误的配置入口；它接受大小写不敏感且可带首尾空白的 `off/0/false/base/full`，其他值返回含合法模式列表的 `String` 错误。`set_mode` 只有归一化成功后才修改状态。
- `current_mode` 把理论上的 `(recorder=false, logging=true)` 非规范组合也报告为 `full`；当前公开 setter 不会制造该组合。
- 类别未知时，字符串解析返回零，显示则返回 `unknown(N)`。组合类别也落到 `unknown(N)`；若需要为组合输出多个名字，必须新增明确语义，不能依赖现有 `name`。
- 所有 `RwLock`/`Mutex` 获取都使用 `expect`，锁中毒会 panic；sink 实现的 panic 也会沿同步调用栈传播，本文件没有隔离或降级机制。
- `RingBufferSink::new(0)` 不报错，而是修正为容量 1。
- `dump_flight_recorder_to_logger` 对空缓冲或 10 秒冷却期返回 0；快照不会在转储后清空，所以冷却期结束后同一批事件仍可再次输出。冷却检查与更新时间是分离的原子 load/store，并发调用可能同时通过检查。
- `extract_rand_from_trace_id` 只接受恰好 20 字节；其他长度返回 0。`convert_events_for_rendering` 不报告格式错误或同批 trace ID 后缀不一致，只选第一个非零值。
- 渲染时只有 `fields` 非空才创建 `args`；因此只有 trace ID、没有字段的事件不会在 `args` 中携带 trace ID。重复字段键写入 `serde_json::Map` 时后值覆盖前值，日志转换同样如此。
- `trace_event` 的注释将日志输出归于 full 模式，但它总会调用全局 sink；只有默认 `LogSink` 自行检查 `LOGGING_ENABLED`。替换为自定义 sink 后，base/off 的实际行为取决于类别过滤和 sink 实现，扩展者不能假设全局 sink 一定只在 full 模式收到事件。

## 并发与资源生命周期

所有进程级对象通过 `OnceLock` 初始化一次且不会销毁。模式切换使用 `SeqCst` 原子操作，单个标志读写有全序，但两个标志不是一个原子事务；并发读者可能短暂观察到切换中的组合，`current_mode` 会将非规范组合折算为 `full`。sink 替换通过写锁完成，记录路径仅在读锁内克隆 `Arc`，实际 `record` 调用不持有全局锁，避免下游回调阻塞 sink 配置。

`RingBufferSink` 每次写入、清空和快照都持有同一个互斥锁；快照会在锁内克隆全部事件，容量 1024 限制了默认最坏复制规模。`MultiSink` 顺序、同步调用每个下游，并为除最后所有权优化之外的通用实现逐个克隆事件；慢 sink 会直接拉长调用者延迟。

会话 `Trace` 由 `Arc` 放入 `Context`，其事件缓冲和触发位由 `flightrecorder.rs` 内部 `RwLock` 管理。`generate_trace_id` 读取其随机值；`discard_or_flush` 在会话边界决定收集并刷新随机值。调用约束与 Go 注释一致：每次语句执行生成一次 ID，而不是每次重试生成。

冷却期使用进程级 `LAST_DUMP_TIME`，生命周期覆盖整个进程且本文件没有公开重置 API。结构化日志和 recorder dump 都是同步执行；本文件不创建线程、异步任务或通道，也不负责持久化。

## 与 Go 版本的对应关系

总体结构与 `pkg/util/traceevent/traceevent.go` 对齐：三种模式、类别过滤、事件模型、全局 sink、进程环形缓冲、20 字节 trace ID、10 秒 dump 冷却、MultiSink，以及 Trace Event 渲染均有一一对应实现。`pkg/util/traceevent/traceevent_test.rs` 也复现了 Go 测试对模式、类别、trace ID、环形覆盖顺序、字段保留和冷却期的断言。

当前可见差异必须在后续对齐时保留关注：

- Go `IsEnabled` 在 classic kernel 且非测试时直接禁用；Rust `is_enabled` 本身没有 kernel 判断，限制目前由 `pkg/sessionctx/variable/sysvar_builtins.rs` 的系统变量验证承担，并非所有调用入口都受同一保护。
- Go 包 `init` 会设置默认 sink/模式并调用 `RegisterWithClientGo`；Rust 依靠静态初值和 `OnceLock` 达到 base 默认值，但本文件不会主动调用 `adapter.rs::register_with_client_go`。该适配函数当前也只记录注册状态，注释说明真实 client-go 接线由包级集成任务负责。
- Go 事件字段是 `zap.Field` 并预留三项追加容量；Rust 使用拥有的 JSON 值，没有对应容量协议。Rust 重复键在转为对象时会覆盖，Go logger 可保留重复字段语义。
- Go dump 会输出含 `reason`、事件数和冷却摘要的专用日志，并返回 `()`；Rust 忽略 `reason`、冷却时静默返回，并返回输出条数，逐条复用普通 `[trace-event]` 格式。
- Go 渲染会记录错误长度、同批随机后缀不一致以及全零 tid；Rust 不记录这些诊断。两端都从 trace ID 的第 16 至 20 字节按本机字节序解释 `tid`，而 ID 生成使用大端，因此该数值是平台相关的字节解释，不能当作原始大端后缀数值。
- Go 的 `Event`、`TraceCategory`、`Sink` 是 `pkg/util/tracing` 的别名，并通过 client-go context API 存取 ID；Rust 在本文件内拥有独立类型和轻量 `Context`。

## 扩展指南

- 新增类别时，应同时分配未占用单比特，更新 `ALL_CATEGORIES`、`TraceCategory::name`、`parse_trace_category`、Go 对照常量/字符串，以及 `traceevent_test.rs::test_category_names`；若类别来自 client-go，还需同步 `adapter.rs::ClientCategory`、`map_category` 和控制位映射。注意位图上限和持久化配置兼容性。
- 新增事件相位或渲染字段时，修改 `Phase`、`Event`/`RenderEvent` 与 `convert_events_for_rendering`，并增加独立测试文件 `pkg/util/traceevent/traceevent_test.rs` 中的序列化、空字段、非法 trace ID 和混合 trace ID 用例；不要把测试内嵌进生产源文件。
- 调整模式语义时，以 `set_mode`、`current_mode` 和 `trace_event` 为主要接入点，同时明确自定义 sink 是否受 logging 开关控制。若要求无瞬时中间态，两个布尔原子应改成单个原子枚举，而不是继续增加跨原子不变量。
- 改动 recorder 容量或并发策略时，保持“覆盖最旧、快照从旧到新”的不变量，并扩展 `test_ring_buffer_snapshot_order`。提高默认容量会线性增加驻留内存和 dump/快照复制成本。
- 完善 dump 行为时，应决定是否对齐 Go 的 reason/摘要日志、是否成功后清空、以及如何用 compare-exchange 防止并发重复 dump，并为冷却边界写确定性测试，避免依赖 11 秒真实睡眠。
- 接入真实 client-go 回调属于 `adapter.rs` 的边界，不应在本文件复制外部客户端逻辑；classic kernel 的统一禁用策略也应在公共入口或上层配置边界做一致化设计。
- 修改 trace ID 格式时必须同步 `generate_trace_id`、`extract_rand_from_trace_id`、会话 previous-trace 使用方、Go 实现及任何外部消费者；这是兼容性风险最高的接口之一。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标 `pkg/util/traceevent/traceevent.rs` 已索引，按行读取覆盖 1–621 行。
- RustCodeGraph 符号查询确认目标文件中的 `trace_event`（345 行）、`generate_trace_id`（378 行）、`dump_flight_recorder_to_logger`（493 行）、`convert_events_for_rendering`（543 行）；精确 callers/callees 查询未返回静态边，该限制已在“依赖与调用关系”中说明。
- 已读取直接 Rust 证据：`pkg/util/traceevent/lib.rs`、`adapter.rs`、`flightrecorder.rs`、`traceevent_test.rs`、`test/integration_test.rs`，以及实际调用点 `pkg/session/runtime/dispatch.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/util/util.rs`。
- 已读取边界声明：`pkg/util/traceevent/Cargo.toml` 的 crate 名、入口、四项直接依赖和 `go-package = "pkg/util/traceevent"` 移植元数据。
- 已读取 Go 对照：`pkg/util/traceevent/traceevent.go` 与 `traceevent_test.go`，用于核对模式、事件扇出、ID 格式、环形缓冲、dump、渲染及测试意图。
- Rust 独立测试证据：`test_suite` 覆盖类别过滤、事件/字段、trace ID、base/full 和冷却；`test_trace_event_modes` 覆盖合法/非法模式；`test_ring_buffer_snapshot_order` 覆盖覆盖顺序；`rendering_rejects_oversized_trace_id` 覆盖严格 20 字节边界；集成测试覆盖会话 sink、采样/触发器以及 SQL 变量产生 `stmt.start`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文仅描述有上述源码或测试支持的当前行为。
