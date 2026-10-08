# `pkg/server/extract.rs`

## 文件定位

`pkg/server/extract.rs` 是 `astersql-server` crate 中 Extract status HTTP 接口的适配层。模块由 `pkg/server/lib.rs` 以公开模块 `pub mod extract` 装配，`pkg/server/http_status.rs` 的 status 路由构建逻辑创建 `ExtractTaskServeHandler`，并把 `/extract_task/dump` 请求交给 `handle`。文件本身不解析 Extract 参数、不执行计划抽取，也不直接访问对象存储；这些业务行为由依赖 crate `astersql-server-handler-extractorhandler` 的 canonical handler 和 `pkg/server/extract_runtime.rs` 的生产运行时实现。

`pkg/server/Cargo.toml` 将本目录声明为 `astersql-server`（`lib.rs` 为 crate 根），并通过路径依赖 `handler/extractorhandler` 引入 `astersql-server-handler-extractorhandler`。因此本文件的职责是跨越 server 自有的 `Request`/`Response`、`Domain` 接口与 extractorhandler 的请求、响应写出器、运行时 trait 之间的边界。

## 核心职责

1. `ExtractTaskServeHandler::new` 从可选的 server `Domain` 中取得 `ExtractRuntime`，把运行时是否可用固化在 handler 状态中。
2. `ExtractTaskServeHandler::handle` 把 status server 的查询参数转换为 extractorhandler 的 `HttpRequest`，调用 `NewExtractTaskServeHandler(...).ServeHTTP(...)`，再返回 server 层 `Response`。
3. `SharedExtractRuntime` 用 `Arc<dyn ExtractRuntime>` 提供可克隆的运行时包装，并逐项转发 canonical handler 所需的时间、任务提交、对象读取、failpoint 和日志操作。
4. `ResponseWriter` 把 extractorhandler 的 `HttpResponseWriter` 操作映射到内存中的 server `Response`：更新 header/status、追加 body，或生成 500 错误响应。

这些职责刻意保持为薄适配：任务类型识别、begin/end 与布尔参数解析、failpoint 分支、任务提交以及 dump 文件读取都不在本文件内实现，证据见 `pkg/server/handler/extractorhandler/extractor.rs`。

## 主要符号

- `SharedExtractRuntime(Arc<dyn ExtractRuntime>)`：私有、可克隆的新类型。它解决 trait object 本身需要共享所有权、而 canonical `ExtractTaskServeHandler<R>` 按值持有具体运行时的问题。其 `ExtractRuntime` 实现的 `now`、`parse_time`、`extract_task`、`extract_task_directory`、`open_extract`、`failpoint_enabled`、`log_error`、`log_warning` 均无附加策略，只委托给内部对象。
- `ResponseWriter { response: Response }`：私有响应适配器。`new()` 建立状态码 200、空 header、空 body 的初始响应；`set_header` 覆盖同名 header；`write_status` 替换状态码；`write` 追加全部输入并返回输入长度；`write_error` 把状态码设为 500，并以 `ExtractError::to_string()` 的字节完整替换 body。
- `ExtractTaskServeHandler { runtime: Option<SharedExtractRuntime> }`：本文件唯一公开类型，并派生 `Clone`。字段保持私有，调用者只能经 `new` 建立实例并经 `handle` 处理请求。
- `ExtractTaskServeHandler::new(domain: Option<Arc<dyn Domain>>) -> Self`：公开构造函数。`domain` 缺失、或 `Domain::extract_runtime()` 返回 `None`，都会产生 `runtime: None`；成功时把运行时包装为 `SharedExtractRuntime`。
- `ExtractTaskServeHandler::handle(&self, request: &Request) -> Response`：公开同步入口。它只复制 `request.query`，并为 extractorhandler 请求创建默认 `RequestContext`；原请求的 method、path、raw query、headers 与 body 不向下传递。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

