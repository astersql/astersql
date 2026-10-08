# [`pkg/store/mockstore/unistore/tikv/mvcc.rs`](./mvcc.rs)

## 文件定位

该文件是 `astersql-store-mockstore-unistore-tikv` crate 中的内存 MVCC 核心。模块由同目录 `lib.rs` 以 `pub mod mvcc` 暴露，`server.rs` 的 `Server` 持有 `Arc<MvccStore>`，把 KV RPC 风格的读写、事务状态、锁解析、GC 和调试查询转发到这里。它服务于进程内 mock TiKV，不是持久化 TiKV 引擎。

crate 边界由 `pkg/store/mockstore/unistore/tikv/Cargo.toml` 定义：库入口是 `lib.rs`，移植元数据指向 Go 包 `pkg/store/mockstore/unistore/tikv`。当前顶层 crate 的直接通用依赖只有启用 failpoint feature 的 `fail`；Windows 目标还声明 client、cophandler、lockstore、独立 `tikv/mvcc` 子 crate等依赖。本文对象是顶层 `tikv/mvcc.rs`，不要与 `pkg/store/mockstore/unistore/tikv/mvcc/mvcc.rs` 混淆。

## 核心职责

- 用 `MvccStore` 管理按原始键排序的 `StoreState`，每个 `KeyState` 同时保存当前锁、按 `commit_ts` 排序的写历史和按 `start_ts` 保存的长值。
- 实现乐观/悲观事务主路径：`pessimistic_lock`、`prewrite`/`flush`、`commit`、`rollback`、`cleanup` 和 `resolve_lock`。
- 实现事务辅助协议：`txn_heartbeat`、`check_txn_status`、`check_secondary_locks`、锁扫描和读缓冲。
- 实现快照点查、批量点查、正反向范围扫描及读锁校验，并以 `MvccError` 表达锁冲突、写冲突、时间戳过期等协议错误。
- 维护 `latest_ts`、GC safe point 和关闭标志，并提供调试用完整 MVCC 查询与范围删除。

它刻意把 Go 版的 Badger、lockstore、WriteBatch、Region request context 和后台任务压缩为单进程内存模型，但保留了主要事务语义与回归断言；因此它适合作为 mock 行为实现，而不能替代持久化、崩溃恢复或真实分布式并发测试。

## 主要符号

- 常量：`MAX_SYSTEM_TS` 是读锁判断中的“无穷大”时间戳；`SHORT_VALUE_MAX_LEN` 为 64，区分短值和 `defaults` 长值路径。
- 请求/操作模型：`MutationOp`、`Mutation`、`PrewriteRequest`、`PessimisticLockRequest` 描述事务操作；`PrewriteResult` 和 `PessimisticLockResult` 返回 1PC/min-commit-ts、旧值与存在性。
- 存储模型：`Lock` 保存 primary、`start_ts`、TTL、操作、值、`for_update_ts`、`min_commit_ts`、async-commit 次键等；`Write` 用 `WriteKind` 表示 Put/Delete/Lock/Rollback；`KeyState` 聚合单键三类状态，私有 `StoreState` 聚合所有键。
- 状态/查询模型：`Action`、`TxnStatus`、`SecondaryLocksStatus`、`LockPair`、`KvPair` 和 `MvccInfo` 分别承载事务状态动作、次键状态、锁读结果、KV 结果和调试视图。
- 错误：`MvccError` 覆盖死锁、锁冲突、写冲突、已存在、主键不匹配、悲观锁/事务不存在、已提交、提交时间戳过期、非法请求和 GC-too-early。当前文件会产生其中除 `Deadlock`、`InvalidRequest` 外的大部分变体；这两个变体主要是接口/转换面的预留。
- 生命周期状态：`SafePoint::{new,update_ts,ts,take_changed}` 使用原子值记录 GC 水位与变更位；`MvccStore::{new,close,is_closed,latest_ts}` 管理共享存储及观测状态。
- 私有算法：`check_prewrite` 做预写预检；`apply_committed_mutation` 实现 1PC；`commit_lock` 把锁转换为写；`latest_non_rollback_write`、`visible_value`、`check_read_lock` 和 `physical_ts` 支撑冲突、可见性、锁判断与 TTL 计算。

## 执行流程

