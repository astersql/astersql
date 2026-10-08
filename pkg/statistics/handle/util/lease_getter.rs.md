# `pkg/statistics/handle/util/lease_getter.rs`

## 文件定位

本文件属于 `astersql-statistics-handle-util` crate；crate 根 `pkg/statistics/handle/util/lib.rs` 以 `pub mod lease_getter` 声明模块，并通过 `pub use lease_getter::*` 重导出其公开 API。它对应 Go 文件 `pkg/statistics/handle/util/lease_getter.go`，目标是把“统计缓存租约”封装成可在多个线程间共享、运行时可更新的窄接口。

当前 Rust 主统计句柄尚未接入这里的具体实现：仓库搜索只在本文件发现 `AtomicLeaseGetter` 和 `new_lease_getter`，而 `pkg/statistics/handle/handle.rs` 的 `Handle<B>` 仍直接保存 `lease: Duration`，通过其固有方法 `Handle::lease`/`Handle::set_lease` 读写。另一方面，`pkg/statistics/handle/types/interfaces.rs` 已将本文件的 `LeaseGetter` 作为组合 trait `StatsHandle` 的父能力。因此，本文件目前既是接口层契约，也是一个可复用但尚未被 Rust 主句柄构造链采用的原子实现。

## 核心职责

- `LeaseGetter` 定义读取和更新统计租约的线程安全协议，并以 `Send + Sync` 约束实现者可跨线程传递和共享。
- `AtomicLeaseGetter` 将租约规范化为纳秒 `u64`，用单个 `AtomicU64` 提供无互斥锁读写。
- `duration_nanos` 处理 `Duration` 到原子存储格式的转换，并在超过 `u64::MAX` 纳秒时饱和截断。
- `new_lease_getter` 隐藏具体实现，直接返回可共享的 `Arc<dyn LeaseGetter>`。

本文件只保存和发布租约值，不负责根据租约调度任务、刷新统计缓存或等待过期；这些消费语义位于调用侧，例如 `pkg/statistics/handle/cache/statscache.rs`、`pkg/statistics/handle/storage/stats_read_writer.rs` 和 `pkg/domain/domain.rs`。

## 主要符号

- `pub trait LeaseGetter: Send + Sync`：公开对象安全 trait。`lease(&self) -> Duration` 取得当前快照；`set_lease(&self, Duration)` 使用共享引用更新值，因此实现必须提供内部可变性和并发安全。
- `pub struct AtomicLeaseGetter { nanos: AtomicU64 }`：公开具体类型，但字段私有，调用方不能绕过转换和内存序直接访问原子值。
- `AtomicLeaseGetter::new(Duration) -> Self`：用 `duration_nanos` 转换初值并初始化原子槽。
- `impl LeaseGetter for AtomicLeaseGetter`：`lease` 以 `Ordering::Acquire` 加载纳秒并用 `Duration::from_nanos` 还原；`set_lease` 以 `Ordering::Release` 发布转换后的纳秒。
- `fn duration_nanos(Duration) -> u64`：模块私有转换函数；先取 `u128` 纳秒，再与 `u64::MAX` 取最小值，最后安全收窄。
- `pub fn new_lease_getter(Duration) -> Arc<dyn LeaseGetter>`：公开工厂，构造 `AtomicLeaseGetter` 后擦除为共享 trait 对象。

文件没有模块级常量、条件编译项、错误类型或异步函数。

## 执行流程

构造流程是：调用 `new_lease_getter(lease)`，进入 `AtomicLeaseGetter::new`，经 `duration_nanos` 将初始值转换为纳秒，初始化 `AtomicU64`，再放入 `Arc` 并向上转型为 `Arc<dyn LeaseGetter>`。RustCodeGraph 确认了 `new_lease_getter -> AtomicLeaseGetter::new -> duration_nanos` 这条文件内调用链。

读取流程是：对 trait 对象或具体实例调用 `LeaseGetter::lease`，以 Acquire 顺序加载一个完整的 `u64` 快照，再构造同纳秒数的 `Duration`。单次读取不会观察到撕裂的中间值。

更新流程是：调用 `LeaseGetter::set_lease`，先由 `duration_nanos` 完成饱和转换，再以 Release 顺序一次性替换原子槽。并发读者会看到更新前或更新后的完整值，不会看到部分写入。

本文件不比较时间戳，也不自行解释零值。消费侧决定零租约的含义；例如 `pkg/domain/domain.rs::load_needed_histograms` 在主句柄租约为零时提前返回，而 `pkg/statistics/handle/storage/stats_read_writer.rs` 仅在租约大于零时应用加载间隔判断。