1. `pkg/server/http_status.rs` 从 `Server::domain()` 获取 `Option<Arc<dyn Domain>>`，调用 `ExtractTaskServeHandler::new`，然后把闭包注册到 `/extract_task/dump`。
2. `new` 调用 `Domain::extract_runtime()`。生产实现 `CanonicalServerDomain` 在 `pkg/server/runtime.rs` 中持有并返回同一个 `Arc<CanonicalExtractRuntime>`；缺少 Domain 或运行时则保留 `None`。
3. 请求到达时，`handle` 先克隆 `SharedExtractRuntime`。若不存在运行时，立即通过 `serve_error(503, "domain is not initialized")` 返回带换行文本和 `X-Go-Pprof: 1` 的 503 响应。
4. 运行时存在时，`handle` 构造 canonical `NewExtractTaskServeHandler(runtime)`，把 `Request.query` 克隆到 `ExtractRequest`，并使用 `RequestContext::default()`。
5. `ResponseWriter::new` 生成默认 200 的空响应，随后调用 canonical `ServeHTTP`。后者在 `pkg/server/handler/extractorhandler/extractor.rs` 中构造 `ExtractTask`，处理 `extractTaskServeHandler` failpoint，提交任务；非 dump 返回任务名，dump 则打开产物并按 32 KiB 缓冲区循环读取。
6. canonical handler 对 writer 的状态、header、body 或错误操作被写入 `ResponseWriter.response`，`handle` 最后按值返回该 `Response`。

## 数据与状态

- handler 的长期状态只有 `runtime: Option<SharedExtractRuntime>`。构造完成后没有修改路径；`handle` 每次克隆其中的 `Arc`，不会复制底层运行时。
- 请求转换只保留 `HashMap<String, String>` 形式的 query。重复 query key 在进入本文件前已被 server 请求解析层折叠；本文件不会重新解析 `raw_query`。
- 下游 `RequestContext` 总是默认值，即空 `request_id` 且 `cancelled == false`。当前适配器没有把 TCP/HTTP 请求取消状态或请求标识传播给 extract runtime。
- `ResponseWriter` 在内存中拥有完整的 `Response`。每次成功 `write` 都把字节追加到 `Vec<u8>`；`write_error` 会丢弃此前 body 并替换为错误文本，但不会清除此前设置的 headers。
- canonical dump 路径虽然以 32 KiB 分块从 `ExtractReader` 读取，适配器仍把所有分块累积进 `Response.body`。所以它降低了读取端的单次缓冲需求，却不是端到端流式发送；大产物的响应内存占用仍随产物大小增长。

## 依赖与调用关系

上游直接调用关系：

- `pkg/server/http_status.rs` 导入 `crate::extract::ExtractTaskServeHandler`，在 status 路由装配处调用 `new(server.domain())`，并由 `/extract_task/dump` 路由闭包调用 `handle(request)`。
- `pkg/server/extract_test.rs` 直接构造 handler 并调用 `handle`，验证适配器确实委托给 canonical handler。

下游直接依赖关系：

- `crate::server::Domain` 提供可选 `extract_runtime`；trait 默认返回 `None`，生产 `CanonicalServerDomain` 在 `pkg/server/runtime.rs` 返回已构造的运行时。
- `crate::http_status::{Request, Response, serve_error}` 定义 server 层 HTTP 数据结构与未初始化时的 503 响应。
- `astersql_server_handler_extractorhandler::extractor` 提供 `ExtractRuntime`、`ExtractReader`、`ExtractTask`、`RequestContext`、`HttpResponseWriter`、`NewExtractTaskServeHandler` 等 canonical 边界。
- `pkg/server/extract_runtime.rs` 的 `CanonicalExtractRuntime` 把任务提交接到 domain `ExtractHandle`，并把 dump 文件读取接到全局外部存储。

RustCodeGraph 的目标文件查询确认 `pkg/server/extract.rs` 已索引、包含 24 个符号，并报告它被 7 个文件使用；精确调用边命令在本次检查的 30 秒窗口内未返回，因此上述调用边以已索引源码和精确引用搜索交叉核对。

