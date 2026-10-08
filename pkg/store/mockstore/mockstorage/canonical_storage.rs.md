# `pkg/store/mockstore/mockstorage/canonical_storage.rs`

## 文件定位

本文件是 `astersql-store-mockstore-mockstorage` crate 的**规范 KV 接口适配层**：它不拥有底层数据，而是为 `storage.rs` 中的 `KVTxn`、`Transaction`、`Snapshot`、`MemManager` 和 `mockStorage` 实现 `astersql_kv`（本文记为 `kv`）定义的 traits。crate 入口 `lib.rs` 以 `mod canonical_storage` 装入本模块，并通过 `pub use canonical_storage::*` 与 `pub use storage::*` 将适配器和存储类型一起导出。

因此上层通过 `dyn kv::Storage`、`dyn kv::Transaction` 或 `dyn kv::Snapshot` 使用 mock 时，会进入本文件的 trait 方法；实际 MVCC 版本、时间戳、写集和关闭状态仍由 `storage.rs` 管理。典型构造入口是 `storage.rs::NewMockStorage`，会同时初始化 `OracleHandle`、`ClientHandle`、内存缓存和 `canonicalOptions`。仓库中的会话运行时和存储测试通过 `NewMockStorage(...)` 使用这条链，例如 `pkg/session/runtime/session.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs` 与 `pkg/store/store_test.rs`。

`Cargo.toml` 声明本 crate 的生产依赖为 `astersql-kv` 和 `astersql-store-mockstore-unistore`；coprocessor、driver/txn、helper 只位于永不成立的 `cfg(any())` 依赖段，不能据此认为 Rust 适配器直接使用这些 crate。

## 核心职责

1. 将内部错误、键和值转换为 `kv` 公共类型：`storage_error`、`transaction_error`、`not_found`、`value_entry` 和 `key_name` 统一适配错误身份、`ValueEntry` 与批量读 map 键。
2. 为事务和快照提供点查、正反向扫描与预物化迭代；事务读取遵循“本地写集覆盖 start-ts 快照”，删除标记遮蔽旧值。
3. 为 `KVTxn` 补齐 `Mutator`/`MemBuffer` 语义，包括空值拒绝、总写入大小限制、flags、statement staging、快照视图与批量读。
4. 为 `Transaction` 补齐提交/回滚、schema checker、async commit、公平锁标志、内存钩子、选项、表信息和乐观锁依赖等 `kv::Transaction` 契约。
5. 把 `mockStorage` 暴露为完整 `kv::Storage`，并桥接快照、oracle、client、MPP、SST 导入、选项、锁等待与 keyspace 元数据。
6. 明确 mock 的能力边界：默认 coprocessor client 返回不支持错误，MPP 无 store，`ShowStatus` 未实现，部分磁盘/检查点/flush 接口为空操作。

## 主要符号

- `CanonicalIterator`（公开）：持有预物化的 `(kv::Key, Vec<u8>)` 列表和当前位置，实现 `kv::Iterator`。`Next` 只允许从有效位置推进，`Close` 直接将位置移到末尾。
- `BufferedRetriever`（私有）：以 `BTreeMap` 保存物化视图，实现 `kv::Getter`/`kv::Retriever`，供 `KVTxn::SnapshotGetter` 返回不会随事务后续写入变化的只读视图。
- `impl kv::Getter/Retriever/Mutator/MemBuffer for KVTxn`：规范接口的核心 union-read 与写缓冲适配。`canonical_scan` 在嵌入式 RPC 后端上也会把 RPC 快照结果与本地 `writes` 合并。
- `impl kv::Getter/Retriever/Mutator/FairLockingController/Transaction for Transaction`：在 `KVTxn` 之上加入固定快照、选项、提交时间戳、schema checker、内存回调和表信息。
- `impl kv::Getter/Retriever/Snapshot for Snapshot`：固定版本点查/扫描，点查结果缓存于 `Snapshot::cache`；本地 MVCC 读保留真实 `CommitTs`，嵌入式 RPC 返回值的 `CommitTs` 为 0。
- `impl kv::MemManager for MemManager`：按 `(table_id, key)` 缓存 union-get 结果；未命中时从传入快照读取并回填，`Delete(table_id)` 清理整表缓存。
- `CanonicalClient`、`UnsupportedResponse`：默认 coprocessor 占位实现；`Send` 返回响应对象，第一次未关闭的 `Next` 报“不支持 coprocessor requests”，关闭后 `Next` 返回 `None`。
- `CanonicalMppClient`：报告 0 个 MPP store；构造任务返回空列表，派发和建连返回错误，可见性检查成功，取消为空操作。
- `CanonicalOracle` 与公开的 `OracleHandle`：前者把 `KVStore::CurrentTimestamp` 包为 ready future；后者通过 `Arc<RwLock<Arc<dyn Oracle>>>` 支持逐 store 替换 oracle，且本身继续实现 `kv::oracle::Oracle`。
- 公开的 `ClientHandle`：通过 `Arc<RwLock<Option<Arc<dyn kv::Client + Send + Sync>>>>` 支持注入 RPC client；未注入时回落到静态 `CANONICAL_CLIENT`。
- `impl kv::Storage for mockStorage`：顶层适配入口，覆盖 `ImportSST`、`Begin`、`GetSnapshot`、client/oracle/MPP、生命周期、选项和元数据查询。
- `option_key` 与 `leak_option_value`：将有限的动态键值类型转成内部 `OptionKey` 和 `'static` 引用；支持的 key 为 `String`、`&'static str`、`i64`、`u64`、`bool`，value 另支持 `Vec<u8>`、`usize`。

