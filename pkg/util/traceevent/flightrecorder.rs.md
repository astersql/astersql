# [`pkg/util/traceevent/flightrecorder.rs`](./flightrecorder.rs)

## 文件定位

本文件属于 `astersql-util-traceevent` crate。crate 入口 `pkg/util/traceevent/lib.rs` 将它声明为 `flightrecorder` 子模块并公开重导出；`pkg/util/traceevent/Cargo.toml` 表明该 crate 直接依赖 `rand`、`serde`、`serde_json` 和 `log`，其中本文件直接使用前三者中的 `rand` 与 `serde`，事件日志输出则委托给同 crate 的 `traceevent::log_event`。

它实现的是“按条件保留一次会话追踪”的飞行记录器：`traceevent.rs::trace_event` 在类别启用且 recorder 模式开启时，把事件同时写入进程级 `RingBufferSink` 和 `Context` 中可选的会话级 `Trace`；本文件负责保存后者的事件、累计 dump 触发位、按配置决定 flush 或 discard，并把保留的事件送入同步通道或日志。不要把本文件的 `Trace`/`HttpFlightRecorder` 与 `traceevent.rs::flight_recorder()` 返回的进程级环形缓冲混为一谈。

Rust 生产接线目前是部分迁移状态。`pkg/sessionctx/variable/sysvar_builtins.rs::SetTraceEventConfig` 通过全局变量 `tidb_trace_event` 启停日志记录器，`traceevent.rs::{is_enabled,get_enabled_categories}` 和 `adapter.rs::handle_trace_control_extractor` 读取它；仓库内 Rust 生产源码尚未找到 `check_flight_recorder_dump_trigger`、`start_http_flight_recorder` 或 `Trace::discard_or_flush` 的调用，相关完整链路主要由 `pkg/util/traceevent/test/integration_test.rs` 验证。对应 Go 版本则已在 session、executor、store、server 和 DDL 等生产路径接线。

## 核心职责

- 用 `Trace` 实现 `traceevent::Sink`，为单个 `Context` 缓冲 `Event`，并用一个 `u64` 位图记录本次追踪命中的条件。
- 将 `FlightRecorderConfig.dump_trigger` 的递归 `sampling`、`suspicious_event`、`user_command`、`and`、`or` 配置编译成“规范名到 bit 索引”的映射和满足条件的真值表。
- 解析 `enabled_categories` 为 `TraceCategory` 位图，提供 `*` 全选及 `-` 后续减除语义。
- 维护进程级的可替换 `HttpFlightRecorder` 单例；选择有界同步通道导出或逐条写日志，并提供采样计数。
- 暴露 Go 风格别名，降低从 `flightrecorder.go` 移植调用点时的接口差异。

## 主要符号

- `TraceState { events, bits, random }`：`Trace` 的锁内状态。`events` 是当前会话批次，`bits` 最多表达 64 个触发叶子，`random` 用于 `traceevent.rs::generate_trace_id` 的 4 字节随机后缀。
- `Trace`：持有 `RwLock<TraceState>`；`new/new_with_random` 创建会话 sink，`events/bits/random` 返回快照，`mark_bits` 置位，`discard_or_flush` 完成本批次结算；其 `Sink::record` 实现只追加事件。
- `UserCommandConfig`：支持 `sql_regexp`、`sql_digest`、`plan_digest`、`stmt_label`、`by_user`、`table` 六类叶子。
- `SuspiciousEventConfig` 与 `DevDebugConfig`：支持 `slow_query`、`query_fail`、`resolve_lock`、`region_error`、`is_internal`，以及 `execute_internal_trace_missing`、`send_request_trace_id_missing` 两个调试子类型。
- `DumpTriggerConfig`：serde 配置树；`compile` 递归注册叶子并生成 `Vec<u64>` 真值表。其 `kind` 通过 JSON 字段 `type` 反序列化。
- `CompiledDumpTriggerConfig`：保存 `name_mapping: HashMap<String, usize>`、与索引同序的 `config_ref` 以及 `truth_table`；`add_trigger` 保证规范名唯一且叶子数不超过 64。
- `truth_table_for_and`、`truth_table_for_and_one`、`truth_table_for_or`、`check_truth_table`：分别实现组合条件的笛卡尔积按位或、单项按位或、候选拼接和覆盖匹配。
- `FlightRecorderConfig`：包含启用类别和 dump 树；`initialize` 设置默认类别表达式与 `sampling=1`，`compile` 生成运行时映射。
- `HttpFlightRecorder`：运行时对象，保存可选 `SyncSender<Vec<Event>>`、类别位图、`AtomicI64` 采样计数器、原配置和编译结果。
- `start_http_flight_recorder`、`start_log_flight_recorder`、`get_flight_recorder`、`close_flight_recorder`：全局实例生命周期 API；实际安装由内部 `new_flight_recorder` 完成。
- `check_flight_recorder_dump_trigger`：按规范名定位配置，执行调用者提供的判定闭包，成功后给 `Context` 的 `Trace` 置位。
- `MAX_EVENTS`：值为 4096，仅控制 flush/discard 后是否释放过大的 `Vec` 容量，并不是录制时的硬上限。

