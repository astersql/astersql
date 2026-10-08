# `pkg/server/handler/tikvhandler/dxf.rs`

## 文件定位

本文件是 `astersql-server-handler-tikvhandler` crate 中 DXF（Distributed eXecution Framework）运维 HTTP 接口的业务层。模块由 `pkg/server/handler/tikvhandler/lib.rs` 公开导出；`pkg/server/http_status.rs` 将服务器请求转换为本文件的 `Request`，构造 handler，最后把 `JsonValue`/`DxfError` 转回 HTTP 响应。当前接线覆盖 `/dxf/schedule/status`、`/dxf/task/active`、`/dxf/nodes`、`/dxf/task/history`、`/dxf/schedule`、`/dxf/schedule/task_cleanup_batch_size`、`/dxf/schedule/max_concurrent_task`、`/dxf/import-into/history/job/{keyspace}/{job_id}`、`/dxf/schedule/tune` 和 `/dxf/task/{taskID}/max_runtime_slots`（`pkg/server/http_status.rs:2153-2202`）。

本文件不直接拥有 HTTP socket、路由表或 DXF 存储实现。它负责方法检查、输入解析、业务约束、上下文标记、响应形状和错误到状态码的局部映射；除 cleanup batch size 直接调用 proto 的进程级配置外，其余外部副作用经 `DxfRuntime` 注入。crate 边界及所需 DXF、KV、meta、naming、logutil 依赖见 `pkg/server/handler/tikvhandler/Cargo.toml`。

## 核心职责

- 提供八组持有 `Arc<dyn DxfRuntime>` 的 handler，以及另行持有 `StorageHandle` 的 `DXFScheduleTuneHandler` 和无运行时字段的 `DXFTaskCleanupBatchSizeHandler`（`global_handler!`、`NewDXFScheduleTuneHandler`、`NewDXFTaskCleanupBatchSizeHandler`）。
- 查询调度状态、活跃任务摘要、注册节点、历史任务和 IMPORT INTO 历史 job；将运行时返回值转换为稳定的 `JsonValue` 响应（各 `ServeHTTP`）。
- 修改暂停缩容标志、调度放大系数、进程内最大并发任务数、进程内清理批量和单任务最大运行槽位（`parsePauseScaleInFlag`、`DXFScheduleTuneHandler::ServeHTTP`、两个内存配置 handler、`DXFTaskMaxRuntimeSlotsHandler::ServeHTTP`）。
- 复刻 Go handler 的参数默认值和校验：历史页大小默认使用 storage 常量、页 token 必须为正、keyspace 必须合法、TTL 默认一小时、任务 ID/槽位必须为正、`max_runtime_slots < required_slots`、目标 step 必须适用于任务类型。
- 在需要访问内部任务数据的调用上设置 `DxfContext.internal_dist_task = true`，并为所有运行时调用附加十秒截止时间（`timed_context`、`background_timed_context`）。

## 主要符号

- `DxfErrorKind`、`DxfError`、`DxfResult<T>`：统一错误载体。`TaskNotFound` 是 IMPORT INTO 历史查询映射 HTTP 404 的唯一专门类别，其余错误默认为 `General`。
- `DxfContext`：携带请求 ID、Unix 秒截止时间和内部 DXF 任务标记。`timed_context` 保留传入字段并覆盖截止时间；`background_timed_context` 从默认上下文开始。
- `Request`：已解析的 HTTP 视图。私有读取器分别实现 query、form 首值、form 多值和 path 缺省为空串的语义。真正的 HTTP 表单/query 合并发生在 `pkg/server/http_status.rs:dxf_request`。
- `JsonValue`、`ResponseWriter`：与具体 HTTP/serde 层解耦的响应抽象；上层 `DxfHttpResponseWriter` 记录数据或错误，`dxf_response` 再生成状态码和 JSON。
- `TTLInfo`、`TTLFlag`、`TTLTuneFactors`、`Flag`：暂停缩容和调度调参的数据模型。`ttl_flag_json`、`tune_factors_json` 保持 Go 嵌入结构的扁平 JSON 字段。
- `Task`、`ExtraParams`、`Step`：运行槽位修改所需的最小任务视图，避免 HTTP 层直接依赖完整任务管理器类型。
- `DxfRuntime`：生产副作用边界，涵盖时钟/时长解析、校验、状态查询、PD keyspace 加载、事务写入、进程配置、任务读写、step 语义和日志。
- `parseTaskHistoryQuery` 与 `parseStoredTaskHistoryQuery`：共享 `parse_task_history_query`；前者走运行时校验，后者供无运行时的 HTTP fallback 使用真实 storage/naming 校验。
- `DXF_OPERATION_DEFAULT_TTL_SECONDS`：一小时默认 TTL；`REQUEST_DEFAULT_TIMEOUT_SECONDS`：十秒请求截止时间。

