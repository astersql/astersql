# `pkg/testkit/testfailpoint/failpoint.rs`

## 文件定位

本文件是 workspace crate `astersql-testkit-testfailpoint` 的核心实现，源码为 [`failpoint.rs`](./failpoint.rs)，crate 根 [`lib.rs`](./lib.rs) 通过 `pub mod failpoint` 声明模块并以 `pub use failpoint::*` 将全部公开 API 提升到 crate 根。因而调用方通常写成 `astersql_testkit_testfailpoint::enable(...)`，而不是直接引用内部模块路径。

它位于测试基础设施层，但不是只在 `cfg(test)` 下编译的测试文件：生产实现中的故障注入边界也会调用 `inject`、`inject_value`、`eval_bool`、`eval_string` 或 `is_active`，测试再通过同一 crate 注册对应行为。例如 [`pkg/session/runtime/ddl.rs`](../../session/runtime/ddl.rs) 调用 `inject_value`/`inject`，[`pkg/session/runtime/control.rs`](../../session/runtime/control.rs) 调用 `eval_string`/`is_active`。因此它不参与正常 SQL 业务决策；未注册 failpoint 时，它应让这些边界保持无注入效果。

[`Cargo.toml`](./Cargo.toml) 指定库入口为 `lib.rs`，唯一直接依赖是启用 `failpoints` feature 的 `fail = 0.5.1`，并用 `package.metadata.porting.go-package = "pkg/testkit/testfailpoint"` 记录 Go 对照包。根 [`Cargo.toml`](../../../Cargo.toml) 将本 crate 纳入 workspace，多个会话、DDL、执行器和 RealTiKV 测试 crate 以路径依赖复用它。

## 核心职责

本文件提供四组能力：

1. `enable`、`enable_call`、`disable`、`FailGuard` 对 `fail` crate 做薄封装，把配置错误转换为立即 panic，并用 RAII 守卫替代 Go 测试的 `t.Cleanup`。
2. `eval_bool`、`eval_string`、`inject`、`is_active` 为运行时代码提供布尔/字符串求值、纯副作用触发和只读注册状态检查。
3. `enable_concurrent_call` 与 `ConcurrentCallGuard` 用自建全局表支持同一回调体在多个注入线程中重叠执行；`enable_value_call`、`inject_value` 与 `ValueCallGuard` 补足上游 `fail` crate 不支持“把运行时参数传给回调”的能力。
4. `enable_pause` 与 `PauseGuard` 用互斥锁、条件变量和回调 failpoint 构造测试线程与被测线程之间的单向暂停栅栏。

这些辅助接口都按 failpoint 名称关联注册方和注入点。名称是跨 crate 的行为契约；本文件既不集中声明名称，也不校验某个注入点是否真实存在。

## 主要符号

