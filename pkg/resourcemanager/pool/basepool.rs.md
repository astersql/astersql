# [`pkg/resourcemanager/pool/basepool.rs`](./basepool.rs)

## 文件定位

`basepool.rs` 属于 `astersql-resourcemanager-pool` crate；该 crate 的入口是
`pkg/resourcemanager/pool/lib.rs`，它声明 `pub mod basepool` 并将本文件的公开项全部再导出。
`pkg/resourcemanager/pool/Cargo.toml` 没有运行时依赖或 feature，说明这里刻意只依赖 Rust
标准库，承担资源池实现之间可共享的最小元数据层，而不是完整线程池。

当前实际消费者是 `pkg/resourcemanager/pool/spool/spool.rs`。spool crate 通过
`pkg/resourcemanager/pool/spool/lib.rs` 的 `#[path = "../basepool.rs"]` 将同一源文件编译为
自己的 `pool` 模块，再把 `BasePool` 放入 `PoolInner::base`。因此本文件既能作为独立
`astersql-resourcemanager-pool` crate 的 API 编译，也被 spool 直接复用；它不负责 worker
创建、容量控制、任务通道或资源管理器注册。

## 核心职责

- 定义与 Go `pkg/resourcemanager/pool/basepool.go` 文案一致的三个公开哨兵常量：
  `ERR_POOL_CLOSED`、`ERR_POOL_OVERLOAD`、`ERR_POOL_PARAMS_INVALID`。spool 的
  `Error::fmt` 使用它们形成稳定的用户可见错误文本。
- 用 `BasePool` 保存池的注册名、单调递增的无符号任务 ID，以及最近一次成功调谐的
  `SystemTime`。
- 为多线程读取/更新任务 ID 和调谐时间提供同步。名称只允许通过 `&mut self` 修改，
  预期在池发布给其他线程或注册到资源管理器之前完成初始化。
- 保持 Go `BasePool` 的关键语义：新建时记录当前时间，任务 ID 从 1 开始，`u64`
  溢出后回绕。

## 主要符号

- `ERR_POOL_CLOSED: &str`：提交到已关闭池时的固定文本。它在
  `spool::Error::Closed` 的 `Display` 实现中使用。
- `ERR_POOL_OVERLOAD: &str`：无可用并发槽时的固定文本。虽然文本提到 `Block is set`，
  Rust 当前实现会在 `Pool::check_and_add_running` 无法取得槽位且 `Blocking` 为假时返回
  `Error::Overload`；文案本身是为兼容 Go 保留的常量，不应据此推断控制流。
- `ERR_POOL_PARAMS_INVALID: &str`：构造参数无效时的固定文本；`Pool::new` 在 `size == 0`
  时返回对应的 `Error::InvalidParams`。
- `BasePool`：包含私有字段 `last_tune_ts: RwLock<SystemTime>`、`name: String` 和
  `generator: AtomicU64`。字段私有，外部只能经方法访问；同模块测试可直接构造边界状态。
- `BasePool::new() -> Self`：以当前系统时间、空名称和计数器 0 构造实例。
- `set_name(&mut self, String)` / `name(&self) -> &str`：写入和借用名称。写方法要求独占
  可变借用，因此没有额外锁。
- `gen_task_id(&self) -> u64`：以 `SeqCst` 原子加一，并对返回的旧值执行
  `wrapping_add(1)`；正常首个结果为 1，计数器原值为 `u64::MAX` 时结果为 0。
- `last_tuner_ts(&self) -> SystemTime` / `set_last_tune_ts(&self, SystemTime)`：分别通过
  `RwLock` 读写时间戳；即使锁被中毒，也用 `PoisonError::into_inner` 继续读取或覆盖数据。

## 执行流程

1. `spool::Pool::new` 校验容量后调用 `BasePool::new`，随后在尚未构造共享 `Arc<PoolInner>`
   前调用 `set_name`。名称又被用作指标标签和 `InstanceResourceManager::Register` 的注册键。
2. `Pool::run_with_concurrency` 成功预留实际并发槽后调用 `gen_task_id`，把结果传给
   `poolmanager::Meta::new`，再由 `TaskManager::RegisterTask` 追踪这一组并发任务。
3. `Pool::tune` 对非零目标容量加 admission 锁，先调用 `set_last_tune_ts(SystemTime::now())`，
   然后交换容量并执行 overclock/downclock。因此即使新容量等于旧容量，只要参数非零，
   调谐时间仍会刷新；`size == 0` 的早退则不会刷新。
4. `Pool::last_tuner_ts` 把基类时间戳暴露给 `GoroutinePool::LastTunerTs`。资源管理器的
   `schedule.rs::ResourceManager::Exec` 和 CPU 调度器据此与 `MinSchedulerInterval` 比较，
   避免连续调容。
