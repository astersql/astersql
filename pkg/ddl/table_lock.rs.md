# `pkg/ddl/table_lock.rs`

## 文件定位

`table_lock.rs` 位于 `astersql-ddl` crate，模块由 `pkg/ddl/lib.rs` 的 `pub mod table_lock` 对外公开。它用内存数据结构表达 MySQL 风格 `LOCK TABLES`、`UNLOCK TABLES` 和 `ADMIN CLEANUP TABLE LOCK` 所需的表锁元数据、兼容性检查及逐步状态推进逻辑。

该文件是 Go `pkg/ddl/table_lock.go` 的聚焦移植模型，但当前 Rust 接线范围有限：仓库搜索只找到测试代码直接调用 `check_rename_table_lock`、`check_drop_schema_lock`、`lock_table` 和 `unlock_table`；`advance_lock_tables`、`advance_unlock_tables` 没有文件外调用者。Rust 的 `pkg/ddl/executor.rs::lock_tables` / `unlock_tables` 使用另一套简化的 `TableInfo.table_lock` 与 `SessionContext.locked_tables` 路径，并未调用本文件。因此，本文件当前不是 Rust DDL worker 的实际分派入口，不能把 Go 中 `job_worker.go -> onLockTables/onUnlockTables` 的完整接线视为 Rust 已支持。

从 DDL 机制看，这是 job-based 表锁算法的纯状态部分：它不执行 SQL 解析、job 持久化、owner 调度、schema 同步或元数据 I/O，而是让上层以重复调用的方式推进一个批次。依据为 `advance_lock_tables` / `advance_unlock_tables` 的参数全部是可变内存对象和标量，没有事务、存储或异步运行时依赖。

## 核心职责

- 用 `SessionInfo` 以 `server_id + session_id` 标识锁拥有者；用 `TableLockType` 和 `TableLockState` 表达锁种类及 `None -> PreLock -> Public` 可见性阶段。
- 用 `check_table_locked` 判定新请求与现存锁是否兼容：`Read` 可共享 `Read`，`ReadOnly` 可共享 `ReadOnly`；写锁类不共享；唯一持锁会话可在非 `ReadOnly` 锁上重新申请或切换类型。
- 用 `lock_table` / `unlock_table` 修改单表锁记录，并让重复加锁、无权解锁和清理操作保持幂等或显式返回冲突。
- 用 `advance_lock_tables` 先逐项释放旧锁，再预检整个新锁集合，最后按表、按状态分步推进；用 `advance_unlock_tables` 逐项释放锁并报告批次完成状态。
- 用 `check_rename_table_lock` 和 `check_drop_schema_lock` 提供 RENAME TABLE 与 DROP SCHEMA 的锁前置条件检查。

它不负责把错误转换成 TiDB 错误码、不负责保存 `LockTablesArgs` 进度、不负责等待集群 schema 版本同步，也不负责串行化并发调用；这些能力在 Go 版本由 job context、meta mutation、job 状态和 owner worker 提供，在本 Rust 文件中是调用方责任。

## 主要符号

- `SessionInfo { server_id, session_id }`：会话复合身份。`find_session_info_index` 必须同时匹配两个字段，避免不同实例上的相同连接号被视为同一持锁者。
- `TableLockType::{Read, ReadOnly, Write, WriteLocal}`：四类锁。内部函数 `lock_types_are_shareable` 只认可 `Read/Read` 和 `ReadOnly/ReadOnly` 两种共享组合。
- `TableLockState::{None, PreLock, Public}`：锁元数据的三阶段状态；`Default` 为 `None`。
- `TableLockInfo`：保存锁类型、持锁会话列表、状态和 `timestamp`。共享锁可能有多个会话；冲突错误以列表首个会话作为 owner 信息。
- `LockTableInfo`：本文件使用的最小表视图，包含 schema ID、table ID、错误显示用表名和可选锁信息。它不是 `pkg/ddl/table.rs::TableInfo`。
- `TableLockTarget` 与 `LockTablesArgs`：描述批量目标、请求会话、锁/解锁进度下标和 cleanup 模式。两个下标是可恢复执行所需的显式进度状态。
- `TableLockError`：领域错误枚举，包括表不存在、重命名缺少写锁、持锁时禁止操作、锁冲突和非法状态。
- `check_rename_table_lock`：无锁时放行；有锁时要求状态为 `Public`、请求会话在持锁列表中且锁类型严格为 `Write`。
- `check_drop_schema_lock`：只要请求会话出现在传入任意表的持锁列表中就返回 `LockOrActiveTransaction`，不按 schema ID 过滤。
- `check_table_locked`：执行兼容性预检；它只读元数据，不修改锁。
- `lock_table`：创建初始 `None` 锁记录，或向可共享锁追加尚未存在的会话；遇到 `PreLock` 时幂等返回。
- `unlock_table`：cleanup 模式清空整段锁；普通模式只移除当前会话，最后一个会话离开时删除锁记录；返回是否发生修改。
- `TableLockOutcome { schema_version, finished }`：一次推进后的版本与完成标志。
- `advance_lock_tables` / `advance_unlock_tables`：批处理状态机入口；前者可能失败，后者把缺表或非持锁者视为无需更新。

