# `pkg/lightning/membuf/limiter.rs`

## 文件定位

本文件属于 Cargo crate `astersql-lightning-membuf`。crate 边界由 `pkg/lightning/membuf/Cargo.toml` 定义，入口 `pkg/lightning/membuf/lib.rs` 通过 `pub mod limiter` 声明模块并用 `pub use limiter::*` 重导出其公开项。它为同 crate 的块池与顺序分配器提供共享内存配额控制：`pkg/lightning/membuf/buffer.rs` 的 `Pool` 可持有 `Arc<Limiter>`，在借出固定块、归还固定块以及记录小对象元数据开销时使用该限制器。

该文件不是独立进程入口，也不直接分配业务内存；它只维护“尚可获取的配额”和等待队列。工作区中 `astersql-ingestor-engineapi`、`astersql-ingestor-globalsort`、`astersql-ingestor-ingestctrl`、`astersql-ingestor-simplesst` 的 Cargo 清单依赖该 crate，但当前 Rust 生产代码搜索到的外部使用主要面向 `Pool`，限制器的直接生产调用位于同 crate 的 `buffer.rs`。

## 核心职责

- `NewLimiter` 创建一个可在线程间共享的限制器，并令当前余额和初始上限都等于传入值。
- `Limiter::Acquire` 提供阻塞式配额获取：余额足够时立即扣减，否则按到达顺序入队并等待明确唤醒。
- `Limiter::TryAcquire` 提供非阻塞式获取：余额不足或已经有人排队时立即失败，避免新请求绕过等待者。
- `Limiter::Release` 归还配额，并严格从队首开始连续满足当前能够完整支付的请求。
- `ErrCannotAcquireMemory` 为上层非阻塞分配失败提供稳定文本；限制器自身的 `TryAcquire` 返回 `bool`，实际把该文本转换成 `String` 错误的是 `buffer.rs` 的 `Buffer::tryAllocLocation`。

因此，本文件控制的是逻辑配额而不是进程 RSS。调用方必须以相同单位成对调用获取与释放；在当前块池接线中，该单位是字节数。

## 主要符号

- `pub const ErrCannotAcquireMemory: &str`：固定错误文本 `cannot acquire memory from membuf limiter`，由 `buffer.rs` 的非阻塞路径对外返回。
- `Waiter { n, ready }`：私有等待项。`n` 是请求量；`ready: Arc<(Mutex<bool>, Condvar)>` 是该请求独享的完成标志和条件变量。它把 Go 实现中平行的 `waitNums` 与 `waitChs` 两个切片合并为一个对象。
- `LimiterState { limit, waiters }`：受同一把互斥锁保护的全部可变状态。`limit` 是当前可用余额，`waiters: VecDeque<Waiter>` 是 FIFO 队列。
- `pub struct Limiter { initLimit, state }`：`initLimit` 保存构造时的基准值，仅用于识别超量归还；`state: Mutex<LimiterState>` 串行化余额和队列修改。
- `pub fn NewLimiter(limit: usize) -> Arc<Limiter>`：直接返回 `Arc`，使调用方能安全地在线程、`Pool` 和 `Buffer` 生命周期之间共享同一实例。
- `Limiter::Acquire(&self, n: usize)`：阻塞式入口，无返回值；配额已经在返回前扣除。
- `Limiter::TryAcquire(&self, n: usize) -> bool`：非阻塞式入口；只有成功时才修改余额。
- `Limiter::Release(&self, n: usize)`：归还并唤醒等待者；超量归还会记录错误和回溯，但仍保留增加后的余额。
- `#[cfg(test)] mod limiter_test`：仅测试构建时把独立文件 `pkg/lightning/membuf/limiter_test.rs` 接入模块，没有把测试逻辑内嵌到生产文件。

## 执行流程

