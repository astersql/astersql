# `pkg/lock/lock.rs`

## 文件定位

`pkg/lock/lock.rs` 是 `astersql-lock` crate 的表锁访问检查实现。crate 入口 `pkg/lock/lib.rs` 通过 `mod lock; pub use lock::*;` 再导出本文件的公开项；`pkg/lock/Cargo.toml` 将库入口指定为 `lib.rs`，并声明该 crate 对应 Go 包 `pkg/lock`。

本文件位于 SQL 规划完成、真正执行 DDL/DML 之前的访问判定边界：它把“当前会话持有哪些表锁”与“当前 InfoSchema 快照里目标表被怎样锁定”合并，判断某种 `mysql::PrivilegeType` 操作是否允许。需要注意的是，截至本次核验，工作区只有根 `Cargo.toml` 的 `facade_lock` workspace 别名声明，没有其他 Cargo manifest 依赖 `astersql-lock`；RustCodeGraph 也没有找到 Rust `NewChecker` 的外部调用者。因此这里是已经实现并由局部单测覆盖的 Rust 移植模块，但尚无证据表明它已接入 Rust 生产请求主链。当前可验证的在线调用链仍在 Go：`pkg/planner/core/optimizer.go::CheckTableLock` 构造 `lock.NewChecker` 并逐项检查 planner 的 `visitInfo`。

## 核心职责

- `Checker::CheckTableLock` 对单个数据库/表/权限组合执行完整表锁访问规则，包括系统库绕过、库级操作转发、会话自持锁、其他会话持锁以及只读锁的特殊规则。
- `Checker::CheckLockInDB` 处理没有具体表名的库级操作：先限制持锁会话的库级 DDL，再遍历带表锁属性的表并复用单表检查。
- `checkLockTpMeetPrivilege` 实现“本会话已经锁住目标表”时的最小权限矩阵：`Write`/`WriteLocal` 放行所有已进入该分支的权限，`Read` 只放行 `SelectPriv`。
- `ERR_TABLE_*` 四个惰性错误原型在 InfoSchema Rust 接口尚未导出包级错误原型时，按相同 `ClassSchema` 错误码在本 crate 内补建，以维持 Go 错误码、RFC 标识、脱敏位置和格式化消息契约。
- `ErrLockedTableDropped` 标识本会话删除自己持有写锁的表这一特殊控制流。Go planner 收到同名错误后会停止检查后续 `visitInfo`，让删除锁表后的其他表访问继续进行；Rust 侧尚未发现对应消费者。

## 主要符号

- `type StandardError = Box<dbterror::terror::Error>`：四个标准 schema 错误原型的装箱别名。
- `modelLockType(TableLockType) -> ModelTableLockType`：按内部数值把 AST 锁类型转换成模型层锁类型，供 `meta.Lock.Tp` 比较。它不做合法性校验。
- `ERR_TABLE_NOT_LOCKED_FOR_WRITE`、`ERR_TABLE_NOT_LOCKED`、`ERR_TABLE_NOT_EXISTS`、`ERR_TABLE_LOCKED`：使用 `std::sync::LazyLock` 延迟构造的 `ClassSchema` 标准错误。`ERR_TABLE_NOT_EXISTS` 目前只用于声明对齐，函数体实际按返回错误的字符串 code 判断缺表。
- `Checker<'a> { ctx, is }`：只借用 `dyn TableLockReadContext` 和 `dyn InfoSchema`，自身不拥有、修改或同步会话锁与 schema 状态。
- `ErrLockedTableDropped: LazyLock<infoschema::Error>`：消息为 `other table can be accessed after locked table dropped` 的特殊错误值。
- `NewChecker(ctx, is) -> Checker`：保存两项只读借用，不执行查询或预计算。
- `Checker::CheckTableLock(db, table_name, privilege, alter_writeable)`：单目标检查的公开入口，返回 `Result<(), infoschema::Error>`。
- `Checker::CheckLockInDB(db, privilege)`：库级检查公开入口。
- `checkLockTpMeetPrivilege(lock_type, privilege) -> bool`：crate 内部权限矩阵函数；独立测试位于 `pkg/lock/lock_aster_unit_test.rs`。

