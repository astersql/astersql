# `pkg/util/set/float64_set.rs`

[查看对应 Rust 源文件](./float64_set.rs)

## 文件定位

本文件属于 `astersql-util-set` crate（`pkg/util/set/Cargo.toml`），实现与 Go 包 `pkg/util/set` 中 `Float64Set` 对应的 Rust 浮点集合。crate 入口 `pkg/util/set/lib.rs` 以 `pub mod float64_set` 挂载该文件，并通过 `pub use float64_set::*` 将 `Float64Set` 与 `NewFloat64Set` 重导出给 crate 使用者。

它是通用工具层的数据结构，不直接参与 SQL 解析、规划或存储协议。除了提供普通浮点集合，文件内 crate 可见的 `FloatKey` 还被 `pkg/util/set/set_with_memory_usage.rs` 复用，以保证普通集合和带内存统计集合采用同一套 Go `float64` map 键语义。

## 核心职责

- 用 `HashMap<FloatKey, ()>` 表示只关心键存在性的集合，并公开构造、插入、存在性查询和计数接口（`Float64Set`、`NewFloat64Set`、`Insert`、`Exist`、`Count`）。
- 弥合 Rust 浮点数不能直接满足常规 `HashMap` 键约束与 Go `map[float64]struct{}` 行为之间的差异（`FloatKey`）。
- 将 `+0.0` 和 `-0.0` 归一为同一个键；让任何 NaN 查询均失败；让每次 NaN 插入产生一个新的集合条目（`FloatKey::for_lookup`、`FloatKey::for_insert`）。
- 为 `Float64SetWithMemoryUsage` 提供 crate 内共享的浮点键编码，而不暴露 `FloatKey` 到 crate 外部（`pub(crate) struct FloatKey`）。

## 主要符号

- `FloatKey(u64)`：crate 内部哈希键，派生 `Clone`、`Copy`、`Debug`、`Eq`、`Hash`、`PartialEq`。它保存归一或人工生成后的位模式，而不是直接保存 `f64`。
- `FloatKey::for_lookup(value: f64) -> Option<FloatKey>`：查询键转换。NaN 返回 `None`；任一有符号零返回 `Some(FloatKey(0))`；其他值使用 `f64::to_bits()`。
- `FloatKey::for_insert(value: f64, next_nan_payload: &mut u64) -> FloatKey`：插入键转换。非 NaN 复用 `for_lookup`；NaN 使用指数位模板 `0x7ff0_0000_0000_0000` 与递增 payload 生成互不相等的键，并推进计数器。
- `Float64Set { inner, next_nan_payload }`：公开集合类型；字段保持私有。`inner` 是成员表，`next_nan_payload` 是下一次 NaN 插入使用的序号。
- `impl Default for Float64Set`：等价于 `NewFloat64Set(&[])`，产生空集合并将 NaN payload 初始化为 1。
- `NewFloat64Set(fs: &[f64]) -> Float64Set`：公开构造函数。按输入切片长度预分配 map，并逐项调用公开的 `Insert`，因此构造与后续插入共享完全相同的规范化规则。
- `Float64Set::Exist(&self, val: f64) -> bool`：公开只读查询；只有 `for_lookup` 返回键且 `inner` 包含该键时才为真。
- `Float64Set::Insert(&mut self, val: f64)`：公开变更操作；通过 `for_insert` 生成键，然后向 `inner` 写入单元值。
- `Float64Set::Count(&self) -> usize`：公开计数操作，直接返回 `inner.len()`。

本文件没有 trait 定义、类型别名、条件编译项或异步入口；唯一模块级常量 `NAN_PAYLOAD_MASK` 是 `for_insert` 内部的局部常量。

## 执行流程

构造流程从 `NewFloat64Set` 开始：先按 `fs.len()` 创建 `HashMap`、把 `next_nan_payload` 设为 1，再按切片顺序对每个值调用 `Insert`。普通非零、非 NaN 值按原始位模式成为键；两个有符号零都成为键 0；重复普通值覆盖原有单元值，不改变 `Count`；每个 NaN 则分配新的 payload，因此都会增加 `Count`。

插入流程为 `Float64Set::Insert -> FloatKey::for_insert -> FloatKey::for_lookup`。`for_lookup` 能处理的值直接返回稳定键；NaN 没有查询键，于是 `for_insert` 检查 payload 范围，构造专用于内部 map 的唯一键并将计数器加一，随后 `Insert` 写入 `inner`。