1. `NewLimiter(limit)` 构造空的 `VecDeque`，令 `state.limit = initLimit = limit`，再用 `Arc` 包装限制器。
2. `Acquire(n)` 先锁定 `state`。若 `state.limit >= n`，直接扣减并返回；否则创建私有唤醒对象，将 `Waiter` 压入队尾，释放状态锁后在条件变量上循环等待。循环检查布尔标志可抵御条件变量的伪唤醒。
3. `TryAcquire(n)` 在状态锁内同时检查队列和余额。只有队列为空且余额不少于 `n` 时才扣减并返回 `true`；任何已有等待者都会令它返回 `false`，包括请求量为零的情况。
4. `Release(n)` 在状态锁内先增加余额。若余额超过 `initLimit`，通过 `log::error!` 记录当前余额、初始上限和 `Backtrace::force_capture()` 的真实调用栈。
5. `Release` 随后只检查队首：余额不足以满足队首时立即停止，即使队尾有较小请求也不越过；余额足够时预先扣除该等待者所需配额，将其移出队列并收集唤醒对象，然后继续检查新的队首。
6. 状态锁释放后，`Release` 才逐个设置完成标志并调用 `notify_all`。等待线程看到标志为真后返回；因为配额已在状态锁内为它预扣，不需要醒来后再次竞争余额。

在上层主路径中，`buffer.rs` 的 `Pool::acquire` 在取缓存块或新分配块之前调用 `Acquire(blockSize)`，`Pool::release` 在缓存或释放块之后调用 `Release(blockSize)`；`Buffer::recordSmallObjOverhead` 和 `releaseSmallObjOverhead` 以 256 KiB 批量获取和归还元数据配额；`Buffer::tryAllocLocation` 则先汇总块与元数据所需配额，再调用一次 `TryAcquire`，成功后才修改 Buffer 状态。

## 数据与状态

核心不变量是：`LimiterState.limit` 与 `LimiterState.waiters` 只能在 `Limiter.state` 的互斥锁保护下读取或修改。余额表示尚未承诺给调用者或等待者的配额；当 `Release` 决定唤醒等待者时，会先从余额扣除其 `n`，所以已移出队列但尚未真正被调度运行的线程也已经拥有配额。

等待队列使用 `VecDeque`，入队只发生在尾部，出队只发生在头部。严格 FIFO 会造成队首阻塞：如果队首请求过大，后续小请求即使能被当前余额满足也继续等待。这是公平性选择，不是吞吐量优先策略。

`initLimit` 不随运行变化，且没有动态调额 API。代码使用 `usize` 算术：调用契约要求获取/释放数量合理并成对；实现没有使用 checked arithmetic，也没有把超过初始上限作为可恢复错误。`Acquire(0)` 在余额检查下通常立即成功；但 `TryAcquire(0)` 在已有等待者时仍失败，以保持不插队语义。

## 依赖与调用关系

直接标准库依赖为 `Arc`、`Mutex`、`Condvar`、`VecDeque` 与 `Backtrace`。唯一 Cargo 运行时第三方依赖是 `log = "0.4"`，用于超量释放诊断；`rand = "0.8"` 只属于该 crate 的开发依赖，目标文件不直接使用。

已核实的主要调用关系如下：

- `pkg/lightning/membuf/lib.rs` 声明并重导出本模块。
- `buffer.rs::WithPoolMemoryLimiter` 把 `Arc<Limiter>` 写入 `Pool`。
- `buffer.rs::Pool::acquire` → `Limiter::Acquire`；`Pool::release` → `Limiter::Release`。
- `buffer.rs::Buffer::recordSmallObjOverhead` → `Acquire`；`releaseSmallObjOverhead` → `Release`。
- `buffer.rs::Buffer::tryAllocLocation` → `TryAcquire`，失败时返回 `ErrCannotAcquireMemory`。
- RustCodeGraph 对 `NewLimiter` 的已解析反向边指向 `limiter_test.rs` 的 `test_limiter`、`test_wait_up_multiple_caller` 与 `release_overflow_logs_a_real_stack`；限定路径源码搜索还确认了 `buffer_test.rs` 和 `migration_aster_unit_test.rs` 的构造及调用。

