# `pkg/util/traceevent/adapter.rs`

源文件：[`adapter.rs`](adapter.rs)

## 文件定位

本文件属于 Cargo 包 `astersql-util-traceevent`（见 [`Cargo.toml`](Cargo.toml)），位于 crate 根 [`lib.rs`](lib.rs) 声明并公开重导出的 `adapter` 模块。它处在 TiKV client-go 风格跟踪接口与 Rust traceevent 内核之间：把客户端类别转换为 `TraceCategory`，把客户端事件送入 `trace_event`，并从全局飞行记录器状态提取客户端控制位。

当前 Rust crate 没有 client-go/client-rust 依赖，`register_with_client_go` 也没有安装真实回调；它只记录“已调用注册”的状态。因此本文件已经实现适配语义与可测试 API，但尚不是等同于 Go 版的外部客户端接线层。

## 核心职责

1. 用 `ClientCategory` 表示 client-go 的四个已知类别，并用 `Other(u32)` 无损承载未来未知类别。
2. 通过 `map_category` 把客户端类别映射到内核类别；未知值统一落入 `UNKNOWN_CLIENT`。
3. 在 `handle_client_go_trace_event` 中先按当前飞行记录器配置过滤事件，再将启用事件交给 `trace_event`；未知类别额外写入 `client_go_category` 字段，避免丢失原始编号。
4. 在 `handle_trace_control_extractor` 中独立计算 TiKV request/write/read 三个类别位，并在上下文存在 `Trace` 且其 bitset 命中飞行记录器真值表时增加 `IMMEDIATE_LOG`。
5. 暴露 snake_case API 及 Go 风格别名，便于 Rust 调用与迁移代码对照。

## 主要符号

- `ClientCategory::{Txn2Pc, TxnLockResolve, KvRequest, RegionCache, Other(u32)}`：客户端类别的 Rust 表示。私有方法 `raw` 把已知变体还原为 `0..=3`，对 `Other` 原样返回；该值只在未知事件诊断字段中使用。
- `TraceControlFlags(u32)`：私有底层值的轻量位标志。`IMMEDIATE_LOG`、`TIKV_REQUEST`、`TIKV_WRITE_DETAILS`、`TIKV_READ_DETAILS` 分别占第 0 至第 3 位；`contains` 检查交集，`with` 返回按位或后的新值，`category_bits` 只把后三个 TiKV 位转换为内核类别位图。
- `REGISTERED: AtomicBool`：进程级注册标记。`register_with_client_go` 以 `Release` 写入，`client_go_registered` 以 `Acquire` 读取。
- `handle_client_go_trace_event`：事件入口，执行映射、启用检查、未知类别补字段和下游记录。
- `handle_client_go_is_category_enabled`：客户端类别启用查询，等价于 `is_enabled(map_category(category))`。
- `handle_trace_control_extractor`：控制位提取入口；类别位不依赖上下文 sink，立即日志位依赖上下文 `Trace` 与全局飞行记录器。
- `map_category`：四个已知客户端类别到 `TXN_2PC`、`TXN_LOCK_RESOLVE`、`KV_REQUEST`、`REGION_CACHE` 的穷举映射，其余到 `UNKNOWN_CLIENT`。
- `handleClientGoIsCategoryEnabled`、`handleClientGoTraceEvent`、`handleTraceControlExtractor`、`mapCategory`、`RegisterWithClientGo`：上述函数的 Go 风格公开重导出别名，不包含额外行为。

## 执行流程

事件路径从 `handle_client_go_trace_event(ctx, category, name, fields)` 开始。函数先调用 `map_category`，随后用 `is_enabled` 查询全局 `HttpFlightRecorder` 的启用类别；未启用时立即返回，不修改字段也不创建事件。若映射结果为 `UNKNOWN_CLIENT`，就在原字段尾部追加 `Field::u32("client_go_category", category.raw())`。最后调用 `trace_event`：该下游会再次检查类别，构造 Instant `Event`，按模式写入进程环形 recorder、上下文 `Trace` 和全局 sink。

启用查询路径只是“映射后查询”，不会记录事件或改变全局状态。

控制位路径先读取 `get_enabled_categories()`，分别测试 `TIKV_REQUEST`、`TIKV_WRITE_DETAILS`、`TIKV_READ_DETAILS` 并组装对应 flags。随后仅为计算 `IMMEDIATE_LOG` 检查 `ctx.sink()` 和 `get_flight_recorder()`：任一不存在就保留已算出的类别 flags 返回；二者都存在时，以 `recorder.should_keep(trace.bits())` 对编译后的 dump 真值表求值，命中后增加立即日志位。因此“没有 sink”不等于“没有类别 flags”。

