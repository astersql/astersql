# `pkg/resourcemanager/util/mock_gpool.rs`

## 文件定位

该文件位于 `astersql-resourcemanager-util` crate。crate 入口 `pkg/resourcemanager/util/lib.rs` 通过 `pub mod mock_gpool` 声明模块，并用 `pub use mock_gpool::*` 导出其公开符号；`pkg/resourcemanager/util/Cargo.toml` 将 `lib.rs` 指定为库入口，且没有为本模块声明额外 feature 或第三方依赖。

这是资源管理器测试支持层中的内存 Mock，而不是真正执行任务的线程池。它以最小状态实现 `pkg/resourcemanager/util/util.rs` 中的 `GoroutinePool` trait，使调度器可以通过统一的 `Arc<dyn GoroutinePool>` 边界读取名称和容量、执行调容并检查原始并发度。当前 Rust 直接使用点包括 `pkg/resourcemanager/schedule_test.rs`、`pkg/resourcemanager/util/migration_aster_unit_test.rs` 和 `pkg/resourcemanager/util/shard_pool_map_test.rs`。

## 核心职责

- `MockGPool` 保存池名、当前并发容量和创建时的原始并发度，供测试构造可被资源管理器识别的池。
- `NewMockGPool` 用同一个初始并发值初始化“当前容量”和“原始容量”；后续 `Tune` 只改变前者，因此测试可以验证超频上限仍以初始值为基准。
- `impl GoroutinePool for MockGPool` 将 Mock 接入生产调度抽象。`pkg/resourcemanager/schedule.rs::ResourceManager::Exec` 通过该抽象调用 `LastTunerTs`、`Cap`、`Name`、`Tune` 和 `GetOriginConcurrency`。
- 本文件不创建工作线程、不接受任务，也不维护真实运行指标。统计类接口多数故意 `panic!("implement me")`，只能用于不触发这些路径的测试。

## 主要符号

- `pub struct MockGPool`：Mock 的唯一数据类型。字段均为私有，外部只能通过公开方法或 `GoroutinePool` trait 观察状态。
  - `name: String`：构造后不变，由 `Name(&self) -> &str` 借用返回。
  - `concurrency: AtomicI32`：可调的当前容量；`Tune` 写入，`Cap` 读取。
  - `origin_concurrency: i32`：初始容量快照；`GetOriginConcurrency` 返回该值，调容不会覆盖它。
- `pub fn NewMockGPool(name: String, concurrency: i32) -> MockGPool`：按值接收名称并返回具体对象，而不是 `Arc` 或 trait object；调用者在需要共享时自行包装，例如 `schedule_test.rs` 使用 `Arc::new(...)`。
- 已实现的固有方法：`Tune`、`LastTunerTs`、`Cap`、`Name`、`GetOriginConcurrency`。
- 明确未实现并会 panic 的固有方法：`ReleaseAndWait`、`MaxInFlight`、`InFlight`、`MinRT`、`MaxPASS`、`LongRTT`、`UpdateLongRTT`、`ShortRTT`、`GetQueueSize`、`Running`。
- `impl GoroutinePool for MockGPool`：实现 trait 要求的七个方法；每个方法显式转发到同名固有方法，避免 trait 方法内部递归调用自身。其中 `ReleaseAndWait` 和 `Running` 仍会进入上述 panic 桩。

## 执行流程

1. 测试调用 `NewMockGPool(name, concurrency)`；构造器保存名称，并将当前容量与原始容量同时设为传入值。
2. 调用者通常把对象包装为 `Arc<dyn GoroutinePool>`，放入 `PoolContainer`。`pkg/resourcemanager/schedule_test.rs::TestSchedulerOverloadTooMuch` 和 `pkg/resourcemanager/util/shard_pool_map_test.rs::pool_container` 展示了这条接线。
3. 调度器执行 `ResourceManager::Exec` 时先调用 `LastTunerTs`。Mock 每次返回“调用时刻减 10 秒”，因此其 elapsed 通常大于 `MinSchedulerInterval` 的 200 毫秒，调容分支可以立即执行；它并不记录真实的最近调容时间。
4. 调度器从 `Cap` 读取当前值。降频时减一后调用 `Tune`；超频时加一，并与 `GetOriginConcurrency() + MaxOverclockCount` 比较，通过后才调用 `Tune`。
5. `Tune` 以原子写更新当前容量；之后 `Cap` 能读到新值，而 `GetOriginConcurrency` 继续返回构造值。`schedule_test.rs` 据此验证初始并发为 1 时最多超频到 2。
6. 若调用测试尚未支持的统计或生命周期方法，则控制流立即 panic，不产生回退值。