外部 ingestor 模块通过 Cargo 依赖和 re-export 使用该 membuf crate，例如 `pkg/ingestor/ingestctrl/local.rs` 将其重导出为 `membuf`，`pkg/ingestor/engineapi/ingest_data.rs` 的接口接收 `membuf::Pool`。当前直接证据没有显示这些外部生产文件自行构造 `Limiter`；它们经由 `Pool` 接口间接处在可选限额能力的上游。

## 错误处理与边界

公开获取 API 不返回 `Result`：`Acquire` 要么立即成功，要么无限等待到未来的 `Release` 满足它；没有超时、取消或关闭机制。若某个等待者所需配额永远无法被归还，它会持续阻塞，且严格 FIFO 会同时阻塞其后的请求。

`TryAcquire` 用 `false` 表示不能立即满足，不区分“余额不足”和“已有等待者”。`ErrCannotAcquireMemory` 由上层 `Buffer::tryAllocLocation` 把这个失败转换成字符串错误。该路径在扣减成功前不修改 Buffer 状态，相关测试验证了失败时 `TotalSize` 不变。

所有互斥锁和条件变量等待都调用 `unwrap()`；如果锁因持锁线程 panic 而 poisoned，后续操作也会 panic。`Release` 超过 `initLimit` 只写错误日志，既不回滚余额也不 panic；`limiter_test.rs::release_overflow_logs_a_real_stack` 验证日志包含真实 `stack=` 且不是占位文本。

实现没有显式阻止 `usize` 加法溢出，也没有校验单次请求是否大于永远可达到的最大配额。安全扩展时不能把日志告警误当作余额上限保护。

## 并发与资源生命周期

`NewLimiter` 返回 `Arc<Limiter>`；每个 `Waiter` 也用 `Arc` 保存自己的条件变量状态，因此等待线程与队列可以在不同锁作用域持有同一唤醒对象。`Mutex<LimiterState>` 保证余额检查、扣减、入队和出队是原子的；每个等待者独享条件变量，避免无关请求被同一通知唤醒后争抢配额。

`Acquire` 入队后先释放全局状态锁，再锁定自身完成标志并等待。`Release` 在全局状态锁内决定并预扣所有可唤醒请求，但把通知延迟到释放全局锁之后，从而避免醒来的线程立即与释放者争用同一把锁。设置布尔标志与条件变量等待使用同一把私有互斥锁，配合 `while !awakened` 防止丢失通知和伪唤醒。

