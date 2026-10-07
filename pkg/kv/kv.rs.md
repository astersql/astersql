# `pkg/kv/kv.rs`

## 文件定位

`pkg/kv/kv.rs` 是 `astersql-kv` crate 的核心契约文件。crate 入口 `pkg/kv/lib.rs:494-500` 通过私有模块 `kv` 包含本文件并执行 `pub use kv::*`，因此这里定义的 trait、请求结构、常量与辅助函数构成上层 crate 实际使用的公开 KV API。`pkg/kv/Cargo.toml` 将库入口指定为 `lib.rs`，默认采用 Classic 内核；`nextgen` feature 只向 `kerneltype`、`keyspace` 传播。

它位于 SQL 层与具体存储实现之间：上层会话、执行器、DDL、TTL、domain 和 distsql 面向这里的 `Storage`、`Transaction`、`Snapshot`、`Client`、`Request` 等抽象编程，具体 TiKV/本地存储适配器再实现这些契约。例如 `pkg/store/driver/kv_adapter.rs` 实现了 `Storage` 的 DDL region、TTL region、TiFlash placement 与 SST 导入能力；`pkg/session/runtime/import_sst.rs`、`ttl_metadata.rs`、`modify_column_cloud_planner.rs` 和 `relational_scan.rs` 分别消费这些能力。

本文件不是存储算法实现，也不是纯门面：它既定义跨模块协议，也实现范围整理、并发准入、标签编码等可执行逻辑。键、错误、选项、MPP 等配套类型分别来自同 crate 的 `key.rs`、`error.rs`、`option.rs`、`mpp.rs` 及 `lib.rs` 中的依赖适配模块。

## 核心职责

1. 定义读取、写入和迭代的最小能力层次：`Getter` → `Retriever`，`Mutator`，以及组合 trait `RetrieverMutator`；用 `EmptyRetriever`/`EmptyIterator` 提供确定的空实现（`kv.rs:78-168`）。
2. 定义事务内存层和事务生命周期：`MemBuffer` 暴露 staging、flags、快照视图与批量读；`Transaction` 在此之上增加提交、回滚、悲观锁、公平锁、事务选项、表信息缓存和 pipeline DML 能力（`kv.rs:170-291`）。
3. 定义存储访问边界：`Client`/`Response`/`ResultSubset` 描述 coprocessor 请求流，`Snapshot` 描述版本化读取，`Driver`/`Storage` 描述存储实例及 Oracle、PD、DDL/TTL/TiFlash/SST 等可选能力（`kv.rs:293-314, 644-918`）。
4. 承载请求协议数据：请求/子请求类型编号、`StoreType`、`IsoLevel`、优先级、`Request`、`Paging`、`PartitionIDAndRanges` 和 `ClientSendOption`（`kv.rs:305-350, 479-640, 948-959`）。
5. 维护扫描范围结构：`KeyRanges` 保存“分区 → range”的二维结构以及对应 row-count hints，并负责转换、展平、排序、遍历和一致性检查（`kv.rs:352-477`）。
6. 限制 coprocessor 并发：`CoprRequestLimiter` 控制一组共享请求的总在途量，`QueryCopStoreLimiter` 为每个非零 store ID 延迟创建并复用独立 limiter（`kv.rs:488-583`）。
7. 构造资源组标签：`ResourceGroupTagBuilder` 将 keyspace、SQL/plan digest、表 ID 和 key label 编码为 protobuf，再写入 TiKV RPC 请求（`kv.rs:961-1035`）。
8. 保存与 Go/API 兼容有关的全局值与适配函数，包括事务大小原子限制、client value/option 别名、二进制键映射、资源组解码回调等（`kv.rs:32-76, 1029-1035`）。

## 主要符号

