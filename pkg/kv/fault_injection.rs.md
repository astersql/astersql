# `pkg/kv/fault_injection.rs`

## 文件定位

[`pkg/kv/fault_injection.rs`](fault_injection.rs) 属于 `astersql-kv` crate。`pkg/kv/Cargo.toml` 以 `lib.rs` 为库入口；`pkg/kv/lib.rs` 在公开模块 `fault_injection` 中通过 `include!("fault_injection.rs")` 装入本文件，再用 `pub use fault_injection::*` 将公开项提升到 crate 根。因此调用方通常使用 `kv::InjectionConfig`、`kv::NewInjectedStore`，不必写完整模块路径。

本文件位于 KV 抽象层而不是某个具体 TiKV 客户端实现中：它接收任意 `Box<dyn Storage>`，返回仍满足 `Storage` 的装饰器，并在 `Begin`、`GetSnapshot` 产生的事务和快照外继续包一层。它的用途是让测试或混沌场景在不改变真实存储实现的前提下，稳定模拟单键读、批量读和提交失败。

文件没有 feature gate 或其他条件编译项，属于 crate 的常规公开 API。独立测试由 `pkg/kv/lib.rs` 的 `#[cfg(test)] #[path = "fault_injection_test.rs"]` 装入；生产源码没有内嵌测试。

## 核心职责

- `InjectionConfig` 保存两类可在运行时切换的故障：`getError` 同时控制事务和快照的 `Get`/`BatchGet`，`commitError` 只控制事务 `Commit`。
- `NewInjectedStore`、`InjectedStore` 保持 `Storage` 接口不变；只在创建事务或快照时把同一份 `Arc<InjectionConfig>` 传入下层包装器。
- `InjectedTransaction` 仅短路 `Get`、`BatchGet`、`Commit`。事务迭代、写入、回滚、悲观锁、公平锁、选项、时间戳、内存缓冲、表信息、磁盘满策略、checkpoint、pipeline flush 等操作全部委托给底层事务。
- `InjectedSnapshot` 仅短路 `Get`、`BatchGet`；迭代和 `SetOption` 透明委托给底层快照。
- 未配置错误时，包装器不自行合成结果，而是回落到底层实现，所以真实返回值、错误和副作用仍由被包装对象决定。

## 主要符号

- `pub type SharedError = Error`：本文件使用的共享错误别名。配置保存可克隆的 `Error`，命中注入分支时返回其克隆。
- `struct InjectionErrors`：私有状态容器，包含 `getError: Option<SharedError>` 与 `commitError: Option<SharedError>`；`Default` 表示两项均未注入。
- `pub struct InjectionConfig`：以 `RwLock<InjectionErrors>` 保护配置。`SetGetError` 与 `SetCommitError` 分别设置或用 `None` 清除对应错误。
- `pub fn NewInjectedStore(store, cfg) -> Box<dyn Storage>`：唯一构造入口，把底层 `Storage` 与共享配置装入 `InjectedStore`。
- `pub struct InjectedStore`：持有公开字段 `Storage: Box<dyn Storage>` 和私有 `cfg`。其 `Storage::Begin`/`GetSnapshot` 创建注入版子对象；其余 `Storage` 方法逐项转发。
- `pub struct InjectedTransaction`：持有公开字段 `Transaction: Box<dyn Transaction>` 和共享配置。它实现 `Getter`、`Retriever`、`Mutator`、`RetrieverMutator`、`FairLockingController`、`Transaction`，从而仍可作为完整事务 trait object 使用。
- `pub struct InjectedSnapshot`：持有公开字段 `Snapshot: Box<dyn Snapshot>` 和共享配置。它实现 `Getter`、`Retriever`、`Snapshot`。

这些符号均未声明额外泛型或条件编译。`InjectedStore`、`InjectedTransaction`、`InjectedSnapshot` 没有公开构造函数之外的固有方法；行为由对应 trait 实现定义。

## 执行流程

