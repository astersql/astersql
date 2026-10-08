# `pkg/server/handler/extractorhandler/extractor.rs`

## 文件定位

本文件是 `astersql-server-handler-extractorhandler` crate 的核心实现，负责把 Extract HTTP 查询参数转换为抽取任务，并把任务名或生成的 zip 内容写回 HTTP 响应。crate 根 `pkg/server/handler/extractorhandler/lib.rs` 通过 `pub mod extractor` 和 `pub use extractor::*` 导出这里的 API；`pkg/server/handler/extractorhandler/Cargo.toml` 将 crate 映射到 Go 包 `pkg/server/handler/extractorhandler`，没有声明 feature。

生产调用链为 `pkg/server/http_status.rs` 注册 `/extract_task/dump` → `pkg/server/extract.rs::ExtractTaskServeHandler::handle` 构造本文件的 `NewExtractTaskServeHandler` → `ExtractTaskServeHandler::ServeHTTP`。真正的 Domain 与外部存储操作不在本文件内，而由 `pkg/server/extract_runtime.rs::CanonicalExtractRuntime` 实现 `ExtractRuntime` 后注入。因此，本文件是协议与流程编排层，不是计划包生成或存储实现层。

## 核心职责

- 定义与具体 HTTP 框架、Domain 和对象存储解耦的请求、响应、reader、runtime 边界：`HttpRequest`、`HttpResponseWriter`、`ExtractReader`、`ExtractRuntime`。
- 由 `buildExtractTask` 按 `type` 分派任务；当前只接受大小写不敏感的 `plan`，并由 `buildExtractPlanTask` 解析时间窗和三个布尔开关。
- 由 `ExtractTaskServeHandler::ServeHTTP` 编排建任务、failpoint 短路、提交任务，以及非 dump/dump 两种响应路径。
- 由 `streamExtractResponse` 从 runtime 提供的目录和 reader 分块传输 zip，避免把整个产物载入内存。
- 用 `ExtractError`/`ExtractResult` 统一穿过抽象边界的错误，并把日志策略和 HTTP 错误呈现交给注入对象。

本文件不负责路由匹配、真实时间格式实现、Domain 抽取算法、归档生成或外部存储选择；这些职责分别位于 server 路由/适配层和 `CanonicalExtractRuntime`/Domain 中。

## 主要符号

- `EXTRACT_PLAN_TASK_TYPE: &str = "plan"`：当前唯一任务类型的内部判别值。
- `ExtractError(String)` 与 `ExtractResult<T>`：保留可比较、可显示的错误文本；实现 `std::error::Error`，但不保存错误源链。
- `RequestContext { request_id, cancelled }`、`HttpRequest { query, context }`：框架无关的最小请求模型。查询参数采用 `HashMap<String, String>`，同名多值已在上游折叠。
- `Timestamp(i64)`：秒级 Unix 时间包装；本文件只比较、传递和对缺省 begin 做饱和加法。
- `ExtractType::Plan`、`ExtractTask`：任务 DTO。`ExtractTask` 记录任务类型、后台标记、起止时间、是否跳过统计信息及是否使用历史视图。
- `ExtractReader: Send`：流式源，提供 `read` 与显式 `close`；`ExtractRuntime: Send + Sync`：注入时钟、时间解析、任务提交、目录/文件打开、failpoint 和日志能力；`HttpResponseWriter`：注入响应头、状态、body 写入和错误呈现能力。
- `ExtractTaskServeHandler<R> { ExtractHandler: R }` 与 `NewExtractTaskServeHandler`：持有泛型 runtime 的公开 handler 及构造器。字段和函数名保留 Go 命名，crate 根用 `#![allow(non_snake_case)]` 接纳这种兼容风格。
- `ServeHTTP`：公开主入口；`streamExtractResponse`：公开流式下载函数；`buildExtractTask`、`buildExtractPlanTask`、`extractBoolParam`：公开的解析辅助函数，亦供独立测试直接验证。
- `query_value`、`join_path`：私有辅助函数，分别实现缺失参数回退为空串，以及近似 Unix `filepath.Join` 的路径清理。

## 执行流程