## 执行流程

`CheckTableLock` 按以下顺序执行，顺序本身属于兼容行为：

1. 当数据库名和表名同时为空，或权限是 `LockTablesPriv` 时直接成功。
2. `metadef::IsMemOrSysDB(db)` 命中的系统库或内存库不支持表锁，直接成功。
3. `table_name` 为空且 `alter_writeable == false` 时转入 `CheckLockInDB`。`alter_writeable` 为真时不会走库级分支。
4. `ShowDBPriv` 和 `AllPrivMask` 直接成功；后者当前仅用于 `SHOW CREATE TABLE`。`CreatePriv`/`CreateViewPriv` 在会话已持任意表锁时返回 `ErrTableNotLocked`，否则成功。这个分支刻意先报锁错误，与 MySQL 可能先检查目标是否存在不同。
5. 通过 `InfoSchema::ModelTableInfoByName` 获取表元数据。错误 code 为 `ErrNoSuchTable` 或 `ErrTableNotExists` 时按 `DROP TABLE IF EXISTS` 语义忽略；其他错误转换为共享 `infoschema::Error` 返回。表元数据没有 `Lock` 时直接成功。
6. 对 `DropPriv`，若元数据保留的原始表名与传入表名相同且当前会话持有锁，则扫描 `GetAllTableLocks()` 找相同表 ID：自持 `Write` 锁返回 `ErrLockedTableDropped`；自持 `Read`、`WriteLocal` 或 `ReadOnly` 返回 `ErrTableNotLockedForWrite`。
7. 当 `alter_writeable == false` 且会话持有任意表锁时，只允许访问会话锁集合内的目标表。`CheckTableLocked(meta.ID)` 命中后交给 `checkLockTpMeetPrivilege`；权限不匹配返回 `ErrTableNotLockedForWrite`，目标不在本会话锁集合则返回 `ErrTableNotLocked`。
8. 对其他会话留下的表锁元数据，`SelectPriv` 可穿过 `Read`、`WriteLocal`、`ReadOnly`；`alter_writeable` 可穿过 `ReadOnly`。其余情况使用小写表名、锁类型和第一个锁会话格式化 `ErrTableLocked`。

`CheckLockInDB` 先在本会话持锁时拒绝 `CreatePriv`、`DropPriv`、`AlterPriv`，返回 `table::ErrLockOrActiveTransaction`。没有触发该限制时，`CreatePriv` 直接成功；其他权限调用 `InfoSchema::ListTablesWithSpecialAttribute(TableLockAttribute)`，遍历所有返回的 `TableInfos`，以传入数据库名和每张表的小写名调用 `CheckTableLock(..., false)`，遇到首个错误立即返回。

## 数据与状态

`Checker` 的状态只有两个生命周期受 `'a` 约束的共享引用。`ctx` 提供当前会话锁映射的只读快照式接口：`HasLockedTables`、`GetAllTableLocks` 和 `CheckTableLocked`；`is` 提供表元数据查询以及按特殊属性枚举表。检查器不缓存查询结果，因此每次调用都观察这些接口当时返回的状态。

锁判断同时依赖两类数据：会话侧 `TableLockTpInfo.TableID`/锁类型，以及模型侧 `TableInfo.ID`、`Name`、可选 `Lock`。`Lock.Sessions` 是生成最终 `ErrTableLocked` 消息的所有者信息。名称比较有意区分 `Name.O`（原始拼写，用于 DROP 的精确分支和错误参数）与 `Name.L`（小写形式，用于通用锁定错误和库级递归检查）。

四个标准错误和 `ErrLockedTableDropped` 使用进程级 `LazyLock`，首次访问时构造，之后复用。除惰性初始化外，本文件没有可变全局状态。

## 依赖与调用关系

下游依赖由 `pkg/lock/Cargo.toml` 明确：

- `astersql-lock-context`：提供 AST/模型锁类型及 `TableLockReadContext`。
- `astersql-infoschema` 与 `astersql-infoschema-context`：提供 InfoSchema 查询、表锁属性枚举和共享错误类型。
- `astersql-meta-metadef`：识别系统库与内存库。
- `astersql-parser-mysql`：提供权限常量。
- `astersql-table`：提供持锁/活动事务下禁止库级 DDL 的错误。
- `astersql-util-dbterror`：按 TiDB 标准错误码构建 schema 类错误。

