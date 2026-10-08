# `pkg/store/mockstore/unistore/tikv/write.rs`

## 文件定位

本文件位于 `astersql-store-mockstore-unistore-tikv` crate，crate 入口 `pkg/store/mockstore/unistore/tikv/lib.rs` 通过 `pub mod write` 导出它。它用纯内存数据结构表达 UniStore 的写批、版本记录和锁表更新，目标是复刻同目录 `write.go` 中 `writeBatch`/`dbWriter` 的核心语义，供 Rust 移植代码和独立测试使用。

当前接线范围必须特别区分：`pkg/store/mockstore/unistore/tikv/mvcc/db_writer.rs` 另行定义了 Go 风格的 `DBWriter`、`WriteBatch` trait，但本文件的同名具体类型没有实现这些 trait；当前 Rust `MvccStore` 在自己的 `RwLock<StoreState>` 上直接完成预写、提交和回滚，`Server::kv_delete_range` 也调用 `MvccStore::delete_file_in_range`，而不是调用这里的 `DbWriter::delete_range`。仓库搜索到的直接使用者是 `write_test.rs` 和 `main_test.rs`。因此，本文件目前是可执行、被测试的写路径组件，但不是 Rust RPC 主链的底层存储实现。

`Cargo.toml` 的 `[lib] path = "lib.rs"` 确认其 crate 边界；本文件自身只依赖标准库和同 crate 的 `mvcc::{Lock, MutationOp, MAX_SYSTEM_TS}`。Cargo 的 Windows 条件依赖列出了完整 UniStore 周边 crate，但这里没有直接使用它们。

## 核心职责

1. 用 `WriteBatch` 收集一次事务动作产生的版本化 DB 变更和锁表变更，使调用方可以先构造、后统一应用。
2. 把 2PC 动作映射为存储变更：`prewrite`/`pessimistic_lock` 设置锁，`commit` 写提交版本或额外事务状态并删锁，`rollback` 写回滚状态并按需删锁，`pessimistic_rollback` 只删锁。
3. 由 `MemoryWriteBackend` 保存按 `(user key, version)` 排序的版本条目和按 user key 索引的锁，并执行 Go 对齐的锁条目大小检查。
4. 由 `DbWriter` 管理开闭状态、观测到的最大时间戳、DB 后锁的应用顺序，以及 `[start, end)` 范围版本删除。
5. 用 `encode_user_meta` 与 `extra_txn_status_key` 保持 Go 的元数据字节布局和回滚/`Op::Lock` 辅助键编码。

文件不负责 MVCC 冲突检查、Region 校验、latch 获取、持久化恢复或 RPC 转换；这些能力分别位于 `mvcc.rs`、`server.rs`、`mvcc/db_writer.rs` 的接口设计及 Go 实现中。

## 主要符号

- `BATCH_CHANNEL_SIZE: usize = 1024`：与 Go `batchChanSize` 对齐的容量常量。Rust 当前实现没有通道或后台 worker，因此该常量当前未参与执行。
- `DELETE_RANGE_BATCH_SIZE: usize = 4096`：`DbWriter::delete_range` 每次持有 DB 写锁删除的最大条目数，对应 Go `delRangeBatchSize`。
- `DbEntry`：一条版本化记录，包含 `key`、`version`、`value`、16 字节 `user_meta` 和 `delete` 标记。内存后端以 `(key, version)` 为主键。
- `LockEntry::{Set, Delete}`：锁表操作。`Set` 携带完整 `Lock`，`Delete` 仅携带 user key。
- `WriteBatch`：公开的事务批，保存 `start_ts`、`commit_ts`、`db_entries`、`lock_entries`；五个方法分别表达预写、提交、回滚、悲观加锁和悲观锁回滚。
- `MemoryWriteBackend`：用 `RwLock<BTreeMap<(Vec<u8>, u64), DbEntry>>` 保存有序版本，用 `RwLock<HashMap<Vec<u8>, Lock>>` 保存锁；`get_lock`、`versions` 提供只读观测，`apply_db`、`apply_locks` 是内部应用入口。
- `DbWriter`：持有共享后端 `Arc<MemoryWriteBackend>`、`AtomicU64 latest_ts` 和 `AtomicBool open`。公开方法为 `new`、`open`、`close`、`is_open`、`new_write_batch`、`write`、`latest_ts`、`delete_range`。
- `encode_user_meta(start_ts, commit_ts)`：依次按小端编码两个 `u64`，与 Go `mvcc.NewDBUserMeta` 使用的 `defaultEndian` 布局一致。
- `extra_txn_status_key(key, start_ts)`：在 key 后追加 `!start_ts` 的大端字节，再把首个 key 字节加一，对应 Go `codec.EncodeUintDesc` 后 `ret[0]++` 的排序编码。

