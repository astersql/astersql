# `pkg/server/rpc_server.rs`

## 文件定位

本文件属于 `astersql-server` crate；crate 根在 `pkg/server/lib.rs`，其中以 `pub mod rpc_server` 暴露本模块，crate 清单 `pkg/server/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一边界。它定义一层与具体 gRPC 库解耦的 RPC 服务模型，覆盖单次/流式 Coprocessor、批量命令以及 MPP 任务状态上报，并通过 trait 注入会话、执行器、流和协调器。

当前 Rust 接线范围有限：仓库内除 `pkg/server/rpc_server_test.rs` 外，没有 `RpcServer::new` 或这些公开处理方法的 Rust 调用点；因此这里是可测试的服务核心与移植接口，并非已经监听端口、注册 protobuf service 的生产传输层。生产 Go 主链仍由 `pkg/server/http_status.go::startStatusServerAndRPCServer` 调用 `pkg/server/rpc_server.go::NewRPCServer` 创建和启动真正的 `grpc.Server`。

## 核心职责

- `RpcServerConfig::from(&StatusConfig)` 把 `pkg/server/server.rs::StatusConfig` 中与 gRPC 传输相关的保活时间、并发流数、窗口和最大发送消息大小投影为本模块配置；最小 ping 间隔固定为 5 秒。
- `RpcServer` 保存共享的 `Domain`、`RpcSessionFactory`、`CoprocessorExecutor` 和 `MppCoordinator`，为各处理入口提供依赖注入，而不直接构造数据库会话或绑定网络协议。
- `coprocessor`/`coprocessor_stream` 将执行错误或 panic 转成 `CoprocessorResponse::other_error`，使单个请求故障不会直接展开穿过 RPC 边界。
- `batch_commands` 按批次顺序处理 `Coprocessor` 和 `Empty` 两类命令，保留输入的 `request_ids`，并为每批发送一个响应。
- `SessionGuard` 以 RAII 保证已创建会话最终执行 `close`；单次 Coprocessor 路径还会在关闭前分离内存跟踪器。
- `report_mpp_task_status` 仅做同步转发，不在本文件内保存 MPP 状态。

## 主要符号

- 数据传输类型：`CoprocessorRequest`/`CoprocessorResponse`、`BatchCommand`/`BatchRequest`、`BatchResponseItem`/`BatchResponse`、`MppTaskStatusRequest`/`MppTaskStatusResponse`。它们是当前 Rust 自有模型，不是 kvproto 生成类型；批请求的 ID 与命令分别存放，代码没有检查两者长度一致。
- 会话抽象：`RpcSession` 定义对端地址、内存跟踪初始化/分离和关闭；`RpcSessionFactory::create(Arc<dyn Domain>)` 是会话创建边界。两者均要求跨线程传递能力，工厂还要求共享能力。
- 执行抽象：`CoprocessorExecutor::execute` 是单次执行入口；默认 `execute_stream` 调用一次 `execute` 后调用 `CoprocessorStream::send`，实现者可覆盖为真正的多响应流。
- 传输抽象：`CoprocessorStream::send` 代表服务端流输出；`BatchCommandStream::receive/send` 代表双向批流。trait 本身未要求 `Send`，生命周期与线程模型由调用方负责。
- MPP 抽象：`MppCoordinator::report_status` 接收只读请求并返回结果。
- 配置：`RpcServerConfig` 及其 `From<&StatusConfig>` 实现。`RpcServer::config` 和 `diagnostic_log_file` 对外可读，但本文件未消费它们来配置真实服务器或诊断服务。
- 生命周期实现：私有 `SessionGuard::{session, detach_memory_tracker_on_drop}` 与 `Drop::drop`。
- 服务入口：`RpcServer::{new, coprocessor, coprocessor_stream, batch_commands, report_mpp_task_status}`；私有辅助为 `create_session`、`handle_coprocessor` 和模块函数 `panic_text`。
- 本文件没有模块级常量、条件编译项或本地测试模块；测试由 `pkg/server/lib.rs` 通过 `#[path = "rpc_server_test.rs"]` 独立挂载。

## 执行流程

