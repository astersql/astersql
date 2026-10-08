# `pkg/session/txnmanager.rs`

## 文件定位

`pkg/session/txnmanager.rs` 位于 `astersql-session` crate；`pkg/session/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/session/lib.rs` 通过 `pub mod txnmanager;` 公开该模块，并在 `#[cfg(test)]` 下把独立测试文件 `pkg/session/txnmanager_test.rs` 接入。

该文件实现一套会话级事务管理门面：`TxnManager` 持有当前 `TxnContextProvider`，根据新事务请求选择 provider，并把时间戳、快照、语句生命周期、错误建议和提交前设置等操作转发给 provider。它同时维护慢事务事件时间线。

需要特别注意当前接线状态：Rust 的跨 crate 公共事务接口定义在 `pkg/sessiontxn/interface.rs::TxnManager`，而本文件的同名结构体没有实现该 trait；仓库文本搜索也没有发现生产 Rust 代码构造此结构体，直接构造和调用目前集中在 `pkg/session/txnmanager_test.rs`。因此，本文件是对 Go `pkg/session/txnmanager.go` 的局部移植和可测试实现，不能仅凭模块公开性断言它已经成为 Rust SQL 会话主链使用的事务管理器。

## 核心职责

1. 以 `TxnContextProvider` 统一抽象具体事务上下文，屏蔽乐观、悲观、陈旧读以及隔离级别差异。
2. 在 `EnterNewTxn` 中选择、初始化并安装 provider；初始化失败时通知会话回滚。
3. 为读 TS、for-update TS、快照、语句开始/提交/回滚/重试、悲观语句阶段及提交前选项提供统一转发入口。
4. 保存当前 `StatementNode`，用 `astersql_parser::Normalize` 和会话的日志脱敏配置生成事件名。
5. 记录事务内事件之间的耗时，并在 `OnTxnEnd` 判断慢事务、上报 trace 结果和完整事件序列。

这些职责分别由 `TxnManager`、`TxnContextProvider`、`TxnManagerSession` 和 `TxnProviderFactory` 分层承担：管理器控制顺序和状态，provider 实现事务语义，会话接口提供配置及观测能力，工厂负责构造具体 provider。

## 主要符号

- `GlobalTxnScope`、`GlobalReplicaScope`：没有 provider 时，`GetTxnScope` 和 `GetReadReplicaScope` 返回的全局默认值。
- `EnterNewTxnType::{Default, WithBeginStmt}`：本文件支持的两种进入方式。与 Go 相比缺少 `EnterNewTxnBeforeStmt` 和 `EnterNewTxnWithReplaceProvider` 枚举项；预置 provider 仍可通过请求字段直接传入，但没有对应进入类型语义。
- `TxnMode::{Optimistic, Pessimistic}`：provider 选择的事务模式；使用枚举后不存在 Go 字符串模式的“非法值”分支。
- `IsolationLevel::{ReadCommitted, Serializable, RepeatableRead}`：仅在悲观模式下选择三类工厂方法，默认是可重复读。
- `StmtErrorHandlePoint`、`StmtErrorAction`：语句错误发生位置和 provider 给出的后续动作；无 provider 时固定返回 `NoIdea`。
- `StatementNode`：仅保留 `original_text` 的轻量语句表示，用于事件记录，而非完整 AST。
- `Event`：保存事件名以及自上一个事件以来的 `Duration`。
- `TxnContextProvider`：具体事务实现必须满足的转发契约，覆盖初始化、TS/快照、语句钩子、激活、预热、计划建议和提交前设置。
- `EnterNewTxnRequest`：组合进入类型、可消费的预置 provider、陈旧读 TS、可选事务模式及仅因果一致性标志。`Provider` 在使用时通过 `take()` 移出请求。
- `TxnManagerSession`：管理器所需的会话侧能力。`EnableRedactLog` 有默认值 `"OFF"`；`ConnectionID`、`TxnStartTS` 和 `TxnStatementCount` 在当前文件中没有直接读取，只是接口保留能力。
- `TxnProviderFactory`：构造陈旧读、乐观和三种悲观隔离 provider。乐观构造接收槽位号，其他普通 provider 接收因果一致性标志。
- `TxnManager`：核心状态对象；`getTxnManager` 只是调用 `TxnManager::new` 的 Go 风格构造入口，不具备 Go `getTxnManager` 的会话缓存行为。

