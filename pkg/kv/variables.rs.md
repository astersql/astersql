# `pkg/kv/variables.rs`

## 文件定位

本文件定义 `astersql-kv` crate 中供 KV 重试与锁冲突退避使用的会话侧兼容变量。模块入口 [`pkg/kv/lib.rs`](lib.rs) 以 `#[path = "variables.rs"] mod variables_impl` 纳入文件，并通过 `pub use variables_impl::*` 从 crate 根重新导出 `Variables`、`NewVariables`、`DefBackoffLockFast` 和 `DefBackOffWeight`。[`pkg/kv/Cargo.toml`](Cargo.toml) 表明该 crate 的库入口为 `lib.rs`；本文件自身只使用标准库原子类型，不受 `nextgen` feature 影响，也不直接依赖 TiKV 客户端 crate。

它处于 SQL 会话配置和存储驱动退避器之间：会话层保存同名退避参数，DistSQL 上下文携带参数及 kill 信号，`pkg/store/driver/backoff` 再读取这些值计算退避上限、选择快速锁退避基数并在睡眠后检查中止信号。本文件只定义这组跨层数据契约和一个中止查询方法，不负责系统变量解析、重试调度、睡眠、错误转换或事务提交。

## 核心职责

- 用 `DefBackoffLockFast = 10` 和 `DefBackOffWeight = 2` 固定与 client-go 一致的两个默认退避参数。
- 用 `Variables<'a>` 把快速锁退避基数、最大退避权重和调用方持有的 `AtomicU32` kill 信号组合成一个公开值。
- 用引用生命周期而非拥有或复制 kill 信号，保证持有 `Variables` 的下游观察同一个会话中止状态。
- 用 `IsKilled` 将“0 表示未中止，任意非 0 值表示已中止且数值可编码原因”的信号约定折叠为布尔判断。
- 用 `NewVariables` 提供默认参数初始化，同时保留调用方传入的原子量引用。

该实现是 Go/client-go `Variables` 的兼容子集，而不是当前 client-go 结构的完整镜像：Rust 文件没有 `DisableTxnFile`、`TxnFileMinMutationSize`、`KillSignalHandler` 和 `DefaultVars`。这些缺口必须如实视为当前迁移边界，不能据此宣称完整支持 client-go 的所有会话变量。

## 主要符号

- `pub const DefBackoffLockFast: i32 = 10`：快速锁冲突退避的默认基础参数。存储驱动仅在退避配置名为 `txnLockFast`（大小写不敏感）时用它覆盖调度器基础值。
- `pub const DefBackOffWeight: i32 = 2`：最大累计退避时间的默认放大权重。`TiKvBackoffer::from_parts` 在带变量构造路径中读取它并在溢出保护允许时乘到 `max_sleep_ms`。
- `pub struct Variables<'a>`：公开字段的数据载体。`BackoffLockFast`、`BackOffWeight` 为 `i32`；`Killed: &'a AtomicU32` 把实例生命周期约束在信号所有者之内。该类型没有 `Clone`、`Copy`、`Default` 或内部可变参数更新 API。
- `pub fn Variables::IsKilled(&self) -> bool`：以 `Ordering::SeqCst` 读取 `Killed`，仅判断是否非零，不解释具体 kill reason。
- `pub fn NewVariables(killed: &AtomicU32) -> Variables<'_>`：把两个默认常量与传入的原子引用组装为新值；返回生命周期由 `killed` 的借用推导。

文件没有私有辅助函数、trait、枚举、宏或条件编译项。命名保留 Go 风格的大写字段和函数，以维持迁移代码的对应关系。

## 执行流程

默认构造路径如下：

1. 信号所有者创建 `AtomicU32`，初值通常为 0。
2. 调用 `NewVariables(&killed)`；函数写入默认的 `BackoffLockFast = 10`、`BackOffWeight = 2`，并借用原子量，不复制其当前值。
3. 上游可以在把变量交给下游前修改两个公开退避字段。生产路径也可像 `DistSQLContext::clone`、`DistSQLContext::Detach` 和 `TiKvBackoffer::from_parts` 一样，直接用结构体字面量重建变量。
4. `NewBackofferWithVars` 借用 `Variables` 后，`TiKvBackoffer::from_parts` 读取 `BackOffWeight` 放大最大累计睡眠；执行 `txnLockFast` 退避时读取 `BackoffLockFast` 作为基础参数。
5. 每次实际退避完成后，存储驱动直接读取 `Killed` 的数值；非零时构造带原始 signal 的 `QueryInterruptedWithSignal` 错误。调用 `IsKilled` 的路径则只获得布尔结果。