- `UnCommitIndexKVFlag`：值为字节 `'1'`，标记更新时未改变、无需提交的索引 KV。`TxnEntrySizeLimit` 与 `TxnTotalSizeLimit` 是以配置默认值初始化的 `AtomicU64`，允许进程内动态读取/更新限制（`kv.rs:32-38`）。
- `KeyMapName(&[u8]) -> String`：将任意二进制 key 逐字节编码为小写十六进制。Rust 的批量读映射使用 `String` 键时借此避免 UTF-8 有损转换碰撞；这是 Rust 相对 Go `map[string]` 直接承载字节串所需的适配（`kv.rs:47-57`）。
- `NewValueEntry`、`BatchGetToGetOptions`、`WithReturnCommitTS`：转发到底层 `tikvstore` 适配层。空 `BatchGetOption` 输入返回 `None`，对应 Go 的 nil slice，而非空输入转换为逐键选项（`kv.rs:59-76`）。
- `Getter`、`Retriever`、`Mutator`、`Iterator`：基础 KV 操作契约。正向迭代范围为起点到可选上界，反向迭代接受可选起点和下界；迭代推进可能失败，持有者必须调用 `Close`（`kv.rs:78-109, 157-161, 922-929`）。
- `EmptyIterator`、`EmptyRetriever`：空读取实现。`Get` 返回 `ErrNotExist`，两个迭代入口返回无效 iterator；对无效 iterator 调用 `Next` 返回错误，`Close` 无操作（`kv.rs:111-155`）。
- `MemBuffer` 与 `StagingHandle`：事务内写集合。`Staging` 创建临时层，`Release` 发布到上层，`Cleanup` 丢弃未发布修改；`InspectStage` 是 `FindKeysInStage` 的遍历基础。句柄 `0` 无效，`-1` 指向最后一个活动 stage（`kv.rs:163-219`）。
- `Transaction`：组合 `RetrieverMutator` 和 `FairLockingController`。默认 `StageStatement`、`ReleaseStatement`、`CleanupStatement` 返回 `ErrNotImplemented`，因此实现者若支持 statement 级远端/本地回滚，必须显式覆盖；其余方法涵盖 commit/rollback、锁键、选项、快照、mem-buffer checkpoint、pipelined flush 等（`kv.rs:223-282`）。
- `FairLockingController`：公平锁的 start、retry、cancel、done 四阶段协议及模式查询（`kv.rs:284-291`）。
- `Client`、`ClientSendOption`：向 KV 层发送 `Request` 并返回可选 `Response`，同时携带内存追踪、事件回调、限流开关、TiFlash replica read 和 warning 回调（`kv.rs:293-314`）。
- `StoreType`：`TiKV=0`、`TiFlash=1`、`TiDB=2`、`UnSpecified=255`，`Name` 返回稳定的小写名称（`kv.rs:330-350`）。
- `KeyRanges`：私有字段保护二维范围、hints 与 partitioned 标志的不变量；构造器区分分区/非分区请求，方法负责只读访问、状态转换和排序（`kv.rs:352-477`）。
- `CoprRequestLimiter`：`Mutex<usize> + Condvar + capacity` 的共享计数器。`AcquireWithContext` 成功时返回 `false` 且调用方必须释放；上下文或 done token 取消时返回 `true`。`TryAcquire` 不等待，`Release` 对冗余释放直接 panic（`kv.rs:488-549`）。
- `QueryCopStoreLimiter`：`Mutex<HashMap<u64, Arc<CoprRequestLimiter>>>` 保存 statement/query 作用域的按 store 限流器；store ID 0 不限流，同一 ID 返回同一 `Arc`（`kv.rs:551-583`）。
- `Request`：coprocessor/KV 请求的聚合载体，包含时间戳、范围、并发、限流器、隔离级别、顺序/缓存/副本策略、资源组、分页、超时、key 读取预算、runaway/resource-control hook 和连接标识（`kv.rs:585-629`）。
- `ResultSubset`、`Response`：流式结果块与结果迭代协议。`Next` 以 `Ok(None)` 表示耗尽，`Close` 负责释放底层请求资源；Rust 还允许结果块提供 read-pool 与 cop runtime 证据（`kv.rs:642-674`）。
- `Snapshot`、`SnapshotInterceptor`、`BatchGetter`：版本快照读取、四类读取拦截点以及批量读取能力。`SnapCacheSize` 的默认值为 0，后端可覆盖以暴露缓存条目数（`kv.rs:676-731`）。
- `SSTWriteLimiter`、`SSTImportOptions`、`SSTImportStats`：物理 SST 导入的限速、取消上下文、统计数据契约（`kv.rs:751-769`）。
- `Storage`：全局存储接口。核心必实现方法包括 begin/snapshot/client/MPP/oracle/version/close/status/cache/lock wait/codec/options/cluster/keyspace；DDL/TTL/TiFlash/SST/PD 编码相关方法带有保守默认行为（`kv.rs:771-905`）。
- `EtcdBackend`、`StorageWithPD`、`SplittableStore`：分别标识真实 TiKV 的 etcd/GC 能力、PD client 能力和 region split/scatter 能力（`kv.rs:907-946`）。
- `ResourceGroupTagBuilder` 与 `DecodeTableIDFunc`：builder 保存 digest/keyspace，通过可替换函数指针解出 table ID，以避免 KV crate 反向依赖 tablecodec（`kv.rs:961-1035`）。

