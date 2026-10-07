# `pkg/distsql/request_builder.rs`

源文件：[`request_builder.rs`](./request_builder.rs)

## 文件定位

本文件属于 `astersql-distsql` crate。crate 入口 [`lib.rs`](./lib.rs) 通过 `pub mod request_builder` 公开该模块；[`Cargo.toml`](./Cargo.toml) 的 `[lib] path = "lib.rs"` 确认了这一边界。它位于 SQL 执行层与 KV 客户端之间：上游把 DAG、Analyze 或 Checksum 的序列化载荷、扫描范围和会话选项写入 `RequestBuilder`，`Build` 产出 `KvRequest`；下游 [`distsql.rs`](./distsql.rs) 中的 `Select`、`Analyze`、`Checksum` 再把该值交给 `KvClient::send` 或 `send_with_options`。

文件只负责“描述和编码请求”，不发送 RPC、不切分 Region，也不消费响应。`lib.rs` 还定义了一个 API 和约束均不同的包级简化 `RequestBuilder`；本文只讨论 `request_builder` 模块中的完整版，二者不能混用。

## 核心职责

1. `RequestBuilder` 以链式 setter 收集请求类型、载荷、key ranges、MVCC 时间戳、扫描方向、并发、存储类型、作用域、资源组和批处理控制，并在 `Build` 中生成可发送的 `KvRequest`。
2. `SetFromSessionVars` 把 `SessionVars` 中与 DistSQL 有关的字段复制到构造器；并发字段采用“未设置则填入，否则取较小值”的上限语义。
3. `TxnScopeChecker`、`SetFromInfoSchema`、`verifyTxnScope` 和独立函数 `VerifyTxnScope` 提供事务作用域校验边界。
4. `TableRangesToKVRanges`、`TableHandlesToKVRanges`、`PartitionHandlesToKVRanges`、`IndexRangesToKVRanges`、`CommonHandleRangesToKVRanges`、`BuildTableRanges` 把表/索引逻辑范围编码成 TiDB key 的半开区间。
5. `SplitRangesAcrossInt64Boundary` 保留有序扫描时 signed/unsigned 两组的返回顺序；`estimatedRegionRowCount` 保留 Go 侧小 limit 并发优化所用常量，但当前 Rust 文件没有消费它。

## 主要符号

- `IsolationLevel::{Snapshot, ReadCommitted}`、`Priority::{Low, Normal, High}`：请求的隔离级别和优先级；默认分别是 `Snapshot`、`Normal`。
- `RequestPayload::{Empty, Dag, Analyze, Checksum}`：已经序列化的载荷字节。`Empty` 表示允许先生成 Go 零值语义的请求，不代表可执行层一定接受空载荷。
- `PartitionIDAndRanges`：把物理分区 ID 与该分区的 `Vec<KeyRange>` 绑定；同时是非 global txn scope 校验遍历的数据源。
- `KvRequest`：发送边界上的完整值对象。除常规扫描字段外，还包含 `copr_request_limiter`、`query_cop_store_limiter`、`store_batch_size`、`allow_batch_task_data_merge` 和 `execute_batch_tasks_serially`。`Build` 总把 `query_cop_store_limiter` 置为 `None`，后续 `distsql::Select` 才从 `DistSQLContext` 注入它。
- `SessionVars`：Go `DistSQLContext` 的收窄替身；默认并发为 15、两个 scope 为 `global`，其他开关关闭。
- `RequestBuilder`：持有尚未提交的字段、延迟错误 `error`、单次使用标志 `used` 以及 `Arc<dyn TxnScopeChecker>`。公开 setter 返回 `&mut Self`，支持连续调用。
- `TxnScopeChecker: Send + Sync`：由 schema/placement 侧实现的跨线程安全校验接口。
- `HandleRange`：整数 handle 的 low/high 及开闭信息。
- 私有编码函数 `encode_i64`、`table_prefix`、`record_prefix`、`index_prefix`、`prefix_next`、`handle_range`：建立 `t{table-id}_r` 和 `t{table-id}_i{index-id}` key 布局，并把闭上界转换成半开上界。

