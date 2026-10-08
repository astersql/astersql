# `pkg/server/handler/ttlhandler/ttl.rs`

## 文件定位

本文件属于 Cargo crate `astersql-server-handler-ttlhandler`，crate 入口是同目录的 `lib.rs`，并由其中的 `pub mod ttl` 对外暴露。它实现 `/test/ttl/trigger/{db}/{table}` 的核心请求控制流：检查 HTTP 方法、规范化路由参数、取得 session domain、触发指定表的 TTL 清理任务，并把结果或错误交给运行时写回。crate 的 Go 来源包由 `Cargo.toml` 的 `package.metadata.porting.go-package = "pkg/server/handler/ttlhandler"` 明确记录。

在 Rust 服务器中的直接入口位于 `pkg/server/http_status.rs`：路由注册把 `/test/ttl/trigger/{db}/{table}` 交给 `ttl_trigger_response`，后者构造 `TtlStatusRuntime`，再调用 `NewTTLJobTriggerHandler(()).ServeHTTP(&mut runtime)`。需要注意，当前这个 Rust 适配器使用 `Store = ()`、`SessionDomain = ()`，并在 `trigger_new_ttl_job` 中仅模拟 `test_ttl.t1` 成功；因此本文件的控制流已接入 Rust HTTP 状态服务，但真实 session/TTL client 后端尚未在该适配器中接通。

## 核心职责

- 通过 `TTLHandlerRuntime` 把 HTTP、session domain、TTL client、响应写出和日志记录抽象成可替换边界，使 handler 本身不依赖具体 Web 框架。
- 通过 `TTLJobTriggerHandler<S>` 保存存储句柄，并在每次请求中按固定顺序执行方法检查、参数解析、domain 获取和任务触发。
- 保持 Go `TTLJobTriggerHandler.ServeHTTP` 的关键语义：只接受 POST；将库名、表名转为小写；domain 或触发失败时记录并写回原错误；成功时先写数据，再记录包含库表名和完整响应的成功日志。
- 以 `TTLResponse` 类型别名直接复用 `astersql_ttl_client::TriggerNewTtlJobResponse`，不在 handler 层复制 TTL client 的响应模型。

本文件不负责路由匹配、HTTP 状态码映射、JSON 编解码细节、查表或调度 TTL worker；这些行为分别属于运行时适配器、`astersql-ttl-client` 及其后端实现。

## 主要符号

- `pub type TTLResponse = astersql_ttl_client::TriggerNewTtlJobResponse`：触发成功结果的别名。实际类型在 `pkg/ttl/client/command.rs` 中包含 `table_result: Vec<TriggerNewTtlJobTableResult>`。
- `pub trait TTLHandlerRuntime`：handler 与外界的完整端口。关联类型 `Error`、`Store`、`RequestContext`、`SessionDomain` 允许具体服务器选择错误、存储、上下文和 domain 表示；十个方法分别覆盖请求读取、domain/TTL 操作、响应写出和日志。
- `pub struct TTLJobTriggerHandler<S> { pub store: S }`：只持有存储依赖的泛型 handler。字段公开，当前没有额外配置或内部可变状态。
- `pub fn NewTTLJobTriggerHandler<S>(store: S) -> TTLJobTriggerHandler<S>`：保持 Go 命名和构造入口，将传入的 store 按值保存。
- `TTLJobTriggerHandler<S>::ServeHTTP<R>(&self, runtime: &mut R)`：主控制流。约束 `R: TTLHandlerRuntime<Store = S>`，在编译期保证 handler 的 store 类型与运行时所需类型一致。
- `pub fn serve_http<S, R>(handler, runtime)`：自由函数形式的薄转发入口，只调用 `handler.ServeHTTP(runtime)`，不增加任何分支或状态。
- `#![allow(dead_code, non_snake_case)]`：允许当前迁移阶段尚未被所有生产路径使用的符号，并保留 Go 风格的 `NewTTLJobTriggerHandler`、`ServeHTTP` 名称。

## 执行流程

`ServeHTTP` 的执行顺序如下：