## 执行流程

进入事务的主流程位于 `TxnManager::EnterNewTxn`：

1. `newProviderWithRequest` 首先消费 `request.Provider`；若存在，跳过工厂选择。
2. 否则，当 `StaleReadTS > 0` 时，先调用 `TxnManagerSession::SetStaleReadTS`，再由 `NewStaleReadProvider` 创建陈旧读 provider。
3. 普通事务取请求中的 `TxnMode`，缺省时使用 `DefaultTxnMode`。乐观模式读取 `optimistic_slot`，将其在 `0/1` 间异或翻转，再调用 `NewOptimisticProvider`；悲观模式按 `IsolationLevelForNewTxn` 分派到 RC、Serializable 或 RR 工厂。
4. 管理器调用 `provider.OnInitialize(request.Type)`。失败时执行 `RollbackTxn`，错误原样返回，provider 不会安装到管理器。
5. 显式 `WithBeginStmt` 会设置 `SetInTxn(true)`；随后安装 provider、调用 `TraceTxnEnter`、重置事件时钟并记录 `enter txn`。

语句流程从 `OnStmtStart` 开始：先保存语句；若没有 provider 立即报错且不记录事件；否则将原始 SQL 按 `EnableRedactLog` 规范化/脱敏，记录为事件，再将语句引用交给 provider。执行中可调用读/锁 TS、快照、悲观阶段钩子、错误建议、激活、重试和优化建议。成功或失败分别通过 `OnStmtCommit`、`OnStmtRollback` 记录事件并转发；`OnStmtEnd` 只记录时间点。

事务结束时，`OnTxnEnd` 先清除 provider 和当前语句，记录 `txn end`，计算从最近 `resetEvents` 开始的总时长，再依据 `SlowTxnThresholdMs` 判慢。它总会调用 `TraceTxnEnd`，仅慢事务调用 `LogSlowTxn`，最后刷新 `lastInstant`。

## 数据与状态

`TxnManager` 的长期依赖是共享的 `Arc<dyn TxnManagerSession>` 与 `Arc<dyn TxnProviderFactory>`；可变会话状态由调用者通过 `&mut TxnManager` 串行访问。

`provider` 是当前事务上下文的唯一所有者。`EnterNewTxn` 成功后替换它，`OnTxnEnd` 将其置空。`stmtNode` 保存当前语句的自有副本，并在事务结束时清除。`events` 预分配 10 个元素，`resetEvents` 清空但保留容量；每个事件的 `duration` 是 `lastInstant.elapsed()`，不是从事务开始累计的时长。

`enterTxnInstant` 只在构造和 `resetEvents` 时设定，用于总事务时长；`lastInstant` 用于相邻事件间隔。`optimistic_slot` 在每次选择乐观 provider 时于 0、1 间切换，但本文件的工厂返回新的 boxed provider，具体是否复用缓冲完全由工厂实现决定。

`EnterNewTxnRequest::Provider` 具有一次性消费语义：`newProviderWithRequest` 的 `take()` 会把它改为 `None`。陈旧读路径还会先修改会话的 stale-read TS；若随后工厂失败，本文件不负责恢复这个会话字段。

## 依赖与调用关系

直接依赖如下：

- `std::sync::Arc`：共享会话与工厂；`std::time::{Duration, Instant}`：事件及事务耗时。
- crate 根的 `SessionError`、`SessionResult`：统一错误与返回类型。
- `astersql_parser::Normalize`：将当前 SQL 按会话脱敏模式规范化；`pkg/session/Cargo.toml` 声明本地路径依赖 `astersql-parser = ../parser`。
- `pkg/session/lib.rs`：公开模块并仅在测试配置下挂载 `txnmanager_test.rs`。

文件内的主要调用边是：`getTxnManager -> TxnManager::new`；`EnterNewTxn -> newProviderWithRequest -> TxnProviderFactory::*`，以及 `EnterNewTxn -> TxnContextProvider::OnInitialize`；各 `Get*`、`OnStmt*`、`ActivateTxn`、`Advise*`、`SetOptionsBeforeCommit` 再转发到当前 provider；`OnStmtStart/OnStmtCommit/OnStmtRollback/OnTxnEnd -> recordEvent`。