## 执行流程

### 点读与批量读

`GetValue` 调用传入 `Getter::Get(ctx, key, &[])`，成功后只返回 `ValueEntry.Value`，提交时间戳等元数据被丢弃；错误通过 `?` 原样传播（`kv.rs:88-95`）。`BatchGetValue` 同理先调用 `BatchGetter::BatchGet`，再将每个 `ValueEntry` 映射成 value 字节；映射键采用实现返回的 `String`（`kv.rs:733-744`）。调用方若需要 commit TS，应使用 `WithReturnCommitTS` 并直接消费 `ValueEntry`。

### MemBuffer staging

调用方先用 `Staging` 建立最新临时写层，在该层执行 `SetWithFlags`、`DeleteWithFlags` 或 flags 更新；校验成功后用 `Release(handle)` 合并到上层，失败路径用 `Cleanup(handle)` 丢弃。`FindKeysInStage` 通过 `InspectStage` 遍历指定层，只把 predicate 接受的 key 收集到新向量（`kv.rs:185-219`）。statement 级 staging 通过 `Transaction::{StageStatement,ReleaseStatement,CleanupStatement}` 与远端 staged writes 一并协调，但默认实现明确拒绝支持。

### 分区范围整理

构造器将分区请求保留为二维 ranges，将非分区请求包装成单元素二维向量。`SetToNonPartitioned` 只允许至多一个分区；多分区时返回错误，避免错误降格。`SortByFunc` 先检查/排序各分区首 range 的顺序，再检查/排序每个分区内部，减少已排序输入的工作。`ForEachPartitionWithErr` 将同下标 hints（缺失则空 slice）交给回调并在首个错误停止。`IsFullySorted` 同时检查分区首 `StartKey` 和分区内部 `StartKey` 单调性（`kv.rs:359-477`）。

### coprocessor 并发准入

`NewCoprRequestLimiter` 对非正容量返回 `None`。等待者进入 `AcquireWithContext` 后持有计数锁循环：先检查两个取消源，再检查容量；有空位时递增并返回 `false`，满载时以 10ms 超时等待 condvar，以弥补 `CancellationToken` 不能直接唤醒 `std::Condvar`。持 token 的调用方完成后必须 `Release`，它递减并唤醒一个等待者。`QueryCopStoreLimiter::GetStoreLimiter` 对非零 store ID 在锁内按需插入 limiter，同一 store 共享容量，不同 store 相互隔离（`kv.rs:497-583`）。`pkg/util/mock/context.rs:626` 将 session 变量 `QueryCopStoreLimit` 转换为 query limiter；store/cop/distsql 测试文件进一步验证请求侧接线。

### 存储扩展能力与 SST 导入

`Storage` 的通用主链是 `Begin`/`GetSnapshot` 获取事务或快照，`GetClient`/`GetMPPClient` 发送分布式请求，`CurrentVersion`/`GetOracle` 提供时间戳。`TimestampFuture` 优先向 Oracle 请求普通或低精度异步时间戳；Oracle 不提供 future 时，用 `CurrentVersion(scope)` 包装同步结果（`kv.rs:842-860`）。