RustCodeGraph 对本文件确认的内部调用边为：`CheckTableLock -> modelLockType`、`CheckTableLock -> CheckLockInDB`、`CheckTableLock -> checkLockTpMeetPrivilege`，以及 `CheckLockInDB -> CheckTableLock`。后两者构成受参数约束的回边：只有单表入口遇到空表名才转库级检查；库级枚举回调总是传非空表名，因此不会无限递归。

上游方面，`pkg/lock/lib.rs` 再导出全部公开项，但没有发现 Rust 外部调用者。Go 对照主链为 `pkg/planner/optimize.go::OptimizeForForeignKeyCascade`（以及正常优化路径）调用 `pkg/planner/core/optimizer.go::CheckTableLock`，后者在 `config.TableLockEnabled()` 为真时构造 checker，并依次传入 planner 收集的数据库名、表名、权限和 `alterWritable`。

## 错误处理与边界

- 早返回是规则的一部分：空目标、`LockTablesPriv`、系统/内存库、SHOW 类权限以及无锁元数据均不继续查询或报锁错误。
- 缺表错误仅通过字符串 code `ErrNoSuchTable`/`ErrTableNotExists` 被忽略；其他 InfoSchema 错误原样转换并传播。新增 InfoSchema 缺表变体时必须同步这里，否则 `IF EXISTS` 行为可能漂移。
- `GenWithStackByArgs` 保留标准错误类别并附带参数；调用方应比较错误类别/错误值，而不是依赖完整字符串。
- 最终 `ErrTableLocked` 分支对 `lock.Sessions.first()` 使用 `expect("a locked table must record at least one session")`。因此不变量是：只要 `TableInfo.Lock` 存在且走到拒绝分支，`Sessions` 必须非空；损坏或不完整元数据会触发 panic，而不是返回可恢复错误。
- `modelLockType` 只复制数值，新锁类型若在 AST 与模型层编码不一致会导致静默误判。
- `alter_writeable` 不是“忽略所有锁”：它跳过本会话锁集合限制，并只对 `ReadOnly` 元数据放行；其他锁仍返回 `ErrTableLocked`。
- `CheckLockInDB` 遍历 InfoSchema 返回的全部带锁属性表，而非先按 schema 结果过滤；它将调用者传入的 `db` 与枚举到的表名组合后查询。这与 Go 实现一致，扩展时不应未经验证改变遍历语义。

## 并发与资源生命周期

本文件不创建线程、任务、通道或事务，也不获取互斥锁。并发一致性依赖调用者提供的 `InfoSchema` 快照和 `TableLockReadContext` 实现：`Checker` 只借用它们，生命周期保证 checker 不会比依赖对象活得更久，但 trait 本身没有在本文件声明跨线程同步保证。

一次检查中会多次调用 `HasLockedTables`，并可能随后读取全部锁或单表锁；本文件没有把这些读取包在同一临界区。因此实现方若允许并发修改，必须自行保证这些读取之间的语义一致性。InfoSchema 查询也没有缓存；库级检查的成本至少随带锁属性的表数量线性增长，并对每张表再次执行名称查询。

`LazyLock` 保证错误原型的线程安全一次性初始化。返回错误通过克隆或共享转换离开函数，不借用局部元数据；`Checker` 析构时仅释放两个共享引用，不负责释放会话锁。

## 与 Go 版本的对应关系

Rust 文件逐分支对应 `pkg/lock/lock.go`：`Checker` 字段、`ErrLockedTableDropped`、`NewChecker`、`CheckTableLock`、`checkLockTpMeetPrivilege` 和 `CheckLockInDB` 均有同名 Go 符号，主判断顺序与错误类别保持一致。

