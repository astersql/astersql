# `tools/fake-oauth/main.rs`

## 文件定位

本文件是 `astersql-tools-fake-oauth` crate 的 Rust 可执行入口，也是库入口复用的实现模块。`tools/fake-oauth/Cargo.toml` 的 `[[bin]]` 将它声明为 `astersql-tools-fake-oauth` 二进制；`tools/fake-oauth/lib.rs` 又通过 `#[path = "main.rs"] pub mod main` 暴露同一实现，并由 `entry()` 调用 `main::main()`。crate 的移植元数据把其 Go 来源标为 `tools/fake-oauth`、类型标为 `binary`，工作区根 `Cargo.toml` 将该 crate 纳入成员列表。

它对照 `tools/fake-oauth/main.go`，提供测试用的固定 OAuth 令牌端点契约。需要特别区分当前实现与 Go 工具：Go 版使用 `net/http` 真正监听端口；Rust 版经本地 `stubs` 模块模拟路由和监听，只在进程内保存状态，不创建网络监听套接字（`tools/fake-oauth/stubs.rs::ListenAndServe`）。因此，当前 BR/Lightning 集成流程仍由 `Makefile` 编译 Go `main.go`，并由 `br/tests/br_gcs/run.sh`、`lightning/tests/lightning_gcs/run.sh` 启动 `fake-oauth`。

## 核心职责

本文件只维持 Go 假 OAuth 服务的最小可观察契约：

1. 用 `TOKEN_PATH` 固定注册路径 `/oauth/token`。
2. 对命中请求忽略方法和请求内容，经 `ResponseWriter::Write` 写入 `TOKEN_JSON` 的精确字节。
3. 用 `LISTEN_ADDR` 固定启动地址 `:5000`，并以 `None` 表示 Go `http.ListenAndServe` 的 `nil` handler，即使用默认 mux。
4. 保持 Go 原版的容错语义：响应写入结果和监听结果都通过 `let _ = ...` 丢弃。

该文件不解析 OAuth grant、客户端凭据或请求体，不生成动态 token，也不设置响应头。它是集成场景的认证桩，不是通用 OAuth 服务。

## 主要符号

- `pub mod stubs`：载入同目录 `stubs.rs`，提供 `HandleFunc`、`ListenAndServe`、请求/响应类型和可重置的进程内默认 mux。该模块公开是为了让独立对照测试检查副作用。
- `pub const TOKEN_PATH: &str`：唯一端点的精确路径 `/oauth/token`。`stubs::ServeMux::serve` 按 `Request.path` 精确匹配，尾斜杠不会命中。
- `pub const LISTEN_ADDR: &str`：启动参数 `:5000`，逐字对应 Go 的 `http.ListenAndServe(":5000", nil)`。
- `pub const TOKEN_JSON: &[u8]`：固定成功响应的原始字节，内容为 access token `ok`、token type `service_account` 和 3600 秒有效期。空格和字段排列也是对照测试锁定的契约。
- `pub fn token_response_body() -> &'static [u8]`：直接返回静态 `TOKEN_JSON` 切片，不分配、不复制，主要为库调用和对照测试提供稳定观察面。
- `pub fn register_routes()`：在默认 mux 上注册闭包。闭包忽略 `Request`，把 `TOKEN_JSON` 写入 `ResponseWriter`，并忽略 `Write` 的 `io::Result`。
- `pub fn run_main()`：先调用 `register_routes()`，再调用 `stubs::ListenAndServe(LISTEN_ADDR, None)`；监听错误被显式忽略。
- `pub fn main()`：二进制入口的最薄包装，仅调用 `run_main()`。库侧另由 `lib.rs::entry()` 调用它。

文件中没有自定义类型、trait、`impl` 或条件编译项；状态类型和同步原语均位于 `stubs.rs`。

## 执行流程

直接运行 Rust 二进制时，控制流是 `main` → `run_main` → `register_routes` → `stubs::HandleFunc`。注册调用把路径和一个 `'static + Send + Sync` 闭包放进默认 `ServeMux` 的路由表。随后 `run_main` 调用 `stubs::ListenAndServe(":5000", None)`；该桩记录地址和“使用默认 mux”，然后立即返回成功或注入的错误，二者都不会使 `run_main` 返回错误或 panic。

