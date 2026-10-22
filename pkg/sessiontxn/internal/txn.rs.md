# `pkg/sessiontxn/internal/txn.rs`

源文件：[`txn.rs`](txn.rs)

## 文件定位

`SessionTxnContext` 将上述逻辑所需的会话能力收窄为九个读取/操作方法，使真实会话适配器与独立测试替身能够复用相同逻辑。

## 核心职责

1. `set_txn_assertion_level` 将会话变量枚举 `variable::AssertionLevel` 一一映射为 RPC 层 `kvrpcpb::AssertionLevel`，并以 `kv::AssertionLevel` 选项写入事务。
2. `commit_before_enter_new_txn` 在不激活事务的前提下查询旧事务；仅当旧事务有效时提交，并在提交成功后记录 schema 版本、旧事务 StartTS 和事务作用域。
3. `get_snapshot_with_ts` 用指定时间戳构造快照，再按与 Go 相同的顺序传播非默认的拦截器、内部请求标记、请求来源和基于负载的副本读阈值。
4. `SessionTxnContext` 将上述逻辑所需的会话能力收窄为九个读取/操作方法，使真实会话适配器与独立测试替身能够复用相同逻辑。

本文件不负责事务时间戳申请、事务激活、隔离级别决策、快照实际读写或提交协议；这些能力均由调用方和 `kv::Transaction`/`kv::Snapshot` 实现提供。

## 主要符号

- `pub type SessionTxnError = sessionctx::GoError`：沿用 sessionctx 的 Go 风格动态错误类型，作为事务查询与提交的统一错误通道。
- `pub trait SessionTxnContext`：最小会话事务边界。
  - `txn(false)` 取得当前事务且不得强制激活；返回可变事务借用或错误。
  - `commit_txn` 执行提交；`txn_scope`、`schema_meta_version` 为成功日志提供上下文。
  - `get_snapshot` 创建指定 `kv::Version` 的快照。
  - `in_restricted_sql`、两个 request-source getter 和 `load_based_replica_read_threshold` 提供需传播的会话选项。
- `pub fn set_txn_assertion_level(&mut dyn kv::Transaction, variable::AssertionLevel)`：穷举 Off/Fast/Strict 三种输入；每次恰好调用一次 `Transaction::SetOption`。
- `pub fn commit_before_enter_new_txn<C: SessionTxnContext + ?Sized>(...) -> Result<(), SessionTxnError>`：泛型允许普通实现和 trait object；以短作用域释放 `txn()` 返回的可变借用后再访问/提交 `sctx`。
- `pub fn get_snapshot_with_ts<C: SessionTxnContext + ?Sized>(..., ts: u64, interceptor: Option<Box<dyn kv::SnapshotInterceptor>>) -> Box<dyn kv::Snapshot>`：始终返回已创建的快照，按条件设置零到五个选项。

文件没有模块级常量、struct、enum、`unsafe` 或条件编译项；选项键和精简 KV trait 定义在相邻 `lib.rs`。

## 执行流程

`set_txn_assertion_level` 的流程是：匹配会话断言枚举，将 `AssertionLevelOff/Fast/Strict` 转成 `Off/Fast/Strict`，随后调用 `txn.SetOption(kv::AssertionLevel, Some(...))`。匹配没有兜底分支，因此枚举未来新增成员时编译器会强制维护映射。

`commit_before_enter_new_txn` 的流程是：

1. 调用 `sctx.txn(false)`；错误立即通过 `?` 返回。
2. 在局部借用范围内检查 `txn.Valid()`；有效时保存 `txn.StartTS()`，无效时得到 `None`。
3. 无效事务直接返回 `Ok(())`，不提交、不读取日志字段。
4. 有效事务先复制 `txn_scope`，再调用 `commit_txn(ctx)`；提交错误立即返回且不会写成功日志。
5. 提交成功后读取 `schema_meta_version`，连同保存的 StartTS/scope 写一条 info 日志，最后返回成功。