5. 关闭、等待线程和注销均由 `Pool::release_and_wait` 完成；`BasePool` 没有独立的关闭或
   `Drop` 流程。

## 数据与状态

`name` 是实例创建期间写一次、运行期间只读的普通 `String`。Rust 类型系统保证
`set_name(&mut self, ...)` 与任何并发共享借用互斥；`name()` 返回的 `&str` 生命周期绑定到
`BasePool`，不会复制名称。

`generator` 的初值为 0，`gen_task_id` 返回原子递增后的逻辑值。它保证同一个实例上的每次
并发调用占有一个不同的 `u64` 值，直到整个编号空间发生回绕；回绕属于明确兼容语义，不能
把 0 当作永远非法的 ID。该字段不持久化，进程重启或重建池会重新从 1 开始。

`last_tune_ts` 在构造时取 `SystemTime::now()`，随后由有效 `Pool::tune` 更新。它是墙上时钟
而非单调时钟；调用侧通过 `SystemTime::elapsed` 处理。当前调度代码对未来时间或系统时间
倒退得到的错误采用 `unwrap_or_default()`，按零间隔处理并暂缓再次调谐。

## 依赖与调用关系

本文件仅依赖 `std::sync::{AtomicU64, RwLock}`、`std::sync::atomic::Ordering` 和
`std::time::SystemTime`。`pkg/resourcemanager/pool/Cargo.toml` 证明独立 pool crate 没有外部
依赖；spool 的完整实现则在自己的 Cargo 清单中依赖资源管理器、channel 和 Prometheus。

直接关系如下：

- `pkg/resourcemanager/pool/lib.rs` 公开声明并再导出本模块。
- `pkg/resourcemanager/pool/spool/lib.rs` 用路径模块复用本文件；
  `pkg/resourcemanager/pool/spool/spool.rs::PoolInner` 持有 `BasePool`。
- `Pool::new` 调用 `BasePool::new`、`set_name`；`Pool::run_with_concurrency` 调用
  `gen_task_id`；`Pool::tune` 调用 `set_last_tune_ts`；`Pool::name` 与
  `Pool::last_tuner_ts` 分别委托给同名元数据访问器。
- `Pool` 的 `GoroutinePool` 实现把名称与调谐时间送入
  `pkg/resourcemanager/schedule.rs`、`pkg/resourcemanager/scheduler/cpu_scheduler.rs` 的
  调度冷却链。它们通过 trait 间接访问，不直接依赖 `BasePool` 类型。

RustCodeGraph 将 `basepool.rs` 识别为含 12 个符号的文件，并显示其直接文件级使用者为
`basepool_test.rs`；精确 `callers/callees` 查询没有返回方法边。因此上述跨文件调用边同时用
仓库内符号引用搜索核验，未把空图结果解释为“没有生产调用者”。

## 错误处理与边界

本文件不返回 `Result`，三个错误项只是稳定文本。将文本映射成可匹配错误类型、判断池状态
和参数合法性，都由 `spool.rs::Error` 与 `Pool` 完成。修改文案会改变 Go/Rust 兼容测试和
调用方观察到的 `Display` 内容，属于兼容性变更。

两个 `RwLock` 访问器都显式恢复中毒锁的内部值：读侧不会因其他线程曾在持有写锁时 panic
而 panic，写侧也仍会覆盖时间戳。这提供可用性，但不表示旧值一定可信；当前 setter 的赋值
表达式本身不会执行用户代码。

`SystemTime` 可能位于未来，也可能因系统时钟调整而非单调变化；本层不校验时间顺序。
任务 ID 不检测耗尽，溢出后按 Go `atomic.Uint64.Add` 的无符号语义回绕。`BasePool::new`
也不实现 `Default`，调用方应显式构造，以保留“创建时记录当前时间”的语义。

## 并发与资源生命周期

`AtomicU64` 使多个提交线程可以共享同一个 `BasePool` 生成 ID；当前实现采用最强的
`Ordering::SeqCst`。ID 生成只要求唯一递增/回绕，不与名称或时间戳建立复合事务，因此未来
若调整内存序，必须先证明所有消费者不依赖全局顺序。

`RwLock<SystemTime>` 允许并发读取并串行化更新，`set_last_tune_ts` 使用 `&self`，所以
`BasePool` 可在 `Arc<PoolInner>` 内更新。`String` 没有锁，但共享之后没有通过 `&self`
修改它。由这些字段组合而成的 `BasePool` 自动满足 `Send + Sync`，对应编译期断言位于
`pkg/resourcemanager/pool/migration_aster_unit_test.rs`。

本类型不拥有线程、通道、文件描述符或注册句柄，也没有自定义 `Drop`。其生命周期从
`Pool::new` 构造开始，随 `PoolInner` 的最后一个 `Arc` 释放而结束；任务线程 join、停止标志、
等待者协调和资源管理器注销属于外层 `Pool` 的责任。