查询流程为 `Float64Set::Exist -> FloatKey::for_lookup -> HashMap::contains_key`。NaN 在第一步得到 `None`，短路为 `false`；有符号零和其他普通值使用与插入一致的稳定键查表。

计数流程没有遍历：`Count` 读取 `HashMap::len()`，所以复杂度为 O(1)。构造是对 n 个输入逐项插入，期望时间 O(n)；单次插入和查询遵循 `HashMap` 的期望 O(1) 特征。

## 数据与状态

`inner: HashMap<FloatKey, ()>` 是集合的权威成员状态。值始终为 `()`，不存在与成员键分离的业务载荷。普通值的身份由归一后的 IEEE 754 位模式决定：零被统一为 `0`，其余非 NaN 值保留 `to_bits()`，因此不同的有限值和正负无穷按各自位模式区分。

`next_nan_payload: u64` 仅为 NaN 插入服务，初始值为 1。每次 NaN 插入消耗一个 payload 并递增，所以 NaN 不会像普通重复值那样去重。`Clone` 会同时复制 map 和当前计数器；克隆后的两个集合状态独立，但会从相同的下一 payload 继续各自在自己的 map 中分配，这不会造成跨集合冲突。

关键不变量是：查询键永远不会为 NaN；所有零共用一个键；同一集合内已分配的 NaN 键在计数器耗尽前互不相同；`Count()` 始终等于 `inner.len()`。字段私有使 crate 外调用者不能绕开这些规则直接写 map 或计数器。

## 依赖与调用关系

下游依赖仅有 Rust 标准库 `std::collections::HashMap` 和 `f64` 的 `is_nan`、比较、`to_bits` 操作；本文件不使用 `pkg/util/set/Cargo.toml` 声明的 `hack-crate`、`memory-crate` 或 `types-crate`。这些依赖服务于同 crate 的其他集合实现。

RustCodeGraph 给出的内部调用边为：`Default::default -> NewFloat64Set`，`NewFloat64Set -> Insert`，`Insert -> for_insert -> for_lookup`，以及 `Exist -> for_lookup`。此外，图索引确认 `set_with_memory_usage.rs` 的 `Float64SetWithMemoryUsage::Insert` 调用 `FloatKey::for_insert`，其 `Exist` 调用 `FloatKey::for_lookup`；这是本文件唯一明确的跨文件实现级复用。

直接测试调用者包括 `pkg/util/set/float64_set_test.rs::TestFloat64Set` 和 `pkg/util/set/migration_aster_unit_test.rs::primitive_sets_match_go_membership_and_float_key_semantics`。`pkg/util/set/lib.rs` 负责公开重导出；仓库中的上层 facade 又在 `pkg/lib.rs` 重导出 util-set crate。当前 Rust 源码搜索未发现生产模块直接构造普通 `Float64Set`，因此它目前主要是可复用公开工具 API，而不是某条已确认 SQL 主链的专属节点。

## 错误处理与边界

公开 API 不返回 `Result` 或 `Option` 错误；正常插入、查询和计数没有业务错误分支。内存分配失败遵循标准库分配行为，本文件不做恢复或错误包装。

NaN 是最重要的语义边界：`Exist(NaN)` 恒为 `false`，即使此前插入过相同位模式的 NaN；每次 `Insert(NaN)` 都增加成员数。这不是常规数学集合语义，而是刻意复现 Go map 对不可自等键的行为。`+0.0` 与 `-0.0` 则必须去重并可相互查询。

NaN payload 的可用范围是 `1..=0x000f_ffff_ffff_ffff`。当计数器超出该范围时，`for_insert` 通过 `assert!` 以 `"too many NaN keys"` panic；这是理论上的资源上限保护，不是可恢复错误。公开类型没有删除、清空或迭代接口；需要这些能力时不能假设本文件已经支持。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件句柄或网络资源。集合完全由调用者拥有，`Insert` 要求 `&mut self`，Rust 借用规则阻止同一实例在无同步协调时并发写；`Exist` 和 `Count` 仅需 `&self`。若要跨线程共享并变更，调用者必须在本文件之外提供合适的同步容器。

