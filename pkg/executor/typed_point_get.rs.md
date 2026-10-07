# `pkg/executor/typed_point_get.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 入口 `pkg/executor/lib.rs` 将其公开为 `pub mod typed_point_get`，依赖边界由 `pkg/executor/Cargo.toml` 声明。它把物理计划 `PointGetPlan` 落成一个实现 `ExecExecutor` 的单行读取器，位于“planner 产出点查计划 → `builder::BuildTypedPointGet` 构建 → session runtime 绑定快照/事务 → executor 输出 `Chunk`”这条链上。

直接生产入口是 `pkg/executor/builder.rs::BuildTypedPointGet`。实际 session 接线位于 `pkg/session/runtime/scan_adapter_runtime.rs`：普通路径返回 `TypedPointGet`，满足 MaxTS、非锁读、无事务、无显式分区且非 table-dual 等条件的 prepared point get 则缓存 `Arc<Mutex<TypedPointGet>>`，并通过 `SharedTypedPointGet` 暴露同一 actor。

## 核心职责

- `TypedPointGet` 用已固定的快照 `kv::Retriever` 做至多一次点查，不创建范围迭代器。主键/句柄计划直接编码 record key；唯一索引计划先编码并读取 index key、解出行句柄，再读取 record key（`TypedPointGet::next_inner`）。
- 它把 record value 交给 `TypedKVScan::append_decoded_row_for_table` 解码到调用者提供的 `chunk::Chunk`，并记录本页实际返回的 record key 与扫描行数。
- `PointLockRuntime` 将 session/事务相关的本地写缓冲读取和悲观锁操作留在 session crate；本文件只实施 RC/RR 的点查锁顺序。
- 对“唯一索引存在、对应行不存在”的强一致性读，使用结构化 consistency reporter 或兼容错误 8133 报告索引/记录不一致；弱一致性读返回空结果。
- `RecreatedFromPlan`、`RebindRetriever` 和 `SharedTypedPointGet` 支撑形状不变的 prepared MaxTS 点查 actor 复用，同时确保每条语句改绑自己的快照与统计源。

## 主要符号

- `PointLockRuntime`：公开 trait。`IsReadCommitted` 决定 RC/RR 分支；`LocalValue` 返回三态：`None` 表示本地事务没有覆盖，`Some(Some(bytes))` 表示本地值，`Some(None)` 表示本地删除；`LockKey(key, only_if_exists, wait_ms)` 执行规范化悲观锁并可返回当前值。生产实现是 `pkg/session/runtime/scan_adapter_runtime.rs::SessionPointLockRuntime`。
- `TypedPointGet`：核心状态机。计划形状字段包括逻辑/物理表 ID、句柄、索引 ID/元数据/值和列数；运行期字段包括 retriever、`TypedKVScan` decoder、锁配置、一致性诊断配置、生命周期标记和统计。
- `TypedPointGet::new`：crate 内构造器，创建 decoder 并设定默认诊断、锁和生命周期状态。公开构建器随后用 `WithIndexMetadata` 与 `WithLockPlan` 补齐计划元数据。
- `WithConsistencyDiagnostics` / `SetDiagnosticMode`：分别注入 reporter 的日志/存储后端，以及逐语句更新弱一致性和脱敏模式。
- `RecreatedFromPlan`：只允许关闭/未打开的 actor 复用；校验表、分区、锁、索引、输出列和索引值个数等形状未改变，然后只更新句柄、索引值、table-dual 和运行状态。
- `RebindRetriever`：在可复用状态下同步替换自身与 decoder 的 retriever，防止缓存 actor 继续读取上一语句快照。
- `get_optional` / `get_and_lock_optional`：把 `kv::ErrNotExist` 规范化为空值，并实现本地值优先、RC 仅锁存在键、RR 先锁（包括不存在键）再从本地/固定快照读取。
- `next_inner`：唯一的数据路径，承担生命周期检查、kill 检查、索引/行两段读取、分区句柄处理、一致性诊断、行解码和计数。
- `SharedTypedPointGet`：持有 `Arc<Mutex<TypedPointGet>>` 的公开包装器。构造时复制 schema/chunk config，其余有状态操作在互斥锁内转发给 actor。
- 两个 `ExecExecutor` 实现：提供 `Open/Close/Next`、schema/chunk、锁键与扫描行数；该执行器不是写执行器，没有外键检查/cascade，并声明 `CalculateNoDelay == false`。

