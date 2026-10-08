# `pkg/util/hack/map_abi.rs`

## 文件定位

本文件属于 `astersql-util-hack` crate。crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，入口 [`lib.rs`](lib.rs) 将它声明为公开的 `map_abi` 模块；包根当前默认重导出的是 Go 1.26 对应的 `map_abi_go126`，因此本文件的 Go 1.25 兼容 API 通常经 `astersql_util_hack::map_abi::*` 显式访问。

它位于业务容器与内存记账之间：上层使用 `MemAwareMap<K, V>` 获得 map 操作及插入时的内存增量，底层 [`swiss_map.rs`](swiss_map.rs) 用安全的 Rust 所有权模型模拟 Swiss table 的目录、table、group、seed 和 clear 序列。本文件同时保留 Go 1.25 runtime ABI 的结构镜像，供语义对照、布局公式和潜在 FFI 边界使用，但 Rust `HashMap` 不能直接重解释为 Go runtime map。

可见的直接使用者包括 [`pkg/kv/key.rs`](../../kv/key.rs) 的 `MemAwareHandleMap`、[`pkg/executor/aggfuncs/aggfuncs.rs`](../../executor/aggfuncs/aggfuncs.rs) 的 `AggPartialResultMapper`，以及 [`pkg/util/set/set_with_memory_usage.rs`](../set/set_with_memory_usage.rs) 的多种带内存追踪集合。

## 核心职责

1. 以 `swissMapTable`、`groupsReference`、`swissMap`、`abiType`、`swissMapType`、`SwissMapWrap` 镜像 Go 1.25 Swiss-map 的关键内存布局，并提供目录、group、key、element 的裸指针寻址公式。
2. 用 `swissMap::Size`、`swissMap::Cap`、`groupCap` 和 `SwissMapWrap::Size` 表达 Go ABI 下的容量及精确内存统计规则：小 map 只有一个 group；目录中连续重复的 table 指针只计一次。
3. 用 `MemAwareMap<K, V>` 包装安全的 `SwissMap<K, V>`，提供构造、插入、查询、长度判断、固定测试 seed 和内存统计。
4. 通过 checkpoint 而不是每次插入后遍历目录来维护 `Bytes`：`approxSize` 使用 Go 版本保留的经验比例，`RealBytes` 则委托安全 Swiss-map 模型计算实际分配量。
5. 明确隔离不能在纯 Rust 中成立的 ABI 操作：`ToSwissMap` 和 `MemAwareMap::unwrap` 会 panic，`checkMapABI` 也是空的兼容占位；真实业务路径不依赖这三个入口。

## 主要符号

- `maxTableCapacity = 1024`：单次 checkpoint 间隔的上限，也对应 Go Swiss table 在目录层拆分前的最大容量。
- `swissMapGroupSlotsBits` / `swissMapGroupSlots`：每个 group 固定 8 个 slot。
- `swissMapTable`：记录 `used`、`capacity`、`growthLeft`、局部深度、目录索引和 group 数组引用。字段顺序来自 Go 1.25 `internal/runtime/maps.table`。
- `groupsReference` / `groupReference`：分别表示 group 数组和单个 group 的裸指针视图；`group`、`key`、`elem` 都是 `unsafe` 地址运算。
- `swissMap`：镜像 Go map header；`Used` 必须为首字段。`directoryAt` 查目录，`Size` 去重统计 table，`Cap` 去重累计容量，`MockSeedForTest` 只允许空 map 改 seed。
- `abiType` / `swissMapType`：镜像 Go `internal/abi.Type` 与 `SwissMapType`，包含 key/element/group 类型指针、hasher、group/slot 大小和 element 偏移。
- `SwissMapWrap`：成对保存 ABI 类型指针和 map 数据指针；`from_raw_parts` 是唯一能建立有效 ABI 视图的 Rust 入口，调用者承担完整安全契约。
- `approxSize(groupSize, maxLen)`：计算 `groupSize * maxLen * 204 / 1000`，用于 checkpoint 的近似值。
- `groupCap(groupSize, slotSize)`：按 `(groupSize - groupSlotsOffset) / slotSize` 计算 group 容量。
- `MemAwareMap<K, V>`：实际业务包装，字段 `M` 保存安全 `SwissMap`，`groupSize` 保存布局大小，`nextCheckpoint` 控制估算频率，`Bytes` 保存单调不减的估计字节数。
- `NewMemAwareMap(capacity)`：以给定容量建立 `SwissMap`，再调用 `Init` 完成记账初始化，返回装箱后的包装器。

