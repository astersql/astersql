# `pkg/distsql/distsql.rs`

## 文件定位

本文件是 `astersql-distsql` crate 的发送与结果包装层。crate 入口 `pkg/distsql/lib.rs` 以 `pub mod distsql` 暴露它；请求的完整结构与构造逻辑位于 `pkg/distsql/request_builder.rs`，响应的解码、统计归集和关闭逻辑位于 `pkg/distsql/select_result.rs`。因此这里不规划 SQL、也不直接实现 TiKV/TiFlash 网络协议，而是把已经构造好的 `KvRequest` 交给 `KvClient`，再为返回的 `ResponseSource` 配置 `selectResult` 及其统计上下文。

`pkg/distsql/Cargo.toml` 表明该文件属于 `astersql-distsql`，本文件直接使用的 crate 依赖是 `astersql-config`、`astersql-kv` 和 `astersql-util-execdetails`。RustCodeGraph 将该文件识别为 45 个符号，并显示直接使用者集中在同 crate 的 `request_builder.rs`、`select_result.rs` 及三份独立测试文件；仓库搜索没有发现 `Select`、`Analyze`、`Checksum` 等发送入口被 Rust 生产代码直接调用，故当前实现主要是可复用 API 与已覆盖测试的迁移边界，不能据此声称 Rust SQL 执行主链已经完整接线。

## 核心职责

- 用 `KvClient` 抽象实际传输，用 `ResponseSource` 抽象分批响应，隔离本文件与具体 KV 客户端实现。
- 用 `DistSQLSelectResult` 保存 DAG、Analyze、Checksum、MPP 的标签、字段数、计划 ID、存储类型和分页信息，并把 `SelectResult` 操作委托给内部 `selectResult`。
- 在 `Select`、`SelectWithRuntimeStats`、`Analyze`、`Checksum` 与 `GenSelectResultFromMPPResponse` 中建立不同请求类型所需的结果统计上下文。
- 把查询级 coprocessor store limiter 注入发送副本，避免修改调用方持有的原请求，也不覆盖独立的 `copr_request_limiter`。
- 生成 TiFlash 出站 metadata，选择 DAG 的 Default/Chunk 编码及 chunk 字节序。
- 提供 `KvExecCounter`/`RequestContext` 的最小拦截器绑定模型；它只保存计数器，真正调用 `on_request` 的传输拦截逻辑不在本文件中。

## 主要符号

- `ResponseSource for Box<dyn ResponseSource>`：将 `next_response`、`close`、带错误取响应、未消费统计和 limiter 等待统计逐项委托给盒内对象，使 `selectResult<Box<dyn ResponseSource>>` 满足统一泛型约束。
- `KvClient: Send + Sync`：发送边界。必需方法 `send(&KvRequest)` 返回 `Box<dyn ResponseSource>`；`send_with_options` 默认忽略 `ClientSendOptions` 并回退到 `send`，需要消费 Analyze 执行信息开关的客户端必须覆写它。
- `ClientSendOptions`：目前只有 `enable_collect_execution_info`，由 `Analyze` 在发送时从全局配置快照生成。
- `DistSQLSelectResult`：公开元数据包装器；`inner` 私有并负责实际拉取、解码、统计和关闭。其 `SelectResult` 实现不改变语义，只委托 `NextRaw`、`Next`、`IntoIter`、`Close` 和 `concurrency`。
- `new_select_result`：公共发送入口的共同构造器，初始化标签、行宽、SQL 类型、存储类型、分页，并把计划 ID 置为空/零。
- `DistSQLContext`：发送期依赖与会话设置的快照，包括 `client`、查询 limiter、执行明细、运行时统计、内部 SQL 标记、chunk 开关及 TiFlash 配额。`streaming`、`concurrency` 字段在本文件的发送流程中没有被读取，实际结果并发取自 `KvRequest::concurrency`。
- `GenSelectResultFromMPPResponse`：不发送请求，直接将现有 MPP `ResponseSource` 包装为 TiFlash 结果，并安装计划 ID 与动态 `reports_directly` 回调。
- `Select` / `SelectWithRuntimeStats`：DAG 发送入口；后者在前者结果上补充 root/cop plan ID 并重建统计上下文。
- `Analyze` / `Checksum`：统计与校验发送入口。两者按调用约定命名结果，但当前 Rust 实现不重新校验 `KvRequest` payload 类型。
- `SetTiFlashConfVarsInContext`：把线程、外部落盘阈值、内存配额、spill ratio 和 hash join v2 开关追加到 `(String, String)` metadata。
- `EncodeType`、`Endian`、`DAGRequest` 及 `SetEncodeType` 辅助函数：描述并选择 chunk RPC 编码。
- `KvExecCounter`、`RequestContext`、`WithSQLKvExecCounterInterceptor`：计数器注入的简化接口。

