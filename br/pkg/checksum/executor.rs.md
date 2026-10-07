# `br/pkg/checksum/executor.rs`

## 文件定位

本文件是 `astersql-br-pkg-checksum` crate 的 checksum 请求构造与执行实现，对应 Go 文件 [`br/pkg/checksum/executor.go`](executor.go)。crate 根 [`br/pkg/checksum/lib.rs`](lib.rs) 以 `pub mod executor` 装配本文件，并通过 `pub use executor::*` 暴露其公开 API；[`br/pkg/checksum/Cargo.toml`](Cargo.toml) 指定 `lib.rs` 为库入口，且当前没有外部 Rust 依赖，因此 TiDB/TiKV 侧类型与接口来自同 crate 的 [`stubs.rs`](stubs.rs)，不是完整的 TiKV 客户端实现。

在已接线的上游中，`br/pkg/backup/schema.rs::schemaInfo::calculateChecksum` 用 `checksum::NewExecutorBuilder` 构造执行器，设置 BR 请求来源和并发度，调用 `Executor::Execute` 后把 `ChecksumResponse` 的 `Checksum`、`TotalKvs`、`TotalBytes` 写入 schema 备份元数据。RustCodeGraph 的文件节点还显示本文件被同目录测试及若干迁移测试引用；这些测试引用是契约证据，不应误写成生产调用链。

## 核心职责

本文件承担三个连续阶段：

1. `ExecutorBuilder` 保存表元数据、快照 TS、可选旧表、并发/退避、keyspace、资源组和请求来源配置。
2. `buildChecksumRequest`、`buildRequest`、`buildTableRequest`、`buildIndexRequest` 把一张逻辑表展开成表记录与公开索引的 DistSQL checksum `Request` 列表；分区表还会按定义顺序为每个分区重复展开。
3. `Executor::Execute` 顺序发送各请求，读取并聚合流式响应，在单请求失败时按 checksum 退避策略重试，成功后触发进度回调，最后检查上下文取消状态。

固定协议包括：算法为 `ChecksumAlgorithm::Crc64_Xor`；表记录请求使用 `ChecksumScanOn::Table`，索引请求使用 `ChecksumScanOn::Index`；请求优先级为 `PriorityLow`；checksum 按 XOR 聚合，KV 数和字节数按 `u64` 回绕加法聚合。

## 主要符号

- `ExecutorBuilder`：拥有构造期配置。`NewExecutorBuilder(table, ts)` 把并发初始化为 `DefDistSQLScanConcurrency`，其余覆盖项使用空值或零值。
- `ExecutorBuilder::{SetOldTable, SetConcurrency, SetBackoffWeight, SetOldKeyspace, SetNewKeyspace, SetResourceGroupName, SetRequestSource, SetExplicitRequestSourceType}`：消费并返回 builder 的链式配置入口；`request_source` 是只读观察入口。
- `ExecutorBuilder::Build`：调用 `buildChecksumRequest`，将构造错误以 `Error::Trace` 传播，并生成只包含 `reqs` 与 `backoff_weight` 的 `Executor`。
- `buildChecksumRequest`：先展开逻辑表 ID，再依次展开每个新表分区 ID；存在旧表时通过 `GetPartitionByName` 按分区名取得旧分区 ID。
- `buildRequest`：为一个表或分区 ID 先构造表请求，再为每个 `StatePublic` 索引构造索引请求；存在旧表时要求旧表有完全相等的 `CIStr` 索引名。
- `buildTableRequest`：选择 common-handle 的 `FullNotNullRange` 或整数句柄的 `FullIntRange(false)`，并可建立记录前缀 rewrite 规则。
- `buildIndexRequest`：使用 `FullRange` 和表/索引编码前缀，可建立索引前缀 rewrite 规则。
- `sendChecksumRequest`：调用 `DistSQLChecksum`，循环消费 `Response::NextRaw`，反序列化 `ChecksumResponse`，聚合后始终尝试 `Response::Close`。
- `updateChecksumResponse`：公开的纯聚合函数；`Checksum ^= update.Checksum`，两个计数字段用 `wrapping_add`。
- `Executor::{Len, Each, RawRequests, Execute}`：分别提供请求数、请求遍历、载荷反序列化和实际执行。
- `checkContextDone`：在上下文带错误时返回带有 `context is cancelled by other error` 语境的错误。
- `VariablesWrap`：本文件内部包装 `stubs::Variables`，把 builder 的正 `backoff_weight` 传给客户端发送路径。

