# `pkg/util/set/set_with_memory_usage.rs`

## 文件定位

本文件属于 `astersql-util-set` crate；crate 入口 `pkg/util/set/lib.rs` 以 `set_with_memory_usage` 模块加载并公开重导出其 API。`pkg/util/set/Cargo.toml` 表明它直接依赖 `astersql-util-hack`、`astersql-util-memory` 和 `astersql-types`，分别提供内存感知映射、内存追踪器和 `MyDecimal`。

它是 Go `pkg/util/set/set_with_memory_usage.go` 的 Rust 对应实现：在集合/映射操作之上报告近似内存增量。当前 Rust 仓库中的实际引用集中在 `pkg/util/set/set_with_memory_usage_test.rs` 与 `pkg/util/set/migration_aster_unit_test.rs`；`pkg/executor/aggregate/agg_hash_executor.rs` 仅保留了注释中的拟接线代码。因此，该文件已提供可用的 crate API，但尚不能据现有调用证据声称已进入 Rust SQL 执行主链。

## 核心职责

- 用 `hack::MemAwareMap<K, V>` 实现五种容器：字符串映射、字符串到 Decimal 指针的映射，以及字符串、`f64`、`i64` 集合（`set_with_memory_usage.rs:34-359`）。
- 构造时返回当前 `Bytes`，让调用方把初始占用纳入上层记账；后续插入返回底层 `MemAwareMap::Set` 产生的增量（各 `New*` 和 `Insert`）。
- 为两个字符串映射和字符串集合提供可选的 `Arc<memory::Tracker>`：绑定后，非零增量由容器立即调用 `Tracker::Consume`，并向调用方返回 `0`，从而避免重复记账（`set_with_memory_usage.rs:64-80,140-154,215-229`）。
- 通过 `FloatKey` 复现 Go `map[float64]` 的关键语义：`+0.0` 与 `-0.0` 视为同一键；每次插入 NaN 都形成新成员，而 NaN 查询恒为不存在（`set_with_memory_usage.rs:254-306`；`float64_set.rs:31-59`）。

这里的“内存用量”是 `MemAwareMap` 的 checkpoint 近似值，不是 Rust 分配器逐次返回的精确实时占用；底层只在元素数达到 `nextCheckpoint` 时更新 `Bytes`（`pkg/util/hack/map_abi.rs:434-447`）。

## 主要符号

- `new_mem_aware_map<K, V>(capacity)`：私有构造辅助，解包 `hack::NewMemAwareMap` 返回的 `Box`，统一五类容器的初始化路径（`set_with_memory_usage.rs:28-30`）。
- `StringToStringMapWithMemoryUsage`：公开 `mem_aware_map`，私有可选 `tracker`；提供 `Insert`、`SetTracker`、`Exist`、`Count`、`Len`、`Empty`、`Bytes`（`set_with_memory_usage.rs:34-106`）。
- `StringToDecimalMapWithMemoryUsage`：形状与字符串映射相同，但值为 `*mut types::MyDecimal`；容器只保存地址，不拥有或释放指针指向的对象（`set_with_memory_usage.rs:110-180`）。
- `StringSetWithMemoryUsage`：以 `MemAwareMap<String, ()>` 表达集合；`Insert<S: Into<String>>` 接受可转成所有权字符串的值（`set_with_memory_usage.rs:184-250`）。
- `Float64SetWithMemoryUsage`：私有 `MemAwareMap<FloatKey, ()>` 加 `next_nan_payload`；没有 Tracker 接口（`set_with_memory_usage.rs:254-308`）。
- `Int64SetWithMemoryUsage`：公开 `MemAwareMap<i64, ()>`；没有 Tracker 接口（`set_with_memory_usage.rs:312-361`）。
- 五个 `New*WithMemoryUsage`：接收空参数或初始切片，返回 `(容器, 初始字节数 i64)`；相应 `Default` 丢弃字节返回值，仅返回空容器。

公开 API 沿用 Go 风格大写命名；`lib.rs` 在 crate 根重导出这些符号。文件中没有 trait、枚举、模块级常量或条件编译项。

## 执行流程

构造流程如下：

1. `New*` 用期望元素数调用 `new_mem_aware_map`；后者经 `NewMemAwareMap` 创建带容量的 `SwissMap` 并执行 `Init`（`map_abi.rs:504-516`）。
2. 字符串映射/Decimal 映射直接读取初始化后的 `Bytes`；三种集合逐项调用自身 `Insert`，重复的非 NaN 键由底层映射覆盖。
3. 集合构造完成后返回最终 `Bytes`，而不是累加每次 `Insert` 的返回值（`set_with_memory_usage.rs:198-209,268-278,325-332`）。

