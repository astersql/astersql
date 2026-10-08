# `pkg/util/globalconn/globalconn.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-util-globalconn`，由同目录的 [`lib.rs`](lib.rs) 作为 `globalconn` 模块装入并公开再导出。它实现 Global Connection ID（GCID）的位级编解码，以及关闭或开启 GlobalKill 时使用的两类连接号分配器；实际 ID 池算法位于同 crate 的 [`pool.rs`](pool.rs)。[`Cargo.toml`](Cargo.toml) 仅直接依赖 `log`，并用 `package.metadata.porting.go-package = "pkg/util/globalconn"` 标明 Go 对照包。

当前 Rust 接线状态需要与 Go 生产链区分：根 workspace 和 `pkg/lib.rs` 已纳入并再导出此 crate，`pkg/domain/Cargo.toml`、`pkg/executor/Cargo.toml` 也声明了依赖，但全仓 Rust 符号搜索只发现本 crate 测试使用这些 API，未发现 domain/executor 的生产 Rust 调用。完整应用中已验证的生产接线仍在 Go：`pkg/domain/domain.go` 根据配置创建 `NewGlobalAllocator` 或 `NewSimpleAllocator`，`pkg/executor/simple.go` 用 `ParseConnID` 支持跨实例 kill。因此，本文件是已经实现且可测试的 Rust 能力模块，但不能仅凭 Cargo 依赖断言它已进入 Rust 请求主链。

## 核心职责

1. `GCID::ToConnID` 与 `ParseConnID` 在逻辑三元组 `(ServerID, LocalConnID, Is64bits)` 和对外 `u64` 连接号之间转换。最低位是版本标记：32 位布局为 `server:11 + local:20 + 0`，64 位布局为保留最高符号位、`server:22 + local:40 + 1`。
2. `Allocator` 抽象统一提供 `NextID`、`Release` 和 `GetReservedConnID`。`SimpleAllocator` 服务于未开启 GlobalKill 的本地自增模式；`GlobalAllocator` 把实例 ServerID 编入结果，优先使用 32 位本地号池，必要时切换到 64 位池。
3. `ReservedCount = 200` 从普通连接号空间尾部保留内部连接号。简单模式从 `u64::MAX` 向下取；全局模式固定使用 64 位布局，并从 40 位本地号最大值向下取。
4. `initByLDFlagsForGlobalKill` 保留 Go 链接期测试参数语义，可在测试布局启用时改写 32 位字段宽度及上限；Rust 的 `init` 只是普通公开函数，不会像 Go `init()` 一样自动执行。

## 主要符号

- `GCID { ServerID, LocalConnID, Is64bits }`：连接号的逻辑表示。`ToConnID(&self) -> u64` 在编码前验证字段上限，越界即 panic。
- `ParseConnID(id) -> Result<(GCID, bool), String>`：解析编码；元组中的 `bool` 表示旧客户端把带 64 位标记的 ID 截断到低 32 位，而不是一般解析成功标志。
- 位宽和上限：可变的 `ServerIDBits32`、`MaxServerID32`、`LocalConnIDBits32`、`MaxLocalConnID32`，以及固定的 `MaxServerID64`、`LocalConnIDBits64`、`MaxLocalConnID64`。最高位必须为零，以便结果不超过有符号 64 位范围。
- `Allocator` trait：不带 `Send`/`Sync` 约束的接口，具体实现通过内部池与原子状态提供并发能力。
- `NewSimpleAllocator() -> SimpleAllocator`：用 `AutoIncPool::Init(u64::MAX - ReservedCount)` 初始化普通号池。该池未开启“已占用集合”，所以 `Release` 不会让下一次 `Get` 立即复用同一编号。
- `GlobalAllocator`：包含 `AtomicI32 is64bits`、`Box<dyn Fn() -> u64 + Send + Sync>`、`LockFreeCircularPool local32` 和 `AutoIncPool local64`。
- `GlobalAllocator::NewGlobalAllocator(getter, enable32Bits)`：32 位池以 `2^LocalConnIDBits32` 个槽初始化，实际可用容量因环形队列留空槽而少 1；64 位池容量为 `2^40 - ReservedCount`，开启去重并最多尝试 `LocalConnIDAllocator64TryCount = 10` 次。
- `Allocate`、`is64`、`upgradeTo64`、`downgradeTo32`：实现模式选择和切换；`Allocator for GlobalAllocator` 再把逻辑 GCID 编码、释放或生成保留号。
- `ldflagIsGlobalKillTest`、`ldflagServerIDBits32`、`ldflagLocalConnIDBits32` 与 `initByLDFlagsForGlobalKill`：测试位布局的全局可变开关和解析入口。

