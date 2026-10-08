# `pkg/session/sessmgr/mdl.rs`

## 文件定位

本文件属于 `astersql-session-sessmgr` crate（`pkg/session/sessmgr/Cargo.toml`），实现单个真实 SQL 会话在事务期间持有的元数据锁（MDL）可共享状态。`pkg/session/sessmgr/lib.rs` 将私有模块 `mdl` 中的 `TransactionMDL` 公开再导出；因此 Server、session runtime 和跨 keyspace 内部会话协调器只依赖 sessmgr 的公开类型，而不需要借用线程局部的完整会话对象。

它位于两条链路的交界处：事务读取表时由 `pkg/session/runtime/session.rs::mdl_stats_table` 登记表及 schema 版本；DDL schema 检查循环则经 `InfoSchemaCoordinator::CheckOldRunningTxn`，最终由 `pkg/server/server.rs` 或 `pkg/domain/crossks/coordinator.rs::RegisteredMDLSession::remove_lock_ddl_jobs` 调用 `TransactionMDL::check_jobs`，筛掉仍被旧事务阻塞的作业。

## 核心职责

- 以“表 ID → 会话实际使用的 schema 版本”记录当前事务涉及的普通表；该记录只表示事务的 MDL 观察状态，不拥有事务、表元数据或 DDL 作业。
- 在加载最新元数据之前先写入版本 `0`，封住“已经开始访问表、但尚未获得新 schema 版本”的竞态窗口（`begin_table`）；成功后以真实版本替换（`finish_table`），失败或无需继续保护时撤销（`remove_table`）。
- 让 DDL 协调线程在不访问会话内部 `RefCell` 状态的情况下检查候选作业：若作业涉及的任一表被本事务以更低版本访问，`check_jobs` 就从传入集合删除该作业。
- 复刻 Go `variable.RemoveLockDDLJobs` 对 restricted/internal SQL 的旁路语义：`restricted == true` 时不以该会话阻塞 DDL。
- 在事务结束、语句级自动提交清理及会话析构时统一清空持有记录；检查本身只修改候选作业集合，绝不释放事务锁。

## 主要符号

- `pub struct TransactionMDL`：唯一公开类型，`#[derive(Default)]`；默认状态为空版本表且 `restricted == false`。字段均为私有，调用者只能通过方法维护不变量。
- `versions: Mutex<HashMap<i64, i64>>`：键是物理表 ID，值是本事务针对该表已加载的 schema meta version。值 `0` 是加载中的保守栅栏，不是“未持有”。
- `restricted: AtomicBool`：标记 Go `SessionVars.InRestrictedSQL` 对应状态；独立于版本表更新。
- `set_restricted(&self, bool)`：以 `Release` 写入旁路标记；`pkg/session/runtime/control.rs::SetInRestrictedSQL` 同时更新会话状态和此共享副本。
- `begin_table(&self, id)`：用 `entry(id).or_insert(0)` 首次登记表；重复调用不会把已有真实版本降回 `0`。
- `finish_table(&self, id, version)`：无条件插入/覆盖最终版本，用于完成 `begin_table` 开始的加载过程。
- `remove_table(&self, id)`：删除单表记录，覆盖查表失败、表非 public、表 ID 切换及不兼容列变更等回退路径。
- `clear(&self)`：清空本事务的全部表版本；不改变 `restricted`，因此内部会话属性可跨多个事务保持。
- `check_jobs(&self, jobs)`：原地保留不受本事务阻塞的 `JobMDL`。判定式为：存在 `id ∈ job.table_ids`，且 `versions[id] < job.ver` 时删除该作业；未访问的表、相等或更新版本均不阻塞。

## 执行流程