文件没有 trait、impl、条件编译项、异步函数或模块级常量；公开 API 均为上述数据类型和函数。

## 执行流程

`advance_lock_tables` 一次调用只完成一个有限步骤：

1. 若 `index_of_unlock < unlock_tables.len()`，先定位当前待解锁表。表存在且 `unlock_table` 实际修改锁时，schema 版本加一；缺表或当前会话不持锁时仍推进 `index_of_unlock`。该次返回 `finished: false`，即使这恰好处理了最后一个解锁项，也要由后续调用进入加锁阶段。
2. 所有旧锁处理完后，若 `index_of_lock == 0`，遍历全部 `lock_tables`。目标缺失返回 `TableNotFound`；不兼容返回 `TableLocked`。此预检减少“已经锁了前几张表才发现后续冲突”的部分成功情况，但此前要求释放的旧锁不会回滚，这与 Go 注释描述的 MySQL 行为一致。
3. 若加锁下标已经到末尾，原样返回当前版本并标记完成。
4. 取得当前目标，调用 `lock_table` 创建或复用锁记录。随后按状态推进：`None` 只变为 `PreLock`，下标不前进；`PreLock` 或 `Public` 变为 `Public` 并把 `index_of_lock` 加一。两种分支都写入 `start_timestamp`，schema 版本加一。
5. 当下标等于目标数时返回 `finished: true`；否则调用方必须再次调用。新建的排他锁通常需要两次推进才能完成一张表；已为 `Public` 的共享锁可在一次调用中追加会话并完成该表。

`advance_unlock_tables` 更简单：若尚有目标，最多处理一个；只有锁元数据实际改变才递增版本，但不论缺表、无锁或非持有者都会递增进度下标。返回时以 `index_of_unlock == unlock_tables.len()` 判断完成。

RENAME/DROP 校验不参与上述批处理：`check_rename_table_lock` 是针对单表和单会话的权限检查；`check_drop_schema_lock` 扫描调用方提供的表集合。测试 `table_lock_rename_aster_unit_test.rs` 和 `db_rename_test.rs::test_rename_table_with_locked` 直接组装 `Public` 锁验证这两个入口。

## 数据与状态

核心不变量如下：

- 会话相等性由 `(server_id, session_id)` 二元组定义，而不是单独的连接 ID。
- `LockTableInfo.lock == None` 表示未锁；有值时 `sessions` 在正常路径应至少有一个元素。代码对空列表仍有防御：构造 `TableLocked` 时若没有首元素，会回退使用当前请求会话，但这会掩盖损坏元数据的真实 owner。
- `lock_table` 新建记录时固定 `state = None`、`timestamp = 0`；时间戳和可见状态只能由 `advance_lock_tables` 后续写入。
- 共享锁追加会话前会查重，因此同一会话重复申请不会产生重复项。
- 普通解锁只移除请求会话；cleanup 解锁忽略 owner，直接删除全部锁信息。
- schema 版本只在锁元数据实际写入或状态推进时递增。跳过缺表、未持有锁的解锁不会增加版本，但仍消耗批次进度。
- `schema_id` 存在于表和目标中，但本文件的 `BTreeMap` 只按 `table_id` 索引，且推进函数不验证目标 schema ID 与表记录一致。调用方必须保证 table ID 的作用域和对应关系正确。
- `finished` 表示参数下标已走完，不代表进度已持久化、schema 已全局同步或 job 已写入历史表。

Rust 类型使用拥有所有权的 `String`、`Vec` 和 `BTreeMap`；除了传入的表集合、参数下标及版本计数器外，没有全局状态。

## 依赖与调用关系

直接依赖只有 Rust 标准库 `std::collections::BTreeMap`。`pkg/ddl/Cargo.toml` 声明 crate 名为 `astersql-ddl`、库入口为 `lib.rs`；本文件没有直接使用 Cargo 中的其它 AsterSQL crate 或第三方 crate，因此它的状态模型与生产元数据类型是隔离的。

RustCodeGraph 给出的内部边包括：

- `advance_lock_tables -> unlock_table`
- `advance_lock_tables -> check_table_locked`
- `advance_lock_tables -> lock_table`
- `advance_unlock_tables -> unlock_table`
- `check_table_locked -> find_session_info_index, lock_types_are_shareable`

