# [`pkg/util/syncutil/mutex_sync.rs`](mutex_sync.rs)

## 文件定位

该文件是 `astersql-util-syncutil` crate 的普通同步锁后端，对应 Go 的 `pkg/util/syncutil/mutex_sync.go`。crate 入口 `pkg/util/syncutil/lib.rs` 始终声明 `mutex_sync` 和 `mutex_deadlock` 两个模块；未启用 Cargo feature `deadlock` 时，入口以 `pub use mutex_sync::*` 将本文件的公开项提升为 crate API。启用该 feature 时，对外改为导出 `mutex_deadlock`，本模块仍会被编译为公开子模块，但不再是 crate 根的默认锁表面。

`pkg/util/syncutil/Cargo.toml` 指定库入口为 `lib.rs`，只依赖启用了 `deadlock_detection` 能力的 `parking_lot 0.12`。本文件本身不启动检测逻辑；它代表默认、无检测的构建变体。

## 核心职责

本文件只承担三项兼容接线职责：

1. 将 `parking_lot::Mutex` 原名再导出为 `Mutex`。
2. 将 `parking_lot::RwLock` 以 Go 风格名称 `RWMutex` 再导出。
3. 公开常量 `EnableDeadlock = false`，让调用方能识别当前默认变体不提供死锁检测。

因此它不是一套自研锁算法，也没有额外包装层、统计、超时或死锁报告逻辑。实际加锁、等待、公平性和 guard 行为由 `parking_lot` 类型提供；本文件的价值是稳定 AsterSQL 的跨语言命名与 feature 切换边界（证据：`mutex_sync.rs:24-31`、`lib.rs:20-26`）。

## 主要符号

- `pub use parking_lot::Mutex`：公开泛型互斥锁。调用方用 `Mutex::new(value)` 创建锁，通过 `lock()` 得到独占 guard；例如 `pkg/executor/internal/applycache/apply_cache.rs` 的 `ApplyCache.cache` 用它串行访问本身不保证并发安全的 `SimpleLRUCache`。
- `pub use parking_lot::RwLock as RWMutex`：公开泛型读写锁，并保留 Go 侧 `RWMutex` 的大写拼写。`migration_aster_unit_test.rs::rwmutex_allows_readers_and_blocks_a_writer` 证明多个读 guard 可并存，读 guard 存续期间 `try_write()` 返回 `None`，读者退出后写入可见。
- `pub const EnableDeadlock: bool = false`：普通变体的能力标志。`pkg/server/tidb_library_test.rs::test_memory_leak` 根据 crate 根导出的该值选择内存增长上限；迁移测试同时断言普通变体为 `false`、检测变体为 `true`。
- 文件级 `allow(dead_code, non_snake_case, non_upper_case_globals)`：允许兼容 Go 导出名及并非每个构建都直接使用的变体符号，不改变运行时行为。

本文件没有自定义 struct、trait、函数、`impl` 或条件编译项；`Mutex` 与 `RWMutex` 是外部类型的再导出，不是本地 wrapper 或 type alias。

## 执行流程

默认构建中的接线与运行流程如下：

1. Cargo 将 `pkg/util/syncutil/lib.rs` 作为 `astersql-util-syncutil` 的库入口。
2. `lib.rs` 声明 `mutex_sync`；在 `cfg(not(feature = "deadlock"))` 下把本文件的公开项再导出到 crate 根。
3. 下游 crate 通过 `astersql_util_syncutil::Mutex`、`RWMutex` 或依赖别名访问这些类型。例如 `ApplyCache::NewApplyCache` 构造 `syncutil::Mutex::new(...)`，`ApplyCache::get`/`put` 获取 guard 后操作缓存。
4. `lock()`、`read()` 或 `write()` 阻塞直至获得相应 guard；guard 离开作用域时释放锁。`try_write()` 等非阻塞 API 直接沿用 `parking_lot` 的返回约定。
5. 若构建启用 `deadlock` feature，步骤 2 改为导出 `mutex_deadlock::*`，crate 根消费者无需更换导入路径。