1. `Server` 构造时接收一个 `Arc<MvccStore>`；写 RPC 通过 `Server::with_latches` 先按键取得 Region latch，再调用本文件的方法，最后把 `MvccError` 转成 RPC 响应。读 RPC 直接调用 `get`、`batch_get` 或 `scan`。
2. 悲观加锁先克隆并按键排序 mutation，在持有全局写锁时完成所有锁冲突、最新非回滚写冲突、旧值/存在性收集；全批通过后才安装 `PessimisticLock`。这一“先预检再落锁”保证后序键冲突不会留下前序部分锁。
3. `prewrite` 把悲观 mutation 排在前面、其余按键排序，并计算 `max(request.min_commit_ts, start_ts+1, for_update_ts+1)`。`check_prewrite` 校验已有锁、悲观锁身份、`start_ts` 之后的写冲突，以及 Insert/CheckNotExists 的存在性。
4. 若启用 1PC 且 `max_commit_ts` 足够，预检后由 `apply_committed_mutation` 直接生成提交写并清锁；否则长值先记录到 `defaults[start_ts]`，再安装含事务元数据的锁。`CheckNotExists` 只断言不存在，不创建锁。
5. `commit` 先检查整批键：锁必须匹配 `start_ts` 且 `commit_ts >= min_commit_ts`，无锁时只接受已经存在的同事务非回滚写作为幂等成功。全部通过后第二遍调用 `commit_lock`，所以批次不会部分提交。`rollback` 同样先扫描任意已提交写，再清锁/长值并幂等写入 rollback 记录。
6. `check_txn_status` 先返回既有提交/回滚结果；匹配锁若主键不一致则报错，async-commit 锁保持不动，普通锁按 HLC 物理时间判断 TTL 过期并回滚，否则尝试推高 `min_commit_ts`。没有写或锁时，根据 `rollback_if_not_exist` 写入防迟到提交的 rollback 记录或返回 `TxnNotFound`。
7. `get` 先拒绝早于 safe point 的版本，再经 `check_read_lock` 检查写锁，最后用 `visible_value` 从不晚于版本的写历史逆序寻找首个 Put/Delete。`batch_get` 把单键错误装入 `KvPair`；`scan` 对有序键区间执行同样逻辑，支持空 end、反向、limit 和 key-only。
8. `resolve_lock` 先在读锁下收集指定 `start_ts` 的全部键，释放读锁后根据 `commit_ts == 0` 调用批量回滚或提交。`cleanup` 复用 `check_txn_status`。
9. `update_safe_point` 更新原子水位；`gc` 在写锁下为每个键保留 safe point 前最后一个非回滚锚点及所有不早于水位的写，再只保留仍被现存写引用的 default 值。

## 数据与状态

`StoreState.keys`、`KeyState.writes` 和 `KeyState.defaults` 都是 `BTreeMap`：键、提交时间戳和开始时间戳具有确定顺序，范围扫描、逆序可见性查询和 GC 锚点查找都依赖这一性质。每个原始键最多有一个当前 `Lock`，但可以有多个 `Write`；rollback 以 `commit_ts == start_ts` 的写记录占位，阻止迟到提交。

Put/Insert 的值最终存入 `Write.value`。超过 64 字节时，普通 2PC 预写还会在 `defaults[start_ts]` 保存副本，`commit_lock` 优先从该映射取回；提交后该 default 会一直保留到 GC 根据存活写的 `start_ts` 清理。1PC 直接把值放进 Write，不经过 defaults。Delete 写使 `visible_value` 立即返回不存在；Lock/Rollback 写会被跳过并继续查找更旧版本。

`latest_ts` 只由悲观加锁的 `for_update_ts`、普通预写的 `start_ts`、1PC/2PC 的提交时间戳原子推高。回滚和只读操作不更新它。`closed` 只供生命周期观测：`close` 设置标志，当前各读写方法并不会因关闭而拒绝请求。

## 依赖与调用关系

直接上游是 `pkg/store/mockstore/unistore/tikv/server.rs`：`kv_get`/`kv_scan`/`kv_batch_get` 对应读路径；`kv_pessimistic_lock`、`kv_prewrite`、`kv_commit`、`kv_rollback` 等经 `with_latches` 进入写路径；`kv_gc` 连续调用 `update_safe_point` 与 `gc`；调试 RPC 调用 `mvcc_get_by_key`/`mvcc_get_by_start_ts`。`Server::stop` 调用 `MvccStore::close`。`pkg/store/mockstore/unistore/rpc.rs` 还通过 `Server::mvcc_store` 暴露句柄并在调试/扫描请求中使用它。

