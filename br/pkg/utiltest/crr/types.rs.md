# `br/pkg/utiltest/crr/types.rs`

## 文件定位

[`types.rs`](./types.rs) 是 `astersql-br-pkg-utiltest-crr` 测试夹具 crate 的共享数据契约层。crate 由 [`Cargo.toml`](./Cargo.toml) 定义为 library，入口 [`lib.rs`](./lib.rs) 以 `pub mod types` 装入本文件，并通过 `pub use types::*` 将其公开符号扁平再导出。它本身不执行 CRR（跨区域复制）仿真，而是为 [`builder.rs`](./builder.rs)、[`pd_sim.rs`](./pd_sim.rs)、[`flush_sim.rs`](./flush_sim.rs) 和 [`harness.rs`](./harness.rs) 提供常量、确定性随机源、测试上下文以及 Region/flush 快照类型。

对应的 Go 实现是 [`types.go`](./types.go)，两者都属于 `br/pkg/utiltest/crr` 夹具，而不是生产 TiKV/PD 数据模型。Cargo 的 `package.metadata.porting` 也将 Go 包路径记为 `br/pkg/utiltest/crr`、crate 类型记为 `library`。

## 核心职责

本文件承担四类职责：

1. 用 `DEFAULT_TASK_NAME`、`DEFAULT_TASK_START_PHYSICAL`、`REGION_ID_TAG` 及其包内 Go 风格别名统一任务名、物理时间基线和元数据文件名标签。
2. 用 `deriveDeterministicSeed` 和 `DeterministicRNG` 从根 seed 与组件名派生相互隔离的伪随机流，供 PD scatter、任务起始时间抖动和每次 store flush 使用。
3. 用 `TestContext` 保存根 seed，并记录一条可供失败回放的 `SEED: ...` 日志。
4. 用 `RegionBoundary`、`RegionState`、`FlushRecord` 定义构建输入、运行时只读快照和 flush 产物快照；`clone_record` 显式深拷贝其中的可变容器。

这些类型把仿真组件之间的参数和结果形状集中在一处，但不负责验证完整 Region 布局、分配 TSO、写文件、推进 checkpoint 或清理临时目录；这些行为分别位于 `builder.rs`、`pd_sim.rs`、`flush_sim.rs` 和 `harness.rs`。

## 主要符号

- `DEFAULT_TASK_NAME: &str = "drr_test_task"`：公开默认任务名。包内别名 `defaultTaskName` 被 `pd_sim.rs` 和 `harness.rs` 使用。
- `DEFAULT_TASK_START_PHYSICAL: i64 = 1_700_000_000_000`：公开物理时间基线；`defaultTaskStartPhysical` 在 `NewPDSimWithTestContext` 中与随机抖动相加后交给 `oracle::ComposeTS`。
- `REGION_ID_TAG: u8 = b'r'`：公开元数据名称标签；`regionIDTag` 由 `flush_sim.rs::formatTaggedMetaName` 写入 meta 文件名。
- `DeterministicRNG { rng: StdRng }`：封装 `rand 0.8` 的 `StdRng`。内部状态私有，调用方只能经 `IntN`、`Int63n` 和 `Uint64InRange` 前进随机流。
- `newDeterministicRNG(seed, component)`：以 `deriveDeterministicSeed` 的结果初始化 `StdRng`。
- `deriveDeterministicSeed(seed, component) -> i64`：计算 `seed` 与组件名 FNV-1a 64 位哈希的异或值，清除最高位，并把零修正为一。
- `fnv1a64(data) -> u64`：私有 FNV-1a 实现，使用标准 offset basis、prime 和 wrapping multiplication。
- `DeterministicRNG::IntN(n)` / `Int63n(n)`：分别从半开区间 `[0,n)` 取 `usize` / `i64`。
- `DeterministicRNG::Uint64InRange(lower, upper)`：在 `lower < upper` 时从闭区间 `[lower,upper]` 取值，否则原样返回 `lower`。
- `TestContext { seed, logs }`：`seed` 私有，只能用 `Seed()` 读取；`logs` 公开。`NewTestContext` 用当前 Unix 秒构造，`NewTestContextWithSeed` 用显式 seed 构造并写入回放日志，`RNG(component)` 派生组件随机源。
- `RegionBoundary`：静态布局输入，包含左闭右开区间的 `StartKey`、`EndKey` 和 leader `StoreID`。
- `RegionState`：运行时 Region 快照，除键范围和 store 外还含 `ID`、`Epoch`、`Checkpoint`。
- `FlushRecord`：一次 flush 的序号、store、Region 列表、checkpoint/flush/min/max 时间戳及 meta/log 路径。`clone_record()` 对 `RegionIDs`、`MetadataPath`、`LogPaths` 做独立拷贝。