文件外调用证据来自仓库搜索：

- `pkg/ddl/table_lock_rename_aster_unit_test.rs` 直接测试 rename/drop 校验和单表加解锁。
- `pkg/ddl/db_rename_test.rs::test_rename_table_with_locked` 在重命名场景中使用同一组函数。
- 没有 Rust 生产文件调用 `advance_lock_tables` 或 `advance_unlock_tables`；`pkg/ddl/executor.rs::lock_tables` / `unlock_tables` 是平行实现，不构成本文件调用者。
- Go 主链已接线：`pkg/ddl/job_worker.go` 按 action 调用 `onLockTables` / `onUnlockTables`，后者定义于 `pkg/ddl/table_lock.go` 并通过 meta mutation 更新表信息和 job 进度。

因此安全理解本文件时应区分“算法内部调用图”和“应用生产调用图”：前者完整可见，后者目前缺少 Rust worker 接线。

## 错误处理与边界

- `advance_lock_tables` 是主要可失败入口。目标不存在返回 `TableNotFound(table_id)`；冲突返回带表名、锁类型和 owner 的 `TableLocked`；理论上加锁后锁记录意外缺失时返回 `InvalidState(None)`。
- `check_rename_table_lock` 对非 `Public` 锁返回 `InvalidState`，对其他会话持有的锁返回 `TableLocked`，对本会话的非 `Write` 锁返回 `TableNotLockedForWrite`。`WriteLocal` 也不满足 rename 条件。
- `check_drop_schema_lock` 不区分锁状态和锁类型，只检查当前会话是否出现在任意锁的会话列表中。
- 解锁是宽容路径：缺表、表未锁、会话不在 owner 列表都不报错。只有实际移除会话或 cleanup 清空锁时才报告修改。
- `lock_table` 对 `PreLock` 无条件幂等返回，依赖“DDL owner 串行执行、该预锁属于当前请求”的上层保证；这个假设来自同路径 Go 实现注释，Rust 类型本身没有记录 job 身份来验证它。
- 批量加锁预检发生在旧锁释放之后，失败不会恢复已释放锁。调用方需要按 MySQL 的“先释放再等待/失败”语义处理，而不能假设事务原子回滚。
- Rust 枚举没有实现 `std::error::Error` 或 `Display`，也没有映射到 `infoschema` / `dbterror` 错误码；上层若接入生产链必须补充稳定的错误转换。

## 并发与资源生命周期

本文件没有锁原语、线程、任务、通道、异步代码、数据库事务或 RAII 资源。所有修改都通过独占的 `&mut BTreeMap`、`&mut LockTablesArgs` 和 `&mut i64` 完成，这只保证单次 Rust 调用期间的内存独占，不提供跨请求或跨节点并发控制。

锁生命周期为：创建 `TableLockInfo(None, timestamp=0)`，推进到 `PreLock`，再次推进到 `Public`，最后由一个或多个 owner 解锁；共享锁在最后一个会话移除时销毁记录，cleanup 可提前销毁整个记录。进度生命周期保存在 `index_of_lock` / `index_of_unlock` 中，但目前仅存在内存；崩溃恢复与 owner 切换不由该文件实现。

Go 生产路径依赖 DDL owner 串行执行 lock/unlock job，并在每步通过 `updateVersionAndTableInfo` 持久化元数据、推进 schema 版本。本 Rust 文件若未来接入并行执行器，必须在外层提供等价的 job 串行化、持久化、重试幂等和版本同步；不能仅凭 `&mut` 借用推断分布式安全。

## 与 Go 版本的对应关系

`pkg/ddl/table_lock.go` 是最直接对照：

- Rust `find_session_info_index` 对应 Go `findSessionInfoIndex`，只是用 `Option<usize>` 代替 `-1`。
- Rust `lock_table` / `check_table_locked` / `unlock_table` 分别对应 Go `lockTable` / `checkTableLocked` / `unlockTable`，保留共享读/只读锁、重复加锁、唯一 owner 切换和 cleanup 语义。
- Rust `advance_lock_tables` 合并了 Go `onLockTables` 的算法部分及其对 `unlockTables` 的优先调用；Rust `advance_unlock_tables` 对应 Go `unlockTables` / `onUnlockTables` 的逐项推进部分。
- 两版都先解锁旧表，只在第一次加锁时预检全部目标；都忽略批量解锁中已经被删除的表；都让 `None -> PreLock -> Public` 每步产生 schema 版本变化。
- Go 使用 `model.TableInfo`、`model.LockTablesArgs`、`jobContext`、`model.Job`、meta transaction 和 TiDB 错误体系；Rust使用本文件自定义的最小类型与内存 `BTreeMap`，未实现 job 取消、`FillArgs` 持久化、`FinishTableJob` 或 `updateVersionAndTableInfo`。
- Go 对未知锁状态有 `default` 分支并取消 job；Rust `TableLockState` 是封闭枚举，match 已穷尽，`InvalidState` 主要用于 rename 的非 Public 状态和防御性检查。
- Rust 额外提供 `check_rename_table_lock` / `check_drop_schema_lock` 作为独立校验模型。对应用户可见行为由 Go `pkg/ddl/db_rename_test.go` 覆盖：持写锁者可 rename、读锁者收到“未为写锁”错误、持锁会话不能 drop database。