## 执行流程

1. `pkg/server/http_status.rs` 的路由闭包取得 `Server`/`Domain`，构造本文件的 `Request`，并从 domain 取得 `Arc<dyn DxfRuntime>`；不可用时上层直接返回 unavailable 或 404。
2. handler 首先检查 HTTP 方法和输入。查询接口只允许 GET，暂停/恢复与运行槽位只允许 POST，调参和两个进程内配置接口允许 GET/POST。
3. 查询类 handler 创建十秒 `DxfContext` 后调用 `DxfRuntime`。节点查询保留请求上下文；状态和活跃摘要以默认背景上下文开始。历史任务先解析 `page_size`、`page_token`、`keyspace`；IMPORT INTO 历史查询还校验路径中的正 job ID，并设置内部任务标记。
4. `DXFScheduleHandler` 将 `action=pause_scale_in|resume_scale_in` 解析为 `TTLFlag`。pause 解析 `ttl` 或使用 3600 秒默认值，resume 禁用标志且不携带 TTL，然后调用 `update_pause_scale_in_flag`。
5. `DXFScheduleTuneHandler` 先校验 keyspace 并尝试从 PD 加载。GET 读取当前 factors；POST 解析 TTL 和浮点 `amplify_factor`、检查运行时提供的上下界、设置内部任务标记，再要求运行时在新事务中写入。
6. 最大并发和 cleanup batch size 的 GET 返回当前值与 `persistence=memory_only`；POST 解析整数、调用各自 setter 校验并修改进程级状态，然后回读响应。前者经 `DxfRuntime`，后者直接经 `astersql_dxf_framework_proto`。
7. 最大运行槽位接口解析正任务 ID、正槽位值和可重复的 `target_step`，读取任务，检查槽位严格小于 `required_slots`，逐一验证并格式化 step，最后更新 `ExtraParams` 并返回任务摘要。
8. `ResponseWriter` 接收成功数据、默认错误或显式状态码错误；`pkg/server/http_status.rs:dxf_response` 将默认错误映射为 400，将显式 404/500 保持到 HTTP 层。

## 数据与状态

`Request`、`DxfContext`、任务摘要和 TTL 类型均按值构造；handler 的共享依赖是只读的 `Arc<dyn DxfRuntime>`。`DXFScheduleTuneHandler` 额外保存 `StorageHandle`，作为 PD keyspace 加载和新事务写入的目标句柄。`JsonValue::Object` 使用有序 `Vec<(String, JsonValue)>`，测试会直接断言字段及响应形状。

暂停缩容和调参数据带绝对过期 Unix 秒，计算使用 `saturating_add`，避免整数溢出 panic。历史分页默认页大小由 `astersql_dxf_framework_storage::DefaultHistoryTaskPageSize` 提供，未传 token 时为 0；显式 token 必须大于 0。`Task.extra_params` 在本地副本上更新后整体传给运行时，原任务的 key、required slots 和类型保持不变。

最大并发任务数与 cleanup batch size 是当前 TiDB 进程内状态，响应明确声明 `memory_only`；它们不会因本文件调用而持久化或广播，调用者应把请求发送给当前 DXF owner。调度 tune factors 则要求 `set_schedule_tune_factors_in_new_txn` 在新事务中持久化。

## 依赖与调用关系

