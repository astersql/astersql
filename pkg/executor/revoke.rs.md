# `pkg/executor/revoke.rs`

## 文件定位

本文件是 `astersql-executor` crate 中 REVOKE 权限回收算法的 Rust 移植，源码由 [`lib.rs`](lib.rs) 的 `pub mod revoke` 公开，crate 边界由 [`Cargo.toml`](Cargo.toml) 定义。它把 Go [`revoke.go`](revoke.go) 中直接依赖 session、InfoSchema、权限管理器和内部 SQL 执行器的逻辑，拆成泛型 `RevokeExec<B>` 与 `RevokeBackend` 适配边界。

当前仓库事实需要区分两层：`RevokeExec::Next` 及辅助函数已有完整算法和独立测试；但在目标文件和测试之外搜索 Rust `RevokeExec`、`composeTablePrivUpdateForRevoke` 等符号，只找到 [`lib.rs`](lib.rs) 的模块声明，没有找到生产 builder 构造、具体 `RevokeBackend` 实现或执行器主链接线。因此它目前是“已导出的可适配实现”，不能仅凭模块公开就认定 Rust SQL 请求已经会执行它。Go 生产入口仍是 [`revoke.go`](revoke.go) 的 `RevokeExec.Next`。

## 核心职责

- `RevokeExec::Next` 保证一条 REVOKE 语句只执行一次，在独立系统会话事务中按用户回收权限，并在提交后通知权限缓存更新。
- `revokeOneUser`、`revokePriv` 根据 `GrantLevel` 把请求分发到全局、库、表、列四种路径；`Usage` 是无副作用的占位权限。
- `revokeDynamicPriv` 处理动态权限：统一转成大写；未注册名称产生警告但仍尝试删除，以保持 MySQL 兼容行为。
- `composeScopePrivilegeUpdate`、`composeTablePrivUpdateForRevoke`、`composeColumnPrivUpdateForRevoke` 计算系统权限表的新值；执行 SQL 与转义不在这里完成，而由 `RevokeBackend::execute_internal` 消费类型化的 `RevokeSql`。
- `RevokeBackend` 集中隔离事务、系统会话、用户/授权行查询、InfoSchema 名称解析、告警、SQL 执行和域通知，使核心逻辑可用 mock 精确验证。

## 主要符号

- `PrivilegeKind`：区分 `Usage`、动态权限 `Extended`、`All`、`Grant` 和普通静态权限；它控制校验和更新算法。
- `PrivilegeType`：携带显示名、SET 名、系统表列名以及全局/库级适用标志。缺少必需的 `set_name` 或 `column_name` 会成为显式错误。
- `PrivilegeElement`：一项待撤销权限；`columns` 为空时表级处理，非空时列级处理。
- `GrantLevel` / `GrantLevelSpec`：描述 `*.*`、`db.*`、`db.tbl` 或未知级别，并保存原始库表名。
- `UserIdentity` / `UserSpec`：描述目标用户；`current_user` 在执行时被认证用户替换。
- `ResolvedTable` / `ResolvedColumn` / `TargetTableError`：表达后端对库表名、列名大小写和“表不存在”这一可区分错误的解析结果。
- `RevokeSql`：内部副作用的类型化命令，包括 `Begin`、`Commit`、`Rollback`，以及对 `mysql.user`、`mysql.db`、`mysql.tables_priv`、`mysql.columns_priv`、动态权限记录的更新或删除。适配器必须负责标识符和值的正确转义。
- `RevokeBackend`：生产适配契约；关联类型 `Context`、`Session`、`Error` 避免核心算法绑定具体运行时。
- `RevokeExec<B>`：一次性执行器。`BaseExecutor` 持有后端，`Privs`、`Level`、`Users` 是语句输入，`done` 是幂等门闩；`ObjectType` 当前被保存但未被本文件逻辑读取。
- `TablePrivilegeUpdate` / `ColumnPrivilegeUpdate`：SET 字符串更新结果，并用 `delete_row` 表示更新后是否应删除空授权行。
- `userSpecToUserList`、`privUpdateForRevoke`、`SetFromString`：分别生成通知用户名列表、从 SET 列表删除权限、解析逗号分隔 SET 字符串。