## 执行流程

**构造与开始事务。** `NewMockStorage`（`storage.rs`）创建 `mockStorage` 并安装 `OracleHandle`、`ClientHandle`。调用 `kv::Storage::Begin` 时，本文件只把 `kv::tikv::TxnOption::{Default, StartTS}` 转成内部选项，再委托 `mockStorage::Begin`；内部 `KVStore::Begin` 检查关闭/失败注入，使用显式 start-ts 或 oracle 分配时间戳，最后由 `newTiKVTxn` 组合 `KVTxn` 与 start-ts 快照。

**读写与扫描。** `Transaction::Get` 先查 `inner.writes`：`Some(value)` 是未提交写，`None` 是删除并返回 not-found；未命中才读固定 `snapshot`，并把对外 `CommitTs` 清零以保持 Go transaction getter 契约。`KVTxn::canonical_scan`/`scan` 先取得 start-ts 下的有序快照，再以本地写覆盖、以删除标记移除，最后按需反转。`CanonicalIterator` 顺序暴露预物化结果，所以创建迭代器后的底层变化不会改变该迭代器。

`kv::Mutator::Set/Delete` 在真正写入内部 map 前重新计算替换该 key 后的总缓冲大小，并与全局原子量 `kv::TxnTotalSizeLimit` 比较；空 value 的 `Set` 返回 `ErrCannotSetNilValue`。`Transaction` 写成功后调用可选 `memory_hook`，参数是当前缓冲字节数。

**statement staging。** `Staging` 克隆当前 `writes` 和 `flags` 并生成递增句柄；`Release` 仅删除对应快照记录；`Cleanup` 恢复该层基线、重算 `buffered_write_size`，并截断该层及之后的 staging；`InspectStage` 只回调相对基线新增或变化的当前键。删除在回调中表示为空 `Vec<u8>`，调用者须结合 flags/契约理解，而不能把它当普通空值写入。

**提交与回滚。** `Transaction::Commit` 有两条后端路径：嵌入式 RPC 路径先验证事务有效，分配 commit-ts，对非空写集运行可选 schema checker，再调用 RPC commit 并清空/失效事务；本地路径读取 `EnableAsyncCommit` 和 `SchemaChecker`，分别委托 `KVTxn::CommitAsync`、`CommitWithSchemaChecker` 或 `Commit`。底层写冲突由 `transaction_error` 转成可识别的 `ErrTxnRetryable`。schema checker 拒绝时，本地实现会保留写集和事务有效性。`Rollback` 在 RPC 后端先发送写集 key，再委托内部回滚清空状态并失效。

`LockKeys` 用“写回当前值”或“写入删除标记”模拟乐观 lock-only mutation，使目标 key 进入底层原子 MVCC 冲突检查而不改变可见值；`LockKeysFunc` 先执行回调再加锁。公平锁方法仅维护 `fair_locking` 布尔标志，并未模拟真实排队。