- `pub use fail::FailGuard`：公开底层 RAII 守卫。`enable` 和 `enable_call` 返回它；守卫存活期间条目有效，销毁时由 `fail` crate 清理。
- `enable(name: &str, expression: &str) -> FailGuard`：调用 `FailGuard::new` 注册表达式，例如 `return(true)` 或带次数的 `1*return(value)`；错误经 `unwrap_or_else` 变为包含名称和原错误的 panic。
- `enable_call<F>(name, callback) -> FailGuard`：调用 `FailGuard::with_callback` 注册无参数的 `Fn() + Send + Sync + 'static` 回调，配置失败同样 panic。
- `ConcurrentCallback`、`concurrent_callbacks()`、`ConcurrentCallGuard`、`enable_concurrent_call`：维护 `OnceLock<Mutex<HashMap<String, (u64, Arc<dyn Fn() + Send + Sync>)>>>`。每次注册取得递增标识并覆盖同名旧值；守卫销毁时仅在表内标识仍与自身一致时删除，避免旧守卫误删后来覆盖的注册。
- `ValueCallback`、`value_callbacks()`、`ValueCallGuard`、`enable_value_call`、`inject_value`：与并发回调表采用相同的“名称 + 注册标识 + `Arc` 回调”模式，但回调签名为 `Fn(&str)`，注入端显式传入字符串值。
- `disable(name)`：调用 `fail::remove(name)`。它只操作底层 `fail` 注册表；不会删除 `concurrent_callbacks` 或 `value_callbacks` 中的自建条目。
- `inject(name)`：先从并发回调表复制 `Arc` 并在锁外调用，再执行 `fail::eval(name, |_| ())`。若同一名称同时存在两类注册，两条路径都会执行，顺序固定为自建并发回调在前、底层 `fail` 求值在后。
- `eval_bool(name) -> bool`：用 `fail::eval` 取得可选字符串；仅精确的 `"1"`、`"true"`、`"on"` 为真，未注册、无值或其他大小写/文本均为假。
- `eval_string(name) -> Option<String>`：把 `fail::eval` 的外层“是否求值”与内层“是否带值”展平；未注册和无字符串值都表现为 `None`。
- `is_active(name) -> bool`：检查并发回调表或 `fail::list()`，不通过 `eval` 探测，因此不会消耗动作、执行回调或卡在暂停点。它不检查 `value_callbacks`，所以单独通过 `enable_value_call` 注册的名称不能靠此函数判断活跃。
- `PauseState { reached, resumed }`：暂停点的一次性共享状态；两个字段初始均为 `false`。
- `PauseGuard`：持有 `Arc<(Mutex<PauseState>, Condvar)>` 和私有 `_guard: FailGuard`。公开方法 `wait_until_reached`、`wait_until_reached_timeout`、`resume` 分别负责无限等待命中、限时等待命中和恢复被测线程。
- `enable_pause(name) -> PauseGuard`：借助 `enable_call` 安装回调。回调先置 `reached = true` 并通知观察者，再等待 `resumed = true`。

文件没有模块级业务常量、trait、enum 或条件编译项；两个 `NEXT_ID: AtomicU64` 分别是对应注册函数内部的静态计数器。

## 执行流程

表达式型故障注入流程如下：测试调用 `enable` 并保留 `FailGuard`；运行时代码在同名边界调用 `eval_bool`、`eval_string` 或 `inject`；`fail` crate 按表达式求值；守卫离开作用域后自动注销。显式 `disable` 可以提前移除底层条目，随后再销毁守卫仍由底层实现处理。

普通回调流程是：测试通过 `enable_call` 注册回调并保留守卫；运行时代码对同名条目执行 `fail::eval`（可直接执行，也可经本文件的 `inject`）；回调在触发线程中同步运行；守卫销毁后不再触发。

并发回调流程是：

1. `enable_concurrent_call` 分配注册标识，将 `Arc` 回调放入全局表并返回 `ConcurrentCallGuard`。
2. 注入线程调用 `inject`；函数持锁查找并克隆 `Arc`，随后释放锁。
3. 注入线程在锁外同步执行回调，因此其他线程可同时取出并执行同一回调，守卫销毁也不必等待正在运行的回调结束。
4. 守卫销毁时按“名称与标识均匹配”删除；若同名注册已被新注册覆盖，旧守卫不改变新条目。

带值回调的流程类似，但注册端使用 `enable_value_call`，生产注入点必须显式调用 `inject_value(name, value)`。`inject_value` 同样先在锁内克隆 `Arc`、再在锁外同步调用，避免回调重入时持有注册表锁。

暂停流程是：测试调用 `enable_pause` 并保留 `PauseGuard`；被测线程对同名底层回调 failpoint 求值后，将 `reached` 置真、通知条件变量并阻塞等待 `resumed`；测试线程用 `wait_until_reached` 或超时版本确认边界已到达，完成中间状态断言后调用 `resume`；回调被唤醒，被测线程继续。`reached` 和 `resumed` 不会复位，所以一个守卫表达的是一次“到达后放行”的门闩；恢复后再次命中不会再次暂停。

## 数据与状态