## 执行流程

1. 调用方先通过 `request_builder::RequestBuilder` 得到 `KvRequest`，再选择本文件的发送入口。
2. `Select` 克隆请求；若 `DistSQLContext::query_cop_store_limiter` 存在，仅向克隆写入同一个 `Arc`，随后调用 `context.client.send`。成功后用请求的 `concurrency`、`store_type`、`paging` 和字段数构造 `dag` 结果，并安装普通查询的 `ResultStatsContext`。
3. `SelectWithRuntimeStats` 先完整执行 `Select`；只有发送成功后才写入 `cop_plan_ids`、`root_plan_id`，并用这些 ID 替换内部统计上下文，使后续消费响应时能够归集到对应计划节点。
4. `Analyze` 克隆请求并将 `request_source` 强制设为 `stats`；读取 `astersql_config::get_global_config().instance.enable_collect_execution_info`，通过 `send_with_options` 发送。结果标为 Analyze，保存 `plan_id`，并让 `collect_raw_details` 与发送时快照一致。
5. `Checksum` 直接用传入请求调用 `send`，构造 `checksum/general` 结果；它没有附加统计上下文。
6. MPP 路径由上游提供已经建立的响应源；`GenSelectResultFromMPPResponse` 固定标签为 `mpp`、存储为 TiFlash、并发为 0，并把 `reports_directly` 回调交给 `select_result.rs` 在每批执行摘要到来时判断归集路线。
7. 调用方通过 `NextRaw`、`Next` 或 `IntoIter` 消费结果；本文件只转发，错误、统计合并、幂等关闭等具体行为由 `selectResult` 完成。
8. 编码路径中，`SetEncodeType` 仅在会话开启 chunk 且 `checkAlignment` 成功时选择 `Chunk`，然后用 `GetSystemEndian` 写入内存布局；否则选择 `Default`。

## 数据与状态

`DistSQLSelectResult` 的公开字段是发送时元数据快照：`label` 区分 `dag/analyze/checksum/mpp`，`sql_type` 区分 `internal/general`，`row_len` 来自字段类型数量，`cop_plan_ids/root_plan_id` 标识统计节点，`store_type` 决定 TiKV/TiFlash 归类，`paging` 来自请求。响应推进状态、缓存、关闭标记与统计归集状态都封装在 `inner: selectResult<Box<dyn ResponseSource>>` 中。

共享对象均通过 `Arc` 传递：`client` 可跨线程共享，query limiter 在请求克隆中共享同一实例，`exec_details` 与 `runtime_stats` 供响应消费阶段更新。`runtime_stats` 额外由 `Mutex` 串行化写入。`ClientSendOptions` 是按次值拷贝，确保 Analyze 使用发送时的配置，而不是消费响应时重新读取全局开关。

TiFlash metadata 采用追加语义，不去重也不覆盖已有键。四个整数会话变量仅把 `-1` 当成“未设置”；其他负值仍原样发送。每节点查询内存配额在非正数时规范化为字符串 `"0"`；spill ratio 和 hash join v2 始终追加。

## 依赖与调用关系