上游主链是 `pkg/server/http_status.rs` 的十个 `dxf_*_response` 适配函数及其路由注册。`pkg/server/handler/tikvhandler/lib.rs` 通过 `pub use dxf::*` 暴露构造器和类型。RustCodeGraph 的文件节点还显示本文件被 `pkg/server/handler/tests/dxf_test.rs`、`pkg/server/handler/tikvhandler/dxf_test.rs`、`pkg/server/handler/tikvhandler/global_variables.rs`、`pkg/dxf/framework/handle/status.rs` 等文件引用；精确生产 HTTP 调用边由 `http_status.rs` 的构造器调用核实。

下游主要分为三类：

- `DxfRuntime` 动态调用：状态/任务查询、历史读取、PD keyspace 加载、事务写 tune factors、进程并发配置、任务参数更新和日志。动态 trait 分派意味着静态图只能落到 trait 方法，具体生产实现由 domain 注入。
- 直接 crate 调用：历史 fallback 使用 `astersql_dxf_framework_storage::ValidateHistoryTaskPageSize` 和 `astersql_util_naming::CheckKeyspaceName`；cleanup 配置调用 `astersql_dxf_framework_proto::{Get,Set}TaskCleanupBatchSize`；节点响应消费同 crate 的 `ManagedNode`。
- HTTP 适配：`dxf_request` 处理 query/form/path，`DxfHttpResponseWriter` 实现 `ResponseWriter`，`dxf_json` 和 `dxf_response` 完成 serde JSON 与状态码转换（均在 `pkg/server/http_status.rs`）。

## 错误处理与边界

方法错误、缺失/非法数字、非法 action/TTL/keyspace、越界 factor、无效 step 和不满足槽位不变量均在发生副作用前通过 `write_error` 返回，最终通常是 HTTP 400。运行时查询失败在调度状态、活跃摘要、节点、历史列表、调参读写和任务参数更新等路径上按实现选择显式 500；IMPORT INTO 历史的 `TaskNotFound` 专门返回 404，其他运行时错误保留默认错误路径。

历史页大小的底层校验错误被规范化为 `invalid page_size <value>`，不泄露校验器内部文案。keyspace 只有非空历史过滤值才校验，而 IMPORT INTO/tune 的目标 keyspace 必须非空。浮点解析允许 Rust `f64` 能解析的值；实际接受范围还受 `factor < minimum || factor > maximum` 限制，扩展时应特别评估 NaN 的比较语义。TTL 秒数没有在本层另做正值检查，合法范围由 `DxfRuntime::parse_duration_seconds` 的生产实现决定。

本文件的 `Request` 不是完整 HTTP 解析器：例如 percent decoding、body/query 优先级和路由变量提取属于 `pkg/server/http_status.rs`。因此修改参数来源时必须同时检查适配层，不能只调整这里的 `form_value`。

## 并发与资源生命周期

`DxfRuntime: Send + Sync` 且由 `Arc` 共享，使 handler 可安全跨请求复用；本文件自身不创建线程、异步任务、锁或通道。十秒截止时间以 Unix 秒写入 `DxfContext`，真正的取消和资源释放责任在运行时实现；与 Go 的 `context.WithTimeout`/`defer cancel` 相比，这里没有本地 cancel guard，因此生产运行时必须主动遵守 deadline。

调参写入要求运行时开启独立事务，成功后才写响应。任务 extra params 同样在完整校验后一次性更新，避免部分 step 已落库。两个 `memory_only` 配置是进程级共享可变状态，其线程安全和范围校验由 proto/runtime setter 负责；相关 Rust 测试对 cleanup 全局值使用互斥锁并在结束时恢复，说明测试和扩展代码必须避免并发污染。

