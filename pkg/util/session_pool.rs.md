# `pkg/util/session_pool.rs`

## 文件定位

本文件是 `astersql-util` crate 中的通用会话资源池实现，源码由 [`pkg/util/lib.rs`](lib.rs) 的 `pub mod session_pool` 暴露。crate 边界由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义；该实现直接使用标准库的 `Arc`、`Mutex`、`VecDeque` 和 `Any`，并使用 `anyhow` 表达工厂及取资源错误、使用 `fail` 提供与 Go failpoint 对齐的错误注入。

它位于具体 SQL session 实现之下：本文件不知道事务、SQL 或存储类型，只管理实现了 `Resource` 的共享对象。直接可见的下游使用包括 `pkg/domain/sqlsvrapi/lib.rs` 对 `DestroyableSessionPool` 的再导出，以及 `pkg/dxf/framework/dxfutil/lib.rs` 中 `TaskManager` 持有该 trait object、调用 `Get`/`Put` 并借助 `as_any` 恢复具体 session 上下文。它不是 `pkg/session/runtime/system_session.rs` 中高级系统会话池的实现，两者不应混为一谈。

## 核心职责

- 用 `NewSessionPool` 组装容量、资源工厂、三个可选生命周期回调和共享可变状态，并以 `Arc<dyn DestroyableSessionPool>` 隐藏具体 `Pool` 类型。
- 在 `Get` 中优先复用 FIFO 空闲资源；没有空闲资源时调用 `Factory` 即时创建。`capacity` 只限制可缓存的空闲资源数，并不限制同时借出的资源数或工厂创建总数。
- 在 `Put` 中先通知 put 回调，再把资源放回空闲队列；池已关闭或空闲队列已满时直接关闭资源。
- 在 `Destroy` 中通知 destroy 回调并立即关闭资源，用于调用方认定资源不可复用的路径。
- 在 `Close` 中原子地禁止后续借用、取空空闲队列，并在锁外关闭取出的资源。

## 主要符号

- `Resource: Any + Send + Sync`：可池化对象契约。`close(&self)` 释放底层资源；`as_any(&self)` 为 trait object 提供只读向下转型入口。实现者必须自行保证 `close` 在可能重复调用时的安全性。
- `PooledResource = Arc<dyn Resource>`：池与调用方之间传递的共享资源句柄。池无法保证资源在 `close` 时没有其他 `Arc` 持有者，因此 `close` 是逻辑关闭而非内存析构。
- `Factory = Arc<dyn Fn() -> Result<PooledResource> + Send + Sync>`：空闲队列为空时同步创建资源的工厂；错误由 `Get` 原样通过 `?` 传播。
- `ResourceCallback = Arc<dyn Fn(&PooledResource) + Send + Sync>`：借出、归还、显式销毁的通知回调。回调没有 `Result`，失败只能 panic。
- `SessionPool`：公开的 `Get`、`Put`、`Close` 接口；方法名保留 Go 风格。
- `DestroyableSessionPool: SessionPool`：增加 `Destroy` 的公开接口。
- `PoolState { resources, closed }`：受同一个互斥锁保护的空闲 FIFO 队列和关闭标记。
- `Pool`：私有实现，保存不可变的 `capacity`、`factory`、三个回调，以及 `Mutex<PoolState>`。
- `NewSessionPool(...) -> Arc<dyn DestroyableSessionPool>`：唯一构造入口；初始状态为空且未关闭，`VecDeque` 预分配到指定容量。

## 执行流程

`Get` 的流程如下：

1. 获取 `state` 锁；若 `closed` 为真，立即返回 `anyhow!("session pool closed")`。
2. 从 `resources` 队首弹出一个空闲资源并释放锁。
3. 若队列为空，在锁外调用 `factory`。因此耗时创建不会阻塞其他线程访问池，但多个并发 `Get` 可以同时创建资源。
4. 执行 `mockSessionPoolReturnError` failpoint；触发时返回同名错误。
5. 成功路径执行可选 `get_callback`，再返回资源。