1. 调用方创建 `Arc<InjectionConfig>`，通过 `SetGetError(Some(err))` 或 `SetCommitError(Some(err))` 启用故障，再把底层 `Box<dyn Storage>` 与配置交给 `NewInjectedStore`。
2. `InjectedStore::Begin` 先调用底层 `Storage::Begin(opts)`；成功后才构造 `InjectedTransaction`，并用 `Arc::clone` 共享当前配置。`GetSnapshot(ver)` 同理先取得底层快照，再构造 `InjectedSnapshot`。
3. 调用事务或快照的 `Get` 时，包装器获取配置读锁。若 `getError` 为 `Some`，立即返回该错误的克隆，不触达底层对象；否则在仍持有读锁的作用域内调用底层 `Get`。
4. 事务与快照的 `BatchGet` 使用完全相同的 `getError` 判定。因此不存在“只让单键读失败”或“只让批量读失败”的独立开关。
5. 调用事务 `Commit` 时读取 `commitError`；配置存在则短路返回，配置为空则委托到底层 `Commit`。这个开关与读错误互不影响。
6. `SetGetError(None)`/`SetCommitError(None)` 清除故障后，已经由该 store 创建的事务和快照也会在下一次受控操作中看到新配置，因为所有包装器共享同一个 `Arc`，而不是在创建时复制错误值。
7. 所有不属于上述三个故障点的方法直接转发；例如 `Set` 后仍只改变底层事务，`Rollback` 仍由底层决定结果，迭代操作也不会检查 `getError`。

## 数据与状态

持久状态只有 `InjectionConfig.errors`。其两个 `Option<Error>` 分别形成读故障域和提交故障域：读故障域跨事务、快照以及单键/批量 API，共享同一错误；提交故障域只覆盖事务提交。配置本身不记录调用次数、目标 key、事务 ID、版本或一次性消费状态，因此一次启用会持续影响所有共享该配置的包装对象，直到调用 setter 覆盖或清除。

包装层保存的是 trait object 所有权：store 拥有 `Box<dyn Storage>`，每次 `Begin`/`GetSnapshot` 返回的新包装器分别拥有底层 `Box<dyn Transaction>`/`Box<dyn Snapshot>`。配置通过 `Arc` 共享，底层对象则没有在本文件内额外共享。字段 `Storage`、`Transaction`、`Snapshot` 是公开的，允许调用方显式访问被包装对象；`cfg` 私有，配置只能通过创建时传入的 `Arc` 以及公开 setter 改写。

`Default` 配置等价于完全透明模式。注入错误不改变底层状态：读和提交在进入底层方法前即被短路；例如提交故障不会自动回滚、关闭或使事务失效，后续生命周期仍由调用方与底层事务决定。

## 依赖与调用关系

上游装配由 `pkg/kv/lib.rs` 完成；同文件的 `use crate::*` 让本文件获得 `Storage`、`Transaction`、`Snapshot`、`Getter`、`Retriever`、`Mutator`、`Error`、`Key`、`ValueEntry`、选项类型以及 `context`、`tikv`、`oracle`、`model`、`kvrpcpb`、`deadlockpb` 等 crate 内类型。`pkg/kv/Cargo.toml` 声明该 crate 名为 `astersql-kv`，本文件本身没有引入额外第三方依赖；显式导入仅有标准库 `Any`、`HashMap`、`Arc`、`RwLock`。

RustCodeGraph 将目标文件识别为被 8 个文件使用，但针对重名 API 的 `callers` 未产生可用静态边。全仓 Rust 精确搜索显示 `NewInjectedStore`/setter 的直接调用只在 `pkg/kv/fault_injection_test.rs::test_fault_injection_basic` 和 `pkg/kv/txn_test.rs::test_retry_exceed_count_error`；当前没有证据表明生产主链直接构造该包装器。因此它是公开、完整且受测试的测试辅助边界，不能据现有证据描述为生产请求默认经过的层。