资源生命周期与 `Float64Set` 值本身一致：构造时分配 `HashMap` 容量，插入时可能扩容，值离开作用域时由标准库自动释放。没有显式 `Drop` 实现。`Clone` 进行独立状态复制，而不是共享底层 map。

## 与 Go 版本的对应关系

Go 对照实现位于 `pkg/util/set/float64_set.go`。公开概念逐项对应：Go 的 `type Float64Set map[float64]struct{}` 对应 Rust 的 `Float64Set` 包装结构；Go 变参构造 `NewFloat64Set(fs ...float64)` 对应 Rust 切片构造 `NewFloat64Set(fs: &[f64])`；`Exist`、`Insert`、`Count` 的名称和成员语义保持一致。返回计数类型因语言习惯从 Go `int` 变为 Rust `usize`。

Rust 不能直接以 `f64` 作为满足 `Eq + Hash` 的 `HashMap` 键，因此增加了 Go 文件中不需要的 `FloatKey` 和 `next_nan_payload`。这层适配保留两项容易丢失的 Go 行为：`-0.0 == +0.0` 导致同键，以及 NaN 与包括自身在内的任何值都不相等，导致重复写入 NaN 产生多个不可按 NaN 查回的 map 条目。

`pkg/util/set/float64_set_test.go::TestFloat64Set` 与 `pkg/util/set/float64_set_test.rs::TestFloat64Set` 都验证普通浮点数的重复插入去重、计数、命中和未命中。Rust 额外的迁移回归 `pkg/util/set/migration_aster_unit_test.rs::primitive_sets_match_go_membership_and_float_key_semantics` 明确覆盖有符号零、无穷、重复 NaN 插入及 NaN 查询，作为 Rust 适配层语义的直接证据。

## 扩展指南

新增成员操作时，应优先在 `impl Float64Set` 中实现，并继续通过 `FloatKey::for_lookup` 或 `for_insert` 访问 `inner`，不要直接用未经规范化的浮点位模式。若改变零或 NaN 规则，必须同步检查 `pkg/util/set/set_with_memory_usage.rs`，因为其浮点集合直接复用这两个转换函数；普通版与内存统计版不应产生不同的键语义。

新增或修改测试应放在独立文件，而不是嵌入生产源文件。普通 API 回归应更新 `pkg/util/set/float64_set_test.rs`；Go 特有边界及迁移契约应更新 `pkg/util/set/migration_aster_unit_test.rs`，并与 `pkg/util/set/float64_set_test.go` 或新的 Go 对照用例核对。若新增删除、清空、迭代等 API，还应决定 NaN 条目是否可枚举或删除，并为多种 NaN 位模式、`±0`、无穷及普通重复值补充确定性测试。

兼容性风险集中在公开的 PascalCase API、`Count` 返回类型和 Go 键语义；性能风险集中在构造容量、哈希表扩容以及每个 NaN 都占独立条目。若改变 `FloatKey` 的可见性或布局，还需检查 `Float64SetWithMemoryUsage` 的编译与行为。本文档任务不改变运行时代码，因此未执行 Cargo 或代码测试。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/util/set` 确认目标 Rust/Go 源码及独立测试均在图中。
- RustCodeGraph 源码与调用查询：`node pkg/util/set/float64_set.rs::Float64Set`、`NewFloat64Set`、`Exist`、`Insert`、`Count`、`for_lookup`、`for_insert`；查询确认本文所列内部调用边以及 `set_with_memory_usage.rs` 对两个 `FloatKey` 转换函数的调用。
- crate 与模块边界：`pkg/util/set/Cargo.toml`、`pkg/util/set/lib.rs`、`pkg/lib.rs`。
- Rust 实现与直接复用点：`pkg/util/set/float64_set.rs`、`pkg/util/set/set_with_memory_usage.rs`。
- Go 对照：`pkg/util/set/float64_set.go`。
- 独立测试：`pkg/util/set/float64_set_test.rs`、`pkg/util/set/float64_set_test.go`、`pkg/util/set/migration_aster_unit_test.rs`。
- 人工复核结论：本文件存在是为了在 Rust `HashMap` 上提供与 Go 浮点 map 一致的轻量集合；运行时所有公开路径都汇入统一键转换；安全扩展必须同时维护普通集合、带内存统计集合及独立边界测试的一致性。