## 执行流程

1. `RevokeExec::Next` 先检查 `done`；首次进入即将其置为 `true`，随后调用 `new_transaction_in_statement`，语义与 Go 版“像 DDL 一样先结束旧事务”对应。
2. 它取得系统会话。取得失败时清除 `in_transaction` 并返回；取得成功后执行 `RevokeSql::Begin`。
3. 对每个 `Users` 元素，若指定 `CURRENT_USER`，就用 `authenticated_user` 返回的认证用户名和主机原地替换。随后 `user_exists` 必须确认账号存在，否则整批失败。
4. `checkDynamicPrivilegeUsage` 收集并大写全部动态权限名；只要目标不是 `GrantLevel::Global` 就返回非法权限级别错误。
5. `revokeOneUser` 解析空库名为当前库。库级和表级在修改前分别要求 `database_grant_exists` 或 `table_grant_exists` 为真；表级允许 InfoSchema 中表已不存在，但仍使用请求表名查找并删除遗留授权行。
6. 每项权限进入 `revokePriv`：全局走 `revokeGlobalPriv`，库级走 `revokeDBPriv`，表级根据列列表是否为空选择 `revokeTablePriv` 或 `revokeColumnPriv`，未知级别报错。
7. 全局 `ALL` 先删除该用户所有动态权限，再把全部全局静态权限列置 `N`；单项动态权限删除 `global_grants` 对应记录；其他全局权限只更新目标列。全局更新特意把主机名转为小写，与 Go 实现一致。
8. 库级先把目标权限列置 `N`，再携带全部库权限列和 Grant 列执行 `DeleteEmptyDatabase`，由适配层删除全为 `N` 的空行。
9. 表级先读取当前 Table/Column SET。撤销 `ALL` 时表 SET 只可能保留 Grant option、列 SET 清空；单项撤销则同时从两个 SET 移除目标。更新后若表 SET 为空，再删除整条表授权记录。
10. 列级通过小写键匹配真实列并使用其原始名称写系统表。每列先更新 SET；若已空则删除对应列授权行并结束循环。
11. 所有用户成功后执行 `Commit`，标记已提交，再按原顺序发送用户名列表给 `notify_privilege_update`。任一步在提交前失败都会尝试 `Rollback`；最后无条件释放系统会话并清除事务标志。

## 数据与状态

`RevokeExec` 的主要可变状态是 `done` 和 `Users`：`done` 在任何后续校验之前就置真，所以一次失败后同一实例也不会重试；`CURRENT_USER` 会永久改写对应 `UserIdentity`。权限更新本身不在内存中缓存，当前 SET 值由后端在事务内按需读取。

`RevokeSql` 是算法与数据库适配之间的数据协议。它保留用户、主机、库、表、列及赋值列表的结构化形式，避免核心代码拼接 SQL；但安全性最终依赖生产适配器正确渲染和转义这些字段。SET 字符串由 `SetFromString` 按逗号直接拆分，保持原顺序，不去重、不修剪空白；这假设系统权限表提供规范化值。

表级 `delete_row` 只由新的 `table_privilege` 是否为空决定，不参考 `column_privilege`；这复刻了 Go `composeTablePrivUpdateForRevoke`。列级 `ALL` 不读取原值而直接得到空 SET。用户名通知只包含用户名、不包含主机名，并保持语句中的用户顺序。

## 依赖与调用关系

上游设计入口是 SQL 执行器对 `RevokeExec::Next` 的调用；同路径 Go 版明确实现 `exec.Executor` 并在 `Next` 中执行该流程。Rust 侧 [`lib.rs`](lib.rs) 导出 `revoke`，测试模块也从 crate 根引用公开符号；但仓库搜索未发现 Rust 生产调用者或具体后端实现，所以当前真实 Rust 上游仅能确认到独立测试 [`revoke_test.rs`](revoke_test.rs)。