`pkg/util/cteutil/storage.rs::StorageMutex` 展示了一个重要边界：当 Go 风格接口要求 `Lock` 与 `Unlock` 分成两个方法时，不能简单让 Rust guard 在 `lock()` 返回时析构；该调用方另用 `Mutex<bool> + Condvar` 显式保存锁定状态。本文件只提供 RAII 锁原语，不模拟分离式解锁 API。

## 数据与状态

本文件自身没有可变静态数据、全局锁实例或堆分配。`EnableDeadlock` 是编译期布尔常量。被保护的数据由每个调用方作为类型参数 `T` 放入 `Mutex<T>` 或 `RWMutex<T>`，锁状态和等待队列则封装在 `parking_lot` 实现内部。

锁 guard 是访问受保护数据的唯一正常入口：互斥 guard 提供独占访问；读写锁的读 guard 可并存，写 guard 与所有其他 guard 互斥。状态的所有权和生命周期属于具体锁实例，而不是此模块级单例。

## 依赖与调用关系

下游依赖只有 `parking_lot`：`Mutex` 直接重导出，`RwLock` 重命名后重导出。crate 配置中的 `deadlock` feature 控制 `lib.rs` 选择哪个模块，而不是改变本文件内部逻辑。

RustCodeGraph 对 `pub use` 的跨 crate 调用边未建立精确反向边（文件节点显示 `used by 0 files`），因此真实调用点由精确引用搜索补齐：

- `pkg/executor/internal/applycache/apply_cache.rs`：`ApplyCache.cache` 使用 `syncutil::Mutex`，所有缓存读写先取得独占 guard。
- `pkg/util/cteutil/storage.rs`：`StorageMutex.locked` 使用该 crate 的 `Mutex<bool>`，配合 `Condvar` 模拟 Go 的分离式 `Lock`/`Unlock`。
- `pkg/executor/test/oomtest/oom_test.rs`：全局 `OnceLock<Mutex<OomCapture>>` 保护测试日志捕获状态。
- `pkg/server/tidb_library_test.rs`：读取 `EnableDeadlock`，使测试阈值与锁变体的额外开销相适配。

Cargo 清单显示 `pkg/executor`、`pkg/session`、`pkg/server`、`pkg/domain`、若干 executor 子 crate 及 util crate 以路径依赖消费 `astersql-util-syncutil`；仓库根还通过 `facade_util_syncutil` 将其纳入统一 facade。以上关系说明该文件位于通用并发基础设施层，而非某条 SQL 业务流程的专用实现。

## 错误处理与边界

本模块没有 `Result`、错误类型或显式 panic 路径，也不拦截底层锁 API 的行为。获取阻塞锁时没有在本层设置超时；普通变体不会生成死锁检测报告，调用者若发生锁顺序循环，`EnableDeadlock = false` 只说明能力状态，并不会主动恢复。

`migration_aster_unit_test.rs::mutex_remains_usable_after_a_holder_panics_like_go` 验证持锁线程 panic 后，其他线程仍可取得同一 `Mutex` 并读取 panic 前写入的值。这说明本选型没有 `std::sync::Mutex` 式的 poisoning 恢复错误，调用方也不需要处理 `PoisonError`；但共享数据在 panic 时是否仍满足业务不变量，仍由调用方负责。

类型命名兼容不等于调用语法完全兼容：Go wrapper 支持零值直接使用，并通过 `Lock`/`Unlock`、`RLock`/`RUnlock` 操作；Rust 消费者通常显式构造泛型锁并依赖 guard 的作用域释放。跨 `await`、回调或长耗时操作持有 guard 会扩大临界区，必须在调用方审查。

## 并发与资源生命周期

锁实例可被放入 `Arc` 等共享所有权容器后跨线程使用。迁移测试 `mutex_serializes_parallel_updates` 用 8 个线程各执行 1,000 次加一，最终得到 8,000，验证互斥更新不会丢失；`rwmutex_allows_readers_and_blocks_a_writer` 用 barrier 固定两个读者的持锁区间，验证写者被排除。

资源释放依赖 RAII：成功获取锁后产生 guard，guard 被 `drop` 或离开词法作用域时解锁。本模块不创建线程、任务、channel、timer 或后台检测器，也没有需要显式关闭的资源。普通变体尤其不会调用 `mutex_deadlock.rs::init` 或维护检测线程；迁移测试只对检测变体显式调用 `init()`。