## 执行流程

典型的直接使用流程如下（证据见 `main_test.rs::memory_db_writer_preserves_lock_then_commit_order`）：

1. 调用方创建 `Arc<MemoryWriteBackend>` 和 `DbWriter::new`；writer 初始为关闭状态，必须先 `open`。
2. 预写阶段调用 `new_write_batch(start_ts, 0)`。该方法用 `start_ts` 更新 `latest_ts`，返回空批；`WriteBatch::prewrite` 将 `LockEntry::Set` 追加到锁操作列表。
3. `DbWriter::write` 先检查 writer 已打开，再调用 `apply_db` 和 `apply_locks`。预写批没有 DB 条目，结果是在锁表中放入锁。
4. 提交阶段调用 `new_write_batch(start_ts, commit_ts)`，这次用 `commit_ts` 更新 `latest_ts`。`WriteBatch::commit` 根据 `lock.op` 选择 DB 行为：
   - 普通 `Put`、`Delete`、`Insert` 等非 `PessimisticLock`/`Lock` 操作，在 `commit_ts` 版本写入 `lock.value` 和 start/commit 元数据；这里的 `delete` 字段仍为 `false`，删除的 MVCC 语义由上层操作/读路径解释，而非物理删除该版本。
   - `PessimisticLock` 不写 DB，效果等同悲观锁回滚。
   - `Lock` 仅当 key 等于主键时写入额外事务状态记录，版本为 `start_ts`；次键不写该记录。
   - 所有分支最后都追加 `LockEntry::Delete`。
5. `write` 始终先 `apply_db`，成功写入版本后才 `apply_locks` 删除锁，维持“提交版本先可见、随后解锁”的顺序。
6. 回滚时，`WriteBatch::rollback` 在额外事务状态 key、`start_ts` 版本处写空值，元数据 commit_ts 为 0；只有 `delete_lock == true` 才追加删锁。
7. 悲观锁路径只通过 `pessimistic_lock` 设置锁，或通过 `pessimistic_rollback` 删除锁，不生成 DB 版本。

范围删除是另一条独立流程：`delete_range` 拒绝空 end，先在 DB 读锁下收集 user key 属于 `[start, end)` 的全部 `(key, version)`，再按 4096 项分块取得写锁并删除。锁表不在该方法的删除范围内。

## 数据与状态

- DB 状态以 `(Vec<u8>, u64)` 排序。`versions(key)` 通过从 `(key, 0)` 到 `(key, u64::MAX)` 的闭区间范围查询返回该 user key 的所有版本，因此同一 key 的版本按时间戳升序排列。
- 锁状态以 user key 唯一索引；再次 `Set` 同一 key 会覆盖旧锁，`Delete` 对不存在的 key 是无操作。
- `WriteBatch` 只是一组可变向量，没有已提交标记；调用 `write(batch)` 会消费 batch，避免同一个值被直接重复提交。
- `latest_ts` 在创建批时更新，而不是在 `write` 成功后更新。commit_ts 大于 0 时观察 commit_ts，否则观察 start_ts；`MAX_SYSTEM_TS` 是哨兵值，会被忽略。`fetch_max` 使时间戳单调不降。
- `open` 仅控制 `write` 是否接受批次；`delete_range` 当前不检查开闭状态。`close` 也不清空数据，后端可继续通过查询方法读取。
- `max_lock_entry_size == 0` 表示不限制。限制值大于 0 时，大小按 `user key + Go Lock.MarshalBinary()` 估算：固定头 40 字节，加 primary、value，以及每个 async-commit secondary 的 2 字节长度前缀和内容。
- `DbEntry::delete` 支持后端物理删除精确 `(key, version)`，但本文件的事务方法不会产生 `delete: true`；`delete_range` 直接从 map 删除，也不经 `DbEntry`。