1. 调用 `runtime.request_method()`。若结果不是精确字符串 `"POST"`，则由 `method_not_allowed_error` 构造错误，调用 `write_error` 后立即返回。此分支不会读取路径、获取 domain、触发任务或记录成功/失败日志。
2. 分别读取 `path_value("db")` 和 `path_value("table")`，对两个字符串执行 Unicode 小写转换 `to_lowercase()`；随后取得 `request_context()`。
3. 调用 `get_session_domain(&self.store)`。成功值只在本次同步调用中保存；失败时以固定消息 `failed to get session domain` 调用 `log_failure`，再把同一个错误按值传给 `write_error`，然后返回。
4. 调用 `trigger_new_ttl_job(&domain, context, &database, &table)`。成功时先调用 `write_data(&response)`，再以规范化后的库表名和同一完整响应调用 `log_success`；失败时以固定消息 `failed to trigger new TTL job` 记录失败，再写回错误。

`serve_http` 没有不同流程，只提供函数式调用形式。独立测试 `ttl_test.rs` 分别覆盖非 POST、domain 失败、触发失败和触发成功四条路径，并验证失败后不会继续产生后续副作用。

## 数据与状态

handler 自身唯一的持久字段是泛型 `store: S`。`ServeHTTP` 只借用 `&self`，不会替换或修改该字段；是否存在存储内部可变性由具体 `S` 决定。每次调用的 `database`、`table`、`context`、`domain` 和 `response/error` 都是局部变量，不跨请求保留。

运行时以 `&mut R` 传入，因为响应写出、日志、调用计数或底层客户端操作可能改变适配器状态。`ttl_test.rs` 的 `RecordingRuntime` 具体记录 `domain_calls`、`trigger_calls`、写出的响应/错误及成功/失败日志，证明可观察状态全部通过 trait 边界产生。

响应数据不由本文件重塑。`TTLResponse` 保持 client 的完整 `table_result` 向量；成功测试使用包含一个默认表结果的响应，并断言写出与日志得到相同完整值。库表名在进入 domain/TTL 操作前统一小写，这是传给下游和成功日志的不变量。

## 依赖与调用关系

上游调用链在当前 Rust 服务中是：`pkg/server/http_status.rs` 的路由 `/test/ttl/trigger/{db}/{table}` → `ttl_trigger_response` → `NewTTLJobTriggerHandler(())` → `ServeHTTP` → `TtlStatusRuntime` 的 trait 实现。RustCodeGraph 将 `ttl.rs` 标记为被 `pkg/server/http_status.rs` 使用，并识别 `serve_http` 到 `ServeHTTP` 的直接调用边。

`ServeHTTP` 的图内下游调用边全部指向 `TTLHandlerRuntime` 方法：`request_method`、`path_value`、`request_context`、`get_session_domain`、`trigger_new_ttl_job`、`method_not_allowed_error`、`write_error`、`write_data`、`log_success` 和 `log_failure`。因此真实副作用取决于 trait 实现，而不是本文件内的具体库调用。

`Cargo.toml` 声明了 `astersql-kv`、`astersql-server-handler`、`astersql-session`、`astersql-ttl-client` 和 `astersql-util-logutil`。当前 `ttl.rs` 在类型层面直接引用的外部 crate 只有 `astersql-ttl-client`；其余依赖对应 Go 实现中的真实存储、通用 handler、session 和日志边界，当前由 trait 隔离，尚未在本文件中直接使用。

真实 TTL client 的 Rust 函数位于 `pkg/ttl/client/command.rs::trigger_new_ttl_job`：它把库表名封装为 `TriggerNewTtlJobRequest`，以 `TTL_CMD_TYPE_TRIGGER_TTL_JOB` 调用 `CommandClient::command`，再反序列化为 `TriggerNewTtlJobResponse`。本文件并未直接调用该函数，而是要求运行时实现 `trigger_new_ttl_job`；接入真实后端时应在具体运行时适配器内完成这条连接。

## 错误处理与边界