三个数据结构都派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`；默认值只是全零/空容器，并不自行满足完整布局或有效 flush 的业务约束。

## 执行流程

典型主链如下：

1. 测试调用 `NewTestContextWithSeed` 固定根 seed，或者调用 `NewTestContext` 使用当前 Unix 秒；构造函数把 seed 写入 `logs`。
2. `builder.rs::BuildRegionLayout` 及其 option 构造 `Vec<RegionBoundary>`，并负责非空、连续、区间有序、有效 store 和末尾闭合等验证。
3. `pd_sim.rs::NewPDSimWithTestContext` 接收边界和 `TestContext`。它用 `tc.RNG("pd-sim")` 派生随机流，通过 `Int63n(1 << 20)` 生成任务起始时间抖动，再将每个 `RegionBoundary` 物化到 fakecluster。
4. `PDSim::RegionSnapshot` / `RegionSnapshotsOnStore` 把 fakecluster 状态转换为本文件的 `RegionState`；`PDSim::Scatter` 用保存在 `PDSimState` 中的 RNG 和 `IntN` 选择目标 store。
5. `flush_sim.rs::NewFlushSimWithTestContext` 保存 `tc.Seed()`。每次 `FlushStore` 以 `seed + storeID + flushSeq` 构造组件名并调用 `newDeterministicRNG`，再用 `Uint64InRange` 为各 Region 选 min/max TS。
6. `FlushStore` 汇总为 `FlushRecord`，先把 `clone_record()` 放入受锁保护的历史列表，再向调用方返回另一份深拷贝；`Records` 和 `RecordsUpTo` 同样只返回深拷贝快照。
7. `harness.rs::NewLocalTestHarnessWithTestContext` 用同一 `TestContext` 接线 PDSim、FlushSim、CRRWorker 和 checkpoint advancer，使整个测试拓扑共享根 seed、但各组件通过 component 名隔离随机流。

## 数据与状态

`DeterministicRNG` 的唯一可变状态是 `StdRng` 内部游标；每次取样都会推进它。因此复现不仅依赖相同的根 seed 和 component，还依赖相同的调用顺序与边界参数。不同 component 先经 FNV-1a 派生不同 seed，避免 `pd-sim` 和 `flush-sim-store-<id>-flush-<seq>` 互相消费同一随机序列。

`TestContext` 把根 seed 与诊断日志绑定。`NewTestContextWithSeed` 保留传入值，包括零值和负值；真正派生时先按二进制位转换为 `u64` 参与异或，然后保证派生结果落在正 `i64` 范围且不为零。`NewTestContext` 只取 Unix 秒，粒度为一秒；系统时间早于 Unix epoch 时回退到 `1`。

`RegionBoundary` 和 `RegionState` 的键使用 `Vec<u8>`，因此边界是原始字节而不是 UTF-8 字符串。空 `StartKey` / `EndKey` 分别在相邻构建与 PD 校验代码中表示全键空间的负无穷/正无穷端。类型本身不强制这些不变量。

`FlushRecord` 是值快照。显式克隆确保历史列表与外部调用方不会共享 `RegionIDs`、路径字符串或 `LogPaths` 的可变缓冲；数值字段按值复制。`Sequence` 是 flush 的创建序号，而 `CheckpointTS` 决定 `RecordsUpTo` 的筛选范围，两者不可互换。

## 依赖与调用关系

直接外部依赖只有 Cargo 中的 `rand = "0.8"`，本文件具体使用 `StdRng`、`Rng::gen_range` 和 `SeedableRng::seed_from_u64`；时间种子来自标准库 `SystemTime` / `UNIX_EPOCH`。`serde` 及其他 BR crate 虽属于同一 Cargo target 的依赖，但本文件没有直接引用。

主要上游调用关系经 RustCodeGraph 与目录限定引用搜索核对如下：

- `lib.rs` 声明并再导出整个 `types` 模块。
- `builder.rs` 构造和深拷贝 `RegionBoundary`；`builder_test.rs` 验证其键范围、store 轮转和二进制边界。
- `pd_sim.rs` 使用 `TestContext::RNG`、`DeterministicRNG::{Int63n,IntN}`、默认任务/时间常量、`RegionBoundary` 和 `RegionState`。
- `flush_sim.rs` 使用根 seed、`newDeterministicRNG`、`Uint64InRange`、`REGION_ID_TAG` 的包内别名、`RegionState`、`FlushRecord::clone_record`。
- `harness.rs` 使用 `TestContext::Seed`、`RegionBoundary` 和默认任务名来创建临时目录并组合仿真器。
- `parity_test.rs` 直接验证 `deriveDeterministicSeed`、`NewTestContextWithSeed`、公开默认任务名和 Region/flush 主链；`harness_test.rs`、`pd_sim_test.rs`、`pd_sim_service_test.rs` 通过公开再导出构造上下文。

下游算法都在本文件内：`newDeterministicRNG -> deriveDeterministicSeed -> fnv1a64`，`TestContext::RNG -> newDeterministicRNG`，`Uint64InRange -> Int63n`。本文件不做 I/O，也不调用 stream、storage 或 fakecluster API。

## 错误处理与边界

本文件没有 `Result` 返回值，错误边界主要表现为回退、提前返回或 panic 前置条件：

- `deriveDeterministicSeed` 使用 wrapping FNV 乘法，清最高位后若为零则返回一，因此不会产出非正派生 seed。
- `NewTestContext` 在系统时间无法表示为 Unix epoch 之后的 duration 时回退 seed `1`；它不会把时间错误暴露给调用方。
- `IntN` 要求 `n > 0`，`Int63n` 要求 `n > 0`；违反时 `rand::gen_range(0..n)` 会 panic。当前调用点分别以非空 store 列表长度和常量 `1 << 20` 调用，约束由上游校验保证。
- `Uint64InRange` 对 `lower >= upper` 返回 `lower`，因此单点和逆序边界不会进入随机库。对于 `lower < upper`，调用方还必须保证 `upper - lower + 1` 能安全表示为正 `i64`；极大的 `u64` 跨度可能在加一、转换或 `gen_range` 时溢出/panic。当前 flush 调用使用同一 TSO 时间域内的 checkpoint/latest TS，现有测试没有覆盖极端 `u64` 跨度。
- `RegionBoundary`、`RegionState`、`FlushRecord::default()` 不验证非零 ID、边界连续性、时间戳顺序或路径非空。布局不变量由 `builder.rs` / `pd_sim.rs::validateBoundaries` 负责，flush 产物有效性由 `flush_sim.rs` 负责。
- `clone_record` 不会失败；其代价与 Region ID 数量、路径字符串长度和日志路径数量线性相关。

## 并发与资源生命周期

本文件不创建线程、锁、通道、事务或外部资源。`DeterministicRNG` 的取样方法需要 `&mut self`，在类型层面要求单一可变访问；它没有显式同步封装。实际使用中，`PDSimState` 把 RNG 放在 `Mutex` 后，`FlushSim` 则为每次 flush 创建局部 RNG。

`TestContext::RNG` 只读取 seed，所以每次调用都得到一个从相同派生 seed 起步的新随机源；它不会共享前一个 RNG 的游标。`logs: Vec<String>` 也没有内部同步，构造后当前代码只读取它；若未来并发追加日志，应由调用方加锁或改变类型。

`RegionBoundary`、`RegionState` 和 `FlushRecord` 自身只拥有内存，不持有借用或句柄。`clone_record` 建立独立所有权，服务于 `FlushSim` 锁内保存、锁外返回的生命周期边界。真正的锁、取消桥接线程、文件存储和临时目录清理由 `pd_sim.rs`、`flush_sim.rs` 与 `harness.rs` 管理。

## 与 Go 版本的对应关系

Rust 的常量值、FNV-1a 派生步骤、`lower >= upper` 回退、三个数据结构的字段形状和 flush 深拷贝意图与 [`types.go`](./types.go) 对应。Rust 保留 Go 风格函数/字段名，并额外提供大写公开常量及小写 crate 内别名，以同时支持 crate 外测试和机械对照。

存在以下明确差异：

- Go `TestContext` 持有 `testing.TB`，构造时调用 `Helper()` / `Log()`；Rust 不保存测试框架句柄，而是把 seed 文本写到公开 `Vec<String>`。因此 Rust 记录不会自动出现在测试 runner 日志中。
- Go 构造函数返回指针，Rust 返回拥有所有权的值，调用方按需以共享引用传递。
- Go 使用 `math/rand.Rand`，Rust 使用 `rand::rngs::StdRng`。两边的 FNV-1a 派生 seed 规则对齐，但这不足以证明后续随机数序列跨语言逐项一致。现有 `parity_test.rs` 只断言相同输入的派生结果稳定且为正，没有 Go golden 序列；因此“随机序列跨语言一致”属于未验证结论，不能作为当前兼容保证。
- Go `FlushRecord.clone` 先做浅拷贝再复制两个 slice；Rust `clone_record` 逐字段构造并克隆 `Vec` / `String`，达到相同的独立所有权效果。
- Rust 数据结构额外派生 `Default` 和相等比较，便于模拟器的缺失快照返回与测试断言；Go 依赖结构体零值和字段比较。

## 扩展指南

- 新增共享字段时，先判断它属于布局输入、运行时快照还是 flush 产物，并同步对应的 Go 结构、Rust 构造点/转换点及深拷贝逻辑。给 `FlushRecord` 增加容器字段时必须同时更新 `clone_record`，避免历史记录与调用方共享可变状态。
- 新增随机组件时，通过 `TestContext::RNG` 或 `newDeterministicRNG` 使用稳定且唯一的 component 名；不要复用另一个组件的名字，也不要依赖无关代码的随机调用次数。
- 修改 seed 派生算法、组件名或随机调用顺序会改变已有测试场景。至少在独立的 `parity_test.rs` 中增加固定输入的派生 golden、组件隔离和边界测试；若目标是跨 Go/Rust 随机流一致，还需加入两端相同序列的 golden 证据，不能只比较 seed。
- 修改 `Uint64InRange` 时应增加单点、逆序、普通闭区间以及接近 `u64::MAX` 的独立测试，并明确是保持 Go 的转换/溢出语义还是提供受检错误。不要把测试内嵌回 `types.rs`；本仓库要求 Rust 测试保存在独立测试文件。
- 修改 `RegionBoundary` 要同步 `builder.rs`、`pd_sim.rs::validateBoundaries` / `toRegionState` 和 `builder_test.rs`；修改 `RegionState` 要同步 fakecluster 转换与 `flush_sim.rs`；修改 `FlushRecord` 要同步 `FlushStore`、`Records`、`RecordsUpTo` 和 `parity_test.rs` 的产物断言。
- `DEFAULT_TASK_NAME`、物理时间基线和 `REGION_ID_TAG` 进入任务校验、TSO 和持久化文件名。变更它们可能破坏 Go 兼容、测试可复现性或备份元数据命名解析，应同时审查对应 Go 文件及 stream backup meta 的标签约定。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utiltest/crr` 找到目标及相邻 21 个 Go/Rust 文件。
- RustCodeGraph `node --file br/pkg/utiltest/crr/types.rs`：读取完整 183 行源码，并报告目标被 `builder.rs`、`flush_sim.rs`、`harness.rs` 及测试等 10 个文件使用。
- RustCodeGraph 精确查询：`NewTestContextWithSeed`、`TestContext`、`DeterministicRNG`；图结果确认 `deriveDeterministicSeed -> fnv1a64`、`Uint64InRange -> Int63n`、`NewTestContext -> NewTestContextWithSeed` 等内部边，并列出 `parity_test.rs`、`harness_test.rs`、`pd_sim_test.rs`、`pd_sim_service_test.rs` 的调用。
- RustCodeGraph 按行读取：`pd_sim.rs` 的构造、快照和 scatter；`flush_sim.rs` 的随机范围、记录写入与克隆；`harness.rs` 的组件接线；`builder.rs` 的边界构造和验证。
- 配置与入口：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)；该目录不存在 `doc.go`。
- Go 对照：[`types.go`](./types.go)，以及使用这些类型的 `builder.go`、`pd_sim.go`、`flush_sim.go`、`harness.go` 引用。
- 独立 Rust 测试：[`parity_test.rs`](./parity_test.rs)、[`builder_test.rs`](./builder_test.rs)、[`harness_test.rs`](./harness_test.rs)、[`pd_sim_test.rs`](./pd_sim_test.rs)、[`pd_sim_service_test.rs`](./pd_sim_service_test.rs)。未发现 `types_test.rs` 或与 `types.rs` 同名的独立测试；最直接的 seed 与记录断言位于 `parity_test.rs`。

本任务为纯文档分析，按计划不运行 Cargo。结构验证要求文档存在且恰好包含本文的 11 个固定二级章节；行为事实通过上述源码、调用图、Cargo、Go 对照与独立测试交叉核对。