`Put` 先在锁外执行 `put_callback`，然后锁定状态。若池已关闭或 `resources.len() >= capacity`，它释放锁并调用 `resource.close()`；否则将资源追加到队尾。后续 `Get` 从队首取出，形成 FIFO 复用。特别地，溢出关闭和关闭后归还均不会调用 `destroy_callback`。

`Destroy` 不读取池状态，也不占用状态锁；它先执行可选 `destroy_callback`，再调用 `close`，且永远不把资源放回队列。

`Close` 在锁内检查幂等关闭、设置 `closed = true` 并 `drain` 全部空闲资源，然后释放锁并逐个 `close`。第二次及后续调用直接返回。已借出的资源不在队列中，故不会被本次 `Close` 主动找到；它们之后经 `Put` 归还时会因关闭态而被关闭，或由调用方显式 `Destroy`。

## 数据与状态

池只有两项运行时可变状态：`PoolState.resources` 保存当前空闲资源，`PoolState.closed` 是不可逆的关闭标记。二者同锁更新，避免 `Put` 在观察到未关闭后与 `Close` 交错入队而遗留资源。`Close` 先在锁内移走队列，再在锁外执行用户资源的 `close`，从而避免慢关闭长期占锁或资源关闭逻辑重入池时直接死锁。

`capacity` 的不变量是 `resources.len() <= capacity`。容量为零是合法输入：`Get` 仍可调用工厂创建资源，但每次 `Put` 都满足“队列已满”并关闭资源，因此表现为不缓存。该结构不记录借出集合、活跃数或所有权租约，调用者必须确保每次借出的逻辑资源最终只选择一次 `Put` 或 `Destroy`；重复归还、归还后继续使用以及同时归还同一个 `Arc` 都不会被池检测。

回调通常用于外部登记或计数，不属于池状态事务：`get_callback` 在资源已从队列移除或新建之后运行；`put_callback` 在确认能否缓存之前运行；`destroy_callback` 只在显式销毁时运行。因而回调观察到的是生命周期事件，而不是“成功入队”或“所有关闭事件”。

## 依赖与调用关系

- 上游装配：`pkg/util/lib.rs` 导出 `session_pool`；`pkg/util/Cargo.toml` 声明 crate 名 `astersql-util`，并声明 `anyhow` 与启用 `failpoints` feature 的 `fail` 依赖。清单中的 `autotests = false` 表示测试依赖 `lib.rs` 显式挂载，`session_pool_test.rs` 正是通过 `#[cfg(test)]` 和 `#[path = "session_pool_test.rs"]` 接入。
- 直接消费者：`pkg/domain/sqlsvrapi/Cargo.toml` 以 `util-dependency` 引用 `astersql-util`，`pkg/domain/sqlsvrapi/lib.rs` 再导出 `DestroyableSessionPool`，`Runtime::SysSessionPool` 以 `Arc<dyn DestroyableSessionPool>` 返回 keyspace 范围的系统池。
- 直接消费者：`pkg/dxf/framework/dxfutil/Cargo.toml` 同样以 `util-dependency` 引用本 crate；其 `lib.rs` 再导出整个 `session_pool` 模块。`TaskManager::WithNewSession` 调用 `Get`，通过 `Resource::as_any` 转为具体 `sessionctx::Context`，执行回调后调用 `Put`。
- 下游调用：`Get` 调用 `Mutex::lock`、`VecDeque::pop_front`、可选 `Factory`、failpoint 和可选 get 回调；`Put` 调用可选 put 回调、状态锁、`VecDeque::push_back` 或 `Resource::close`；`Close` 调用 `VecDeque::drain` 和每个资源的 `close`；`Destroy` 调用可选 destroy 回调与 `close`。
- RustCodeGraph 的 `query NewSessionPool` 与 `query DestroyableSessionPool` 能定位本文件符号，但 `files --filter pkg/util/session_pool`、限定节点的 `node/callers/callees` 未能解析本 Rust 文件的图节点。因此上面的具体调用关系以源码、模块入口、Cargo 依赖和精确 `rg` 使用点为依据，不把未取得的图边描述成已验证结果。

## 错误处理与边界