1. `pkg/server/extract.rs::handle` 把 status server 的 query 克隆到 `HttpRequest`，创建默认 `RequestContext` 和响应适配器，再调用 `ServeHTTP`。
2. `ServeHTTP` 先调用 `buildExtractTask`。后者读取 `astersql_server_handler::util::TYPE`（值为 `type`）；仅当值大小写不敏感地等于 `plan` 时进入 `buildExtractPlanTask`，否则记录 `unknown extract task type` 并返回同文本错误。
3. `buildExtractPlanTask` 读取 `begin`/`end`。缺少 `begin` 时取 `runtime.now() + 30 分钟`，使用 `saturating_add` 防止 `i64` 上溢；缺少 `end` 时再次调用 `runtime.now()`。显式值交给 `runtime.parse_time`，任一解析失败都会记录对应 begin/end 日志并立即返回原错误。
4. 同一函数用 `extractBoolParam` 解析 `isDump`（默认 `false`）、`isSkipStats`（默认 `false`）和 `isHistoryView`（默认 `true`），构造 `ExtractType::Plan` 且 `is_background_job = false` 的任务，并将 `is_dump` 独立返回。
5. 建任务成功后，`ServeHTTP` 查询名为 `extractTaskServeHandler` 的 failpoint。启用时写状态 200 和 `mock`，不提交真实任务；body 写失败则交给 `write_error`。
6. 正常路径调用 `runtime.extract_task(&RequestContext::default(), task)` 取得任务名。这里刻意使用默认上下文；与 Go 的 `context.Background()` 对应，提交阶段不继承 HTTP 请求取消状态。
7. 非 dump 路径写状态 200 和任务名；写 body 失败只记录 `extract handler failed`，不再调用 `write_error`。dump 路径把原请求的 `request.context` 传给 `streamExtractResponse`，使打开/读取阶段可以感知请求取消。
8. `streamExtractResponse` 用 `join_path(runtime.extract_task_directory(), name)` 得到对象路径，打开 reader 后先设置 zip 的 `Content-Type` 和带 `{name}.zip` 的 `Content-Disposition`，再使用 32 KiB 缓冲循环读取。
9. 每个读块允许 writer 部分写入并循环补齐；若 writer 在仍有数据时返回 0，则报 `short response write`，避免死循环。无论复制成功还是失败，函数随后都调用一次 `reader.close()`；关闭错误被忽略，最后返回复制阶段结果。

## 数据与状态

本文件没有全局可变状态。handler 按值拥有 `R`，`ServeHTTP` 只借用 `&self`；任务、请求和时间均是调用内值。可变响应状态只存在于每次调用独占的 `&mut dyn HttpResponseWriter`，reader 也只在一次 `streamExtractResponse` 调用内存活。

关键默认值直接决定兼容行为：计划任务不是后台任务；`skip_stats=false`；`use_history_view=true`；`is_dump=false`。缺省时间窗按照现有 Go 代码形成 `begin=now+30min`、`end=now`，即 begin 晚于 end；这是已移植事实，不能在本层凭直觉交换。由于 begin 和 end 缺省时各调用一次 `now()`，两者不保证来自完全相同的时刻。

`HttpRequest.query` 只保存单值，参数缺失与显式空字符串在 `query_value` 后等价。`extractBoolParam` 仅接受 Go `strconv.ParseBool` 的字面量集合：真值 `1/t/T/true/True/TRUE`，假值 `0/f/F/false/False/FALSE`；空值或其他文本静默回落到调用者提供的默认值。

## 依赖与调用关系

上游直接调用者是 `pkg/server/extract.rs::ExtractTaskServeHandler::handle`；RustCodeGraph 的 `callers NewExtractTaskServeHandler --json` 将其解析为 `extract.rs::handle`。再上游是 `pkg/server/http_status.rs` 的 `/extract_task/dump` 路由。server 适配器用 `SharedExtractRuntime(Arc<dyn ExtractRuntime>)` 把动态 runtime 包成满足本文件泛型约束的值，并把本文件的响应抽象转换为 server `Response`。

本文件唯一直接使用的同仓生产依赖是 `astersql_server_handler::util` 中的 `BEGIN`、`END`、`TYPE`、`IS_DUMP`、`IS_SKIP_STATS`、`IS_HISTORY_VIEW` 常量。`Cargo.toml` 还声明 `astersql-domain`、`astersql-planner-extstore`、`astersql-types`、`astersql-util-logutil` 等移植边界依赖，但这些具体系统没有在本文件中直接调用；生产接线位于 server 的 `pkg/server/extract_runtime.rs`。

下游 runtime 关系由 trait 方法体现。`pkg/server/extract_runtime.rs::CanonicalExtractRuntime` 用系统时钟实现 `now`，用 `%Y-%m-%d %H:%M:%S` 解析时间，把本文件的 `ExtractTask` 转成 Domain `ExtractTask` 后调用 `ExtractHandle::extract_task`，并通过 `GetGlobalExtStorage(...).Open(...)` 提供归档 reader。由此主链是 status HTTP → 本文件编排 → server runtime → Domain 抽取/外部存储。