1. `ConcreteSession::mdl_stats_table` 先从事务快照/Domain 找到初始表。局部临时表直接返回，全局临时表设置 `skip_lock`，二者都不会进入本结构的普通 MDL 流程。
2. 对普通表先调用 `begin_table(initial.ID)` 写入 `0`。这使并发 DDL 检查在随后的最新 InfoSchema 查询尚未完成时也会保守地认为事务版本落后。
3. 从 Domain 当前 InfoSchema 重新取表。查找失败或表不是 public 时删除初始记录并返回；若最新表 ID 改变，则先登记新 ID，再移除旧 ID，避免转换中出现无保护窗口。
4. 调用 `finish_table(table.ID, schema.SchemaMetaVersion())` 发布实际版本。后续兼容性检查若发现 public 列 ID 不兼容，会再次 `remove_table` 并记录 metadata error；成功路径把表缓存在会话 `mdl_tables` 中。
5. schema/DDL 检查侧把待检查的 `JobMDL` 集合交给 Server 连接管理器和跨 keyspace 内部会话注册表。每个活跃会话依次执行 `check_jobs`；一次检查只会继续缩小集合，所以最终留下的是未被任何旧事务阻塞、可以推进的作业。
6. 事务提交或回滚由 `finish_transaction_with_retry` 的 `ReleaseMDL` guard 清理；新事务初始化、部分显式控制路径、自动提交语句 guard 以及 `ConcreteSessionInner::drop` 也调用 `clear`。关闭连接会回滚事务并清理 MDL，而仅从 Server 注册表移除连接不会修改仍由外部 `Arc` 持有的事务状态。

## 数据与状态

`versions` 的状态转换是 `不存在 → 0 → schema version → 不存在`。其中 `0` 保证加载窗口安全；`begin_table` 的 `or_insert` 保证重复读取同一表不会破坏已经发布的版本。表 ID 变化时，代码按“先加入新 ID、后删除旧 ID”的顺序保持连续保护。

`check_jobs` 接收 `HashMap<job_id, Arc<JobMDL>>`，只通过 `retain` 改变 map 的成员，不修改共享 `JobMDL`。`JobMDL` 定义在依赖 crate `astersql-infoschema-issyncer-mdldef`（`pkg/infoschema/issyncer/mdldef/mdl.rs`），包含最低版本 `ver` 和相关物理表集合 `table_ids`。一个作业只要与本会话任一旧版本表相交便被删除；没有交集时保留。

`restricted` 与 `versions` 是正交状态。切换为 restricted 不会清空既有记录，只使检查暂时跳过；切回普通会话后既有记录重新参与判定。事务清理反过来只清版本表，不重置 restricted 属性。

## 依赖与调用关系

- crate 边界：`pkg/session/sessmgr/Cargo.toml` 通过 `mdldef-dependency` 路径依赖 `pkg/infoschema/issyncer/mdldef`；`lib.rs::mdldef` 再导出定义，并公开再导出 `TransactionMDL`。
- 写入侧：`pkg/session/runtime/session.rs::mdl_stats_table` 调用 `begin_table`、`finish_table`、`remove_table`；`pkg/session/runtime/control.rs::SetInRestrictedSQL` 调用 `set_restricted`。
- 生命周期侧：`pkg/session/runtime/control.rs::finish_transaction_with_retry`、事务初始化路径，`pkg/session/runtime/dispatch.rs::StatementMDL::drop` 与 `ConcreteSessionInner::drop` 调用 `clear`。
- 普通连接检查侧：`pkg/server/server.rs::CheckOldRunningTxn` 遍历连接，从 connection context 取得 `Arc<TransactionMDL>` 后调用 `check_jobs`。
- 内部会话检查侧：`pkg/domain/crossks/coordinator.rs::RegisteredMDLSession::remove_lock_ddl_jobs` 在两种 `JobMDL` 表示间转换，调用 `check_jobs` 后把过滤结果映射回协调器集合。
- 下游仅使用 Rust 标准库的 `HashMap`、`Arc`、`Mutex`、`AtomicBool`，以及 `JobMDL` 的 `ver/table_ids`；本文件不执行 I/O、不调存储事务，也不记录日志。

