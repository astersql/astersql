# `pkg/server/http_status.rs`

## 文件定位

本文件属于 `astersql-server` crate 的 status/运维 HTTP 子系统，由 [`pkg/server/lib.rs`](lib.rs) 以 `pub mod http_status` 暴露。它不是 SQL 协议入口，而是 `Server` 在独立监听地址上提供健康检查、指标、诊断、元数据、TiKV/DDL/DXF 管理等 HTTP 路由的实现。生产启动链由 `pkg/server/server.rs` 的 `Server::run` 调用本文件的 `Server::start_status_http`；RustCodeGraph 也给出了 `run → start_status_http → build_status_router/serve_status_loop → serve_stream → Router::handle_from` 的调用边。

crate 边界由 [`pkg/server/Cargo.toml`](Cargo.toml) 确定：包名是 `astersql-server`，Go 对照包为 `pkg/server`。本文件直接使用同 crate 的 `extract`、`http_handler`、`server`，并依赖 `astersql-server-handler`、`astersql-server-handler-tikvhandler`、`astersql-server-handler-ttlhandler`、`astersql-planner-extstore`、`astersql-metrics`、`serde_json` 和 `rustls` 等组件。

## 核心职责

1. 定义轻量 HTTP 抽象：`Method`、`Request`、`Response`、`Handler`、`Route` 与线程安全的 `Router`。
2. 用 `build_status_router` 注册完整 status 路由表面，并将请求桥接到 Server/Domain、TiKV handler、DXF handler、TTL handler、升级 handler、外部存储及指标注册表。
3. 由 `start_status_http` 创建可选 TLS 的非阻塞 `TcpListener`，在 `serve_status_loop` 中接收连接，再由 `serve_stream` 解析单个 HTTP/1.1 请求和写回响应。
4. 提供少量本地实现：`/status`、`/metrics`、`/variables/global`、plan-replayer 下载、ballast、诊断端点、server info，以及 handler 不可用时按类别返回 503。
5. 维护 Go status server 的外部契约，包括 `serve_error` 的 `X-Go-Pprof` 头和换行、profiling 请求审计字段、ballast 错误文本、若干 JSON 形状和路由名称。

## 主要符号

- `DEFAULT_STATUS_PORT: u16 = 10080`：默认 status 端口常量；实际监听使用 `ServerConfig.status`，端口为 0 时交给操作系统分配。
- `Method`：识别 GET/POST/PUT/DELETE，其余方法保存在 `Other(String)`；`Method::parse` 只做精确大写匹配。
- `Request`：保存方法、去掉 query 的路径、单值 query map、原始 query、headers 和 body。`raw_query` 专门保留 bare key、重复键和编码差异，供 TiKV handler 与 profiling 审计使用。
- `Response`：保存状态码、响应头和字节体；`new`、`text`、`json` 负责常用构造及 Content-Type。
- `Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>`：路由闭包的共享类型。
- `Router`：内部是 `Arc<Mutex<Vec<Route>>>`；`add`、`add_method`、`add_profiling` 注册路由，`mount` 挂载子路由，`handle`/`handle_from` 按注册顺序执行第一条匹配路由。
- `route_matches`：支持尾部 `/*` 前缀匹配以及分段 `{var}` 占位符；它只判断形状，不负责提取参数。
- `serve_error`：构造 Go pprof 兼容错误响应，正文追加换行，并设置 `Content-Type: text/plain; charset=utf-8` 与 `X-Go-Pprof: 1`。
- `Ballast`：在 `Mutex<Vec<u8>>` 中持有压舱内存；`new` 决定上限，`set_size` 校验并调整大小，`handler` 实现查询/设置端点。
- `Status`、`DetailStatus`、`status_response`：组成 `/status` JSON；先检查 `Server::health`，再报告连接数、crate 版本、Git hash 和当前固定为 100% 的统计初始化比例。
- `DxfHttpResponseWriter`、`dxf_request`、`dxf_response`：把本地 HTTP 请求/响应转换为 DXF handler 接口。
- `tikv_request`、`tikv_tool`、`tikv_response`：把路径、query、form、body 和方法转换为 TiKV handler 请求，使用 Domain 提供的运行时，并将动态返回值恢复成 Go 兼容 JSON。
- `TtlStatusRuntime`：实现 TTL handler 的运行时 trait。当前只对 `test_ttl.t1` 模拟成功，其他表返回不存在，因此是局部适配而非完整 Session/Domain TTL 接线。
- `build_status_router(Arc<Server>) -> Router`：本文件的路由装配中心。
- `Server::start_status_http`：生产启动入口；处理禁用、重复启动、TLS、监听器和 worker 保存。
- `build_status_tls_config`：要求证书和私钥成对；Common Name 白名单还要求 CA；最终借助 `astersql_util::security` 构造 rustls server config。
- `serve_status_loop`、`serve_stream`、`parse_target`：分别负责监听生命周期、单连接请求/响应和 request-target 拆分。