- `Get` 有三个显式错误来源：池关闭时固定返回 `session pool closed`；工厂错误原样传播；failpoint 返回 `mockSessionPoolReturnError`。只有完全成功后才调用 `get_callback`。
- failpoint 位于资源出队/创建之后。触发时资源不会重新入队，也不会调用 get/destroy 回调或显式 `close`；局部 `Arc` 被丢弃。若其他引用仍存在或 `Resource` 的实际清理只依赖显式 `close`，调用方不能假定此路径完成了资源关闭。
- 所有状态锁均用 `expect("session pool mutex poisoned")`；任一持锁线程 panic 导致锁中毒后，后续 `Get`、`Put` 或 `Close` 会 panic，而不是返回可恢复错误。
- 三类回调和 `Resource::close` 都没有错误返回通道；panic 会向调用线程传播。`put_callback` 在获取状态锁之前运行，若其 panic，资源既未入队也未关闭；`destroy_callback` panic 时后续 `close` 不会执行。
- `Put` 不验证资源来源或当前借出状态。队列满、容量为零、池已关闭都属于正常关闭分支，不会返回错误，也不会调用 destroy 回调。
- `Close` 只保证拒绝新的 `Get` 并释放当时的空闲资源，不等待在途 `Get`、不追踪借出资源，也没有阻塞式 drain 完成协议。

## 并发与资源生命周期

`Pool`、trait、工厂、资源和回调都要求 `Send + Sync`，公共句柄由 `Arc` 共享。`Mutex<PoolState>` 将关闭标记与空闲队列的修改串行化；工厂、回调和资源关闭均尽量在状态锁外执行。`Put` 的回调发生在加锁前，因此它可以与 `Close` 并发，之后 `Put` 会重新读取关闭态并在需要时关闭资源。

`Get` 在释放锁后才创建资源，这避免持锁创建，但也形成刻意的并发窗口：线程 A 确认池未关闭后释放锁并进入工厂，线程 B 可完成 `Close`，随后线程 A 仍可能成功返回新资源，因为 `Get` 不会在工厂后复查 `closed`。这与本文件当前代码一致；需要“Close 返回后绝不再有在途 Get 成功”的调用方不能只依赖该接口。