## 执行流程

构造流程从 `NewMemAwareMap` 开始：先调用 `SwissMap::with_capacity`，再把该值交给 `MemAwareMap::Init`。`Init` 通过 `swiss_map::group_size::<K, V>()` 得到布局大小，由 `real_bytes_for_capacity` 调用 `SwissMap::size` 得到初始 `Bytes`；元素数不超过 8 时，下一个 checkpoint 固定为 16，否则设为 `used + min(used, 1024)`。

插入流程由 `Set` 执行。它先写入 `M`，再读取新长度。若长度未达到 `nextCheckpoint`，返回 0；达到时计算 `max(旧 Bytes, approxSize(groupSize, used))`，返回新旧字节数之差，并把下一个 checkpoint 推进 `min(used, 1024) + used`。因此小规模插入不会每次触发记账，大规模 map 的检查间隔最终最多增加 1024 个元素。

`SetExt` 在调用 `Set` 前以 `contains_key` 判断键是否已存在，返回 `(delta, insert)`；覆盖已有键时 `insert` 为 false。`Get` 返回 `(Option<&V>, bool)`，`Count`/`Len` 返回元素数，`Empty`/`Exist` 提供常见判定。

精确统计有两条实现路径。ABI 路径的 `swissMap::Size` 先计算 header 和目录指针数组；`dirLen == 0` 时直接加一个 group，否则顺序扫描目录，并跳过与前一项相同的 table 指针。业务路径的 `RealBytes` 不解引用 Go 指针，而是调用安全 `SwissMap::size` 复现同一分配模型。`Cap` 同样只累计唯一 table；group 的 key/element 地址则由 `groupSlotsOffset`、`SlotSize` 与 `ElemOff` 组合得到。

## 数据与状态

ABI 镜像中的状态全部依赖 Go 1.25 布局：`dirPtr`/`dirLen` 决定小 map 或目录模式，`globalDepth`/`globalShift` 描述目录寻址，`writing` 是 Go runtime 的并发写检测标志，`tombstonePossible` 表示是否可能含墓碑，`clearSeq` 用于发现迭代期间的 clear。Rust 代码只在得到合法 Go FFI 指针时才能解释这些字段。

业务状态由 `SwissMap<K, V>` 持有。`M.len()` 是 `Set`、`Count`、`Len` 和 checkpoint 的元素数来源；`groupSize` 由 key/value 大小计算；`Bytes` 是近似账面值，采用 `max` 保证不会因覆盖、删除或估算波动而下降；`RealBytes` 表示当前已分配结构的模型值。因此 clear 后长度可以归零，但容量、`RealBytes` 和既有 `Bytes` 仍保留，等待后续复用。

`mockSeedForTest` 仅为确定性测试服务。`MemAwareMap::MockSeedForTest` 委托 `SwissMap::set_seed`；测试证明 clear 会推进 `clear_seq` 并更换 seed。生产逻辑不应依赖固定 seed。

## 依赖与调用关系

下游依赖均在 crate 内或标准库中：`std::collections::HashMap` 只出现在不可用的 `ToSwissMap` 签名中，`std::ffi::c_void` 和 `std::mem` 支撑 ABI 指针与布局计算，`std::hash::Hash` 约束键；实际存储依赖 `crate::swiss_map::SwissMap`。[`Cargo.toml`](Cargo.toml) 没有普通依赖，只有测试用 `astersql-testkit-testsetup` 开发依赖。

RustCodeGraph 对目标文件列出 51 个符号；精确源码和仓库搜索显示 `NewMemAwareMap` 的直接上游包括 `pkg/kv/key.rs::NewMemAwareHandleMap/newMemAwareMap`、`pkg/executor/aggfuncs/aggfuncs.rs::new_agg_partial_result_mapper_with_capacity`、`pkg/util/set/set_with_memory_usage.rs::new_mem_aware_map`。这些上层随后调用 `Set`，把返回 delta 交给句柄记账、聚合内存控制或 `memory::Tracker::Consume`。