RustCodeGraph 将 `mdl.rs` 识别为 12 个符号，并显示该文件被 Server/runtime 等 6 个文件使用；其方法级 `callers/callees` 查询未产生有效边，因此上述精确调用点以路径限定的 `rg` 和相邻源码为直接证据，没有把图中缺失的边解释成“未接线”。

## 错误处理与边界

所有公开方法均返回 `()`；表不存在、表状态非法或元数据不兼容等业务错误由上层 `mdl_stats_table` 处理，本结构只负责及时撤销对应记录。`check_jobs` 的“删除”含义是从“可推进候选”中移除被阻塞作业，不是取消 DDL 作业，更不是释放事务 MDL。

边界判定使用严格小于：`held_version < job.ver` 才阻塞；版本相等满足 DDL 要求。`begin_table` 的 `0` 会阻塞任何正版本作业。没有被当前事务记录的表不参与判定。restricted 会话无条件返回，保持 Go 内部 SQL 不阻塞 DDL 的语义。

所有 `versions.lock()` 都使用 `unwrap()`；若持锁线程 panic 导致 mutex poison，后续调用也会 panic，本文件没有恢复策略。`check_jobs` 不返回或传播应用错误，原子标记也不存在失败分支。

## 并发与资源生命周期

`TransactionMDL` 预期包在 `Arc` 中跨连接 worker 与 schema 检查线程共享。`versions` 的每次读写都由同一 `Mutex` 串行化；`check_jobs` 在完成整个 `retain` 期间持有版本锁，因此看到的是一份一致的表版本快照，也使表状态更新等待本轮检查完成。它不持有调用方 `jobs` 之外的协调器锁，具体遍历锁顺序由 Server/跨 keyspace 上层决定。

`restricted` 使用 `Release` store / `Acquire` load，使检查线程观察到旁路状态前后的发布关系；版本表自身的可见性仍由 mutex 保证。restricted 的检查发生在取得版本锁之前，避免内部会话无意义竞争。

资源释放依赖显式事务 guard 和会话析构：commit/rollback 即使中途返回错误，`ReleaseMDL::drop` 仍清理版本；自动提交非写语句的 `StatementMDL::drop` 在无活跃事务时清理；会话析构先清 MDL 再回滚残留事务。测试还确认 Server unregister 本身不会释放外部仍持有的 MDL，而 connection close/会话 drop 会释放。

## 与 Go 版本的对应关系

最直接的 Go 语义来源是 `pkg/sessionctx/variable/session.go::RemoveLockDDLJobs`：它跳过 `InRestrictedSQL` 会话，锁住事务上下文，遍历 `GetRelatedTableForMDL()`，并在表相交且会话版本小于作业版本时从 `jobs` 删除作业。Go 的 `pkg/server/server.go::CheckOldRunningTxn` 遍历客户端并对每个 SessionVars 调用该函数；Rust 则把最小必要状态抽成可 `Arc` 共享的 `TransactionMDL`，避免 schema 线程借用线程局部会话。

Rust 保留了 restricted 旁路、表交集和严格版本比较，但没有移植 Go 的事务开始时间、连接 ID与节流日志参数；因此本文件不会输出“old running transaction block DDL”日志。Rust 还显式加入 `begin_table(0)`/`finish_table(version)` 两阶段协议，以覆盖读取最新元数据期间的竞态，并由上层在事务边界清理。

`JobMDL` 的 Rust/Go 定义分别位于 `pkg/infoschema/issyncer/mdldef/mdl.rs` 与 `.go`：字段语义一致；Rust 使用 `HashSet<i64>` 和 snake_case 字段，Go 使用 `map[int64]struct{}` 和导出字段。二者都把定义放在独立包/crate 以避免上层循环依赖。

## 扩展指南