空闲资源的正常生命周期为“工厂创建或从队首取得 → get 回调 → 调用方使用 → put 回调 → 入队”。异常资源应走“借出 → destroy 回调 → close”。空闲队列满或池关闭后的归还则是“put 回调 → close”。池关闭时，队列内资源是“drain → close”，不会补发 put/destroy 回调。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/session_pool.go`](session_pool.go)，对应测试是 [`pkg/util/session_pool_test.go`](session_pool_test.go)。Rust 保留了 Go 的 `SessionPool`、`DestroyableSessionPool`、`NewSessionPool`、`Get`、`Put`、`Destroy`、`Close` 名称和主要行为：空闲优先、池空则创建、容量限制空闲缓存、归还溢出即关闭、显式销毁回调、关闭幂等以及关闭后 `Get` 的固定错误文本。

实现机制有所不同：Go 用有缓冲 channel 保存空闲资源，并用 `RWMutex` 保护关闭/发送的竞态；Rust 用一个 `Mutex` 同时保护 `VecDeque` 与关闭位。Go channel 的非阻塞收发与 Rust 队首弹出/队尾追加都形成 FIFO 风格的空闲复用。Go 的 `pools.Resource` 只要求 `Close`，Rust 额外要求 `Any + Send + Sync` 并提供 `as_any`，以支持 trait object 的类型恢复及跨线程共享。

已核对的细节差异包括：Go failpoint 设置命名返回值 `err`，函数仍携带局部 `resource` 返回；Rust failpoint 直接 `Err` 提前返回并丢弃局部句柄。Rust 的中毒锁会 panic，而 Go 锁无此状态。两版 `Put` 都在判断关闭/容量之前执行 put 回调，且溢出关闭不执行 destroy 回调；Rust 测试中关于“溢出触发 on_destroy”的注释不能作为实现事实，实际计数下降来自 `put_callback`。

Rust 独立测试 [`pkg/util/session_pool_test.rs`](session_pool_test.rs) 移植了 Go `TestSessionPool` 的容量 1 借还、回调计数、溢出关闭、重复关闭、关闭后归还和关闭后取资源错误。`pkg/util/security_2_aster_unit_test.rs` 还覆盖复用、溢出关闭、关闭时释放空闲资源及关闭后归还。当前测试没有单独断言工厂错误、failpoint、显式 `Destroy`、零容量、锁中毒或 `Get`/`Close` 并发窗口。

## 扩展指南

- 新增池生命周期行为时，优先修改私有 `Pool` 的相应 trait 实现，并保持 `PoolState` 中相互依赖的状态在同一临界区更新；不要在持锁期间调用工厂、外部回调或资源 `close`。
- 若要增加统计，应先定义事件口径：当前 put 回调表示“调用了 Put”，不表示“成功缓存”；destroy 回调表示“显式 Destroy”，不覆盖溢出、关闭后归还或 `Close` drain。改变口径会影响 Go 对齐和现有调用方的登记/计数假设。
- 若要强制限制活跃资源总数，需要新增许可/等待机制；仅修改 `capacity` 检查无法做到，因为当前容量有意只描述空闲缓存。应同时定义工厂失败、关闭唤醒和公平性。
- 若要强化关闭屏障，必须处理已通过首次关闭检查但仍在工厂中的 `Get`，并定义借出资源是否等待回收；这属于接口并发语义变化，需同步检查 Go 版本和直接消费者。
- 若新增返回错误的方法或允许回调失败，应避免在错误路径遗失资源，并明确是重入队、显式关闭还是转交调用方。
- 测试应继续放在独立文件 [`pkg/util/session_pool_test.rs`](session_pool_test.rs)，不要内嵌到生产文件；与 Go 行为同步时也更新 [`pkg/util/session_pool_test.go`](session_pool_test.go) 的对应意图。至少补充所改分支的工厂错误、`Destroy` 回调、零容量、并发 `Get`/`Put`/`Close` 和 panic/清理行为。性能风险集中在全局状态锁竞争、工厂并发风暴及回调/关闭耗时，兼容风险集中在回调顺序、固定错误文本和关闭竞态语义。

## 验证依据

- 生产实现：[`pkg/util/session_pool.rs`](session_pool.rs)，核对 `Resource`、类型别名、两个公开 trait、`PoolState`、`Pool`、`NewSessionPool` 以及四个生命周期方法的全部实现。
- crate 与模块装配：[`pkg/util/Cargo.toml`](Cargo.toml) 和 [`pkg/util/lib.rs`](lib.rs)，核对 crate 名、依赖、`autotests = false`、模块导出及独立测试挂载；`pkg/util` 下没有 `doc.go` 可作为额外包契约。
- Go 对照：[`pkg/util/session_pool.go`](session_pool.go) 与 [`pkg/util/session_pool_test.go`](session_pool_test.go)，核对接口、channel/锁实现、回调时序、错误文本和测试断言。
- Rust 测试：[`pkg/util/session_pool_test.rs`](session_pool_test.rs) 与 `pkg/util/security_2_aster_unit_test.rs`，核对容量、复用、回调、关闭和错误边界；本任务依照计划不运行 Cargo，因此这里只引用并人工审查测试证据。
- 直接调用证据：`pkg/domain/sqlsvrapi/{Cargo.toml,lib.rs,server.rs}` 和 `pkg/dxf/framework/dxfutil/{Cargo.toml,lib.rs}`，核对依赖别名、trait 再导出及 `TaskManager::WithNewSession` 的 `Get` → downcast → callback → `Put` 链路。
- RustCodeGraph：`status` 显示索引可用（11,467 文件、307,296 节点）；`query NewSessionPool --kind function` 和 `query DestroyableSessionPool` 定位了本文件声明及相关符号，但文件过滤和限定 `node/callers/callees` 未解析出该 Rust 节点。调用关系因此进一步使用精确 `rg` 与上述源码读取验证，并明确保留这一图覆盖限制。
- 结构验收使用任务指定命令，确认文件存在且固定二级标题恰为 11 个；同时人工复核本文没有把测试注释、理想设计或未取得的调用图边写成当前实现事实。