## 数据与状态

对象只有三个状态量，没有任务队列、worker 集合或统计窗口。`name` 与 `origin_concurrency` 构造后不可变；`concurrency` 是唯一可变状态。

`concurrency` 使用 `AtomicI32`，`Tune` 与 `Cap` 均采用 `Ordering::SeqCst`。这保证不同线程对容量更新具有全序可见性，且无需 `&mut self` 或外部锁。代码不校验并发值是否为正，也不做饱和处理；直接调用 `Tune` 时，任意 `i32` 都会被保存。调度器侧的加减使用 wrapping 算术并承担策略约束，Mock 本身不重复实现策略。

`LastTunerTs` 没有对应字段。每次调用都从 `SystemTime::now()` 动态减去 10 秒，所以结果会随调用时间向前移动，并不能用来判断一次具体 `Tune` 发生的时刻。

## 依赖与调用关系

- 标准库下游依赖：`AtomicI32`/`Ordering` 提供无锁容量状态；`SystemTime`/`Duration` 构造调度间隔测试所需的过去时间。
- crate 内下游依赖：`crate::util::GoroutinePool` 定义接入资源管理器所需的 trait。该 trait 还要求 `Send + Sync`；`MockGPool` 的字段类型使其自动满足这两个约束。
- 上游模块：`pkg/resourcemanager/util/lib.rs` 声明并重新导出本模块；在 `cfg(test)` 下，`pkg/resourcemanager/lib.rs` 还通过 `resourcemanager_test_support::util` 向该 crate 的调度测试公开 `MockGPool` 与 `NewMockGPool`。
- 上游行为调用：`pkg/resourcemanager/schedule.rs::ResourceManager::Exec` 通过 trait object 使用 `LastTunerTs -> Cap/GetOriginConcurrency/Name -> Tune` 链路。
- 上游测试：`pkg/resourcemanager/schedule_test.rs` 验证超频上限；`pkg/resourcemanager/util/migration_aster_unit_test.rs` 直接验证名称、容量、原始容量、调容和时间戳；`pkg/resourcemanager/util/shard_pool_map_test.rs` 使用 Mock 构造可注册的池容器。
- RustCodeGraph 的文件节点将 `mock_gpool.rs` 标记为被 `pkg/resourcemanager/schedule_test.rs` 使用；对测试文件装配产生的关系，源码搜索还确认了上述同 crate 测试引用。

## 错误处理与边界

本文件没有 `Result` 返回值或可恢复错误。唯一显式失败方式是未实现方法中的 panic。因此，Mock 当前适合验证调容和容器注册，不适合走需要 `Running` 的降频筛选、真实运行统计或释放等待的路径。例如 `ResourceManager::Schedule` 的 Downclock 防护会读取 `Running`，若传入本 Mock 则会 panic。

构造器与 `Tune` 不拒绝零值或负容量；安全取值依赖调用测试和上层调度策略。`SystemTime::now() - Duration::from_secs(10)` 在常规系统时间下有效，但表达的是固定偏移的测试值，而非持久化时间状态。`UpdateLongRTT` 即使收到合法闭包也不会调用闭包，而是直接 panic。

## 并发与资源生命周期

`MockGPool` 没有显式 `Drop`、后台线程、通道、锁或异步任务。所有权由调用者管理；常见模式是用 `Arc` 共享，并以 `Arc<dyn GoroutinePool>` 存入 `PoolContainer`。对象释放时只按 Rust 默认规则释放 `String` 和原子值。

并发读写当前容量是安全的，且使用最强的顺序一致性内存序。名称和原始容量只读，无需同步。这个安全性不等价于真实池生命周期：`ReleaseAndWait` 未实现，不能用它证明任务排空、worker 停止或资源回收行为。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/resourcemanager/util/mock_gpool.go`。Rust 保留了 Go 的类型名、构造器名和 CamelCase 方法名，并通过 `#![allow(non_snake_case)]` 接受这种迁移命名。