当前仓库中 RustCodeGraph 确认 `NewVariables` 的直接调用者是 [`pkg/kv/variables_test.rs`](variables_test.rs) 的专项测试与 [`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs) 的组合迁移测试。生产数据流主要使用公开 `Variables` 类型和结构体字面量，而不是强制经过 `NewVariables`。

## 数据与状态

`Variables` 不拥有全局状态。两个退避参数在实例中按值保存；kill 状态留在调用方拥有的 `AtomicU32` 中，实例只持有共享引用。因而多个 `Variables` 可以指向同一信号，信号所有者的后续 `store` 会被所有借用者观察到。

关键不变量是：kill 值为 0 时继续工作，任意非零值都表示应停止，且具体数值可能表示不同中止原因。`IsKilled` 不清零、不消费信号，也不区分 1、2、`u32::MAX` 等原因。独立测试覆盖了这些非零值以及恢复为 0 后重新返回 false 的行为。

两个数值字段均公开，类型层不限制负数、零或异常大值。正常会话入口在 [`pkg/sessionctx/variable/sysvar_builtins.rs`](../sessionctx/variable/sysvar_builtins.rs) 对系统变量施加范围与默认值规则；直接构造者必须自行维持有效值。尤其是下游权重计算包含除以 `BackOffWeight` 的溢出保护，绕过入口写入 0 可能导致除零 panic。

## 依赖与调用关系

crate 内部依赖只有 `std::sync::atomic::{AtomicU32, Ordering}`。模块经 [`pkg/kv/lib.rs`](lib.rs) 公开后，被下列直接层次使用：

- [`pkg/sessionctx/variable/variable.rs`](../sessionctx/variable/variable.rs) 的 `SessionKVVars::default` 读取两个默认常量，为 SQL 会话建立退避默认值。
- [`pkg/sessionctx/variable/sysvar_builtins.rs`](../sessionctx/variable/sysvar_builtins.rs) 注册 `tidb_backoff_lock_fast` 与 `tidb_backoff_weight`，更新会话中的对应字段。
- [`pkg/distsql/context/context.rs`](../distsql/context/context.rs) 的 `DistSQLContext::clone` 复制退避参数并保留原 kill 引用；`Detach` 复制参数，但把 `Killed` 改接到共享 `SQLKiller.Signal`。
- [`pkg/store/driver/backoff/backoff.rs`](../store/driver/backoff/backoff.rs) 通过 `astersql-kv` 依赖借用或构造默认 `Variables`；`from_parts` 消费权重，实际退避调度消费快速锁参数，退避结束检查原始 kill reason。

RustCodeGraph 的 `node variables.rs::NewVariables` 给出两条测试调用边，并给出到两个默认常量和 `Variables` 构造的下游边。精确 `callers/callees` 对 `IsKilled` 在本地索引未返回边，因此生产字段消费和跨 crate 数据流用限定范围源码检索补证；不能把图中缺边解释为类型未被生产代码使用。

## 错误处理与边界

本文件的 API 都不返回 `Result`，没有 I/O、分配或显式 panic 分支。`NewVariables` 只组装字段；`IsKilled` 对所有 `u32` 都有确定结果。实际的中止错误由下游退避器创建，本文件既不保存错误，也不把 kill reason 映射成具体错误类型。

边界行为包括：

- `Killed` 不能为 null，因为 Rust 使用有效引用；这比 Go 的 `*uint32` 更严格。需要“无信号”的下游必须像默认退避器一样提供一个长期存活的零值原子量。
- `Variables` 不能比被借用的原子量活得更久，编译器通过 `'a` 保证这一点；不能从局部信号构造后返回一个逃逸的变量。
- `IsKilled` 使用 `SeqCst`，而当前生产退避器直接读取字段时使用 `Relaxed`。两条入口对“是否非零”的值判断一致，但内存顺序保证不同；需要依赖 kill 之前其他写入可见性的代码应调用 `IsKilled` 或统一下游策略，不能假定所有读取都是顺序一致的。
- 公开数值字段允许绕开系统变量校验；新增调用者必须防止负基数、零权重及乘法边界。下游已有上限乘法溢出检查，但该检查不替代输入有效性约束。

## 并发与资源生命周期

`AtomicU32` 允许多个线程无数据竞争地更新和读取 kill 信号，`IsKilled` 采用最强的 `SeqCst` 顺序。`Variables` 自身没有锁、任务、通道、析构逻辑或后台资源；两个普通整数若要在共享使用开始后修改，调用者必须通过独占访问或外部同步，不能把公开字段当成原子配置。

生命周期 `'a` 是本文件最重要的资源约束：`Variables` 借用而不拥有信号。`TiKvBackoffer<'a>` 继续借用该变量；`DistSQLContext<'a>` 则在 clone 时复用信号，在 detach 时把信号重新绑定到生命周期合适的 `SQLKiller.Signal`。默认退避路径使用静态 `DEFAULT_KILLED`，从而得到 `Variables<'static>`。这些设计避免悬垂引用，也意味着不能在仍有下游借用时销毁或替换信号所有者。

## 与 Go 版本的对应关系

仓库同路径 [`pkg/kv/variables.go`](variables.go) 只是门面：`Variables` 是 `github.com/tikv/client-go/v2/kv.Variables` 的类型别名，`NewVariables` 直接委托 client-go。`go.mod` 固定的 client-go 版本中，默认常量同样为 10 和 2，构造函数同样保存调用方的 kill 指针，并约定 0/非 0 表示未中止/已中止。

Rust 保持的语义包括两个字段名和类型宽度意图、默认值、非零 kill 判断，以及共享而非快照式的信号连接。差异如下：

- Go 构造函数返回指针，Rust 返回按值结构；Rust 内部仍通过引用共享 kill 信号。
- Go kill 指针可以为 `nil`，Rust `&AtomicU32` 不可为空。
- Go 的原子读取对应顺序一致语义，`IsKilled` 因而使用 `SeqCst`；但 Rust 存储退避器的直接字段读取当前使用 `Relaxed`。
- 当前 client-go 结构还包含 `DisableTxnFile`、`TxnFileMinMutationSize`、`KillSignalHandler`，并提供 `DefaultVars`；本 Rust 文件均未移植。会话层虽有 txn-file 配置字段，但它们不属于此 `Variables` 类型，不能认为已经通过本契约传到存储退避器。
- Go 同路径没有独立行为测试；Rust 回归位于单独的 [`pkg/kv/variables_test.rs`](variables_test.rs)，符合测试与源文件分离要求。

## 扩展指南

若新增变量字段，应先确认目标是对齐 `pkg/kv/variables.go` 门面背后的 client-go 契约，还是 AsterSQL 自有配置。对齐字段需要同步检查至少四处：本文件的结构和 `NewVariables` 默认值、`TiKvBackoffer::from_parts` 的默认结构体字面量、`DistSQLContext::clone/Detach` 的重建逻辑、以及会话 `SessionKVVars` 到 DistSQL/KV 变量的转换。漏改任一字面量会造成编译失败或跨层语义丢失。

新增 kill handler 或改变内存顺序时，必须明确优先级、并发初始化时机和错误传播位置，并统一审查 `IsKilled` 与存储退避器的直接 `Killed.load`，避免同一信号出现不同可见性保证。新增默认实例时，应使用具有足够生命周期的静态原子量，不能用临时局部值伪造 `'static`。

测试应继续放在独立文件，不得内嵌进 `variables.rs`。至少同步扩展 [`pkg/kv/variables_test.rs`](variables_test.rs)，覆盖默认值、共享信号、全部新增字段及边界；涉及退避消费时同步 [`pkg/store/driver/backoff/backoff_test.rs`](../store/driver/backoff/backoff_test.rs)；涉及 detach/clone 时同步 [`pkg/distsql/context/context_test.rs`](../distsql/context/context_test.rs)。兼容性风险集中在默认值和字段传播不一致，并发风险集中在信号生命周期与原子顺序，性能风险较低但每次退避检查处于重试热路径。

## 验证依据

本说明使用以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/kv` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph `query DefBackoffLockFast`、`query IsKilled`、`query NewVariables --json`：定位目标常量与 `variables.rs::IsKilled`、`variables.rs::NewVariables`；`node variables.rs::NewVariables` 显示它构造 `Variables`、引用两个默认常量，并由两个 Rust 测试调用。
- 目标与入口：[`pkg/kv/variables.rs`](variables.rs)、[`pkg/kv/lib.rs`](lib.rs)、[`pkg/kv/Cargo.toml`](Cargo.toml)。目标文件共两个常量、一个结构体、一个方法和一个自由函数，无条件编译项。
- Go 对照：[`pkg/kv/variables.go`](variables.go)、`go.mod` 固定版本对应的 client-go `kv/variables.go`。
- 直接生产链：[`pkg/sessionctx/variable/variable.rs`](../sessionctx/variable/variable.rs)、[`pkg/sessionctx/variable/sysvar_builtins.rs`](../sessionctx/variable/sysvar_builtins.rs)、[`pkg/distsql/context/context.rs`](../distsql/context/context.rs)、[`pkg/store/driver/backoff/backoff.rs`](../store/driver/backoff/backoff.rs)。
- 独立测试：[`pkg/kv/variables_test.rs`](variables_test.rs) 的 `new_variables_matches_client_go_defaults_and_kill_reasons`，以及 [`pkg/kv/mpp_2_aster_unit_test.rs`](mpp_2_aster_unit_test.rs) 的 `mpp_2_versions_and_client_variables_match_go`。

本任务是纯文档分析，按计划未运行 Cargo。交付验证只检查目标文档存在、固定二级标题恰好为 11 个，并人工复核链接、符号与当前代码事实。
