# `pkg/server/handler/optimizor/statistics_handler.rs`

## 文件定位

本文件属于 `astersql-server-handler-optimizor` crate；`pkg/server/handler/optimizor/lib.rs` 以 `pub mod statistics_handler` 公开该模块。它把统计信息相关的三个 HTTP 处理流程拆成与 Web 框架和具体 `Domain` 类型无关的 Rust 控制流：当前统计导出、历史统计导出、统计任务优先级队列快照。

这里的“HTTP 处理器”目前是可移植的处理核心而不是已经接入 Rust 服务器的完整端点。仓库内只有 `pkg/server/handler/optimizor/statistics_handler_test.rs` 实现 `StatisticsRuntime`；未找到生产运行时实现或 Rust 路由注册。实际在线路由仍见 Go 的 `pkg/server/http_status.go`：`/stats/dump/{db}/{table}`、`/stats/dump/{db}/{table}/{snapshot}` 和 `/stats/priority-queue` 分别接入三个 Go handler。

## 核心职责

- `StatsHandler::ServeHTTP`：读取 `db`、`table` 路由变量及 `dumpPartitionStats` 查询参数，查找当前表并导出 JSON 统计信息。
- `StatsHistoryHandler::ServeHTTP`：确认历史统计开关已启用，解析快照时间，优先使用快照 InfoSchema 中的表；快照表查询失败时记录回退并改用当前表，然后导出指定快照的历史统计。
- `StatsPriorityQueueHandler::ServeHTTP`：读取统计任务优先级队列的当前快照并写出。
- `StatisticsRuntime<D>`：集中定义所有环境相关操作，包括路由/查询参数读取、表查找、统计服务调用、错误构造、日志以及响应写入，使三个 handler 只负责步骤次序和短路规则。
- `parse_go_boolean`：严格复现 Go `strconv.ParseBool` 接受的 12 个布尔字面量。

## 主要符号

- `pub trait StatisticsRuntime<D>`：以 `Error`、`Table`、`Payload` 三个关联类型隔离具体实现。它定义 17 个操作；其中读取和查找操作返回拥有的值或 `Result`，写响应和记录日志通过 `&mut self` 暴露副作用。
- `pub struct StatsHandler<D>` / `NewStatsHandler`：拥有泛型 `domain: D`。`Domain(&self) -> &D` 暴露只读引用，供需要访问同一服务域的调用者使用。
- `StatsHandler::ServeHTTP<R>(&self, runtime: &mut R)`：当前统计导出的公开入口，约束 `R: StatisticsRuntime<D>`。
- `pub struct StatsHistoryHandler<D>` / `NewStatsHistoryHandler`：拥有历史统计流程使用的服务域。
- `StatsHistoryHandler::ServeHTTP`：历史统计入口；所有校验和表解析成功后才调用 `dump_historical_stats`。
- `pub fn getSnapshotTableInfo<D, R>(...) -> Result<R::Table, R::Error>`：薄委托函数，把 `domain`、`snapshot`、库名和表名原样传给 `StatisticsRuntime::snapshot_table`；独立存在以对应 Go helper 并形成明确的替换/测试点。
- `pub struct StatsPriorityQueueHandler<D>` / `NewStatsPriorityQueueHandler`：拥有队列查询所需服务域。
- `StatsPriorityQueueHandler::ServeHTTP`：队列快照入口。
- `fn parse_go_boolean(&str) -> Option<bool>`：模块私有解析器；真值为 `1/t/T/true/TRUE/True`，假值为 `0/f/F/false/FALSE/False`，其他输入返回 `None`。

文件没有条件编译项。`#![allow(dead_code, non_snake_case)]` 允许当前尚未生产接线的符号以及为对齐 Go 而保留的命名。

## 执行流程

`StatsHandler::ServeHTTP` 的顺序是：

1. 先调用 `set_json_content_type`，保证成功和错误路径都已声明 JSON 内容类型。
2. 读取 `db`、`table`，并只查看 `dumpPartitionStats` 的第一个值。
3. 参数缺失或第一个值为空时默认 `true`；非空时用 `parse_go_boolean` 解析。非法值经 `invalid_boolean` 构造错误、`write_error` 写出，并立即返回。
4. 通过 `current_table` 查找当前表；失败写错并停止，不调用 dump。
5. 调用 `dump_stats(domain, database, table, dump_partition_stats)`；成功写 `Payload`，失败写错误。

`StatsHistoryHandler::ServeHTTP` 的顺序是：

1. 设置 JSON 内容类型，并先调用 `historical_stats_enabled`。读取开关失败直接写错；值为 `false` 时用 `historical_stats_disabled` 构造专用错误；两者都在读取路由和解析快照前短路。
2. 读取 `db`、`table`、`snapshot`，通过运行时的 `parse_snapshot` 转为 `u64`。解析失败后不查询表。
3. 调用 `getSnapshotTableInfo`，最终委托 `snapshot_table` 查找快照表。
4. 快照查询成功就使用快照表；失败则先调用 `log_snapshot_fallback`，再调用 `current_table`。当前表也失败时写出当前表错误并结束，原快照错误只用于日志。
5. 使用选定的表和同一个 `snapshot` 调用 `dump_historical_stats`，然后在成功/失败两条路径分别调用 `write_data`/`write_error`。