- 方法检查是区分大小写的精确比较；只有 `"POST"` 被接受。错误对象和最终 HTTP 状态码由运行时决定，本文件自身不携带状态码。
- 路径值缺失时，本文件不会单独报参数错误；如果运行时返回空字符串，它仍会把空值传给下游。路由完整性或更早的参数校验属于上游适配器职责。
- `get_session_domain` 和 `trigger_new_ttl_job` 的错误均不包装，日志观察到 `&Error` 后，原值被移动给 `write_error`。trait 未要求 `Error: Clone`、`Display` 或标准错误 trait。
- `write_data` 与日志方法不返回 `Result`，所以本层无法检测序列化、网络写出或日志失败。这与 Go `handler.WriteData` 在此调用点不返回错误的控制流相符。
- 成功路径严格先写响应、后记日志；错误路径严格先记失败日志、后写错误。修改顺序会改变可观察副作用，应同步调整测试并评估与 Go 行为的偏差。
- 当前 Rust `TtlStatusRuntime` 把 handler 错误统一映射为 HTTP 400，并提供模拟结果；不能用它证明真实 domain 获取、命令发送、并发 job 冲突或后端错误映射已经接通。

Go 集成测试 `pkg/server/handler/tests/http_handler_serial_test.go::TestTTL` 进一步说明真实系统边界：成功响应包含 `table_result`，任务最终写入 `mysql.tidb_ttl_job_history`；运行中已有任务可能使首次触发失败并需重试；不存在的表返回 400 和 `table test_ttl.t2 not exists`。这些是 Go 端端到端证据，不是当前 Rust 模拟适配器已实现全部行为的证据。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。`ServeHTTP` 是同步函数，借用 `&self` 和独占借用 `&mut runtime` 直到调用结束；同一个 handler 能否跨线程共享由 `S` 以及外层服务器施加的 `Send`/`Sync` 条件决定，本 trait 和 impl 未主动要求这些界限。

请求上下文由 `request_context` 按值生成并移动进 `trigger_new_ttl_job`，设计意图是让下游传播请求取消或超时；本文件自身不检查取消。domain 只在触发调用期间借用，响应只在写出和成功日志期间借用，随后释放。错误先以共享引用记录，再按值交给响应写出，避免为日志而要求复制错误。

