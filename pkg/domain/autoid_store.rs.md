# `pkg/domain/autoid_store.rs`

## 文件定位

本文件是 `astersql-domain` crate 内部的 AutoID 持久化适配层。模块由 `pkg/domain/lib.rs` 以私有 `mod autoid_store` 装配，不对 crate 外直接暴露；`pkg/domain/domain.rs::Domain::new_with_storage_handle` 用 Domain 唯一的 `Arc<StorageHandle>` 构造 `KvAutoIdStore`，擦除为 `Arc<dyn astersql_meta_autoid::IdStore>` 后保存到 `Domain::auto_id_store`。因此它位于 Domain 生命周期管理、规范 KV 存储与 `pkg/meta/autoid` 分配算法之间，而不是 AutoID 批量计算算法本身。

`pkg/domain/Cargo.toml` 将本 crate 定义为 `astersql-domain`，并直接依赖 `astersql-kv` 与 `astersql-meta-autoid`；本文件正是这两个 crate 的边界接线。源码没有条件编译项，也没有内嵌测试模块。

## 核心职责

1. `KvAutoIdStore` 把 Domain 共享的 `StorageHandle` 实现为 `IdStore`，让 `DefaultAllocator` 等上层分配器可以在新 KV 事务中原子地读写高水位。
2. `KvAutoIdTransaction` 把 `astersql_kv::Transaction` 的 `Get`/`Set` 接口缩窄为 `IdTransaction` 的 `get`、`put`、`inc`、`copy_to`。
3. `KvAutoIdTransaction::key` 将逻辑键 `(database_id, table_id, AutoIdKeyKind)` 映射为独立的 canonical 扁平键，隔离 RowID、V5 起的 AUTO_INCREMENT、AUTO_RANDOM、SEQUENCE 值与 SEQUENCE cycle 轮次。
4. `get`/`put` 在 KV 字节值与十进制 `i64` 之间转换，并把底层错误归一为 `AutoIdError::Storage`。
5. `run_in_transaction` 保留业务闭包产生的原始 `AutoIdError`，同时把它临时桥接为 KV 错误以触发底层回滚。

本文件不负责分配窗口、increment/offset 校验、溢出判定或本地缓存状态；这些逻辑位于 `pkg/meta/autoid/autoid.rs` 的 `DefaultAllocator`。它也没有删除键的能力，因为当前 `IdTransaction` trait 只定义读、写、递增与复制。

## 主要符号

- `AUTO_ID_KEY_PREFIX: &str = "mAutoID:canonical:v1"`：本适配器所有物理键的命名空间与格式版本。最终键形如 `mAutoID:canonical:v1:DB:<database_id>:<kind>:<table_id>`。
- `SEPARATE_AUTO_INCREMENT_VERSION: u16 = 5`：表元数据版本分界；语义对应 Go `pkg/meta/meta_autoid.go::sepAutoIncVer = model.TableInfoVersion5`。
- `KvAutoIdStore { storage: Arc<StorageHandle> }`：crate 内可见的持久化 store。`new` 仅保存共享句柄，不打开事务或复制底层 Storage。
- `KvAutoIdTransaction<'a>`：一次 `RunInNewTxn` 回调期间存在的非公开借用适配器，持有 `&'a mut dyn astersql_kv::Transaction` 和一个 KV `Context`。
- `KvAutoIdTransaction::key(AutoIdKey) -> astersql_kv::Key`：纯映射函数。`RowId` 与版本小于 5 的 `IncrementId` 使用 `TID`；版本至少 5 的 `IncrementId` 使用 `IID`；`RandomId`、`SequenceValue`、`SequenceCycle` 分别使用 `TARID`、`SID`、`SequenceCycle`。
- `KvAutoIdTransaction::storage_error`：把任意可显示错误丢失类型信息后转成 `AutoIdError::Storage(error.to_string())`。
- `IdTransaction::{get, put, inc, copy_to}` 实现：提供单事务内的高水位操作。`inc` 是先读再以 `wrapping_add` 计算后写；`copy_to` 只在源值非零时写目标。
- `IdStore::run_in_transaction` 实现：经 `StorageHandle::with_storage` 调用 `astersql_kv::RunInNewTxn(..., retryable = true, ...)`，并决定最终返回业务错误还是存储错误。

公开性边界是有意收紧的：只有 `KvAutoIdStore` 和其构造函数是 `pub(crate)`；事务适配器、键编码与错误转换均为模块私有。

## 执行流程

典型调用链如下：