`StatsPriorityQueueHandler::ServeHTTP` 只执行三步：设置 JSON 内容类型、调用 `priority_queue_snapshot`、按结果写数据或错误。

## 数据与状态

三个 handler 唯一持久字段都是按值拥有的 `domain: D`；构造函数不校验或转换它。请求级状态全部存在调用者提供的 `&mut R` 中，本文件自身不缓存表、统计 JSON、快照或队列状态。

当前统计流程中的 `dump_partition_stats` 默认值为 `true`。查询参数存在多个值时仅首值有效；首值为空也视为没有显式设置，因此仍为 `true`。历史流程中的 `snapshot: u64` 语义由运行时决定：Go 对照实现把路由中的本地时间戳解析为 `time.Time`，再用 `oracle.GoTimeToTS` 转为 TSO；Rust 控制流不自行解释字符串。

`Table` 和 `Payload` 都是运行时关联类型。handler 只在一次调用内借用表并把 payload 引用传给 `write_data`，不规定其具体结构或序列化方式。

## 依赖与调用关系

本文件没有 `use` 导入，直接依赖仅为 Rust 语言的泛型、trait、`Result`、`Option` 和字符串切片。crate 的 `Cargo.toml` 声明了 domain、infoschema、statistics-handle、server-handler、types、table 等迁移依赖，但这些具体 crate 尚未由本文件引用；未来生产 `StatisticsRuntime` 适配器才需要把这些具体服务映射到 trait 方法。

RustCodeGraph 对 `getSnapshotTableInfo` 给出的下游调用边是 `getSnapshotTableInfo -> StatisticsRuntime::snapshot_table`。源文件控制流还直接形成以下 trait 调用链：

- `StatsHandler::ServeHTTP -> route_value/query_values/current_table/dump_stats/write_data|write_error`；非法布尔值额外经过 `invalid_boolean`。
- `StatsHistoryHandler::ServeHTTP -> historical_stats_enabled/parse_snapshot/getSnapshotTableInfo`，快照失败分支经过 `log_snapshot_fallback -> current_table`，最终到 `dump_historical_stats -> write_data|write_error`。
- `StatsPriorityQueueHandler::ServeHTTP -> priority_queue_snapshot -> write_data|write_error`。

Rust 上游仅发现模块声明和独立测试直接构造 handler；未发现生产调用者。Go 上游则由 `pkg/server/http_status.go` 的 `startHTTPServer` 注册三个路由，`newStatsHandler`、`newStatsHistoryHandler`、`newStatsPriorityQueueHandler` 从 `TiDBDriver` 获取 `Domain` 后调用对应构造函数。

## 错误处理与边界

本文件不返回 handler 级 `Result`，而是把所有失败转换为一次 `runtime.write_error(error)` 并立即结束当前分支。这样要求运行时负责 HTTP 状态、错误编码和日志格式。

关键短路不变量由 `statistics_handler_test.rs` 验证：非法布尔值后不会查表；当前表查找失败后不会 dump；历史开关关闭或读取失败后不会解析快照；快照字符串解析失败后不会查快照表；回退到当前表仍失败时不会导出历史统计；任何 dump/队列错误都不会写成功 payload。

快照表失败是唯一被有意降级的错误：先记录原错误，再尝试当前 InfoSchema。若回退查找也失败，响应的是后一个错误。这个策略保持了 Go 版本“历史元数据不可用时使用最新表定义”的兼容行为，但可能让表结构与统计快照时刻不一致；扩展时不能悄悄移除日志或改变回退次序。

空的 `db`、`table`、`snapshot` 不在本层单独校验，而由 `current_table`、`snapshot_table` 或 `parse_snapshot` 决定错误。`parse_go_boolean` 大小写集合是封闭的，诸如 `yes`、任意空白或混合大小写 `TrUe` 都非法。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源。`ServeHTTP` 只需 `&self`，因此 handler 的 domain 不在请求处理中被替换；所有可变请求状态通过独占的 `&mut R` 串行推进。这一签名本身不保证 `D` 或 handler 可跨线程共享：生产适配层若要挂到并发服务器，必须按所用框架补足 `Send`/`Sync`、共享所有权和生命周期约束。

表和 payload 都局限于单次栈上控制流，失败分支通过立即 `return` 结束。资源清理完全归具体 `StatisticsRuntime` 实现负责；trait 没有取消、超时或显式 close 接口。Go 集成测试中的服务器、响应体和临时文件生命周期属于 Go 适配/测试层，不是此 Rust 文件当前提供的能力。

## 与 Go 版本的对应关系

Rust 的三个结构体、三个 `New...` 构造函数、`Domain`、三个 `ServeHTTP` 和 `getSnapshotTableInfo` 与 `pkg/server/handler/optimizor/statistics_handler.go` 同名或一一对应，流程分支保持一致：