文件内部的关键调用边为：`flush -> prewrite`，`batch_get -> get -> check_read_lock/visible_value`，`scan -> check_read_lock/visible_value`，`cleanup -> check_txn_status`，`resolve_lock -> commit|rollback`，`prewrite -> check_prewrite -> visible_value`，以及 `commit -> commit_lock`。这些边与 `server.rs` 直接调用点共同弥补 RustCodeGraph 对 impl 方法调用边未解析出的限制。

下游只使用标准库集合、`Arc`、`RwLock` 和原子类型；错误没有依赖外部错误框架。服务器外层的 Region latch 提供同键 RPC 写互斥，本文件自己的 `RwLock<StoreState>` 则提供实例内的最终内存同步。

## 错误处理与边界

- `pessimistic_lock` 和 `prewrite` 在更晚提交写、其他事务锁或缺失的声明悲观锁上失败；Insert/CheckNotExists 在 `start_ts` 可见值存在时返回 `AlreadyExists`。
- `commit` 拒绝早于锁上 `min_commit_ts` 的提交；同事务已提交可幂等成功，rollback 或完全缺失则为 `TxnNotFound`。`rollback` 遇到任一已提交键返回 `AlreadyCommitted`，且因预检不会污染前序键。
- 读取 `version < safe_point` 时，点查返回 `GcTooEarly`；扫描返回只含该错误的一个 `KvPair`。等于 safe point 的读允许执行。
- `check_read_lock` 忽略未来锁、resolved 锁、非 Put/Delete 锁，以及 `MAX_SYSTEM_TS` 下非 async-commit 主键读；committed 列表中的锁作为 `LockPair` 返回，其他阻塞写锁报 `KeyLocked`。
- 范围均采用 `BTreeMap` 半开区间；`scan` 的空 end 表示正无穷，`check_range_lock`、`scan_lock` 和 `delete_file_in_range` 没有对应的空-end 特判，调用者需传入有效上界。
- 所有 `RwLock` 获取都以 `expect("MVCC store lock poisoned")` 处理中毒，发生持锁 panic 后会再次 panic，而非转换成 `MvccError`。时间戳加一和 TTL 运算使用饱和算术；`physical_ts` 固定右移 18 位。

## 并发与资源生命周期

`MvccStore` 通常以 `Arc` 被 `Server` 和测试共享。所有键状态共用一个 `RwLock`：点查、扫描和调试查询可并发持读锁，任何键的预写、提交、回滚或 GC 都取得全局写锁，因此实现优先保证一致性与简单性，而非真实 TiKV 的分片吞吐。批量 mutation 排序用于稳定加锁/处理顺序；事务批次原子性来自“持有同一写锁 + 全批预检 + 第二遍应用”。

`latest_ts`、safe point、变更位和关闭位使用 Acquire/Release 或 AcqRel 顺序。`SafePoint::take_changed` 原子读取并清除通知位，但当前目标模块没有调用它。与 Go 版不同，Rust `SafePoint::update_ts` 是无条件 `store`，不会保证单调递增；`gc` 也是显式同步调用，没有 Go 的 PD safe-point 后台轮询或 Badger compaction filter。