## 执行流程

1. `BuildTypedPointGet` 要求计划含 `TableInfo`，解析显式分区得到 physical table ID；索引点查必须是完整唯一键，然后调用 `TypedPointGet::new`，附加索引元数据和锁计划。
2. session runtime 为当前 statement 创建 `OwnedKVSnapshotSource`，设置弱一致性/脱敏模式；存在事务时还注入 `SessionPointLockRuntime`。缓存路径先执行 `RecreatedFromPlan`，成功后用 `RebindRetriever` 改绑当前语句快照，再返回 `SharedTypedPointGet`。
3. `Open` 校验锁读必须有 lock runtime，随后清空 `done`、page keys 和扫描计数，并进入 opened 状态。`Next`/`NextWithContext` 进入 `next_inner`；后者会在开始、索引读取后和 record 读取后检查 SQL killer。
4. `next_inner` 先重置输出和本页 key。第二次及后续调用因 `done` 直接返回空；`table_dual` 也返回空，因此一次 open 周期最多产生一行。
5. 索引计划优先用完整 `TableInfo`/`IndexInfo` 调 `GenIndexKey`；global index 用逻辑表 ID，local index 用物理表 ID。若缺少元数据则使用兼容编码路径。非 distinct 编码或缺失 index KV 都视为空结果。
6. 索引值由 `DecodeIndexHandle` 解成 handle；若为 `PartitionHandle`，record key 使用其中的 partition ID，否则使用计划物理表 ID。句柄计划则要求 `handle: Some(i64)` 并直接创建 `kv::IntHandle`。
7. 对 index key 和 record key 都通过 `get_and_lock_optional` 读取。record 缺失时：无索引则返回空；有索引且弱一致则返回空；强一致则报告 8133 数据不一致。
8. record 存在时，decoder 按实际 table ID 解码一行到 chunk，只把 record key 放入 `page_keys`，并将 `scanned_rows` 加一。`TakeLockKeys` 以 `mem::take` 转移这些 key。
9. `Close` 只将 actor 标为 closed；缓存 actor 此后可复用。`Detach` 对非锁读克隆快照读取所需状态与 detached decoder，但锁读返回 `None`，避免把 session 绑定的锁运行时跨边界复制。

## 数据与状态

- `logical_table_id` 用于校验计划形状、global index 编码和无元数据错误文本；`physical_table_id` 表示普通表或已选分区，供 local index 与 record key 使用。index value 解出 `PartitionHandle` 时可以覆盖实际 record table ID。
- `handle` 与 `index_id/index_values/index_columns` 是互斥的两类访问描述。构建器保证索引完整且唯一；`next_inner` 仍防御缺失 handle、缺失解码 handle和非 distinct key。
- `opened/closed/done` 组成轻量生命周期状态机：未 open 或已 close 时 `Next` 报错；一次成功进入 `next_inner` 后立即置 `done`，即使最终为空或报一致性错误，也不会在同一 open 周期重试。
- `page_keys` 仅收集成功解码行的 record key，不包含中间 index key；`TakeLockKeys` 消耗集合。`scanned_rows` 只在成功追加行后增加，`Open` 重置它，`Detach` 复制当前计数。
- `weak_consistency`、`redact_mode`、`consistency_logger/storage` 只影响索引已命中而 record 缺失时的处理与诊断，不改变正常解码。
- `retriever` 是 statement 固定快照的拥有型 trait object；decoder 持有对应克隆。缓存复用必须同时改绑两处，这由 `RebindRetriever` 保证。

## 依赖与调用关系

上游调用链为：`pkg/session/runtime/scan_adapter_runtime.rs` → `pkg/executor/builder.rs::BuildTypedPointGet` → `TypedPointGet::new` → `ExecExecutor::{Open,NextWithContext/Next,Close}`。prepared MaxTS 缓存链额外经过 `RecreatedFromPlan`、`RebindRetriever` 与 `SharedTypedPointGet::new`。