DDL/TTL/TiFlash 可选方法默认返回 `None` 或无操作，表示非 PD/非 region store；查询失败仍以 `Err` 区分于“不支持”。`ImportSST` 默认报“不支持”，不得静默降级为 SQL 写入。`ImportSSTWithOptions` 先检查取消，再计算全部 key/value 字节并调用共享 limiter，最后委托 `ImportSST`；`pkg/session/runtime/import_sst.rs:232,286` 是直接调用者，`pkg/store/driver/kv_adapter.rs:2281-2283` 提供真实适配（`kv.rs:771-905`）。

### 资源组标签

builder 先收集 keyspace、SQL digest 和 plan digest。`EncodeTagWithKey` 仅写入非空字段；非空 key 还经 `DecodeTableIDFunc` 解出表 ID，并由 `resourcegrouptag` 推导 label。protobuf 编码成功返回字节，失败返回 `None`。`BuildProtoTagger` 生成闭包调用 `Build`；`Build` 从 RPC 请求提取首 key，仅在编码结果非空时覆盖 `ResourceGroupTag`（`kv.rs:968-1026`）。

## 数据与状态

- 全局可变状态有三处：两个原子事务大小限制，以及 `static mut DecodeTableIDFunc`。前两者适合跨线程无锁读写；后者依赖外部在初始化阶段安装稳定回调，读取必须经过 `unsafe`，未安装时表 ID 回退为 0（`kv.rs:37-38, 1029-1035`）。
- `KeyRanges` 将 `ranges[i]` 与可选 `rowCountHints[i]` 关联。hints 数量不足不会越界，而是传空 slice；接口没有强制 hints 长度与 range 数量相等，因此消费者必须把 hints 当提示而非完整元数据（`kv.rs:437-445`）。
- `Request` 主要是一次请求的值状态。两个 limiter 和 runaway/resource-control hook 通过共享引用跨子任务协调；`MaxKeysReadCounter: Option<AtomicU64>` 记录 statement 范围的 key 读取预算；多数布尔值和数值字段由请求构建器按执行计划设置（`kv.rs:585-629`）。
- `CoprRequestLimiter.in_flight` 始终应位于 `0..=capacity`。成功 acquire 增一，且必须恰好 release 一次；冗余 release 违反不变量并 panic。`QueryCopStoreLimiter.stores` 的 key 是 TiKV store ID，值在 limiter 生命周期内保持共享身份（`kv.rs:490-583`）。
- `SSTImportOptions` clone 时共享 `Arc<dyn SSTWriteLimiter>`，但复制可取消 `Context`；`SSTImportStats` 汇总 keys、bytes、write RPC 和 ingest RPC，具体值由实现填充（`kv.rs:751-769`）。
- `ResourceGroupTagBuilder` 的 setter 原地更新并返回 `&mut Self` 以支持链式调用；digest/keyspace 最终复制进 protobuf。builder 本身不发送请求（`kv.rs:961-1026`）。

## 依赖与调用关系

crate 边界由 `pkg/kv/Cargo.toml` 确认：本文件直接需要标准库同步原语、`tokio-util` cancellation、`protobuf`/`kvproto`，并通过 crate 入口使用 `astersql-config`、codec、dbterror、resourcegrouptag、execdetails 等本地依赖。`pkg/kv/lib.rs` 还提供迁移期的 `tikvstore`、`tikv`、`oracle`、`parser`、`memory`、`resourcegroup`、`tikvrpc` 等适配命名空间。

已核对的直接上游与下游关系包括：