下游关系分为两类：`Begin`、`GetSnapshot` 产生包装后的动态分派对象；其余绝大多数方法一对一调用同名底层 trait 方法。只有 `Get`、`BatchGet`、`Commit` 在委托前读取 `InjectionConfig` 并可能终止调用链。修改这些 trait 的签名会要求本文件同步全部转发实现，而增加新的故障点则应在最窄的对应 wrapper 方法中接入。

## 错误处理与边界

- 注入分支返回配置错误的 `clone`，不包装、不改写消息，也不执行底层调用。测试用错误字符串相等验证同一注入语义，而不是依赖对象地址。
- `InjectedStore::Begin` 使用 `?` 传播底层错误；失败时不会构造或返回 `InjectedTransaction`。其他透明转发方法也保持底层错误原样。
- 受控读和提交若遭遇 `RwLock` 中毒，会用 `errors::New(err.to_string())` 转为 crate `Error` 并返回；此时不会调用底层操作。
- 两个 setter 使用 `if let Ok(...)`。写锁中毒时它们静默放弃更新，也没有返回值通知调用方。这与受控操作显式返回锁错误不对称，是兼容时必须保留或有意修改的边界。
- `getError` 不覆盖 `Iter`/`IterReverse`，也不覆盖事务写入与锁操作；`commitError` 不覆盖 `Rollback` 或 `MayFlush`。新增测试不能把当前实现概括成“所有 KV 操作都失败”。
- 配置不支持按 key、版本、调用次数或上下文过滤，也不支持延迟、panic、部分结果与一次性故障。空 key、空 key 列表和取消的 context 在命中注入时均不会进入底层，由注入错误优先返回。
- 包装器允许外部通过公开的底层字段绕开注入层；需要强制故障语义的调用方应只经返回的 trait object 操作，不能直接调用这些字段。

## 并发与资源生命周期

`Arc<InjectionConfig>` 让 store、它产生的所有事务与快照共享配置；`RwLock` 允许多个受控读/提交并发检查错误，setter 取得独占写锁。各 getter/commit 中读 guard 的词法作用域覆盖了随后的底层调用，所以在未注入、操作被转发时，setter 会等待底层 `Get`、`BatchGet` 或 `Commit` 返回。这与 Go 实现中 `defer RUnlock()` 覆盖底层调用的锁范围一致，但意味着慢底层操作会延迟配置切换。

setter 完成写锁更新后，后续成功取得读锁的调用会看到新值。实现不承诺正在执行的底层操作会被中途改成失败，也没有原子计数或每事务快照配置。多个 setter 竞争时，以最终取得写锁并写入的值为准。

本文件不创建线程、异步任务、通道或后台资源，也没有独立 `close`。`InjectedStore::Close` 直接关闭底层 store；wrapper 被 drop 时由 `Box` 释放底层对象，由 `Arc` 引用计数管理配置。事务提交故障不会自动释放事务，快照读故障也不会改变快照版本。

## 与 Go 版本的对应关系

直接对照是 `pkg/kv/fault_injection.go`。两版都有 `InjectionConfig`、两个 setter、`InjectedStore`、`InjectedTransaction`、`InjectedSnapshot` 和 `NewInjectedStore`；都让读错误同时覆盖事务/快照的 `Get`、`BatchGet`，让提交错误只覆盖事务 `Commit`，并在锁保护下持有读锁直到底层调用结束。其余方法依靠 Go 接口嵌入或 Rust 显式逐项转发实现透明包装。

所有权表达不同但职责对应：Go 以嵌入字段 `Storage`/`Transaction`/`Snapshot` 自动提升未重写方法，Rust 必须分别实现完整 trait 并显式转发；Go 共享 `*InjectionConfig`，Rust 用 `Arc<InjectionConfig>`；Go 的 `error` 可为 `nil`，Rust 用 `Option<Error>`；Go `sync.RWMutex` 没有锁中毒返回，Rust 对中毒分别采用“受控操作返回错误、setter 静默不更新”的策略。

