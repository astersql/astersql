# [`pkg/server/internal/advertisedstatus/checker.rs`](./checker.rs)

## 文件定位

该文件属于 `astersql-server-internal` crate；crate 根在 `pkg/server/internal/lib.rs`，其中用 `#[path = "advertisedstatus/checker.rs"] pub mod advertisedstatus` 将本文件公开给上层 server crate。它实现的是一次性、仅告警的 advertised status endpoint 身份校验：TiDB 状态 HTTP 服务启动后，请求配置所宣告地址上的 `/info`，确认该地址最终返回当前实例的 DDL ID，而不是另一实例、代理或错误服务。

生产入口位于 `pkg/server/server.rs` 的 `Server::run`：状态 HTTP、MySQL 和 PostgreSQL 接受循环启动成功后，代码从有效状态监听地址、`advertise_address`、`Domain::local_ddl_id()` 和状态 TLS 配置组装 `advertisedstatus::Options`，再调用 `advertisedstatus::start`。返回的 `CheckHandle` 保存在 `Server::advertised_status_check` 中；`Server::close_listeners` 在关闭各监听器之前先丢弃它，从而取消并回收诊断任务。

## 核心职责

1. `Options::endpoint` 判断检查所需前提是否齐全，并用实际状态监听端口、配置的宣告主机及 TLS 状态构造 `/info` URL；IPv6 主机自动加方括号。
2. `TlsOptions::builder` 重建带相同 CA 与客户端证书身份的 `reqwest::ClientBuilder`；`new_http_client` 在此基础上禁用代理和重定向，并设置 5 秒总超时，避免代理或跳转后的其他实例被误判为当前实例。
3. `check_endpoint`/`check_response` 发出 GET，请求成功时限制响应体为 1 MiB，使用 `astersql_domain_serverinfo::StaticInfo::Unmarshal` 解析 `/info` JSON 中的 `ddl_id`，再与本地 DDL ID 比较。
4. `start_with_client_builder` 把异步 HTTP 检查放进独立 OS 线程和单线程 Tokio runtime，成功匹配时静默；失败时通过 reporter 输出一次结构化警告；取消时抑制告警。
5. `CheckHandle` 拥有取消信号和工作线程，销毁时先通知取消再 `join`，把检查生命周期绑定到 `Server`。

该模块不改变启动结果、不重试、不修正配置，也不阻止 server 提供服务；它只提供拓扑配置诊断。

## 主要符号

- `CHECK_TIMEOUT: Duration`：HTTP 客户端总超时，固定为 5 秒。
- `BODY_LIMIT: usize`：允许读取的最大响应体，固定为 `1 << 20` 字节；实现最多保留 `BODY_LIMIT + 1` 字节来识别越界。
- `WARNING_MESSAGE`：生产 reporter 使用的固定警告消息。
- `TlsOptions { ca, certificate, key }`：TLS 文件路径集合。`scheme()` 只要任一 TLS 字段存在就选择 `https`；`builder()` 加载 CA，并且仅在证书和私钥同时存在时加载客户端身份。
- `Options { report_status, status_address, advertise_address, local_id, tls }`：一次检查的不可变输入。`endpoint()` 返回 `None` 表示前提不足，调用方不会创建线程。
- `CheckResult { reason, remote_id, status, error }`：检查的结构化结果。默认值（尤其是空 `reason`）代表身份匹配；失败原因采用稳定字符串：`request-failed`、`unexpected-status`、`invalid-response`、`missing-identity`、`identity-mismatch`。
- `new_http_client`：强制 `.no_proxy()`、禁止重定向并设置超时的客户端构造器。
- `check_endpoint`：可取消的异步检查入口；取消与网络检查用带 `biased` 的 `tokio::select!` 竞争。
- `check_response`：HTTP 状态、响应体上限、JSON/`ddl_id` 和身份匹配的核心判定逻辑。
- `warning_action`、`warning_fields`：把原因映射为运维建议，并生成 endpoint、本地 ID、原因、动作以及按需出现的远端 ID、HTTP 状态和错误字段。
- `CheckHandle`：取消发送端与 `JoinHandle<()>` 的 RAII 所有者；`cancel()` 可显式置位，`Drop` 保证取消和回收。
- `start`：生产入口，固定使用 `BgLogger()` 以 `Warn` 级别记录 `WARNING_MESSAGE`。
- `start_with_reporter`：注入结果 reporter 的入口，供测试观察一次性报告。
- `start_with_client_builder`：crate 内注入 HTTP builder 的最底层入口，用于验证 TLS、DNS 和生命周期边界。