测试分派请求时，`stubs::serve_default` 按路径查找闭包。命中 `/oauth/token` 后，闭包调用 `ResponseWriter::Write(TOKEN_JSON)`，把固定字节追加到响应缓冲区；HTTP 方法不会参与判断。未知路径或 `/oauth/token/` 不会命中。

库调用路径是 `tools/fake-oauth/lib.rs::entry` → `main::main`，其后流程相同。RustCodeGraph 对目标文件识别出 5 个主要符号，并标记 `tools/fake-oauth/parity_test.rs` 使用该文件；精确 callers/callees 查询未解析出额外跨文件调用边，所以上述边以函数体、`lib.rs` 和独立测试的直接引用为依据。

## 数据与状态

本文件自身只有三个不可变静态常量，没有可变全局数据。`token_response_body` 返回的切片具有 `'static` 生命周期，指向 `TOKEN_JSON`，不会转移所有权。

可变状态全部由 `stubs.rs` 持有：默认 `ServeMux` 是 `OnceLock<ServeMux>`，内部以 `Mutex<HashMap<String, HandlerFn>>` 保存路由；监听记录是另一组 `OnceLock<Mutex<ListenState>>`，保存最近的 `ListenCall` 和强制错误开关。`register_routes` 捕获的只是静态 `TOKEN_JSON`，闭包不拥有请求级可变状态。

同一进程中重复调用 `register_routes` 不具备幂等性：同一路径已存在时，`ServeMux::HandleFunc` 会 panic。`parity_test.rs::duplicate_route_registration_panics_like_go_serve_mux` 明确锁定该行为。测试必须先调用 `stubs::reset_for_test()` 清除路由、监听记录和错误注入。

## 依赖与调用关系

上游入口有两条：Cargo 的 `[[bin]]` 直接以本文件的 `main` 启动；库入口 `lib.rs::entry` 调用 `main::main`。直接行为验证者是 `tools/fake-oauth/parity_test.rs`，它调用 `register_routes`、`run_main`、`token_response_body` 并读取三个常量。

下游依赖全部来自同 crate 的 `stubs` 模块：

- `register_routes` → `stubs::HandleFunc` → 默认 `ServeMux::HandleFunc`。
- 注册的 handler → `stubs::ResponseWriter::Write`。
- `run_main` → `stubs::ListenAndServe`，参数 `None` 代表默认 mux。

`Cargo.toml` 没有声明第三方 Rust 依赖；HTTP 形状完全由本地桩和标准库实现。Go 对照则依赖标准库 `net/http`。仓库级集成入口通过 `Makefile` 构建 Go 文件，并在 BR/Lightning 的 GCS 脚本中后台启动它，这些脚本不是当前 Rust 桩的调用者。

## 错误处理与边界

`register_routes` 忽略 `ResponseWriter::Write` 返回值，与 Go 的 `_, _ = w.Write(...)` 对齐；当前桩写内存缓冲区总是返回成功，但上层没有建立“写入失败需上报”的契约。`run_main` 同样忽略 `ListenAndServe` 返回值，与 Go 的 `_ = http.ListenAndServe(...)` 对齐；`parity_test.rs::contract_error_paths` 注入监听错误并验证 `run_main` 不 panic。

路由边界是精确字符串匹配。`GET` 和 `POST` 均可命中，因为 handler 不检查 `Request.method`；错误路径、尾斜杠路径均返回未命中且不写响应体。响应默认状态为 200，但本文件不设置 `Content-Type` 或显式状态码。重复注册是可见 panic，而不是覆盖旧 handler。

最重要的实现边界是“无真实网络”：`stubs::ListenAndServe` 只记录调用。因此不能把 Rust 二进制已可替代集成测试中的 Go 服务作为当前事实；若要替代，必须先实现或接入真实 HTTP 监听并补独立集成测试。

## 并发与资源生命周期

`main.rs` 不创建线程、异步任务、通道或运行时，也不显式关闭资源。handler 由全局默认 mux 持有至测试重置或进程结束；常量和 `OnceLock` 状态均为进程生命周期。`HandlerFn` 要求 `Send + Sync`，路由表及监听状态分别由互斥锁保护。

当前桩在持有路由表锁时调用 handler。现有 handler 只写调用者独占的 `ResponseWriter`，不会重入 mux，因此不会形成当前代码中的锁循环；未来 handler 若在执行期间再次注册或分派路由，可能导致死锁，扩展时必须先调整锁的持有范围。