RustCodeGraph 报告目标文件被 88 个文件引用，但精确查询 `TxnManager::EnterNewTxn` 没有返回本文件的 impl 方法节点，而是返回 Go 实现及 `pkg/sessiontxn/interface.rs` 的 trait 方法。文本级调用核验显示，本文件结构体的直接使用在 `pkg/session/txnmanager_test.rs`；`pkg/sessiontxn/interface.rs::{NewTxn, NewTxnInStmt}` 调用的是另一套 trait 对象接口。扩展时必须先确认目标是局部结构体还是 `astersql-sessiontxn` 的公共 trait 主链。

## 错误处理与边界

- `provider_ref`、`provider_mut` 以及依赖它们的严格转发方法在 provider 缺失时返回 `SessionError("context provider not set")`。
- `OnStmtStart`、`OnStmtCommit`、`OnStmtRollback` 在记录事件前显式检查 provider；独立测试 `missing_provider_does_not_record_commit_or_rollback_events` 证明失败不会污染慢事务事件序列。
- `OnStmtErrorForNextAction` 的无 provider 边界不是错误，而是 `Ok(NoIdea)`；`OnLocalTemporaryTableCreated`、`AdviseWarmup`、`AdviseOptimizeWithPlan` 也允许无 provider 并静默成功/忽略。
- `AdviseWarmup` 在 `BulkDMLEnabled` 为真时不调用 provider，保留“优化后再评估 bulk DML”的 Go 语义。
- provider 初始化失败会调用会话回滚并返回原错误，但 provider 工厂创建失败直接返回，不执行该回滚分支。
- `GetTxnInfoSchema` 只有在完全没有 provider 时才回落到 `LatestInfoSchema`；provider 存在但返回 `None` 时不回落。测试 `provider_schema_none_does_not_fall_back_to_latest_schema` 固定了这一不变量。
- 慢事务阈值为 0 时永不判慢；比较以整数毫秒为单位并包含等于阈值的情况。

## 并发与资源生命周期

`TxnManagerSession` 和 `TxnProviderFactory` 要求 `Send + Sync`，以便通过 `Arc` 安全共享；`TxnContextProvider` 只要求 `Send`。`TxnManager` 的变更操作要求 `&mut self`，文件内部没有锁、异步任务、通道或后台资源，预期由会话所有者串行驱动同一个管理器。

provider 生命周期为“创建/取出预置值 -> 初始化 -> 安装 -> 多次语句钩子 -> `OnTxnEnd` 丢弃”。`OnTxnEnd` 没有调用 provider 的显式 close/rollback；资源终止行为只能由 provider 的析构或外层事务流程承担。调用方若在旧事务未结束时再次成功 `EnterNewTxn`，会直接替换旧 provider。

事件缓冲属于管理器并跨事务复用容量。`LogSlowTxn` 接收事件切片，仅在调用期间借用；会话实现若需异步保存必须自行复制。测试中的 `MockSession` 用 `Mutex` 复制事件名，且通过短暂 sleep 使 1ms 阈值稳定触发，但生产文件自身不创建线程。

## 与 Go 版本的对应关系

主要结构与 `pkg/session/txnmanager.go` 对应：两者都持有会话、当前语句、当前 provider、两槽乐观复用概念以及慢事务事件；进入事务都遵循“选 provider、初始化失败回滚、显式 BEGIN 设置 in-txn、记录 enter 事件”的顺序；语句钩子和慢事务日志也保持相同主干语义。

当前 Rust 版本存在明确差异：

- Go `getTxnManager` 从 `SessionVars.TxnManager` 读取并缓存每会话单例；Rust `getTxnManager` 每次新建对象。
- Go 入口类型有 Default、WithBeginStmt、BeforeStmt、WithReplaceProvider；本文件只有前两种。因此 Go 测试 `pkg/sessiontxn/txn_manager_test.go::TestEnterNewTxn` 覆盖的 lazy before-statement 和 replace-provider 进入语义尚不能由此枚举表达。
- Go 直接使用真实 `infoschema.InfoSchema`、`kv.Snapshot`、`kv.Transaction` 和 AST；本文件分别简化为 `Option<String>`、`String`、`String` 和只含原 SQL 的 `StatementNode`。
- Go 乐观 provider 是管理器内嵌的两个实例，并避免新旧事务复用同一槽；Rust 仅交替传递槽位给工厂，未检查旧 provider 是否对应该槽。
- Go 允许空字符串/字符串事务模式并对非法值报错；Rust 强类型 `TxnMode` 把非法模式排除在类型之外。
- Go 的 for-update TS 路径带 failpoint 一致性断言，事务 trace 直接组装 connection/start-TS/statement-count 字段，慢日志直接输出这些字段；Rust 将 trace 和日志交给 `TxnManagerSession`，但当前 `TraceTxnEnter` 参数没有 start TS，`TraceTxnEnd` 参数没有 statement count，且保留的三个 getter 未在本文件使用。
- Go `SetOptionsBeforeCommit` 同时接收真实事务和 checker；Rust provider 契约只接收 checker。

