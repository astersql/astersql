# `pkg/executor/compact_table.rs`

## 文件定位

[`compact_table.rs`](compact_table.rs) 属于 `astersql-executor` crate；[`lib.rs`](lib.rs) 以 `pub mod compact_table` 将它公开，而 [`Cargo.toml`](Cargo.toml) 通过 `[lib] path = "lib.rs"` 定义该 crate。它承载 `ALTER TABLE ... COMPACT TIFLASH REPLICA` 的 Rust 侧执行算法：发现 TiFlash store，针对每个 store 并行工作，并在单个 store 内按物理表、按响应页串行发送压缩请求。

当前仓库中的生产入口只可追踪到 [`executorBuilder::buildCompactTable`](builder.rs)：它校验副本类型与存储、解析分区 ID，再把实际构造委托给 `ExecutorBuilderDependencies::build_compact_table_executor`。代码搜索未找到该 trait 方法的 Rust 生产实现，也未找到测试以外的 `CompactTableTiFlashExec` 构造或 `CompactRuntime` 实现。因此，本文件的算法和抽象边界是实装的，但其 Rust 生产适配接线在当前仓库中未验证；不能仅凭 Go 版本推断已经接入完整 SQL 主链。

## 核心职责

- `CompactTableTiFlashExec::Next` 保证一条执行器实例只执行一次，并且不产生结果行，只重置 `CompactChunk`。
- `CompactTableTiFlashExec::doCompact` 检查 TiFlash 副本、筛选 TiFlash store、创建语句级 `CompactRun`，并为每个 store 启动一个 scoped worker。
- `storeCompactTask::work` 在一个 store 内确定物理表集合并串行处理；分区表在未指定分区时遍历全部定义，指定分区时保持请求给出的 ID 顺序，非分区表使用逻辑表 ID 作为物理表 ID。
- `storeCompactTask::compactOnePhysicalTable` 根据 `HasRemaining` 分页，用上一页 `CompactedEndKey` 作为下一页 `StartKey`，同时处理 TiFlash 业务错误、取消和无效分页响应。
- `storeCompactTask::sendRequestWithRetry` 只对 `CompactTransportErrorKind::Network` 退避重试；取消、超时及 gRPC 取消等非网络错误直接返回。
- `CompactRuntime`、`CompactRun` 和 `CompactBackoff` 把集群发现、告警、日志、RPC、取消、资源释放及重试策略隔离为必须由适配层实现的边界。

## 主要符号

- 常量 `compactRequestTimeout`、`compactMaxBackoffSleepMs`、`compactProgressReportInterval` 分别固定单次 RPC 超时为 1 小时、单页重试最大累计睡眠参数为 5000 毫秒、进度日志最小间隔为 10 秒，与 Go 文件同名常量一致。
- `AdapterResult<T>` 是 `Result<T, astersql_errors::SharedError>` 的别名；本模块的外部失败统一通过共享错误传递。
- `CompactChunk` 是无结果列的输出占位，`Reset` 当前无状态可清理，但维持执行器 `Next` 的接口语义。
- `ServerType`、`ServerInfo` 描述节点角色与地址；`getTiFlashStores` 调用 `CompactRuntime::GetStoreServerInfo` 后只保留 `ServerType::TiFlash`。
- `TableInfo`、`TiFlashReplicaInfo`、`PartitionInfo`、`PartitionDefinition` 是本算法所需的最小表元数据投影。
- `CompactRequest` 携带逻辑表 ID、物理表 ID 和页起始键；`CompactResponse` 携带业务错误、剩余标记及已压缩键范围；`CompactRpcResponse` 允许表达缺失响应体这一协议错误。
- `CompactErrorKind` 区分 `CompactInProgress`、`TooManyPendingTasks`、`PhysicalTableNotExist` 和 `Unknown`；`CompactTransportErrorKind` 区分取消、超时、gRPC 取消与可重试网络错误。
- `CompactLogEvent` 用结构化事件表示开始、完成、进度、失败、物理表跳过及无效页；`CompactLogContext` 固定携带表名、表 ID、请求分区和 store 地址。
- `CompactRun` 管理一次语句作用域内的取消、RPC 与 `Finish`；`CompactBackoff` 管理单页请求的有界退避；`CompactRuntime` 创建前两者并承接发现、告警和日志。
- `CompactTableTiFlashExec` 保存 runtime、表信息、分区 ID 和一次性 `done` 状态；`storeCompactTask` 保存单 store worker 的共享输入及进度计数；`CompactTaskError::stop_all` 决定错误是否传播为全局取消。
- 私有函数 `hex_encode` 只用于把无效页的起止键编码为小写十六进制日志字段。

