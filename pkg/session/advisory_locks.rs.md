# `pkg/session/advisory_locks.rs`

## 文件定位

该文件位于 `astersql-session` crate。`pkg/session/Cargo.toml` 将 crate 根指定为 `pkg/session/lib.rs`，而 `lib.rs` 通过 `pub mod advisory_locks;` 公开本模块。模块只直接使用 crate 根的 `SessionError` 和 `SessionResult`，没有直接引入 Cargo 外部依赖，也没有条件编译分支。

它实现的是一把“会话级命名咨询锁”的底层事务对象：用一条永不提交的悲观事务持有 `mysql.advisory_locks` 表中某个锁名对应的行锁，并用回滚释放。当前 Rust 仓库中，`advisoryLock`、`AdvisorySession` 和本文件的包级封装函数都没有文件外生产引用，`AdvisorySession` 也没有实现者；因此该模块目前是已导出但尚未接入 Rust 会话主链的移植实现，不能据此宣称 Rust SQL 会话已经通过本模块持锁。

## 核心职责

- `AdvisorySession` 把底层会话所需的四项能力抽象出来：执行无参数、整型参数、字符串参数的内部 SQL，以及记录关闭阶段的错误。
- `advisoryLock` 保存单个锁的执行上下文、专用内部会话、一次性清理回调、引用计数和所有者会话标识。
- `advisoryLock::GetLock` 依次设置等待超时、开启悲观事务、向系统表插入锁名；成功后让引用计数加一，插入失败时主动关闭资源。
- `advisoryLock::IsUsedLock` 用一秒锁等待执行同样的插入探测，并保证探测事务最终回滚。
- `IncrReferences`、`DecrReferences`、`ReferenceCount` 和 `Close` 支持上层会话按 MySQL 的可重入命名锁语义管理同名锁。

该文件不负责锁名合法性与大小写归一、SQL 函数返回值、超时/死锁到 SQL 结果的映射、按名称保存锁对象、单锁释放或批量释放；Go 版本把这些上层职责放在 `pkg/session/session.go` 和表达式层。Rust 中相应表达式接口位于 `pkg/expression/expropt/advisory_lock.rs`，但尚未与本模块接线。

## 主要符号

- `InternalSourceType`：内部 SQL 来源枚举。`None` 表示未标记；`InternalTxnOthers` 表示咨询锁 SQL 等内部事务。`GetLock` 和 `IsUsedLock` 在发 SQL 前都会写入后者。
- `AdvisoryContext { source }`：传给每次内部 SQL 的轻量上下文。它在锁对象内复用，因此来源标记会保留到后续 `Close` 的 `ROLLBACK`。
- `AdvisorySession: Send`：对象安全的执行边界。三个执行方法均返回 `SessionResult`；带参数的方法分开声明，避免本模块自己拼接锁名和超时。`LogCloseError` 专门处理无法再向调用者返回的关闭错误。
- `advisoryLock`：单锁状态。名称沿用 Go 的非 CamelCase 命名，文件通过 `#![allow(non_camel_case_types, non_snake_case)]` 保留移植符号风格。字段全部公开，但类型自身并未在 `lib.rs` 重导出。
- `advisoryLock::{IncrReferences, DecrReferences, ReferenceCount}`：只做整数加减/读取，不检查溢出或下溢，也不决定何时真正释放。
- `advisoryLock::Close`：尝试执行 `ROLLBACK`；失败只调用 `LogCloseError`，随后仍通过 `Option::take` 至多执行一次 `clean`。
- `advisoryLock::GetLock`：真实获取路径。仅当三条内部 SQL全部成功时增加 `reference_count`。
- `advisoryLock::IsUsedLock`：探测路径。返回插入尝试的原始结果，但无论设置超时、开启事务还是插入在哪一步结束，都会调用 `Close`。
- 文件末尾六个同名自由函数：分别薄封装上述方法，为 Go 风格包级调用保留入口；当前没有文件外调用者。

## 执行流程

获取锁的流程由 `advisoryLock::GetLock(lock_name, timeout)` 驱动：

