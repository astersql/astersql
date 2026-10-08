# `pkg/server/handler/optimizor/optimize_trace.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-server-handler-optimizor`，由同目录 `lib.rs` 以 `pub mod optimize_trace` 暴露。它是 Optimize Trace 下载端点的参数适配层：把 HTTP 路由中的文件名、Optimizer Trace 目录、本机地址和 status 端口整理成 `DownloadFileRequest`，再交给抽象运行时执行下载。文件本身不读取文件、不访问集群拓扑，也不直接写 HTTP 响应。

在 Go 应用中，对应路由是 `/optimize_trace/dump/{filename}`（`pkg/server/http_status.go`），并由 `pkg/server/handler/optimizor/optimize_trace.go` 的 handler 调用共用下载逻辑。当前 Rust 应用层尚未完成同等接线：`pkg/server/http_status.rs` 仍将该路由注册为 `HandlerKind::Optimizer` 的 unavailable 占位，因此不能把本文件的存在理解为 Rust status server 已经对外提供 Optimize Trace 下载。

## 核心职责

- `OptimizeTraceHandler` 保存构造下载请求所需的节点地址和 status 端口。
- `ServeHTTP` 从 `OptimizeTraceRuntime` 获取路由文件名、trace 目录和内部 HTTP scheme，构造一个完整的 `DownloadFileRequest`。
- `join_clean` 模拟本场景所需的 Go `filepath.Join` 清理语义，尤其保证以 `/` 开头的路由名不会替换既定 trace 目录。
- 下载失败时，`ServeHTTP` 把错误交给运行时的 `write_error`，使协议层自行决定 HTTP 错误格式。

本文件刻意把环境相关行为放在 trait 后面，便于独立测试请求组装；真正的本地文件读取、跨 TiDB 节点转发、ZIP 响应头和 404 处理不在本文件中实现。

## 主要符号

- `DownloadFileRequest`：下载执行所需的值对象。`file_path` 是清理后的本地/外部存储路径；`file_name` 保留原始路由值；`address`、`status_port` 标识本节点；`url_path` 用于远端转发；`downloaded_filename` 决定附件基础名；`scheme` 决定内部 HTTP 协议。
- `OptimizeTraceRuntime`：`ServeHTTP` 的运行时边界。关联类型 `Error` 对错误类型不作约束；三个只读方法提供路由名、目录和 scheme；`download_file` 执行下载；`write_error` 把失败写回调用环境。
- `OptimizeTraceHandler { address, status_port }`：不持有请求、文件或网络资源的轻量配置对象。字段公开，构造入口同时提供 Go 风格的 `NewOptimizeTraceHandler`。
- `NewOptimizeTraceHandler(address, status_port)`：仅保存两个参数，不做地址解析、端口校验或 I/O。
- `OptimizeTraceHandler::ServeHTTP(runtime)`：文件的主要公开行为，完成一次请求参数组装和一次下载调用。
- `serve_http(handler, runtime)`：蛇形命名的包级转发入口，不增加逻辑。
- `join_clean(directory, file_name)`：私有路径归一化函数，逐个处理 `Component`；跳过 `.`，遇到 `..` 弹出一层，普通片段追加，忽略后续根分隔符，Windows 前缀则追加其原始表示。

## 执行流程

1. 上层构造 `OptimizeTraceHandler`，传入当前节点地址和 status 端口。
2. `ServeHTTP` 调用 `runtime.route_file_name()`，得到路由中的原始文件名并保留该字符串。
3. 它调用 `runtime.optimizer_trace_directory()`，再通过 `join_clean` 生成 `file_path`。路径清理只影响文件定位字段，不会改写 `file_name`。
4. 它填入节点地址、端口、`runtime.internal_http_scheme()`、固定附件名 `optimize_trace`，并用原始文件名形成 `optimize_trace/dump/{file_name}`。
5. `runtime.download_file(request)` 取得请求所有权并执行后续工作。成功时直接结束；失败时调用一次 `runtime.write_error(error)`，错误不再向调用者返回。
6. 若使用包级 `serve_http`，该函数只是调用上述方法，流程完全相同。

例如路由名为 `/trace.zip`、目录为 `/tmp/optimizer` 时，`file_path` 为 `/tmp/optimizer/trace.zip`，但 `file_name` 仍是 `/trace.zip`，所以 `url_path` 会保留为 `optimize_trace/dump//trace.zip`。这是独立测试明确锁定的当前行为。