## 执行流程

典型请求构造流程如下：

1. `RequestBuilder::new` 建立 Go 零值兼容的构造器：请求类型和载荷尚未设置，并发为 0，隔离级别为 Snapshot，优先级为 Normal，事务和读副本 scope 为 `global`。
2. 调用方用 `SetDAGRequest`、`SetAnalyzeRequest` 或 `SetChecksumRequest` 选择类型并放入已序列化字节。Analyze 额外设置调用方指定的隔离级别、低优先级和不填 cache；Checksum 也不填 cache。
3. 调用方设置普通/分区 ranges、start TS、顺序、存储类型及限流/批处理字段。需要会话默认值时调用 `SetFromSessionVars`；显式并发不会超过会话并发。
4. 非 global txn scope 可通过 `SetFromInfoSchema` 安装 checker。`Build` 先拒绝成功使用过的 builder，再取出延迟错误，然后调用 `verifyTxnScope`。校验只遍历 `partition_ranges` 的物理分区 ID；scope 为空或严格等于小写 `global` 时短路通过，没有 checker 时也不会拒绝。
5. `Build` 使用默认 `RequestType::Dag`、`RequestPayload::Empty`、`StoreType::TiKv` 和 60 秒超时补齐未设置字段，克隆 ranges、字符串和 `Arc`，返回独立的 `KvRequest`。
6. `distsql::Select` 克隆该请求，可从上下文覆盖 `query_cop_store_limiter`，随后调用 `KvClient::send`；`Analyze` 会把克隆请求的 `request_source` 改为 `stats` 后调用 `send_with_options`；`Checksum` 直接调用 `send`。

range 编码是另一条独立流程：`encode_i64` 翻转符号位后使用大端字节，使有符号整数的数值顺序与 key 字典序一致；表/记录/索引前缀函数加上 TiDB 标识字节；开区间低端或闭区间高端由 `prefix_next` 前移。连续整数 handles 会在 `handle_range` 中合并成一个 `[first, last-next)` 区间并产生行数 hint，分区 handles 仅在分区相同且数值连续时合并。

## 数据与状态

`RequestBuilder` 是可变、一次性提交的暂存对象；`KvRequest` 是可克隆的发送快照。成功进入 `Build` 后 `used` 保持为 `true`，再次调用返回 `RequestBuilder cannot be reused`。若预存 `error`，`Build` 会取出错误并把 `used` 恢复为 `false`；当前公开 setter 没有写入 `error`，该路径为延迟错误机制预留。若 `verifyTxnScope` 失败，`?` 会直接返回，但不会恢复 `used`，因此这类失败后也不能重用。

`key_ranges` 表示非分区范围，`partition_ranges` 表示按物理分区组织的范围；实现不强制二选一，也不校验 hints 与 ranges 等长。range 辅助函数有意直接构造 `KeyRange { start, end }`，因此保留 Go 能表达的空区间或反向区间，而不是调用会拒绝 `start >= end` 的 `KeyRange::new`。

`Arc<CoprRequestLimiter>` 和 `Arc<dyn TxnScopeChecker>` 允许请求快照共享限流器和只读 checker。`KvRequest.query_cop_store_limiter` 不属于 builder 状态，由发送入口按查询上下文添加。

## 依赖与调用关系

- crate 内依赖：从 `lib.rs` 使用 `DistSqlError`、`DistSqlResult`、`KeyRange`、`RequestType`、`StoreType`；`distsql.rs` 使用本模块的 `KvRequest` 作为 `KvClient`、`Select`、`Analyze`、`Checksum` 的请求类型。
- workspace 依赖：`Cargo.toml` 在普通依赖中声明 `astersql-kv`，本文件使用其中的 `CoprRequestLimiter` 和 `QueryCopStoreLimiter`。`astersql-config` 与 `astersql-util-execdetails` 是同 crate 其他模块的直接依赖，不应误记成本文件的直接调用。
- 模块公开关系：`lib.rs: pub mod request_builder` 是真实公开入口；仓库范围 `rg` 发现生产 Rust 代码中 `distsql.rs` 直接消费 `KvRequest`，没有发现其他生产文件直接调用本文件的 range 转换函数。
- 测试调用关系：[`request_builder_test.rs`](./request_builder_test.rs) 通过 `crate::request_builder as production` 直接覆盖本模块；[`distsql_test.rs`](./distsql_test.rs) 用本模块构造请求并验证 `Select` 传递 limiter、store type、paging 等字段。
- RustCodeGraph 限制：仓库索引总体可用，但 `files --filter pkg/distsql/request_builder` 没有命中，针对该路径的 `explore` 也无输出，故本文件的 callers/callees 由 `rg` 和逐文件源码核验，而非图边推导。