上游类型来自 `pkg/distsql/request_builder.rs` 的 `KvRequest`，以及 crate 根的 `StoreType`、`ResponseSource`、`SelectResponse`、`DistSqlResult`。下游核心是 `pkg/distsql/select_result.rs` 的 `selectResult::new`、`with_stats_context` 和 `SelectResult`；实际统计是否采集、Analyze 原始明细处理、MPP 执行摘要路由、未消费响应统计和关闭语义均在那里执行。

`Select -> KvClient::send -> ResponseSource -> selectResult` 是普通 DAG 主链。`SelectWithRuntimeStats -> Select` 是明确的内部调用边。`Analyze -> KvClient::send_with_options` 会在默认实现中继续到 `send`。`SetEncodeType -> canUseChunkRPC -> checkAlignment`，chunk 分支再经过 `setChunkMemoryLayout -> GetSystemEndian`。MPP 包装没有客户端发送步骤。

RustCodeGraph 的文件关系还显示 `request_builder.rs`、`select_result.rs` 使用本文件，但精确 callers/callees 查询未产生额外生产调用边；仓库 `rg` 只定位到 `pkg/distsql/distsql_test.rs` 对发送入口的直接调用，以及 `pkg/executor/internal/builder/builder_utils.rs` 中另一个 trait 方法 `SetEncodeType`，后者通过其上下文抽象调用，不能证明它直接调用本文件同名函数。

## 错误处理与边界

所有发送入口以 `DistSqlResult` 传播客户端错误，使用 `?` 后不会构造半成品结果。与 Go 实现不同，`KvClient::send` 的返回类型不允许空响应，因此 Rust 无需再做 nil 检查。`SelectResult` 消费错误由内部 `selectResult` 原样传播；本文件的装箱委托也不吞错。

`Analyze` 和 `Checksum` 的注释要求相应 payload，但实现不做类型判定；`distsql_test.rs::analyze_and_checksum_forward_requests_without_rechecking_payload_kind` 明确锁定了“错误类型仍转发”的当前边界，调用方必须保证类型正确。`Analyze` 会覆盖克隆请求的 `request_source`，不修改原请求。`Select` 同理只修改请求克隆，这保证 query limiter 注入不会产生调用方可见副作用。

Chunk 能力检查目前只验证 `enable_chunk_rpc` 与 `align_of::<i128>() >= 8`，它不是 Go 侧 `types.MyDecimal` 大小检查的逐字复刻。metadata 容器也只是键值向量，并非真实 gRPC `context.Context`；调用方若需要去重、传输或冲突策略，必须在外围实现。

## 并发与资源生命周期

`KvClient`、`KvExecCounter` 和 MPP 直报回调均要求 `Send + Sync`，使上下文及结果构造可以安全跨线程共享这些依赖。`DistSQLContext` 可克隆，其中客户端、limiter、执行明细和统计集合共享所有权；克隆不会复制底层 limiter 配额或统计数据。统计集合通过 `Arc<Mutex<RuntimeStatsColl>>` 保护。

响应源的所有权在发送成功后移入 `selectResult`。`DistSQLSelectResult::Close` 只转发给内部对象；幂等关闭、错误后未消费 cop 统计回收等生命周期保证由 `select_result.rs` 实现，并由 `go_merge_42_analyze_records_raw_details_once_and_estimates_each_request`、`go_merge_42_analyze_preserves_subset_stats_on_error_and_close` 覆盖。MPP 的 `reports_directly` 是动态回调，不是构造时布尔快照，测试证明两批响应之间切换回调结果会改变统计路由。

本文件不会创建线程、异步任务或通道。`request.concurrency` 只是传入内部结果用于观测；并发调度归客户端/响应源所有。`DistSQLContext::streaming` 与 `concurrency` 当前未在本文件消费，扩展时不应误认为它们已经控制发送行为。

## 与 Go 版本的对应关系

Go 对照为 `pkg/distsql/distsql.go`。Rust 保留了 `GenSelectResultFromMPPResponse`、`Select`、`SetTiFlashConfVarsInContext`、`SelectWithRuntimeStats`、`Analyze`、`Checksum`、`SetEncodeType`、端序辅助函数和 SQL KV 计数器绑定这些概念，并用独立测试覆盖关键迁移语义。

