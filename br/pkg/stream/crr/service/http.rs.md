# `br/pkg/stream/crr/service/http.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream-crr-service` library crate。crate 入口 `br/pkg/stream/crr/service/lib.rs` 以公开模块 `http` 挂载它，并由同一 crate 的 `service.rs` 提供 `Service`、`status.rs` 提供 `StatusSnapshot` 与 JSON 编码器。`br/pkg/stream/crr/service/Cargo.toml` 只声明内部 checkpoint crate 和 `prometheus` 依赖；这里没有绑定 Hyper、Axum 等具体 HTTP 运行时。

它对应 Go 文件 `br/pkg/stream/crr/service/http.go`，负责定义 CRR checkpoint 服务的三条运维端点。当前 Rust 实现是框架无关的注册/响应抽象；全仓搜索到的 `HttpMux` 实现和 `Service::Register` 调用只位于 `service_test.rs`、`parity_test.rs`，没有发现生产 HTTP server 适配与启动接线，因此不能把该文件描述成已经监听端口的服务器。

## 核心职责

- 用 `HttpMux`、`HttpResponseWriter` 和 `HttpHandler` 表达最小 HTTP 接口，使状态端点不依赖具体 Web 框架。
- `Service::Register` 一次注册 `/livez`、`/readyz`、`/status` 三个固定路径，并让每个闭包持有自己的 `Arc<Service>`。
- `/livez` 只检查快照的 `Live`，`/readyz` 只检查 `Ready`，分别返回 200 或 503；二者没有响应体。
- `/status` 把 `Service::Status()` 返回的一致性快照编码为 JSON，成功返回 200，编码失败返回 500 和错误文本。
- `RegisterOrPanic` 显式复刻 Go 向 `Register` 传入 nil mux 时的快速失败契约。

本文件不推进 checkpoint、不修改状态、不创建监听 socket，也不解释请求方法或请求体；状态产生和生命周期在 `service.rs`/`status.rs` 中完成。

## 主要符号

- `STATUS_OK`、`STATUS_SERVICE_UNAVAILABLE`、`STATUS_INTERNAL_SERVER_ERROR`：分别固定为 200、503、500，对齐 Go `net/http` 状态码。
- `HttpRequest { Method, Path }`：最小请求视图。字段公开，但本文件注册的三个 handler 都将请求参数命名为 `_req` 并忽略，因此目前不会限制 HTTP method，也不会自行按 `Path` 二次路由。
- `HttpResponseWriter`：响应输出 trait。`Header` 提供可变 header map，`WriteHeader` 写状态码，`WriteBody` 写字节体；调用顺序由 handler 负责。
- `HttpHandler`：`Box<dyn Fn(&HttpRequest, &mut dyn HttpResponseWriter) + Send + Sync>`。`Send + Sync` 允许 mux/服务器在多线程环境共享 handler，但具体调度由实现 `HttpMux` 的适配器决定。
- `HttpMux::HandleFunc(path, handler)`：唯一的路由注册操作；覆盖策略、404、method dispatch 等均不在本 trait 契约内。
- `Service::Register(self: &Arc<Self>, mux: &mut M)`：公开注册入口。要求调用者以 `Arc<Service>` 持有服务，为每条路由 clone 一次 `Arc`。
- `Service::handle_liveness`、`handle_readiness`、`handle_status`：私有处理器，分别实现存活、就绪和状态响应。
- `RegisterOrPanic(service, Option<&mut M>)`：公开辅助函数；`Some` 转发给 `Register`，`None` 以固定消息 `service: nil mux` panic。

## 执行流程

1. 调用方准备 `Arc<Service>` 和某个 `HttpMux` 实现，调用 `Service::Register`；若调用边界需要表达 Go 的 nil mux，可调用 `RegisterOrPanic`。
2. `Register` 按 `/livez`、`/readyz`、`/status` 的顺序分别 clone `Arc<Service>`，将闭包交给 `HandleFunc`。注册完成后 handler 的所有权归 mux。
3. `/livez` 请求到达时，闭包调用 `handle_liveness`。处理器调用 `Service::Status()`；后者在 `service.rs` 中转发到 `StatusStore::snapshot_copy()`。`Live=false` 写 503，否则写 200。
4. `/readyz` 同样读取新快照；`Ready=false`（包括 starting、degraded 或 stopped 状态）写 503，否则写 200。它与 `Live` 的判断互不替代。
5. `/status` 先设置 `Content-Type: application/json`，再将快照交给 `status.rs::encode_status_snapshot`。成功时补上 Go `json.Encoder.Encode` 会产生的尾换行，依次写 200 和 UTF-8 JSON body；失败时写 500 和错误字符串。
6. `RegisterOrPanic(None)` 不注册任何路由而立即 panic；`Some(mux)` 的行为完全由上述注册流程决定。