1. 组装：`RpcServer::new` 从 `StatusConfig` 生成 `RpcServerConfig`，保存诊断日志路径和四个 `Arc<dyn ...>` 依赖，返回 `Arc<RpcServer>`。
2. 单次 Coprocessor：`coprocessor` 在 `catch_unwind(AssertUnwindSafe(...))` 中调用 `handle_coprocessor`。后者经 `create_session` 创建会话并初始化内存跟踪器；若请求带 `peer_address`，写入会话；随后标记守卫在析构时先 detach，再调用执行器。正常响应原样返回，普通 `Err(String)` 和 panic 都成为空 payload、带 `other_error` 的响应。
3. 流式 Coprocessor：`coprocessor_stream` 在 panic 边界内创建会话。创建失败会立即向流发送一个错误响应；成功后调用 `CoprocessorExecutor::execute_stream`。离开闭包时守卫关闭会话，但该路径没有设置 peer address，也没有启用 detach 标志。panic 则在边界外尝试向流发送错误响应。
4. 批量命令：`batch_commands` 反复 `receive`，直至得到 `None`。每批按 `requests` 顺序映射：`Coprocessor` 复用上述单次入口，`Empty` 回显 `test_id`；然后把原 `request_ids` 与结果列表交给 `send`。receive/send 的普通错误通过 `?` 返回，panic 被捕获后返回 `Ok(())`。
5. MPP：`report_mpp_task_status` 直接调用注入的 `MppCoordinator::report_status`，没有 panic 转换或额外校验。

## 数据与状态

`RpcServer` 自身构造后不修改字段；共享依赖均由 `Arc` 持有，因此 clone 服务句柄只增加引用计数。请求、响应和配置采用拥有所有权的 `String`/`Vec<u8>`/`Vec<_>`，本文件不保存请求级状态。`BatchRequest` 被按值消费，`request_ids` 被移动进响应，而请求列表被逐项消费；顺序保持不变，但没有 request ID 与响应数量匹配的不变量检查。

会话状态由 `SessionGuard` 独占一个 `Box<dyn RpcSession>`。`create_session` 仅在工厂成功后调用 `initialize_memory_tracker`；若初始化方法内部 panic，尚未构造守卫，因而本模块不能保证调用 `close`。守卫正常构造后，无论执行器返回错误还是 panic 展开到 `catch_unwind`，`Drop` 都会执行。`detach_memory_tracker` 标志默认是 `false`，只有 `handle_coprocessor` 在执行前将其设为 `true`。

`diagnostic_log_file`、`CoprocessorRequest::request_id`、`MppTaskStatusRequest::{task_id,state}` 的解释和使用均委托给外层或注入实现；本文件只是保存或转发。没有锁、全局单例、缓存或内部任务队列。

## 依赖与调用关系

上游边界如下：

- `pkg/server/lib.rs` 公开模块，并在测试配置下挂载 `pkg/server/rpc_server_test.rs`。
- 当前生产 Rust 源码没有调用 `RpcServer::new`、`coprocessor`、`coprocessor_stream`、`batch_commands` 或 `report_mpp_task_status`；RustCodeGraph 的符号调用查询未给出这些方法的生产调用边，`rg` 复核只找到独立测试中的直接调用。
- Go 生产上游是 `pkg/server/http_status.go::startStatusServerAndRPCServer -> NewRPCServer -> grpcServer.Serve`，这是 Go 实现的接线证据，不能视为 Rust 调用边。

下游关系如下：

- `RpcServerConfig::from -> StatusConfig`，类型来自 `pkg/server/server.rs`；本文件直接使用的 crate 内 Rust 类型只有 `Domain` 和 `StatusConfig`。
- `create_session -> RpcSessionFactory::create -> RpcSession::initialize_memory_tracker`。
- `handle_coprocessor -> RpcSession::set_peer_address -> CoprocessorExecutor::execute`，离开作用域后 `SessionGuard::drop -> detach_memory_tracker（按标志） -> close`。
- `CoprocessorExecutor::execute_stream` 默认实现为 `execute -> CoprocessorStream::send`；`coprocessor_stream` 通过该虚调用允许实现者替换流式语义。
- `batch_commands -> BatchCommandStream::{receive,send}`，其中 Coprocessor 分支回调 `RpcServer::coprocessor`。
- `report_mpp_task_status -> MppCoordinator::report_status`。

`pkg/server/Cargo.toml` 将本模块编入 `astersql-server`，但本文件只使用标准库和 crate 内抽象，没有直接引用清单中的 gRPC/kvproto 依赖。这也说明当前 Rust 层没有完成 Go `grpc.NewServer`、protobuf service 注册或网络 Serve 的等价接线。

## 错误处理与边界