## 依赖与调用关系

上游事实：

- `lib.rs` 导出 `write` 模块，并仅在 `cfg(test)` 下把独立 `write_test.rs` 编入 crate。
- `write_test.rs` 直接构造 `MemoryWriteBackend`/`DbWriter`，验证字节编码、锁大小与空 end panic。
- `main_test.rs` 直接验证“预写加锁 -> 提交写版本并删锁 -> close 后拒绝 write”以及超大锁报错。
- RustCodeGraph 对 `new_write_batch` 的调用图确认它调用 `update_latest_ts` 并构造 `WriteBatch`；对 `WriteBatch::commit` 的精确节点确认其调用 `encode_user_meta`、`extra_txn_status_key` 并构造 `DbEntry`/`LockEntry::Delete`。

下游依赖：

- `WriteBatch` 依赖 `mvcc::Lock` 的 `op`、`primary`、`value`、`secondaries` 等字段，并依据 `MutationOp::{PessimisticLock, Lock}` 分支。
- `DbWriter::write` 下沉到 `MemoryWriteBackend::{apply_db, apply_locks}`。
- `MemoryWriteBackend` 只依赖标准库同步原语与集合，不依赖 Badger、lockstore 或异步运行时。

当前没有主链调用边：`Server::{kv_prewrite, kv_commit, kv_batch_rollback}` 调用 `MvccStore` 自身的方法，`Server::kv_delete_range` 调用 `MvccStore::delete_file_in_range`。若未来要把本文件接入主链，必须先解决其 API 与 `mvcc/db_writer.rs` trait、latch/context 参数和当前 `MvccStore` 状态模型之间的差异，不能仅替换构造函数。

## 错误处理与边界

- `DbWriter::write` 在关闭状态返回字符串错误 `DB writer is closed`；没有隐式打开或自动重试。
- `apply_locks` 在条目超过配置上限时返回 `unistore lock entry too big {size} > {limit}`。它逐条应用，因此同一批中错误之前的锁操作已经生效，没有批内回滚。
- `write` 先无返回值地应用全部 DB 条目，再应用锁条目；若锁阶段失败，DB 变更不会撤销。这保持“不能先删锁再提交 DB”的安全顺序，但不提供跨两个内存结构的原子性。扩展调用方不能把它误认为事务性持久化接口。
- `RwLock` 中毒通过 `expect` 触发 panic，错误文本分别是 `DB backend poisoned` 和 `lock backend poisoned`，不转换为 `Result`。
- `delete_range` 的空 end 明确 panic `invalid end key`，与 Go `collectRangeKeys` 一致；它不是“无界上界”的表示。范围采用半开区间 `[start, end)`，空 start 可以覆盖从最小 key 开始的范围。
- `extra_txn_status_key` 会访问 `value[0]`，因此空 key 会 panic；当前函数是私有的，调用它的 `commit`/`rollback` 没有另行校验。Go 实现同样在结果首字节上自增，但安全扩展应由上层保证事务 key 非空或显式定义空 key 行为。
- `extra_txn_status_key` 的首字节使用 `wrapping_add(1)`；`0xff` 会回绕为 `0x00`。这是当前 Rust 事实，测试只覆盖普通 `"key"`，没有证明该边界与 Go 的字节自增完全一致。
- `BATCH_CHANNEL_SIZE` 当前未使用；不能据此宣称 Rust 有 Go 的队列背压、合批或 worker 错误传播。