## 执行流程

1. `Next` 先调用 `CompactChunk::Reset`。若 `done` 已置位则立即成功；否则先置 `done = true`，再进入 `doCompact`，所以即使首次执行返回错误，后续 `Next` 也不会重跑。
2. `doCompact` 检查 `TableInfo::TiFlashReplica`：不存在或 `Count == 0` 时追加 `compact skipped: no tiflash replica in the table` 告警并成功结束，且不会创建 `CompactRun`。
3. `getTiFlashStores` 从 runtime 获取节点列表并过滤非 TiFlash 节点。随后 `BeginRun` 创建语句级运行对象；表信息、分区 ID、runtime、run 和原子取消标志通过 `Arc` 共享。
4. `std::thread::scope` 为每个 TiFlash store 创建一个 `storeCompactTask`。各 store 并行；同一 task 中，`work` 按物理表顺序串行调用 `compactOnePhysicalTable`，符合 TiFlash 同表一次压缩的约束。
5. 对每个物理表，分页循环先检查本地原子标志和 `CompactRun::IsCancelled`，再按需记录进度，并发送包含逻辑/物理表 ID 与当前 `StartKey` 的请求。
6. 传输成功后，先解释业务错误；无业务错误且 `HasRemaining == false` 时完成该物理表。若仍有数据，则要求 `CompactedEndKey` 非空，并把它复制为下一页 `StartKey`。
7. worker 的 store-local 错误停止该 store 的剩余分区，但已转成用户告警，`work` 对调度层返回成功；`stop_all` 错误会设置共享取消标志、调用 `run.Cancel` 并返回错误，使其他 worker 在取消检查点停止。
8. 主线程逐一 join worker。worker panic 会触发全局取消，并最终返回 `TiFlash compact worker panicked`；正常和业务错误路径在 join 后均调用 `run.Finish`，最终以其结果收尾。

## 数据与状态

- `CompactTableTiFlashExec::done` 是实例级一次性开关，不是跨实例或跨会话状态；它在实际工作前置位。
- `tableInfo` 与 `partitionIDs` 在 `doCompact` 中被克隆进 `Arc`，worker 只读共享。指定分区列表不在任务内重新校验是否属于表定义，校验责任位于上游 `resolve_compact_partition_ids`。
- `storeCompactTask` 的 `allPhysicalTables`、`compactedPhysicalTables`、`startAt` 和 `lastProgressOutputAt` 归单 worker 独占；进度比率按已结束尝试的物理表数除以该 store 的物理表总数计算。
- `start_key` 只在一个物理表的分页循环内存在；切换物理表或 store 会重新从空键开始，因此多个 store 拥有独立分页序列。
- `cancelled: Arc<AtomicBool>` 使用 `Release` 写、`Acquire` 读传播本模块的全局停止信号；`CompactRun` 还必须同步外部查询 kill、deadline 和在途 RPC 取消。
- `CompactLogContext` 每次记录事件时从只读元数据克隆字符串和分区 ID，事件不会借用 worker 生命周期内的数据。

## 依赖与调用关系

RustCodeGraph 给出的核心调用链是 `CompactTableTiFlashExec::Next -> doCompact -> getTiFlashStores / CompactRuntime::BeginRun / storeCompactTask::work`，以及 `work -> compactOnePhysicalTable -> sendRequestWithRetry`。`sendRequestWithRetry` 再调用 `CompactRun::SendCompact`、`CompactBackoff::Backoff` 与取消接口；`compactOnePhysicalTable` 调用日志、告警和响应访问器。