## 错误处理与边界

- `Build` 的显式错误包括 builder 重用、预存延迟错误和 txn scope 不匹配；错误统一装入 `DistSqlError(String)`。
- txn scope 比较区分大小写：空字符串和精确的 `global` 通过，`GLOBAL` 会委托 checker。测试 `txn_scope_zero_value_and_global_match_go_exactly` 固定了该行为。
- `verifyTxnScope` 只验证 `partition_ranges`，不会从普通 `key_ranges` 解码物理表 ID；没有 checker 时非 global scope 也通过。这比 Go 版从所有 key ranges 提取 table ID 并查询 InfoSchema 的行为更窄。
- `IndexRangesToKVRanges` 与 `CommonHandleRangesToKVRanges` 当前签名返回 `DistSqlResult`，但函数体没有错误分支；输入字节被视为已经编码的端点，不执行 datum/collation/timezone 编码。
- `TableHandlesToKVRanges` 和 `PartitionHandlesToKVRanges` 假定输入已经按希望的扫描顺序排列；它们只合并相邻输入项，不排序、不去重。`i64::MAX` 被显式阻止执行 `last + 1`，避免溢出。
- `SplitRangesAcrossInt64Boundary` 仅按 `low < 0` 分组，不会像 Go 版那样拆开跨越 `MaxInt64` 的 unsigned datum 范围；`common_handle` 为真时原样返回第一组。
- `BuildTableRanges` 只接受 table ID 和 index ID 列表，不检查索引状态、global index 或分区元数据。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或网络连接。`RequestBuilder` 通过 `&mut self` 串行变更，源码没有声明它可在多个执行线程间并发共享。

并发相关字段只作为策略数据传递：`concurrency` 限制 coprocessor 扫描并发，`allow_batch_cop`、`store_batch_size`、`allow_batch_task_data_merge`、`execute_batch_tasks_serially` 控制批任务形态。`SetFromSessionVars` 对并发取上限，但本文件不执行并发调度。

共享资源由 `Arc` 管理：构造和克隆请求只增加引用计数，不在此处关闭或独占 limiter/checker。`CoprRequestLimiter` 随 `KvRequest` 传到客户端；查询级 `QueryCopStoreLimiter` 在 `Select` 的局部请求克隆中注入。请求发送后的网络流和响应生命周期属于 `distsql.rs`/`select_result.rs`，不属于本文件。

## 与 Go 版本的对应关系

直接对照文件为 [`request_builder.go`](./request_builder.go)，测试对照为 [`request_builder_test.go`](./request_builder_test.go)。Rust 保留了 Go 的主要形状：链式 setter、构造后 `Build`、Analyze/Checksum 不填 cache、会话并发为上限、table/index key 前缀、连续 handle 合并及 hints、txn scope 校验，以及三个批处理控制字段。

当前 Rust 并非 Go 的等价完整移植，重要差异包括：