当前 Rust 是缩减接口而非完整逐行等价：Go `Select` 建立 tracing region、测试 hook、事务事件回调、内存 tracker、限流动作、CopLiteWorker/failpoint、TiFlash replica read/warning，并绑定真实 client-go RPC interceptor；Rust `Select` 目前只处理 query limiter、发送和结果统计上下文。Go 的 TiFlash 配置写入 gRPC outgoing context，Rust 写入调用方提供的 metadata 向量。Go chunk 对齐依据 `MyDecimal` 布局，Rust依据 `i128` 对齐。Go `WithSQLKvExecCounterInterceptor` 在 counter 非空时绑定 RPC interceptor，Rust版本要求非空 `Arc` 并仅存进 `RequestContext`。

Rust 在已移植范围内保留的重要一致性包括：`-1` 是四项 TiFlash 整数变量的唯一省略哨兵、非正内存配额发送 0、Analyze 将来源标为内部统计并在发送时捕获执行信息开关、SelectWithRuntimeStats 保存计划 ID、MPP 的直报判断保持动态、显式 cop limiter 与 query limiter 相互独立。差异应视为迁移状态与扩展风险，而非自动判定为缺陷。

## 扩展指南

- 新增发送选项时，优先扩展 `ClientSendOptions` 和 `KvClient::send_with_options`，并在测试客户端中捕获验证；不要绕过 `KvClient` 直接引入具体传输。
- 新增结果统计字段时，同步修改本文件构造的 `ResultStatsContext`、`pkg/distsql/select_result.rs` 的消费逻辑，以及独立测试 `pkg/distsql/distsql_test.rs`；Rust 测试不要内嵌回生产源文件。
- 扩展 `Select` 时保持“克隆后注入”的不变量，避免修改调用方请求；特别要验证 `copr_request_limiter` 与 `query_cop_store_limiter` 不互相覆盖。
- 增加 TiFlash metadata 时明确省略哨兵、数值规范化、重复键策略，并对照 `distsql.go` 与会话变量定义；该路径每次请求都会执行，避免额外分配或昂贵计算。
- 改动 chunk 编码前同时核对 Rust 数据布局、tipb 协议表示与 Go `checkAlignment` 语义；大端/小端和未满足对齐时的回退都需要独立测试。
- 若要把 `KvExecCounter` 真正接入 RPC，必须在具体 `KvClient` 发送实现中消费 `RequestContext::counter` 并调用 `on_request`，同时补充生产调用边；仅修改绑定函数不会产生计数。
- 若后续接入 executor 主链，应先用 RustCodeGraph/仓库搜索确认真实调用者，再记录 wiring；不要以 Go 调用关系代替 Rust 证据。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/distsql` 列出 25 个文件；`node --file pkg/distsql/distsql.rs --offset 1 --limit 500` 读取完整 431 行和 45 个符号，并报告直接使用文件。对关键符号执行了 `query` 及 `callers/callees`；后者未给出可用生产调用边，因此未据此扩大结论。
- 源码与 crate 边界：`pkg/distsql/distsql.rs`、`pkg/distsql/lib.rs`、`pkg/distsql/Cargo.toml`、`pkg/distsql/request_builder.rs`、`pkg/distsql/select_result.rs`。
- Go 对照：`pkg/distsql/distsql.go`，重点核对发送选项、TiFlash metadata、Analyze/Checksum、chunk 编码与 interceptor 语义。
- Rust 独立测试：`pkg/distsql/distsql_test.rs`，覆盖 limiter 独立性、Analyze 原始/未消费统计、MPP 动态路由、请求标签与并发、payload 不复核、TiFlash 哨兵/配额和 chunk 端序。Go 回归参考：`pkg/distsql/distsql_test.go`。
- 仓库搜索：对主要公开入口执行 `rg`，未发现本文件发送 API 的 Rust 生产直接调用；同名 trait/函数按路径区分，没有当成本文件调用者。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务规定的命令验证恰有 11 个固定二级章节，并人工复核所有“当前已支持”陈述均能回指上述源码或测试。