## 执行流程

编码时，`GCID::ToConnID` 先由 `Is64bits` 选择布局。64 位路径校验 40 位本地号和 22 位 ServerID，上置最低标记位，再分别左移 1 和 41 位；32 位路径依据当前可变上限校验，最低位保持零，并分别左移 1 和 21 位。

解析时，`ParseConnID` 先拒绝最高位为 1 的值。若最低位为 1，则把仅有低 32 位的值识别为旧客户端截断，返回默认 `GCID` 与 `isTruncated=true`；否则按 40/22 位掩码解析完整 64 位 GCID。若最低位为零，则要求高 32 位全零，再按 20/11 位掩码解析 32 位 GCID。

普通分配路径是 `SimpleAllocator::NextID -> AutoIncPool::Get`；返回值中的池状态被忽略。`Release` 调用 `Put`，保留号路径先验证 `reservedNo < 200`，再返回 `u64::MAX - reservedNo`。

全局分配路径是 `GlobalAllocator::NextID -> Allocate -> GCID::ToConnID`。`Allocate` 每次先调用 `serverIDGetter`：只有当前未处于 64 位模式且 ServerID 能装入 11 位时才从 `local32` 取号；池空则调用 `upgradeTo64` 并继续执行 64 位路径。ServerID 超过 32 位布局上限也直接走 64 位。64 位池若在 10 次冲突尝试后仍取不到编号，被视为不可能状态并 panic。

释放路径先调用 `ParseConnID`。解析错误或截断值只记录错误并返回；完整 64 位 GCID 把本地号归还 `local64`，完整 32 位 GCID归还 `local32`。32 位归还成功且 `local32.Len() < local32.Cap()/2` 时调用 `downgradeTo32`，使后续分配重新尝试 32 位池。

## 数据与状态

`GCID` 本身是可复制的无资源值。编码唯一性来自 ServerID、局部连接号和布局标记的组合；32 位和 64 位 GCID 即使局部号相同，最低标记位和整体布局仍使最终 ID 不同。

`GlobalAllocator` 的长期状态包括当前模式、一个可动态读取 ServerID 的闭包以及两个本地号池。测试 `TestGlobalAllocatorAcceptsCapturingServerIDGetter` 证明 getter 可捕获共享状态，且每次 `Allocate` 都重新读取，因此 ServerID 并非构造时快照。`local32` 初始化为“装满可用编号”的无锁环形池；`local64` 用原子自增与受 `Mutex<HashSet<_>>` 保护的占用集合避免回绕冲突。

32 位布局参数和 ldflag 字符串使用 `static mut`，读写均依赖 `unsafe`。它们是进程级共享状态，不具备同步保护；正常假设是初始化阶段设置，业务并发开始后只读。若并发修改，Rust 代码无法提供数据竞争安全保证。

## 依赖与调用关系