`resolve_lock` 特意先收集键再释放读锁，避免在持读锁时调用需要写锁的 `commit`/`rollback`。没有后台线程、channel、I/O 句柄或析构逻辑；`close` 仅置位，真正 Server 停止还由 `server.rs` 关闭 RegionManager 和 InnerServer。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/tikv/mvcc.go`。两版共享 MVCCStore 概念及 PessimisticLock、Prewrite、Commit、Rollback、CheckTxnStatus、CheckSecondaryLocks、Cleanup、ScanLock、ResolveLock、Get/BatchGet/Scan、UpdateSafePoint、MvccGetByKey/StartTs 和 DeleteFileInRange 等协议意图。Rust 测试中的注释明确以 Go 的 WriteBatch、CheckNotExists、主键 MAX_SYSTEM_TS 读和 async-commit 分支为对齐依据。

实现载体并不等价：Go 版把已提交版本放在 Badger，把锁放在 lockstore，通过 dbWriter/WriteBatch 持久化，并结合 requestCtx、Region latch、lock waiter、deadlock detector、PD safe point 更新循环与 compaction filter；Rust 顶层文件把三类状态全部存入单个内存 `BTreeMap`，协议上下文被压缩为方法参数，等待/唤醒、死锁探测、磁盘错误、Region 边界和崩溃恢复由其他模块承担或尚未模拟。

可见的语义差异还包括：Go safe point 只递增，Rust 可被降低；Go 扫描支持 isolation level、committed locks、sample step、commit-ts 返回和内部键边界，Rust 只实现 resolved locks、方向/limit/key-only；Go `DeleteFileInRange` 删除 Badger 的数据/default 两个编码区间，Rust 直接移除内存键状态；Go GC 依靠 Badger 压缩过滤，Rust `gc` 立即遍历并保留一个可见锚点。扩展时必须先判断需求是补 mock 协议语义，还是不应在此内存模型复制的真实存储能力。

## 扩展指南

- 新增事务操作时，先扩展 `MutationOp`，再同步检查 `check_prewrite`、普通锁构造、`apply_committed_mutation`、`commit_lock`、`visible_value` 和 `check_read_lock` 是否需要识别该操作；同时在独立的 `mvcc_test.rs` 增加行为测试，不要把测试放入生产文件。
- 修改批量写必须保留全批预检后应用的不变量，并覆盖“后序键失败时前序键无副作用”。避免为了简化测试把逻辑改为逐键提交。
- 修改可见性或锁规则时，同步核对 `get`、`batch_get`、`scan`、`check_keys_lock` 和 `check_range_lock`，并与 Go 的 `checkLock`、`GetPair`、`BatchGet`、`collectRangeLock`/`Scan` 对照。
- 修改时间戳/GC 时重点验证 safe point 单调性、等于水位的读、锚点版本、Delete/Lock/Rollback 与 default 值清理。若要对齐 Go，应优先修正 `SafePoint::update_ts` 的单调递增契约，而不是加入与 Badger 无关的表面 API。
- 增加并发能力前应明确锁粒度和 Server Region latch 的组合顺序，防止锁序反转；若拆分全局 `RwLock`，批量原子性和跨键 `resolve_lock` 需要新的事务化机制。
- RPC 形态或返回字段改变时同步更新 `server.rs` 的转换/接线以及 `rpc.rs` 调用面；持久化、lock waiter、deadlock detector 或 Region 语义应优先落在对应现有模块，而不是塞入此单文件内存状态机。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/unistore/tikv/mvcc.rs` 将目标识别为含 93 个符号的 Rust 文件。
- RustCodeGraph `node --file ... --offset ...` 分段读取了 `mvcc.rs` 全部 1,187 行、`mvcc_test.rs` 的有效测试区（900-1275）、`server.rs` 的 Server/MVCC 接线区，以及 Go `mvcc.go` 的存储、事务、读扫描和 safe-point/GC 相关区段。
- 对 `MvccStore::prewrite` 等 impl 方法执行精确 `callers`/`callees` 未返回边，宽泛 `explore` 又被仓库同名符号污染；因此调用关系另由 `rg` 的精确方法调用点与 `server.rs` 源码交叉核实。确认的直接边包括 `Server::kv_prewrite -> MvccStore::prewrite`、`Server::kv_commit -> commit`、`Server::kv_gc -> update_safe_point/gc`。
- crate/模块证据：`pkg/store/mockstore/unistore/tikv/Cargo.toml` 与 `pkg/store/mockstore/unistore/tikv/lib.rs`。Go 对照：`pkg/store/mockstore/unistore/tikv/mvcc.go`。
- 独立 Rust 测试 `pkg/store/mockstore/unistore/tikv/mvcc_test.rs` 覆盖快照可见性、回滚幂等、1PC、事务状态/TTL、CheckNotExists、MAX_SYSTEM_TS 主键读、悲观锁读与批次原子性、async commit、主键不匹配、长值/Delete、心跳、扫描、safe-point 拒绝、GC 锚点、ResolveLock，以及 Commit/Rollback 后序失败不产生部分修改。Go 回归背景位于同路径 `mvcc_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构命令验证目标文档存在且固定二级标题恰好为 11 个，并人工复核本文能够回答文件为何存在、运行主链和安全扩展位置。