## 执行流程

1. `Server::run` 在 Domain 安装完成后调用 `start_status_http`。若 `report_status` 为 false，或已经保存了 status listener，函数直接成功返回。
2. `start_status_http` 先用 `build_status_tls_config` 校验 TLS 配置，再 bind `status.host:status.port`，设为 nonblocking，克隆 listener 保存到 `Server`，构造 `build_status_router`，最后启动名为 `astersql-status-http` 的 worker。
3. `build_status_router` 按顺序注册路由。第一组是 `/status`、`/metrics` 和 NextGen 条件下的 `/variables/global`；随后是 settings、info、pprof/debug、schema/MVCC/table/region/DDL/TiFlash/DXF/ingest/plan-replayer、standby 子路由、尚未接线的类型化占位路由、extract 和根路径。
4. `serve_status_loop` 在 `server.health() && !server.force_shutdown()` 时 accept。`WouldBlock` 休眠 10 ms；每个连接另起一个 `astersql-status-request` 线程。配置 TLS 时先创建 `rustls::ServerConnection`，否则直接处理 TCP stream。
5. `serve_stream` 最多读取 64 KiB 一次，解析请求行和 `\r\n\r\n` 后的 body，调用 `parse_target` 形成 `Request`，再交给 `Router::handle_from`。
6. `Router::handle_from` 持有路由表锁并查找首个“方法约束通过且路径匹配”的 Route；profiling 路由先调用 `log_profiling_request`，然后执行 handler。未命中返回 404。
7. 各桥接 handler 从路径分段和 query 构造下游请求，从 `Server::domain()` 获取真实运行时；运行时不存在时通常调用 `unavailable(HandlerKind)` 返回带类别的 503，而明确的资源不存在、方法错误或输入错误分别使用 404、405/400。
8. `serve_stream` 将 `Response` 写成带 Content-Length 和 `Connection: close` 的 HTTP/1.1 响应，然后关闭该连接的处理生命周期。

## 数据与状态

- 路由状态保存在 `Router.routes: Arc<Mutex<Vec<Route>>>` 中，注册顺序是行为的一部分：同形状或重复路径由先注册者遮蔽后注册者。例如具体实现先于通用 `/test/{mod}/{op}`，已接线路由也先于末尾的 unavailable 占位表面。
- `Route` 保存 pattern、可选方法、共享 handler 和 profiling 标志；路径变量在各 handler 内再次按分段解析，而非由 Router 注入。
- `Ballast.bytes` 是真实占用的字节向量。显式上限优先；上限为 0 时，Linux 从 `/proc/meminfo` 读取物理内存并取 `min(2 GiB, 总内存/4)`，其他平台或探测失败回退到 2 GiB。
- `Request.query` 对重复键只保留收集过程中的 map 结果，不能表达完整 URL query 语义；需要 Go `URL.Query().Get` 相容行为的 profiling 逻辑改读 `raw_query`，保留首值、跳过分号对和非法编码，只记录 `seconds/debug/gc` 的非空值。
- status listener 与 worker 句柄存入 `Server`，关闭行为由 `Server` 的健康/强制关闭状态及其 listener/worker 生命周期共同控制。
- `Status.version` 来自 `CARGO_PKG_VERSION`，`git_hash` 来自可选编译变量 `ASTER_GIT_HASH`；`initialized_statistics_percentage` 当前不是动态统计值。
- handler 桥接返回的数据含 `Box<dyn Any>` 风格动态值；`tikv_response` 按已知类型逐层 downcast 并序列化。未知类型降为 JSON `null`，因此扩展下游返回类型时必须同步这里。

## 依赖与调用关系

上游关系：

- `pkg/server/server.rs::Server::run` 是 `start_status_http` 的生产调用者。
- `pkg/server/http_status_test.rs` 直接调用 `build_status_router`、`Router::handle`、`Ballast::handler` 和 `serve_error`，也通过 TCP 验证 listener。
- `pkg/server/runtime.rs` 与 `pkg/server/server.rs` 提供 Domain/runtime、TLS、listener、配置和关闭状态；`pkg/server/standby.rs::StandbyController::handler` 可返回待挂载的子 `Router`。

下游关系：

