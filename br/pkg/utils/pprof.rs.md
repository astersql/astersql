# `br/pkg/utils/pprof.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-utils`（见 `br/pkg/utils/Cargo.toml`），由 `br/pkg/utils/lib.rs` 以 `pub mod pprof` 暴露。它不是业务备份或恢复算法，而是 BR 进程的运维可观测性边界：绑定 status 地址、可选地套用 TLS、注册 metrics/pprof HTTP 路由，并在后台线程接受连接。固定地址启动链为 `br/cmd/br/cmd.rs::startStatusServer` → `StartStatusListenerWithHandler`；Unix 动态启动链为 `br/pkg/utils/dyn_pprof_unix.rs::StartDynamicPProfListener` → `StartStatusListener`。

## 核心职责

- `listen` 以进程级单例状态保护监听器创建，拒绝重复启动，并记录操作系统最终分配的地址。
- `RegisterDefaultStatusHandlers` 构造默认 `StatusHandler`，提供 Prometheus `/metrics`、命令行、CPU profile、symbol 和 trace 兼容端点。
- `StartStatusListenerWithHandler` 将 TCP 监听器包装成 `astersql_util::security::Listener`，把接受循环放到后台线程；`StartStatusListener` 在其上组合默认 handler。
- `serve_listener` 与 `handle_connection` 实现轻量 HTTP/1.x 服务。它只满足 status/pprof 端点需要，并非通用 HTTP 服务器。

## 主要符号

- `STARTED_PPROF: OnceLock<Mutex<String>>` 与 `started_pprof()`：延迟创建、进程内共享的“当前已绑定地址”。空字符串表示允许启动，非空表示已有监听器。
- `StatusRequest { method, target, body }`：handler 输入。`target` 保留查询串，`body` 保存当前单次读取中请求头后的字节。
- `StatusResponse { status, content_type, body }`：handler 输出；私有构造器 `ok` 和 `error` 统一成功/文本错误响应。
- `StatusHandler = Arc<dyn Fn(&StatusRequest) -> Option<StatusResponse> + Send + Sync>`：可在线程间共享的路由函数；`None` 表示未匹配，由连接层转为 404。
- `RegisterDefaultStatusHandlers() -> StatusHandler`：公开默认路由工厂。
- `StartStatusListenerWithHandler(&str, Option<&TLS>, StatusHandler) -> Result<(), SharedError>`：公开的可注入 handler 启动入口。
- `StartStatusListener(&str, Option<&TLS>) -> Result<(), SharedError>`：公开便捷入口，自动安装默认 handler。
- 私有辅助：`query_seconds`、`cpu_profile`、`wrap_listener`、`serve_listener`、`handle_connection`、`failed_to_connect`。其中 `inject_failpoint_determined_port` 目前为空函数，`failed_to_connect` 也未被启动路径调用。

## 执行流程

1. `br/cmd/br/cmd.rs::startStatusServer` 先生成默认路由并叠加命令注册器；显式配置 `status-addr` 时调用 `StartStatusListenerWithHandler`。未配置时，Unix 信号监听器会在收到启动信号后调用 `StartStatusListener`。
2. `listen` 持有 `STARTED_PPROF` 互斥锁检查重复启动，调用空的 failpoint 钩子后执行 `TcpListener::bind`。绑定成功后以 `local_addr()` 写回真实地址，因此 `:0` 也能得到实际端口。
3. `wrap_listener` 在存在 `TLS` 时调用 `TLS::WrapListener`，否则创建 `Listener::Plain`。随后 `StartStatusListenerWithHandler` 克隆地址字符串并启动后台 serve 线程，函数本身立即返回。
4. `serve_listener` 无限执行 `Listener::accept`。每接受一个 `Connection`，克隆 `Arc` handler，并再启动一个连接线程，避免慢连接阻塞后续 accept。
5. `handle_connection` 最多读取 65536 字节一次，解析请求行和 `\r\n\r\n` 后的请求体。handler 返回 `None` 时生成 404；随后写出状态行、内容类型、长度及 `Connection: close`。HEAD 请求只写响应头。
6. 默认 handler 先去掉查询串再匹配路径。`/metrics` 编码默认 Prometheus registry；`/debug/pprof/profile` 由 `query_seconds` 把采样秒数限制在 1–300 秒，`cpu_profile` 以 100 Hz 采样并用 prost 编码 pprof protobuf；其余端点按各自兼容语义返回。