## 数据与状态

`OptimizeTraceHandler` 的持久状态只有两个拥有所有权的标量：`String` 地址和 `u16` 端口。每次调用都会克隆地址，并新建 `DownloadFileRequest`；handler 自身不会被修改，因此可重复使用。

`DownloadFileRequest` 同时保留“清理后的存储路径”和“未经清理的路由名”。这个区分是重要不变量：前者服务于文件查找，后者服务于转发 URL 和诊断语义。`downloaded_filename` 固定为 `optimize_trace`，与 Go 共用下载器设置 `Content-Disposition: ... optimize_trace.zip` 的意图一致。

本文件没有全局变量、缓存、拓扑列表、响应缓冲区或可变共享状态。所有一次请求相关的副作用均由传入的 `&mut R` 保存和执行。

## 依赖与调用关系

直接标准库依赖只有 `std::path::{Path, PathBuf}`。该文件没有直接使用 `Cargo.toml` 中的大部分 crate 依赖；那些依赖由同一 `astersql-server-handler-optimizor` 包的 Plan Replayer 和统计 handler 使用。Cargo manifest 没有为本模块声明 feature 开关。

RustCodeGraph 将目标文件列为被 `optimize_trace_test.rs` 和 `plan_replayer.rs` 关联使用；精确名称搜索能定位 `OptimizeTraceRuntime`、`OptimizeTraceHandler`、`NewOptimizeTraceHandler` 和包级 `serve_http`，但没有给出从 Rust status server 到 `ServeHTTP` 的调用边。原始引用搜索进一步确认，生产 Rust 代码中没有 `OptimizeTraceRuntime` 实现，也没有调用目标 handler；`pkg/server/http_status.rs` 的实际路由是 503 占位。

设计上，`download_file` 对应 Go 的共用 `handleDownloadFile` 边界。Rust 的共用下载实现目前位于相邻 `plan_replayer.rs::handleDownloadFile`，包含本地读取、远端拓扑转发、ZIP 响应和 404 行为，但它接收不同的 `PlanReplayerRuntime`，本文件没有调用它。因此二者是语义上的相邻实现，不是当前 Rust 调用链上的直接下游。

## 错误处理与边界

`ServeHTTP` 只处理 `download_file` 返回的错误：它把错误移动给 `write_error`，不返回 `Result`，也不重试。获取路由名、目录和 scheme 的 trait 方法本身不能失败；若这些操作需要失败语义，运行时接口必须扩展。

本文件不校验空文件名、地址格式、端口、scheme，也不限制文件名字符。`join_clean` 会处理 `.` 和 `..`，但它不是安全沙箱：连续的父目录组件可以反复 `pop`，从而把结果移出最初的 trace 目录。绝对文件名的根分隔符被忽略，这是为了匹配现有 Go 对照和测试，而不是完整的路径授权策略。接入真实 HTTP 前，应由路由层/存储层确定允许的文件名范围并增加穿越边界回归测试。

`Component::Prefix` 分支面向 Windows 路径；当前实现把前缀追加到已有路径。仓库测试只验证了 Unix 根路径名，没有为 Windows 前缀、空路径、多层 `..` 或下载失败后的 `write_error` 次数提供证据。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。`ServeHTTP` 借用 `&self` 和独占借用 `&mut runtime`：同一次调用期间，运行时不能被其他安全 Rust 代码并发修改；是否能让同一 handler 跨线程共享则取决于外层容器和运行时，类型本身没有声明额外的 `Send`/`Sync` 约束。

请求对象在栈上构造，随后按值移入 `download_file`；本文件不保留其引用。文件句柄、HTTP 响应体和远端连接若存在，均必须由运行时在 `download_file` 内负责关闭或释放。相比之下，Go 共用下载器会对本地 reader 和远端 response body 执行关闭；这只能作为未来 Rust runtime 实现的资源管理要求，不能算作本文件当前已实现的行为。

## 与 Go 版本的对应关系

Rust `OptimizeTraceHandler` 对应 Go 同名结构，但当前只保留 `address` 和 `statusPort`；Go 结构还持有 `*infosync.InfoSyncer`，供共用下载器发现其他 TiDB 节点。Rust 构造函数因此也少一个 `infoGetter` 参数。