- `astersql_metrics::metrics::GatherText` 生成 `/metrics`。
- `astersql_planner_extstore` 与 `astersql_util_replayer` 读取 plan-replayer zip。
- `astersql_server_handler_tikvhandler` 承担 schema、MVCC、DDL、region、table、settings、TiFlash、DXF、ingest 等大部分业务规则；本文件主要做 wire/domain 适配和 JSON 输出。
- `astersql_server_handler_ttlhandler` 与 `astersql_server_handler::upgrade_handler` 分别处理 TTL 触发与集群升级操作。
- `ExtractTaskServeHandler` 处理 `/extract_task/dump`；`TikvHandlerTool::routes` 提供尚未安装服务的占位路径列表。
- `astersql_util::security::NewTLSConfig` 和 `rustls` 负责 TLS；`astersql_util_logutil` 记录 profiling 审计事件。

RustCodeGraph 对 `build_status_router` 的 callees 同时显示大量本文件 handler 和 `Server::standby_handler`、`Server::config/domain`；对 `serve_stream` 显示下游为 `parse_target` 与 `Router::handle_from`，与源码流程一致。

## 错误处理与边界

- 启动阶段的 TLS 配置、bind、nonblocking、listener clone 和 worker spawn 错误均转换成带操作上下文的 `String` 返回给调用者。
- TLS 证书/私钥缺一不可；设置 CN 白名单但缺少 CA 会提前失败。TLS 握手配置创建失败时，请求线程不写响应便结束。
- 路由表或 ballast 的互斥锁中毒使用 `expect`，会 panic，而不是转成 HTTP 500。
- `serve_stream` 只执行一次、最多 64 KiB 的 read，不解析 Content-Length、chunked encoding，也不填充 headers；慢请求、分片 body、大请求和 keep-alive 不具备完整 HTTP server 语义。读写错误大多被静默丢弃，连接采用 `Connection: close`。
- Router 的通配语义有限：`/*` 只做字符串前缀，`{var}` 可匹配空白以外的任意同位置分段；路径参数合法性由 handler 继续检查。
- `unavailable` 明确返回 503 JSON，防止尚未接线的路径假成功。部分 handler 根据 Go 约定使用 400 而非 405；扩展时应保持相应 Go handler 的现有状态码与正文。
- `serve_error` 始终给正文追加换行；普通 `Response::text` 不会追加。plan-replayer 的方法错误是 405，缺文件是 404，存储访问错误是 500。
- `Ballast::handler` 对非法 UTF-8、非整数、负数和超上限值返回 400；与 Go switch 一致，GET/POST 之外当前返回空 200，而不是 405。
- `tikv_response` 未识别的动态数据返回 `null`；这是可观测兼容边界，不应误认为下游一定没有数据。
- `debug_pprof_response`、`debug_gogc_response`、`debug_zip_response` 是运行中 Rust 进程的有界诊断文本，不是 Go profile/trace/zip 二进制格式。

## 并发与资源生命周期

