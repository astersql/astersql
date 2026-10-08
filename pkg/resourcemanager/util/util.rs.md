# `pkg/resourcemanager/util/util.rs`

## 文件定位

本文件是 `astersql-resourcemanager-util` crate 的公共契约层，源码由 `pkg/resourcemanager/util/lib.rs` 的 `pub mod util` 声明并通过 `pub use util::*` 再导出。它不执行调度，也不拥有任务队列，而是定义资源管理器、调度器、分片池映射和具体池实现之间共同使用的时长原语、池接口、池容器及组件标识。

crate 边界由 `pkg/resourcemanager/util/Cargo.toml` 确认：包名为 `astersql-resourcemanager-util`，库入口为 `lib.rs`，未声明 feature 或第三方运行时依赖。本文件自身只依赖标准库的原子类型、共享所有权、时间类型。

## 核心职责

1. `AtomicDuration` 为调度间隔提供可跨线程读写的非负纳秒值；全局 `MinSchedulerInterval` 用它保存默认的 200 ms 防抖间隔。
2. `GoroutinePool` 把具体执行池抽象成资源管理器所需的最小控制面：释放、调容、最近调容时间、当前容量、运行数、名称和原始并发度。
3. `PoolContainer` 将一个共享池对象与 `Component` 标签绑定，使同一套调度流程能按组件分类处理。
4. `Component` 及同名常量为 DDL、分布式任务、检查表和导入任务提供稳定整数标识；`MaxOverclockCount` 把单池最大超频增量固定为 1。

这些定义的直接消费链可在 `pkg/resourcemanager/rm.rs`、`pkg/resourcemanager/util/shard_pool_map.rs` 和 `pkg/resourcemanager/schedule.rs` 中复核。

## 主要符号

- `pub struct AtomicDuration(AtomicU64)`：以纳秒数保存 `Duration` 的内部包装；内部字段私有，调用方只能经 `new`、`Load`、`Store` 访问。
- `AtomicDuration::new(value: Duration) -> Self`：`const fn` 构造器，将 `value.as_nanos()` 转为 `u64`，因此可用于静态初始化。
- `AtomicDuration::Load(&self) -> Duration`：以 `Ordering::SeqCst` 原子读取纳秒数，再构造 `Duration`。
- `AtomicDuration::Store(&self, value: Duration)`：以 `Ordering::SeqCst` 原子替换纳秒数；无需可变引用。
- `pub static MinSchedulerInterval: AtomicDuration`：默认 200 ms。它是可通过 `Store` 更新的进程级静态值，不是编译期常量。
- `pub static MaxOverclockCount: i32`：值为 1；本文件没有修改入口，调度执行时读取它。
- `pub trait GoroutinePool: Send + Sync`：对象安全的线程安全池接口。七个方法分别是 `ReleaseAndWait`、`Tune`、`LastTunerTs`、`Cap`、`Running`、`Name`、`GetOriginConcurrency`。
- `pub struct PoolContainer`：公开字段 `Pool: Arc<dyn GoroutinePool>` 与 `Component: Component`。`Arc` 允许注册表、调度循环和外部持有者共享同一池实例。
- `#[repr(i32)] pub enum Component`：`UNKNOWN = 0`、`DDL = 1`、`DistTask = 2`、`CheckTable = 3`、`ImportInto = 4`，派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`。
- `UNKNOWN`、`DDL`、`DistTask`、`CheckTable`、`ImportInto`：对应枚举变体的公开常量别名，保留 Go 风格调用形式。

## 执行流程

本文件的类型参与的主流程如下：

1. 具体池实现实现 `GoroutinePool`。生产实现证据是 `pkg/resourcemanager/pool/spool/spool.rs` 中 `impl GoroutinePool for Pool`；测试实现见 `pkg/resourcemanager/util/mock_gpool.rs`。
2. `ResourceManager::Register`（`pkg/resourcemanager/rm.rs`）接收 `Arc<dyn GoroutinePool>`、名称和 `Component`，构造 `PoolContainer`，再交给 `ShardPoolMap::Add` 保存。
3. `ResourceManager::Start` 每 100 ms 唤醒一次后台循环；`schedule` 通过 `ShardPoolMap::Iter` 取得容器。标记为 `DistTask` 的池被直接跳过，其余池进入 `schedulePool`。
4. `schedulePool` 读取 `Running`、`Cap`，并把 `Component` 与 `&dyn GoroutinePool` 交给各 `Scheduler::Tune` 决策。
5. `Exec` 对非 `Hold` 命令读取 `LastTunerTs`。只有已过去时间严格大于 `MinSchedulerInterval.Load()` 时才调容。缩容调用 `Tune(Cap - 1)`；扩容调用 `Tune(Cap + 1)`，且不得超过 `GetOriginConcurrency() + MaxOverclockCount`。
6. 资源管理器注销时只从映射删除容器；池自身何时调用 `ReleaseAndWait` 由具体池拥有者决定，本文件不自动触发释放。

`AtomicDuration` 的独立读写流程很短：构造或 `Store` 将时长转为纳秒整数，`Load` 再还原成 `Duration`；完整值通过一次原子操作发布，不会出现撕裂读取。

## 数据与状态

`AtomicDuration` 的唯一状态是一个 `AtomicU64` 纳秒计数。所有读写采用全序一致的 `SeqCst`，所以不同线程能在同一全局顺序中观察更新。它没有比较交换、增减或快照版本，多个写者采用“最后一次进入全序的写入生效”语义。

`PoolContainer` 自身没有锁：池实例通过 `Arc` 共享，组件标签是可复制枚举。容器进入 `ShardPoolMap` 后又被包装为 `Arc<PoolContainer>`，并由各分片的 `RwLock<HashMap<...>>` 保护（见 `pkg/resourcemanager/util/shard_pool_map.rs`）。

`Component` 的 `#[repr(i32)]` 和显式判别值保证当前 Rust 侧整数布局稳定；常量别名只是值复制，不维护额外状态。`MaxOverclockCount` 虽声明为 `static`，但类型不是原子且没有内部可变性，运行时不可安全修改。