注册路径目前只把 `REGISTERED` 设为 `true`。它没有幂等保护之外的资源操作，也没有把三个 handler 传给外部客户端；集成测试只验证标记及 handler 行为。

## 数据与状态

`ClientCategory` 和 `TraceControlFlags` 都是可复制的小值类型。flags 的底层 `u32` 不公开，调用者只能使用常量、`contains`、`with` 和 `category_bits`；这阻止外部随意构造未知位，但也意味着新增位需要在本类型实现中显式加入。

文件自身唯一可变全局状态是 `REGISTERED`。类别配置和飞行记录器实例由 `flightrecorder.rs` 的 `GLOBAL_FLIGHT_RECORDER: OnceLock<RwLock<Option<Arc<HttpFlightRecorder>>>>` 管理；`get_flight_recorder` 克隆 `Arc` 后返回。上下文的 sink 是 `Option<Arc<Trace>>`，`Context::sink` 同样克隆 `Arc`。因此 extractor 在检查期间持有稳定对象，不借用全局锁跨越真值表计算。

事件 fields 由调用者转移所有权。已知类别保持原向量不变；未知类别只在末尾追加一个字段，不去重同名键。`category_bits` 不包含 `IMMEDIATE_LOG`，也不包含 adapter 的事务/KV/region 类别，只返回三个 TiKV 详情相关内核位。

## 依赖与调用关系

直接内部依赖如下：

- `crate::traceevent` 提供 `Context`、`Field`、类别常量、`get_enabled_categories`、`is_enabled` 和 `trace_event`。
- `crate::flightrecorder::get_flight_recorder` 提供当前全局 `HttpFlightRecorder`；其 `should_keep` 调用 `check_truth_table(bits, truth_table)`。
- 标准库 `AtomicBool`/`Ordering` 提供注册状态发布与读取。

crate 根 `lib.rs` 公开 `adapter` 并 `pub use adapter::*`，所以这些入口既可通过模块路径调用，也可从 crate 根调用。Cargo 清单声明的外部依赖为 `log`、`rand`、`serde`、`serde_json`，本文件没有直接使用它们，也没有声明 TiKV 客户端依赖。

RustCodeGraph 将本文件索引为 13 个符号，但对 `handle_client_go_trace_event`、`handle_trace_control_extractor`、`register_with_client_go`、`map_category` 的精确 callers/callees 查询没有返回跨文件边。文本核验显示直接 Rust 使用者主要是 `adapter_test.rs`、`adapter_1_aster_unit_test.rs` 和 `test/integration_test.rs`；未发现生产 Rust 启动路径调用 `register_with_client_go`。这与源码注释所述“具体接线由包级集成任务完成”一致。

## 错误处理与边界

本文件所有公开函数都不返回 `Result`。未启用类别、缺少上下文 sink、未启动飞行记录器均是正常分支：前者静默丢弃事件，后两者只阻止 `IMMEDIATE_LOG`，不影响 TiKV 类别 flags。

未知客户端类别不会报错，而是映射为 `UNKNOWN_CLIENT` 并保留原始 `u32`。只有启用了 `unknown_client` 类别时，事件才会实际进入 `trace_event`；否则在追加诊断字段前就返回。若调用者已经提供同名 `client_go_category`，本实现会再追加一个字段，消费者需注意重复键的可能性。

`trace_event` 会再次执行类别启用检查，因此全局 recorder 在 adapter 首次检查和实际记录之间被关闭或替换时，事件可能被第二次检查过滤；这是并发配置变更下的安全退化，而非一致性快照保证。

下游 `get_flight_recorder` 读取全局 `RwLock` 时使用 `expect`，锁中毒会 panic；adapter 不捕获该 panic。注册函数也无法报告真实外部回调安装失败，因为当前根本不执行安装。

## 并发与资源生命周期

`REGISTERED` 使用 Release/Acquire 配对，保证其他线程观察到 `true` 时具备发布/获取顺序；目前注册函数没有伴随初始化数据，所以它主要提供线程安全的状态标记。重复调用只重复存储 `true`，没有注销或复位 API。

全局飞行记录器通过 `RwLock<Option<Arc<_>>>` 安装、获取和关闭。extractor 获得 `Arc` 快照后，即使其他线程随后关闭全局实例，本次 `should_keep` 仍可安全完成。`Context` 克隆 `Arc<Trace>`，而 `Trace::bits` 的同步由其实现承担；Rust 测试同时运行 100 个 extractor 和 10 个 `mark_bits` 线程，验证该访问不会 panic。Go 版在读取 `t.bits` 时显式持有 `RLock`，Rust 版通过 `Trace` 自身的同步封装实现相同目的。