主要下游依赖如下：

- `astersql-kv::{Retriever, Key, Handle, IntHandle, PartitionHandle}`：点读、句柄抽象和不存在错误判定。
- `astersql-tablecodec` 与 `astersql-util-codec`：唯一索引 key、record key、索引 handle 和 datum 编码/解码。
- `TypedKVScan`：复用列 schema、chunk 配置与行值解码，而不复用其范围扫描流程。
- `astersql-util-logutil-consistency::Reporter`：索引/记录不一致的结构化日志和错误生成。
- `ExecutionContext.sql_killer`：带上下文的执行路径在 I/O 阶段之间响应 kill 信号。
- `pkg/session/runtime/scan_adapter_runtime.rs::SessionPointLockRuntime`：读取事务本地写入/删除，校验悲观事务、申请行锁、执行 RC/RR 冲突判断。

`pkg/executor/Cargo.toml` 明确声明了上述 errors、kv、meta-model、physicalop、tablecodec、types、chunk、codec 和 consistency 本地 crate 依赖；本模块没有独立 feature gate。

## 错误处理与边界

- 生命周期错误：open 状态下不能重建计划或改绑 retriever；未 open/已 close 不能 next；锁读在 open 时必须已注入 canonical lock runtime。
- 计划/编码错误：缺失表信息在 builder 阶段失败；无句柄、索引 KV 无法解出 handle、索引编码/行解码失败均原样传播为 `AdapterResult` 错误。
- KV 不存在是预期控制流：index 或直接 record 不存在返回零行；但已存在唯一索引而 record 缺失是强一致性错误。元数据齐全时 reporter 可记录表名、索引名和受脱敏配置约束的诊断；元数据不全时生成兼容的 `[executor:8133]` 错误。
- `table_dual`、非 distinct 索引 key 和弱一致性下的缺行都静默返回空 chunk。输出在每次 next 开始时重置，错误前不会留下部分行。
- `Mutex::lock` 使用 `expect("PointGet actor lock poisoned")`；actor 内发生 panic 导致锁中毒时，后续包装器调用也会 panic，而不是转换为 `AdapterResult`。
- 当前 Rust 构建器拒绝非唯一或值数不完整的 index point get；显式 `PartitionIdx` 禁止缓存 actor 的 `RecreatedFromPlan` 复用。扩展这些边界前必须同步审查 builder 与 session 缓存条件。

## 并发与资源生命周期

`TypedPointGet` 自身不是并发容器。锁运行时使用 `Rc<dyn PointLockRuntime>`，表明普通执行器绑定当前 session/线程；持有它的锁读不能 `Detach`。快照 retriever、consistency sink/storage 使用 `Arc`，可供 detached decoder 或共享 actor 保存拥有权。

`SharedTypedPointGet` 用 `Arc<Mutex<_>>` 串行化同一缓存 actor 的 open/next/close、统计读取和 detach。它在构造时复制不可变的 schema 与 chunk config，使 `Schema`/`NewChunk` 无需持锁；真正的读取状态始终在 mutex 内。session runtime 只在 actor `IsReusable`（从未 open 或已 close）、计划形状校验通过时复用，并在交给新 record set 前换成当前 statement retriever。

一次 open 周期最多执行一次点查；`Close` 不释放 retriever，而是把 actor转为可复用状态。悲观行锁的实际持有、等待和释放属于 session/transaction 生命周期，不由本文件释放；本文件只传递 `lock_wait_ms` 并收集成功返回行的 key。

## 与 Go 版本的对应关系

直接对照是 `pkg/executor/point_get.go::PointGetExecutor`。两版共同保留了核心语义：一次 `Next` 至多一行；唯一索引先找 handle 再取 record；global index 可携带 partition ID；RC 锁只锁存在键，RR 在读取前锁键；事务本地值优先；唯一索引命中但 record 缺失时强一致性报 8133、弱一致性返回空。

