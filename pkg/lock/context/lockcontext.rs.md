# `pkg/lock/context/lockcontext.rs`

源文件：[lockcontext.rs](./lockcontext.rs)

## 文件定位

`TableLockReadContext` 隔离“读取当前会话持有哪些表锁”的能力，让检查器和规划代码无需知道实际容器或会话类型。

## 核心职责

- `TableLockReadContext` 隔离“读取当前会话持有哪些表锁”的能力，让检查器和规划代码无需知道实际容器或会话类型。
- `TableLockContext` 在读接口之上增加批量加入、定向释放和全部释放能力；父 trait 约束保证写上下文同时具备完整读契约。
- 方法以 table ID、`ast::TableLockType` 和 `model::TableLockTpInfo` 传递信息，保持与 `pkg/lock/context/lockcontext.go` 的接口形状对应。
- 本文件只规定操作形状，不规定容器、过滤规则、顺序、同步方式或错误策略；这些语义必须由实现及其测试补足，不能从 trait 声明自行推导。

## 主要符号

### `pub trait TableLockReadContext`

- `CheckTableLocked(&self, tbl_id: i64) -> (bool, ast::TableLockType)`：按表 ID 查询。布尔值区分“未持锁”和“持锁”，第二个值承载锁类型；独立测试把未命中约定为 `(false, ast::TableLockNone)`。
- `GetAllTableLocks(&self) -> Vec<model::TableLockTpInfo>`：返回调用者拥有的锁信息集合。返回 `Vec` 意味着接口不借出实现内部容器；trait 本身不承诺元素顺序。
- `HasLockedTables(&self) -> bool`：提供无需构造完整列表的空/非空判断。`pkg/lock/lock.rs::Checker` 多次用它选择“当前会话处于 LOCK TABLES 模式”相关分支。

### `pub trait TableLockContext: TableLockReadContext`

- `AddTableLock(&mut self, locks: &[model::TableLockTpInfo])`：批量将锁信息交给实现保存。
- `ReleaseTableLocks(&mut self, locks: &[model::TableLockTpInfo])`：按锁信息列表发起释放。Go 会话实现与 Rust 独立测试都实际按其中的 `TableID` 删除，而不比较 schema ID 或锁类型。
- `ReleaseTableLockByTableIDs(&mut self, table_ids: &[i64])`：无需构造完整 `TableLockTpInfo` 即可按 ID 批量释放。
- `ReleaseAllTableLocks(&mut self)`：清空当前上下文保存的全部会话锁。

两个 trait 均为公开 API；文件没有模块级常量、结构体、枚举、自由函数、默认方法或条件编译项。

## 执行流程

本文件没有可执行实现，运行流程由调用者与实现者共同形成。当前可验证的链路如下：

1. Rust 检查侧通过 `pkg/lock/lock.rs::NewChecker` 注入一个 `&dyn TableLockReadContext`。
2. `Checker::CheckTableLock` 先过滤空目标、`LOCK TABLES` 权限以及系统/内存库；进入会话锁相关分支后调用 `HasLockedTables`。
3. DROP 检查可能遍历 `GetAllTableLocks`，普通表级检查则用 `CheckTableLocked(meta.ID)` 得到本会话是否持锁及锁类型，再由 `checkLockTpMeetPrivilege` 判断所需权限是否满足。
4. Rust 写接口目前只有 `migration_aster_unit_test.rs::SessionTableLocks` 的测试实现：加入一批锁、查询、按完整信息或 ID 释放、最后清空，以验证 trait 组合和移植语义。
5. Go 完整应用中，`pkg/session/session.go::session` 用 `lockedTables` 映射实现这些操作。`pkg/ddl/executor.go` 的锁表流程先读取旧锁，在提交 DDL job 前暂存新锁；成功后释放旧锁并确认新锁。解锁或 cleanup 成功后相应清空或定向删除； truncate 的成功提交/完成回调还负责在旧、新 table ID 之间迁移或回滚会话锁记录。

第 5 步是 Go 对照主链，不代表等价 Rust DDL 写链已经接通。仓库内没有生产类型对本文件两个 trait 的 `impl`，这是当前迁移边界。

## 数据与状态

接口围绕三类数据工作：`i64` table ID、`ast::TableLockType` 锁类型，以及包含 `SchemaID`、`TableID`、`Tp` 的 `model::TableLockTpInfo`。trait 不拥有这些状态；状态属于实现者。

Go 的权威实现 `pkg/session/session.go` 使用 `map[int64]model.TableLockTpInfo`，Rust 的测试实现使用 `HashMap<i64, (i64, u8)>`，两者都以 table ID 为唯一键。因此再次加入相同 table ID 会覆盖旧条目，定向释放的身份依据也是 table ID。两边还都跳过 `TableLockReadOnly`：Go 注释说明只读锁与会话无关，Rust 迁移测试据此验证该类型不会进入会话本地映射。

`GetAllTableLocks` 返回集合快照而不是容器引用。Go 通过遍历 map 构造 slice；Rust 测试实现通过遍历 `HashMap` 构造 `Vec`，所以调用者不得依赖稳定顺序。`HasLockedTables` 与集合是否为空应保持一致，这是 `assert_read_contract` 明确检查的不变量。

## 依赖与调用关系