独立测试用 `static TEST_LOCK: Mutex<()>` 串行化共享全局状态，并在场景前后调用 `reset_for_test`。`contract_resource_cleanup` 验证响应缓冲区可由所有者清空，且重置后旧 handler 不再可见。生产入口本身没有清理过程；重复启动同一进程会触发重复注册 panic。

## 与 Go 版本的对应关系

Rust `main`/`run_main` 对应 Go `main`；`register_routes` 对应 `http.HandleFunc` 调用；handler 中的 `w.Write(TOKEN_JSON)` 对应 Go 闭包的 `w.Write([]byte(...))`；`stubs::ListenAndServe(LISTEN_ADDR, None)` 对应 `http.ListenAndServe(":5000", nil)`。路径、地址、JSON 字节、注册先于监听的顺序以及两个被忽略的错误结果均保持一致。

迁移为了可测试性新增了 Go 中没有的 `token_response_body`、三个命名常量以及 `run_main`/`register_routes` 分层。行为载体也存在实质差异：Go 使用真实 `net/http` 默认 mux 和套接字；Rust 使用功能受限的本地桩，未实现 HTTP header、网络协议、Go ServeMux 的完整匹配规则或真实阻塞监听。`parity_test.rs` 验证的是当前文件依赖的最小契约，不证明完整 `net/http` 等价。

## 扩展指南

- 修改路径、地址或响应字段时，应同步修改 `TOKEN_PATH`、`LISTEN_ADDR` 或 `TOKEN_JSON`，并更新 `tools/fake-oauth/main.go` 以及 `parity_test.rs` 中的精确字节、路径和监听断言；还要检查 GCS 测试配置里的 `token_uri`。这是外部协议变更，兼容风险高于普通内部重构。
- 新增端点应接入 `register_routes`，并在独立的 `parity_test.rs` 增加正常路径、错误路径、重复注册和重置隔离测试；不要把测试内嵌到 `main.rs`。
- 如需解析请求方法或请求体，应先扩展 `stubs::Request`，同时保留 Go 对照语义；当前 handler 对方法无条件接受，收紧会改变兼容行为。
- 如需真实可用的 Rust 服务，应替换或扩展 `stubs::ListenAndServe`，补充端口绑定、关闭、并发请求和启动失败测试，并确认 BR/Lightning 构建脚本切换目标。不能仅改变 `main.rs` 后宣称完成网络迁移。
- 如 handler 需要访问 mux、注册新路由或执行长耗时工作，应先审视 `ServeMux::serve` 持锁调用 handler 的设计，避免重入死锁和全局串行化带来的性能问题。
- 错误传播策略是 Go 兼容契约的一部分；若改成返回 `Result`、日志或退出码，应同步调整 `run_main`/`main`/`lib.rs::entry` 的接口和错误路径测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `tools/fake-oauth` 的 5 个文件；`files --filter tools/fake-oauth` 显示 `main.rs` 有 5 个符号；`node --file tools/fake-oauth/main.rs --offset 1 --limit 240` 展示完整 65 行源码并标记其被 `parity_test.rs` 使用；`query` 定位了 `token_response_body`、`register_routes` 和 `run_main`。精确 callers/callees 查询没有返回额外调用边，因此未据此声称其他图关系。
- 源码：`tools/fake-oauth/main.rs`（常量、handler、注册/启动/入口顺序）和 `tools/fake-oauth/stubs.rs`（默认 mux、精确分派、监听记录、锁与重置生命周期）。
- crate 边界：`tools/fake-oauth/Cargo.toml`、`tools/fake-oauth/lib.rs`、工作区根 `Cargo.toml`。
- Go 对照：`tools/fake-oauth/main.go`；构建与实际集成使用证据来自 `Makefile`、`br/tests/br_gcs/run.sh`、`lightning/tests/lightning_gcs/run.sh`。
- 独立测试：`tools/fake-oauth/parity_test.rs` 覆盖固定响应、200 状态、精确路径、常量字节、监听错误忽略、未知路径、重复注册 panic 和全局状态清理。同目录未发现 Go 单元测试；仓库引用搜索未发现其他 Rust 测试直接使用这些符号。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收应确认该文档存在，且固定的十一个二级标题各出现一次。