- 主 listener 使用 nonblocking accept 和 10 ms 退避；一个 status worker 管 accept，每个已接收连接新建一个 OS 线程。当前没有线程池、并发上限、读取超时或显式 backpressure，诊断端口暴露范围应由部署配置控制。
- `Arc<Server>` 使 worker 持有服务状态；循环定期观察 `health` 与 `force_shutdown`。`pkg/server/http_status_test.rs::status_listener_reports_health_and_exits_during_shutdown` 验证 `Server::close` 后 status 与 MySQL listener 都停止接收连接。
- Router clone 共享同一 `Arc<Mutex<Vec<Route>>>`；请求执行期间 `handle_from` 仍持有 routes 锁。handler 若尝试修改同一路由表可能死锁，耗时 handler 也会串行阻塞其他请求取得路由锁；新增 handler 应避免回调 Router 注册接口。
- ballast 的读取和 resize 均由独立 Mutex 串行化。resize 可能产生大额分配或释放，应受 `max_size` 与运维权限保护。
- TLS config 用 `Arc` 共享给请求线程；每个连接创建独立 `rustls::ServerConnection`。
- plan-replayer 读取和多数 Domain/handler 调用是同步的，会占用该连接线程；debug trace 刻意避免重型 backtrace，防止超过客户端超时。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/server/http_status.go`](http_status.go)。Rust 的 `DEFAULT_STATUS_PORT`、`serve_error`、`sleep_with_cancel`、`Ballast`、profiling 审计、`Status`/`DetailStatus`、路由集合和 TLS status listener 分别对应 Go 的 `defaultStatusPort`、`serveError`、`sleepWithCtx`、`Ballast`、`withProfilingRequestLog`、同名状态类型、`startHTTPServer` 与 `listenStatusHTTPServer`。

已保持的关键语义包括：ballast 默认最多 2 GiB 且参考物理内存四分之一；错误正文/头；profiling 只记录三个允许的 query；plan-replayer 下载头；status/metrics 路径；多类 TiKV 管理路径和 Go 风格 JSON 字段。

仍有明确差异：

- Go 使用 `net/http`、gorilla mux、cmux，并在 status 监听器上复用 HTTP、gRPC/channelz；Rust 是轻量单请求 HTTP/1.1 parser，本文件没有复刻 cmux/gRPC 服务。
- Go pprof、traceevent、GOGC 与 debug zip 连接 Go runtime；Rust 返回进程派生的诊断文本，格式不兼容 Go profiling 工具。
- Rust 的若干路由在 runtime 未提供时返回类型化 503；Go 通常直接构造完整 handler。文件注释也明确这些是尚未挂载服务的占位。
- `TtlStatusRuntime` 当前仅为固定测试表提供成功路径，不等价于 Go 中使用真实 session/domain 触发 TTL job 的完整实现。
- `/status` 的统计初始化百分比当前固定为 100，`/info/all` 当前按单节点生成；不能据此声称已覆盖 Go 的动态集群信息。

## 扩展指南

- 新增路由从 `build_status_router` 接入。先确认路径是否与现有模板等形，按“具体路径在通用模板之前、真实实现位于占位之前”的规则选择注册位置；仅需特定方法时使用 `add_method`。
- 新增 Domain/TiKV/DXF 功能时，优先扩展对应 handler crate 和 Domain runtime，再在本文件增加薄适配；不要把业务规则复制进路由层。缺少生产 runtime 时继续返回明确的 503/404，而不是测试替身或空成功。
- 新的路径变量需在 adapter 内显式解析并映射到下游所需键名；同时覆盖无效分段、方法、query、body 和 runtime 缺失分支。
- 下游 `ResponseWriter` 新增返回类型时，必须同步 `tikv_response` 或 `dxf_json` 的序列化分支，并核对 Go `encoding/json` 的字节/Base64、空值、字段名和数字语义。
- 修改 wire parser 前应明确是否继续维持轻量边界；若要支持 headers、分片 body、keep-alive、超时或请求大小限制，应成组设计并补独立测试，避免局部解析产生安全假设。
- TLS 或线程模型变更要同步检查 `Server` 中 listener/worker 保存与 `close`/shutdown join 路径，尤其是未完成请求取消和资源释放。
- 测试必须放在独立的 [`pkg/server/http_status_test.rs`](http_status_test.rs)，不要嵌入生产源文件。已有测试覆盖 global variables 脱敏、ballast、错误头、listener 关闭、schema/TiFlash、metrics、plan-replayer、DXF keyspace gate、profiling 日志以及 advertised info；新增行为应扩展最接近的测试。
- 若目标是与 Go 对齐，应同时阅读 Go handler 的直接依赖，而不是只按路由名推测；pprof、TTL、gRPC 和集群 info 尤其不能以当前占位/等价诊断作为完整完成证据。

## 验证依据

- Rust 源码：[`pkg/server/http_status.rs`](http_status.rs)，完整检查 1–2484 行；重点符号为 `Router`、`Ballast`、`build_status_router`、`Server::start_status_http`、`serve_status_loop`、`serve_stream`、TiKV/DXF/TTL adapters。
- crate 与模块：[`pkg/server/Cargo.toml`](Cargo.toml) 的 package、porting metadata 与依赖；[`pkg/server/lib.rs`](lib.rs) 的 `pub mod http_status` 和 `#[path = "http_status_test.rs"]` 独立测试挂载。
- Go 对照：[`pkg/server/http_status.go`](http_status.go) 的 `startStatusHTTP`、`listenStatusHTTPServer`、`serveError`、`sleepWithCtx`、`Ballast`、`withProfilingRequestLog`、`startHTTPServer`、`startStatusServerAndRPCServer`、`Status` 与 `handleStatus`。
- Rust 测试：[`pkg/server/http_status_test.rs`](http_status_test.rs) 中 12 个 `#[test]`，覆盖上述主要兼容和生命周期边界；本任务为纯文档分析，按计划未运行 Cargo。
- RustCodeGraph：索引状态为 11,467 files / 307,296 nodes / 1,848,419 edges；查询了目标文件、`build_status_router`、`start_status_http`、`serve_stream`、`status_response` 的 node/callers/callees。图确认 `Server::run` 是启动上游、`build_status_router` 是路由装配入口、`serve_status_loop` 调用 `serve_stream`，而 `serve_stream` 调用 `parse_target` 与 `Router::handle_from`。
- 文档结构按任务要求用固定 11 个二级标题验证；内容人工复核区分了当前实现、占位能力、Go 差异与未提供的完整 HTTP/runtime 语义。