可见的实现层差异包括：Go 用 `InfoSchema.TableByName(context.Background(), ...)` 得到 table 对象后读取 `Meta()`，Rust 直接调用 `ModelTableInfoByName`；Go 用 `infoschema.ErrTableNotExists.Equal(err)`，Rust 暂以两个字符串 code 匹配缺表；Go 直接使用 InfoSchema 包级错误原型，Rust 因公开接口缺失而在本文件重建四个 `ClassSchema` 原型；Go 用 `Lock.Sessions[0]`，Rust 用带断言消息的 `first().expect(...)` 显式表达同一非空不变量。

`pkg/lock/lock_aster_unit_test.rs` 只直接覆盖 `checkLockTpMeetPrivilege`：写锁/本地写锁放行常见表级权限，读锁只允许 SELECT，`None`/`ReadLocal`/`ReadOnly`/未知类型不在该辅助函数中放行。端到端 Go 证据主要在 `pkg/ddl/db_table_test.go::TestLockTables` 与 `TestWriteLocal`，覆盖自持读/写/本地写锁、其他会话访问、创建表/视图、库级 DDL 和清理锁；`pkg/ddl/table_modify_test.go::TestLockTableReadOnly` 覆盖 `ReadOnly` 的读放行、写拒绝及 ALTER 特例。Rust 当前没有针对完整 `Checker` 的独立测试，也没有生产上游接线证据。

## 扩展指南

- 新增锁类型时，先核对 AST `TableLockType` 与模型 `ModelTableLockType` 的编码，再同步 `modelLockType` 的假设、DROP 分支、其他会话 SELECT 分支、`alter_writeable` 分支和 `checkLockTpMeetPrivilege`。至少扩充 `pkg/lock/lock_aster_unit_test.rs` 的权限矩阵；不要把测试写进生产文件。
- 新增或调整权限时，保持 `CheckTableLock` 的早返回顺序，特别是 SHOW/CREATE、缺表和自持锁分支。同步 Go `pkg/lock/lock.go`，并在独立 Rust 测试中覆盖允许与拒绝两侧。
- 若 InfoSchema 导出标准错误原型，应以等价错误码、RFC、脱敏和参数格式替换本地 `ERR_TABLE_*`，并补错误类别断言，避免只比较消息文本。
- 若要把 Rust 实现接入生产链，应在实际 planner/session 的 Cargo crate 中显式依赖 `astersql-lock`，将 planner 访问项转换为本入口参数，并移植 Go `pkg/planner/core/optimizer.go::CheckTableLock` 对 `ErrLockedTableDropped` 的特殊 break 语义。接线前不能仅凭 workspace alias 认为模块已启用。
- 若消除空 `Sessions` panic，必须先确定损坏锁元数据的兼容错误类别，并同时更新 Go/Rust 语义；简单返回任意错误会改变错误码契约。
- 性能修改应重点度量 `ModelTableInfoByName` 的单次查询，以及 `CheckLockInDB` 的“枚举后逐表再查”路径；源码注释明确要求当前保持与 Go 一致，不能只为减少查询改变可见行为。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/lock` 找到 `lock.rs`、`lock.go`、`lib.rs`、独立 Rust 测试和 context 文件。
- RustCodeGraph 精确符号：`pkg/lock/lock.rs:72:function:NewChecker`、`:84:function:CheckTableLock`、`:204:function:CheckLockInDB`、`:236:function:checkLockTpMeetPrivilege`；图中确认本文件的内部调用边和 `NewChecker` 无外部 Rust caller。
- 生产实现：`pkg/lock/lock.rs`；crate 边界与依赖：`pkg/lock/lib.rs`、`pkg/lock/Cargo.toml`、根 `Cargo.toml` 的 `facade_lock` 声明。
- 会话锁接口：`pkg/lock/context/lockcontext.rs::TableLockReadContext`。
- Go 语义对照：`pkg/lock/lock.go`；Go 生产上游：`pkg/planner/core/optimizer.go::CheckTableLock`、`pkg/planner/optimize.go::OptimizeForForeignKeyCascade`。
- 独立测试：`pkg/lock/lock_aster_unit_test.rs`、`pkg/ddl/db_table_test.go::TestLockTables`、`pkg/ddl/db_table_test.go::TestWriteLocal`、`pkg/ddl/table_modify_test.go::TestLockTableReadOnly`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且固定二级标题恰好为 11 个，并人工复核未把无 Rust 调用者的模块描述成已接线。