1. 把 `ctx.source` 改为 `InternalTxnOthers`。
2. 通过 `ExecuteInternalWithInt` 执行 `SET innodb_lock_wait_timeout = %?`。失败直接返回，此分支不会在本文件内调用 `Close`。
3. 执行 `BEGIN PESSIMISTIC`。失败同样直接返回，也不会在本文件内调用 `Close`。
4. 通过字符串绑定执行 `INSERT INTO mysql.advisory_locks (lock_name) VALUES (%?)`。插入成功意味着悲观事务持有对应行锁；对象保留会话与事务，不提交。
5. 插入失败时调用 `Close` 回滚并运行清理回调，然后原样返回插入错误；成功时把 `reference_count` 加一并返回 `Ok(())`。

探测流程由 `advisoryLock::IsUsedLock(lock_name)` 驱动：它先设置内部来源，再在闭包中依次执行固定的一秒超时、开启悲观事务和插入。闭包以 `?` 保留最先发生的错误；闭包结束后无条件 `Close`，最后返回闭包结果。因此探测成功表示当时可以取得该行锁，探测失败则可能表示锁被占用，也可能是设置、事务或执行基础设施错误，本文件不解释错误类别。

释放由 `Close` 完成：先尝试回滚，回滚错误只记日志；再 `take()` 清理回调并执行。重复调用 `Close` 仍会重复发送 `ROLLBACK`，但不会重复运行回调。引用计数对应的释放策略必须由上层实现：Go 的 `session.ReleaseAdvisoryLock` 先减计数，降到零才 `Close`；`ReleaseAllAdvisoryLocks` 则直接逐锁关闭。

## 数据与状态

`advisoryLock` 的状态归属于单把锁，而不是全局注册表：

- `ctx` 是可变执行上下文；目前唯一状态是 `source`。
- `session: Box<dyn AdvisorySession>` 独占一条内部会话能力。独立会话是关键不变量：不同命名锁需要能按任意顺序回滚，不能把多把锁塞进同一个普通事务。
- `clean: Option<Box<dyn FnOnce() + Send>>` 表示专用会话归还/销毁动作。`FnOnce` 与 `take()` 共同保证回调最多消费一次。
- `reference_count: i32` 表示同一外部会话对同名锁成功获取的次数。`GetLock` 只为首次事务获取加一；上层若发现同名锁已存在，应调用 `IncrReferences`，而不是再次开启事务。
- `owner: u64` 保存所有者连接/会话标识，但本文件不读取它。Go 上层的 `IsUsedAdvisoryLock` 在同会话命中时直接返回该值。

本文件没有锁名字段，也没有名称到锁对象的映射。因此对象与锁名之间的对应关系只能由调用者维护；Go 版本由 `session.advisoryLocks map[string]*advisoryLock` 保证。

## 依赖与调用关系

上游边界目前分成“预期的 Go 对照链”和“Rust 当前事实”：

- Go 主链为表达式内置函数调用会话的 `GetAdvisoryLock` / `IsUsedAdvisoryLock` / `ReleaseAdvisoryLock` / `ReleaseAllAdvisoryLocks`，会话再创建或操作 `advisoryLock`（`pkg/session/session.go:2069-2146`）。会话关闭也会调用 `ReleaseAllAdvisoryLocks`（`pkg/session/session.go:3510-3522`）。
- Rust 表达式层定义了另一个 `AdvisoryLockContext` 接口以及可选属性提供者（`pkg/expression/expropt/advisory_lock.rs`），但代码搜索未发现生产环境中的 session 实现，也未发现它调用本文件。
- RustCodeGraph 对目标文件识别出 21 个符号；文件内调用边包括 `GetLock -> ExecuteInternalWithInt/ExecuteInternalWithString/ExecuteInternal/Close`、`IsUsedLock -> ExecuteInternal/ExecuteInternalWithString/Close`、`Close -> ExecuteInternal/LogCloseError`，以及六个自由函数到对应方法的转发。
- RustCodeGraph 的同名符号结果会混入 Go 实现和其他 crate 的 `GetLock`，因此文件外关系又用全仓 Rust 精确引用搜索复核；除 `pkg/session/lib.rs` 的模块声明外没有目标类型或 trait 的生产引用。