- Rust 载荷 setter 接收已经序列化的 `Vec<u8>`；Go 接收 tipb protobuf 并在 builder 内 Marshal，还会分析 DAG executor、limit 和默认并发。
- Go 的 `kv.Request` 包含 replica read、store labels、paging 大小、task ID、cacheable、schema version、memory tracker、runaway checker、超时/读键配额等更多字段；Rust 的 `SessionVars` 与 `KvRequest` 仅覆盖其中一部分。
- Go `verifyTxnScope` 从 key ranges 解码全部访问的物理表并查询 placement bundle；Rust 仅遍历显式 `partition_ranges`，以 trait checker 代替 InfoSchema。
- Go 索引/common-handle 编码处理 datum、collation、timezone、错误上下文、内存追踪和中断信号；Rust 只拼接调用方提供的端点字节。
- Go 的 `SplitRangesAcrossInt64Boundary` 面向 unsigned datum 并能拆分跨界范围；Rust 只对 `HandleRange.low` 的正负分组。
- Go `BuildTableRanges` 根据 `TableInfo` 展开分区和 public/global index；Rust 按调用方传入的单表 ID 与 index IDs 机械生成前缀范围。
- Go 只在测试模式检查 builder 重用，且失败返回策略与 Rust 不完全一致；Rust 在所有构建中都拒绝成功后的重用。

因此扩展 Rust 时应以 Go 行为和现有 Rust 独立测试共同约束，不能因为当前接口更简单就推断缺失能力已经由其他层实现。

## 扩展指南

- 新增请求元数据时，应同时修改 `RequestBuilder` 暂存字段、对应 setter、`KvRequest` 字段和 `Build` 的复制/default 逻辑；若发送入口会覆盖字段，还要核对 `distsql.rs`。
- 扩充 `SessionVars` 时，明确字段是覆盖、默认填充还是上限约束，并在 `request_builder_test.rs` 增加独立测试；不要把测试内嵌到生产 `.rs`。
- 扩充 txn scope 时，应决定是否继续只看 `partition_ranges`，或补齐 Go 的 key 解码语义；新增 checker 实现必须满足 `Send + Sync`，错误消息应保留物理表/scope 诊断信息。
- 修改 key 编码前必须同步核对 `request_builder.go` 中 `encodeHandleKey`、`TableHandlesToKVRanges`、`PartitionHandlesToKVRanges`、`EncodeIndexKey` 和 `BuildTableRanges`；重点回归开闭端点、空区间、`i64::MAX`、跨分区连续值、common handle、多表展开和 prefix-next 全 `0xff` 情况。
- 启用 `estimatedRegionRowCount` 相关优化时，应复刻 Go 对 DAG executor/Limit/IndexLookup 的判定，而不是仅凭 range 数量简化。
- 需要补测试时优先扩展同目录独立文件 `request_builder_test.rs`；跨发送边界的字段再扩展 `distsql_test.rs`。Go 对照回归在 `request_builder_test.go`，但本纯文档任务未运行这些测试。

## 验证依据

- 完整读取：`pkg/distsql/request_builder.rs`（661 行）、`pkg/distsql/request_builder_test.rs`、`pkg/distsql/request_builder.go`、`pkg/distsql/request_builder_test.go`、`pkg/distsql/Cargo.toml`、`pkg/distsql/lib.rs`、`pkg/distsql/distsql.rs` 及相关 `pkg/distsql/distsql_test.rs` 片段。目标包不存在 `doc.go`。
- RustCodeGraph：`rustcodegraph status` 显示项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；但针对目标路径的 `explore` 无输出，`files --filter pkg/distsql/request_builder` 返回无匹配，故没有把缺失图边当成“无调用者”。
- 源码搜索：符号枚举确认本文件的全部公开类型、trait、函数、setter 和私有编码辅助；仓库范围调用搜索确认 `distsql.rs` 消费 `KvRequest`，`lib.rs` 公开模块，两个独立 Rust 测试文件覆盖构造与发送边界。
- 测试证据（仅阅读，未执行）：`request_builder_test.rs` 覆盖连续 handle 合并、分区隔离、开闭端点与空范围、多表索引展开、零值构建、请求字段透传、Analyze/Checksum cache 语义、会话并发上限、单次使用、txn scope、signed/unsigned 顺序、整表范围及 limiter/批处理字段。
- 任务是纯文档分析，按计划未运行 Cargo、Rust 或 Go 测试。交付结构检查要求本文恰有 11 个指定二级标题；最终还应检查只新增本文档且没有修改源代码、Cargo 或只读 `plan.md`。