## 执行流程

构造流程从 `NewExecutorBuilder` 开始。`Build` 首先在 `buildChecksumRequest` 中取得新表分区定义，并按 `(索引数 + 1) * (分区数 + 1)` 预估容量。它总是先针对 `new_table.ID` 调用 `buildRequest`；随后按新表分区定义顺序处理各分区。若配置了旧表，每个新分区必须通过 `GetPartitionByName(&old.Info, &part_def.Name)` 找到旧分区 ID，否则构造失败。

对每个表/分区 ID，`buildRequest` 先调用 `buildTableRequest`。表请求在配置旧表时生成 `old_keyspace || GenTableRecordPrefix(old_table_id)` 到 `new_keyspace || GenTableRecordPrefix(table_id)` 的 rewrite 规则；随后根据 `TableInfo::IsCommonHandle` 选择完整非空范围或完整整数范围，并由 `RequestBuilder` 写入范围、TS、checksum 载荷、并发、资源组和请求来源。之后按 `TableInfo::Indices` 原顺序遍历，只为 `StatePublic` 索引调用 `buildIndexRequest`；索引 rewrite 使用 `EncodeTableIndexPrefix(table_id, index_id)`。

执行时，`Executor::Execute` 串行遍历 `reqs`。每个请求建立新的 `Variables`，仅在 `backoff_weight > 0` 时覆盖其 `BackOffWeight`，再通过 `WithRetry(..., NewChecksumBackoffStrategy())` 调用 `sendChecksumRequest`。后者取得响应流并逐块 `Unmarshal`，块内结果先聚合为单请求响应；请求成功后再聚合进总响应并调用一次 `update_fn`。所有请求处理完毕后，`checkContextDone` 才决定是否返回聚合结果。

`Len` 和 `Each` 不发送请求；`RawRequests` 也只反序列化每个 `Request::Data` 中的 checksum 载荷，适合检查请求顺序、扫描类型和 rewrite 规则。

## 数据与状态

`ExecutorBuilder` 在构造期拥有 `TableInfo` 和可选 `MetaTable`，链式 setter 通过移动所有权避免共享可变状态。`Build` 消耗 builder，并把不可再变的请求向量与退避权重交给 `Executor`。请求数量的基本公式是每个逻辑表/分区一个表请求，加上该表每个公开索引一个索引请求；非公开索引不会进入向量。

rewrite 状态只有在提供 `old_table` 时产生。记录和索引规则都把调用者给出的 keyspace 字节原样放在编码前缀之前；没有旧表时 `old_table_id`/`old_part_id` 的零值不会进入规则，因为规则本身为 `None`。旧索引以完整 `CIStr` 相等匹配，而旧分区由 `GetPartitionByName` 按其实现的规范化名称查找，两者匹配规则不同。

执行期总响应从 `ChecksumResponse::default()` 开始。每个响应块以及每个请求结果都复用 `updateChecksumResponse`：校验和满足 XOR 的结合性，`TotalKvs` 与 `TotalBytes` 模拟 Go `uint64` 的模 $2^{64}$ 加法。`resp: Option<ChecksumResponse>` 由成功的重试闭包填入；`WithRetry` 成功后代码依赖这一不变量并 `unwrap`。

## 依赖与调用关系

上游生产边为 `br/pkg/backup/schema.rs::schemaInfo::calculateChecksum` → `NewExecutorBuilder` → `ExecutorBuilder::Build` → `Executor::Execute`。`br/pkg/checksum/lib.rs` 负责模块装配和扁平重导出。当前 RustCodeGraph 的通用 `callers`/`callees` 查询在本次分析中超时，因此具体生产调用点使用精确源码搜索核实；文件节点确认索引覆盖本文件且记录了引用文件集合。

