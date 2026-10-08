# `pkg/store/driver/txn/txn_driver.rs`

## 文件定位

该文件属于 `astersql-store-driver-txn` crate；crate 入口 `pkg/store/driver/txn/lib.rs` 以 `mod txn_driver` 装配它并通过 `pub use txn_driver::*` 导出其公开符号。`pkg/store/driver/txn/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/store/driver/txn`，直接依赖 canonical `astersql-kv`、表键编解码 crate `astersql-tablecodec`，以及带固定 tag `v0.4.2-aster.10` 的 `tikv-client`。

文件同时承担两种边界职责。第一种是生产 `pkg/store/driver/kv_adapter.rs` 实际使用的 client-rust 公共辅助层：`ClientTransactionMode`、`map_client_error` 和 `canonical_value`。第二种是 `tikvTxn`，它用共享 `BTreeMap`、`tikvSnapshot` 和 `memBuffer` 表达 TiDB 事务适配语义；当前仓库中它的直接构造调用集中在独立测试 `pkg/store/driver/txn_test.rs`，而真实 TiKV 网络事务由 `kv_adapter.rs` 的 `ClientTransaction` 持有 `tikv_client::Transaction` 执行。因而不能把这里的 `tikvTxn::commit_inner` 描述成真实 TiKV 2PC 网络实现。

## 核心职责

- `ClientTransactionMode::{Optimistic,Pessimistic}` 把上层事务模式转成 client-rust 的 `TransactionOptions`，并把 drop check 设为 `CheckLevel::None`；`as_str` 提供稳定模式名。
- `map_client_error` 把 client-rust 错误压到 canonical `pkg/kv` 错误边界。它递归检查 `KeyError`、多错误集合和悲观锁包装中的 `WriteConflict`；找到冲突时保留三个时间戳、键、主键及 reason，否则生成无同步栈符号化的普通错误。
- `canonical_value` 把 `Option<Vec<u8>>` 转为 canonical `ValueEntry`，缺失值映射为 `ErrNotExist`。
- `tikvTxn` 组合脏写缓冲与不可变快照，实现缓冲优先的点查/批量查、正反向联合扫描、语句级检查点、事务选项缓存、提交、重复键错误美化和公平锁状态适配。
- `TiDBKVFilter` 在提交前识别 untouched 索引值，并拒绝“untouched 且 `PresumeKeyNotExists`”这一不变量冲突。

## 主要符号

- `ClientTransactionMode`：公开枚举；`options(self)` 生成官方 client-rust 事务配置，`as_str(self)` 返回 `optimistic` 或 `pessimistic`。
- `map_client_error(tikv_client::Error) -> canonical_kv::errors::SharedError` 与 `canonical_value(Option<Vec<u8>>) -> Result<canonical_kv::ValueEntry, _>`：生产适配器使用的错误和值转换入口。
- `ReturnedValue`、`LockContext`：悲观锁结果模型；后者聚合最大冲突提交时间戳、逐键返回值及可注入的后端错误。
- `AssertionLevel`、`PrewriteEncounterLockPolicy`、`LifecycleHooks`、`TxnSizeLimits`：事务选项的强类型载体。默认大小限制为单条目 6 MiB、事务总量 100 MiB。
- `TxnOption`：公开事务选项总和类型；一部分委托给 `tikvSnapshot::SetOption`，其余写入 `TxnSettings` 或事务字段。`TxnOptionValue` 是当前四种可查询选项的返回包装。
- `TxnSettings`：私有配置快照，保存悲观模式、异步提交、1PC、因果一致性、scope、请求来源、资源组、大小限制、生命周期钩子等状态。
- `tikvTxn` 与 `NewTiKVTxn`：简化事务实体及构造器。`from_entries` 是测试便捷入口。
- `TiDBKVFilter::IsUnnecessaryKeyValue`、私有 `is_untouched_index_key_value`：提交过滤及 TiDB 索引编码尾标记判断。
- `table_with_index`：构造只含一个索引的 `TableInfo`，供调用者和测试准备重复键元数据。

## 执行流程