- `pkg/util/mock/context.rs:626`：根据会话变量构造 `NewQueryCopStoreLimiter` 并装入请求上下文。
- `pkg/session/runtime/relational_scan.rs:2737`：调用 `Storage::TimestampFuture` 获取扫描时间戳。
- `pkg/session/runtime/import_sst.rs:232,286`：携带 `SSTImportOptions` 调用物理导入。
- `pkg/session/runtime/ttl_metadata.rs:69`：通过 `Storage::TTLRegionRanges` 获取 TTL 扫描边界。
- `pkg/session/runtime/modify_column_cloud_planner.rs:168`：读取 `DDLRegionSplitConfig`。
- `pkg/domain/domain.rs:3731-3739,3826-3829`：发布或删除 TiFlash learner placement rule。
- `pkg/store/driver/kv_adapter.rs:1989,2031,2214,2283`：实现上述存储扩展点与 SST 导入适配。
- `pkg/store/copr/**`、`pkg/store/driver/coprocessor_adapter_test.rs`、`pkg/distsql/**`：消费 `Request` 中的 request-level/per-store limiter，验证并发接线。
- `ResourceGroupTagBuilder` 下游调用 `parser::Digest::Bytes`、`resourcegrouptag::{GetFirstKeyFromRequest,GetResourceGroupLabelByKey}`、`tipb::ResourceGroupTag` protobuf 编码以及可选 `DecodeTableIDFunc`。

RustCodeGraph 报告 `pkg/kv/kv.rs` 被 35 个 Rust 文件使用；其精确 `callers/callees` 查询本次未产生结果，因此上述边均由针对符号的仓库精确搜索和对应源码位置复核，不据此推断未见的动态调用。

## 错误处理与边界

- 所有存储/事务/迭代错误统一使用 `errors::SharedError`。薄适配函数用 `?` 保留原始错误；partition callback 在首个错误短路（`GetValue`、`BatchGetValue`、`ForEachPartitionWithErr`）。
- 未命中由 `Getter` 实现返回 `ErrNotExist`；`EmptyRetriever` 明确遵守该约定。`Mutator::Set` 的契约禁止 nil/空 value，具体实现应返回 `ErrCannotSetNilValue`，本文件本身不执行检查（`kv.rs:78-86, 130-161`）。
- `Transaction` 的 statement staging 默认返回 `ErrNotImplemented`；调用方不能把 trait 存在误解为所有后端均支持（`kv.rs:224-235`）。
- `SetToNonPartitioned` 拒绝多分区输入。空 `KeyRanges` 的 `FirstPartitionRange` 返回空 slice，排序和 fully-sorted 检查把空分区视为不会破坏结果（`kv.rs:390-472`）。
- limiter 构造对容量 `<=0` 返回 `None`，store ID 0 返回 `None`；等待取消返回 `true` 且没有获得 token。`Release` 没有可恢复错误通道，冗余调用会 panic，锁 poisoned 时 `unwrap` 也会 panic（`kv.rs:497-580`）。
- 多数 `Storage` 扩展能力以 `None` 表示“不适用”；SST 导入、PD endpoints/keyspace ID 等若无法安全提供则返回明确错误。非空 keyspace 缺少 DDL codec 时不允许用 classic codec 猜测编码（`kv.rs:771-905`）。
- `ResourceGroupTagBuilder::EncodeTagWithKey` 将 protobuf 编码错误折叠为 `None`，`Build` 因而保留原请求标签；解码回调未安装时 table ID 为 0。Rust 的 `Build` 接收 `&mut Request`，因此不存在 Go `nil *Request` 的入口（`kv.rs:991-1035`）。

## 并发与资源生命周期

`Transaction` 的注释明确不保证线程安全；其 `&mut self` 写接口也要求调用方串行化事务变更。`MemBuffer` 例外地公开 `RLock`/`RUnlock`，用于 UnionScan 等多执行线程共享读取；这是一对协议方法，调用方必须保证配对（`kv.rs:170-204, 223-282`）。

迭代器和响应都具有显式资源生命周期：每个 `Retriever::Iter`/`IterReverse` 返回值最终必须 `Close`，`Response` 在耗尽或提前结束后也必须 `Close`。trait 没有用 Rust `Drop` 强制清理，因此漏调会把底层资源释放责任留给具体实现（`kv.rs:97-109, 667-674, 922-929`）。