## 执行流程

1. `Server::run` 先启动 status HTTP，再调用 `start(options)`；若 `report_status` 为假、状态地址缺失、宣告地址为空或本地 DDL ID 为空，`Options::endpoint` 返回 `None`，整个检查直接跳过。
2. endpoint 使用 `TlsOptions::scheme()`、宣告主机和 `status_address.port()` 形成 `http(s)://host:port/info`。主机字符串只负责宣告地址，端口取已绑定监听器的有效端口，因此配置端口为 `0` 时也能检查真实端口。
3. `start_with_client_builder` 建立初值为 `false` 的 watch channel，然后立即启动工作线程并返回 `CheckHandle`，不会等待网络结果。
4. 工作线程在 `catch_unwind` 内建立 current-thread Tokio runtime。builder 读取 TLS 材料，`new_http_client` 加上直连、无跳转和超时策略，再由 runtime 执行 `check_endpoint`。
5. `check_endpoint` 先检查取消位；未取消时，以取消变化为优先分支，与 `check_response` 竞争。取消结果统一表示为 `request-failed`/`context canceled`。
6. `check_response` 发出 GET。发送错误归为 `request-failed`；收到响应后保留含远端 reason phrase 的完整状态字符串。非 2xx 直接归为 `unexpected-status`，不解析正文。
7. 2xx 正文按 chunk 读取且不超过上限加一。读失败为 `request-failed`；超过 1 MiB 或 JSON 解析失败为 `invalid-response`；缺少或为空的 `ddl_id` 为 `missing-identity`；存在但不等于本地 ID 为 `identity-mismatch`；相等则保持空 `reason`。
8. runtime 用 `shutdown_background()` 结束，避免 server 生命周期等待不可中止的 OS DNS 作业。只有接收端此时仍未取消且 `reason` 非空时才调用 reporter；匹配成功和关机取消均保持静默。
9. reporter 自身或检查逻辑若 panic，会被最外层 `catch_unwind` 隔离，只额外记录 `advertised status endpoint check panicked`，不会终止 server。

## 数据与状态

输入 `Options` 在启动线程时整体移动，检查过程中不再读取 `Server` 或全局配置。`TlsOptions` 先克隆出来交给 builder 闭包；endpoint 与 `local_id` 则留在移动闭包中的 `options` 内。这样检查看到的是启动瞬间的一致快照，而不是后续可变配置。

`CheckResult` 同时保留分类和诊断细节：`reason` 用于机器可读分支及动作建议，`status` 在收到 HTTP 响应后即写入，即使随后读取正文失败也仍可诊断；`remote_id` 仅在 JSON 包含非空 `ddl_id` 后写入；`error` 仅保存请求、构建、解析或缺失字段的文字错误。身份匹配没有单独的成功枚举，而以 `reason == ""` 表示，这也是 reporter 抑制条件。

取消状态由 `tokio::sync::watch<bool>` 承载。`CheckHandle` 持有发送端，worker 持有接收端；`send_replace(true)` 是幂等式取消。线程句柄装在 `Option` 中，保证 `Drop` 最多 join 一次。

## 依赖与调用关系

上游调用链为 `Server::run`（`pkg/server/server.rs`）→ `advertisedstatus::start` → `start_with_reporter` → `start_with_client_builder` → `check_endpoint` → `check_response`。关机链为 `Server::close_listeners` → `Option<CheckHandle>::take` → `CheckHandle::drop` → `cancel`/`JoinHandle::join`。

直接下游依赖如下：

- `reqwest`：TLS HTTP 客户端、超时、禁用代理和重定向、流式 body chunk；`pkg/server/internal/Cargo.toml` 关闭默认 feature，仅启用 `rustls-tls`。
- `hyper::ext::ReasonPhrase`：优先保留响应线上实际 reason phrase，缺失时才退回状态码的 canonical reason。
- `tokio`：current-thread runtime、`watch` 取消通道和 `select!`；Cargo feature 为 `rt`、`time`、`sync`、`macros`。
- `astersql-domain-serverinfo::StaticInfo`：复用 `/info` 的协议类型及其 `Unmarshal`，其中 `StaticInfo::ID` 就是 DDL/节点唯一 ID。
- `astersql-util-logutil`：生产告警的 `BgLogger`、`LogLevel` 和结构化 `LogField`。
- 标准库线程与网络类型：`SocketAddr` 提供有效端口，`thread::spawn`/`JoinHandle` 隔离并管理 runtime。