## 数据与状态

唯一持久状态是 `AtomicLeaseGetter::nanos: AtomicU64`。存储单位固定为纳秒，因而从 `Duration` 转换后没有低于纳秒的额外精度损失（`Duration` 本身也以秒和纳秒表达）。`Duration::ZERO` 对应原子值 `0`。

可表示上界为 `u64::MAX` 纳秒。任何更长的 `Duration` 都被 `duration_nanos` 映射到同一个上界，所以这种转换不是对超大输入的一一映射；读取只能返回饱和值，不能恢复原始超长时长。这是明确的数据不变量，不是错误路径。

`Arc<dyn LeaseGetter>` 共享的是同一个原子槽；克隆 `Arc` 只增加引用计数，不复制租约状态。直接构造多个 `AtomicLeaseGetter` 则各自拥有独立状态。

## 依赖与调用关系

文件的直接依赖全部来自标准库：`std::time::Duration` 表示租约，`std::sync::atomic::AtomicU64` 及 `Ordering` 提供原子读写，`std::sync::Arc` 管理共享所有权。`pkg/statistics/handle/util/Cargo.toml` 将其归入 `astersql-statistics-handle-util`，但该文件本身不使用清单中的其他 AsterSQL crate 依赖，也不受 feature 或 `cfg` 分支控制。

模块出口是 `pkg/statistics/handle/util/lib.rs`。接口层 `pkg/statistics/handle/types/interfaces.rs` 重导出 `LeaseGetter`，并把它列入 `StatsHandle` 的父 trait。`pkg/statistics/handle/cache/statscache.rs` 通过 `stats_util::LeaseGetter::lease(self)` 消费满足 `StatsHandle` 的对象，说明该契约预期参与统计缓存主链。

但是，RustCodeGraph 对精确符号的查询以及全仓 `rg` 均未找到 `new_lease_getter`、`AtomicLeaseGetter` 在本文件之外的 Rust 引用；图查询只确认了文件内工厂调用链。当前实际运行主链的租约更新由 `pkg/domain/domain.rs::set_stats_lease` 调用 `Handle::set_lease`，其存储位于 `pkg/statistics/handle/handle.rs::Handle<B>::lease` 字段，并非本文件的原子槽。扩展或接线时必须保留这一区别，不能假定两处状态会自动同步。

## 错误处理与边界

所有 API 都是无失败返回值：构造、读取、更新不返回 `Result`，也没有显式 panic 分支。原子操作和 `Duration::from_nanos` 对任意 `u64` 都有效。

主要边界是超长租约：`duration_nanos` 使用饱和而非溢出、回绕或报错。调用方若需要拒绝超大配置，必须在进入本接口前验证，因为本文件会静默保存为 `u64::MAX` 纳秒。

Rust `Duration` 不表示负时长，因此 Rust API 从类型层排除了负租约。零值被原样保存，但是否表示禁用异步刷新由消费侧定义。本文件不校验业务范围，也不提供比较并交换、旧值返回、订阅通知或持久化保证。

## 并发与资源生命周期

`LeaseGetter: Send + Sync`、`AtomicU64` 和返回类型 `Arc<dyn LeaseGetter>` 共同允许多个线程持有同一实例，并通过 `&self` 并发读写。没有互斥锁、等待、后台任务或通道，因此本文件不会阻塞，也不存在锁中毒处理。

读取使用 Acquire、写入使用 Release，形成针对同一原子值的发布/获取关系，并保证每次租约更新以一个不可分割的 `u64` 出现。这里没有伴随租约一起发布的其他字段，所以该内存序强于单纯读取数值所需的 Relaxed 语义；修改内存序前仍应明确是否未来会把租约更新作为其他状态的发布边界。

生命周期由 `Arc` 引用计数控制：最后一个强引用释放时，trait 对象和其中的原子槽一并销毁。代码没有循环引用或显式清理逻辑。

## 与 Go 版本的对应关系

`pkg/statistics/handle/util/lease_getter.go` 的 `LeaseGetter`、私有 `leaseGetter`、`NewLeaseGetter`、`Lease` 和 `SetLease` 分别对应 Rust 的 `LeaseGetter`、`AtomicLeaseGetter`、`new_lease_getter`、`lease` 和 `set_lease`。两版都提供共享安全的即时读写，不包含过期调度逻辑。