- 两版都保存 `name`、`concurrency`、`originConcurrency`/`origin_concurrency`，构造时两个并发字段取同一值，`Tune` 只更新当前容量。
- 两版 `LastTunerTs` 都返回当前时间减 10 秒；两版相同的统计与生命周期方法都以 `"implement me"` panic。
- Go 构造器返回 `*MockGPool`，Rust 返回拥有所有权的 `MockGPool`，由调用者按需包装 `Arc`。
- Go 的 `concurrency` 是普通 `int32`；Rust 使用 `AtomicI32`，以满足 `GoroutinePool: Send + Sync` 下共享 trait object 的并发安全要求。这是表示方式差异，不改变测试可见的 Tune/Cap 语义。
- Go `Name` 返回 `string` 值；Rust 返回借用的 `&str`，避免复制并把生命周期绑定到池对象。
- Go 类型以方法集隐式满足接口；Rust 需要显式 `impl GoroutinePool`，且该 trait 只包含资源管理器实际使用的七个方法。其他统计方法仍作为固有方法保留，以对齐 Go 表面 API，但当前不属于 Rust trait 契约。

相关 Go 证据包括 `pkg/resourcemanager/schedule_test.go` 的超频上限测试，以及 `pkg/resourcemanager/util/shard_pool_map_test.go` 中用 Mock 填充池映射的测试。Rust 对应测试分别位于 `pkg/resourcemanager/schedule_test.rs`、`pkg/resourcemanager/util/shard_pool_map_test.rs`，另有 `pkg/resourcemanager/util/migration_aster_unit_test.rs` 补充直接语义验证。

## 扩展指南

- 若增加 `GoroutinePool` trait 方法，应同步修改 `pkg/resourcemanager/util/util.rs`、本文件的 trait 实现、所有其他实现和独立测试；不要把测试逻辑内嵌到本生产源文件。
- 若测试需要 `Running`、释放等待或 RTT/队列指标，应先为 `MockGPool` 增加明确状态字段与可控构造/设置接口，再替换对应 panic。需要同步核对 Go Mock 是否也应扩展，避免 Rust 测试替身无依据地偏离 Go 行为。
- 若改变调容语义，应优先修改 `Tune`、`Cap`、`GetOriginConcurrency` 或 `LastTunerTs`，并同步更新 `pkg/resourcemanager/util/migration_aster_unit_test.rs`；涉及资源管理器上限时还应更新 `pkg/resourcemanager/schedule_test.rs`。
- 若改变共享状态，继续保证 `MockGPool` 满足 `Send + Sync`。新增多个相关状态时需明确是否要求一致快照；单独原子字段只能保证各字段自身的原子性。
- 若要记录真实最近调容时间，应明确并发更新方式，并调整当前“始终绕过 200 毫秒间隔”的测试假设；否则可能让连续两次 `Exec` 的现有测试不再进入第二次上限判断。
- 不应把本 Mock 当作生产池扩展入口。真实 worker 生命周期和指标实现位于 `pkg/resourcemanager/pool/workerpool/workerpool.rs`、`pkg/resourcemanager/pool/spool/spool.rs` 等实现中。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录中的 `mock_gpool.rs`、Go 对照和相关 Rust 测试均已索引。
- RustCodeGraph 文件节点：`pkg/resourcemanager/util/mock_gpool.rs` 共 158 行、25 个索引符号，并显示 `pkg/resourcemanager/schedule_test.rs` 的使用关系；文件源码确认所有字段、方法、panic 桩和 trait 转发。
- RustCodeGraph 文件节点：`pkg/resourcemanager/util/util.rs` 确认 `GoroutinePool: Send + Sync` 的七个方法，以及 `MinSchedulerInterval = 200ms`、`MaxOverclockCount = 1`。
- RustCodeGraph 文件节点：`pkg/resourcemanager/schedule.rs` 确认 `Exec` 的间隔检查、当前容量读取、原始容量上限判断和 `Tune` 调用链。
- RustCodeGraph 文件节点：`pkg/resourcemanager/util/mock_gpool.go`、`pkg/resourcemanager/schedule_test.go`、`pkg/resourcemanager/util/shard_pool_map_test.go` 用于核对 Go 字段、方法、panic 和测试意图。
- Rust 独立测试：`pkg/resourcemanager/util/migration_aster_unit_test.rs::mock_pool_preserves_name_origin_and_tuning_behavior` 验证名称、Tune/Cap、原始容量不变以及至少约 9 秒的时间偏移；`pkg/resourcemanager/schedule_test.rs::TestSchedulerOverloadTooMuch` 验证超频上限；`pkg/resourcemanager/util/shard_pool_map_test.rs::TestShardPoolMap` 验证 Mock 可作为池容器成员参与映射操作。
- crate 边界：`pkg/resourcemanager/util/Cargo.toml` 与 `pkg/resourcemanager/util/lib.rs` 确认库入口、模块声明和公开再导出。本任务是纯文档分析，按任务要求未运行 Cargo。