本文件没有条件编译项；测试由 `pkg/server/internal/lib.rs` 的 `#[cfg(test)]` 独立装入 `advertisedstatus/checker_test.rs`，符合生产逻辑与测试逻辑分文件的约束。

## 错误处理与边界

- 前提不足不是错误：`start*` 返回 `None`，不请求也不告警。
- TLS 文件读取、PEM 解析、runtime/client 创建失败在 worker 内转为 `CheckResult::failed`，分类为 `request-failed`；错误不会向 `Server::run` 返回。
- HTTP 发送、超时、TLS 握手和正文读取错误均为 `request-failed`。`CheckResult::request_failed` 会沿错误 source 链追加文本，避免只留下最外层 reqwest 描述。
- 仅 2xx 为可接受状态；3xx 不跟随，4xx/5xx 不解析正文，均为 `unexpected-status`。
- 正文恰好 1 MiB 可接受，超过一个字节即为 `invalid-response`；该限制防止诊断请求无界占用内存。
- JSON 必须能按 `StaticInfo` 协议解析并提供非空 `ddl_id`。字段缺失、`null` 或空字符串是 `missing-identity`；错误类型（例如数字 ID）及畸形 JSON 是 `invalid-response`。
- endpoint 构造只根据字符串是否含 `:` 判断 IPv6 并加方括号；调用者应传裸主机/IP，而不是已有 scheme、端口或方括号的完整 URL。
- `TlsOptions::builder` 只有证书和私钥同时存在时才配置客户端身份；只配置其中之一会选择 HTTPS 但不会提前报告“不成对”，最终可能在连接或服务端认证阶段失败。
- worker panic 被隔离；线程 `join` 的错误被忽略，因为 panic 已由内部边界尽量捕获，诊断任务不得破坏关闭流程。

## 并发与资源生命周期

每次 `start` 最多创建一个 OS 线程，并在线程内创建一个单线程 Tokio runtime；模块自身没有循环和重试，所以网络检查最多执行一次。调用立即返回句柄，server 启动不等待检查完成。

`CheckHandle::drop` 的顺序是不变量：先发送取消，再 join worker。`Server::close_listeners` 又保证在 status listener 尚存活时先 drop 该句柄，避免正常关闭被记录成 endpoint 故障。取消分支在 `tokio::select!` 中带优先级，worker 完成后也再次检查 receiver，因而取消与结果几乎同时发生时优先抑制告警。