## 数据与状态

本文件自身不保存可变业务状态。三条闭包仅各自保存一个 `Arc<Service>`；请求期间产生的 `StatusSnapshot` 是独立值。`Service::Status()` 调用 `StatusStore::snapshot_copy()`，后者在读锁下 clone 快照及 map 字段，返回后即释放锁，所以编码和响应写入不会长期占用状态锁。

`Live`/`Ready` 的来源不在本文件：`StatusStore::start()` 将二者置为 true，`stop()` 将二者置为 false，计算失败事件把 `Ready` 置为 false 并进入 degraded，成功推进事件恢复 `Ready`。因此 `/livez` 表达运行循环是否存活，`/readyz` 表达服务是否可声明就绪；`/status` 无论 degraded 与否都尝试导出完整快照。

`HttpRequest.Method`、`HttpRequest.Path` 目前只由适配层或测试夹具填充。由于 handler 忽略整个请求对象，调用者若需要 GET-only、鉴权、超时、请求大小限制或路径规范化，必须在具体 mux/HTTP server 适配层实现。

## 依赖与调用关系

上游关系：

- `lib.rs` 公开 `http` 模块，但没有将其中符号扁平 `pub use`；外部调用应经 crate 的 `http` 模块访问这些类型。
- `service_test.rs::test_service_status_endpoints` 和 `parity_test.rs::contract_normal_status_and_http` 使用内存 `TestMux` 调用 `Service::Register`。
- `service_test.rs::test_service_register_panics_on_nil_mux` 与 `parity_test.rs::contract_boundary_defaults_and_register_guard` 调用 `RegisterOrPanic` 验证 nil 边界。
- 仓库搜索未找到上述注册 API 的非测试调用者，也未找到 `HttpMux` 的生产实现；当前生产启动链是否暴露这些端点尚未接线，不能由此文件推断。

下游关系：

- `Service::Register` 下调调用者提供的 `HttpMux::HandleFunc`。
- 三个处理器调用 `service.rs::Service::Status`，再经 `status.rs::StatusStore::snapshot_copy` 读取状态。
- `handle_status` 调用 `status.rs::encode_status_snapshot`；该手写编码器负责字段名、时间、map 排序、转义和 Go `omitempty` 对齐，可能因 RFC3339 年份超界返回错误。
- 响应最终通过调用者实现的 `HttpResponseWriter` 输出；本文件不控制网络缓冲、flush、连接关闭或重复写头的具体效果。

## 错误处理与边界

- 未启动、已停止或其他 `Live=false` 状态：`/livez` 返回 503；不写 body。
- starting、degraded、stopped 等 `Ready=false` 状态：`/readyz` 返回 503；不写 body。degraded 不妨碍 `/status` 返回状态详情。
- `encode_status_snapshot` 成功：`/status` 固定设置 JSON content type、返回 200，并在单个 JSON 值后追加 `\n`。
- 编码失败：`/status` 已经设置 JSON content type，但 body 是错误文本；处理器写 500 后写错误字符串。当前可达失败证据来自时间字段超出 Go RFC3339 支持的 `[0,9999]` 年范围。
- nil mux：Rust 泛型引用本身不能为 nil，故 `RegisterOrPanic` 用 `Option` 表达该边界并保持固定 panic 文案。直接调用 `Service::Register` 时编译期要求真实可变引用。
- 未知路径、重复路由、writer I/O 失败和 handler panic 均没有在抽象中建模。`WriteHeader`/`WriteBody` 无返回值，因此网络写失败无法从本层传播。
- 请求 method 不参与判断；只要外部 mux 将请求分派到 handler，非 GET 请求也会得到同样响应。这是当前代码事实，不应假定存在 method guard。

## 并发与资源生命周期

`HttpHandler` 要求 `Send + Sync`，每个闭包捕获 `Arc<Service>`，因此注册后的 handler 可跨线程保存和调用，不借用注册调用栈。三次 clone 意味着 mux 持有路由期间会保持服务存活；只有 mux 释放所有 handler 且其他 `Arc` 也释放后，`Service` 才能析构。

每次请求独立调用 `Status()`，得到深拷贝快照后再编码，不共享响应 buffer，也不持有 `StatusStore` 的 `RwLock` 执行 writer 操作。具体 `HttpMux` 和 `HttpResponseWriter` 的线程安全、路由并发度、背压与连接生命周期不由这些 trait 保证：`Register` 只需要注册期的 `&mut M`，请求期 writer 由服务器逐次传入。

本文件不会启动线程或异步任务，也没有显式 shutdown 钩子。`Service::Run` 的开始/停止改变快照，handler 对服务的 `Arc` 所有权则独立于运行循环；所以“对象仍存活”不等于 `/livez` 必为 200，最终以快照的 `Live` 为准。