限制器没有显式析构或关闭流程。若最后的 `Arc<Limiter>` 在仍有线程阻塞于 `Acquire` 时无法由所有参与者正常协调释放，等待线程持有的引用与执行上下文会使整个等待关系继续存在；调用方应确保资源生命周期内最终归还已获取配额。`buffer.rs::Buffer::Destroy` 是当前成对归还块和小对象元数据配额的重要上层生命周期入口，而 `Reset` 只释放小对象元数据账本、保留已借块供复用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/membuf/limiter.go`，公开概念和算法保持一致：`NewLimiter` 初始化余额与初始上限；`Acquire` 余额不足时 FIFO 排队；`TryAcquire` 在有等待者或余额不足时失败；`Release` 从队首连续满足请求；超量归还记录余额、初始上限和调用栈。

Rust 的主要表达差异如下：

- Go 使用 `int`，Rust 使用 `usize`；两者都依赖调用者传入非负且单位一致的数量，但整数边界并不完全相同。
- Go 用 `mu` 同时保护 `limit`、`waitNums`、`waitChs`；Rust 用 `Mutex<LimiterState>` 保护余额和 `VecDeque<Waiter>`，将数量与通知对象绑定，消除了两个切片错位的可能。
- Go 每个等待者创建无缓冲 channel，`Release` 通过 `close` 一次性放行；Rust 使用私有 `Mutex<bool> + Condvar`，必须循环检查标志以处理伪唤醒。
- Go 构造函数返回 `*Limiter`，Rust 返回 `Arc<Limiter>`，把跨线程共享所有权显式纳入 API。
- Go 的 `ErrCannotAcquireMemory` 是 `error` 值，Rust 当前是 `&str` 常量，并由 Buffer 层按需复制成 `String`。
- Go 在持有限制器互斥锁时关闭等待 channel；Rust 先完成配额扣减和出队，再释放全局锁并通知，以减少锁竞争，但对调用方保持“醒来即已获得配额”的语义。

`pkg/lightning/membuf/limiter_test.go` 的 `TestLimiter` 与 `TestWaitUpMultipleCaller` 分别验证并发持有量不超限和一次释放连续唤醒多个等待者；Rust 的 `limiter_test.rs` 保留这两项意图，并额外验证超量归还日志回溯。`migration_aster_unit_test.rs::migration_limiter_blocks_and_preserves_fifo_against_try_acquire` 补充验证了等待者存在时 `TryAcquire(0)` 也不能插队。

## 扩展指南

若新增动态调额、超时或取消能力，最可能修改 `LimiterState`、`Acquire` 和 `Release`。必须继续在同一状态锁内原子地维护余额与队列，并明确取消发生在“配额预扣前”还是“预扣后”；否则可能泄漏配额或唤醒错误等待者。对应测试应放在独立的 `pkg/lightning/membuf/limiter_test.rs`，覆盖队首取消、超时与 Release 并发、伪唤醒以及多个等待者的顺序，不能把测试写回生产文件。

若改变公平策略，应同时审查 `TryAcquire` 的“有等待者即失败”条件和 `Release` 的只看队首逻辑，并同步 Go 对照语义；允许跳过大请求可提高利用率，但会引入饥饿和 Rust/Go 行为差异。性能调整要保留“锁内预扣、锁外通知”的不变量，并测量大量小请求下每等待者一组 `Arc/Mutex/Condvar` 的成本。

若修改错误类型或溢出策略，应同步 `ErrCannotAcquireMemory`、`buffer.rs::Buffer::tryAllocLocation`、`buffer_test.rs` 及迁移测试。把超量释放从日志改为拒绝或 panic 属于兼容性变化；引入 checked arithmetic 时也需定义溢出的公开行为。任何新增公开符号还需通过 `lib.rs` 当前的通配重导出评估 API 暴露面。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/lightning/membuf` 列出目标源、Go 对照和独立测试；`node --file pkg/lightning/membuf/limiter.rs --offset 1 --limit 260` 返回完整 138 行源码；`query Limiter --kind struct` 定位 Rust/Go 定义；`node NewLimiter` 给出构造关系以及来自三个 Rust limiter 测试的反向调用边。
- 生产源码：`pkg/lightning/membuf/limiter.rs`（目标实现）、`pkg/lightning/membuf/lib.rs`（模块声明与重导出）、`pkg/lightning/membuf/buffer.rs`（直接生产调用与错误转换）。
- crate 与上游边界：`pkg/lightning/membuf/Cargo.toml`；工作区根 `Cargo.toml` 的 `facade_lightning_membuf`；以及 `pkg/ingestor/{engineapi,globalsort,ingestctrl,simplesst}/Cargo.toml` 对该 crate 的路径依赖。
- Go 对照：`pkg/lightning/membuf/limiter.go` 与 `pkg/lightning/membuf/limiter_test.go`。
- Rust 测试证据：`pkg/lightning/membuf/limiter_test.rs`、`pkg/lightning/membuf/buffer_test.rs`、`pkg/lightning/membuf/migration_aster_unit_test.rs`。这些文件覆盖并发上限、批量唤醒、真实回溯日志、非阻塞失败原子性和 FIFO 防插队。
- 限定路径 `rg` 搜索核实 `Acquire`、`TryAcquire`、`Release`、`NewLimiter`、`WithPoolMemoryLimiter` 及 crate 名的直接引用；没有发现外部 Rust 生产文件直接构造本限制器，因此文中只把外部模块描述为 membuf/Pool 的上游使用者。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付前使用任务文件规定的命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核没有把未发现的直接接线表述为已支持事实。