1. 构造：`NewTiKVTxn` 对共享 `Arc<RwLock<BTreeMap<Key, ValueEntry>>>` 调用 `NewSnapshot`，创建按 `pipelined` 配置的 `memBuffer`，初始化 `TxnSettings`、时间戳、有效性和公平锁标志。
2. 点查：`Get` 先调用 `mem_buffer.Get`；仅在 `NotFound` 时回落 `snapshot.Get`。缓冲中的空值是删除墓碑，对上层仍返回 `NotFound`；最终由 `apply_commit_ts_option` 决定是否保留提交时间戳。
3. 批量查：`BatchGet` 将缓冲和快照交给 `NewBufferBatchGetter`，由三层读取逻辑处理覆盖、删除和未命中，再逐项应用批量 commit-ts 选项。
4. 扫描：`Iter`/`IterReverse` 分别打开 memBuffer 与 snapshot 迭代器，交给 `NewUnionIter` 合并；脏写覆盖快照，墓碑隐藏快照值。快照打开失败时会主动关闭已打开的 dirty 迭代器。
5. 写入：`Set` 在调用 `mem_buffer.Set` 前检查单条目和累计大小；`Delete` 写入空值墓碑。`GetMemDBCheckpoint` 与 `RollbackMemDBToCheckpoint` 支持语句级回退。
6. 加锁：`LockKeys` 在 `is_committer_working` 保护期内先转换注入的后端错误，否则依据 `max_locked_with_conflict_ts` 生成 `WriteConflict`；`LockKeysFunc` 先执行调用方钩子再复用该流程。
7. 提交：`Commit` 设置 committer 工作标志，调用 `commit_inner`，恢复标志后无论成功失败均触发可选 `CommitHook`。`commit_inner` 校验事务仍有效及 commit-ts 上界，逐项运行 `TiDBKVFilter`，再持有 storage 写锁应用写入或删除，最后记录 `commit_ts` 并令事务失效。
8. 选项：`SetOption` 在 committer 工作时拒绝变更；读相关选项下推给 snapshot，其他选项保存在 `TxnSettings`、回调字段或列映射缓存。`GetOption` 当前只暴露线性一致性、scope 和两类请求来源。

## 数据与状态

`tikvTxn.storage` 是提交目标，`snapshot` 是构造时绑定的只读接口，`mem_buffer` 保存本事务脏写；三者共同形成“buffer 优先、snapshot 回退、commit 刷入 storage”的状态机。这里的 snapshot 不是冻结的 MVCC 副本：`snapshot.rs::NewSnapshot` 持有同一份 `Arc<RwLock<BTreeMap<...>>>`，只是在 API 上不提供写操作。`start_ts` 构造后不变，`commit_ts` 初始为 0、成功提交后更新；`valid` 初始为 `true`，成功提交后变为 `false`，重复提交会返回后端错误。

删除不使用额外枚举，而以空 `Vec<u8>` 作为 tombstone。`idx_name_cache` 可同时以物理分区 id 和逻辑表 id 缓存 `TableInfo`；`CacheTableInfo(id, None)` 会删除旧项，避免留下陈旧元数据。`column_maps_cache` 和 `vars` 使用类型擦除边界，但 `SetVars` 当前只接受能 downcast 成 `String` 的值。

`TxnSettings` 中的 `causal_consistency` 与公开选项 `GuaranteeLinearizability` 语义相反：设置后保存其逻辑非值，读取时再取反。`resource_group_tagger`、commit hook 和 commit-ts 上界检查均为 `Arc<dyn Fn... + Send + Sync>`，允许跨线程共享回调。

## 依赖与调用关系

RustCodeGraph 对目标文件显示 120 个符号节点。已核对的关键下游边包括：`NewTiKVTxn -> NewSnapshot`，`from_entries -> NewTiKVTxn`，`LockKeys -> extract_key_error / generate_write_conflict_for_locked_with_conflict`，`Commit -> commit_inner -> TiDBKVFilter::IsUnnecessaryKeyValue -> is_untouched_index_key_value`，`Iter/IterReverse -> NewUnionIter`，以及 `BatchGet -> NewBufferBatchGetter`。

生产调用方面，`pkg/store/driver/kv_adapter.rs` 导入 `ClientTransactionMode`、`map_client_error`、`canonical_value`：`ClientSnapshot::new` 和 `begin_transaction` 用 `mode.options()` 创建 client-rust 快照/事务；扫描、点查、开始事务及提交等路径用 `map_client_error`；事务点查再用 `canonical_value` 统一未命中语义。该文件中的 `tikvTxn` 经 `lib.rs` 导出，但仓库搜索到的 `NewTiKVTxn` Rust 调用点位于 `pkg/store/driver/txn_test.rs`。