RustCodeGraph 对本文件确认的内部边包括：`GlobalAllocator::NextID` 调用 `Allocate` 和 `GCID::ToConnID`；`Allocate` 调用 `is64`、`upgradeTo64` 及池的 `Get`；`GlobalAllocator::Release` 调用 `ParseConnID` 和条件性的 `downgradeTo32`；构造器引用 `LocalConnIDBits64`、`ReservedCount`、`LocalConnIDAllocator64TryCount` 并实例化 `GlobalAllocator`。

下游依赖来自 crate 根再导出的 [`AutoIncPool`](pool.rs) 和 [`LockFreeCircularPool`](pool.rs)，以及 `std::sync::atomic::{AtomicI32, Ordering}` 和 `log`。所有模式状态读写均使用 `SeqCst`。

上游方面，[`lib.rs`](lib.rs) 公开再导出本模块，根 `pkg/lib.rs` 又通过 `facade_util_globalconn` 暴露它。RustCodeGraph 的 callers 对主要入口没有给出跨文件生产调用；精确 `rg` 补查也只找到 [`globalconn_test.rs`](globalconn_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的直接使用。因此当前可验证的 Rust 上游是测试与门面，生产语义位置须以 Go 的 `pkg/domain/domain.go`、`pkg/executor/simple.go` 为对照，而不能宣称 Rust 主链已经调用。

## 错误处理与边界

- `ToConnID` 对字段超过布局上限执行 panic；它不做截断或饱和。`GetReservedConnID` 对 `reservedNo >= ReservedCount` 同样 panic。
- `ParseConnID` 对最高位为 1 返回 `Err("unexpected connectionID exceeds int64")`；32 位标记下若高 32 位非零，返回 `Err("unexpected connectionID exceeds uint32")`。最低位为 1 且高 32 位全零不是错误，而是 `isTruncated=true`。
- `GlobalAllocator::Release` 吞掉解析失败/截断输入并记录日志，不向调用方返回错误；32 位池拒绝归还时也只记录日志。调用方无法通过返回值确认释放成功。
- 完整 64 位分配器依赖 ServerID 不超过 `MaxServerID64`。`Allocate` 本身不校验这一点，越界最终在 `NextID` 或调用者显式执行 `ToConnID` 时 panic。
- `SimpleAllocator::NextID` 忽略 `AutoIncPool::Get` 的布尔值，但其初始化参数让普通模式采用一次自增、无占用去重；这是与 Go 一致的既有约定，不应在局部扩展中擅自改成返回 `Result`。
- ldflag 数字解析失败会 panic；位移宽度也依赖输入合理。测试参数应在调用初始化入口前设置，且不能在并发使用期修改。

## 并发与资源生命周期

`GlobalAllocator` 设计为长生命周期共享对象：模式使用 `AtomicI32`，32 位池使用原子 head/tail/slot 序号，64 位池的递增计数为原子值、占用集合由互斥锁保护。ServerID getter 被要求 `Send + Sync + 'static`，所以它可以捕获 `Arc<AtomicU64>` 等共享状态。对象销毁时闭包、池槽和占用集合随所有权自动释放，没有后台线程或显式 close。

模式切换不是一个事务屏障：多个线程可以同时观察或写入 `is64bits`，但 `SeqCst` 保证单一全序；真正的 ID 唯一性仍由两个池各自的原子/互斥机制和编码布局保证。32 位释放后的降级条件检查的是归还后的当前占用长度；并发变化可能让模式在边界附近往返，但不会把 32/64 位编码混淆。

Rust 测试中的 `lock_free_pool_preserves_values_under_concurrent_producers_and_consumers` 为底层无锁池提供多生产者/消费者完整性证据；`globalconn_test.rs` 中的 benchmark 目前只是普通未调用函数骨架，并没有接入 Rust benchmark harness，因此不能作为实际性能或并发压力验证结果。

## 与 Go 版本的对应关系

Rust 文件逐项保留了 [`globalconn.go`](globalconn.go) 的 GCID 字段、位布局、常量、`Allocator` 三方法、两类分配器、升级/降级阈值、保留号算法和 ldflag 解析。`AtomicI32` 对应 Go `sync2.AtomicInt32`，闭包类型对应 `func() uint64`，`Result<(GCID, bool), String>` 承载 Go 的 `(GCID, isTruncated, error)`。

主要语言差异有三点：Rust 构造器按值返回分配器而非 Go 指针；Rust 的 `init()` 不会自动运行；Go 包级变量可由链接参数设置，而 Rust 当前以公开 `static mut &str` 和显式初始化函数模拟，带来额外 `unsafe` 约束。另有测试覆盖差异：Go `BenchmarkLocalConnIDAllocator` 真正用 `RunParallel` 执行三类池的多并发 benchmark，Rust 同名函数只保留结构，没有 `#[bench]` 或 criterion 接线。

Go 的生产链已经在 `pkg/domain/domain.go` 构造分配器，在 `pkg/executor/simple.go` 解析 GCID 并决定本地或远程 kill；对应 Rust 生产代码尚未找到 API 调用。迁移后续若接线，必须保持这些选择条件、截断兼容和错误行为，而不只是让 crate 编译存在。

## 扩展指南

修改位布局时，应同时更新 `GCID::ToConnID`、`ParseConnID`、上限常量、`GlobalAllocator::NewGlobalAllocator` 的池容量及保留号计算，并同步独立测试 [`globalconn_test.rs`](globalconn_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；还要对照 Go 的 [`globalconn.go`](globalconn.go) 与 [`globalconn_test.go`](globalconn_test.go)，避免破坏跨版本协议。位布局是外部可见兼容契约，不能只改单侧掩码或位移。

新增分配策略时，优先通过 `Allocator` trait 接入，并保持测试逻辑位于独立测试文件，不放回生产 `.rs`。若改变池复用或并发行为，还需同步检查 [`pool.rs`](pool.rs) 及其独立测试；特别关注重复 ID、32/64 位模式切换抖动、锁竞争和保留区碰撞。

把该能力接入 Rust domain/executor 时，至少需要复刻 Go 的启停选择、动态 ServerID getter、连接关闭时 `Release`、kill 语句的截断检测及远程路由，并新增调用链级独立测试。不要把现有 Cargo 依赖当作接线完成证据。若要让 ldflag 初始化自动发生，应先确定 Rust 进程初始化入口，再显式调用 `initByLDFlagsForGlobalKill`；不要依赖函数名 `init` 产生 Go 式副作用。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标 [`globalconn.rs`](globalconn.rs) 含 39 个符号。`node --file` 用于核对完整实现，`query` 定位 `ParseConnID`、`GlobalAllocator`、`Allocate`、`NextID`、`Release`、构造器和初始化入口，`callers/callees` 核对主要内部调用边。
- 已读生产与装配文件：[`globalconn.rs`](globalconn.rs)、[`pool.rs`](pool.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、Go 对照 [`globalconn.go`](globalconn.go)。另以 workspace `Cargo.toml`、`pkg/lib.rs`、`pkg/domain/Cargo.toml`、`pkg/executor/Cargo.toml` 和精确全仓搜索核对 crate 边界与当前 Rust 接线状态。
- 已读测试：Rust [`globalconn_test.rs`](globalconn_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，Go [`globalconn_test.go`](globalconn_test.go)。这些测试覆盖编码越界 panic、解析溢出与截断、32/64 位往返、保留号、捕获式 ServerID getter、32 位分配/释放以及底层池并发完整性；Rust benchmark 仅为未接线骨架。
- Go 生产调用证据：`pkg/domain/domain.go` 创建两类分配器，`pkg/executor/simple.go` 调用 `ParseConnID` 并使用 `GCID` 处理 kill。全仓 Rust 精确搜索未发现对应生产调用，这是“Rust API 已实现但生产主链未验证接线”的依据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级章节，并人工复核所有“已支持/已接线”陈述均有上述文件或查询证据。