## 并发与资源生命周期

- `MemoryWriteBackend` 可经 `Arc` 在多线程共享。DB 与锁表各有独立 `RwLock`：同一结构内读写互斥，不同结构间没有共同事务锁。
- `DbWriter::open`/`close` 通过 `AtomicBool` 的 Release 写和 Acquire 读发布状态；`latest_ts` 用 Acquire 读取、AcqRel `fetch_max` 更新。重复 open/close 只是覆盖布尔值，没有 Go worker 的创建、join 或只能关闭一次的通道约束。
- `write` 的开闭检查与后续写入不是一个不可分割操作：另一个线程可在检查后调用 `close`，当前写仍会继续。这与“关闭后台 worker 并等待退出”的 Go 生命周期不同。
- DB 和锁分别加锁，提交期间其他线程可能在 DB 已更新而锁尚未删除的窗口读取状态；该顺序避免无锁但无版本的危险状态，却没有提供完整快照一致性。
- `delete_range` 先收集 key，再分块删除，减少一次持有写锁的时长；收集与每块删除之间允许并发写入，所以新插入范围内的版本可能不在快照 key 列表中。与 Go 版本相比，Rust 没有在每块删除前获取 user-key latch。
- 本文件没有线程、任务、通道和外部文件句柄；资源释放由 `Arc`、`RwLock` 和集合的 RAII 完成。`close` 是逻辑状态切换，不负责释放后端。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/tikv/write.go`：

- Rust `WriteBatch` 对应 Go `writeBatch`；五个事务方法的主要分支一致。普通提交写 `commitTS` 版本，悲观锁提交不写版本，`Op_Lock` 仅为主键写额外状态，之后删锁；回滚写 commitTS=0 的状态记录。
- `encode_user_meta` 对应 `mvcc.NewDBUserMeta`。Rust 测试 `commit_user_meta_uses_go_little_endian_layout` 用固定十六进制时间戳验证两个小端 `u64` 的精确字节序。
- `extra_txn_status_key` 对应 `mvcc.EncodeExtraTxnStatusKey`：降序时间戳等价为 `!start_ts` 的大端字节，首字节加一把辅助状态记录移出普通 user key 排序区。`rollback_key_matches_go_extra_txn_status_encoding` 验证了该布局。
- 锁大小检查对齐 Go 的 `len(entry.Key.UserKey)+len(entry.Value)`，Rust 用 `40 + primary + value + secondaries` 重建 `Lock.MarshalBinary()` 长度；`lock_limit_counts_go_marshaled_header` 防止只计算业务 value 的错误简化。
- 两者都要求 DB 提交先完成，再处理锁批，避免先删除锁造成不一致；Rust 是同步的两个函数调用，Go 是容量 1024 的 DB/lock 通道、两个 worker、批次等待与错误回传。
- Go `dbWriter` 写真实 Badger `DBBundle` 与 `LockStore`，支持 worker 合批；Rust `MemoryWriteBackend` 是进程内 `BTreeMap`/`HashMap`，没有持久化、合批或真实锁存储编码。
- Go `DeleteRange` 使用读事务、DB reader、按 user key 计算 latch hash、每批获取/释放 latch，并通过 DB worker 删除；Rust 只对内存 map 分块加写锁，没有 latch 参数。两者的批大小 4096、空 end panic 和半开范围意图一致。
- Go `updateLatestTS` 是 load 后单次 CAS；Rust `fetch_max` 在并发下更直接保证单调最大值，且两者都忽略最大系统时间戳。

Go 测试通过 `mvcc_test.go::NewTestStore` 把 `NewDBWriter(dbBundle)` 接入真实 Go `MVCCStore`/`Server`，覆盖主链集成；Rust 对应测试目前是 `write_test.rs` 和 `main_test.rs` 的直接组件测试，不能据此宣称已达到同等接线覆盖。

## 扩展指南

- 新增或修改事务操作时，优先修改 `WriteBatch` 中对应方法，并在独立 `write_test.rs` 增加对 `db_entries`/`lock_entries` 最终效果的回归；若影响完整预写/提交行为，还应同步 `mvcc_test.rs`，不要把测试内嵌回生产源文件。
- 改动编码时必须逐字节对照 `mvcc/mvcc.go::{NewDBUserMeta, EncodeExtraTxnStatusKey}`，覆盖 0、`u64::MAX`、高位字节、空 key/首字节 `0xff` 等边界；编码变化会影响版本排序和事务状态查询兼容性。
- 改动锁大小算法时必须同步 Go `Lock.MarshalBinary()` 的固定头和 secondary 长度前缀规则，并保留超限错误测试。遗漏字段可能让内存测试接受真实 lockstore 无法容纳的条目。
- 若接入 Rust 主链，先让具体 writer/batch 实现或适配 `mvcc/db_writer.rs` 的 trait，再决定 `MvccStore` 是继续维护 `StoreState` 还是统一落到后端；同时补齐 context、latch、错误类型和读快照语义。双写两个状态模型会产生可见性分裂，应避免。
- 若补齐 Go worker 语义，需要重新设计 `BATCH_CHANNEL_SIZE`、关闭时排空/拒绝策略、批间错误隔离与等待机制；不能仅增加线程而保持当前非原子的错误模型。
- 优化 `delete_range` 时要保持 `[start, end)`、全版本删除和 4096 分批不变量；若支持并发插入或主链接入，应引入与 user key 对应的 latch 或定义一致性保证，并新增并发回归测试。
- 性能风险集中在 `versions` 的全量 clone、范围删除先收集全部 key、每批反复获取写锁，以及锁大小每次遍历全部 secondaries；在引入优化前应保留字节布局和操作顺序测试。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件已被索引。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/tikv/write.rs --offset 1 --limit 520`：读取目标文件全部 277 行，核对全部常量、类型、impl 和私有编码函数。
- RustCodeGraph `query WriteBatch`、`query MemoryWriteBackend`、`query DbWriter`、`query encode_user_meta`、`query extra_txn_status_key`：确认同名 trait/Go 类型的歧义，以及本文件符号和测试导入位置。
- RustCodeGraph `callees new_write_batch`：确认 `new_write_batch -> update_latest_ts` 及 `WriteBatch` 构造边；`callees commit` 的目标节点确认 `commit -> encode_user_meta/extra_txn_status_key`。
- RustCodeGraph 文件节点：`mvcc/db_writer.rs`、`lib.rs`、`mvcc.rs`、`server.rs`，用于核对 trait 边界、模块导出和当前 Rust 主链没有接入本 writer 的事实。
- 读取 `pkg/store/mockstore/unistore/tikv/Cargo.toml`，核对 crate 名称、lib 入口、porting 元数据和依赖边界。
- 读取 Go 对照 `write.go`、`mvcc/mvcc.go` 及 `mvcc_test.go::NewTestStore`，核对 worker、Badger/lockstore、元数据编码、额外状态 key、范围删除和 Go 主链接线。
- 读取独立 Rust 测试 `write_test.rs`，以及 `main_test.rs` 中 `memory_db_writer_preserves_lock_then_commit_order`、`memory_db_writer_rejects_oversized_lock_entries`，核对现有回归覆盖。
- 使用 `rg` 搜索 `MemoryWriteBackend|DbWriter|new_write_batch|delete_range` 的 Rust 引用，确认生产 Rust 主链无直接使用者；测试文件是当前具体实现的直接调用者。

本任务为纯文档分析，未运行 Cargo 或代码测试。结构验收应确认文件存在，并且上述十一个固定二级标题各出现一次；人工复核重点是当前接线限制、DB 后锁顺序、编码布局、并发差异及扩展测试位置均有源码或对照文件支持。