`Next` 的内部调用链为：`checkDynamicPrivilegeUsage` → `revokeOneUser` → `revokePriv` → `revokeGlobalPriv` / `revokeDBPriv` / `revokeTablePriv` / `revokeColumnPriv`。表、列两条路径再调用相应 `compose*UpdateForRevoke`，后者依赖 `privUpdateForRevoke` 与 `SetFromString`。

下游能力全部经 `RevokeBackend` 注入：事务与系统会话、用户和授权行存在性、InfoSchema 名称解析、动态权限注册表、权限表当前 SET、内部 SQL、告警和权限缓存通知。本文件自身只使用标准库 `std::fmt::Display`；[`Cargo.toml`](Cargo.toml) 将它编入 `astersql-executor`，未为本文件声明专属 feature，`nextgen` feature 也不控制此模块。

RustCodeGraph 的文件节点能够展示源码及符号，但针对泛型 impl 方法的精确 `callers`/`callees` 查询未返回可用边；因此本节没有采用全仓库同名 `Next` 的模糊边，而以源内调用、`lib.rs` 装配和精确符号搜索为证据。

## 错误处理与边界

后端错误通过 `Result<_, B::Error>` 原样传播；领域错误由 `RevokeBackend::error` 构造。主要边界包括：目标用户不存在、动态权限用于非全局级、库/表授权行不存在、未知撤销级别、权限不适用于目标作用域、权限元数据缺少列名或 SET 名、列不存在，以及后端解析/查询/执行/通知失败。

表级撤销刻意允许目标表已经从 InfoSchema 消失：只要 `mysql.tables_priv` 中仍有授权行，就可用请求名称清理，呼应 Go 注释中的 issue #28533。列级不同：必须解析到真实表和列；表不存在会变成 `table not found`，未知列会返回 `Unknown column`。`target_schema_and_table` 成功却返回 `None` 在列级通过 `expect` 触发 panic，这是后端契约不变量，生产适配器必须保证列级成功解析一定携带表。

提交之前的错误触发回滚；回滚失败仅交给 `log_rollback_error`，不会覆盖原始错误。`Commit` 成功而 `notify_privilege_update` 失败时不会回滚，因为持久化已完成；调用者会收到通知错误，此时需要把它理解为“权限已写入但缓存通知失败”。取得系统会话失败时没有可回滚会话，但会清除事务标志。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道。并发隔离依赖后端提供的独立系统会话和数据库事务；所有用户与权限都在同一 `Begin`/`Commit` 范围内串行处理，因此提交前任一失败应回滚整条语句。

系统会话从 `get_system_session` 成功后一直持有到结果产生，无论成功或失败都会由 `release_system_session` 归还；外层语句事务标志随后清零。`done` 只提供单实例、单调用序列的一次性语义，不是原子量，也没有同步保护；执行器实例不应被多个线程并发调用。生产 `RevokeBackend` 还必须确保异常路径不会跳过会话归还和事务状态恢复。

## 与 Go 版本的对应关系

Rust `RevokeExec::Next` 对应 Go [`revoke.go`](revoke.go) 的 `RevokeExec.Next`：二者均一次执行、创建内部权限事务、替换 `CURRENT_USER`、逐用户校验和撤销、提交后通知，并在未提交时回滚。Rust 用 `RevokeBackend` 和 `RevokeSql` 表达 Go 中的 `sessiontxn`、系统 session、InfoSchema、`sqlescape.MustFormatSQL`、权限管理器和 domain 通知。

辅助函数也逐一对应：`checkDynamicPrivilegeUsage`、`revokeOneUser`、`revokePriv`、四个作用域撤销函数、`privUpdateForRevoke`、两个 SET 更新组合函数均保留 Go 分支。关键兼容细节包括动态权限大写和未注册警告、表不存在时仍可撤销遗留授权、库/表/列权限清空后删行，以及表级 `REVOKE ALL` 不撤销 Grant option。