coprocessor limiter 以 `Arc` 跨 worker 共享，内部计数与 store map 分别由 `Mutex` 保护。condvar 唤醒一个等待者；取消只能通过 10ms 周期检查观察，因此取消响应不是即时通知。调用者只有在 `AcquireWithContext` 返回 `false` 或 `TryAcquire` 返回 `true` 时才持 token，并必须执行一次 `Release`（`kv.rs:490-583`）。

SST limiter 要求实现 `Send + Sync`，同一 `Arc` 可跨 store 写 RPC 使用；`WaitN` 必须尊重传入 context 的取消。`ImportSSTWithOptions` 在等待前先检查一次取消，但等待期间和后续是否及时中止依赖 limiter 与具体存储实现（`kv.rs:760-840`）。

全局 `DecodeTableIDFunc` 是裸 `static mut`，没有锁或 once-cell 保护；安全使用前提是启动接线阶段写入、并发请求阶段只读且不再替换。若未来需要运行期替换，应先改成同步初始化原语并补独立并发测试。

## 与 Go 版本的对应关系

主要基线是同路径 `pkg/kv/kv.go`。基础读写接口、staging、事务、公平锁、Client、请求类型号、`StoreType`、`KeyRanges`、`Request`、Snapshot/Storage、region split、优先级、隔离级别和资源组标签的命名与语义总体逐项对齐；`pkg/kv/kv_test.go` 则是 limiter 和标签行为的直接测试对照。

需要明确的 Rust 差异/扩展如下：

- Go 的 `map[string]ValueEntry` 可用 string 无损承载任意字节，Rust 暂以 UTF-8 `String` 为键，所以增加 `KeyMapName` 的十六进制编码约定（`kv.rs:47-57`）；调用方与实现必须统一使用它。
- Go 空 slice/nil 的差异由 `Option` 表示：`BatchGetToGetOptions` 空输入返回 `None`；迭代上下界也使用 `Option<Key>`。Go 指针/接口 nil 则通常映射为 `Option<Arc<_>>`、`Option<Box<_>>` 或 `Option` 字段。
- Rust `Transaction` 增加 statement staging 默认方法、mem-buffer checkpoint、pipelined DML；`Storage` 增加 DDL/TTL/TiFlash/SST、异步 timestamp future、PD endpoints/keyspace codec 等移植期扩展。这些并非当前 Go `kv.go:865-910` 基础 `Storage` 接口的逐项成员，真实接线须以 Rust 调用者和 adapter 为准。
- Rust `ResultSubset` 增加 `ReadPoolTaskDetails` 与 `CopRuntimeEvidence` 默认方法，`Snapshot` 增加 `SnapCacheSize`，均以保守默认值保持旧实现可用。
- Go limiter 使用 channel/context 的等待语义；Rust 使用 `Mutex + Condvar`，并以 10ms polling 观察两个 `CancellationToken`。外部协议仍保持“返回 true 表示取消退出，false 表示已获得且需释放”。
- Go `ResourceGroupTagBuilder::Build` 防御 nil request；Rust 引用类型天然排除 nil。Go protobuf marshal 返回 nil bytes，Rust返回 `Option<Vec<u8>>`。两者都只在编码成功且非空时覆盖标签。
- Go `DecodeTableIDFunc` 是可空函数变量；Rust 用 `static mut Option<fn>` 保持回调形状，但增加了 `unsafe` 共享状态风险。

`pkg/kv/kv_test.rs` 与 Go 测试意图基本对齐：容量为 1 的等待/释放与取消、冗余 release panic、32 worker 不突破容量、按 store 隔离，以及空/短/长 digest 和 keyspace 的 protobuf 编解码。Rust 还显式处理 NextGen 测试进程缺少 Go `init()` 的全局 keyspace 初始化差异（`kv_test.rs:197-215`）。

## 扩展指南