## 依赖与调用关系

上游调用者与持有者：

- `pkg/resourcemanager/rm.rs` 使用 `Component`、`GoroutinePool`、`PoolContainer` 完成注册，并由 `ShardPoolMap` 管理容器。
- `pkg/resourcemanager/schedule.rs` 是这些契约的核心消费者：读取 `DistTask`、`MinSchedulerInterval`、`MaxOverclockCount`，并调用池接口完成调度决策与执行。
- `pkg/resourcemanager/scheduler/scheduler.rs` 的 `Scheduler::Tune` 接收 `Component` 和 `&dyn GoroutinePool`，将池观测面提供给具体调度器。
- `pkg/resourcemanager/pool/spool/spool.rs` 为生产 `Pool` 实现 `GoroutinePool`；`pkg/resourcemanager/util/mock_gpool.rs` 为测试 mock 实现同一接口。
- `pkg/resourcemanager/util/shard_pool_map.rs` 存储 `PoolContainer`，提供注册、删除和遍历入口。

下游依赖全部来自标准库：`Arc` 提供共享所有权，`AtomicU64`/`Ordering` 提供同步，`Duration` 表示非负间隔，`SystemTime` 表示最近调容的墙钟时间。RustCodeGraph 已索引本文件的 30 个符号，并显示该文件被 `rm.rs`、`schedule.rs`、`scheduler.rs`、`spool.rs`、`mock_gpool.rs`、`shard_pool_map.rs` 等文件使用；精确 trait/常量 `callers` 查询未生成边，因此上述动态分派和字段引用由源码检索核实。

## 错误处理与边界

- 本文件的 API 都不返回 `Result`。具体池若释放或调容失败，当前 trait 没有传播错误的通道；实现只能自行处理、记录或 panic。扩展错误语义属于跨实现的兼容性变更。
- `AtomicDuration::new`/`Store` 将 `u128` 的 `Duration::as_nanos()` 用 `as u64` 转换。超过 `u64::MAX` 纳秒的时长会截断低 64 位，而不是报错或饱和；调用方应把值限制在该范围内。
- Rust `Duration` 只能表示非负时长，不能表示 Go `time.Duration` 可表达的负值。当前调度常量与测试都只使用非负值，因此现有路径不触及此差异。
- `SystemTime` 可能因墙钟回拨或实现返回未来时间而使 `elapsed()` 失败。`pkg/resourcemanager/schedule.rs` 当前用 `unwrap_or_default()` 把该情况视为已过去 0 时间，从而跳过本轮调容。
- `Component::UNKNOWN` 的注释说明它主要用于测试，但类型系统并不禁止生产注册它。未知的未来整数也不会自动转换为枚举值；反序列化边界需单独校验。
- `PoolContainer` 的字段公开，构造时不会校验名称、容量、组件与具体池内部组件是否一致。

## 并发与资源生命周期

`GoroutinePool: Send + Sync` 是最重要的并发不变量：任何放入 `Arc<dyn GoroutinePool>` 的实现都必须可安全跨线程转移和共享。trait 本身不规定实现内部使用锁还是原子，也不保证多个 `Tune` 调用串行；这些保证由具体实现承担。

`AtomicDuration` 使用 `SeqCst`，为全局间隔更新提供强于当前单值读写最低需要的排序保证。`PoolContainer` 的 `Arc` 只管理生命周期，不替池内部状态提供同步。注册映射删除最后一个登记项时，如果外部仍持有 `Arc`，池会继续存活。