请求级 context 的继承有意不同：节点和历史列表从 `request.context` 克隆以保留请求 ID/取消语义；状态、活跃摘要、IMPORT INTO、调度修改、调参和运行槽位从默认背景 context 开始。改变这一点会影响与 Go 版本的取消行为，应先补对应测试。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/server/handler/tikvhandler/dxf.go`。Rust 保留了 Go 的 handler 分组、方法限制、默认一小时 TTL、十秒超时意图、历史分页规则、keyspace/job ID 校验、404/500 分支、tune factor 范围、新事务写入、进程内配置响应和运行槽位不变量。

Rust 的主要结构性差异是把 Go 中对 `handle`、`storage`、`meta`、`proto`、PD 和 logger 的直接调用集中到 `DxfRuntime`，便于 `pkg/server/http_status.rs` 注入 domain 运行时和测试替身；cleanup batch size 仍与 Go 一样直接调用 proto 全局配置。Go 的 `http.Request.FormValue` 语义由 Rust 上层 `dxf_request` 合并 query/body 来模拟，多值 `target_step` 则保留 body 列表。

Go 使用 `context.WithTimeout` 并显式 cancel，Rust 只传递 deadline 数据。Go `DXFNodesHandler` 继承 `req.Context()`，Rust 对应地从 `request.context` 创建 timed context。Go 的 `schstatus.TTLFlag`/`TTLTuneFactors` 通过嵌入字段序列化为扁平 JSON，Rust 由 `ttl_flag_json`/`tune_factors_json` 显式构造同形响应。`pkg/server/handler/tikvhandler/dxf_test.go` 与 Rust 单元测试共同验证节点错误/取消和 pause TTL；更完整的 Rust HTTP/运行时对齐覆盖在 `pkg/server/handler/tests/dxf_test.rs`。

## 扩展指南

新增 DXF HTTP 操作时，优先沿用现有边界：在本文件增加最小请求/响应模型和 handler；把外部副作用加入 `DxfRuntime` 及其生产/测试实现；在 `pkg/server/http_status.rs` 增加 Request/Response 适配和路由；在独立的 `pkg/server/handler/tikvhandler/dxf_test.rs` 或 `pkg/server/handler/tests/dxf_test.rs` 增加回归测试，不把测试嵌入生产源文件。

修改已有参数时应同步核查三处：本文件的解析与不变量、`http_status.rs:dxf_request` 的参数来源/优先级、Go `dxf.go` 的可观察语义。修改 JSON 字段时还要同步 `ttl_flag_json`/`tune_factors_json` 或对应 `JsonValue` 构造，以及 HTTP 端到端字符串断言。新增运行时调用必须明确是否继承请求 context、是否设置 `internal_dist_task`、是否要求新事务，以及失败应映射 400、404 还是 500。

性能风险主要在分页上限、节点/任务列表大小、额外的 PD 加载和事务次数；兼容风险主要在错误文案、字段名/扁平结构、form/query 优先级和 `memory_only` 作用域。涉及进程级配置的测试需序列化并恢复旧值。若改变 Rust 行为，应先与 `dxf.go` 和相邻 Go 测试确认差异确属移植目标，而不是为通过测试而简化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter pkg/server/handler/tikvhandler` 确认目标、Go 对照和测试均已索引；`node --file pkg/server/handler/tikvhandler/dxf.rs` 读取 932 行完整实现；`callees parseTaskHistoryQuery` 确认 Rust 入口调用两个运行时校验方法和共享解析函数。自然语言 `explore` 对同名宽泛词结果噪声较大，宏生成构造器又与 Go 同名，因此生产调用边以索引文件引用提示和 `http_status.rs` 的精确构造器引用交叉核验。
- 源码与 crate：`pkg/server/handler/tikvhandler/dxf.rs`、`pkg/server/handler/tikvhandler/lib.rs`、`pkg/server/handler/tikvhandler/Cargo.toml`、`pkg/server/http_status.rs`。
- Go 对照：`pkg/server/handler/tikvhandler/dxf.go`；Go 测试 `pkg/server/handler/tikvhandler/dxf_test.go` 覆盖节点读取错误/请求取消及 pause TTL 的非法、默认和显式值。
- Rust 独立测试：`pkg/server/handler/tikvhandler/dxf_test.rs` 覆盖 pause/resume TTL、扁平 JSON 和 cleanup 非法方法不改值；`pkg/server/handler/tests/dxf_test.rs` 覆盖路由、状态/节点、调参、最大并发、运行槽位、历史分页/keyspace、IMPORT INTO 聚合/404 和 cleanup 的进程内语义。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且固定二级标题恰好为 11 个。