安全扩展时应保持两个关键不变量：所有受保护状态只能在对应 guard 存活期间访问；多锁操作必须建立稳定的获取顺序，因为默认变体不能发现或打破死锁。

## 与 Go 版本的对应关系

Go 文件 `pkg/util/syncutil/mutex_sync.go` 受 `//go:build !deadlock` 控制，定义 `EnableDeadlock = false`，并分别以嵌入 `sync.Mutex`、`sync.RWMutex` 的 struct 暴露 `Mutex` 与 `RWMutex`。Rust 侧由 `lib.rs` 的 Cargo feature 条件导出承担等价的构建选择，由 `parking_lot` 类型再导出承担普通锁能力。

保持一致的语义包括：普通变体能力标志为 false、互斥更新串行化、多读者共享且写者互斥，以及持锁执行路径异常退出后锁仍可继续使用。上述内容分别由 `build_variants_expose_the_go_enable_deadlock_values`、`mutex_serializes_parallel_updates`、`rwmutex_allows_readers_and_blocks_a_writer` 和 `mutex_remains_usable_after_a_holder_panics_like_go` 覆盖。

需要注意的表达差异是：Go 类型是嵌入标准库锁的具名 wrapper，具有可用零值和显式解锁方法；Rust 类型是 `parking_lot` 泛型类型的直接再导出，通过 `new/default` 初始化并由 guard 自动解锁。Rust 的 `RWMutex` 只是公开重命名，底层真实类型名仍是 `RwLock`。Go 同路径没有独立 `mutex_sync_test.go`；Rust 的对齐测试集中在同目录独立文件 `migration_aster_unit_test.rs`，没有嵌入生产源文件。

## 扩展指南

- 若要增加所有锁变体共有的公开能力，先判断 `parking_lot` 原生 API 是否已满足；若必须增加 wrapper，应同时评估 `mutex_sync.rs` 与 `mutex_deadlock.rs` 的同名表面，避免 feature 切换后调用方失配。
- 若要改变变体选择，修改点在 `pkg/util/syncutil/lib.rs` 和 `Cargo.toml` 的 `deadlock` feature，而不是在本文件加入运行时分支；同时保持 `EnableDeadlock` 与实际导出一致。
- 若要增加检测、日志、超时或锁顺序跟踪，应放入检测变体或明确的新包装层，不能让默认变体的 `false` 能力标志与实际行为矛盾。
- 新增或修改并发语义时，应在独立的 `pkg/util/syncutil/migration_aster_unit_test.rs` 中补测试，不要把测试逻辑写进 `mutex_sync.rs`。至少覆盖互斥结果、读写排斥、panic 后可用性以及两个 feature 表面的兼容性。
- 兼容风险主要是下游公开类型变化、Go/Rust 初始化与解锁模型差异；性能风险主要来自新增包装、统计或扩大临界区。修改公开导出前应检查全部 `astersql-util-syncutil` Cargo 消费者和 `syncutil::Mutex`/`RWMutex` 引用。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列出 `lib.rs`、两个锁变体、Go 对照与独立迁移测试。
- RustCodeGraph `node --file pkg/util/syncutil/mutex_sync.rs`：核对完整 31 行源码及三个公开项；`query EnableDeadlock`/`callers` 也暴露了 re-export 反向边未解析的索引限制。
- RustCodeGraph `node`：阅读 `pkg/util/syncutil/lib.rs`、`mutex_sync.go`、`migration_aster_unit_test.rs` 及直接消费点 `apply_cache.rs`、`storage.rs`、`oom_test.rs`、`tidb_library_test.rs`。
- 配置与引用搜索：阅读 `pkg/util/syncutil/Cargo.toml`，并以 `rg` 核对 `astersql-util-syncutil`、`astersql_util_syncutil`、`EnableDeadlock`、`syncutil::Mutex` 和 `RWMutex` 的 Cargo/源码引用。
- 测试证据：`pkg/util/syncutil/migration_aster_unit_test.rs` 是目标模块的独立 Rust 测试文件；Go 同路径无专用 `mutex_sync_test.go`，其原始实现 `mutex_sync.go` 是语义对照依据。本纯文档任务按计划未运行 Cargo。