上游方面，[`lib.rs`](lib.rs) 导出模块并在 `#[cfg(test)]` 下挂载 [`compact_table_test.rs`](compact_table_test.rs)。[`builder.rs`](builder.rs) 的 `buildCompactTable` 是相邻的 SQL 构建入口，但它只调用依赖接口；RustCodeGraph 对 `CompactTableTiFlashExec` 的直接 Rust trail 仅显示测试辅助函数 `executor` 的构造。因而从 `builder.rs` 到本文件具体类型的生产边目前是缺失或位于索引/仓库之外，而不是可以确认的调用关系。

依赖方面，本文件直接使用标准库的 `Arc`、原子变量、scoped thread、`Duration` 和 `Instant`，以及 `astersql_errors`。[`Cargo.toml`](Cargo.toml) 声明本地依赖 `astersql-errors = { path = "../errors" }`；本文件没有条件编译项，也不依赖 `nextgen` feature。

## 错误处理与边界

- store 发现失败或 `BeginRun` 失败会从 `doCompact` 直接向调用者返回；无 TiFlash 副本则是告警而非错误。TiFlash store 列表为空时仍会创建并最终 `Finish` 一个 run。
- `CompactInProgress` 表示同表已有压缩，`responseFailure` 产生用户告警与失败日志，并以 `stop_all = true` 取消所有 store；这是唯一业务响应中的全局停止类别。
- `TooManyPendingTasks` 与 `Unknown` 产生告警及失败日志，停止当前 store 的剩余物理表，但不取消其他 store。其错误被 `work` 吸收，符合 Go 版本把 worker 错误转为 warning 后忽略的语义。
- `PhysicalTableNotExist` 可能来自长时间压缩期间的 DDL；它只写 `PhysicalTableSkipped` 日志，不发用户告警，并继续当前 store 的后续物理表。
- `HasRemaining == true` 但结束键为空是协议不变量破坏：写通用内部错误告警和包含十六进制键范围的 `InvalidPage`，停止当前 store。
- 网络错误在相同请求和相同 `StartKey` 上退避重试；退避耗尽时保留最后的网络错误消息。非网络传输错误、取消和超时不退避。RPC 包装缺少 body 时返回固定错误 `TiFlash compact response body is missing`。
- 普通 store-local 失败不会使顶层 `Next` 失败；真正向上传播的主要路径是发现/启动/收尾错误和 worker panic。`CompactTaskError::source` 保存详细错误，但 join 聚合只读取 `stop_all`，其文本通过告警/日志保留。

## 并发与资源生命周期

并发粒度是“每个 TiFlash store 一个线程”，没有并发上限；单 store 内所有物理表及单物理表的所有页严格串行。`std::thread::scope` 保证 `doCompact` 离开作用域前所有 worker 已 join，避免后台线程借用或持有悬空状态。

全局停止通过两层状态协作：本文件的 `AtomicBool` 让 worker 快速观察内部 `stop_all`/panic，`CompactRun::Cancel` 则要求生产适配层取消所有在途 RPC，并让 `IsCancelled` 反映查询 kill/deadline。取消只在每页发送前检查；正在执行的 `SendCompact` 是否及时中断取决于 `CompactRun` 实现。

`BeginRun` 成功后，正常、告警、取消和 panic 路径都会在 worker join 后调用一次 `Finish`。panic 路径刻意忽略 `Finish` 错误并返回 panic 错误；无 panic 时 `Finish` 的错误成为 `doCompact` 结果。独立测试用 `finished`、`cancel_calls` 和请求记录验证了资源收尾及取消调用。

## 与 Go 版本的对应关系

[`compact_table.go`](compact_table.go) 是直接语义来源：同样由 `Next` 一次性触发，缺少 TiFlash 副本时告警；按 store 并行、按分区串行；分页沿用 `CompactedEndKey`；对 in-progress、busy、不存在物理表、未知错误和无效结束键作相同分类；网络错误使用最大 5 秒退避预算，单次请求超时 1 小时。