## 执行流程

1. 配置入口把 JSON 反序列化成 `FlightRecorderConfig`。当前 Rust 生产入口 `SetTraceEventConfig` 对非空值调用 `start_log_flight_recorder`，随后把 traceevent 模式设为 `full`；HTTP 入口在 Rust 集成测试中由 `start_http_flight_recorder` 覆盖。
2. `new_flight_recorder` 先调用 `FlightRecorderConfig::compile`。每个叶子以 `dump_trigger...` 规范名注册一个 bit；`and` 对左右候选做笛卡尔积并把 bit 掩码相或，`or` 直接拼接候选。编译失败时不会替换全局实例。
3. 配置编译成功后，`parse_categories` 生成类别位图，构造 `Arc<HttpFlightRecorder>`，再写入 `GLOBAL_FLIGHT_RECORDER`。新启动会直接替换旧实例。
4. `traceevent.rs::trace_event` 先通过 `is_enabled` 过滤类别；在 recorder 模式下，它既写进程级环形缓冲，也调用 `Context::sink()` 得到本文件的 `Trace` 并追加同一事件的克隆。
5. 业务触发点调用 `check_flight_recorder_dump_trigger(ctx, canonical_name, check)`。函数依次检查全局 recorder、上下文 sink、规范名和配置引用；闭包返回 `true` 时调用 `Trace::mark_bits`。
6. `Trace::discard_or_flush` 读取当前 `bits`，由 `HttpFlightRecorder::should_keep` 检查它是否完全覆盖真值表任一掩码。命中时在读锁内克隆事件，释放锁后交给 `collect`。
7. `collect` 对 HTTP 模式执行非阻塞 `SyncSender::try_send`，对日志模式逐条调用 `traceevent::log_event`。无论是否命中、是否存在 recorder、通道是否接收成功，`discard_or_flush` 最后都会清零 bits 和事件，并刷新 random。
8. sampling 叶子的判定由调用者把该叶子的配置传给 `HttpFlightRecorder::check_sampling`；全局计数达到 `sampling` 后清零并命中。因此它是 recorder 实例级计数，不是每个 `Trace` 各自计数。

## 数据与状态

触发条件有两个阶段。配置阶段把每个不同规范名分配为 `1_u64 << idx`；运行阶段把同一次 `Trace` 中已发生的条件累积到 `TraceState.bits`。`truth_table` 中每个值代表一组必须同时满足的位，表中多个值代表 OR。`check_truth_table(bits, table)` 使用 `bits & required == required`，因此额外命中的条件不会导致失败。

`config_ref[idx]` 与 `name_mapping[name] == idx` 保持同序不变量，使触发检查闭包能取得原始叶子配置，例如 sampling 的间隔或用户命令的匹配值。重复规范名会使编译失败，这也意味着同一配置树不能出现两个名称相同但参数不同的同类叶子。第 65 个叶子会因 `u64` 容量限制失败。