`Begin` 的错误表达有语言层差异：Go 构造 `InjectedTransaction` 并与底层 `err` 一起返回，即便 `err != nil`；Rust `Result` 用 `?` 在错误时只返回 `Err`，成功时才有包装事务。对遵循“错误时不使用事务”的调用方语义等价，但不能声称两版在错误返回值形状上完全相同。

`pkg/kv/fault_injection_test.go::TestFaultInjectionBasic` 与 Rust `pkg/kv/fault_injection_test.rs::test_fault_injection_basic` 覆盖相同主线：启用两个错误后，事务/快照单读、批量读和提交均失败；清除后回到底层 mock 的空读、未找到快照和可重试提交错误。Rust 测试另验证两个默认 `TxnOption` 能穿过 `Begin`。`pkg/kv/txn_test.rs` 还证明包装 store 可进入 `RunInNewTxn` 的重试/失败路径。

## 扩展指南

- 若新增一种可注入错误，先在私有 `InjectionErrors` 增加独立字段，在 `InjectionConfig` 增加设置/清除入口，再只在目标 wrapper 方法委托前检查；不要把无关操作错误地并入 `getError` 或 `commitError`。
- 若需要按调用次数、key、版本或事务筛选，应明确状态更新发生在读锁还是写锁下，并补充并发不变量。当前所有受控操作只读配置；引入计数会改变锁竞争和可重复性。
- 修改 `Storage`、`Transaction`、`Snapshot` 或其父 trait 时，必须同步检查本文件的完整转发列表，避免新方法绕过包装或 trait 实现不完整。公开底层字段是否继续允许绕过注入也应作为兼容决定记录。
- 行为测试应继续放在独立 `pkg/kv/fault_injection_test.rs`，并同步核对 Go 的 `pkg/kv/fault_injection_test.go`。与事务重试的集成影响放在现有 `pkg/kv/txn_test.rs`，不要把测试写回生产 `.rs`。
- 对锁处理的任何重构都要覆盖中毒策略、setter 与慢底层调用的竞争、清除配置后既有事务/快照立即观察新值，以及多个 wrapper 共用配置。缩短读锁持有范围可能改善配置切换延迟，但会偏离当前 Go 的锁范围，需要明确兼容依据。
- 性能风险主要来自每次受控操作的一次 `RwLock` 获取与注入错误克隆；未受控的众多转发方法没有额外锁。新增通用拦截层时应避免让所有 KV 操作无条件承担更重的同步或分配成本。

## 验证依据

- RustCodeGraph `status`：索引包含 11467 个文件、307296 个节点、1848419 条边；`node --file pkg/kv/fault_injection.rs` 完整读取 397 行目标源码并报告 8 个引用文件。
- RustCodeGraph 精确查询：`query NewInjectedStore --kind function`、`query InjectionConfig --kind struct`、`query InjectedStore --kind struct`、`query InjectedTransaction --kind struct`、`query InjectedSnapshot --kind struct` 均同时定位 Rust 与同路径 Go 符号。`callers NewInjectedStore`、`callers SetGetError`、`callers SetCommitError` 未返回可用边，因此调用点按技能规则用全仓 Rust 精确搜索补证。
- 已读生产与装配文件：`pkg/kv/fault_injection.rs`、`pkg/kv/lib.rs`、`pkg/kv/Cargo.toml`；目标包不存在 `pkg/kv/doc.go`，故无额外包契约文件可读。
- 已读 Go 对照：`pkg/kv/fault_injection.go`；已读 Go 测试：`pkg/kv/fault_injection_test.go`。
- 已读独立 Rust 测试：`pkg/kv/fault_injection_test.rs`、`pkg/kv/txn_test.rs`；测试依赖的 mock 行为由 `pkg/kv/lib.rs` 中 `MockStorage`、`MockTxn`、`MockSnapshot`、`MockMap` 的实现核对。
- 本任务是纯文档分析，按计划未运行 Cargo。交付检查使用任务指定的结构命令验证文档存在且恰有 11 个固定二级标题，并人工复核文件定位、实际接线、Go 差异和扩展建议均由上述直接证据支持。