Rust 版把 Go 的具体基础设施显式抽象为 trait：`sessionctx`/statement warning 与日志对应 `CompactRuntime`，`tikv.Storage` 客户端和 `context.Context` 对应 `CompactRun`，`backoff.Backoffer` 对应 `CompactBackoff`，protobuf 请求/响应则由本地数据结构表示。Go 使用 `errgroup.WithContext` 传播全局取消；Rust 使用 scoped thread、原子标志与 `CompactRun::Cancel`。Go 的顶层 `_ = g.Wait()` 总是忽略 worker 错误；Rust 同样吸收普通 worker 错误，但额外把 worker panic 转为顶层错误，并显式执行 `Finish`。

[`compact_table_test.rs`](compact_table_test.rs) 覆盖 Rust 抽象下的核心 Go 意图：无副本与一次性执行、过滤非 TiFlash、分页续键、网络重试、busy/unknown 告警、全局取消、不存在分区跳过、指定分区顺序、多 store 独立页序列、无效页、非重试传输错误、退避耗尽及缺失响应体。Go 测试 [`compact_table_test.go`](compact_table_test.go) 还通过真实 testkit/mock RPC 覆盖多 store、范围/哈希分区、TiFlash 下线及恢复；这些集成层场景在 Rust 独立测试中没有等价生产接线验证。

## 扩展指南

- 新增 TiFlash 业务错误类别时，应同步修改 `CompactErrorKind`、`compactOnePhysicalTable` 的 match、用户告警/日志语义，并在独立的 [`compact_table_test.rs`](compact_table_test.rs) 增加对应分支测试；不要把测试内嵌到生产文件。
- 调整重试策略时，应集中修改 `CompactTransportError::retryable`、`sendRequestWithRetry` 或 `CompactBackoff` 实现，明确取消/超时是否仍不可重试，并覆盖“相同页键重试”和“退避耗尽保留原错误”。
- 调整分页协议时，应维护“`HasRemaining` 为真则结束键必须非空”和“下一页从前页结束键继续”两个不变量；同时考虑键复制的内存成本和恶意/异常服务端导致的无限分页风险。
- 引入并发限制或分区并行时，修改点在 `doCompact` 和 `storeCompactTask::work`。必须保持同表压缩限制、store 间取消传播、进度计数一致性和 `Finish` 恰当收尾，并评估线程数、RPC 压力与 TiFlash 负载。
- 完成生产接线时，应实现 `CompactRuntime`/`CompactRun`/`CompactBackoff`，并让 `ExecutorBuilderDependencies::build_compact_table_executor` 构造本类型；需要验证查询 kill、deadline、RPC body 类型转换、集群发现、warning/log 注入及所有在途 RPC 的取消。
- 对用户可见文案或错误分类的修改需与 [`compact_table.go`](compact_table.go) 及其 Go 测试保持一致；若有意分叉，应在文档和独立测试中记录兼容理由。

## 验证依据

- 源码全量阅读：[`compact_table.rs`](compact_table.rs)，包括常量、所有数据类型、三个边界 trait、执行器、store task、分页/重试及十六进制日志辅助函数。
- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。`node getTiFlashStores` 验证其调用 `CompactRuntime::GetStoreServerInfo` 且被 `doCompact` 调用；`node doCompact` 验证其调用 `getTiFlashStores`、`BeginRun`、`work`、`Cancel`、`Finish` 并被 `Next` 调用；`node compactOnePhysicalTable` 与 `node sendRequestWithRetry` 验证 `work -> compactOnePhysicalTable -> sendRequestWithRetry` 及其日志、响应、RPC、退避和取消下游边。
- crate 与模块证据：[`Cargo.toml`](Cargo.toml) 的 package/lib/dependencies 段、[`lib.rs`](lib.rs) 的 `pub mod compact_table` 与独立测试模块声明。
- 上游证据：[`builder.rs`](builder.rs) 中 `ExecutorBuilderDependencies::build_compact_table_executor` 和 `executorBuilder::buildCompactTable`；仓库搜索只找到 trait 声明，没有找到 Rust 生产实现或测试外的本类型构造，因此生产接线明确记为未验证。
- 对照与测试证据：完整阅读 [`compact_table.go`](compact_table.go)；阅读 Rust 独立测试 [`compact_table_test.rs`](compact_table_test.rs) 的 12 个测试；核对 Go 测试 [`compact_table_test.go`](compact_table_test.go) 的 15 个测试入口与覆盖主题。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、文件存在、链接和 diff 范围。
