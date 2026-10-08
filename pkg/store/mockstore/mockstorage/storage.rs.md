# `pkg/store/mockstore/mockstorage/storage.rs`

## 文件定位

本文件是 `astersql-store-mockstore-mockstorage` crate 的内存 KV 核心与 Go `mockstorage` 门面的 Rust 实现。crate 入口 `pkg/store/mockstore/mockstorage/lib.rs` 将本文件与 `canonical_storage.rs`、`embedded_rpc.rs` 组装并重新导出；`Cargo.toml` 声明它直接依赖 `astersql-kv` 与嵌入式 UniStore crate。它位于测试用 SQL/会话层和底层 KV 行为之间：本文件维护 MVCC 数据、事务写缓冲、快照、时间戳、PD/keyspace 替身和生命周期，`canonical_storage.rs` 再把这些具体类型适配为标准 `astersql_kv::Storage`、`Transaction`、`Snapshot` 等 trait。

RustCodeGraph 对该文件的文件级反向引用包括 `pkg/store/mockstore/mockstorage/canonical_storage.rs`、`pkg/store/driver/tikv_driver.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs` 和 `pkg/session/tests/paging_rpc.rs` 等。应用侧的具体入口之一是 `CreateAnalyzeSession`：它以 `KVStore::NewMemoryWithWallClockTSO` 和 `NewMockStorage` 构造真实事务语义的测试 Domain（`pkg/session/runtime/session.rs`）。本目录没有 `doc.go`；包语义以 crate 入口注释、同路径 Go 文件和上述适配层为准。

## 核心职责

1. `KVStore`/`KVStoreInner` 用 `BTreeMap<Vec<u8>, Vec<VersionedValue>>` 保存每个键按 `commit_ts` 排列的版本，并提供点读、范围扫描、事务提交和固定时间戳 SST 导入。
2. `KVTxn` 在事务私有 `writes` 中缓存写入或删除，保证 read-your-writes，并在提交时执行乐观写冲突检查；悲观事务可通过 `SetPessimistic` 跳过这项旧快照冲突检查。
3. `Snapshot` 固定版本读取；`GetSnapshot` 会把 `u64::MAX` 或未来版本钳制到创建快照时的当前时间戳，之后的提交不会改变该快照的可见性。
4. `mockStorage` 聚合 `KVStore`、`CoprStore`、内存缓存、选项、锁等待数据和 keyspace 元数据，暴露与 Go `kv.Storage` 相似的门面；标准 trait 的完整接线位于 `canonical_storage.rs`。
5. `MemoryPdClient`、`CodecPDClient`、`pdCliWithCodec`、`CoprStore` 及多种占位客户端提供可控的 PD/keyspace、协处理器和错误注入行为，避免测试依赖真实集群。
6. `NewEmbeddedRpc` 可把嵌入式 TiKV RPC 存储设为权威 MVCC 后端；本地事务缓冲仍复用这里的 `KVTxn` 数据结构，真正的 RPC 提交/回滚分支由 `canonical_storage.rs` 驱动。

## 主要符号

- `MockStorageError`：统一错误枚举，区分未实现、已关闭、写冲突、Begin、协处理器、PD 和嵌入式 RPC 错误。`canonical_storage::transaction_error` 会把 `WriteConflict` 转成规范的可重试事务错误，其余错误保留文本。
- `MemoryPdClient` 与 `PdClient::LoadKeyspace`：线程安全的 keyspace 表；`SetReturnNone` 可模拟 mock PD 不返回元数据，`FailNextLoad` 是一次性失败注入。
- `Codec`、`NewCodecPDClient`、`NewCodecPDClientWithKeyspace`：分别构造 ApiV1 或带名称/ID 的 ApiV2 编解码描述。`pdCliWithCodec` 在底层 mock PD 返回 `None` 时强制提供构造时保存的元数据。
- `KVStore`：可克隆的共享存储句柄。`NewMemory` 使用逻辑递增 TSO；`NewMemoryWithWallClockTSO` 生成保留 TiDB TSO 物理时间布局且严格单调的时间戳；`NewEmbeddedRpc` 附加 `EmbeddedRpcStore`。`Begin`、`GetSnapshot`、`CurrentTimestamp`、`Close` 是主要生命周期入口。
- `KVTxn`：保存 `start_ts`、事务选项、写集、缓冲字节数、staging、标志、悲观模式及请求元数据。`Get`/`Set`/`Delete` 操作私有视图，`Commit`、`CommitAsync`、`CommitWithSchemaChecker` 发布版本，`Rollback` 丢弃私有状态。
- `Transaction`：面向上层的包装，除 `KVTxn` 和 `Snapshot` 外还保存提交时间戳、规范选项、表信息、checkpoint、内存钩子和公平锁标志；本文件只实现基础委托，标准接口行为在 `canonical_storage.rs` 扩展。
- `Snapshot`：固定 `version` 的只读视图；缓存和请求标记由规范适配层使用。
- `MemManager`：以 `(table_id, key)` 为键的并发缓存；公开方法使用默认表 ID 0，crate 内方法支持分表读写及整表删除。
- `mockStorage`/`MockStorage`：最终门面及公开别名。`NewMockStorage` 先创建 `CoprStore`，成功后初始化 oracle、client、选项、缓存、锁等待和 keyspace 状态。
- `MockLockWaitSetter`：允许测试注入死锁等待条目，而不要求调用者依赖具体类型的方法集合。