- trait 边界统一使用 `String` 作为可恢复错误，缺少结构化错误分类和错误源链。单次入口把会话创建/执行错误写入 `other_error`；流式入口把创建错误写入流，并保留流发送失败作为返回错误。
- `coprocessor` 与 `coprocessor_stream` 用 `AssertUnwindSafe` 强制跨越可变 trait object 的 unwind-safety 检查。panic 文本由 `panic_text` 提取：支持 `&str` 和 `String`，其他载荷统一为 `unknown panic payload`。
- 单次与流式 panic 都会尝试形成 `other_error`；流式 panic 后的 `stream.send` 仍可能失败并返回 `Err`。批流 panic 则被静默转为 `Ok(())`，没有响应或日志，这是与 Go 版“记录日志并返回零错误值”只部分等价的地方。
- `batch_commands` 把 `receive` 返回的 `None` 视为正常 EOF；普通 receive/send 错误直接返回。它没有 Go 版的 unknown-command 分支，因为 Rust 的封闭枚举只允许两种命令。
- `SessionGuard::session` 包含 `expect`，但在现有实现中 `session` 构造后从未 `take`，所以触发代表守卫不变量被未来修改破坏。
- MPP 转发不捕获 panic；配置转换不做范围验证。`max_send_message_size` 等字段也尚未应用到具体传输实现。

## 并发与资源生命周期

`RpcServer` 通过 `Arc` 共享，并要求 `Domain`（定义于 `pkg/server/server.rs`）、`RpcSessionFactory`、`CoprocessorExecutor` 和 `MppCoordinator` 满足相应的 `Send`/`Sync` 约束；这允许服务依赖被并发请求共享，但具体同步安全由实现者保证。本文件没有创建线程、异步任务或通道，所有入口都是同步调用；`batch_commands` 会独占传入的可变 stream，并串行处理每批及批内命令，没有并行度或背压队列。

会话是每个 Coprocessor 调用新建：单次路径的预期释放顺序为 `initialize -> 可选 peer -> execute -> detach -> close`，由 `pkg/server/rpc_server_test.rs::unary_detaches_memory_tracker_before_closing_session` 验证。流式路径为 `initialize -> execute_stream -> close`，测试 `stream_closes_session_without_unary_peer_or_detach_side_effects` 明确证明当前不设置 peer、也不 detach；这可能是有意保留 Go 行为差异，也可能是待补齐的迁移缺口，扩展时不能擅自假定二者一致。