底层表达式/普通回调状态归 `fail` crate 的进程级注册表管理，本文件只持有 `FailGuard`。自建并发回调和带值回调各有一个由 `OnceLock` 延迟初始化的进程级 `Mutex<HashMap<...>>`；同名键只能保存最新注册，后注册会覆盖前注册。

两个自建表中的 `u64` 是守卫所有权代次，不是业务序号。各自的函数内 `AtomicU64` 从 1 开始以 `Ordering::Relaxed` 自增；这里原子值只用于生成跨线程不重复的代次，不承载内存同步，实际表一致性由 `Mutex` 保证。两类表拥有彼此独立的计数器和命名空间。

回调存入 `Arc` 后会在锁外运行。这样既缩短临界区，也允许回调重入辅助 API；相应地，守卫销毁只阻止未来查表取得回调，已经克隆出的回调仍可执行完毕。`inject_value` 把借用的 `&str` 同步传给回调，不缓存该值。

`PauseGuard` 的状态由测试线程和命中线程共享。`Mutex` 保护 `reached/resumed`，`Condvar::wait_while` 以谓词循环抵抗虚假唤醒；`notify_all` 允许多个观察者或等待者重新检查状态。`Duration` 仅由超时等待方法使用，不存在后台计时任务。

## 依赖与调用关系

下游依赖只有标准库同步/集合类型与 `fail` crate：`HashMap` 保存自建注册表，`Arc` 管理跨线程回调和暂停状态，`Mutex`/`Condvar` 提供同步，`OnceLock` 初始化全局表，`AtomicU64` 生成注册代次，`Duration` 表达等待上限；`FailGuard::new`、`FailGuard::with_callback`、`fail::remove`、`fail::eval`、`fail::list` 提供底层故障注入能力。

上游调用分为两侧：

- 测试侧注册与控制：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接覆盖基本注册、清理、显式禁用、值传递和暂停超时；[`tests/realtikvtest/addindextest3/ingest_test.rs`](../../../tests/realtikvtest/addindextest3/ingest_test.rs) 使用 `enable_concurrent_call` 验证并行注入；[`tests/realtikvtest/txntest/txn_state_test.rs`](../../../tests/realtikvtest/txntest/txn_state_test.rs) 用 `PauseGuard` 协调事务测试；[`tests/realtikvtest/ddltest/ddl_test.rs`](../../../tests/realtikvtest/ddltest/ddl_test.rs) 使用值回调观察生产事件。
- 被测运行时代码触发与求值：[`pkg/session/runtime/ddl.rs`](../../session/runtime/ddl.rs) 发出带值事件并查询布尔/活跃状态，[`pkg/session/runtime/control.rs`](../../session/runtime/control.rs) 在事务路径求值字符串并触发回调，[`pkg/session/runtime/planning.rs`](../../session/runtime/planning.rs) 在规划/统计路径调用 `inject` 与 `eval_bool`，[`pkg/store/driver/read_request.rs`](../../store/driver/read_request.rs) 读取字符串型网络故障值。

RustCodeGraph 的文件索引确认本目录包含 `failpoint.rs`（31 个符号）、`lib.rs` 和迁移测试，并能通过 `query` 定位 `enable_call`、`enable_concurrent_call`、`enable_value_call`、`inject_value`、`eval_bool`、`eval_string`、`is_active`、`PauseGuard` 方法及 `enable_pause`。本次索引的 `explore`、文件 `node` 和精确 `callers/callees` 未输出可用调用边，因此上述调用关系由精确 `rg` 引用位置补证，未把空图结果推断成“没有调用者”。

## 错误处理与边界

所有注册失败和同步原语中毒都被视为测试基础设施错误并 panic：`enable`/`enable_call` 的 panic 包含 failpoint 名称和底层错误；自建表与暂停状态的 `.expect(...)` 区分并发回调、值回调和暂停锁。公开 API 不返回可恢复错误，这与 Go 辅助函数用 `require.NoError` 立即终止当前测试的意图一致。

未注册名称的行为是无副作用：`disable` 忽略 `fail::remove` 的返回，`inject` 的自建查找为空且底层求值结果被丢弃，`inject_value` 不调用任何回调，两个求值函数分别返回 `false`/`None`，`is_active` 返回 `false`。