普通插入流程为：容器把键和值交给 `MemAwareMap::Set`；`Set` 先写入，再根据当前元素数和 checkpoint 决定是否增加 `Bytes` 以及是否返回非零 delta（`map_abi.rs:437-447`）。字符串类容器若绑定 Tracker，则把非零 delta 传给 `Consume` 并返回 `0`；否则原样返回 delta。`f64` 插入前先生成 `FloatKey`，NaN 使用递增 payload；`i64` 直接写入。

查询与观测不改变状态：`Exist` 查底层键，`Count`/`Len` 取元素数，`Empty` 判断为空，`Bytes` 返回最近一次 checkpoint 估算。字符串映射独有 `Len`，但当前底层实现中 `Len` 与 `Count` 都返回 `SwissMap::len()`（`map_abi.rs:411-420,492-496`）。

## 数据与状态

每个容器的核心可变状态都是 `MemAwareMap`：`M` 保存实际条目，`groupSize` 和 `nextCheckpoint` 控制估算更新，`Bytes` 保存最近估算值（`map_abi.rs:391-396`）。本文件不缓存单次 delta；delta 是一次 `Set` 的返回值。

字符串相关三类容器额外保存 `Option<Arc<Tracker>>`。`SetTracker` 只影响未来的 `Insert`，不会把构造期字节或过去尚未上报的字节追溯写入新 Tracker，也不会在清除/替换 Tracker 时释放先前消费量。

`Float64SetWithMemoryUsage::next_nan_payload` 从 `1` 开始，每次插入 NaN 递增；达到 `FloatKey` payload 上限时会触发断言。非 NaN 使用稳定 bit key，零值统一成 `0`。`StringToDecimalMapWithMemoryUsage` 中的裸指针可以为空；测试明确插入 `null_mut()` 并只检查键存在性（`migration_aster_unit_test.rs:212-217`）。

本文件没有删除、清空或缩容 API，所以只描述增长记账；覆盖已有键通常不增加元素数，是否返回内存 delta仍由底层 checkpoint 状态决定，而不是由本层单独判断“新键”。

## 依赖与调用关系

下游依赖：

- `crate::hack::{MemAwareMap, NewMemAwareMap}`：保存数据并计算近似字节数；来自 Cargo 依赖 `astersql-util-hack`。
- `crate::memory::Tracker`：接收即时 delta；`Consume` 使用原子计数沿父 Tracker 链传播，并可能触发软/硬限制动作（`pkg/util/memory/tracker.rs:697-749`）。
- `crate::types::MyDecimal`：Decimal 映射的目标类型；本文件仅保存其裸指针。
- `crate::float64_set::FloatKey`：负责零值归一、NaN 插入和查询语义。
- 标准库 `Hash` 与 `Arc`：前者约束通用构造器的键，后者共享 Tracker 所有权。

上游证据：RustCodeGraph 对目标文件报告的文件级使用者包含本 crate 的测试；精确 `rg` 结果显示可执行 Rust 引用只在 `set_with_memory_usage_test.rs` 和 `migration_aster_unit_test.rs`，另有 `agg_hash_executor.rs` 中的注释代码。Go 对照文件则被 `pkg/executor/aggfuncs/func_avg.go`、`func_count_distinct.go`、`func_group_concat.go`、`func_sum.go`、`func_sum_int.go` 等生产代码使用。新增 Rust 生产调用前，应在真实执行器所有权与记账边界中明确由“返回 delta”还是“绑定 Tracker”负责消费，不能两者同时记账。

## 错误处理与边界

本文件的 API 不返回 `Result`，正常插入、查询和计数没有可恢复错误通道。需显式注意以下边界：

- `Bytes: u64` 转为 `i64` 使用 `as`；在极端超过 `i64::MAX` 的估算下会按 Rust 转换规则截断为有符号值，本层没有溢出检查。
- `FloatKey::for_insert` 在 NaN payload 耗尽时断言失败；NaN 查询刻意返回 `false`，即使集合已插入 NaN。
- Decimal 裸指针的有效性、别名关系和释放责任完全在调用方；`Exist`、`Count` 等不解引用它，因此空指针可作为值保存，但将来新增读取 API 必须建立明确的安全契约。
- Tracker 的锁若发生 poison、或超限动作自身失败/恐慌，行为来自 `Tracker::Consume`；本层不捕获。
- `MemAwareMap::Set` 返回 checkpoint 增量，零 delta 不等价于“没有插入或覆盖”，只能解释为本次无需增加近似字节记账。

## 并发与资源生命周期

容器修改方法都需要 `&mut self`，本文件没有为底层映射添加锁或异步任务；共享并发修改必须由调用方提供互斥。只读方法使用 `&self`，但能否跨线程共享还受键、值以及 Decimal 裸指针类型能力约束，不能仅因 Tracker 使用 `Arc` 就推断整个容器线程安全。

