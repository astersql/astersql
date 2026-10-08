# `pkg/standby/standby.rs`

## 文件定位

本文件是 `astersql-standby` crate 的核心业务实现，定义按 keyspace 激活的 standby 控制器、管理 HTTP 路由、退出协调与正常重启记录。crate 入口 `pkg/standby/lib.rs` 通过 `pub mod standby` 暴露本模块，并再导出 `LoadKeyspaceController`、`ActivateRequest`、`State`、退出参数解析函数和状态常量；crate 边界与依赖声明见 `pkg/standby/Cargo.toml`。

它通过 `impl StandbyController for LoadKeyspaceController` 接入 `pkg/server/standby.rs` 定义的生命周期接口。实际 Server 主链在 `pkg/server/server.rs`：`Server` 持有 `Arc<dyn StandbyController>`，启动时调用 `wait_for_activate -> prepare_for_activation -> end_standby`，关闭时调用 `on_server_shutdown`。普通 `Server::new` 使用 `NoopStandbyController`；只有调用 `Server::with_standby` 并注入本控制器时，本文件的 standby 行为才会进入 Server 主链。

## 核心职责

1. 维护不可回退的 `Standby -> Activated -> Terminating` 生命周期，并保存本次激活的 keyspace、export ID、空闲阈值、元数据和 DDL/自动分析开关（`State`、`ControllerState`、`activate`）。
2. 暴露 `/tidb-pool/status`、`/activate`、`/exit`、`/checkconn` 四个管理端点，将 JSON/查询参数、Server 健康状态和退出语义映射为 HTTP 响应（`handler` 及四个私有 handler）。
3. 用条件变量协调“收到激活请求”和“Server 已完成监听初始化”两个不同阶段，使 activate 请求只在 Server 准备结果明确后返回（`wait_for_activate`、`wait_server_started`、`prepare_for_activation`、`end_standby`）。
4. 在 Starter 模式关闭时关闭 AutoID 服务、等待连接排空，并按需重试通知 Manager 释放实例（`on_server_shutdown`、`wait_zero_conn`、`report_manager_free`）。
5. 写入、加载和删除 `keyspace:message` 格式的正常重启日志，为 `/checkconn` 判断连接是否因正常重启关闭提供证据（`save_normal_restart_info`、`load_restart_info`、`is_previous_normal_restart`，以及进程级兼容函数）。

## 主要符号

- `ActivateRequest`：激活 JSON 的反序列化模型。`keyspace_name` 必填；其余字段由 Serde 缺省为空、零或 `false`。`metadata` 通过 `activation_metadata` 返回独立克隆，调用方不能借此修改控制器内部状态。
- `State::{Standby, Activated, Terminating}` 与 `State::as_str`：内部枚举及稳定的外部状态字符串。首次 `activate` 只允许从 `Standby` 进入 `Activated`；相同 keyspace 的重复激活幂等成功；不同 keyspace 或终止中的激活失败。
- `LoadKeyspaceController`：可克隆句柄，内部以 `Arc<ControllerInner>` 共享所有状态；`new` 使用可记录信号的默认替身，`with_exit_signaler` 允许注入实际退出实现或测试替身。
- `ControllerInner` / `ControllerState`：前者保存互斥锁、条件变量、原子量、Manager/退出接口和日志路径；后者把生命周期状态、激活请求、Server 启动结果及激活超时放在同一把锁下，保证这些字段的组合观察一致。
- `ExitSignaler`、`ExitSignal`、`RecordingExitSignaler`：将进程信号副作用抽象为 `Interrupt`/`Terminate`；记录实现的 `take` 会取出并清空最近信号。
- `ManagerClient`：关闭后调用 Manager `free(exit_reason)` 的最小边界。
- `ExitOptions`、`parse_exit_options`、`parse_exit_wait`：解析优雅退出、等待上限、AutoID owner 跳过和 Manager free 标志。等待支持 Go duration 形式及兼容的纯秒整数，最大为 24 小时。
- `StandbyController` 实现：`wait_for_activate`、`end_standby`、trait 版 `handler`、`on_connection_active`、`prepare_for_activation`、`on_server_created`、`on_server_shutdown` 是 Server 生命周期接点。
- `save_tidb_normal_restart_info`、`load_previous_restart`、`is_previous_tidb_normal_restart`：使用全局 `PREVIOUS_RESTART` 的兼容 API；它与每个控制器实例的 `previous_restart` 缓存并存，调用者需要选择同一套 API，不能假设二者自动同步。