RustCodeGraph 对主要函数的 callee 结果确认了内部边：`buildExtractTask → buildExtractPlanTask/query_value/log_error`，`buildExtractPlanTask → now/parse_time/extractBoolParam/query_value`，`streamExtractResponse → extract_task_directory/join_path/open_extract/set_header/read/write/close`。图中对常见方法名存在跨文件同名噪声，文档只采用能由目标源码和适配层复核的边。

## 错误处理与边界

- 未知 `type`、begin/end 解析失败、任务提交失败、reader 打开/读取失败、响应写失败均会终止各自路径；`ServeHTTP` 在建任务、提交和 dump 流失败时调用 `write_error`，并在对应位置记录 error 或 warning。
- 非 dump 的任务名 body 写失败只记录日志，不改写响应。这与 Go `ServeHTTP` 的既有分支一致，扩展时不要无意改变为二次写错误响应。
- 设置响应头的接口没有返回错误；打开 reader 成功后才设置头。复制期间发生错误时，头或部分 body 可能已经发出，`write_error` 是否还能改变线上状态取决于具体 HTTP 适配器。
- `reader.close()` 始终在复制闭包之后调用，但其错误明确被丢弃；复制错误优先返回。若关闭失败需要可观察，必须先决定是否会覆盖更重要的读取/写入错误。
- writer 返回 0 且当前块尚未写完会转换为 `ExtractError("short response write")`。writer 若错误地返回大于剩余切片长度的计数，本文件没有额外防御，trait 实现必须遵守写入计数契约。
- `join_path` 清理 `.`、重复 `/` 和可消解的 `..`，绝对路径不会越过根；相对路径允许保留前导 `..`。它只做字符串级 Unix 路径规范化，不验证 `name` 是否受信任，也不提供文件系统沙箱。生产 runtime 当前打开的是对象存储路径，因此若任务名来源模型发生变化，应重新审计路径穿越边界。
- `Timestamp` 可为负数；本文件不校验窗口顺序或取值范围。生产 runtime 在转 `SystemTime` 时把负数压到 0，这属于下游行为而非本文件保证。

## 并发与资源生命周期

`ExtractRuntime: Send + Sync` 允许同一 handler 被并发请求共享；`ExtractReader: Send` 允许 reader 跨线程移动，但本文件在当前线程同步读取。`HttpResponseWriter` 不要求 `Send`，且以独占可变借用传入，因此每个请求必须使用自己的 writer 或由上游自行同步。

一次请求中，任务值在调用 `extract_task` 时被移动；dump reader 在 `open_extract` 成功后创建，循环结束或出错后显式 `close`，随后离开作用域。没有生成后台线程、异步任务、锁或通道。32 KiB 缓冲是每次 dump 请求独立分配，内存占用与产物大小无关；吞吐与背压由同步 `read`/`write` 调用自然传递。