reqwest 请求 future 被取消后连接资源随之释放。runtime 使用 `shutdown_background()`，专门避免 join 等待 reqwest/tokio 后台中无法中止的 OS DNS 阻塞作业；这意味着相关后台 DNS 工作可能短暂继续，但不再阻塞 server 关闭。CA 与客户端身份在构建 client 时读入内存，builder 返回后源文件可以被删除，测试对此有覆盖。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/server/internal/advertisedstatus/checker.go`，两者保持相同的外部语义：前提检查、使用有效 listener 端口、`/info` GET、5 秒超时、1 MiB 正文上限、禁止代理和跳转、复用 `serverinfo.StaticInfo` 的 `ddl_id`、五类失败原因、成功静默、取消不告警及一次性结构化告警。独立对照测试分别位于 `checker_test.go` 和 `checker_test.rs`。

Rust 为适配所有权与异步运行时做了局部接线差异：

- Go `Options` 持有 `net.Listener` 并从其 `Addr()` 取端口；Rust 为避免跨 crate 传递 listener，持有 `Option<SocketAddr>`。
- Go 使用 server `context.Context`、goroutine 和 `http.Client`；Rust 返回拥有 watch 取消端与工作线程的 `CheckHandle`，在句柄销毁时同步 join。
- Go 从 `util.InternalHTTPClient()` 克隆 transport，以继承进程内部 TLS；Rust 显式接收 CA、证书和私钥路径并重建 reqwest builder。两者都在派生客户端上移除代理且不跟随重定向。
- Go 测试用 context value 注入 reporter；Rust 以 `start_with_reporter` 参数注入，并以 crate-private 的 `start_with_client_builder` 替换 DNS/TLS builder 边界。
- Rust 额外显式处理 runtime 后台关闭和不可中止 DNS 工作，目的是保持 Go context 取消“不因 OS DNS 阻塞 server 退出”的生命周期语义。

这些是实现机制差异，不应被理解为改变“仅告警、一次检查、取消静默”的 Go 合约。

## 扩展指南

- 新增或改变启用前提、URL 规则时修改 `Options::endpoint`，并同步 `endpoint_url_uses_listener_port_for_ipv4_and_ipv6`、`start_prerequisites_do_not_request_or_report` 以及 `pkg/server/http_status_test.rs` 中 server 级接线测试。特别注意 IPv6、端口 `0` 后的有效端口和 `report_status = false` 不读取 domain identity 的契约。
- 新增 HTTP 安全策略时优先落在 `new_http_client`/`TlsOptions::builder`，不要在检查路径重新启用系统代理或自动跳转；同步代理、重定向、CA、mTLS 和 TLS 验证失败测试。
- 新增响应字段或失败分类时修改 `check_response`、`CheckResult`、`warning_action` 和 `warning_fields`，并同时核对 Go 的 `endpointCheckResult`/`endpointWarningAction`；保持成功用空 reason 表示的现有 reporter 合约，除非上下游一并迁移。
- 调整正文读取必须保持有界内存和“恰好上限可接受”的边界，测试 `endpoint_responses_match_go_identity_and_body_limits` 应同步更新。
- 改动取消/线程模型时必须维护三个性质：启动不阻塞、server 正常关闭不告警、drop 不等待不可中止 DNS；对应 Rust 测试为 `endpoint_cancellation_closes_the_request_and_suppresses_warning`、`inflight_cancellation_returns_request_failure_and_closes_connection`、`lifecycle_cancellation_does_not_wait_for_a_blocking_dns_job`。
- 生产 Rust 测试继续放在独立的 `checker_test.rs`，不要内嵌回本文件；Go 行为变化也应同步 `checker_test.go`。性能风险主要来自每次启动一个 OS 线程、TLS/证书读取和 1 MiB 缓冲，但当前每次 server Run 只执行一次，不能在未评估资源上限时扩展为周期轮询。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/server/internal/advertisedstatus` 确认目标 Rust/Go 源及两套独立测试均在图中。
- RustCodeGraph 文件与符号读取：`checker.rs` 中的 `TlsOptions`、`Options`、`CheckResult`、`new_http_client`、`check_endpoint`、`check_response`、`warning_*`、`CheckHandle` 和 `start*`；精确查询确认 `checker.rs::check_endpoint` 位于第 126 行。
- 上游及生命周期证据：`pkg/server/server.rs` 的 `Server::advertised_status_check`、`Server::run`（调用 `advertisedstatus::start`）和 `Server::close_listeners`（先 `take` 句柄）；server 级测试见 `pkg/server/http_status_test.rs` 的 `advertised_status_info_returns_the_local_ddl_identity`、`advertised_status_start_requests_configured_host_and_close_cancels_inflight_request`、`advertised_status_disabled_does_not_read_domain_identity`。
- crate 与依赖证据：`pkg/server/internal/lib.rs` 的公开模块接线；`pkg/server/internal/Cargo.toml` 的 `reqwest`、`hyper`、`tokio`、`astersql-domain-serverinfo`、`astersql-util-logutil` 和测试用 `openssl` 声明。
- 协议证据：RustCodeGraph 的 `pkg/domain/serverinfo/info.rs::StaticInfo` 定义确认 `ID` 为 DDL/节点唯一 ID；目标实现通过其 `Unmarshal` 读取 `/info`。
- Go 对照：`pkg/server/internal/advertisedstatus/checker.go` 的 `Start`、`newEndpointHTTPClient`、`checkEndpoint`、`endpointWarningAction` 和 `logEndpointCheckWarning`；Go 测试为同目录 `checker_test.go`。
- Rust 边界测试：`pkg/server/internal/advertisedstatus/checker_test.rs` 覆盖前提、IPv4/IPv6、状态与正文边界、重定向、代理、TLS/mTLS、请求错误、结构化诊断、取消、连接释放和阻塞 DNS 生命周期。
- 本任务为纯文档分析，依计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。
