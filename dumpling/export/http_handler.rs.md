# `dumpling/export/http_handler.rs`

## 文件定位

`http_handler.rs` 属于 `astersql-dumpling-export` library crate；crate 边界由 `dumpling/export/Cargo.toml` 声明，`dumpling/export/lib.rs` 通过 `include!("http_handler.rs")` 将它并入与 Go `dumpling/export` 包相似的单包符号空间。它位于导出器的运行期基础设施层，给 Dumpling 暴露状态、Prometheus 文本和调试端点，不参与数据扫描或文件写入。

应用主链是 `NewDumper` 引用初始化步骤 `dump.rs::startHTTPService`，后者在 `StatusAddr` 非空时调用本文件的 `startDumplingServiceWithDumper`，再由后台线程进入 `startHTTPServer`。因此，这个文件是 Dumper 初始化与状态/指标观测面之间的桥梁，而不是通用 TiDB HTTP server。

当前文件是真实绑定 TCP 端口的可运行实现，但依赖 `stubs.rs` 中的本地 `Registry`、`HttpServiceHandle` 等轻量边界；这些桩的能力不等同于完整 Prometheus 或生产 HTTP 框架。依据：`dumpling/export/lib.rs`、`dumpling/export/Cargo.toml`、`dumpling/export/dump.rs::startHTTPService`、`dumpling/export/stubs.rs`。

## 核心职责

- `startDumplingServiceWithDumper` 绑定监听地址，构造可共享停止状态的 `HttpServiceHandle`，选择 Dumper 自带或进程默认的指标注册表，并启动服务线程。
- `startHTTPServer` 轮询非阻塞 listener，同时观察 `HttpServiceHandle::stopped` 和 `tcontext::Context::Done`，接受连接或结束服务。
- `serveHTTPConnection` 解析一次 HTTP/1 请求的路径，分派 `/status`、`/metrics` 和 `/debug/pprof/*`，组装并写回一个关闭连接的响应。
- `metricsHandler` 从传入的 `Registry` 抓取实时文本；仅支持注册、不支持抓取的实现会回退到 `DefaultGatherer`。
- `isErrNetClosing_pub` 暴露与 Go `isErrNetClosing` 一致的空值/错误文本判定面，主要用于兼容和测试。

它不负责刷新状态。`/status` 只克隆 `Dumper.status` 中已经生成的 `DumpStatus` 快照并调用 `DumpStatus::toJSON`；快照由 `status.rs::RefreshStatus` 及状态循环维护，所以 HTTP 轮询不会推进速度采样窗口。依据：`serveHTTPConnection`、`status.rs::{RefreshStatus, GetStatus, DumpStatus::toJSON}` 及 `http_handler_test.rs::http_status_reports_progress_and_polling_preserves_speed`。

## 主要符号

- `cmuxReadTimeout: Duration`：10 秒连接读超时。名称沿用 Go 的 cmux 配置，但 Rust 没有创建 cmux，而是把该值设置到每个 `TcpStream`。
- `useOfClosedErrMsg: &str`：兼容 Go 网络关闭错误判断的稳定子串。
- `startHTTPServer(&Context, TcpListener, &HttpServiceHandle, Option<Arc<Mutex<DumpStatus>>>, Arc<dyn Registry>)`：公开服务循环。Dumper 可选，因此状态快照也可选；指标注册表始终存在。
- `serveHTTPConnection(TcpStream, &Context, Option<&Arc<Mutex<DumpStatus>>>, &dyn Registry)`：私有的单连接处理器；最多读取 8192 字节一次，只从请求首行提取第二个空白分隔字段作为路径。
- `metricsHandler(&dyn Registry) -> String`：公开的指标文本生成函数。优先使用传入注册表的 `Gather()`，返回 `None` 时才抓取全局 `DefaultGatherer`。
- `startDumplingService(&Context, &str) -> Result<HttpServiceHandle>`：无 Dumper 的便捷入口；可用于只启动基础路由，此时 `/status` 返回 503。
- `startDumplingServiceWithDumper(&Context, &str, Option<&Dumper>) -> Result<HttpServiceHandle>`：完整入口；成功即返回句柄，服务在线程中继续运行。
- `isErrNetClosing_pub(Option<&Error>) -> bool`：`None` 为 false；否则判断错误消息是否包含 `useOfClosedErrMsg`。

文件自身没有类型、trait 或条件编译项；其可见类型来自 `lib.rs` 的 crate 级导入和 `pub use stubs::*`。依据：`http_handler.rs` 全文件、`lib.rs`、`stubs.rs::{Registry, DefaultRegistry, HttpServiceHandle}`。