内部构造调用链为 `Build` → `buildChecksumRequest` → `buildRequest` → `buildTableRequest`/`buildIndexRequest` → `RequestBuilder::Build`。执行调用链为 `Execute` → `WithRetry` → `sendChecksumRequest` → `DistSQLChecksum`/`Response::{NextRaw, Close}`，聚合统一落到 `updateChecksumResponse`，末尾调用 `checkContextDone`。

所有下游类型和函数均由 `crate::stubs` 导入。关键边界包括 `Client`/`Response` traits、`RequestBuilder`、`Context`、表/索引元数据、编码前缀与范围函数、重试策略及 protobuf 风格的 marshal/unmarshal。`Cargo.toml` 的空 `[dependencies]` 进一步证明该 crate 当前通过本地适配层隔离重依赖；因此这里的 `Client` 接线能力应按仓库当前实现理解，不能等同于已经直接链接完整的 `tikv-client`。

## 错误处理与边界

- 构造函数逐层以 `Error::Trace` 包装并返回 `RequestBuilder`、分区查找和反序列化错误；不会返回半成品 `Executor`。
- 配置旧表但找不到完全同名旧索引时，`buildRequest` 明确 `panic!`，对齐 Go 的 `log.Panic`。`parity_test.rs::rewrite_requires_exact_index_cistr_match` 证明仅大小写表现不同的 `CIStr` 也不能静默匹配。
- 配置旧表但旧表缺少同名分区时，`GetPartitionByName` 返回错误，`Build` 失败；`parity_test.rs::go_rust_public_contract_matches` 覆盖此路径。
- 非 `StatePublic` 索引被跳过；common handle 与整数 handle 使用不同完整范围。`parity_test.rs` 分别验证整数编码边界、common-handle 标志和索引范围。
- `DistSQLChecksum` 会拒绝客户端返回 `None` 响应；`NextRaw` 或 `ChecksumResponse::Unmarshal` 失败时仍调用 `Close`。若读取/反序列化错误与关闭错误同时发生，关闭错误覆盖前者；成功读取后关闭失败也使整个请求失败。
- `WithRetry` 处理发送、读取、反序列化、关闭和 failpoint 注入形成的错误。`take_checksum_retry_err` 是一次性测试注入；重试耗尽或上下文完成时错误向上返回，后续请求与进度回调不再执行。
- 即使所有请求已成功聚合，最终 `checkContextDone` 仍可能把结果转为取消错误，防止上游接受上下文完成后可能不完整的 checksum。

## 并发与资源生命周期

`concurrency` 是写入每个 DistSQL `Request` 的扫描并发参数，不代表本文件并行调度请求；`Executor::Execute` 本身按向量顺序同步执行。`update_fn` 也是每个成功请求完成后在当前调用线程同步触发一次。

每次重试尝试都会新建 `Variables`，`killed` 当前固定为 `0`，只是保留 Go `SessionVars.Killed` 的槽位。`NewChecksumBackoffStrategy` 在 `stubs.rs` 中提供有上限的指数退避；上下文取消可中断继续退避。测试专用线程局部开关能够跳过 sleep 或注入一次重试错误，不属于跨线程共享的生产控制面。

每次成功取得响应流后，`sendChecksumRequest` 都显式调用 `Close`：正常 EOF 后关闭一次，读取或解码失败时也先关闭再返回。`executor_test.rs` 和 `parity_test.rs` 的 `TrackingResponse`/`close_calls` 对关闭次数与失败传播进行观察。`Executor` 不持有后台任务、锁、通道或长期连接；客户端及上下文由调用者借用，响应流的生命周期限于单次 `sendChecksumRequest`。

## 与 Go 版本的对应关系

Rust 文件按函数结构直接对应 `executor.go`：builder 字段与 setter、表/分区/索引展开顺序、低优先级请求、Crc64 XOR、rewrite 前缀、逐请求重试、进度回调以及末尾上下文检查均保持一致。Rust 使用所有权式链式 setter，而 Go 在指针 receiver 上原地修改；可观察构造结果一致。