## 执行流程

典型内存事务流程如下：

1. 调用者以 `KVStore::NewMemory` 或 `NewMemoryWithWallClockTSO` 建立共享内核，再调用 `NewMockStorage`。构造器首先执行 `CoprStore::NewStore`；若 `FailNextCoprocessorStore` 已注入错误，则不返回半初始化门面。
2. `mockStorage::Begin` 委托 `KVStore::Begin`，后者先检查关闭和一次性 Begin 失败，再优先采用 `TxnOption::StartTS`，否则调用 `CurrentTimestamp` 分配开始时间戳；`newTiKVTxn` 同时创建该 `start_ts` 的快照。
3. `KVTxn::Get` 先查私有写集；存在 `Some(value)` 时返回新值，存在 `None` 时表现为删除，否则调用 `read_at(start_ts)`。范围扫描先物化 `scan_at(start_ts)`，再用私有写入覆盖并移除私有删除，因此同一事务的点读和迭代视图一致。
4. 普通 `Commit` 获取全局数据写锁，在同一临界区检查任一待写键的最新 `commit_ts > start_ts`，随后分配 `commit_ts` 并追加版本。这样冲突检查、时间戳分配与版本发布不会暴露中间状态。
5. 异步提交使用 prewrite 期间已经推进的 `current_ts` 与 `start_ts + 1` 的较大值；带 schema checker 的提交先确定将发布的时间戳，仅在写集非空时调用 checker，checker 成功后才取走写集并发布。因此 checker 拒绝时事务仍有效且私有写保留。
6. `Rollback` 清空写集、大小、flags 和 stages，并令事务失效；对已失效事务重复回滚是成功的空操作。
7. `GetSnapshot` 捕获创建当刻的有效版本。`read_entry_at` 从版本数组尾部寻找不晚于快照版本的最新条目，删除墓碑使结果为空；`scan_at` 对范围内每个键执行相同规则。
8. `mockStorage::Close` 若底层已关闭直接成功，否则依次幂等关闭 `CoprStore` 和 `KVStore`；若附加嵌入式 RPC，`KVStore::Close` 还传播 RPC 关闭错误。

固定时间戳导入走 `KVStore::ingest_sst`：先拒绝关闭状态、零/超出 `i64::MAX` 的时间戳以及非严格递增键序列，再按 `commit_ts` 二分插入或替换版本，最后将 `current_ts` 至少推进到导入时间戳。这与普通事务只能在尾部发布新版本不同，允许重试时把旧 SST 版本插入已有新版本之前。

## 数据与状态

- MVCC 主数据受 `KVStoreInner::data: RwLock<...>` 保护；每个键的 `VersionedValue` 以提交时间戳升序保存，`value: None` 是删除墓碑。普通提交追加版本，SST 导入用二分维持顺序。
- `current_ts` 是 oracle 上界，`tso_request_count` 只统计成功到达分配器的请求。逻辑时钟每次加一；墙钟模式使用“Unix 毫秒左移 18 位”并取 `current + 1` 与墙钟值的最大值。
- `closed` 与 `close_count` 分别记录关闭状态及首次关闭次数；`Close` 使用原子交换保证幂等。`uuid` 由进程级 `NEXT_STORE_ID` 生成，仅保证本进程构造实例间唯一。
- `begin_failure`、`copr_failure`、`MemoryPdClient::failure` 都是 `Mutex<Option<String>>` 保护的一次性注入，消费后自动清空；`return_none` 是持续生效的原子开关。
- `KVTxn` 自身不是共享并发对象；写集、stages 和有效性由持有者独占修改。`Snapshot` 内部缓存是 `RefCell`，因此也不是跨线程共享缓存设计，但其 `KVStore` 句柄读取共享 MVCC 状态。
- `mockStorage` 的选项、规范选项、缓存和锁等待分别有独立锁。`AnyValue` 使用 `Arc<dyn Any + Send + Sync>`；规范 trait 选项的静态引用存储策略位于 `canonical_storage.rs`，不能与这里的 `opts` 混为同一张表。

## 依赖与调用关系