内部模块依赖为：`snapshot.rs` 提供快照及选项，`unionstore_driver.rs` 提供 `memBuffer` 与检查点，`batch_getter.rs` 提供缓冲/快照批量合并，`union_iter.rs` 提供扫描合并，`error.rs` 提供 `DriverError`、写冲突和重复键错误格式化。表键头解析与索引 id mask 来自 `astersql-tablecodec`。

## 错误处理与边界

`map_client_error` 只对能提取出 `WriteConflict` 的 client-rust 错误保留结构化冲突语义；其他错误被转为 `NewNoStackError(error.to_string())`。`canonical_value(None)` 明确产生 canonical `ErrNotExist`。本地读路径仅把 memBuffer 的未命中当作快照回退条件，其他缓冲错误直接传播；墓碑不会作为成功的空值返回。

重复键处理由 `extract_key_error` 分流。无法解码表键头、找不到表信息或找不到有效值时退化为 `KeyExists { name: "UNKNOWN" }`；记录键调用 `ExtractKeyExistsErrFromHandle`，索引键调用 `ExtractKeyExistsErrFromIndex`。非流水线模式从 memBuffer 取值，流水线模式使用后端携带值。

`Set` 的大小检查采用“键长 + 值长”和当前 `mem_buffer.Size()`；超限分别返回 `transaction entry is too large` 与 `transaction is too large`。`StartFairLocking` 在 `next_gen_kernel` 标志下返回 `NotImplemented`。`TiDBKVFilter` 遇到 untouched 与 `PresumeKeyNotExists` 同时存在时返回错误，而不是静默过滤。

值得注意的当前边界是：`Iter`/`IterReverse` 在 `NewUnionIter` 自身失败时没有像 Go 版本那样显式关闭两个输入迭代器；只能确认快照打开失败时关闭 dirty 迭代器。文档不推断 trait 对象析构是否等价于 Go 的显式 `Close`。此外 `TxnOption::KVFilter` 当前分支不保存传入值，属于与 Go 动态替换 filter 不同的现状。

## 并发与资源生命周期

共享存储以 `Arc<RwLock<BTreeMap<...>>>` 管理：快照/读取使用其读侧实现，`commit_inner` 在整个批量应用阶段持有写锁；毒化锁通过 `PoisonError::into_inner` 继续使用内部值。`mem_buffer` 以 `Arc` 共享，其内部同步语义由 `unionstore_driver.rs` 负责。

`is_committer_working: AtomicBool` 以 `SeqCst` 在 `LockKeys`、`Commit`、流水线 `MayFlush` 周围建立选项修改禁区，`SetOption` 观察到该标志时返回 `CommitterWorking`。这不是互斥锁：代码依赖调用生命周期正确配对设置/清除；当前函数都是同步执行，清除发生在返回之前。commit hook 在工作标志清除后执行。

扫描迭代器由调用者负责通过 `KvIterator::close` 释放；构造快照迭代器失败时文件显式关闭已创建的 dirty 迭代器。公平锁生命周期通过 `StartFairLocking`/`RetryFairLocking` 将标志置为真，以 `CancelFairLocking`/`DoneFairLocking` 置为假。成功提交将 `valid` 永久置假；本文件没有把失败提交自动标记失效，也没有显式 rollback API。

## 与 Go 版本的对应关系

`pkg/store/driver/txn/txn_driver.go` 的 `tikvTxn` 包装真实 `*tikv.KVTxn`，Rust 的同名结构则组合内存 storage/snapshot/memBuffer，因此 Rust 复刻了接口行为和分支，但不是底层实现的一一替换。真实 client-rust 网络事务位于 `pkg/store/driver/kv_adapter.rs::ClientTransaction`。

主要对齐点包括：缓冲优先读取、空值视为不存在、缓冲与快照的联合扫描、表信息双 id 缓存、重复键错误美化、`LockedWithConflict` 到写冲突、选项下推/缓存、公平锁适配及 untouched index 过滤。`TxnOption` 枚举逐项覆盖了 Go `SetOption` 的 switch 分支，包含 size limits、session id、后台生命周期和 prewrite 遇锁策略。