## 数据与状态

唯一持久的模块级可变状态是 `STARTED_PPROF` 内的地址字符串。其更新不变量是：成功绑定后才从空值变为实际地址；重复启动在持锁期间被拒绝；接受循环报错退出后由 serve 线程清空，使后续启动可重试。单个请求与响应均为拥有所有权的值，连接线程共享的是不可变 `Arc<StatusHandler>`。

CPU profile 的持续时间来自 URL `seconds` 参数：缺失、非数字或找不到该键时使用默认 30 秒，最终统一 `clamp(1, 300)`。profile 内容是 protobuf 字节；metrics 和命令行等端点是文本或原始字节。`StatusResponse.status` 与 `content_type` 使用静态字符串，body 动态分配。

## 依赖与调用关系

上游直接证据包括：`br/pkg/utils/lib.rs` 声明 `pub mod pprof` 并把 `pprof_test.rs` 作为独立测试模块；`br/cmd/br/cmd.rs::startStatusServer` 调用 `RegisterDefaultStatusHandlers` 和 `StartStatusListenerWithHandler`；`br/pkg/utils/dyn_pprof_unix.rs` 调用 `StartStatusListener`。RustCodeGraph 的精确 `query` 也将三个公开函数定位到本文件第 163、249、277 行，并将 `serve_listener`、`handle_connection` 定位到第 282、295 行。

下游依赖由 `br/pkg/utils/Cargo.toml` 核验：`prometheus = 0.14` 提供默认 registry 与文本编码；`pprof = 0.15` 启用 `prost-codec` 生成 profile；`prost = 0.12` 负责 protobuf 编码；`astersql-util` 提供 `security::{Connection, Listener, TLS}`；BR errors/logutil 与 `astersql-errors` 提供错误分类、包装和结构化日志。标准库提供 TCP、锁、线程、读写与持续时间。

## 错误处理与边界

- 重复启动记录警告，并以 `ErrUnknown` 为根因添加当前地址上下文；锁中毒处使用 `expect`，因此锁持有者 panic 会令后续访问 panic。
- bind 失败会记录地址与底层错误并通过 `Trace` 返回；`local_addr`、TLS/plain accept、连接读写错误统一转换为 `SharedError`。
- 启动函数只证明监听器已绑定且后台线程已创建，不证明未来 accept/请求处理一直成功。accept 失败会记录日志并清空全局地址；单连接错误在连接线程中被丢弃，不会终止接受循环。
- HTTP 解析只有一次、最多 64 KiB 的读取，不解析 `Content-Length`、分块编码、keep-alive 或多请求连接；不完整请求体可能被截断。固定 `Connection: close` 是该限制的一部分。
- 未匹配路由为 404；`/metrics` 非 GET/HEAD 为 405；未知 pprof 子路由为 405；Rust 无 Go runtime trace，因此 trace GET 明确返回 501。
- symbol GET 固定报告零符号，POST 仅把 `+` 分隔项映射为 `?`，不是原生符号解析器。`inject_failpoint_determined_port` 当前不具备 Go failpoint 改端口的行为。

## 并发与资源生命周期

`OnceLock` 只控制 Mutex 的一次初始化，真正的“只允许一个监听器”由地址字符串和 `Mutex` 临界区实现。监听器所有权移入 serve 线程；每个已接受连接再移入独立线程，handler 通过 `Arc::clone` 延长到所有连接结束。没有线程句柄、关闭通道或显式优雅退出接口，因此正常运行时监听线程及监听器持续到进程结束；只有 accept 错误才退出并释放监听器、清空状态。