**快照、缓存和外设。** `Snapshot::Get` 先查包含 negative cache 的 `RefCell<HashMap<...>>`，再读 RPC 或本地 MVCC 并回填；扫描不使用这个点查缓存。`MemManager::UnionGet` 以 table-id 隔离缓存。`ImportSST` 统计 key/byte 后委托 `ingest_sst`，以调用者指定 commit-ts 插入有序历史版本。client、MPP 和 oracle 则通过前述句柄或占位实现暴露给上层。

## 数据与状态

- `CanonicalIterator.entries/position`：一次扫描的所有权快照及游标；无借用底层 store，关闭后永久无效。
- `KVTxn.writes`：`BTreeMap<Vec<u8>, Option<Vec<u8>>>`，`Some` 表示 put，`None` 表示 tombstone；有序性支撑确定性扫描合并。`buffered_write_size` 计入 key 和存在的 value。
- `KVTxn.flags/stages/next_stage`：键标志与 staging 快照栈。staging 是完整克隆，语义清晰但成本随写集大小增长。
- `Transaction.options`：以整数 option id 存放 `Box<dyn Any>`；`Pessimistic` 同时更新底层冲突检查模式，`Priority` 同时捕获嵌入式 RPC request marker 并传播给快照。
- `Transaction.commit_ts/vars/table_info/checkpoint/memory_hook/fair_locking`：分别保存提交结果、任意会话变量、表元数据缓存、MemDB 检查点占位、写缓冲变化回调和公平锁模式。
- `Snapshot.version/options/cache/request_marker`：版本在 `KVStore::GetSnapshot` 构造时会把 `u64::MAX` 或未来版本钳制到当前时间戳；cache 同时缓存存在值和不存在结果。
- `OracleHandle`、`ClientHandle`：各 store 独立的可替换委托，clone 后共享同一锁和当前目标。
- `mockStorage.canonicalOptions`：把支持类型的 value 泄漏为进程生命周期的引用后保存。替换或删除 map 项不会回收已泄漏对象，这是刻意的 Go 风格生命周期简化，也是反复设置 option 时的内存风险。

## 依赖与调用关系

上游主链可概括为：`NewMockStorage`（`storage.rs`）→ `mockStorage` → 本文件的 `impl kv::Storage` → `Begin`/`GetSnapshot` → 本文件的 `Transaction`/`Snapshot` trait 实现 → `KVTxn`/`KVStore` 的本地 MVCC，或可选的 `EmbeddedRpcStore`。会话代码通常持有 trait object，因此 RustCodeGraph 没有为这些动态 trait 调用建立精确 `callers` 边；仓库文本使用点确认 `pkg/session/runtime/session.rs` 会构造此 mock，多个 session/store 测试也直接构造它。

直接下游依赖如下：

- `astersql_kv`：所有公共 traits、`Key`、`ValueEntry`、错误类别、transaction options、MPP 类型和全局事务大小限制。
- `crate::storage`：`KVStore`、`KVTxn`、`Transaction`、`Snapshot`、`MemManager`、`mockStorage`、`OptionKey` 以及 MVCC/事务实际实现。
- `crate::EmbeddedRpcStore`（经 `KVStore::EmbeddedRpc`）：可选的 RPC 点查、扫描、提交和回滚路径；其依赖由 crate 的 `astersql-store-mockstore-unistore` 提供。
- 标准库 `BTreeMap`/`HashMap`、`Any`、原子 ordering、`Arc<RwLock<...>>` 与 `Duration`：分别承担有序合并、动态选项、并发委托和 MPP 接口参数。

RustCodeGraph 的文件关系显示 `canonical_storage.rs` 直接使用 `storage.rs`，并被 `pkg/session/runtime/scan_adapter_runtime_test.rs` 关联；`query CanonicalIterator --kind struct` 精确定位到本文件第 60 行。对 `CanonicalIterator`、`OracleHandle`、`ClientHandle`、`mockStorage` 执行的 `callers/callees` 没有输出，不能把“无静态边”解释成“运行时无人调用”。

## 错误处理与边界