- 新增表访问路径时，应复用 `mdl_stats_table` 的两阶段协议：在可能与 DDL 并发的元数据加载前 `begin_table`，成功后 `finish_table`，每个失败/跳过分支对称 `remove_table`。不可只在加载完成后登记，否则会重开竞态窗口。
- 新增事务结束、隐式提交、连接关闭或会话池复用路径时，必须保证最终调用 `clear`；新增 restricted/internal session 入口时必须同步 `set_restricted`。注意两者不能互相替代。
- 若扩充 `check_jobs` 的诊断能力，可在上层传入事务时间/连接标识并保持过滤判定不变；不要在检查函数中清除 `versions`，测试明确要求重复检查持续阻塞。
- 若调整锁或原子顺序，需要保持“检查看到一致版本集合”“新旧表 ID 切换无空窗”和 restricted 旁路三个并发不变量，并评估持锁执行 `jobs.retain` 在大作业集合上的延迟。
- Rust 测试逻辑应继续放在独立测试文件，不内嵌到 `mdl.rs`。优先扩展 `pkg/session/tests/system_session.rs`（真实事务登记/释放、prepared read、版本兼容）、`pkg/server/runtime_test.rs`（连接生命周期与用户/内部事务）或 `pkg/session/runtime/normal_ddl_test.rs`（协调器聚合、restricted 会话）；纯数据结构边界若需新增可放入 sessmgr 同目录独立 `*_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 接入。

兼容风险主要是误放行 DDL 或永久阻塞；性能风险主要来自每个会话对全部候选作业执行“作业 × 相关表”扫描，以及检查期间持有版本 mutex。任何改变都应与 Go 的 `RemoveLockDDLJobs` 过滤语义对照，并覆盖版本相等、无交集、版本 `0`、restricted、重复检查和清理后放行。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/session/sessmgr` 确认目标及相邻文件；`node --file pkg/session/sessmgr/mdl.rs` 读取 44 行完整实现；`query TransactionMDL`、`query check_jobs` 及其余方法确认结构与签名。方法级 `callers/callees` 对精确符号返回空结果，故调用关系另由源码搜索核验。
- 目标与 crate：`pkg/session/sessmgr/mdl.rs`、`pkg/session/sessmgr/lib.rs`、`pkg/session/sessmgr/Cargo.toml`。
- 数据定义与 Go 对照：`pkg/infoschema/issyncer/mdldef/mdl.rs`、`pkg/infoschema/issyncer/mdldef/mdl.go`、`pkg/sessionctx/variable/session.go::RemoveLockDDLJobs`、`pkg/server/server.go::CheckOldRunningTxn`。
- Rust 上游与生命周期：`pkg/session/runtime/session.rs::mdl_stats_table`、`pkg/session/runtime/control.rs::SetInRestrictedSQL`/`finish_transaction_with_retry`、`pkg/session/runtime/dispatch.rs::StatementMDL`/`ConcreteSessionInner::drop`、`pkg/server/server.rs::CheckOldRunningTxn`、`pkg/domain/crossks/coordinator.rs::RegisteredMDLSession`。
- 独立测试：`pkg/session/tests/system_session.rs::crossks_align_infoschema_real_system_table_old_transaction` 与 `crossks_align_infoschema_prepared_read_and_transaction_cleanup`；`pkg/server/runtime_test.rs::normal_ddl_plan_user_mdl_real_driver_and_domain_lifecycle`；`pkg/session/runtime/normal_ddl_test.rs::crossks_align_normal_ddl_coordinator_fences_user_and_internal_transactions` 及 `normal_ddl_plan_user_mdl_real_internal_pool_preserves_go_restricted_bypass`。这些测试分别覆盖真实表读取、prepared 路径、提交/回滚/关闭释放、用户与内部事务聚合、重复检查不释放和 restricted 旁路。
- 按任务约束未运行 Cargo；本次只新增说明文档，验证采用固定章节结构检查与人工事实复核。