实现形态存在三点差异。第一，Go 具体类型私有并持有 `*atomic.Duration`，Rust 具体类型公开且内嵌 `AtomicU64`。第二，Go `SetLease` 注释说明主要供测试使用，Rust trait 没有限制该方法用途，且 `pkg/domain/domain.rs::set_stats_lease` 表明运行时配置也需要类似能力。第三，Go `time.Duration` 是有符号时长，而 Rust `Duration` 非负；Rust 还明确把超过 `u64::MAX` 纳秒的值饱和到上界。

接线进度也不同：Go `pkg/statistics/handle/handle.go` 在构造 `Handle` 时执行 `handle.LeaseGetter = util.NewLeaseGetter(lease)`，所以接口和原子实现直接进入主句柄；Rust `Handle<B>` 当前使用自身字段和固有方法，尚未构造 `AtomicLeaseGetter`。因此本文件是 Go 抽象的实现移植，但尚不是 Rust 主句柄租约状态的真实存储位置。

## 扩展指南

若只需改变存储转换或内存序，应集中修改 `duration_nanos` 或 `impl LeaseGetter for AtomicLeaseGetter`，并新增独立测试文件（建议同目录 `lease_getter_test.rs`，再由 `lib.rs` 的 `#[cfg(test)]` 路径模块接入），不要把测试内嵌进生产源文件。至少覆盖零值、普通读写、超 `u64::MAX` 纳秒饱和、多线程共享更新，以及 trait 对象工厂返回值。

若要把原子实现接入 Rust 主统计句柄，应先决定单一事实源：用 `Arc<dyn LeaseGetter>` 替换 `pkg/statistics/handle/handle.rs::Handle<B>::lease`，或保留现有字段而不启用本实现；不能同时维护两份未同步租约。需要同步检查 `pkg/statistics/handle/types/interfaces.rs` 的 `StatsHandle` 组合约束、`pkg/statistics/handle/cache/statscache.rs` 的显式 trait 调用、`pkg/domain/domain.rs::set_stats_lease`，以及 `pkg/statistics/handle/analyze_runtime_aster_unit_test.rs` 和 `pkg/planner/core/casetest/planstats/plan_stats_test.rs` 的租约往返测试。

扩展返回旧值、条件更新或变更通知会改变 trait 契约和并发语义，应新增方法而非暗改现有 `set_lease` 行为，并评估所有 trait 实现者。改变纳秒存储上限或饱和策略则有兼容风险；引入锁或通知机制会增加热路径争用和生命周期管理成本。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录清单确认 `lease_getter.rs` 被索引且含 10 个符号。
- RustCodeGraph `query`：精确定位 `LeaseGetter`（第 25 行）、`AtomicLeaseGetter`（第 31 行）、`duration_nanos`（第 55 行）和 `new_lease_getter`（第 60 行）；`node` 核对了 trait 的 `Send + Sync` 及两个方法签名。
- RustCodeGraph `explore`：确认 `new_lease_getter -> AtomicLeaseGetter::new -> duration_nanos` 的文件内调用路径；精确 callers 查询未返回文件外调用边，随后用全仓文本引用搜索复核未接线事实。图结果中的同名 Go/Rust 符号存在歧义，因此未把宽泛的同名 blast-radius 列表当作本文件的调用证据。
- 源码与模块证据：`pkg/statistics/handle/util/lease_getter.rs`、`pkg/statistics/handle/util/lib.rs`、`pkg/statistics/handle/util/Cargo.toml`、`pkg/statistics/handle/types/interfaces.rs`、`pkg/statistics/handle/handle.rs`、`pkg/statistics/handle/cache/statscache.rs`、`pkg/domain/domain.rs`。
- Go 对照证据：`pkg/statistics/handle/util/lease_getter.go` 与 `pkg/statistics/handle/handle.go`。
- 测试证据：未发现本原子类型或工厂的同名独立 Rust 单元测试；`pkg/statistics/handle/analyze_runtime_aster_unit_test.rs::handle_exposes_lease_and_forced_delta_flush` 验证当前 `Handle` 的零值和毫秒级往返，`pkg/planner/core/casetest/planstats/plan_stats_test.rs::test_stats_lease_round_trip_matches_go_defer_pattern` 验证 Domain 临时更新后恢复原租约。Go 侧 `pkg/statistics/integration_test.go` 和 `pkg/statistics/handle/syncload/stats_syncload_test.go` 展示了读取、临时设置和 defer 恢复的真实用法。
- 本任务是只读行为分析加文档新增，按计划不运行 Cargo；最终以固定十一章节结构命令验证文档形状，并人工复核上述现状/边界结论均能回指到源码或调用证据。