## 与 Go 版本的对应关系

Rust `Service::Register` 对应 Go `(*Service).Register(*http.ServeMux)`，三条路径、顺序和处理语义一致。Go 可直接接收 nil 指针并在方法内部 panic；Rust 将正常注册和 nil 兼容边界拆成 `Register(&mut M)` 与 `RegisterOrPanic(Option<&mut M>)`。

Go handler 直接使用 `net/http.ResponseWriter`、`*http.Request` 和 `http.ServeMux`；Rust 用三个本地抽象隔离具体 HTTP 库，因此仍需生产适配器才能真正提供网络端点。Go `json.NewEncoder(w).Encode(s.Status())` 自带尾换行，Rust 的 `handle_status` 在 `encode_status_snapshot` 结果后显式追加换行。

Go 编码失败时调用 `http.Error`，该函数通常还会处理文本响应头和换行；Rust 当前只保留核心的 500 状态与错误文本写入，且此前设置的 `Content-Type` 仍为 `application/json`。这是实现层面的可观察差异。成功路径的字段序列化细节由 `status.rs` 独立复刻，而不是由通用 serde 编码器完成。

对应测试为 Go `service_test.go::TestServiceStatusEndpoints`、`TestServiceRegisterPanicsOnNilMux`，以及 Rust `service_test.rs::test_service_status_endpoints`、`test_service_register_panics_on_nil_mux` 和 `parity_test.rs` 的 normal/boundary 契约场景。

## 扩展指南

- 新增端点时，在 `Service::Register` 中注册新路径，把业务处理保持为私有方法；同时在 `service_test.rs` 的独立 `TestMux` 场景验证状态码、header/body 和关键状态分支，并在 `parity_test.rs` 更新公开契约。若来自 Go 移植，应同步核对 `http.go` 与 Go 测试。
- 若接入真实 HTTP server，应在其他生产文件实现 `HttpMux`/`HttpResponseWriter` 适配，明确 method、未知路由、重复注册、写失败、flush、超时和 shutdown 语义；不要把框架类型反向泄漏进状态逻辑。
- 若改变存活或就绪判定，应优先修改 `status.rs` 的状态迁移并证明 `Live`/`Ready` 不变量，而不是在 handler 内重复推导状态；同步测试 starting、running、degraded、stopped。
- 若改变 `/status` schema 或编码错误行为，应修改 `status.rs::encode_status_snapshot` 及其独立 `status_test.rs`，同时保持本文件只负责 HTTP 包装；关注 Go 字段名、omitempty、map 键顺序、时间范围和尾换行兼容。
- 若要限制 HTTP method，可在生产适配层统一实现，或扩展本文件契约并为非 GET 请求增加明确测试；当前静默忽略 `Method`，直接改变会构成行为兼容风险。
- 若让 writer 方法返回错误，需要贯穿 trait、所有适配器和独立测试重新设计传播策略；当前签名无法观测 socket 写失败。

## 验证依据

- RustCodeGraph：`status`/`files --filter br/pkg/stream/crr/service`；`node --file br/pkg/stream/crr/service/http.rs --offset 1 --limit 500` 确认目标文件 124 行、符号与直接测试使用；对 `lib.rs`、`service.rs`、`status.rs`、`service_test.rs`、`parity_test.rs` 使用 `node --file` 核对模块挂载、`Service::Status`、快照复制、JSON 编码和测试夹具。
- 源码：`br/pkg/stream/crr/service/http.rs`；直接 Rust 依赖证据为 `service.rs::Service::Status` 与 `status.rs::StatusStore::snapshot_copy`、`encode_status_snapshot`。
- crate/模块：`br/pkg/stream/crr/service/Cargo.toml`、`br/pkg/stream/crr/service/lib.rs`。
- Go 对照：`br/pkg/stream/crr/service/http.go`；Go 测试：`br/pkg/stream/crr/service/service_test.go` 的 `TestServiceStatusEndpoints`、`TestServiceRegisterPanicsOnNilMux`。
- Rust 独立测试：`br/pkg/stream/crr/service/service_test.rs` 的 `test_service_status_endpoints`、`test_service_register_panics_on_nil_mux`，以及 `br/pkg/stream/crr/service/parity_test.rs` 的 `contract_normal_status_and_http`、`contract_boundary_defaults_and_register_guard`。
- 全仓 `rg` 交叉核验：生产依赖只发现 CRR config crate 使用 service crate 的配置符号；`HttpMux`、`HttpResponseWriter`、`RegisterOrPanic` 和 `Service::Register` 的 CRR 调用/实现只出现在上述独立测试，故本文将生产 HTTP 接线标为未发现，而非已支持。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令验证目标文件存在且固定二级标题恰好 11 个，并人工复核没有把测试替身描述成生产服务器。