Go 测试提供完整产品语义参照，但不能视作此 Rust 文件已经实现这些缺失分支的证明。Rust 独立测试当前只直接覆盖事件错误边界、SQL 脱敏规范化及 info-schema 不回落行为。

## 扩展指南

若增加进入事务类型或 provider 选择分支，应同时修改 `EnterNewTxnType`、`EnterNewTxnRequest`、`newProviderWithRequest` 和 `TxnProviderFactory`，并在独立的 `pkg/session/txnmanager_test.rs` 增加工厂选择、初始化失败回滚、请求 provider 消费和会话状态断言。不要把 Rust 测试嵌回生产文件。

若要把本结构体接入真实 Rust 会话主链，必须先处理它与 `pkg/sessiontxn/interface.rs::TxnManager` 的类型差异，包括真实 info schema、snapshot/transaction、请求上下文、语句类型和提交事务参数；不应通过删除公共 trait 行为或把真实类型继续压缩为字符串来规避适配。接线后还需验证每会话单例生命周期，而不是沿用当前每次构造的 `getTxnManager`。

若调整事件或慢事务观测，优先修改 `recordEvent`、`resetEvents`、`OnStmtStart` 和 `OnTxnEnd`，并同步验证事件顺序、脱敏内容、阈值 0/边界值以及 provider 缺失时不产生虚假 commit/rollback 事件。性能上应注意事件字符串分配和无上限增长；兼容性上应保持 Go 日志字段及事件顺序可比。

若修改 provider 生命周期，应特别检查初始化失败、工厂失败、旧 provider 替换和 `OnTxnEnd` 丢弃四条路径。若要实现 Go 的双 provider 复用，需要让槽位与 provider 实例所有权具有可验证关系，并覆盖连续事务不复用仍活动实例的测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/txnmanager.rs` 确认目标文件已索引且含 101 个符号。
- RustCodeGraph `node --file pkg/session/txnmanager.rs --offset 1/261`：读取目标文件全部 487 行，核对类型、trait、方法、分支和状态字段。
- RustCodeGraph `query TxnManager`、`query EnterNewTxnType`、`node/callers/callees TxnManager::EnterNewTxn`：确认同名 Go/Rust 接口并发现目标 impl 方法的精确图节点缺失；因此用 `rg` 补查直接 Rust 调用。
- `pkg/session/Cargo.toml`：确认 crate 名为 `astersql-session`、crate 根是 `lib.rs`、`nextgen` feature 与本文件无条件编译，以及 `astersql-parser`、`astersql-sessiontxn*` 的本地依赖声明。
- `pkg/session/lib.rs`：确认 `pub mod txnmanager` 和独立 `#[cfg(test)] mod txnmanager_test`。
- `pkg/session/txnmanager_test.rs`：核对缺 provider 不记录提交/回滚事件、SQL 使用脱敏规范化、provider 返回空 info schema 不回落三个 Rust 回归测试。
- `pkg/session/txnmanager.go`：逐项核对 Go 的 provider 选择、双实例复用、进入/语句/结束流程、trace、慢日志和提交前设置。
- `pkg/sessiontxn/interface.rs`：确认当前 Rust 公共 `TxnManager` trait、`TxnManagerContext`、`GetTxnManager`、`NewTxn` 与 `NewTxnInStmt` 的真实边界。
- `pkg/sessiontxn/txn_manager_test.go`：核对 Go 对 Default、WithBeginStmt、BeforeStmt、WithReplaceProvider、stale read、隔离模式、快照和临时表拦截的预期覆盖。
- 文档结论仅基于静态结构与上述测试源码；遵照任务要求未运行 Cargo，也未把 Go 测试通过情况当作 Rust 运行证据。