## 执行流程

激活主流程如下：

1. `handler` 注册 `/tidb-pool/activate`。`activate_http` 用 `parse_activation_request` 解码 JSON 并要求非空 `keyspace_name`；已非 standby 且 Server 不健康时先返回 503。
2. `activate` 在状态锁内完成转换并保存请求，随后 `activated.notify_all()` 唤醒 Server 线程。同 keyspace 重试不重复改变状态，不同 keyspace 返回前置条件失败。
3. Server 的 `run` 路径在 `wait_for_activate` 上阻塞；被唤醒后调用 `prepare_for_activation`，后者执行 `StandbyReadyServer::init_tidb_listener`，把同一结果传给 `end_standby`。
4. `end_standby` 以 `end_once` 保证只发布一次结果，写入 `server_start_result` 并唤醒 `server_started`。仍在 `wait_server_started` 的 HTTP 请求据此返回状态 JSON、408 超时或 500 初始化错误。

退出主流程从 `/tidb-pool/exit` 开始。`exit_http` 先比较 query 中的 `keyspace` 与 `local_keyspace`，再解析选项。Starter 模式下，缺少所需 Manager 返回 503；命中 `skip_auto_id_owner` 返回空 304；非优雅退出设置强制关闭、写日志并请求 `Interrupt`；优雅退出保存等待时长（零值替换为八小时默认值），并按需标记稍后通知 Manager。普通结尾在 Starter 请求 `Terminate`，非 Starter 请求 `Interrupt`。

Server 关闭时，`on_server_shutdown` 仅在 Starter 生效：状态改为 `Terminating`，先关闭 AutoID；强制关闭直接返回，否则按配置等待连接归零。只有连接成功排空且 Server 标记需要通知时，才读取重启日志内容并调用 `report_manager_free`，最多尝试三次、失败间隔 200 毫秒。

`/tidb-pool/checkconn` 要求同时提供 `keyspace_name` 和 `conn_id`。它先查 Server 的正常关闭连接缓存，再查控制器启动时缓存或磁盘中的正常重启信息；命中返回 `normal closed`，无证据返回 `unconfirmed`。

## 数据与状态

- `ControllerState.state`、`activation`、`server_start_result`、`activation_timeout` 共用 `Mutex`。状态转换和激活内容写入因此是一个临界区；`State` 没有回到 `Standby` 的路径。
- `activated` 只表示激活状态已发生；`server_started` 表示监听初始化已有成功或失败结果。二者不能互换，否则 activate HTTP 可能在 Server 真正可接受连接前返回。
- `end_once: AtomicBool` 保证首个 `end_standby` 结果获胜。后续调用被忽略，避免覆盖已经向等待者发布的结果。
- `close_connection_wait_millis: AtomicU64` 保存等待毫秒数；超大 `Duration` 写入时饱和到 `u64::MAX`。`last_active: AtomicI64` 通过 `fetch_update` 只前进不回退，系统时钟早于 Unix epoch 时 `unix_seconds` 返回 0。
- `local_keyspace`、`previous_restart` 分别由独立 `Mutex` 保护；`restart_log_path` 构造后不可普通修改，`with_restart_log_path` 要求 `Arc` 尚未共享，否则会 panic。
- 重启日志是单条 `keyspace:message` 文本，读取时只在第一个冒号处分割，因此 message 可以继续包含冒号。找不到文件视为没有记录；格式错误、读写或删除失败会返回字符串错误。

## 依赖与调用关系

上游主链是 `pkg/server/server.rs::Server::{with_standby, run, close}` 通过 `pkg/server/standby.rs::StandbyController` 动态调用本控制器。`Server::run` 依次等待激活、初始化监听并发布结果；`Server::close` 在关闭监听和连接前调用关闭钩子。`handler` 返回的 `Router` 和路径前缀由 Server 状态 HTTP 层安装，Server 同时以 `StandbyReadyServer` 和 `StandbyShutdownServer` 向控制器提供监听、健康、连接、AutoID 与关闭标志能力。

直接下游依赖包括：

- `astersql_server::http_status::{Request, Response, Router, serve_error}`：路由注册和 HTTP 响应。
- `astersql_server::standby` 三个 trait：Server 生命周期与能力边界。
- `astersql_config_deploymode::IsStarter`：决定 export ID、退出信号和 Starter 关闭分支；`set_starter_mode` 是显式覆盖，主要供测试或特殊接线使用。
- `serde` / `serde_json`：激活请求解码和 JSON 字符串转义。
- 标准库 `Mutex`、`Condvar`、原子类型、文件系统和线程：同步、持久化及 Manager 重试。