Rust 独立测试 [`revoke_test.rs`](revoke_test.rs) 验证事务/清理顺序、重复 `Next` 无操作、非法动态权限回滚、未注册动态权限告警、表列 SET 语义、解析后的列名和未知权限错误。Go [`revoke_test.go`](revoke_test.go) 进一步用 SQL 集成路径覆盖全局、库、表、列撤销、最后权限删行、大小写不敏感名称及已删除表的遗留授权清理。Rust 测试尚未覆盖 Go 集成测试的所有大小写与不存在对象组合，也不证明生产主链接线。

## 扩展指南

- 新增权限类别或作用域时，先扩展 `PrivilegeKind` / `PrivilegeType` / `GrantLevel`，再同步 `revokePriv` 分发和 `composeScopePrivilegeUpdate` 的适用性校验；不可只让 mock 测试通过而省略 Go 已有分支。
- 新增系统表副作用应优先增加新的 `RevokeSql` 变体，并在生产适配器中使用统一标识符/值转义层渲染，不应在核心逻辑中拼接用户输入。
- 改动事务流程时必须保持“提交前失败回滚、会话总是释放、事务标志总是复位、提交后通知失败不伪装成回滚”的状态边界，并补充失败注入测试。
- 调整名称解析时要分别保留表级“表不存在仍可清理授权”和列级“必须解析真实列”的不同契约，同时测试库表列大小写。
- 若要接入 Rust 生产主链，需要实现具体 `RevokeBackend`、把 parser/planner 结果转换为本文件的数据类型、在 executor builder 构造 `RevokeExec`，并为 `RevokeSql` 提供安全执行适配；这些接线当前未在仓库 Rust 源码中检出。
- 测试逻辑应继续放在独立的 [`revoke_test.rs`](revoke_test.rs)，并由 [`lib.rs`](lib.rs) 的 `#[cfg(test)]` 模块加载；涉及最终 SQL 行为时还应与 [`revoke_test.go`](revoke_test.go) 的案例对齐，不把测试嵌回生产文件。
- 兼容风险集中在错误文本、主机/对象名大小写、Grant option 保留规则、空授权行删除时机和通知时序；性能上当前逐用户、逐权限、逐列串行且列级存在逐列读写，批处理优化必须先证明事务语义和错误停止位置不变。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；通过 `node --file pkg/executor/revoke.rs` 阅读目标文件 1–869 行，并用 `query` 精确确认 Rust/Go 的 `RevokeExec`、`revokePriv`、`revokeTablePriv`、`checkDynamicPrivilegeUsage`、`composeTablePrivUpdateForRevoke` 对应符号。
- RustCodeGraph `node --file pkg/executor/revoke_test.rs`：阅读独立 Rust 测试 1–454 行，确认 mock 后端和七个测试所覆盖的状态、副作用与错误边界。
- RustCodeGraph `node --file pkg/executor/revoke.go` 与 `node --file pkg/executor/revoke_test.go`：分别阅读 Go 实现 1–420 行和 Go 测试 1–245 行，核对生产事务流程、系统表更新、兼容分支和集成用例。
- [`Cargo.toml`](Cargo.toml)：确认 crate 名为 `astersql-executor`、库入口为 `lib.rs`，且目标文件没有条件 feature；[`lib.rs`](lib.rs) 确认生产模块导出和独立测试模块装配。
- 精确 `rg` 检查：除目标源、目标测试和 `lib.rs` 模块声明外，未发现 Rust `RevokeExec` 或两个 `compose*ForRevoke` 辅助函数的生产引用；这支持“尚未检出生产适配接线”的限定结论。
- 文档交付只做结构检查，不运行 Cargo；代码行为证据来自现有源码与测试，现有测试在本任务中未执行。