请求取消只通过 `RequestContext` 由 runtime 解释。本文件在 dump 打开时传入请求上下文，但 `ExtractReader::read` 本身不接收上下文，所以打开之后能否及时中止取决于 reader/runtime 实现。任务提交使用默认上下文，不继承请求取消，这是与 Go `context.Background()` 一致的生命周期边界。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/handler/extractorhandler/extractor.go`。Rust 保留了 `ExtractTaskServeHandler`、`NewExtractTaskServeHandler`、`ServeHTTP`、`streamExtractResponse`、`buildExtractTask`、`buildExtractPlanTask`、`extractBoolParam` 的流程和 Go 风格名称。

- Go 直接依赖 `*domain.ExtractHandle`、`http.Request/ResponseWriter`、`extstore`、日志和 failpoint；Rust 用三个 trait 抽出这些能力，再由 `pkg/server/extract.rs` 与 `pkg/server/extract_runtime.rs` 接回真实系统。
- Go 的任务提交使用 `context.Background()`，流式打开使用 `req.Context()`；Rust 分别使用 `RequestContext::default()` 和 `request.context`，保持阶段差异。
- Go 用 `strings.ToLower(...) == "plan"`；Rust 用 `eq_ignore_ascii_case`。对 ASCII 常量 `plan`，可接受输入一致。
- Go 用 `time.Now().Add(30*time.Minute)` 与 `time.Now()`；Rust 用秒级 `Timestamp` 和饱和加法。生产 runtime 的显式时间格式仍是 Go `types.TimeFormat` 对应的 `%Y-%m-%d %H:%M:%S`。
- Go 用 `strconv.ParseBool`；Rust 显式枚举同一组合法拼写。两者都在非法布尔文本时回退默认值而不报错。
- Go 用 `filepath.Join`、`io.Copy` 和 `defer fileReader.Close()`；Rust 用 `join_path`、32 KiB 手写 copy 循环和显式 close。独立测试 `extractor_test.rs::stream_extract_response_cleans_path_like_filepath_join` 验证了 `nested/../task` 的关键清理语义，但 Rust 的字符串实现不是所有平台上 `filepath.Join` 的完整替代。
- Go 的 `io.Copy` 会处理短写；Rust 同样循环补齐，并额外把零进度写入识别为错误。两边都不向调用者报告 close 错误。

## 扩展指南

新增抽取类型时，应先扩展 `ExtractType` 和任务所需数据，再在 `buildExtractTask` 增加明确分派及专用 builder；同时扩展 runtime 到 Domain 的映射，不能只让解析通过而不接真实执行。对应测试应放在独立的 `pkg/server/handler/extractorhandler/extract_test.rs`，覆盖大小写、缺省值、显式参数和错误日志；不要把测试嵌入本源文件。

修改查询参数时，优先复用 `astersql-server-handler` 的公开常量并同步 Go 对照。若改变时间默认值、布尔容错、上下文选择或错误写出策略，这是 HTTP 兼容性变化，需同时检查 `pkg/server/extract.rs` 的适配测试和 Go `extract_test.go` 所表达的端到端意图。

修改流式传输时，以 `streamExtractResponse` 为接入点，并在 `extractor_test.rs` 增加独立 reader/writer 测试，至少覆盖多块读取、部分写、零进度写、read/write/open/close 错误及头设置时序。性能上应保持有界缓冲和流式背压，避免改为一次性读取整个 zip。若要传播 close 错误，应定义“复制错误与关闭错误同时出现”的优先级。

修改路径拼接时，需要同步验证 Unix 对象键、空目录/空名称、绝对路径、连续 `..` 和不可越根规则。当前单测只锁定一个 `filepath.Join` 样例，不能把它视为完整等价证明。

若增加共享缓存、异步 reader 或后台任务，必须继续满足 `ExtractRuntime: Send + Sync`，明确所有权、取消与关闭顺序，并把并发测试放在独立测试文件。crate 的生产 API 已被 server 适配层使用，重命名公开符号或改变 trait 方法会同时影响 `pkg/server/extract.rs`、`pkg/server/extract_runtime.rs`、`pkg/server/server.rs`、`pkg/server/runtime.rs` 及其测试。

## 验证依据

- 目标源码：`pkg/server/handler/extractorhandler/extractor.rs`，RustCodeGraph `node --file ... --offset 1 --limit 500` 返回完整 331 行与 44 个索引符号。
- 图查询：RustCodeGraph `query` 定位 `ExtractTaskServeHandler`、`NewExtractTaskServeHandler`、`streamExtractResponse`、`buildExtractTask`、`buildExtractPlanTask`、`extractBoolParam`；`callers NewExtractTaskServeHandler --json` 返回生产调用者 `pkg/server/extract.rs::handle`；`callees` 查询核对了 builder、runtime trait 方法、路径拼接与流式写出边。
- crate/模块边界：`pkg/server/handler/extractorhandler/Cargo.toml`、`pkg/server/handler/extractorhandler/lib.rs`；查询键定义：`pkg/server/handler/util.rs`。
- 生产入口与下游：`pkg/server/http_status.rs`、`pkg/server/extract.rs`、`pkg/server/extract_runtime.rs`、`pkg/server/runtime.rs`。它们分别证明路由注册、HTTP 适配、Domain/存储 runtime 实现和 Domain 暴露 runtime 的接线。
- Go 语义：`pkg/server/handler/extractorhandler/extractor.go`；Go 集成意图：`extract_test.go`；Go 测试进程设置：`main_test.go`。
- 独立 Rust 测试：`pkg/server/handler/extractorhandler/extract_test.rs` 覆盖 failpoint、默认/显式窗口、Go 查询键与布尔拼写、解析错误日志和 dump 流；`extractor_test.rs` 覆盖路径清理；`pkg/server/extract_test.rs` 覆盖 server 适配器委托到 canonical handler。`main_test.rs` 只负责测试环境的一次性初始化，不证明 handler 业务分支。
- 本任务是纯文档分析，按计划不运行 Cargo；完成检查采用任务指定的固定章节结构命令，并人工复核所有结论均可回指上述源码、调用图或测试。