Go `pkg/ddl/table_modify_test.go::TestConcurrentLockTables` 还证明 Read 可并发成功，而 Write / WriteLocal 的并发请求只能有一个成功。这与 Rust 兼容矩阵一致，但 Rust 当前没有直接覆盖 `advance_*` 的同等独立测试，也没有连接到真实并发 job 环境。

## 扩展指南

- 修改锁兼容矩阵时，应集中调整 `lock_types_are_shareable` 与 `check_table_locked`，并同步检查 `lock_table` 的实际写入规则；两者不一致会造成预检通过、落锁失败或错误共享。
- 修改状态机时，应以 `advance_lock_tables` 为入口，保持“一次调用一个可持久化步骤”、版本只随实际元数据变化递增、完成下标只在 Public 阶段前进等不变量。新增状态还需审视 `check_rename_table_lock` 和错误表示。
- 接入生产 DDL worker 时，不应复用 `pkg/ddl/executor.rs` 当前简化路径后宣称等价；应明确映射到真实 meta/job 类型，补齐参数进度持久化、job cancel/finish、schema version 更新与同步、owner 串行化和错误码转换。
- 若让 `(schema_id, table_id)` 都参与定位，应更换当前仅以 table ID 为键的映射或显式校验 schema ID，以避免错误命中同 ID 对象。
- 若增强损坏元数据检测，可把“有锁但 sessions 为空”从回退 owner 改为显式错误；这属于兼容性变化，需要先与 Go 行为和持久化数据兼容策略对齐。
- 测试必须继续放在独立文件中。优先扩展 `pkg/ddl/table_lock_rename_aster_unit_test.rs` 覆盖 rename/drop；建议新增独立的 `table_lock_test.rs`（并在 `lib.rs` 以 `#[cfg(test)] mod ...` 接入）覆盖 `advance_lock_tables` / `advance_unlock_tables`、共享锁去重、cleanup、缺表、版本增量和每步 `finished`。Go 语义回归应对照 `pkg/ddl/table_modify_test.go`、`pkg/ddl/table_test.go` 与 `pkg/ddl/db_rename_test.go`。
- 性能上，预检是 O(目标表数)，会话查找和移除是 O(单表持锁会话数)，`Vec::remove` 会移动后续元素。通常会话数很小；若未来允许大量共享 owner，再考虑索引结构，同时保持确定性 owner 报错语义。

## 验证依据

- 源文件：`pkg/ddl/table_lock.rs`，逐项核对全部 9 个公开数据类型/枚举、8 个公开函数、1 个私有兼容函数及其分支；文件无 trait、impl、常量或条件编译项。
- crate 与模块边界：`pkg/ddl/Cargo.toml`（`name = "astersql-ddl"`、`[lib] path = "lib.rs"`）及 `pkg/ddl/lib.rs`（`pub mod table_lock` 和独立测试模块声明）。
- RustCodeGraph：`node advance_lock_tables`、`node check_table_locked`、`node advance_unlock_tables` 确认源码与内部调用边；仓库索引可识别目标符号。单独的 callers 查询未给出文件外生产调用者，随后用 `rg` 做全仓补证。
- Rust 调用与测试：`pkg/ddl/table_lock_rename_aster_unit_test.rs`、`pkg/ddl/db_rename_test.rs`；全仓 `rg` 还确认 `advance_lock_tables` / `advance_unlock_tables` 只在定义和文档注释中出现。
- Rust 平行入口：`pkg/ddl/executor.rs::lock_tables`、`pkg/ddl/executor.rs::unlock_tables`，确认当前 executor 未调用本文件状态机。
- Go 对照与主链：`pkg/ddl/table_lock.go`、`pkg/ddl/job_worker.go`；行为测试参考 `pkg/ddl/table_modify_test.go::TestConcurrentLockTables`、`pkg/ddl/table_test.go` 表锁 helper/历史 job 检查以及 `pkg/ddl/db_rename_test.go`。
- DDL 契约背景：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`；仅用作定位，涉及本文件的结论均由源码与测试复核。
- 本任务是纯文档分析，按计划不运行 Cargo；结构校验要求目标文档恰有 11 个固定二级标题。