RustCodeGraph 将本文件列为 94 个符号，并显示文件级直接使用者包含 `pkg/planner/core/planbuilder.rs` 与 `pkg/server/handler/tikvhandler/dxf.rs`；文本核验未发现这些文件直接构造 `LoadKeyspaceController`，主要原因是它们引用本 crate 的其他再导出状态常量。控制器本身的真实运行时关系应以上述 `StandbyController` trait 和 `pkg/server/server.rs` 调用点为准，而不是把文件级依赖误当成控制器调用边。

## 错误处理与边界

- 激活 JSON 无法解析或 keyspace 为空返回 400；终止中或不健康返回 503；已激活到另一 keyspace 返回 412；等待超时返回 408；Server 初始化失败返回 500。
- `activation_timeout == 0` 表示无限等待。当前 API 不接收请求取消信号；这与 Go handler 能感知 `r.Context().Done()` 并触发退出的行为不同。
- exit 的 keyspace 不匹配返回 412，并输出经过 JSON 转义的 remote/local；非法布尔值、负数、超过 24 小时或无法解析的 wait 返回带换行的 400。
- `parse_go_duration` 支持 `ns`、`us`、`µs`、`ms`、`s`、`m`、`h` 及小数，但基于 `f64` 和 `Duration::from_secs_f64`；极端数值可能触发标准库 panic，当前公开解析入口只显式限制最终值不超过 24 小时。
- 多个锁均用 `expect` 处理 poisoned mutex，因此持锁线程 panic 后后续访问也会 panic，而不是转换成业务错误。
- 写正常重启日志失败在独立保存 API 中返回错误，但 exit 流程故意以 `let _ = ...` 忽略写失败并继续发退出信号。Manager free 最终失败只返回 `false`，关闭钩子不再向上抛错。
- `checkconn` 只根据 Server 缓存或重启记录确认“正常关闭”，不会证明连接当前仍存在；无法确认时明确返回 `unconfirmed`。

## 并发与资源生命周期

`LoadKeyspaceController` 的克隆共享同一个 `Arc<ControllerInner>`，适合 HTTP handler 与 Server 线程并发持有。激活线程在 `activated` 条件变量上循环检查谓词，以容忍虚假唤醒；HTTP 线程在 `server_started` 上同样循环或带超时等待。通知发生前都先在状态锁内写入谓词，因此不会因先通知后等待而丢失状态。

原子字段采用 Acquire/Release 或 AcqRel：Starter 覆盖、关闭等待和最后活跃时间可无锁读写；`end_once.swap` 建立首个结果发布边界。`report_manager_free` 在当前线程同步重试并 `sleep`，最多额外阻塞约 400 毫秒（不计 `ManagerClient::free` 自身耗时）。连接排空的阻塞与超时职责下放给 `StandbyShutdownServer::wait_zero_connections_timeout`。