类别位图与触发位图互相独立：前者决定哪些事件进入 sink，后者决定已缓冲事件最终是否保留。`parse_categories` 遇到 `*` 会立即使用全集并停止解析；遇到 `-` 会先设为全集，并让后续普通名称执行减法。未知类别由 `parse_trace_category` 解析为零，不报错。

全局状态为 `OnceLock<RwLock<Option<Arc<HttpFlightRecorder>>>>`。`OnceLock` 只初始化锁本身，锁中的 `Option` 可以被 start/close 反复替换。已由其他线程克隆出的 `Arc` 可在全局 close 或替换后继续存活；新调用只会看到锁内当前值。

## 依赖与调用关系

上游 Rust 生产关系：

- `pkg/sessionctx/variable/sysvar_builtins.rs::SetTraceEventConfig` 调用 `start_log_flight_recorder`/`close_flight_recorder`；系统变量 getter 调用 `get_flight_recorder` 并序列化 `recorder.config`。
- `pkg/util/traceevent/traceevent.rs::{is_enabled,get_enabled_categories}` 读取全局 recorder；`trace_event` 根据类别写入 `Context` 中的 `Trace`；`generate_trace_id` 读取 `Trace::random`。
- `pkg/util/traceevent/adapter.rs::handle_trace_control_extractor` 读取启用类别；当 `should_keep(trace.bits())` 已为真时为 client-go 请求添加 `IMMEDIATE_LOG` 标志。
- 仓库源码搜索未找到 Rust 生产调用 `check_flight_recorder_dump_trigger`、`Trace::discard_or_flush` 或 HTTP 启动入口；这些接口目前由 `pkg/util/traceevent/test/integration_test.rs` 展示预期接线。

下游依赖集中在 `crate::traceevent`：`Event` 是被缓冲/导出的数据，`Context` 携带 trace id 与 `Arc<Trace>`，`Sink` 定义录制接口，`TraceCategory`/`ALL_CATEGORIES`/`parse_trace_category` 提供类别运算，`log_event` 是无通道模式的最终输出。标准库提供 `RwLock`、`Arc`、`OnceLock`、原子计数与有界同步通道；`rand::random` 生成 trace 后缀。

RustCodeGraph 能精确定位本文件和主要符号，但本次 `callers`/`callees` 查询在 60 秒内无结果并被终止；因此上述调用边由已索引文件节点内容和全仓 `rg` 的精确符号引用交叉核对，不采用索引输出中宽泛的 “used by” 文件列表作为调用证据。

## 错误处理与边界

配置编译以 `Result<_, String>` 返回可供 Go 风格接口传播的消息。错误覆盖：未知顶层/用户命令/可疑事件/dev_debug 类型，sampling 非正数，缺失对应子配置，空 `and`/`or`，用户命令匹配值为空，规范名重复，以及超过 64 个触发叶子。serde 负责 JSON 语法和字段类型错误；所有配置结构使用 `#[serde(default)]`，因此字段缺失通常先落为默认值，再由 compile 给出语义错误。

运行时触发检查对 recorder 未启动、`Context` 无 sink、规范名未知或配置引用缺失都静默返回；判定闭包只在这些前提满足后执行。与 Go 版本需要把通用 tracing sink 动态断言成 `Trace` 不同，Rust `Context` 的 sink 类型固定为 `Arc<Trace>`，消除了类型断言失败分支。

HTTP/channel 导出采用 `try_send`，通道满或断开时错误被有意忽略，当前 API 不向调用者报告丢批次。日志导出逐事件调用 `log_event`，没有本地重试。`discard_or_flush` 即使导出失败也会清空当前批次，因此调用方不能把它当作可靠持久化协议。