## 执行流程

1. `dump.rs::startHTTPService` 检查 `Config::StatusAddr`；空地址直接跳过，非空地址把当前 `Dumper` 传入 `startDumplingServiceWithDumper`。
2. 完整入口调用 `TcpListener::bind`，再读取实际 `local_addr`。两个失败点都转换为本 crate 的 `Error` 并附加 `start listening` 上下文；这也支持测试使用 `127.0.0.1:0` 获取系统分配端口。
3. 入口创建 `HttpServiceHandle`：保存实际地址，`started=true`、`stopped=false`；若有 Dumper，则克隆 `d.status` 和 `d.conf.PromRegistry`，否则使用无状态快照和 `DefaultGatherer()`。
4. 克隆 context 与句柄后启动 detached 线程；线程把 listener 设为 nonblocking。设置失败会记录 info 并返回。
5. 服务循环先检查 `stopped` 原子标志，再检查 context 取消；接受成功后在同一服务线程同步调用 `serveHTTPConnection`。`WouldBlock` 时睡眠 5 ms 后继续，其他 accept 错误在排除关闭错误后记录并退出。
6. 单连接处理器恢复阻塞模式、设置 10 秒读超时、做一次最多 8192 字节的读取，并从首行解析路径；查询串在路由匹配前被去掉。
7. 路由结果为状态行、Content-Type 和字符串 body：`/status` 返回 503 或序列化快照；`/metrics` 现场 gather；pprof 索引返回静态链接，其他 pprof 端点返回空 body；未知路径返回 404。
8. 函数写入带 `Content-Length` 和 `Connection: close` 的 HTTP/1.1 响应。只有 `/status` 的写失败会记 warn，然后连接随 `TcpStream` 析构关闭。

依据：`dump.rs::startHTTPService`、`startDumplingServiceWithDumper`、`startHTTPServer`、`serveHTTPConnection`。

## 数据与状态

服务句柄的 `addr` 是绑定后的实际 socket 地址；`started` 和 `stopped` 都是 `Arc<AtomicBool>`。本文件创建时把 `started` 设为 true，但不会在服务线程退出时复位；`HttpServiceHandle::stop` 只以 `SeqCst` 写入 `stopped=true`。调用方因此应把 `stopped` 视为停止请求，而不能把 `started` 当作线程存活探针。依据：`startDumplingServiceWithDumper` 与 `stubs.rs::HttpServiceHandle`。

状态数据是 `Arc<Mutex<DumpStatus>>`。启动时从 `Dumper.status` 克隆 Arc，共享同一快照槽；请求时在锁内克隆 `DumpStatus`，随后对克隆值序列化，因此不会在写 socket 时长期持锁。`DumpStatus::toJSON` 保留 Go JSON 字段名，在进度尚未就绪时省略 `progress` 和 `progressPercent`。

注册表以 `Arc<dyn Registry>` 跨线程共享。`Registry` 要求 `Send + Sync`；`DefaultRegistry::Gather` 会克隆样本闭包、按名字排序并在锁外执行它们。每次 `/metrics` 请求重新 gather，所以注册后的指标更新能立即反映；测试 `http_metrics_serve_live_configured_registry_and_preserve_custom_families` 锁定了这一点。

请求缓冲区和响应 body 都是单连接局部值，不跨请求缓存。文件没有全局可变状态；唯一进程级共享对象是 `stubs.rs::DefaultGatherer` 内部的 `OnceLock<Arc<DefaultRegistry>>`。

## 依赖与调用关系

上游调用链经 RustCodeGraph 验证为：

`NewDumper` → `dump.rs::startHTTPService` → `startDumplingServiceWithDumper` → `startHTTPServer` → `serveHTTPConnection`。

关键下游关系为：

- `serveHTTPConnection` → `status.rs::DumpStatus::toJSON`，生成 `/status` JSON；快照生产侧是 `Dumper::RefreshStatus`。
- `serveHTTPConnection` → `metricsHandler` → `Registry::Gather`，必要时 → `DefaultGatherer::Gather`。
- `startHTTPServer` → `tcontext::Context::{Done, L}`，分别用于取消和日志。
- `startDumplingServiceWithDumper` → `std::net::TcpListener`、`std::thread::spawn`、`Dumper::{status, conf.PromRegistry}`。
- `lib.rs` 的 crate 级 `Arc`、`Mutex`、`Duration`、`Field`、`tcontext` 以及 `pub use stubs::*` 给 include 文件提供名字；本文件本身没有 `use` 列表。