主要差异包括：Go 构造器接收真实 `*tikv.KVTxn` 并安装 `TiDBKVFilter`，Rust `NewTiKVTxn` 接收共享 map；Go `Commit` 委托 client-go 2PC，Rust `commit_inner` 直接刷内存 map；Go 的 committer 检查主要在 `intest` 下启用且 `SetOption` 触发断言，Rust 始终使用原子标志并返回错误；Go `SetVars` 接受 `*tikv.Variables`，Rust 当前仅保存 `String`；Go 扫描在 union iterator 构造失败时显式关闭 dirty 和 snapshot，Rust 当前未显式做该分支清理；Go 的 `KVFilter` 会下推，Rust 对应选项目前为空操作。

独立 Rust 测试覆盖也不等同于 Go 测试布局：`pkg/store/driver/txn_test.rs` 验证点查、批量查、commit-ts、缓冲覆盖/删除、拦截器错误及正反向扫描；`pkg/store/driver/txn/txn_driver_test.rs` 验证 `CacheTableInfo(None)` 清除陈旧项；`driver_test.rs` 验证 client-rust 模式、普通错误/写冲突映射和 canonical 未命中值。未见对本文件提交、大小限制、锁冲突、公平锁或 filter 分支的直接 Rust 单元测试。

## 扩展指南

- 新增事务选项时，应同时扩展 `TxnOption`、必要的 `TxnSettings` 字段、`SetOption` 分支和可能的 `GetOption`/`TxnOptionValue`，并对照 Go `tikvTxn.SetOption`。读路径选项应优先下推到 `SnapshotOption`，提交期选项要检查 committer 期间不可变约束。
- 修改读取语义时，要一起审查 `Get`、`BatchGet`、`Iter`、`IterReverse` 以及 `batch_getter.rs`/`union_iter.rs`，保持“脏写优先、墓碑遮蔽、未命中才访问快照”的一致性；测试应放在独立的 `pkg/store/driver/txn_test.rs` 或同目录 `*_test.rs`，不要嵌入生产文件。
- 修改提交时，应保留有效性检查、commit-ts 上界、KV filter、空值删除、hook 错误观察及成功后失效的顺序。若目标是真实 TiKV 行为，应改动/验证 `kv_adapter.rs::ClientTransaction`，不能只增强本地 map 适配器。
- 修改错误映射时，应覆盖嵌套 `KeyError`、`ExtractedErrors`、`MultipleKeyErrors`、`PessimisticLockError` 和无冲突普通错误；保留 canonical 错误码和 reason 是兼容性重点。
- 修改 untouched 判断或重复键解码时，应以 `astersql-tablecodec` 的编码规则和 Go `tablecodec.IsUntouchedIndexKValue` 为基准；风险集中在唯一索引兼容、错误文案以及误过滤写入。
- 性能敏感点是扫描合并、批量查回落、提交时克隆/遍历全部缓冲项，以及持有 storage 写锁的持续时间；扩展时避免在锁内加入慢回调或网络操作。

## 验证依据

- 源码与模块：`pkg/store/driver/txn/txn_driver.rs`、`pkg/store/driver/txn/lib.rs`、`pkg/store/driver/txn/Cargo.toml`。
- RustCodeGraph：`status` 显示项目索引含目标目录；`files --filter pkg/store/driver/txn` 列出 Rust/Go 对照文件；`node --file ...txn_driver.rs` 读取 1–870 行并报告 120 个符号；`query NewTiKVTxn` 区分 Go/Rust 定义；调用图核对了 `NewSnapshot`、`NewUnionIter`、`NewBufferBatchGetter`、`commit_inner`、`TiDBKVFilter` 等上述边。
- 生产接线：`pkg/store/driver/kv_adapter.rs` 的 import、`ClientSnapshot::new`、`begin_transaction`、扫描与点查路径，证明三项 client-rust 辅助 API 的实际使用，并定位真实网络事务实现。
- Go 对照：`pkg/store/driver/txn/txn_driver.go`，逐项核对构造、读写、扫描、选项、错误、公平锁、flush 和 filter。
- 独立测试：`pkg/store/driver/txn/txn_driver_test.rs`、`pkg/store/driver/txn/driver_test.rs`、`pkg/store/driver/txn_test.rs`。这些测试分别提供缓存清除、client-rust 映射，以及本地事务读/批量读/扫描行为证据。
- 本任务为纯文档分析，按计划不运行 Cargo；最终只执行任务指定的 11 章节结构检查，并人工复核文档没有把内存适配器描述成真实 TiKV 2PC。