`lib.rs` 同时公开 `map_abi` 与 `map_abi_go126`，但仅 glob 重导出后者。因而 Go 1.25 兼容实现不会意外替换当前默认 API；需要本文件语义的调用者必须显式选择 `map_abi`，或者经 `pkg/kv`、`pkg/tablecodec` 等依赖别名再导出。

## 错误处理与边界

普通 map 操作不返回 `Result`，内存计算也没有可恢复错误通道。主要失败方式是违反不变量时 panic 或触发 unsafe 未定义行为：

- `SwissMapWrap::from_raw_parts` 会拒绝空指针，但无法验证指针是否真的是彼此匹配且仍存活的 Go 1.25 map/type；错误的非空指针仍可能导致未定义行为。
- `directoryAt`、`Size`、`Cap`、`groupsReference::group`、`groupReference::key/elem` 都要求调用者保证 ABI、边界、对齐和生命周期正确。
- `ToSwissMap` 与 `MemAwareMap::unwrap` 无条件 panic，因为 Rust `HashMap` 没有 Go runtime header；它们只保留移植形状，不能作为业务扩展点。
- `swissMap::MockSeedForTest` 在 `Used != 0` 时 panic；业务使用的 `MemAwareMap::MockSeedForTest` 也应只对空 map 调用。
- `groupCap` 假设 `groupSize >= groupSlotsOffset` 且 `slotSize != 0`，`Set` 中的乘法和整数转换也依赖实际类型规模与 map 长度不造成溢出。现有调用由 `swiss_map::group_size` 构造合法参数。
- `Init` 与 Go 版本不同：Rust 参数是 `Into<SwissMap<K, V>>`，不存在 nil map，因此 Go 的 nil panic 边界只保留在注释中。

## 并发与资源生命周期

`MemAwareMap` 不提供内部锁；`Set` 同时修改存储、`Bytes` 和 `nextCheckpoint`，必须由单一可变借用者操作。需要共享访问时，上层应像其他仓库容器一样在外部使用互斥/RW 锁，而不是绕过 Rust 借用规则。ABI 镜像中的 `writing` 字段只是被观察的 Go runtime 状态，不会让 Rust 包装器自动具备并发安全性。

安全业务路径的资源由 `SwissMap` 和 `Box<MemAwareMap>` 按 Rust 所有权释放，不启动任务、不持有通道、锁或事务。clear 只清除逻辑元素并保留已分配 table/group；drop 才结束整个容器生命周期。

FFI 路径完全不同：`SwissMapWrap`、`groupsReference` 和 `groupReference` 只是借来的裸指针视图，没有所有权，也没有自动释放。建立视图后，Go map/type 结构必须在每次访问期间保持存活、地址稳定且版本匹配；Rust 不得释放或长期缓存这些指针。当前仓库业务路径没有建立此类 FFI 视图。

## 与 Go 版本的对应关系

对应源文件是 [`map_abi.go`](map_abi.go)，其 build tag 为 `go1.25 && !go1.26`。常量、ABI 结构字段、目录去重统计、group slot 地址公式、204/1000 估算系数、checkpoint 规则以及 `MemAwareMap` 的公开方法均按 Go 结构保留。

关键差异在存储实现：Go 的 `ToSwissMap` 可把 `map[K]V` 接口重解释为 runtime 指针，`unwrap` 也能取得真实 map header；Rust 标准 `HashMap` 与 Go ABI 无关，所以这两个入口显式 panic。Rust 的 `MemAwareMap` 改用 [`swiss_map.rs`](swiss_map.rs) 模拟 Go Swiss table 的增长与内存布局，`RealBytes` 因而是该模型的实际分配量，而不是读取 Rust `HashMap` 内部实现。