`Cargo.toml` 没有 HTTP/Prometheus 第三方依赖；HTTP 解析与响应由标准库手写，指标抽象来自 `stubs.rs`。这与该 crate 的注释一致：外部重依赖由本地轻量边界代替。

## 错误处理与边界

- 绑定或读取本地地址失败会返回带 `start listening` 上下文的 `Error`。`dump.rs::startHTTPService` 对非法地址继续返回错误，对端口占用等错误只记 warn 并允许 Dumper 初始化继续；这是上游接线策略，不是本文件吞错。
- listener 设为 nonblocking 失败只记录 info，线程结束，而创建函数已经返回成功句柄；调用方无法通过返回值获知该异步失败。
- `WouldBlock` 是正常轮询分支；其他 accept 错误若不含关闭子串则记录，随后一律退出。Rust 没有 Go 的 `http.ErrServerClosed` 独立判断。
- 设置 stream 阻塞模式和读超时的错误被忽略；读失败或超时静默关闭连接。请求为空、首行畸形或没有路径时按 `/` 处理并返回 404。
- 解析器不校验 HTTP method/version/header，不支持跨多次 read 拼接请求，也不处理大于 8192 字节或慢速分段首行；它只适合这里固定的本地观测端点。
- `snapshot.lock().unwrap()` 在 mutex poisoned 时会 panic；`toJSON` 遇到非有限浮点数时记录 warn，但仍返回 `200 OK` 和空 body，因为响应状态在当前实现中没有切换到 5xx。
- 指标 `Gather()` 用 `Option<String>` 表达是否具备抓取能力，而非表达抓取错误；可抓取注册表返回空字符串也不会回退。
- 写响应失败仅 `/status` 记录 warn；其他路由静默结束。不存在重试或部分写恢复逻辑。

这些边界由 `http_handler.rs`、`status.rs::DumpStatus::toJSON`、`stubs.rs::Registry` 直接给出；当前独立测试覆盖监听失败、503、关闭错误子串、实时指标和状态快照，但没有覆盖畸形/超长请求、锁中毒和异步线程启动失败。

## 并发与资源生命周期

每次启动创建一个 detached OS 线程并把 listener 所有权移入线程；没有保存 `JoinHandle`，所以 `HttpServiceHandle::stop` 不能等待线程完全退出。空闲时线程每 5 ms 检查一次停止标志和 context；正常停止延迟通常由这个轮询间隔决定。

连接在服务线程中串行处理，不会为每个连接另开线程。某个客户端在 accept 后最多可让该线程阻塞到 10 秒读超时；在此期间停止标志与 context 不会被检查，其他客户端也不能被 accept。这是与 Go `net/http` 并发连接模型的重要差异和潜在可用性风险。

状态 Arc、注册表 Arc、context 克隆和句柄克隆保证服务线程所需对象在线程结束前存活。单连接 stream 在函数返回时关闭；响应明确声明 `Connection: close`，不支持 keep-alive 或连接复用。`DumpStatus` 锁只覆盖 clone；指标注册表自己的锁与回调执行策略由 `stubs.rs::DefaultRegistry` 管理。

测试通过八个 scoped 线程并发轮询同一 `/status` 地址，验证响应快照一致且不会改变 `SpeedRecorder`；这验证共享快照的读语义，但由于服务端仍串行处理，不能据此宣称服务端并发执行请求。依据：`http_handler_test.rs::http_status_reports_progress_and_polling_preserves_speed`。

## 与 Go 版本的对应关系

共同点：两版都暴露 `/metrics`、`/status` 和五个 `/debug/pprof/*` 路由；监听错误带 `start listening` 上下文；状态来自 Dumper；指标优先使用配置注册表，并在注册表不能 gather 时使用默认 gatherer；网络关闭错误以稳定子串识别。Rust 独立测试与 Go `http_handler_test.go` 都锁定了进度字段、未就绪时省略进度、轮询不更新速度，以及只服务配置注册表的指标族。

主要差异：