所有 `RwLock` 获取都用 `expect`；一旦持锁线程 panic 导致锁中毒，后续访问会 panic。`mark_bits(idx)` 没有公开边界检查，传入 `idx >= 64` 会触发移位溢出风险；正常路径的索引只能来自受 `add_trigger` 限制的编译结果。`check_sampling` 假定传入已经过 compile 的正 sampling 配置，若绕过编译直接传入零/负值，会每次命中。

## 并发与资源生命周期

`Trace` 以单个 `RwLock` 保护事件、位图和随机数。录制与置位取得写锁；读取快照取得读锁。flush 的判定和事件克隆在读锁内完成，实际 channel/log 输出在锁外进行，避免慢输出阻塞同一 trace 的记录者；随后再取得写锁清理。由于读锁释放到写锁获取之间允许并发 `record`/`mark_bits`，这些新变化也会被随后的清理覆盖，却不一定包含在刚才克隆的批次中。该行为与 Go 文件相同，调用方应把 `discard_or_flush` 放在不会继续向同一会话批次并发写入的边界。

`events.len() > MAX_EVENTS` 时清理会替换为新 `Vec`，释放超大容量；否则 `clear` 保留容量供下一批复用。注意检查发生在输出之后且按长度而非 capacity 判断。每次结算都会生成新的 random，使后续 trace id 后缀与前一批分离。

全局 recorder 读写通过 `RwLock<Option<Arc<_>>>` 同步；`get_flight_recorder` 克隆 `Arc` 后不再持锁。`counter` 使用 `SeqCst` 原子操作，但“加一达到阈值”和“写零”不是一个原子事务；多个线程越过阈值时可能出现多次命中或计数覆盖，不能把 sampling 理解为严格的并发每 N 次一次。`HttpFlightRecorder::close` 只是清除全局引用，不关闭外部拥有的 `SyncSender` 克隆或等待消费者。

测试中的全局状态由 `lib.rs::test_support::test_guard` 串行化；`adapter_test.rs::test_trace_control_extractor` 另用 100 个读取线程和 10 个置位线程验证共享 `Trace` 不 panic。

## 与 Go 版本的对应关系

`pkg/util/traceevent/flightrecorder.rs` 按结构对应 `pkg/util/traceevent/flightrecorder.go`：Rust 的 `TraceState + Trace` 对应 Go `Trace` 的 mutex/事件/位图/random 字段；`CompiledDumpTriggerConfig`、真值表算法、规范名、64 位限制、默认类别、非阻塞通道发送、sampling 计数及 `DiscardOrFlush` 的克隆后输出再清理流程均保持一致。

主要语言差异如下：

- Go 用 `atomic.Pointer[HTTPFlightRecorder]`；Rust 用 `OnceLock<RwLock<Option<Arc<HttpFlightRecorder>>>>`。Rust start/close 需要锁，但返回的实例由 `Arc` 管理寿命。
- Go 配置持有指针且 `configRef` 保存叶子指针；Rust 拥有 `FlightRecorderConfig` 并在 `config_ref` 中克隆 `DumpTriggerConfig`，避免借用生命周期穿透全局对象。
- Go 的 channel 是 `chan<- []Event`；Rust 是 `SyncSender<Vec<Event>>`，两者发送都采用非阻塞分支并丢弃失败。
- Go `Trace.events = nil` 释放大批次；Rust `Vec::new()` 达到同一目的。小批次分别用切片归零和 `Vec::clear` 复用容量。
- Go `CheckFlightRecorderDumpTrigger` 从通用 tracing context 取 sink 并做类型断言；Rust `Context::sink` 在类型层面只返回 `Arc<Trace>`。
- Go 的 `truthTableForAnd` 对单一左项有专门快捷分支；Rust 统一执行等价的笛卡尔积，语义相同。

移植接线尚未完全对齐。Go 生产调用点包括 `pkg/session/session.go` 的采样、用户命令和查询失败触发，`pkg/executor/adapter.go` 的慢查询/plan digest，`pkg/store/copr/coprocessor.go` 的 Region 错误，`pkg/store/driver/tikv_driver.go` 的 dev_debug，以及 `pkg/server/http_status.go` 的 HTTP recorder。Rust 搜索只确认了全局变量到日志 recorder、类别过滤和 adapter 控制标志的生产接线；文档不把 Go 调用点推断为 Rust 已支持。