需要特别区分三套注册机制：

- `disable` 只清理 `enable`/`enable_call` 对应的底层 `fail` 条目，不清理 `enable_concurrent_call` 或 `enable_value_call` 的条目；后两者必须销毁各自守卫。
- `inject` 会触发自建并发回调和底层 `fail` 条目，但不会触发值回调；值回调只能由 `inject_value` 触发。
- `is_active` 能看到自建并发回调和底层 `fail` 条目，但看不到仅存在于值回调表的注册。

`eval_bool` 是严格字符串协议，不做大小写折叠或通用布尔解析。`eval_string` 将“未启用”和“启用但不带字符串值”都映射为 `None`；若调用方必须区分活跃状态，应另行使用适用的注册机制与 `is_active`，但不能用它判断纯值回调表。

暂停 API 的无限等待版本可能永久阻塞；测试无法保证注入点一定到达时应使用 `wait_until_reached_timeout`。更重要的是，命中线程一旦在暂停回调内等待，销毁 `PauseGuard` 不会把 `resumed` 置真；调用方必须先 `resume`，否则被测线程可能被遗留在等待中。当前实现也不适合作为可重复闭合/打开的多轮屏障。

## 并发与资源生命周期

所有回调都在命中 failpoint 的线程中同步执行，本文件不创建线程或异步任务。`Send + Sync + 'static` 约束保证注册闭包可以安全存入全局表并从不同线程调用；闭包内部状态的并发正确性仍由调用者负责。

底层 `FailGuard`、`ConcurrentCallGuard` 和 `ValueCallGuard` 都依赖词法作用域清理。自建守卫的注册标识检查解决“同名 A 注册、同名 B 覆盖、随后 A 先析构”这一竞态：A 的析构不会误删 B。由于注入端在锁内只克隆 `Arc`，执行回调不占用注册表互斥锁；这也是 `enable_concurrent_call` 可以让多个命中线程的回调体重叠的关键。

暂停守卫的 `_guard` 保证注册随守卫销毁，但它不是取消令牌。正确生命周期顺序是：保留 `PauseGuard` → 启动被测操作 → 等待 `reached` → 检查中间状态 → `resume` → 等待被测操作完成 → 最后销毁守卫。若可能提前返回，调用方应设计可靠的恢复路径，避免命中线程滞留。

这些注册表是进程全局状态，使用相同名称的并行测试会互相覆盖或观察到彼此的注册。调用方应使用唯一名称，或用仓库的串行测试保护机制隔离共享 failpoint；文档不能把单个守卫误解为线程局部状态。

## 与 Go 版本的对应关系

Go 对照文件是 [`failpoint.go`](./failpoint.go)，仅公开 `Enable(t, name, expr)`、`EnableCall(t, name, fn)` 和 `Disable(t, name)`。三者用 `require.NoError` 把配置/禁用错误变为测试失败，并由 `t.Cleanup` 在测试结束时自动禁用。

Rust `enable`/`enable_call` 保留“注册失败立即失败”的语义，但把 `testing.TB` 生命周期改为返回值的 RAII 生命周期：调用者必须保存守卫，提前 `drop` 就会提前禁用。Rust `disable` 接受未知名称且不返回错误；其目标是与 Go 辅助函数的正常无错使用方式兼容，但没有 `testing.TB` 可记录断言。

Rust 文件额外提供 `inject`、布尔/字符串求值、活跃检查、并发回调、值回调和暂停栅栏。这些不是当前 Go `testfailpoint` 辅助文件的逐函数翻译，而是为了连接 Rust 生产注入点和迁移测试补充的适配层。其中值回调明确模拟 Go `failpoint.InjectCall` 传递参数的可观察边界；暂停辅助则把测试常见的“命中后等待测试放行”模式封装为 RAII 注册加条件变量。