Go 的 `SetExt` 比较写入前后的 `Used`，Rust 则在写入前调用 `contains_key`，对满足 `Eq + Hash` 的键保持“新键/覆盖”的外部语义。Go `Init` 拒绝 nil map；Rust 类型系统消除了 nil。Go `checkMapABI` 在启动时验证 runtime 版本，Rust 版本为空函数，因为本文件的业务存储不依赖运行中的 Go runtime。

Go 1.26 的布局变化由相邻 [`map_abi_go126.rs`](map_abi_go126.rs) 单独维护；修改 ABI 镜像时不能假定两个版本字段完全相同。

## 扩展指南

新增普通 map 能力时，应优先扩展 `MemAwareMap` 并委托 `SwissMap`，保持 `Bytes` 单调、checkpoint 更新顺序和 `Set` 的 delta 语义。若增加删除或批量写接口，需要先明确它是否只改变逻辑长度、是否回收 table，以及返回的内存 delta 如何与现有上层 `memory::Tracker` 协作；不能简单按元素大小扣减 `Bytes`。

修改容量、group 大小或增长算法时，应同步检查 `swiss_map.rs` 的 `group_size`/`size`/目录增长，本文件的 `approxSize`、`groupCap`、`Init`、`Set`，以及 Go 1.25 对照实现。ABI 字段或指针算法的任何调整都必须以对应 Go runtime 源码和目标架构的 `size_of`/对齐结果为依据，并同步评估 `map_abi_go126.rs`，但不要把 1.26 布局直接复制到本文件。

测试必须放在独立文件。首选扩展 [`map_abi_test.rs`](map_abi_test.rs) 覆盖公开行为和 Go 对照 workload；版本特定类型别名放在 [`map_abi_test_type_go125_test.rs`](map_abi_test_type_go125_test.rs)；跨版本迁移回归可放在 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。至少覆盖空 map、小 map 跨 8→9 元素、重复键、checkpoint 边界、clear 后复用、大容量增长以及 seed 不变量。涉及裸指针时还需覆盖空指针、错误版本、越界与生命周期契约，且不应把 unsafe 测试逻辑嵌入生产源文件。

性能风险主要来自把 `RealBytes` 放进热路径、缩短 checkpoint 间隔或重复扫描目录；兼容风险主要来自 Go runtime ABI 漂移；正确性风险主要来自内存 delta 被上层重复消费或遗漏消费。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/util/hack` 确认目标、两个版本实现和独立测试均已索引；`node --file pkg/util/hack/map_abi.rs` 读取完整 522 行并显示 51 个符号；`query MemAwareMap` 与 `query NewMemAwareMap` 确认同名版本实现和仓库调用候选。通用方法名的 callers/callees 查询存在名称歧义，因此调用边又以文件限定搜索核实。
- 生产源码：[`map_abi.rs`](map_abi.rs) 的 ABI 镜像、裸指针方法、`MemAwareMap` 和构造器；[`swiss_map.rs`](swiss_map.rs) 的安全存储、增长与 size 模型；[`lib.rs`](lib.rs) 的模块公开及默认重导出；[`Cargo.toml`](Cargo.toml) 的 crate 与依赖边界。
- 上游调用：[`pkg/kv/key.rs`](../../kv/key.rs) 的句柄映射、[`pkg/executor/aggfuncs/aggfuncs.rs`](../../executor/aggfuncs/aggfuncs.rs) 的聚合部分结果映射、[`pkg/util/set/set_with_memory_usage.rs`](../set/set_with_memory_usage.rs) 的集合与 tracker 记账。
- Go 对照：[`map_abi.go`](map_abi.go) 的 Go 1.25 build tag、runtime ABI 转换、内存公式、checkpoint 和 panic 条件；[`map_abi_test.go`](map_abi_test.go) 的布局、目录扫描、精确字节数和 benchmark workload。
- Rust 测试：[`map_abi_test.rs`](map_abi_test.rs) 覆盖五种 key/value 布局、1024/2000/51199 次插入、8→9 元素增长、覆盖写、`SetExt`、clear/seed/checkpoint 和原生 map workload 对照；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 覆盖 Go 1.25/1.26 公开行为、目录增长和精确初始字节数。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行任务指定的 11 章节结构检查，并人工复核链接、符号、边界和“如何安全扩展”的说明。