## 错误处理与边界

- Domain 或 extract runtime 不可用时，`handle` 不构造 canonical handler，直接返回 503。该错误与下游 Extract 错误不同：它由 server 层 `serve_error` 生成，而不是 `ResponseWriter::write_error`。
- canonical handler 传入 `write_error` 的任意 `ExtractError` 都被转换成状态 500 和纯文本 body。本适配器不区分参数错误、任务提交错误、存储打开/读取错误，也不添加 content type。
- `ResponseWriter::write` 对任意输入都完整追加并返回 `Ok(data.len())`，因此不会产生短写或自身 I/O 错误。canonical `streamExtractResponse` 中的“零字节短写”保护在此 writer 上不会触发。
- `ResponseWriter::write_error` 在错误发生于部分 dump 写出之后时替换已有 body；canonical 层先设置的 zip headers 仍可能保留。这是当前代码事实，扩展错误映射时需避免形成“500 + zip header + 错误文本”的含混响应。
- canonical `streamExtractResponse` 会尝试关闭 reader，但忽略 `close` 的错误；这不是本文件施加的行为。本文件的 `SharedExtractRuntime::open_extract` 也不包装或改写底层错误。
- `handle` 不检查 HTTP method 或 path；可达性与路径匹配由 `http_status.rs` 的 router 负责，任务类型及参数合法性由 canonical handler 负责。

## 并发与资源生命周期

- `Domain` 与 `ExtractRuntime` 均通过 `Arc` 共享，相关 trait 要求 `Send + Sync`。`ExtractTaskServeHandler` 和 `SharedExtractRuntime` 可克隆，因此同一个运行时能安全地被多个请求 handler 共享；具体内部同步责任属于底层运行时。
- `handle` 是同步函数，不创建线程、异步任务、锁、channel 或事务。每次请求创建独立的 canonical handler、extract 请求和 `ResponseWriter`，这些短生命周期对象在返回 `Response` 后销毁。
- 生产 `CanonicalServerDomain` 在构造时创建 `CanonicalExtractRuntime`，此后 `extract_runtime()` 克隆其 `Arc`；因此 Extract handler 的运行时寿命至少覆盖持有该 `Arc` 的请求处理过程。
- dump reader 的创建、分块读取和关闭由 canonical handler 控制。server 适配器只持有聚合后的 body，不直接持有 reader；如前述，最终响应 body 的内存生命周期持续到外层 HTTP 响应发送完毕。
- 默认 `RequestContext` 不携带取消信号，因而生产 runtime 中针对 `context.cancelled` 的任务/读取取消分支无法由本适配入口触发。

## 与 Go 版本的对应关系

`pkg/server/extract.go` 的 `(*Server).newExtractServeHandler` 是本文件 server 级构造逻辑的直接对照：Go 在 `s.dom != nil` 时把 `s.dom.GetExtractHandle()` 交给 `extractorhandler.NewExtractTaskServeHandler`，否则返回一个空 handler；Rust 则从抽象 `Domain::extract_runtime()` 取得 trait object，并在缺失时显式返回 503，避免空 handler 后续解引用。

`pkg/server/handler/extractorhandler/extractor.go` 与同目录 Rust `extractor.rs` 承担真正业务逻辑。两边均支持 `type=plan`、默认/显式时间窗口、`isDump`/`isSkipStats`/`isHistoryView`、failpoint mock、任务名响应和 zip 产物流式读取。Go 直接使用 `http.Request.Context()`；本文件当前使用默认 `RequestContext`，取消传播并不等价。Go 的 `io.Copy` 直接写 `http.ResponseWriter`，Rust canonical handler 虽分块读取，但本适配器最终聚合为 `Vec<u8>`，内存行为也不完全等价。

测试对应关系：