`ReleaseAndWait` 的契约是释放池并等待在途任务结束，但容器没有 `Drop` 实现，`ResourceManager::Unregister` 也只删除映射项；因此安全关闭必须由拥有者显式调用。不要在映射遍历回调中假定释放已经发生。具体生产 `Pool` 的计数、worker 退出和等待机制应到 `pkg/resourcemanager/pool/spool/spool.rs` 继续阅读。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/resourcemanager/util/util.go`：

- Go 的 `atomic.NewDuration(200 * time.Millisecond)` 对应 Rust 的 `AtomicDuration::new(Duration::from_millis(200))`；`Load`/`Store` 名称保持一致。
- Go `GoroutinePool` 的七个方法在 Rust trait 中逐项保留，参数与返回值也保持同形；Go `string` 值返回在 Rust 中收紧为借用的 `&str`，避免每次查询分配。
- Go 接口值对应 Rust `Arc<dyn GoroutinePool>`。Rust 额外显式要求 `Send + Sync`，并用 `Arc` 表达共享生命周期。
- Go `PoolContainer` 的两个字段和 Rust 完全同名；Rust 的字段名保留非惯用的大写形式以便迁移对照。
- Go `Component int` 的 `iota` 顺序 0..4 对应 Rust `#[repr(i32)]` 的显式值；五个包级常量别名保留 Go API 名称。
- Go `time.Duration`/`atomic.Duration` 以有符号纳秒工作，而 Rust 包装使用无符号纳秒；负值和超出 `u64` 纳秒的输入并非完整语义等价。当前已验证用例只覆盖非负、可表示范围。
- 调度使用方式也保持一致：对照 `pkg/resourcemanager/schedule.go` 与 `schedule.rs`，两侧都跳过 `DistTask`，用最小间隔防抖，并把超频限制在原始并发加 1。Rust 对整数增减使用 `wrapping_add`/`wrapping_sub` 显式模拟 Go `int32` 运行时回绕。

Go 同目录没有独立的 `util_test.go`；Go 侧容器行为由 `pkg/resourcemanager/util/shard_pool_map_test.go`、调度行为由 `pkg/resourcemanager/schedule_test.go` 覆盖。Rust 的直接原子时长测试位于独立文件 `pkg/resourcemanager/util/util_test.rs`，常量与枚举迁移断言还位于 `pkg/resourcemanager/util/migration_aster_unit_test.rs`。

## 扩展指南

- 新增组件时，在 `Component` 中追加显式且不复用的整数值，并增加同名常量别名；同步检查 `schedule` 的组件过滤、所有 `Scheduler::Tune` 实现、注册调用点以及 Go `util.go`。至少扩展 `migration_aster_unit_test.rs` 的数值断言，且保持测试逻辑在独立测试文件中。
- 新增池观测或控制方法时，先确认资源管理器或调度器确实需要该能力；随后必须同步所有 trait 实现，当前至少包括生产 `Pool`、`MockGPool` 和资源管理器相关测试内的 `TestPool`/`MockPool`。新增可能失败的方法应一次性设计错误传播，避免实现间静默分歧。
- 修改调度间隔表示时，应优先消除 `u128 -> u64` 静默截断，并决定是否需要兼容 Go 的负时长。任何排序放宽都要说明跨线程可见性依据。
- 修改 `PoolContainer` 生命周期时，应明确 `Unregister` 是否负责 `ReleaseAndWait`；自动释放可能阻塞持锁或调度线程，也可能改变当前“外部 `Arc` 可继续使用”的兼容行为。
- 性能方面，本文件的原子读取位于周期调度热路径，但每 100 ms 每池通常只读取一次；新增锁或分配应结合 `schedule.rs` 的遍历频率评估。组件判定和容量查询应保持轻量。
- 相关 Rust 测试应继续放在独立文件：本文件的原子值与常量测试放 `util_test.rs`/`migration_aster_unit_test.rs`，容器并发与映射行为放 `shard_pool_map_test.rs`，调度策略放 `pkg/resourcemanager/schedule_test.rs` 或现有资源管理器测试文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/resourcemanager/util` 确认 `util.rs`、`util_test.rs`、Go 对照和相邻实现均已索引。
- RustCodeGraph：`node --file pkg/resourcemanager/util/util.rs --offset 1 --limit 260` 读取了目标文件全部 110 行；精确查询确认 `AtomicDuration`、`GoroutinePool`、`PoolContainer`、`Component`、`MinSchedulerInterval`、`MaxOverclockCount` 等符号。对这些 trait/常量执行的 `callers`/`callees` 没有返回可用边，故使用下列直接源码证据补足动态分派关系。
- crate 与入口：`pkg/resourcemanager/util/Cargo.toml`、`pkg/resourcemanager/util/lib.rs`。
- 生产调用链：`pkg/resourcemanager/rm.rs`、`pkg/resourcemanager/schedule.rs`、`pkg/resourcemanager/scheduler/scheduler.rs`、`pkg/resourcemanager/util/shard_pool_map.rs`、`pkg/resourcemanager/pool/spool/spool.rs`。
- Go 对照：`pkg/resourcemanager/util/util.go`、`pkg/resourcemanager/schedule.go`、`pkg/resourcemanager/rm.go`、`pkg/resourcemanager/util/shard_pool_map_test.go`、`pkg/resourcemanager/schedule_test.go`。
- Rust 独立测试：`pkg/resourcemanager/util/util_test.rs` 验证纳秒精度的构造、读取、更新；`pkg/resourcemanager/util/migration_aster_unit_test.rs` 验证 200 ms、超频上限、枚举别名及容器行为；`pkg/resourcemanager/schedule_test.rs` 和 `pkg/resourcemanager/migration_aster_unit_test.rs` 验证调度边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核唯一新增产物与引用路径。