`get_snapshot_with_ts` 的流程是：

1. 将 `ts` 包装为 `kv::Version { Ver: ts }` 并调用 `get_snapshot`。
2. 拦截器为 `Some` 时写 `kv::SnapInterceptor`。
3. 受限 SQL 时写 `kv::RequestSourceInternal = true`。
4. 普通来源字符串非空时写 `kv::RequestSourceType`；显式来源字符串非空时写 `kv::ExplicitRequestSourceType`。
5. 阈值非零时写 `kv::LoadBasedReplicaReadThreshold`，随后返回快照。

选项顺序不仅便于对照 Go，独立测试也将其作为可观察行为逐项断言。

## 数据与状态

本文件自身不持有长期状态。所有可变状态都在调用者提供的事务、快照或会话上下文中：

- 断言级别写入事务选项，影响后续 KV 写路径的前置条件校验策略。
- 旧事务的有效标志和 StartTS 从 `kv::Transaction` 读取；提交产生的事务状态变化由 `commit_txn` 实现负责。
- 快照版本由无符号 `u64` 时间戳原样包装；`ts == 0` 没有被本层拒绝，测试确认它仍传给 `get_snapshot`。
- 请求来源字符串在写入快照时复制为拥有所有权的 `String`；阈值以 `Duration` 值传递。
- `Option<Box<dyn SnapshotInterceptor>>` 将拦截器所有权移交给快照选项；无拦截器时不写占位值。

关键不变量是“默认值不产生快照选项”：`None`、`false`、空字符串和零 `Duration` 均被省略。断言级别则没有省略路径，三种合法会话值都会显式写入。

## 依赖与调用关系

直接依赖如下：

- `crate::kv`：提供 `Transaction`、`Snapshot`、`SnapshotInterceptor`、`Version` 与各选项键，实际定义在 `pkg/sessiontxn/internal/lib.rs`。
- `crate::kvrpcpb`：提供精简 `AssertionLevel` RPC 枚举。
- `crate::sessionctx`：提供 `ExecutionContext` 与 `GoError`；由 Cargo 中的 `astersql-sessionctx` 重命名重导出。
- `crate::variable`：提供会话 `AssertionLevel`；由 `astersql-sessionctx-variable` 重命名重导出。
- `std::time::Duration` 与 `log::info!`：分别表达阈值和成功日志。

RustCodeGraph 对三个公开函数的精确查询均定位到本文件及 `migration_aster_unit_test.rs` 的对应测试。callers/callees 查询未返回可用的跨文件调用边；`rg` 的符号级复核也只发现独立测试直接调用本文件 API。Cargo 反向依赖存在于 `pkg/sessiontxn/Cargo.toml`、`pkg/sessiontxn/isolation/Cargo.toml`、`pkg/sessiontxn/staleread/Cargo.toml`，但依赖声明不等价于符号已经接线。

Go 版本的调用链证据更完整：`isolation/base.go::OnInitialize` 和 `staleread/provider.go::activateStaleTxn` 调用 `CommitBeforeEnterNewTxn`；两类 provider 的事务激活配置调用 `SetTxnAssertionLevel`；快照获取路径调用 `GetSnapshotWithTS` 后再追加 replica-read、staleness 等 provider 专属选项。

## 错误处理与边界

- `set_txn_assertion_level` 与 `get_snapshot_with_ts` 的精简 trait API 不返回错误，因此本层无法恢复 `SetOption` 或快照构造失败；具体实现若可能失败，必须在 trait 边界设计中显式引入错误，而不能假定此文件会处理。
- `commit_before_enter_new_txn` 只传播两类错误：`txn(false)` 查询失败和 `commit_txn` 提交失败。测试分别验证错误文本原样保留，并验证查询失败时零次提交、提交失败时恰有一次提交尝试。
- 无效旧事务不是错误，函数直接成功退出。
- 成功日志发生在提交之后；因此日志字段读取不参与是否提交的决策，也不会错误地把失败提交记录为成功路径。
- `ts == 0`、空来源、零阈值均是允许输入；只有后面三类默认选项被省略，快照本身仍会构造。
- 本文件不验证来源字符串语义、时间戳可见性、scope 合法性或断言级别与具体存储能力的兼容性，这些属于上游会话策略或下游 KV 实现边界。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`commit_before_enter_new_txn` 接受 `&mut C`，在 Rust 类型层面排除同一上下文在调用期间的并发可变访问；它还刻意将事务可变借用限制在内部块中，先提取 StartTS，再提交上下文，避免事务借用跨越 `commit_txn`。