- 新增基础读写能力时，先判断属于 `Getter`/`Retriever`/`Mutator`、`Snapshot`、`Transaction` 还是 `Storage`；不要把具体后端行为塞入通用 `Request`。修改 trait 会影响所有实现，应先用 RustCodeGraph/精确搜索列出 impl，再更新同目录独立测试，禁止把测试内嵌进 `kv.rs`。
- 新增 `Storage` 可选能力时，默认实现必须能明确区分“不适用”和真实失败。只有安全且兼容的 fallback 才可返回 `Ok(None)`/默认值；物理写入等高风险操作应像 `ImportSST` 一样默认报错。同步更新 `pkg/store/driver/kv_adapter.rs` 及直接消费模块的测试。
- 扩展 `Request` 字段时，要同步所有请求构建、clone/adapter 映射和 mock context，重点检查 `pkg/distsql`、`pkg/store/copr`、`pkg/store/driver` 与 `pkg/util/mock/context.rs`；共享计数器需要明确 query、statement、iterator 或 store 作用域。
- 调整 limiter 时必须保持 acquire 返回值的反直觉协议，覆盖容量边界、同/异 store、等待释放、两类取消、并发峰值和冗余释放。测试放在 `pkg/kv/kv_test.rs`，跨层接线测试位于 `pkg/distsql/*_test.rs`、`pkg/store/copr/*_test.rs` 和 `pkg/store/driver/coprocessor_adapter_test.rs`。
- 修改 `KeyRanges` 时要保持 ranges 与 hints 的下标关联以及“先分区、后 region”的发送顺序；至少覆盖空分区、多分区降格失败、已排序快速路径、分区间/分区内排序和 callback 首错短路。现有相关 Rust 覆盖还散见 `pkg/kv/assertion_1_aster_unit_test.rs`，新增单元测试仍应使用独立测试文件。
- 修改资源组标签时需同时检查 classic/nextgen keyspace、空 key、SQL/plan digest、table ID callback、label 与 protobuf 兼容性；对应文件是 `pkg/kv/kv_test.rs`、`pkg/kv/kv_test.go` 和 `pkg/executor/resource_tag_test.rs`。
- 若要消除 `DecodeTableIDFunc` 的并发风险，优先使用一次初始化或受控可替换容器，并验证 tablecodec 接线不造成 crate import cycle；不能直接在本 crate 引入上层 tablecodec。
- Rust 行为应继续对照 `pkg/kv/kv.go`，但 Rust 已有直接调用所必需的扩展不能为了表面一致而删除。任何行为修改都应同步独立 Rust 测试，并按仓库规则在实现修复后保留顶部 AsterSQL 与 PingCAP 版权注释。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/kv` 显示 `kv.rs` 有 255 个符号，`kv.go` 有 238 个符号。`node --file pkg/kv/kv.rs --offset 1 --limit 700` 与 `--offset 701 --limit 500` 完整读取了 1,035 行目标源码，并报告其被 35 个 Rust 文件使用。
- RustCodeGraph 精确符号查询确认 `KeyRanges`、`ResourceGroupTagBuilder`、`NewCoprRequestLimiter` 在 Go/Rust 同路径均存在。`callers/callees` 查询未返回可用边，因此调用关系改用精确仓库搜索并逐一记录路径；没有把失败查询当作“无调用者”。
- 已读源码/边界文件：`pkg/kv/kv.rs`、`pkg/kv/lib.rs`、`pkg/kv/Cargo.toml`。
- 已读 Go 对照：`pkg/kv/kv.go`；已读独立测试：`pkg/kv/kv_test.rs`、`pkg/kv/kv_test.go`。
- 已核对直接调用/实现证据：`pkg/session/runtime/{relational_scan.rs,import_sst.rs,ttl_metadata.rs,modify_column_cloud_planner.rs}`、`pkg/util/mock/context.rs`、`pkg/domain/domain.rs`、`pkg/store/driver/kv_adapter.rs`，以及搜索命中的 `pkg/distsql`、`pkg/store/copr` 与 `pkg/store/driver` 测试。
- 本任务只新增说明文档，按计划不运行 Cargo 或代码测试。交付前执行任务规定的 11 章节结构命令，并人工检查：仅说明当前可由源码/调用边验证的行为；未把默认方法或可选 trait 能力宣称为所有后端已实现；没有复制整段源码；测试建议均指向独立测试文件。