- `transaction_error` 特判 `MockStorageError::WriteConflict` 为 `kv::ErrTxnRetryable`；其他内部错误只保留其字符串。上层依赖错误身份判断重试，因此新增冲突类别时必须同步此映射。
- 不存在键统一用 `ErrNotExist`。`Transaction::BatchGet` 和 `Snapshot::BatchGet` 只跳过明确的 not-found，传播其他错误；`KVTxn::MemBuffer::BatchGet` 当前却用 `if let Ok` 忽略所有逐键错误，这是一个重要的接口差异。
- 无效迭代器调用 `Next` 报错，但无效位置的 `Key`/`Value` 返回默认空值；调用者必须先检查 `Valid`。
- `Set` 禁止空 value；`Delete` 使用 tombstone。两者都受 `TxnTotalSizeLimit` 约束，替换旧写时会先扣除旧大小。
- `ShowStatus` 固定返回 `ErrNotImplemented`；默认 coprocessor response 报不支持；MPP 派发/建连报“no MPP stores”。这些是显式能力边界，不是可工作的远程执行实现。
- `SetDiskFullOpt`、`ClearDiskFullOpt`、`RollbackMemDBToCheckpoint`、`CancelMPPTasks` 是空操作，`MayFlush` 与 MPP 可见性检查总是成功，checkpoint 是占位值。
- `option_key`/`leak_option_value` 对未知动态类型静默忽略；读取未知 key 返回 `None`。锁中毒通过 `expect(...)` panic，而不是返回业务错误。
- `ImportSST` 的进一步约束来自 `storage.rs::ingest_sst`：store 必须未关闭、commit-ts 非 0 且不超过 `i64::MAX`、输入 key 必须严格递增；本文件只转换错误并返回统计。

## 并发与资源生命周期

底层 `KVStore` 可克隆且通过 `Arc<KVStoreInner>` 共享 MVCC 数据、时间戳和关闭状态；数据及提交发布使用锁与原子量协调。`OracleHandle`、`ClientHandle`、`MemManager` 和 storage options 分别用 `RwLock` 保护共享可变状态。`ClientHandle::Send`/能力查询会先在读锁内 clone 当前 `Arc`，随后在锁外调用被注入对象，避免把外部调用置于锁保护期。

`KVTxn`/`Transaction` 是可变事务对象：提交或回滚后 `valid=false`；提交消费/清空写集，回滚清空写集、flags 和 stages。`Snapshot` 的点查 cache 使用 `RefCell`，说明其内部可变性面向单线程借用模型，而不是并发共享缓存。`CanonicalIterator` 完全拥有结果，既不持锁也不持 store 引用。

`Close` 委托 `mockStorage::Close`/`KVStore::Close`，底层关闭是幂等的，并在首次关闭时关闭嵌入式 RPC；关闭后的新事务和时间戳申请失败。`UnsupportedResponse::Close` 仅翻转本对象状态。`SetOption` 的 `'static` 泄漏不会随 store 关闭释放，扩展可接受类型前必须评估长进程测试中的累积内存。

测试中的 `TxnTotalSizeLimit` 是进程级原子量，`canonical_storage_test.rs::transaction_test_guard` 用全局 mutex 串行化相关事务测试并以 RAII guard 恢复旧值；新增修改该全局值的测试也必须沿用此隔离方式。

## 与 Go 版本的对应关系

同路径 `storage.go` 的 `mockStorage` 直接组合 `*tikv.KVStore` 与 `*copr.Store`，通过 `driver.NewTiKVTxn`/`driver.NewSnapshot` 获得 `kv.Transaction` 和 `kv.Snapshot`。Rust 将可独立运行的内存 MVCC 放在 `storage.rs`，再由本文件显式为内部对象实现同类 `kv` traits；这是结构差异，不应把 Rust 的默认 coprocessor/MPP 占位误认为 Go `copr.Store` 的完整等价实现。

已对齐的顶层行为包括：`Name` 为 `mock-storage`、`Describe` 为空、`ShowStatus` 返回未实现、`Begin` 支持事务选项、`GetSnapshot` 返回指定版本视图、`CurrentVersion` 经 oracle 获取版本、`GetMinSafeTS`、锁等待、cluster ID、keyspace、mem cache 和可设置选项。Rust 测试还固定了 Go 语义：事务 Getter 的 `CommitTs` 为 0、写冲突可由 `IsTxnRetryableError` 识别、显式 start-ts 被保留、回滚写不可见。

主要差异/限制：