1. `Domain::new_with_storage_handle` 创建一个 `KvAutoIdStore`，存入 `Domain::auto_id_store`。
2. `Domain::new_stats_auto_id_allocator` 把该 trait object 传给 `DefaultAllocator::with_options`，同时传入表的 `DBID`、`ID`、`AutoIDCache` 和 `Version`。
3. `pkg/meta/autoid/autoid.rs` 中的分配、rebase、sequence cache 或 transfer 路径调用 `IdStore::run_in_transaction`；RustCodeGraph 的 `run_in_transaction` 探索结果确认其上游包括 `alloc_signed_locked`、`alloc_unsigned_locked`、`alloc_seq_cache`、`force_rebase`、`rebase_seq`、`next_global_auto_id` 与 `transfer`。
4. `run_in_transaction` 取得 `StorageHandle` 的读锁，以默认 `Context` 调用可重试的 `RunInNewTxn`。每次底层回调构造一个只借用当前事务的 `KvAutoIdTransaction`，再执行上层 `FnMut`。
5. 上层操作通过逻辑 `AutoIdKey` 调用 `get`/`inc`/`put`/`copy_to`。键先被编码；值以十进制文本读取或写入。
6. 操作成功时，KV 层提交事务；可重试的提交错误由 `RunInNewTxn` 重新 Begin 并再次调用闭包。操作返回 `AutoIdError` 时，本文件保存其 clone、令 KV 回调返回错误，底层随即回滚，最后再返回原始业务错误。

关键分支包括：

- `get` 遇到 `IsErrNotFound` 返回 `0`，把“尚未持久化”定义为零水位；其他读取、UTF-8 或十进制解析错误均失败。
- `inc` 使用 `wrapping_add`，与 `pkg/meta/autoid` 内存测试 store 的契约一致；合法范围与耗尽检查应由调用它的 allocator 在事务内先完成。
- `copy_to` 的零值短路避免用不存在/零水位覆盖目标，语义对应 Go `autoIDAccessor.CopyTo` 的兼容性保护。

## 数据与状态

持久状态只有 canonical KV 中的十进制 `i64` 文本。逻辑键由 `AutoIdKey { database_id, table_id, kind }` 完整定位；本适配器不读取表对象来验证库表是否仍存在，这允许 rename/transfer 场景继续按原始库表身份处理水位，也与 Go accessor 的注释约束一致。

键种类到物理片段的映射如下：

| `AutoIdKeyKind` | 条件 | 物理片段 |
| --- | --- | --- |
| `RowId` | 始终 | `TID` |
| `IncrementId(version)` | `version < 5` | `TID` |
| `IncrementId(version)` | `version >= 5` | `IID` |
| `RandomId` | 始终 | `TARID` |
| `SequenceValue` | 始终 | `SID` |
| `SequenceCycle` | 始终 | `SequenceCycle` |

版本 5 前 RowID 与 AUTO_INCREMENT 有意共享同一键；版本 5 起两者分离。`SequenceCycle` 保存的是已循环轮次，而不只是布尔值，`pkg/meta/autoid/autoid.rs::alloc_seq_cache` 会读取、递增并写回它。

进程内状态很少：`KvAutoIdStore` 只持有一个 `Arc`；每次事务临时持有 KV transaction、默认 context 和 `operation_error: Option<AutoIdError>`。分配器缓存的 `base/end` 不在本文件中。

## 依赖与调用关系

上游接线：

- `pkg/domain/lib.rs` 声明私有模块。
- `pkg/domain/domain.rs::Domain::new_with_storage_handle` 构造并持有 `KvAutoIdStore`。
- `pkg/domain/domain.rs::Domain::new_stats_auto_id_allocator` 将 store 注入 `astersql_meta_autoid::DefaultAllocator`。
- `pkg/meta/autoid/autoid.rs` 的 allocator 方法通过 `IdStore`/`IdTransaction` trait 消费本适配器；例如分配路径执行 `get` 后 `inc`，sequence cycle 路径执行 `get`/`put`/`inc`，transfer 路径执行 `copy_to`。

下游依赖：

- `crate::canonical_domain::StorageHandle::with_storage`：在 Domain 存储读锁保护下提供 `&dyn Storage`。
- `astersql_kv::{RunInNewTxn, Transaction, Context, Key, IsErrNotFound}`：事务创建、重试、回滚/提交与原始 KV 操作。
- `astersql_meta_autoid::{IdStore, IdTransaction, AutoIdKey, AutoIdKeyKind, AutoIdError}`：上层抽象与统一错误类型。
- `std::sync::Arc`：共享 Domain 唯一存储句柄。