- Go `startDumplingService` 用 cmux 匹配 HTTP/1 并阻塞在 `m.Serve()`；Rust 直接监听 TCP、返回 `HttpServiceHandle` 并在内部创建线程，没有协议复用。
- Go 使用 `net/http.ServeMux`/`http.Server` 并由标准库处理请求和连接并发；Rust 是一次读取、手写路由、单线程串行服务。
- Go pprof 路由接入真实 `net/http/pprof`；Rust 只有静态索引、固定 cmdline 和空的 profile/symbol/trace body，不能视为真实 profiling 支持。
- Go `/status` 使用 `json.Encoder` 写 `d.GetStatus()`；Rust直接克隆共享快照并调用手写 `toJSON`。两者都只记录编码/写入失败，但 Rust 的空 Dumper 入口额外定义了 503 行为。
- Go 的关闭由 listener/cmux 生命周期驱动并过滤 `http.ErrServerClosed`；Rust 通过原子停止标志轮询，公开句柄但不 join。
- Go `startHTTPService` 自己开 goroutine并总是返回 nil；Rust 的完整入口开线程并返回句柄，上游 `startHTTPService` 将句柄存入 `Dumper.http`，还对非法地址与其他监听失败作不同处理。

因此当前 Rust 版对观测数据契约有较强对齐，但 HTTP 协议、pprof 能力和并发模型是明确的简化实现。依据：`dumpling/export/http_handler.go`、`dump.go::startHTTPService`、两侧 `http_handler_test`。

## 扩展指南

- 新增路由应修改 `serveHTTPConnection` 的路径分派，并在独立的 `dumpling/export/http_handler_test.rs` 增加真实 socket 回归；不要把测试写进生产文件。还应核对 Go `http_handler.go` 是否需要同步，以免两版公开观测面漂移。
- 改动 `/status` 字段时，应优先修改 `status.rs::DumpStatus`、`toJSON` 和快照生产逻辑，再同步 Rust `http_handler_test.rs`、`status_test.rs` 以及 Go `status.go`/`http_handler_test.go`。保持 HTTP 读取只读快照，避免轮询改变速度或进度状态。
- 改动 `/metrics` 时要保留“配置 registry 优先、仅 registerer 才回退默认 gatherer”的不变量，并覆盖自定义同名 metric family、实时更新和注销后的行为。相关入口是 `metricsHandler`、`stubs.rs::Registry` 与 `metrics.rs` 的注册/注销逻辑。
- 若要补齐生产级 HTTP，最可能重构 `startHTTPServer`/`serveHTTPConnection`：需要明确 method、请求大小、分段读取、并发、keep-alive、优雅关闭和 join 语义，并为慢客户端、停止期间活跃连接、畸形请求增加测试。单纯增加路由不能解决串行阻塞风险。
- 若要补齐 pprof，不能继续返回占位空 body；应引入或接线真实 profiling 实现，并先评估 `Cargo.toml` 依赖、平台支持、安全暴露和性能成本。
- 调整生命周期时需同时修改 `stubs.rs::HttpServiceHandle` 和 `dump.rs::startHTTPService`/Dumper 关闭路径；`started` 当前不是健康状态，改变其含义可能影响调用方兼容性。
- 保持 `// Copyright 2026 AsterSQL.` 与原 PingCAP Apache 版权行，不要因文档化或重构删除。

## 验证依据

- RustCodeGraph 索引状态：项目包含 7032 个 Rust 文件；`node --file dumpling/export/http_handler.rs` 读取了完整 149 行，并显示直接使用方 `dumpling/export/dump.rs`。
- RustCodeGraph 精确查询/探索：查询了 `HttpServiceHandle`、`Registry`、`DefaultRegistry`、`DumpStatus`、`startHTTPService`；调用流确认 `NewDumper → startHTTPService → startDumplingServiceWithDumper → startHTTPServer → serveHTTPConnection → {DumpStatus::toJSON, metricsHandler}`。
- 已读 Rust 生产路径：`dumpling/export/http_handler.rs`、`lib.rs`、`dump.rs::startHTTPService`、`status.rs::{GetStatus, RefreshStatus, DumpStatus::toJSON}`、`stubs.rs::{Registry, DefaultRegistry, DefaultGatherer, HttpServiceHandle}`。
- 已读 crate 声明：`dumpling/export/Cargo.toml`，确认 package 为 `astersql-dumpling-export`、lib 入口为 `lib.rs`，HTTP/指标边界由本地 stub 与标准库承担。
- 已读 Rust 独立测试：`dumpling/export/http_handler_test.rs`，覆盖路由、监听失败、关闭错误匹配、503、状态快照/并发轮询、配置指标注册表、默认 gatherer 回退。
- 已读 Go 对照：`dumpling/export/http_handler.go`、`dumpling/export/dump.go::startHTTPService`、`dumpling/export/http_handler_test.go`，用于核对路由、cmux、状态与指标契约及迁移差异。
- 按任务约束未运行 Cargo；交付验证仅执行任务指定的 11 章节结构命令，并人工复查本文能够回答文件为何存在、如何运行、如何安全扩展。