- Go 的 `GetCodec` 根据 keyspace metadata 和 PD client 选择 codec；Rust 当前固定返回 `kv::tikv::Codec` 占位。
- Go `SetOption(k, nil)` 删除选项并由 `sync.Map` 管理生命周期；Rust 规范 trait 接受非可选 value，且只支持有限 `Any` 类型并永久泄漏 value。`storage.rs` 另有非规范、可删除且拥有值的 options map，不要混淆两套 API。
- Go `Close` 先关 `copr.Store` 再关 KVStore；Rust 规范实现委托 `mockStorage::Close`，实际资源细节由 `storage.rs` 决定。
- Rust 额外提供本地/embedded-RPC 双路径、可注入 client/oracle、物理 SST 导入和明确的 MPP/coprocessor 占位，以支撑当前 Rust 会话测试；这些不能由同路径 Go 文件直接证明等价。

## 扩展指南

- 新增 `kv::Storage` 能力时，从本文件的 `impl kv::Storage for mockStorage` 接线；若涉及真实状态或 MVCC 算法，应把所有权放在 `storage.rs`，本文件只做类型/错误/trait 适配。同步更新独立的 `canonical_storage_test.rs`，不要把测试写回生产文件。
- 修改读写或扫描时，同时检查 `KVTxn`、`Transaction`、`Snapshot` 三层：点查与扫描必须保持相同的 `[lower, upper)` 边界、正反序、read-your-writes、tombstone 遮蔽和 start-ts 可见性。embedded RPC 与本地路径都要覆盖。
- 修改提交时，必须保留 schema checker 拒绝后的事务有效性与私有写、async commit 时间戳规则、悲观/乐观冲突差异，以及 `WriteConflict → ErrTxnRetryable` 的错误身份。
- 扩展 staging/flags 时，注意 `Cleanup` 的嵌套截断、`buffered_write_size` 重算与 `InspectStage` 的删除表达；必要时在 `canonical_storage_test.rs` 增加独立回归测试。
- 注入新的 client/oracle 行为应通过 `ClientHandle`/`OracleHandle`，保持逐 store 隔离；不要改用全局可变对象。若需要并发共享 `Snapshot`，应先重构 `RefCell` cache 并验证 trait 的线程约束。
- 为 options 增加动态类型时，必须同时修改 `option_key` 或 `leak_option_value`，并明确所有权与释放策略；当前泄漏方案不适合高频、无界设置。
- 若实现 coprocessor、MPP、disk-full、checkpoint 或 flush，替换对应占位符号并加入能力/错误路径测试；不能只把 `IsRequestTypeSupported` 改为 true 而不提供实际响应。
- 兼容风险集中在错误身份、`CommitTs` 暴露、key 编码、范围边界和 Go option 语义；性能风险集中在扫描预物化、staging 全量克隆、逐 key `BatchGet` 与 option 泄漏。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录的 6 个 Rust/Go 源文件均已索引，`canonical_storage.rs` 报告 151 个符号。
- RustCodeGraph 源码读取：`canonical_storage.rs` 全部 1,390 行；直接实现载体 `storage.rs` 的 `KVStore`、`KVTxn`、`Transaction`、`Snapshot`、`MemManager`、`mockStorage` 和 `NewMockStorage`；模块入口 `lib.rs`。
- RustCodeGraph 查询：`query CanonicalIterator --kind struct` 定位 `canonical_storage.rs:60`；`query mockStorage/KVTxn/NewMockStorage` 定位 `storage.rs` 的类型和构造器。对主要适配类型执行 `callers/callees` 未得到静态边，动态 trait 调用关系改由模块装配、构造点和仓库使用点交叉验证。
- crate 边界：`pkg/store/mockstore/mockstorage/Cargo.toml`；确认生产依赖和 `package.metadata.porting.go-package = "pkg/store/mockstore/mockstorage"`。
- Go 对照：`pkg/store/mockstore/mockstorage/storage.go` 全部 203 行，核对构造、事务/快照、版本、选项、关闭、codec、锁等待、cluster ID 和 keyspace。
- 独立 Rust 测试：`pkg/store/mockstore/mockstorage/canonical_storage_test.rs` 全部 419 行；覆盖 trait 合规、提交/回滚/关闭、大小限制、显式 start-ts、零 `CommitTs`、可重试冲突、async commit、schema checker、lock-only 冲突依赖、embedded RPC union scan/错误传播及 SST 历史顺序。
- 上游使用点文本核验：`pkg/session/runtime/session.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs`、`pkg/store/store_test.rs` 等构造并通过标准 KV 接口使用 `NewMockStorage`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工检查没有把占位能力写成已实现能力。