上游构造者通过 crate 根重新导出的 `KVStore` 与 `NewMockStorage` 使用本文件。`pkg/session/runtime/session.rs::CreateAnalyzeSession` 采用墙钟 TSO 版本；`pkg/session/tests/paging_rpc.rs`、多个 session runtime 测试和 `pkg/store/store_test.rs` 也直接构造该存储。RustCodeGraph 还识别到 `pkg/store/driver/tikv_driver.rs` 与 `canonical_storage.rs` 的文件级依赖。

下游关系主要有三组：

- `astersql-kv`：提供规范 Storage/Transaction 相关类型、优先级、表信息和 checkpoint；本文件保存其状态，`canonical_storage.rs` 实现 trait 并把本文件错误映射为公共错误。
- `canonical_storage.rs`：调用 `KVTxn::scan`、`Commit`、`CommitAsync`、`CommitWithSchemaChecker`、`KVStore::ingest_sst`、`mockStorage` 门面方法和 `MemManager` 分表方法。它还负责事务总大小限制、迭代器、schema checker、规范 option 和嵌入式 RPC 分支。
- `embedded_rpc.rs` 与 `astersql-store-mockstore-unistore`：`NewEmbeddedRpc` 创建嵌入式 RPC 后端；实际 RPC 的读、提交、回滚和错误传播由相邻实现与规范适配层完成。

`Cargo.toml` 中 `astersql-store-copr`、`astersql-store-driver-txn`、`astersql-store-helper` 仅位于永不成立的 `cfg(any())` 依赖段，说明 Rust 当前实现没有直接链接这些 Go 对应组件；不能据此声称这些生产依赖已接入本 crate。

## 错误处理与边界

- 关闭后 `Begin`、`CurrentTimestamp`、`ingest_sst` 返回 `Closed`；已创建快照的读取没有单独检查关闭标志。`KVTxn::Commit`/`CommitAsync` 对失效事务也以 `Closed` 表示。
- 乐观提交仅检查写集中的键，并以最新版本时间戳严格大于 `start_ts` 为冲突；读集本身不参与检查。悲观标记会关闭这项本地检查，因为其正确性假定上层已经完成锁排序。
- `CommitWithSchemaChecker` 在 checker 返回错误时不取走写集、不重置大小、不令事务失效；但非异步路径已经推进 oracle，这个未使用时间戳允许形成空洞。
- `GetCodec` 的 ApiV2 路径对 PD 加载错误和 codec 构造错误直接 panic，与同路径 Go 实现一致；若 mock PD 返回 `None`，则用 `pdCliWithCodec` 回退到构造时元数据。ApiV1 路径不查询 keyspace。
- `ShowStatus` 明确返回 `NotImplemented`；etcd/PD 地址为空、TLS 为 `None`、GC worker 是空操作、最小 SafeTS 为 0，`KvClient`/`MppClient`/`Oracle` 也是占位类型。这些是 mock 边界，不代表真实 TiKV 能力。
- 锁中毒使用 `expect`，因此发生 panic 后后续访问会继续 panic；本实现没有尝试恢复 poisoned lock。
- SST 导入要求键严格升序和有效 PD 时间戳，同一键同一时间戳会替换值。普通事务提交不显式拒绝空写集，仍可分配或复用提交时间戳。

## 并发与资源生命周期

`KVStore` 的所有克隆共享同一个 `Arc<KVStoreInner>`。数据读使用读锁，普通提交和 SST 导入使用写锁；`CurrentTimestamp` 在分配时间戳前持有数据读锁，确保“在某事务开始时间戳之前已分配提交时间戳的提交”完成版本发布后，新事务才取得快照，从而避免固定快照在两次读取之间发生变化。

原子字段按用途使用 Acquire/Release 或 AcqRel：关闭和时钟需要跨线程可见性，纯计数读取可用 Relaxed。PD 表、选项、缓存和锁等待使用分离的 `RwLock`，失败注入用短生命周期 `Mutex`，没有跨这些锁的嵌套持有。调用方应避免在持有外部锁时触发可能 panic 的 codec/PD 路径。

事务资源以 `valid` 为终止标志：成功提交或回滚后失效；schema checker 拒绝不会终止；嵌入式 RPC 提交/回滚由适配层在改变本地状态前后协调。存储关闭幂等，首次关闭计数一次；`mockStorage::Close` 先关闭协处理器再关闭底层存储。`MemManager` 与快照缓存随各自 `Arc`/拥有者释放，没有后台 worker；`StartGCWorker` 不创建任务。

## 与 Go 版本的对应关系