## 与 Go 版本的对应关系

Rust `BasePool` 逐项对应 `pkg/resourcemanager/pool/basepool.go::BasePool`：

- Go `atomicutil.Time` 对应 `RwLock<SystemTime>`；`Load`/`Store` 对应读锁/写锁访问器。
- Go `string` 对应 Rust `String`，`NewBasePool`/`SetName`/`Name` 对应
  `new`/`set_name`/`name`。
- Go `atomic.Uint64.Add(1)` 对应 Rust `fetch_add(1, SeqCst).wrapping_add(1)`，包括从 1
  开始和无符号溢出回绕。
- Go 的三个 `error` 哨兵在 Rust 中是 `&str` 常量，不具备 Go 侧错误值身份；真正的 Rust
  错误类型是 spool 的 `Error` 枚举。这是表示方式差异，调用方应匹配枚举而不是字符串。

两版的调谐语义由外层 spool 保持：零容量 tune 被忽略，有效 tune 在容量比较前更新时间。
Go 通过匿名嵌入 `pool.BasePool` 提升方法，Rust 通过 `PoolInner::base` 和显式委托暴露能力。
Go `basepool.go` 没有独立同名测试文件；Rust 的直接测试证据来自
`basepool_test.rs` 与 `migration_aster_unit_test.rs`，Go 的集成行为则可在
`pkg/resourcemanager/pool/spool/spool_test.go` 及 spool 实现中核对。

## 扩展指南

- 新增跨池共享元数据时，先确认它确实属于所有池的公共最小层；容量、执行队列、阻塞策略、
  指标和注册生命周期仍应留在具体池实现。若加入字段，要同步两个构造路径（独立 crate 与
  spool 路径模块）以及 Go 对照语义。
- 改动错误常量时，同步检查 `spool.rs::Error::fmt`、Go 的 `ErrPool*` 和
  `migration_aster_unit_test.rs::error_messages_match_go_sentinels`，评估用户可见文本兼容性。
- 改动 ID 算法时，保留并发唯一性、首值和溢出规则，并同步
  `basepool_test.rs::task_id_wraps_like_go_atomic_uint64` 与
  `migration_aster_unit_test.rs::task_ids_are_one_based_and_unique_under_concurrency`。
- 改动时间戳类型或锁策略时，检查 `Pool::tune` 的更新时间点、`GoroutinePool` trait、
  `schedule.rs::ResourceManager::Exec`、CPU 调度器及 spool 调谐测试；尤其不能无意把墙上时钟
  错误变成 panic。
- `set_name` 当前通过 `&mut self` 表达初始化期不变量。若需要运行时重命名，必须同时设计
  同步方式、资源管理器注册键与 Prometheus label 的一致更新，不能只给 `name` 加锁。
- Rust 单元测试继续放在独立的 `basepool_test.rs` 或 crate 级
  `migration_aster_unit_test.rs`，不要内嵌到生产源文件；生产文件只保留 `#[path]` 测试模块接线。

## 验证依据

- 源码与模块：`pkg/resourcemanager/pool/basepool.rs`、`pkg/resourcemanager/pool/lib.rs`、
  `pkg/resourcemanager/pool/Cargo.toml`。
- 直接生产调用：`pkg/resourcemanager/pool/spool/lib.rs`、
  `pkg/resourcemanager/pool/spool/spool.rs`、`pkg/resourcemanager/pool/spool/Cargo.toml`。
- 调度下游：`pkg/resourcemanager/util/util.rs::GoroutinePool`、
  `pkg/resourcemanager/schedule.rs::ResourceManager::Exec`、
  `pkg/resourcemanager/scheduler/cpu_scheduler.rs::CPUScheduler::Tune`。
- Go 对照：`pkg/resourcemanager/pool/basepool.go`、
  `pkg/resourcemanager/pool/spool/spool.go`、`pkg/resourcemanager/schedule.go`。
- 独立测试：`pkg/resourcemanager/pool/basepool_test.rs` 验证溢出回绕；
  `pkg/resourcemanager/pool/migration_aster_unit_test.rs` 验证错误文案、初始名称与时间、
  `Send + Sync`、并发 ID 唯一且连续；
  `pkg/resourcemanager/pool/spool/migration_aster_unit_test.rs` 验证零容量 tune 不更新时间、
  有效扩容更新时间。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter` 与
  `node --file ... --offset 1 --limit 500` 确认 93 行源文件、12 个符号及测试文件使用关系；
  `query BasePool --kind struct` 定位 Rust/Go 类型和相关方法；方法级 `callers/callees`
  无输出，故跨文件边另由精确符号引用搜索核验。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前以任务规定的命令验证本文恰有 11 个固定
  二级标题，并人工复核每个行为结论都能回溯到上述源码、调用点或测试。