RustCodeGraph `status` 显示索引包含本仓库 Rust/Go 文件；`node KvAutoIdStore` 和 `node KvAutoIdTransaction` 定位到本文件第 15、25 行。精确 trait 方法的 callers/callees 查询未返回边，因此上述具体接线又由 `rg` 对 `domain.rs` 与 `meta/autoid/autoid.rs` 的直接引用补证，而不是把空图结果解释为“无人调用”。

## 错误处理与边界

- KV `Get` 的 not-found 是正常初始状态，转换成 `Ok(0)`；只有真正的存储错误进入 `AutoIdError::Storage`。
- 已存在但不是 UTF-8 或不是合法 `i64` 十进制文本的值会返回存储错误。文件不会修复、忽略或重置损坏值。
- `Set` 错误直接映射为 `AutoIdError::Storage`。
- `inc` 的算术明确采用补码回绕。allocator 的有符号/无符号边界计算负责在调用前判断是否耗尽；若未来绕过 allocator 直接调用，不能期待本适配器提供溢出保护。
- 上层 operation 失败时，其原始 `AutoIdError` 优先返回；包装出的 KV 错误只用于让 `RunInNewTxn` 回滚。Begin、Commit 或重试耗尽等底层错误则最终转成 `AutoIdError::Storage`。
- `copy_to` 不写零值。这是 Go 兼容语义，防止 rename/BR 跨版本场景把目标已有水位覆盖为零；它也意味着不能用此方法主动把目标清零。
- `key` 直接格式化有符号数据库/表 ID，不校验正数或对象存在性；合法身份由上游保证。
- `RunInNewTxn` 被设为可重试，但 operation 自身产生的包装错误不是事务可重试错误，通常立即回滚返回；提交冲突则可能重新执行整个 `FnMut`。因此传入 operation 必须只通过当前事务产生可安全重放的副作用。

## 并发与资源生命周期

`KvAutoIdStore` 可作为 `Arc<dyn IdStore>` 跨线程共享，因为 `IdStore: Send + Sync`，而共享资源由 `Arc<StorageHandle>` 持有。`StorageHandle::with_storage` 在闭包完整执行期间持有内部 Storage 的读锁；`StorageHandle::close` 需要写锁，所以关闭不会与这里的事务访问同时取得底层对象。

每个 `KvAutoIdTransaction` 只活到一次 `RunInNewTxn` 回调结束，其可变事务借用不能逃逸。成功时由 `RunInNewTxn` 提交，operation 或提交失败时由底层路径回滚/返回；本文件不自行 Commit、Rollback 或管理重试退避。KV 事务提供跨进程并发序列化边界，而不是 `KvAutoIdStore` 上的互斥锁。

底层发生可重试提交错误时，`RunInNewTxn` 可能创建新事务并重放 operation。`operation_error` 仅用于在一次业务失败后恢复原始错误；成功提交路径不会留下长期状态。`context` 当前使用 `Context::default()`，没有在本文件设置内部请求来源、deadline 或取消信号。

## 与 Go 版本的对应关系

仓库没有同路径的 `pkg/domain/autoid_store.go`；Rust 文件是为了把移植后的 trait 型 `astersql-meta-autoid` 接到 Domain 规范 Storage 而新增的局部适配层。最接近的 Go 行为依据是 `pkg/meta/meta_autoid.go`：

- Rust `get`/`put`/`inc`/`copy_to` 分别对应 Go `autoIDAccessor.Get`/`Put`/`Inc`/`CopyTo`。
- `SEPARATE_AUTO_INCREMENT_VERSION = 5` 对应 `sepAutoIncVer = model.TableInfoVersion5`；两边都令旧表的 RowID 与 AUTO_INCREMENT 共用 `TID`，新表的 AUTO_INCREMENT 使用 `IID`。
- 五种 kind 字符串与 `pkg/meta/meta.go` 的 `mTableIDPrefix`、`mIncIDPrefix`、`mRandomIDPrefix`、`mSequencePrefix`、`mSeqCyclePrefix` 一致。
- `copy_to` 的“零值不复制”与 Go 实现完全相同，保护 rename 与跨版本 BR 恢复场景。
- Go `Inc` 也明确不验证 schema/table 是否存在，以支持 rename 后沿原始完整 ID 并发使用；Rust 同样没有存在性检查。

物理布局并不等同：Go accessor 使用 `dbKey` 下的 hash field（`HGetInt64`/`HSet`/`HInc`），Rust 适配器则写入以 `mAutoID:canonical:v1` 开头的独立扁平 KV 键。因此本文只能确认 accessor 选择、缺失值、十进制值与事务语义的对齐，不能声称两种实现可以直接读取彼此的底层编码。Go 接口还有 `Del`，当前 Rust `IdTransaction` 没有对应方法。