两端 `ServeHTTP` 的组装字段基本一一对应：Go 的 `domain.GetOptimizerTraceDirName()` 对应 `optimizer_trace_directory()`，`util.InternalHTTPSchema()` 对应 `internal_http_scheme()`，`filepath.Join(..., name)` 对应 `join_clean`，URL 路径和附件名保持相同。Rust 用 `DownloadFileRequest` 与 trait 隔离 HTTP/存储细节，而 Go 直接构造 `downloadFileHandler` 并调用 `handleDownloadFile`。

Go 共用下载器已经实现外部存储查询、本地流式返回、跨节点转发、404、日志和响应头；Rust 目标文件只委托 `download_file`。相邻 Rust `plan_replayer.rs` 有一套内存化的同类算法，但目标文件和 status 路由尚未把它接起来。因此迁移状态是“请求组装和路径兼容逻辑已有独立实现与测试，应用级下载链尚未接线”。

## 扩展指南

- 接入真实 status server 时，优先新增协议层 runtime 适配器并在 `pkg/server/http_status.rs` 替换 Optimizer 占位路由；不要把 socket、拓扑或存储细节塞回本文件。接线需要覆盖本地命中、转发命中、转发防递归、全节点未命中和错误响应。
- 若复用 `plan_replayer.rs::handleDownloadFile`，应先统一请求结构和 runtime 能力，避免维护两套字段相同但类型不兼容的下载模型；同时保持 Optimize Trace 不触发 capture-replayer 历史统计改写。
- 修改路径规则时应改 `join_clean`，并同步独立测试 `pkg/server/handler/optimizor/optimize_trace_test.rs`。至少补充空文件名、`.`、`..`、多层父目录、绝对路径和目标平台前缀用例；路径越界策略必须先明确，不能仅以“与 `filepath.Join` 相似”替代安全约束。
- 修改请求字段时应同步 `DownloadFileRequest`、`OptimizeTraceRuntime::download_file` 的实现者以及 Go 对照字段，并检查转发 URL 是否仍使用原始路由名。
- 修改失败策略时应为 `download_file` 返回错误的场景增加独立测试，确认 `write_error` 的调用次数和是否允许继续写响应。
- Rust 单元测试继续保留在同目录独立 `*_test.rs` 文件，由 `lib.rs` 的 `#[cfg(test)] mod optimize_trace_test` 装配，不要内嵌到生产文件。

## 验证依据

- 生产源码：`pkg/server/handler/optimizor/optimize_trace.rs`，核对了 1 个请求结构、1 个 runtime trait、1 个 handler、构造函数、方法入口、包级转发函数和私有路径函数。
- Crate 边界：`pkg/server/handler/optimizor/Cargo.toml` 与 `lib.rs`，确认包名、模块公开方式、测试模块装配及无本模块 feature 条件。
- RustCodeGraph：`status` 显示索引可用；`files --filter pkg/server/handler/optimizor` 确认目标、Go 对照和独立测试都已索引；文件级 `node` 显示源码全貌及关联文件；精确 `query OptimizeTrace` / `query OptimizeTracer` 确认主要符号候选；`callers/callees` 未发现目标 handler 的生产调用边，随后用仓库引用搜索核实未接线事实。
- Rust 测试：`pkg/server/handler/optimizor/optimize_trace_test.rs::filepath_join_keeps_directory_for_rooted_route_name`，验证根路径形式的路由名在 `file_path` 中仍位于配置目录下，同时原始名字和双斜杠 URL 被保留。
- 相邻下载测试：`pkg/server/handler/optimizor/plan_replayer_test.rs::shared_download_handler_only_rewrites_plan_replayer_capture_files`，证明 Optimize Trace 类型的请求不应触发 Plan Replayer capture 内容改写；其他相邻用例提供转发 URL、404、IPv6 和响应写失败语义的参考，但不等于目标 handler 已接入这些能力。
- Go 对照：`pkg/server/handler/optimizor/optimize_trace.go`、`plan_replayer.go::handleDownloadFile`、`pkg/server/http_handler.go::newOptimizeTraceHandler` 和 `pkg/server/http_status.go`，核对字段映射、共享下载流程和真实路由入口。
- Rust 应用入口：`pkg/server/http_status.rs`，确认 `/optimize_trace/dump/{filename}` 当前注册为 Optimizer unavailable 占位；`pkg/server/http_handler.rs::new_optimize_trace_handler` 是另一轻量数据结构的构造函数，并未调用本文件的 handler。
- 本任务是纯文档分析，按计划不运行 Cargo；结构验证另行执行并记录退出状态。