本文件不创建线程、任务、channel、文件或网络连接，也不拥有需要显式关闭的资源。飞行记录器的启动和关闭属于 `flightrecorder.rs`；测试通过串行互斥与 `close_flight_recorder` 隔离进程级状态。

## 与 Go 版本的对应关系

Rust 的 `map_category`、事件过滤、未知类别字段、类别启用查询，以及 extractor 的三个 TiKV flags 与立即日志判定，逐项对应 [`adapter.go`](adapter.go)。Go 的 `handleTraceControlExtractor` 也先计算类别 flags，再检查 context sink、具体 `*Trace` 类型、飞行记录器和 keep 条件；Rust `Context` 的 sink 类型已经固定为 `Arc<Trace>`，所以无需 Go 的运行时类型断言。

主要差异在注册和接口类型：Go `RegisterWithClientGo` 调用 client-go `trace.SetTraceEventFunc`、`SetIsCategoryEnabledFunc`、`SetTraceControlExtractor`，形成真实回调链；Rust `register_with_client_go` 仅设置原子标记，Cargo 中也没有相应客户端依赖。Go handler 接收 `context.Context`、`trace.Category`、可变参数 `zap.Field`，Rust 接收自有 `Context`、`ClientCategory` 和 `Vec<Field>`。因此 Rust 当前验证的是语义适配，不是外部 client-go ABI/API 集成。

测试对应关系：`adapter_test.rs` 对齐 Go `adapter_test.go` 的 flags、默认类别和并发场景；`adapter_1_aster_unit_test.rs` 补充映射、未知类别字段、真值表及模式布局；`test/integration_test.rs::test_trace_control_integration` 覆盖注册标记和控制位组合，但没有证明真实 TiKV 客户端会调用这些回调。

## 扩展指南

新增客户端类别时，应同时修改 `ClientCategory`、`ClientCategory::raw` 和 `map_category`，并确认 `traceevent.rs` 已定义相应 `TraceCategory`、字符串解析和全集位图；随后在独立测试文件中增加已启用/未启用和未知兼容用例。不得把测试嵌入 `adapter.rs`。

新增控制位时，应在 `TraceControlFlags` 分配稳定且不冲突的位，更新 `handle_trace_control_extractor` 与必要时的 `category_bits`，并在 `adapter_test.rs` 覆盖单独类别、多类别组合、无 sink、无 recorder 及并发配置变化。控制位与客户端协议绑定，位值变化具有兼容风险；详情类事件还可能显著增加记录量和性能开销，因此默认配置应继续单独评估。

若完成真实 Rust 客户端接线，应优先在独立上游客户端仓库实现并发布带 tag 的依赖，再由本仓库 Cargo 清单统一引用该 tag；不能在本仓库 vendor/third_party 复制依赖，也不能用本地 `[patch]`。接线完成后，`register_with_client_go` 应安装三个 handler，并补充能从客户端 API 触发事件/查询/flags 的独立集成测试；在此之前不要把 `client_go_registered == true` 解释为真实回调已安装。

修改未知字段策略时要保持可诊断性，并决定重复 `client_go_category` 键的兼容规则。修改首次 `is_enabled` 过滤或下游二次过滤时，应评估 recorder 热切换下的竞态语义和额外事件构造成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/util/traceevent` 定位目标及相关 Go/Rust 文件；`node --file pkg/util/traceevent/adapter.rs --offset 1 --limit 260` 读取完整 172 行和 13 个符号；对四个关键入口执行 `callers`/`callees` 未得到跨文件边。
- 源码与模块：`pkg/util/traceevent/adapter.rs`、`lib.rs`、`traceevent.rs`（类别、`Context`、`is_enabled`、`trace_event`）、`flightrecorder.rs`（全局 recorder、`should_keep`、真值表和生命周期）。
- crate 边界：`pkg/util/traceevent/Cargo.toml` 和根 `Cargo.toml` workspace/path 声明；前者确认 Go 包映射与直接依赖列表。
- Go 对照：`pkg/util/traceevent/adapter.go`；另以 `traceevent.go` 的初始化调用核验 Go 生产路径会执行 `RegisterWithClientGo`。
- 独立测试：`pkg/util/traceevent/adapter_test.rs`、`adapter_1_aster_unit_test.rs`、`test/integration_test.rs`，以及 Go `adapter_test.go`、`test/integration_test.go`。
- 文档任务按计划不运行 Cargo；结构验证要求本文恰好包含上述 11 个固定二级标题，并在交付前以任务文件给定命令检查。