下游只有 `AdvisorySession` 方法和 crate 根的 `SessionResult`/`SessionError`。SQL 字符串依赖数据库存在 `mysql.advisory_locks(lock_name)` 以及悲观事务、`innodb_lock_wait_timeout` 的语义，但这些设施不由本文件声明或创建。

## 错误处理与边界

- 三种内部 SQL 执行错误都通过 `SessionResult` 原样传播；本文件不区分锁等待超时、死锁、系统表缺失或会话执行失败。
- `GetLock` 只在插入失败时主动 `Close`。设置超时或 `BEGIN PESSIMISTIC` 失败时直接返回，是否需要清理专用会话取决于尚未接入的上层；这是扩展接线时必须验证的资源边界，不能假定 trait 实现会自动清理。
- `IsUsedLock` 会在所有闭包返回路径后调用 `Close`，但关闭失败只记录日志，不替换探测结果。因此可能出现“探测返回成功、回滚失败已记录”的组合。
- `Close` 保证即使 `ROLLBACK` 失败仍执行 `clean`，避免日志错误阻断资源归还；反过来，`clean` 是无返回值回调，无法向上报告清理失败。
- 引用计数没有防御性检查。调用者若在零时继续 `DecrReferences` 会得到负数；Debug 构建中整数溢出会 panic，Release 构建行为取决于编译溢出设置。正常不变量应由上层“仅对已持有锁释放”维护。
- 锁名空值、空串、64 字符上限、Unicode 字符数、大小写归一和负超时钳制都不在本文件处理。相关 Rust 表达式测试位于 `pkg/expression/builtin_miscellaneous_18_aster_unit_test.rs` 与 `pkg/expression/integration_test/integration_part2_aster_unit_test.rs`，不可把这些测试的通过等同于本模块事务路径已覆盖。

## 并发与资源生命周期

`AdvisorySession: Send`、`session: Box<dyn AdvisorySession>` 和 `clean: ... + Send` 允许锁对象跨线程移动，但 `advisoryLock` 的方法都需要 `&mut self`，本文件没有内部 `Mutex`、原子计数或共享所有权。并发串行化必须由拥有者完成；把同一对象并发暴露需要外部同步。

成功获取后的生命周期是：创建专用内部会话与清理回调，开启悲观事务并插入锁名，长期保留未提交事务；同会话重复获取只增加引用；引用归零、批量释放或会话关闭时回滚；最后执行清理回调。Go 实现还禁止持有咨询锁的会话迁移（`pkg/session/session.go:5749-5752`），Rust 当前没有对应接线证据。