Tracker 由 `Arc` 共享，`Tracker::Consume` 的消费计数使用原子操作，并沿父链更新（`tracker.rs:701-749`）。容器持有一个强引用直到 `SetTracker(None)`、替换 Tracker 或容器析构；本层不执行对应负 delta，因此析构容器不会自动从 Tracker 释放先前累计的内存。

普通 `String` 和 `MemAwareMap` 资源随容器析构释放。Decimal 映射只丢弃指针值，不释放指向的 `MyDecimal`。本文件不包含锁、通道、事务、文件句柄或后台任务。

## 与 Go 版本的对应关系

Rust 五类结构、构造器和 `Insert` 主体与 `pkg/util/set/set_with_memory_usage.go` 一一对应；字符串类的“有 Tracker 时立即 Consume 并把返回 delta 清零”保持一致。Rust 用切片参数替代 Go 变参，并用 `Option<Arc<Tracker>>` 替代可空 `*memory.Tracker`。

Go 通过嵌入 `hack.MemAwareMap` 自动暴露方法；Rust 使用具名字段并显式转发 `Exist`、`Count`、`Len`、`Empty`、`Bytes`。Go Decimal 值是受 GC 管理的 `*types.MyDecimal`，Rust 对照为裸指针，生命周期保障并不等价，这是移植时最重要的安全差异。

浮点集合在外观上对应 Go 的 `map[float64]struct{}`，但 Rust 必须借助 `FloatKey` 主动复现 Go 行为：零符号归一，多次 NaN 插入分别计数，NaN 不可查询。`migration_aster_unit_test.rs:202-210` 验证了这些语义。

Go 测试文件是三组 allocation benchmark；Rust `set_with_memory_usage_test.rs` 保留相同行数表（含 425984/425985、851968/851969 边界），但稳定 test harness 中改为单轮插入并断言 `Count`，不提供 Go benchmark 的吞吐或分配统计。因此两者验证意图相关，但测量能力并不相同。

## 扩展指南

- 新增一种带记账容器时，复用 `new_mem_aware_map` 与现有构造/插入模式，并先决定是否需要 Tracker；若需要，明确构造期占用、返回 delta、即时消费三者的唯一记账责任。
- 修改内存语义时应先核对 `pkg/util/hack/map_abi.rs` 的 `Set`、`Init`、`Bytes` 与 checkpoint 规则，避免把估算值写成精确分配值。性能风险集中在预分配容量、字符串克隆和 checkpoint 频率。
- 扩展 Decimal 读取或删除能力前，必须先设计裸指针的所有权、有效期和释放策略；不能在无安全契约时解引用或回收。
- 扩展浮点行为应同步 `FloatKey`，并覆盖 `±0`、无穷、重复普通值、多次 NaN 与 payload 上限；不要直接改用 `f64` 作为 Rust `HashMap` 键来简化。
- 测试逻辑必须继续放在独立文件：功能/边界断言更新 `pkg/util/set/migration_aster_unit_test.rs`，批量插入边界更新 `pkg/util/set/set_with_memory_usage_test.rs`；若要对齐 Go 性能，应另建合适的 Rust benchmark，而不是把测试内嵌进生产文件。
- 接入 Rust 执行器时，重点审查 tracker 生命周期、重复记账、容器析构未释放已消费量，以及 Go 生产调用点与 Rust 执行路径是否真正等价。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`node --file pkg/util/set/set_with_memory_usage.rs` 读取全部 361 行；`query` 定位 `NewStringToStringMapWithMemoryUsage`、`NewFloat64SetWithMemoryUsage`、`MemAwareMap`、`Tracker`、`FloatKey`；`callers/callees --file` 对重名 Go 风格方法未返回边，故未用空结果推断“无调用”。
- 源码与模块：`pkg/util/set/set_with_memory_usage.rs`、`pkg/util/set/lib.rs`、`pkg/util/set/Cargo.toml`。
- 直接依赖实现：`pkg/util/hack/map_abi.rs:388-516`、`pkg/util/memory/tracker.rs:697-749,966-970`、`pkg/util/set/float64_set.rs:16-59`。
- Rust 测试：`pkg/util/set/set_with_memory_usage_test.rs`；`pkg/util/set/migration_aster_unit_test.rs:187-235` 覆盖去重、初始字节、零/NaN、空 Decimal 指针和 Tracker 消费。
- Go 对照：`pkg/util/set/set_with_memory_usage.go`、`pkg/util/set/set_with_memory_usage_test.go`；RustCodeGraph 文件级使用关系与精确 `rg` 用于区分 Go 生产调用和 Rust 当前接线。
- 人工复核结论：本文件存在是为了把集合成员关系与近似内存增量记账组合成统一 API；安全扩展的关键是不重复消费 delta、不误解 checkpoint `Bytes`、不越过 Decimal 指针生命周期边界，并保持 Go 浮点键语义。