- 当前统计：Go 从 Gorilla mux 和 URL query 取值，Rust 委托 `route_value`/`query_values`；二者默认导出分区统计并使用 Go `strconv.ParseBool` 的字面量集合。
- 表查找与 dump：Go 直接调用 `Domain.InfoSchema().TableByName` 和 `StatsHandle().DumpStatsToJSON`；Rust 分别抽象为 `current_table` 和 `dump_stats`。
- 历史统计：Go 直接检查 `CheckHistoricalStatsEnable`，按本地时区解析微秒精度时间并转 TSO；Rust 把开关检查和解析放入运行时，但保留完全相同的先后次序。
- 快照回退：Go 的 helper 先 `GetSnapshotInfoSchema(snapshot)` 再 `TableByName`，失败后记录 info 日志并使用最新 InfoSchema；Rust 用 `snapshot_table` 和 `log_snapshot_fallback` 表达同一策略。
- 队列快照：Go 调用 `StatsHandle().GetPriorityQueueSnapshot`；Rust 对应 `priority_queue_snapshot`。

当前迁移差异是 Rust 尚未实现把真实 `Domain`、HTTP 请求/响应和统计 handle 接到 `StatisticsRuntime` 的生产适配器，也未注册 Rust HTTP 路由。因此该文件已具备可单测的控制流语义，但不能单独证明 Rust 服务已对外提供这些端点。Go 的 `statistics_handler_test.go` 进一步覆盖真实服务器、统计导出/重新导入、分区开关、历史统计和队列未初始化/初始化状态；Rust 测试目前使用内存 fake，覆盖分支顺序而非端到端集成。

## 扩展指南

- 新增请求参数或前置校验时，优先在相应 `ServeHTTP` 中明确其相对顺序；若操作依赖 HTTP、Domain 或统计服务，在 `StatisticsRuntime` 增加最小方法，并在独立的 `statistics_handler_test.rs` fake 中记录事件，验证成功和短路路径。不要把测试嵌入生产源文件。
- 改动 `dumpPartitionStats` 时必须同步核对 Go 的 `strconv.ParseBool` 行为、首值规则和默认 `true`，并扩展 `current_stats_matches_all_go_boolean_literals` 或相邻错误测试。
- 改动历史快照时应同步更新 `history_uses_snapshot_table_and_writes_dump`、禁用/解析错误、回退成功和回退失败测试；生产适配器必须保持时区、精度和 TSO 转换与 Go 一致。
- 增加真实 Rust 接线时，应在本 crate 或服务器 crate 中实现生产 `StatisticsRuntime`，把三个构造函数注册到与 Go 相同的路径，并补独立集成测试。要特别评估 HTTP 错误格式、并发 `Send + Sync` 约束、Domain 共享成本、表元数据回退兼容性以及大统计 payload 的序列化/内存开销。
- 扩展队列快照时需保留“队列未初始化是错误”的现有语义，并覆盖初始化前后响应。
- 若改变公开的 Go 风格名称，应同时检查 `#![allow(non_snake_case)]` 的必要性、模块调用者和迁移映射，避免只为 Rust 命名习惯破坏逐项对照。

## 验证依据

- 源码：`pkg/server/handler/optimizor/statistics_handler.rs`（231 行），核对了 trait、三个 handler、构造函数、helper、私有布尔解析器及全部分支。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/server/handler/optimizor` 确认源文件、Go 对照和两侧测试均被索引；`query StatsHandler` 定位 Rust/Go 同名定义；`node --file ...statistics_handler.rs` 读取完整文件；`callers/callees` 查询确认构造函数无已记录下游调用，且 `getSnapshotTableInfo` 调用 `snapshot_table`。图中 callers 对这些泛型/同名符号未给出结果，因此又用精确文本搜索核验上游。
- crate 边界：`pkg/server/handler/optimizor/Cargo.toml` 与 `lib.rs`。前者声明 `astersql-server-handler-optimizor` 及其 domain/infoschema/statistics/server 依赖，后者公开 `statistics_handler` 并把 `statistics_handler_test.rs` 作为独立测试模块。
- Rust 测试：`pkg/server/handler/optimizor/statistics_handler_test.rs`。覆盖默认分区参数、全部 Go 布尔字面量、首查询值、非法值、表查找与 dump 错误、历史开关/解析/快照回退、helper 参数传递及队列成功/失败。
- Go 对照：`pkg/server/handler/optimizor/statistics_handler.go`、`statistics_handler_test.go`、`pkg/server/http_status.go`。它们分别提供具体实现、端到端行为证据和真实路由接线。
- Rust 全仓精确搜索：除本文件和独立测试外，仅 `lib.rs` 声明模块；未找到 `StatisticsRuntime` 的生产实现或三个 Rust handler 的生产构造调用。因此文中将 Rust 状态描述为“控制流已实现、生产适配/路由未验证且当前未发现”。
- 本任务按计划为纯文档分析，未运行 Cargo。交付前使用任务指定的固定标题命令验证文档存在且恰含 11 个规定章节，并人工复核所有运行状态陈述均有上述源码、图查询、配置或测试依据。