需要特别保留的语义差异处理有三处：Rust 的 `updateChecksumResponse` 使用 `wrapping_add` 明确复制 Go `uint64` 溢出回绕；Rust 在所有提前返回分支显式调用 `Close` 并让关闭错误覆盖原错误，以复制 Go named-return `defer`；Go failpoint 由 `stubs.rs` 的线程局部 consume-once 标志代替。`executor_test.rs` 对应 Go `executor_test.go` 的主要正常、取消、rewrite、common-handle 和 failpoint 场景，`executor_nokit_test.rs` 对应 Go 的 request-source setter 用例，`parity_test.rs` 补充精确编码、错误覆盖、分区失败、重试权重与回调次数等契约矩阵。

当前 Rust crate 还通过 `stubs.rs` 模拟 `kv.Client`、DistSQL、元数据和编码依赖；Go 版本则直接依赖 TiDB/TiKV 包。因此本文能够确认接口和受测语义对齐，但不能据此宣称 Rust 路径已具备 Go 生产栈的全部网络、调度或存储行为。

## 扩展指南

新增 builder 配置时，应在 `ExecutorBuilder` 中保存状态，在 `Build` 及相应的 `build*Request` 参数链中完整传递，最后由 `RequestBuilder` 写入每条表、分区和索引请求；同时更新 `executor_nokit_test.rs` 或 `parity_test.rs`，验证默认值、覆盖值及所有请求上的传播。不要只修改表请求而遗漏索引或分区请求。

新增扫描对象或改变展开规则时，优先修改 `buildChecksumRequest`/`buildRequest`，保持稳定顺序，并重新核对 `Len`、`Each`、`RawRequests` 和 `update_fn` 次数。涉及 rewrite 时必须同时定义旧/新对象的匹配规则和 keyspace 前缀，补充分区缺失、旧索引缺失、非公开状态等失败用例。

改变传输或重试逻辑时，应集中在 `sendChecksumRequest`/`Execute`，保留“每次响应必关闭、关闭错误覆盖、失败不触发进度、最终检查上下文”的契约。同步测试应放在独立的 `executor_test.rs` 或 `parity_test.rs`，不要把测试内嵌到生产文件；若 Go 行为同时变化，还需更新同路径 Go 测试和本文的对照说明。性能风险主要来自分区乘索引导致的请求数增长、串行请求执行和重试退避；兼容风险主要来自键范围编码、rewrite 前缀、索引名匹配和聚合算术变化。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter br/pkg/checksum` 列出目标、crate 根、桩和独立测试；`node --file br/pkg/checksum/executor.rs --offset 1 --limit 500` 及后续 500--583 行读取了完整 583 行源码，并报告本文件被 7 个文件引用。精确 `query ExecutorBuilder` 与 `query sendChecksumRequest` 确认 Rust/Go 对照符号位置；通用 callers/callees 命令本次超时，故调用边又由源码入口和精确文本搜索交叉核验。
- 生产与 crate 证据：`br/pkg/checksum/executor.rs`、`br/pkg/checksum/lib.rs`、`br/pkg/checksum/stubs.rs`、`br/pkg/checksum/Cargo.toml`、`br/pkg/backup/schema.rs::schemaInfo::calculateChecksum`。目标目录没有 `doc.go`。
- Go 对照：`br/pkg/checksum/executor.go`、`executor_test.go`、`executor_nokit_test.go`、`main_test.go`。
- Rust 测试证据：`br/pkg/checksum/executor_test.rs`、`executor_nokit_test.rs`、`parity_test.rs`；覆盖请求构造与顺序、范围编码、rewrite、XOR/回绕累加、上下文取消、重试、退避权重、关闭错误、资源释放和回调次数。
- 本任务是只新增说明文档的分析任务，按计划不运行 Cargo；交付检查使用任务指定的 11 个固定章节结构命令，并人工复核所有主要结论均能回指上述符号或文件。