- `pkg/server/extract_test.rs::server_extract_adapter_delegates_to_canonical_handler` 用自定义 `Domain`/`ExtractRuntime` 开启同名 failpoint，断言 adapter 返回 200 与 `mock`，证明 server 层委托链成立。
- `pkg/server/handler/extractorhandler/extract_test.rs` 独立覆盖参数默认值、Go camelCase query key、Go 风格布尔值、时间解析错误记录以及 dump header/body；这些是 canonical 层测试，不应内嵌进本文件。
- Go `pkg/server/handler/extractorhandler/extract_test.go` 通过真实 status client 和 `/extract_task/dump` 路由覆盖显式、默认时间范围以及 failpoint 成功响应。Rust 适配测试比 Go 集成测试更聚焦，尚未在本文件对应测试中覆盖 runtime 缺失的 503 或大 body 聚合行为。

## 扩展指南

- 若新增 server 与 canonical handler 之间需要传递的请求字段，应修改 `ExtractTaskServeHandler::handle` 的转换，并同步扩展 extractorhandler 的 `HttpRequest`；尤其是取消、request ID、method 或 header，不应只在 server `Request` 中增加而遗漏桥接。
- 若新增 `ExtractRuntime` trait 方法，必须在 `SharedExtractRuntime` 中做完整转发，并在 `pkg/server/extract_runtime.rs::CanonicalExtractRuntime` 与 `pkg/server/extract_test.rs` 的假运行时中实现；漏掉任一层会破坏适配边界。
- 若改变错误状态或响应格式，应集中审视 `ResponseWriter::write_error`、未初始化的 `serve_error` 分支和 canonical handler 的错误调用点，并增加 `pkg/server/extract_test.rs` 的独立回归测试。不要把测试写进 `extract.rs`。
- 若要实现真正的端到端 dump 流式响应，需要调整 server `Response`/router 的响应模型，而不仅是修改 canonical 读取循环；当前 `Vec<u8>` 聚合是架构边界。
- 若扩展 URL 或 HTTP method，路由接线位于 `pkg/server/http_status.rs`，且属于 status HTTP API 行为变更；按 `pkg/server/AGENTS.md` 还必须同步 `docs/tidb_http_api.md`。
- 保持 Go 兼容时应同步检查 `pkg/server/extract.go`、`pkg/server/handler/extractorhandler/extractor.go` 及其 Go/Rust 独立测试，重点关注 query key、时间格式、布尔解析、状态码、header、取消和内存行为。

## 验证依据

- RustCodeGraph：`status` 显示索引有效（11467 个文件），`files --filter pkg/server/extract.rs` 确认目标文件已索引；`node --file pkg/server/extract.rs --offset 1 --limit 500` 读取了完整 140 行并报告 24 个符号及 7 个使用文件；`query ExtractTaskServeHandler`、`query SharedExtractRuntime`、`query ResponseWriter` 用于消歧。精确 `callers/callees` 批量查询在 30 秒窗口内未产出结果，未据此声称额外调用边。
- 目标与装配：`pkg/server/extract.rs`、`pkg/server/lib.rs`、`pkg/server/Cargo.toml`、`pkg/server/http_status.rs`、`pkg/server/server.rs`、`pkg/server/runtime.rs`、`pkg/server/extract_runtime.rs`。
- canonical Rust 实现与测试：`pkg/server/handler/extractorhandler/extractor.rs`、`pkg/server/extract_test.rs`、`pkg/server/handler/extractorhandler/extract_test.rs`。
- Go 对照与测试：`pkg/server/extract.go`、`pkg/server/handler/extractorhandler/extractor.go`、`pkg/server/handler/extractorhandler/extract_test.go`。
- 人工事实复核：确认本文件为何存在（HTTP/trait 边界适配）、如何运行（status 路由到 canonical handler）、如何安全扩展（同步请求转换、runtime 转发、响应模型和独立测试），并明确了取消未传播和 body 聚合两项当前限制。