对应语义的最近独立 Rust 测试位于 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，由 `lib.rs` 的 `#[cfg(test)]` 模块声明加载，没有把测试逻辑内嵌进生产文件。仓库同目录没有 Go `failpoint_test.go`；Go 语义依据来自 `failpoint.go` 本身及全仓 Go 调用方，而 Rust 扩展能力还需参考上述 Rust 单元/集成测试。

## 扩展指南

新增表达式类型或底层回调能力时，优先复用 `fail` crate 并保持 `FailGuard` 的 RAII 契约；若新增自建注册表，应沿用“全局惰性初始化、锁内克隆、锁外调用、代次守卫防误删”的结构。不要让回调在持有注册表 `Mutex` 时执行，否则回调重入注册/禁用路径可能死锁，并会串行化原本允许重叠的调用。

新增求值函数时，应明确区分三种状态：名称未注册、注册但无值、注册且带值，并记录次数表达式是否被消耗。若扩展 `disable` 或 `is_active` 去覆盖自建表，这是可观察的兼容性变化，必须决定它们是统一操作所有注册机制，还是继续保持当前分层语义，并为同名覆盖、旧守卫析构和已取出 `Arc` 的竞态添加测试。

扩展带值回调时，最可能修改 `ValueCallback`、`enable_value_call` 和 `inject_value`，同时同步生产注入点与测试回调签名。当前 `&str` 是同步借用协议；若改为拥有值、泛型值或多参数，需要评估分配、类型擦除、跨线程共享和 Go `InjectCall` 兼容性。

扩展暂停能力时，最可能修改 `PauseState`、`PauseGuard` 方法和 `enable_pause`。增加取消、自动恢复或多轮屏障前必须定义守卫析构时的行为、多个命中线程的放行方式以及超时后的清理策略，避免把一次性门闩悄然变成语义不完整的循环屏障。

测试应继续放在独立文件，而不是写入 `failpoint.rs`。基础契约同步到 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；涉及并发重叠可参考 `tests/realtikvtest/addindextest3/ingest_test.rs`，涉及暂停生命周期可参考 `tests/realtikvtest/txntest/txn_state_test.rs` 和 `pkg/ddl/tests/partition/error_injection_test.rs`，涉及值回调可参考 `tests/realtikvtest/ddltest/ddl_test.rs`。应覆盖同名覆盖与乱序析构、回调重入、并行命中、超时后恢复、锁中毒 panic 信息以及未注册名称；性能风险主要是每次注入的全局互斥锁与字符串查找。

## 验证依据

- 源码与模块边界：[`failpoint.rs`](./failpoint.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、根 [`Cargo.toml`](../../../Cargo.toml)。
- Go 对照：[`failpoint.go`](./failpoint.go) 的 `Enable`、`EnableCall`、`Disable` 及 `t.Cleanup` 行为。
- 最近的独立 Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的 `enable_and_guard_cleanup_match_go_test_cleanup`、`enable_call_registers_callback_and_cleans_it_up`、`disable_removes_an_enabled_failpoint`、`value_call_forwards_runtime_value_and_guard_cleans_up`、`pause_wait_timeout_never_strands_the_test`。
- 代表性调用证据：`pkg/session/runtime/{ddl,control,planning}.rs`，`pkg/store/driver/read_request.rs`，`tests/realtikvtest/{addindextest3/ingest_test,txntest/txn_state_test,ddltest/ddl_test}.rs`，`pkg/ddl/tests/partition/error_injection_test.rs`。
- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件，`files --filter pkg/testkit/testfailpoint` 确认目标、入口、Go 对照和迁移测试；精确 `query` 确认主要公开函数与方法的位置和签名。`explore`、按文件 `node`、精确 `callers/callees` 本次没有返回可用边，因此调用关系另由 `rg` 核验。
- 静态边界复核：文件共 283 行，无条件编译；测试由 `lib.rs` 的 `#[cfg(test)] #[path = "migration_aster_unit_test.rs"]` 独立挂接；文档未把未运行的 Cargo 测试描述为已验证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构命令，并人工复核所有重要行为均能回指上述符号或调用文件。