- 下游类型依赖：`crate::ast::TableLockType` 来自 `astersql-parser-ast`；`crate::model::TableLockTpInfo` 来自 `astersql-meta-model`，均经 `pkg/lock/context/lib.rs` 再导出。
- Rust 直接消费者：`pkg/lock/lock.rs::Checker` 保存 `&dyn TableLockReadContext`，调用三个只读方法完成表锁权限判断。
- Rust 类型约束消费者：`pkg/planner/planctx/context.rs::PlanContext: Common + tablelock::TableLockReadContext`。
- Rust 写侧验证者：`pkg/lock/context/migration_aster_unit_test.rs::SessionTableLocks` 是仓库搜索到的唯一 `TableLockReadContext`/`TableLockContext` 实现。
- crate 依赖者：`pkg/lock/Cargo.toml` 和 `pkg/planner/planctx/Cargo.toml` 直接引用 `astersql-lock-context`；workspace 根 manifest 还以 `facade_lock_context` 暴露该 crate。
- Go 上游：`pkg/ddl/executor.go` 的 `LockTables`、`UnlockTables`、`CleanupTableLock`、`HandleLockTablesOnSuccessSubmit` 和 `HandleLockTablesOnFinish` 使用写接口；`pkg/lock/context/lockcontext.go` 的接口还被 Go planner 路径引用。

RustCodeGraph 对同名跨语言方法会产生误配边，例如把 trait 声明的方法关联到 Go DDL 或无关 Rust `remove`/`clear` 方法。本文只采用经文件内容和精确仓库搜索复核过的关系，不把这些误配当作 Rust 调用链。

## 错误处理与边界

所有方法都不返回 `Result`，因此接口层没有可传播错误。未命中查询通过返回值表达；独立测试采用 `(false, TableLockNone)`。释放不存在的 table ID 在 Go map 与 Rust 测试 `HashMap` 中都是幂等无错误操作。

边界条件包括：

- 空切片应自然成为无操作；trait 没有额外前置条件。
- `ReleaseTableLocks` 的完整结构体不是复合身份：已验证的 Go/Rust 测试语义只看 `TableID`。
- `TableLockReadOnly` 是否跳过并未写入 trait 类型系统，而是当前 Go 实现和迁移测试共同确认的实现语义；新实现若不遵守会与会话锁模型不兼容。
- 未知锁类型可以由 `TableLockType` 的底层值表示；本文件不验证它。消费侧 `pkg/lock/lock.rs::checkLockTpMeetPrivilege` 对未知类型不放行。
- 本文件不检查重复 ID、schema/table 对应关系或 DDL job 成败；这些属于实现和上层流程。

## 并发与资源生命周期

`TableLockContext` 的写方法要求 `&mut self`，在单个 Rust 借用范围内排除同时可变访问；读方法只要求 `&self`。但 trait 没有 `Send`、`Sync` 父约束，也没有内部锁或异步方法，因此不承诺跨线程共享安全，具体并发策略由实现者负责。

会话锁的逻辑生命周期由上层控制。Go 证据显示：锁表 job 提交前先把目标锁加入会话，以覆盖“job 已成功但 session 在返回前被终止”的窗口；job 成功后删除旧锁并保留新锁；UNLOCK 成功后清空；cleanup 成功后定向释放；truncate 提交后临时复制到新 table ID，并在成功或失败完成时删除旧 ID 或新 ID。trait 只提供这些状态转换所需的原子操作形状，不提供事务、回滚钩子、锁守卫或资源清理器。

返回的 `Vec` 由调用者拥有，输入切片仅在方法调用期间借用；接口没有悬挂引用或显式资源释放要求。

## 与 Go 版本的对应关系

- Go 的 `TableLockReadContext` 三个方法与 Rust 同名，参数和返回语义对应。
- Go 的 `TableLockContext` 嵌入读接口；Rust 用 `TableLockContext: TableLockReadContext` 表达同一约束。
- Go 接口由 `pkg/session/session.go::session` 在生产路径隐式满足。Rust trait 需要显式 `impl`，当前仓库尚未给生产 `session` 添加该实现；`pkg/session/session.rs` 只有同形固有方法，且使用其自身导入的锁信息类型。故 Rust 迁移已覆盖契约和读侧消费者，但生产写侧接线仍未由本文件及其直接证据证明。

## 扩展指南

新增只读能力时，应先修改 `TableLockReadContext`，再同步所有实现者和消费者；由于 `TableLockContext` 继承它，写上下文实现也必须补齐新方法。当前至少需要同步 `migration_aster_unit_test.rs::SessionTableLocks`，并检查 `pkg/lock/lock.rs::Checker` 与 `pkg/planner/planctx/context.rs::PlanContext` 是否真正需要新能力。

新增写操作时，应修改 `TableLockContext`，在独立测试文件 `pkg/lock/context/migration_aster_unit_test.rs` 添加状态转换测试；不要把测试内嵌进 `lockcontext.rs`。若目标是完成生产接线，应在会话所属 crate 中显式实现本 trait，并先解决 `pkg/session/session.rs` 使用的锁类型与本 crate 再导出类型之间的一致性，而不是只增加另一个同名固有方法。

任何改变都应同步核对 Go 的 `pkg/lock/context/lockcontext.go`、`pkg/session/session.go` 以及 DDL 回归 `pkg/ddl/executor_test.go::TestHandleLockTable`。兼容风险主要是扩大 trait 导致现有实现编译失败、改变 `ReadOnly` 过滤或 table-ID 身份规则导致会话状态偏离 Go；性能风险主要在把 `HasLockedTables` 实现成构造完整列表，或让 `GetAllTableLocks` 进行不必要排序/深层复制。若引入共享并发访问，需要明确选择内部同步或外部锁，并评估是否为 trait 增加 `Send`/`Sync` 约束。