Rust 将 Go 中紧耦合于 `sessionctx.Context`、transaction 与 snapshot 的部分拆到 `PointLockRuntime` 和 `OwnedKVSnapshotSource`，并新增明确的 `opened/closed` 状态及 `SharedTypedPointGet` 缓存 actor。`RecreatedFromPlan` 对应 Go 的 `Init/Recreated` 复用意图，但 Rust 只接受严格相同且非锁、非显式分区的形状（调用侧还限制 MaxTS、无事务等条件）。

当前 Rust 文件不是 Go 文件全部能力的等量搬运。Go 实现还直接处理临时/缓存表、txn/read-replica scope、runtime stats/index usage、common-handle 特例、分区名过滤与 DDL ignore、row checksum 和虚拟列填充等；这些逻辑在本 Rust 文件中没有对应分支，不能从 Go 行为推断为已支持。Rust 的实际行解码能力以 `TypedKVScan` 及当前 builder/session 接线为准。

## 扩展指南

- 增加新的 key/handle 形式时，优先修改 `BuildTypedPointGet` 的计划约束和 `next_inner` 的 key/handle 分支，并在独立文件 `pkg/executor/typed_point_get_test.rs` 添加编码、缺失值、分区/global index 与错误传播用例；不要把测试嵌入生产文件。
- 改变锁语义时必须成对审查 `get_and_lock_optional` 与 `SessionPointLockRuntime::{LocalValue,LockKey}`，分别覆盖 RC/RR、存在/不存在、本地写入/删除、等待超时和写冲突。锁读仍应禁止 detach，除非引入可证明安全的 session 生命周期模型。
- 扩展缓存复用字段时，应同时更新 `RecreatedFromPlan` 的形状校验和状态重置、`RebindRetriever` 的所有快照持有者，以及 session runtime 的 cache eligibility；漏掉字段会让上一语句状态泄漏到下一语句。
- 增加统计或 page-key 语义时，应明确 index key 与 record key 是否计入、何时清零、`TakeLockKeys` 是否消费，并同步 `SharedTypedPointGet` 的转发接口。
- 对齐 Go 的 checksum、虚拟列、common handle、临时表或运行统计时，应以 `pkg/executor/point_get.go` 的相应分支为语义基线，但只接入当前 Rust 架构所需的 decoder、builder 和 session 边界，不用桩或编译通过代替行为验证。
- 性能敏感点是点查 RPC 次数、索引 key 编码、mutex 临界区和不必要的数据克隆；功能扩展应保持句柄直查单次 KV、索引查最多两段 KV 的基本路径，并用精确测试验证空结果不会额外扫描。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`explore "pkg/executor/typed_point_get.rs TypedPointGetExecutor"` 与 `node --file pkg/executor/typed_point_get.rs` 核对了完整 574 行实现；对 builder、session runtime、Rust 测试和 Go 对照文件执行了按行 `node` 查询。图显示本文件被 `pkg/session/runtime/scan_adapter_runtime.rs` 使用。
- 源码与接线：`pkg/executor/typed_point_get.rs`、`pkg/executor/builder.rs::BuildTypedPointGet`、`pkg/session/runtime/scan_adapter_runtime.rs::{SessionPointLockRuntime, BuildPhysicalExecutor 相关分支}`、`pkg/executor/lib.rs`。
- crate 边界：`pkg/executor/Cargo.toml` 的 package、lib、feature、porting metadata 与相关本地依赖声明。
- 独立 Rust 测试：`pkg/executor/typed_point_get_test.rs` 覆盖句柄点查/无范围迭代器、分区索引、table-dual、缺失索引、8133、规范索引编码、结构化 reporter、弱一致性和快照重绑定。该文件当前没有锁运行时专用单元测试，因此锁语义还以实现与 session runtime 代码为直接证据。
- Go 对照：`pkg/executor/point_get.go::{buildPointGet, PointGetExecutor.Init, Open, Close, Next, getAndLock, lockKeyBase, get}`；用于确认共同语义及明确 Rust 尚未覆盖的能力。
- 本任务是纯文档分析，依计划不运行 Cargo。交付前另运行任务指定的 11 章节结构命令，并人工复核所有“已支持”陈述都能回链到上述实现或测试。