## 扩展指南

- 新增 AutoID 种类时，应同时扩展 `pkg/meta/autoid/autoid.rs::AutoIdKeyKind` 与本文件 `KvAutoIdTransaction::key`，选择不会与现有 `TID/IID/TARID/SID/SequenceCycle` 冲突的稳定片段；若会持久化到已有集群，还需明确 key 格式版本与升级/回滚兼容性。
- 修改 V5 分界或键映射时，必须与 `pkg/meta/meta_autoid.go::IncrementID`、表版本定义和 allocator 的 `TableInfoVersion` 传递一起核对，否则 RowID 与 AUTO_INCREMENT 可能错误共享或分裂水位。
- 修改值编码时需保留对现有十进制 `i64` 数据的读取兼容，或提供显式迁移；尤其要覆盖负数、`i64::MIN/MAX`、非 UTF-8 和非法十进制数据。
- 为事务增加删除、compare-and-set 或其他操作，应先扩展 `IdTransaction` trait，再在本适配器实现，不能把 KV transaction 泄漏给 allocator。
- 修改 `run_in_transaction` 时应保留两类错误的区别：operation 的原始 `AutoIdError` 与底层 Begin/Commit 错误；还应验证提交冲突重试会重放闭包这一约束。
- 最适合新增独立测试的位置是同目录 `pkg/domain/autoid_store_test.rs`，并由 `pkg/domain/lib.rs` 的 `#[cfg(test)]` 模块声明接入，避免把测试写回生产源文件。建议使用可观测的 mock `Storage` 覆盖五类键、V4/V5 分界、缺失值、损坏值、零值 copy、operation 回滚、提交错误与重试；同时保留 `pkg/meta/autoid/autoid_test.rs` 和 `seq_autoid_test.rs` 对 allocator 契约的回归。
- 性能上，`inc` 当前是一次 Get 加一次 Set，而不是底层原子增量原语；任何优化都必须仍处于同一 KV 事务并保留 allocator 对当前值的读取语义。键或值格式变化属于持久化兼容风险，不能只以单元测试通过作为升级安全证据。

## 验证依据

本说明基于以下直接证据完成事实复核：

- 目标实现：`pkg/domain/autoid_store.rs`。
- crate 与模块边界：`pkg/domain/Cargo.toml`、`pkg/domain/lib.rs`。
- Domain 接线与资源生命周期：`pkg/domain/domain.rs::{Domain::new_with_storage_handle, Domain::new_stats_auto_id_allocator}`、`pkg/domain/canonical_domain.rs::StorageHandle`。
- trait、逻辑键与真实上层调用：`pkg/meta/autoid/autoid.rs::{AutoIdKeyKind, AutoIdKey, IdTransaction, IdStore, DefaultAllocator}`；错误定义来自 `pkg/meta/autoid/errors.rs::AutoIdError`。
- KV 事务提交、回滚与重试：`pkg/kv/txn.rs::RunInNewTxn`。
- Go 对照：`pkg/meta/meta_autoid.go::{autoIDAccessor, autoIDAccessors}` 与 `pkg/meta/meta.go` 的五类键前缀/编码函数。
- 相关独立测试：`pkg/meta/autoid/autoid_test.rs` 验证分配、并发和事务失败不推进本地状态；`pkg/meta/autoid/seq_autoid_test.rs` 验证 sequence cache/cycle 与并发唯一性。这些测试使用内存 `IdStore`，不直接实例化 `KvAutoIdStore`；`rg` 在 `pkg/domain/*test.rs` 也未找到该适配器的直接测试，因此真实 KV 键编码与错误转换目前是明确的直接覆盖空白。
- RustCodeGraph：`status` 报告有效索引；`node KvAutoIdStore`、`node KvAutoIdTransaction` 确认定义位置；`explore` 确认 `pkg/meta/autoid` 多个 allocator 方法对 `run_in_transaction` 的调用。精确 trait 方法图边为空，已用上述源码引用补证。

人工复核结论：该文件存在的原因是把 Domain 唯一 KV Storage 转换为 AutoID allocator 所需的窄事务接口；运行时关键不变量是稳定且隔离的键映射、缺失即零、事务成功后才推进持久水位、业务错误原样返回，以及旧版本自增与 RowID 的共享关系。安全扩展必须同步逻辑 kind、持久键兼容、Go accessor 语义和独立测试。