TTL job 的实际异步调度和互斥不在本文件管理。Go 集成测试会轮询历史表等待 running job 结束，说明任务生命周期延伸到 HTTP 返回之后；若接入真实 Rust client，应保持“请求触发、后端异步执行”的边界，不能让 handler 持有长期任务资源。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/handler/ttlhandler/ttl.go`。两版都保存 store，都通过 `NewTTLJobTriggerHandler` 构造，并在 `ServeHTTP` 中依次执行 POST 检查、读取 `db/table`、转小写、取请求上下文、获取 domain、触发 TTL job、写出结果和记录日志。

Go 版使用具体实现：`mux.Vars` 读路径参数，`session.GetDomain(h.store)` 获取 domain，`dom.TTLJobManager().GetCommandCli()` 获取命令客户端，`ttlcient.TriggerNewTTLJob` 发送命令，`handler.WriteError/WriteData` 写 HTTP 响应，`log`/`logutil.Logger(ctx)` 记录日志。Rust 版把这些操作全部放入 `TTLHandlerRuntime`，从而能用 `RecordingRuntime` 做确定性单元测试，但也意味着真实行为只有在具体 runtime 接线后才成立。

可见差异包括：Go 构造函数返回指针，Rust 返回按值 handler；Go handler 方法按值接收器，Rust 用 `&self`；Go 使用 `strings.ToLower`，Rust 使用 Unicode `str::to_lowercase`，对非 ASCII 标识符的转换细节可能不同；Go 的非 POST 错误文本是 `This api only support POST method`，当前 Rust status 适配器构造的是 `This API only supports POST method`，而单元测试替身保留 Go 文本。若错误正文属于兼容契约，应在真实适配器接入时统一。

迁移状态应表述为：handler 分支逻辑和单元测试已经移植，Rust HTTP 路由也已注册；但当前 `TtlStatusRuntime` 是模拟实现，并未复刻 Go 的 `session.GetDomain → TTLJobManager → CommandCli → TriggerNewTTLJob` 真实链路。

## 扩展指南

- 接入真实 Rust 后端时，优先修改 `pkg/server/http_status.rs` 中的 `TtlStatusRuntime` 或引入同职责的生产适配器，实现真实 `Store`、`SessionDomain`、请求上下文和 `trigger_new_ttl_job`；不要把 HTTP/session/client 细节反向塞入本文件，从而破坏现有可测试边界。
- 若增加认证、限流、参数校验或新 HTTP 方法，应先判断它属于通用路由层还是 handler 固有契约。修改 `ServeHTTP` 的分支时，必须在独立的 `pkg/server/handler/ttlhandler/ttl_test.rs` 增加对应回归用例，保持 Rust 源与测试分文件。
- 若改变库表名规范化，需同时检查 Go `strings.ToLower` 的兼容性、非 ASCII 行为、下游查表语义以及成功日志字段；不要只为通过测试删减 Go 逻辑。
- 若改变响应结构，应先在 `pkg/ttl/client/command.rs` 的 `TriggerNewTtlJobResponse` 定义和解析逻辑中建模，再由 `TTLResponse` 继续复用；同步覆盖 handler 的完整响应透传和 HTTP JSON 输出。
- 若让写出或日志可失败，需要重新设计 trait 返回值和错误优先级，并明确“任务已触发但响应写出失败”时是否重试，避免重复 TTL job。
- 性能上本层主要成本是两个路径字符串的分配/小写转换和 runtime 调用。不要为微小分配优化而绕过规范化；真实热点更可能位于 domain/client 和后端调度，应以测量为依据。

## 验证依据

- 源码：`pkg/server/handler/ttlhandler/ttl.rs`，核对 `TTLResponse`、`TTLHandlerRuntime`、`TTLJobTriggerHandler`、`NewTTLJobTriggerHandler`、`ServeHTTP` 和 `serve_http` 的签名、分支及调用顺序。
- crate 边界：`pkg/server/handler/ttlhandler/Cargo.toml` 与 `lib.rs`，核对包名、Go 来源元数据、依赖、公开模块和独立测试模块。
- Rust 上游：`pkg/server/http_status.rs`，核对 `TtlStatusRuntime`、`ttl_trigger_response`、`NewTTLJobTriggerHandler(()).ServeHTTP(...)` 以及 `/test/ttl/trigger/{db}/{table}` 路由注册，并确认当前适配器为模拟后端。
- Rust 下游：`pkg/ttl/client/command.rs`，通过 RustCodeGraph `node` 核对 `TriggerNewTtlJobResponse.table_result` 和真实 `trigger_new_ttl_job` 的请求构造、命令调用与反序列化流程。
- Rust 测试：`pkg/server/handler/ttlhandler/ttl_test.rs`，核对四个分支、调用次数、小写参数、完整响应透传、错误/日志内容和失败后的短路行为。
- Go 对照：`pkg/server/handler/ttlhandler/ttl.go`，核对具体 session/domain/client/HTTP/log 接线；`pkg/server/http_status.go` 核对相同测试路由；`pkg/server/handler/tests/http_handler_serial_test.go::TestTTL` 核对成功结果、job history、运行中任务重试与不存在表错误。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/server/handler/ttlhandler` 列出 `lib.rs`、`ttl.rs`、`ttl_test.rs` 和 Go 对照；文件节点显示 `ttl.rs` 被 `pkg/server/http_status.rs` 使用；`callees` 确认 `ServeHTTP` 到十个 runtime 方法及 `serve_http` 到 `ServeHTTP` 的边。由于同名符号较多，宽泛 callers/callees 查询产生歧义，调用者结论同时以文件节点和 `http_status.rs` 的直接源码为准。
- 本任务为纯文档分析，不运行 Cargo。交付前使用任务规定的结构命令验证目标文件存在且恰有十一个固定二级章节，并人工检查没有把当前模拟适配器描述成真实 TTL 后端。