panic 展开期间，已构造的 `SessionGuard` 仍按 Rust RAII 析构。`Arc` 只管理依赖对象生存期，不终止流或协调关闭；实际 RPC transport、取消信号、deadline 和客户端断连均不在本文件建模。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/rpc_server.go`：

- `NewRPCServer` 对应 `RpcServer::new` 加 `RpcServerConfig::from` 的部分配置语义。两者都使用 keepalive time/timeout、5 秒最小 ping 间隔、最大并发流、初始窗口和最大发送消息；但 Go 同时创建 `grpc.Server`、注册 Diagnostics/TiKV/TopSQL service，Rust 只保存配置与诊断日志路径。
- Go `rpcServer` 内嵌生成的 `tikvpb.TikvServer` 并保存真实 `domain.Domain`、session manager；Rust 改为小型 trait 和 `Arc<dyn ...>` 注入，便于独立测试，但其 DTO 并非 protobuf 类型。
- Go `Coprocessor -> handleCopRequest` 对应 Rust `coprocessor -> handle_coprocessor`：都创建会话、写入 peer、执行 DAG handler，并把 panic/创建错误放入 `OtherError`。Go `createSession` 还完成 InfoSchema、权限、扩展、聚合并发、内存动作、max_allowed_packet 和 session manager 等大量初始化；Rust 只调用工厂和 `initialize_memory_tracker`，这些语义是否存在取决于工厂实现，而仓库当前没有生产实现证据。
- Go `handleCopRequest` 在 defer 中 detach statement memory tracker 并关闭会话；Rust单次路径由 `SessionGuard` 保持同样顺序。Go `CoprocessorStream` 只 close、不设置 peer、不 detach，Rust 当前与此一致。
- Go `BatchCommands` 支持 protobuf 的未知命令回退为空响应并持续循环；Rust 通过封闭 `BatchCommand` 枚举消除了未知变体。两者都串行调用单次 Coprocessor并回传 request IDs；Go receive 遇 EOF 也作为错误记录/返回，而 Rust `None` 是显式正常结束。
- Go `ReportMPPTaskStatus` 调用全局 `InstanceMPPCoordinatorManager`；Rust通过注入 `MppCoordinator` 转发，避免硬编码全局实例。

Go 同路径不存在独立 `pkg/server/rpc_server_test.go`。相关 Go 证据主要来自生产接线 `pkg/server/http_status.go`，以及会创建 `NewRPCServer` 的 `pkg/ddl/tiflash_replica_test.go`、`pkg/executor/cluster_table_test.go`、`pkg/executor/infoschema_cluster_table_test.go` 和 `pkg/infoschema/test/clustertablestest/cluster_tables_test.go`；这些是跨模块集成使用证据，不等价于对本 Rust 实现的测试覆盖。

## 扩展指南

- 接入真实 Rust RPC transport 时，应在独立适配层把 protobuf 请求/响应和 stream 映射到本模块 trait，并将 `RpcServerConfig` 真正应用到服务器 builder；不要在这里混入监听 socket 或生成代码。需要新增独立测试文件或扩展现有 `pkg/server/rpc_server_test.rs`，同时在 `pkg/server/lib.rs` 保持测试模块接线。
- 扩展会话初始化时优先实现 `RpcSessionFactory`/`RpcSession`，并逐项核对 Go `createSession` 的 InfoSchema、权限、扩展、内存限制、系统变量与 session manager 语义；不能把当前两方法的简化接口宣称为已完整移植。
- 修改单次资源清理时，应保持 `detach` 先于 `close`，补充执行器返回错误和 panic 两条回归测试。若让流式路径也 detach 或设置 peer，这是可观察行为变化，应先核对 Go 版本并更新 `stream_closes_session_without_unary_peer_or_detach_side_effects`。
- 新增批命令变体时，同时扩展 `BatchCommand`、`BatchResponseItem` 和 `batch_commands` 映射，明确 request ID 对齐规则，并测试 receive 错误、send 错误、正常 EOF、panic 和多批顺序。若引入未知命令表示，还应对齐 Go 的空响应回退。
- 修改 panic 策略时分别审视三种入口：unary 返回错误响应、stream 发送错误响应、batch 吞掉 panic 并返回成功；尤其要评估 transport 是否会误判批流正常结束。
- 扩展 MPP 时接入 `MppCoordinator`，并为 accepted/error、协调器异常和并发调用增加独立测试；当前测试 `TestMpp` 只返回默认值，未直接断言转发内容。
- 兼容风险集中在 protobuf 字段/错误文本、Go 会话初始化缺口和流终止语义；性能风险集中在批内严格串行、新建会话成本、payload clone（默认流执行）和无背压抽象。任何生产接线都应另行验证并发、取消、消息上限和资源释放。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/server/rpc_server.rs --offset 1 --limit 260` 与 `--offset 220 --limit 140` 覆盖目标文件全部 337 行；对 `RpcServer::{coprocessor,batch_commands,report_mpp_task_status}` 的 callers/callees 查询未返回可用的符号级生产边，因此再以仓库精确搜索确认接线范围。
- 源码：`pkg/server/rpc_server.rs`（全部类型、trait、配置转换、守卫和五个公开服务方法）；`pkg/server/server.rs::{StatusConfig,Domain}`（直接类型依赖）；`pkg/server/lib.rs`（公开模块及独立测试挂载）。
- crate：`pkg/server/Cargo.toml`（`astersql-server`、`lib.rs` crate 根、`autotests = false` 和包级依赖边界）。
- Rust 测试：`pkg/server/rpc_server_test.rs::{unary_detaches_memory_tracker_before_closing_session,stream_session_creation_error_is_sent_as_other_error,stream_closes_session_without_unary_peer_or_detach_side_effects,batch_command_panic_is_recovered_without_transport_error}`。
- Go 对照：`pkg/server/rpc_server.go::{NewRPCServer,Coprocessor,CoprocessorStream,BatchCommands,handleCopRequest,createSession,ReportMPPTaskStatus}`；生产入口 `pkg/server/http_status.go::startStatusServerAndRPCServer`；上述跨包 Go 测试中的 `NewRPCServer` 调用。
- 人工边界复核：Rust 当前没有真实 gRPC 注册/监听、生产会话工厂、protobuf 类型适配、Diagnostics/TopSQL 注册或专门 Go 单元测试证据；文档均按“未接入/未在本文件实现”表述，没有用 Go 能力代替 Rust 现状。