`get_snapshot_with_ts` 只需要 `&C`，但线程安全性仍取决于具体 `SessionTxnContext` 与返回快照实现；trait 没有 `Send`/`Sync` 约束。拦截器由 `Box` 独占并移动进快照，快照也以 `Box<dyn Snapshot>` 交给调用者管理。独立测试使用 `Rc<RefCell<_>>` 记录状态，进一步说明当前抽象允许单线程、非 `Send` 的实现，不能据此声称生产并发安全。

提交调用是同步边界：函数返回前提交已经成功或错误已经传播。文件不实现重试、超时、取消或回滚；调用者必须决定失败后的会话状态和后续动作。

## 与 Go 版本的对应关系

Go 注释要求断言级别在新事务创建后只设置一次；Rust 函数本身同样不强制调用次数，因此该约束仍由调用方保证。

Rust 以 `ExecutionContext` 代替 Go `context.Context`，以 `SessionTxnContext` getter 代替对 `GetSessionVars()`/`GetInfoSchema()` 的直接深层访问；行为测试覆盖有效性与两种错误传播。

Rust 用 `Option<Box<dyn SnapshotInterceptor>>` 表达 Go 的 nil interface，用空字符串和 `Duration::is_zero()` 表达 Go 的 `!= ""` 与 `> 0` 条件。当前 Rust 精简 `kv` trait 仅保留本文件需要的选项，而 Go 返回的完整 `kv.Snapshot` 随后还会由 provider 设置副本读、staleness 等选项。

迁移差异的核心不是这三个局部算法，而是接线方式：Go 函数直接接受完整 `sessionctx.Context` 并处于 provider 主链；Rust 使用待适配的窄 trait，当前直接调用证据只存在于 `migration_aster_unit_test.rs`。

## 扩展指南

- 新增断言级别时，应同步修改 `variable::AssertionLevel`、`kvrpcpb::AssertionLevel`、`set_txn_assertion_level` 的穷举映射，并扩展 `set_txn_assertion_level_maps_all_go_levels`。同时核对 Go 枚举和 `SetTxnAssertionLevel`，避免编号/语义漂移。
- 新增需要传播到快照的会话选项时，应先向 `SessionTxnContext` 添加最小 getter，再在 `get_snapshot_with_ts` 中按 Go 顺序和默认值规则设置；同步扩展 `MockContext`、`decode_option` 以及“全量传播/默认省略”两个独立测试。要评估选项值装箱类型和下游解码兼容性。
- 修改进入新事务前的提交规则时，应优先调整 `commit_before_enter_new_txn`，保持 `txn(false)` 不激活不变量，并为无效、有效、查询失败、提交失败增加或更新回归断言。若日志时机或字段改变，也要核对 Go 对照。
- 将本辅助层接入 Rust 生产主链时，需要为真实会话实现 `SessionTxnContext`，并明确它与 `isolation/base.rs::IsolationRuntime::commit_before_enter_new_txn`、`staleread/provider.rs` backend 的职责归属，避免重复提交或两套选项传播。接线应放在独立生产文件中，测试仍放在独立 `*_test.rs` 文件，不能内嵌进 `txn.rs`。
- 性能风险主要来自每次快照构造的字符串复制、动态分发与 `Any` 装箱；兼容风险主要来自选项键、值类型、默认值判定和设置顺序与 Go/KV 实现不一致。任何优化都应先保留现有独立测试可观察语义。