CPU profile 在连接线程中同步 `sleep(seconds)`，不会阻塞 accept，但会占用一个未设上限的 OS 线程。每连接一线程同样没有并发上限，扩展时需注意慢客户端、线程数量和资源耗尽风险。全局 pprof profiler 是否允许并发采样由 `pprof` 依赖决定；本文件没有额外串行化 profile 请求。

## 与 Go 版本的对应关系

`br/pkg/utils/pprof.go` 是直接语义对照。两版都用互斥保护全局地址、拒绝重复启动、在绑定成功后记录实际地址、支持 TLS 包装、后台 serve，并提供 metrics 与标准 pprof 路径。Rust 的 `StartStatusListenerWithHandler` 接受闭包型 `StatusHandler`，Go 接受 `http.Handler`；Rust 自行实现最小 HTTP 循环，Go 使用 `http.Serve`/`http.ServeMux`。

重要差异是：Go 的 `determined-pprof-port` failpoint 可覆盖地址，Rust 钩子当前为空；Go 使用运行时原生 `net/http/pprof`，Rust 的 profile 由 `pprof-rs` 生成、trace 返回 501、symbol 是兼容占位；Go HTTP 栈支持完整协议与服务生命周期，Rust 仅单次 64 KiB 读取并关闭连接。Rust serve 失败时清空状态与 Go 一致，但连接级错误被静默丢弃。以上差异是当前代码事实，不能描述为完整等价移植。

## 扩展指南

- 新增 status 路由应优先扩展 `RegisterDefaultStatusHandlers` 的路径/方法匹配，并在独立的 `br/pkg/utils/pprof_test.rs` 增加成功、错误方法和未知路径测试；不要把测试写进生产文件。
- 若增加通用 HTTP 能力，应围绕 `handle_connection` 处理分段读取、请求大小、`Content-Length`、超时和并发限制，而不是假定当前解析器已覆盖这些情况。协议变化还需检查 `StatusRequest`/`StatusResponse` 的兼容性。
- 若补齐 failpoint、trace 或 symbol，应以 `br/pkg/utils/pprof.go` 的可观察语义为基准，同时明确 Rust runtime 能力边界；不可用能力应继续显式报错。
- 若增加停服/重启能力，需要同时设计监听器关闭信号、线程句柄、`STARTED_PPROF` 清理顺序以及请求中的 profile 生命周期，避免地址状态与真实监听器失配。
- 调整 TLS 或 listener 行为时，应同步核验 `astersql_util::security::{TLS, Listener, Connection}`；调整命令接线时核验 `br/cmd/br/cmd.rs::startStatusServer` 与 `dyn_pprof_unix.rs`。
- profile 时长、频率或线程模型会改变 CPU 开销和拒绝服务风险，应保留明确上限并补充边界与并发回归测试。

## 验证依据

- 已完整阅读：`br/pkg/utils/pprof.rs`、`br/pkg/utils/Cargo.toml`、`br/pkg/utils/lib.rs`、`br/pkg/utils/pprof.go`、`br/pkg/utils/pprof_test.rs`；调用入口另核对 `br/cmd/br/cmd.rs`、`br/cmd/br/cmd.go` 与 `br/pkg/utils/dyn_pprof_unix.rs`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`query StartStatusListenerWithHandler --kind function --json`、`query RegisterDefaultStatusHandlers --kind function --json`、`query serve_listener --kind function --json`、`query handle_connection --kind function --json` 精确定位本文件符号和签名。`explore/node` 及按 ID 请求 callers/callees 未给出可靠调用边，因此没有据此推断；上、下游关系由仓库内已索引文本搜索和源码交叉核验。
- `br/pkg/utils/pprof_test.rs` 的独立测试覆盖：默认 registry 指标输出、真实进程命令行、未知路由保持未匹配、1 秒 profile 可解码且含 sample type、cmdline 错误方法返回 405、trace 返回 501。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行固定 11 章节结构检查，并人工复核文档只描述当前实现、明确差异和未实现边界。