同路径 `storage.go` 的核心结构是包装 `*tikv.KVStore` 与 `*copr.Store`，再补充 option、memCache、锁等待和 keyspace 元数据。Rust 的 `mockStorage` 保留了相同门面方法和命名：`NewMockStorage`、`Begin`、`GetSnapshot`、`CurrentVersion`、`Close`、`GetCodec`、`SetMockLockWaits`、`GetClusterID`、`GetKeyspace` 等；ApiV2 codec 在 mock PD 返回空元数据时使用包装客户端的回退逻辑也逐分支对应。

Rust 文件比 Go 门面承担更多职责：它在本 crate 内实现了可运行的内存 `KVStore`/MVCC、事务、快照、墙钟 TSO、写冲突、异步提交、schema checker 所需原语、SST 导入和可选嵌入式 RPC。Go 版本把这些行为委托给 `tikv/client-go` 和 driver 包。因此，Rust 中 `KVTxn` 的细节不是对 `storage.go` 某个同名类型的逐行翻译，而是为替代外部客户端所需的局部实现。

仍属简化或占位的差异包括：Rust 的地址返回空 `Vec`（Go 返回 nil slice，接口语义同为空）、TLS/GC/SafeTS/客户端占位；Rust `WaitForEntry` 只保留三个字段；Rust 的公开 `GetCodec` 返回本地 `Codec`，而规范 `kv::Storage::GetCodec` 当前在 `canonical_storage.rs` 返回规范占位 codec。评估兼容性时应同时检查两个层次，不能只看门面同名方法。

## 扩展指南

- 修改 MVCC 可见性或冲突规则时，优先落在 `read_entry_at`、`scan_at`、`commit_at_with_conflict_check` 和 `commit_with_allocated_timestamp`；必须保持点读与扫描规则一致、每键版本有序，并同步 `canonical_storage_test.rs` 的快照、陈旧写者和乐观锁测试。
- 新增事务选项时，需要同时扩展 `TxnOption`、`KVStore::Begin` 的解析，以及 `canonical_storage.rs` 从 `kv::tikv::TxnOption` 的映射；仅保存到 `opts` 而不接线不会产生行为。
- 修改提交路径时必须同时覆盖普通提交、异步提交、schema checker 和嵌入式 RPC 四条分支，并验证 checker 拒绝保留私有写、成功检查的时间戳就是最终发布时间戳。
- 修改 SST 导入时应保持固定导入时间戳、乱序历史版本插入和重试幂等语义；对应独立测试是 `physical_sst_import_preserves_timestamp_and_historical_mvcc_order`，上层入口是 `kv::Storage::ImportSST` 的适配实现。
- 扩展 keyspace/PD 行为时，需同步 `MemoryPdClient`、`pdCliWithCodec`、`mockStorage::GetCodec` 和同路径 Go 分支，并分别测试 PD 错误、未找到和成功三种结果。若把当前 panic 改为可恢复错误，会改变 Go 对齐语义，应显式评审。
- 增加并发共享状态时应使用独立同步原语并明确锁顺序，避免把外部回调或 RPC 放在数据写锁内。新增 Rust 单元测试应继续放在独立的 `canonical_storage_test.rs` 或相邻独立测试文件，不嵌入生产源文件。
- 若实现当前占位方法，应先核对标准 `astersql_kv::Storage` trait 与 Go `helper.Storage` 契约，并同时调整 `canonical_storage.rs`；不要仅让本文件同名方法返回真实值而留下规范接口仍返回占位值。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/store/mockstore/mockstorage` 确认本 crate 的 `storage.rs`、`canonical_storage.rs`、`embedded_rpc.rs`、`lib.rs` 和独立测试；`node --file .../storage.rs` 分段读取了全部 1,261 行，并给出 6 个文件级使用方。对 `NewMockStorage`、`Begin`、`Commit`、`GetCodec` 执行了限定文件的 `callers/callees` 查询，但当前索引未返回符号级边，因此调用关系又由已定位的直接源文件核验。
- 源与边界：`pkg/store/mockstore/mockstorage/storage.rs`、`lib.rs`、`Cargo.toml`、`canonical_storage.rs`、`embedded_rpc.rs`；目标目录不存在 `doc.go`。
- Go 对照：`pkg/store/mockstore/mockstorage/storage.go`，核对构造、门面方法、关闭次序、codec 回退及锁等待注入。
- 独立 Rust 测试：`pkg/store/mockstore/mockstorage/canonical_storage_test.rs` 验证标准 trait、提交/回滚/关闭、显式 start_ts、总大小限制、写冲突、异步提交、schema checker、嵌入式 RPC、SST 历史顺序；`pkg/store/store_test.rs` 验证通过公共 Storage 接口的读写、删除和迭代语义。上层使用证据还包括 `pkg/session/runtime/session.rs` 和 `pkg/session/tests/paging_rpc.rs`。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收使用任务指定命令，要求目标文档存在且恰有 11 个固定二级标题；最终退出码记录在交付结果中。