重启文件生命周期是“退出前覆盖写入，下一次 `wait_for_activate` 启动时读取到实例缓存并删除”。如果读取成功但删除失败，当前实现忽略 `wait_for_activate` 中的整体错误，既不写缓存也不报告失败；进程级 `load_previous_restart` 则会把删除错误返回给调用方。正常关闭时 AutoID 在等待流量迁移前关闭；强制退出跳过连接等待和 Manager free。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/standby/standby.go`，Rust 的请求字段、三态字符串、四个 HTTP 端点、同 keyspace 激活幂等、24 小时 wait 上限、八小时默认连接等待、Starter 退出分支、AutoID 关闭顺序、三次 Manager free 重试和重启日志格式均对应 Go 逻辑。`pkg/standby/standby_test.go`、`standby_nextgen_test.go` 的关键意图由 Rust 独立测试 `standby_test.rs`、`standby_nextgen_test.rs` 覆盖。

当前并非逐项等价，扩展时必须注意：

- Go 使用包级 `mu/state/activateRequest/activateCh`，Rust 把状态收进每个控制器实例；多个 Rust 控制器不会共享激活状态。
- Go `WaitForActivate` 自己创建临时状态 HTTP server、注册 `/status` 与 `/health`、配置 TLS，并在激活后把 keyspace、export ID、空闲阈值、DDL 和自动分析设置写回全局配置。Rust `wait_for_activate` 只加载重启记录并等待条件变量，本文件没有临时监听或全局配置回写；这些能力不能视为已移植。
- Go activate 会处理客户端取消和超时后触发进程退出；Rust 只返回 408，不在超时分支请求退出，也没有请求上下文取消。
- Go `EndStandby` 还关闭临时 HTTP server；Rust 无对应临时 server，只发布启动结果。Server 主链又在 `prepare_for_activation` 后调用一次 `end_standby`，但 `end_once` 使第二次调用安全无效。
- Go Manager free 带统一 context timeout；Rust trait 没有 context，整体耗时取决于每次 `free` 实现，只有重试次数与间隔有界。
- Go 的重启读写错误主要记录日志；Rust 多数辅助函数返回 `Result<String>`，但具体 HTTP/关闭路径仍会选择忽略部分错误。

## 扩展指南

- 新增激活字段：同步修改 `ActivateRequest`、消费该字段的激活后接线、状态响应（若需公开）以及 `standby_test.rs` 的 JSON/克隆测试；同时对照 Go `ActivateRequest` 和 `WaitForActivate` 的配置写回语义，避免只做到可反序列化却没有运行效果。
- 新增状态或状态转换：集中修改 `State`、`as_str`、`activate`、`status_http_response` 和关闭钩子，并为并发等待、重复请求及非法转换添加独立测试。不要绕过 `ControllerState` 的互斥锁直接拼接第二套状态。
- 新增 HTTP 管理端点：在 `handler` 注册，在私有方法中完成输入验证和状态码映射；所有动态 JSON 必须使用 Serde 或 `json_escape`。测试应放在同目录独立文件 `standby_test.rs` 或 `standby_nextgen_test.rs`，不要内嵌到生产源文件。
- 改退出语义：同时检查 `ExitOptions`/解析器、`exit_http`、`on_server_shutdown`、`StandbyShutdownServer` 能力接口和 `pkg/server/server.rs::close` 的调用顺序。需特别验证强制/优雅、Starter/非 Starter、AutoID owner、连接超时和 Manager 缺失的组合。
- 改重启记录：保持跨进程可读格式兼容，或者同时迁移 Go 读写方与两套 Rust API；使用临时路径测试读取、删除、格式错误和 keyspace 不匹配，避免触碰默认 `/tmp` 文件。
- 性能与兼容风险主要在长时间持有状态锁、同步 Manager 调用、等待连接实现和 duration 精度。任何把阻塞工作移入锁内的改动都可能阻塞 status/activate；任何状态码或正文换行变化都可能破坏管理端兼容。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`pkg/standby/standby.rs` 被识别为 94 个符号。使用了 `files --filter pkg/standby`、按文件 `node`、`query LoadKeyspaceController/wait_for_activate/prepare_for_activation/on_server_shutdown` 以及 `callers/callees` 查询。
- 源码事实：`pkg/standby/standby.rs`（常量、数据结构、状态机、HTTP、同步、日志与辅助解析）；`pkg/server/standby.rs`（三个 trait 与默认 Noop）；`pkg/server/server.rs`（控制器持有、启动与关闭调用点）；`pkg/standby/lib.rs`（模块和再导出）。
- crate 事实：`pkg/standby/Cargo.toml` 确认 crate 名为 `astersql-standby`、库入口为 `lib.rs`，并声明 server、deploymode、Serde 等依赖；metadata 指向 Go 包 `pkg/standby`。
- Go 对照：`pkg/standby/standby.go` 的 `Handler`、`WaitForActivate`、`PrepareForActivation`、`EndStandby`、`OnServerShutdown`、`parseExitWait`、`reportManagerFree` 和正常重启辅助函数。
- 测试依据：`pkg/standby/standby_test.rs` 验证元数据克隆、必填 keyspace、激活等待 Server、非 Starter 行为、状态 JSON、健康拒绝、checkconn 优先级及重启记录加载删除；`pkg/standby/standby_nextgen_test.rs` 验证 304 owner 跳过、连接排空成功/超时、Manager 重试、Starter export ID、非法退出参数、默认优雅等待、Manager 缺失及 duration 边界。对应 Go 测试为 `pkg/standby/standby_test.go` 与 `pkg/standby/standby_nextgen_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。最终以固定十一个二级标题的结构检查和人工逐项回查上述符号、调用边及 Go/Rust 差异作为完成证据。