`IsUsedLock` 使用临时专用会话，探测完成即回滚和清理。`clean.take()` 使清理动作幂等，但事务回滚本身不是幂等门控：重复 `Close` 会再次尝试回滚。该差异应由所有者状态机控制。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/session/advisory_locks.go`。Rust 保留了 Go 的 `advisoryLock` 五项状态和六个方法的总体算法：内部来源标记、设置等待超时、`BEGIN PESSIMISTIC`、向 `mysql.advisory_locks` 插入、插入失败回滚、引用计数以及一秒探测。

主要移植差异如下：

- Go 直接持有 `*session` 和 `context.Context`；Rust 用 `AdvisorySession` trait 与 `AdvisoryContext` 隔离尚未完成的会话实现。
- Go 的 `clean` 必定存在且每次 `Close` 都调用；Rust 使用 `Option<FnOnce>`，清理最多执行一次。
- Go 用 `terror.Log(err)` 记录回滚错误；Rust 委托 `AdvisorySession::LogCloseError`。
- Go 的 SQL 参数统一由可变参数 `ExecuteInternal` 传入；Rust 按无参数、整数、字符串拆成三个方法。
- Go `GetLock` 与 Rust 一样只在插入失败时显式关闭；前两步失败不关闭。Rust 没有额外修正这一行为。
- Go 上层 `session.go` 已实现按名称缓存、同名引用累加、所有者查询、单锁与全部释放、会话关闭清理和迁移限制；Rust 本文件及当前生产搜索没有这些接线。

Go 集成测试 `pkg/expression/integration_test/integration_test.go` 覆盖参数数量、超时、非法名称、大小写、引用计数、批量释放及跨会话竞争。Rust 的 `pkg/expression/integration_test/integration_part2_aster_unit_test.rs::TestGetLock` 移植了这些 SQL 场景，但它不是本文件的独立单元测试；当前未找到直接构造 `advisoryLock` 并验证 SQL 顺序、失败清理或回滚日志的 Rust 测试。

## 扩展指南

若要把该模块真正接入 Rust 会话主链，最小接入面应包括：

1. 为真实内部会话实现 `AdvisorySession`，确保参数绑定而非字符串拼接，并明确每个失败阶段的会话归还规则。
2. 在 Rust 会话状态中增加按归一化锁名索引的对象表；首次获取创建独立内部会话，重复获取调用 `IncrReferences`。
3. 实现表达式层 `pkg/expression/expropt/advisory_lock.rs::AdvisoryLockContext`，把四个会话操作桥接到该对象表，而不是另建重复事务算法。
4. 会话关闭时批量 `Close`；若支持会话迁移，应像 Go 一样在仍持锁时拒绝迁移，或设计有证据支持的锁转移协议。
5. 新增独立测试文件，例如同目录 `advisory_locks_test.rs`，不要把测试写回生产源文件。使用假的 `AdvisorySession` 记录调用序列，至少覆盖三步成功、三处分别失败、插入失败回滚、探测所有失败分支均清理、回滚失败仍清理、重复 `Close` 仅清理一次、引用计数约束。
6. 保留并运行表达式层现有边界测试，另外增加真实 session 适配器的集成测试，证明 SQL 函数确实走到此模块。

兼容风险主要是 MySQL 返回值和错误分类、锁名大小写/长度规则以及 `RELEASE_ALL_LOCKS()` 对引用次数的计数；正确性风险集中在失败路径泄漏内部会话或事务、重复清理和同名锁映射；性能风险来自每个唯一锁占用一个长期内部会话与悲观事务。任何优化都必须保留“不同锁可按任意顺序回滚”的资源模型。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件已收录；`files --filter pkg/session/advisory_locks.rs` 报告该文件含 21 个符号。
- RustCodeGraph `explore "pkg/session/advisory_locks.rs"`：读取了目标文件完整源码并得到文件内调用流；另用 `query advisoryLock --kind struct` 和 `query AdvisorySession --kind trait` 区分了 Go/Rust 同名类型。
- 直接读取：`pkg/session/advisory_locks.rs`、`pkg/session/lib.rs`、`pkg/session/Cargo.toml`、`pkg/session/doc.go`。
- Go 对照：`pkg/session/advisory_locks.go`；上层生命周期与调用者：`pkg/session/session.go:2069-2146`、`pkg/session/session.go:3510-3522`、`pkg/session/session.go:4988-4996`、`pkg/session/session.go:5749-5752`。
- Rust 接口与测试：`pkg/expression/expropt/advisory_lock.rs`、`pkg/expression/builtin_miscellaneous_18_aster_unit_test.rs::advisory_locks_preserve_normalization_limits_and_error_mapping`、`pkg/expression/integration_test/integration_part2_aster_unit_test.rs::TestGetLock`。
- Go 行为测试：`pkg/expression/integration_test/integration_test.go` 的 GET_LOCK/RELEASE_LOCK 测试段。
- 全仓 Rust 精确引用搜索仅命中目标文件自身及 `pkg/session/lib.rs` 的模块声明；生产环境中的 `AdvisoryLockContext` 实现搜索也未发现 session 适配器。由此确认“模块已导出、事务对象尚未接入主链”，而不是把表达式测试误判为直接覆盖。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务给定命令验证目标文档恰有十一个固定二级标题，并人工复核未把未接线能力描述为已支持。