## 扩展指南

新增触发叶子时，应同时修改对应配置结构和它的 `compile` 分支，选择稳定且唯一的规范名，并确认 `config_ref` 能携带运行时判定所需字段；随后在实际业务事件处调用 `check_flight_recorder_dump_trigger`。如果同一类条件需要在一棵树中出现多次，当前“规范名必须唯一”的模型不足，需先设计带实例标识的名称或新的索引策略，不能只放宽重复检查。

新增追踪类别应先在 `traceevent.rs` 中分配 bit、更新 `ALL_CATEGORIES` 与 `parse_trace_category`，再评估 `FlightRecorderConfig::initialize` 的默认排除集合，以及 `adapter.rs` 是否需要映射到 client-go 控制标志。改变 `*`/`-` 语法时要明确当前“`*` 提前终止、`-` 之后持续减除”的顺序语义。

若将 HTTP recorder 接入 Rust 生产服务器，应明确通道容量、消费任务、关闭顺序和丢批次指标；当前 `try_send` 没有背压和可观测错误。若需要严格采样，应把 `fetch_add + store(0)` 改成 CAS 循环或单调序号取模，并补并发测试。

修改 flush 生命周期时必须保留“锁内克隆、锁外输出”的数据竞争防护，同时决定读锁到写锁之间的并发新事件应归入当前批还是下一批。若要求无丢失批次，宜在锁内原子地 swap 出完整 `TraceState`，而不是直接延长输出期间的锁持有时间。

测试必须放在独立文件，不内嵌到生产源。配置/真值表变化同步 `pkg/util/traceevent/flightrecorder_test.rs`；并发类别控制同步 `adapter_test.rs`；start、触发、flush、sampling 与通道行为同步 `test/integration_test.rs`。涉及移植语义时还应对照并按需要更新 `flightrecorder_test.go` 和 Go 集成测试，但不能用 Go 已覆盖替代 Rust 独立测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/traceevent` 确认目标源、独立测试、Go 对照和模块入口均已索引。
- RustCodeGraph `node --file pkg/util/traceevent/flightrecorder.rs`：读取 1–535 行，核对全部常量、配置类型、编译算法、全局生命周期、输出和 Go 风格别名。
- RustCodeGraph `query check_flight_recorder_dump_trigger --kind function`：唯一定位到本文件第 494 行。`callers`/`callees` 在 60 秒内没有返回并被终止，调用关系随后用精确源码引用搜索补证。
- RustCodeGraph 节点：`pkg/util/traceevent/traceevent.rs`（`Event`、`Context`、`Sink`、类别解析、事件分流和 trace id）、`adapter.rs`（`IMMEDIATE_LOG` 判定）、`pkg/sessionctx/variable/sysvar_builtins.rs`（`tidb_trace_event` 生产入口）。
- crate/module：`pkg/util/traceevent/Cargo.toml`、`pkg/util/traceevent/lib.rs`；目标目录不存在 `doc.go`，因此没有可额外读取的 Go package contract 文件。
- Rust 测试：`pkg/util/traceevent/flightrecorder_test.rs` 验证合法/非法编译、类别解析和 AND/OR 真值表；`adapter_test.rs` 验证类别控制与并发访问；`test/integration_test.rs::test_flight_recorder` 验证类别过滤、sampling=5 的 10 次中 2 次命中、用户命令、可疑事件、channel 输出与结算。
- Go 对照：`pkg/util/traceevent/flightrecorder.go` 全文及 `flightrecorder_test.go`；全仓精确引用搜索用于确认 Go 生产调用面和 Rust 当前接线边界。
- 本任务是纯文档分析，按任务约束未运行 Cargo；最终以固定 11 章节结构命令和人工事实复核验收。
