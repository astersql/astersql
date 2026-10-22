# `pkg/planner/property/task_type.rs`

## 文件定位

通过 `TaskType::String` 和 `fmt::Display` 提供稳定文本表示，同时保留 Go 版本对非法/未来数值回退到 `UnknownTaskType` 的兼容行为。

## 核心职责

- 用 `TaskType(pub i32)` 表示任务在 TiDB/SQL 根层、TiKV coprocessor 或 MPP/TiFlash 层执行。
- 固定 `RootTaskType`/`CopSingleReadTaskType`/`CopMultiReadTaskType`/`MppTaskType` 的数值为 0/1/2/3，与 Go 版本 `iota` 顺序保持一致。
- 通过 `TaskType::String` 和 `fmt::Display` 提供稳定文本表示，同时保留 Go 版本对非法/未来数值回退到 `UnknownTaskType` 的兼容行为。

## 主要符号

- `pub struct TaskType(pub i32)`：透明的整数 newtype。字段公开，因而调用方可构造未知值；`Copy`/`Eq`/`Hash`/`Ord` 等 derive 使它适合作为轻量属性、比较值或键。`Default` 由单字段 newtype 派生，所以默认底层值为 0，等价于 `RootTaskType`。
- `RootTaskType = TaskType(0)`：TiDB/SQL 层根任务。
- `CopSingleReadTaskType = TaskType(1)`：TableScan 或 IndexScan 类单读 coprocessor 任务。
- `CopMultiReadTaskType = TaskType(2)`：IndexLookup 类先索引后回表的多读 coprocessor 任务。
- `MppTaskType = TaskType(3)`：当前主要在 TiFlash 执行的 MPP 任务。
- `TaskType::{RootTask, CopSingleReadTask, CopMultiReadTask, MppTask}`：与上述模块常量同值的关联常量，便于按类型命名空间访问。
- `TaskType::String(self) -> &'static str`：已知值分别返回 `rootTask`、`copSingleReadTask`、`copMultiReadTask`、`mppTask`，其余值返回 `UnknownTaskType`。
- `impl fmt::Display for TaskType`：将格式化委托给 `String`，保证 `{}` 与显式调用的文本契约一致。

## 执行流程

1. 规划器或属性构造器选择一个常量，并写入 `PhysicalProperty::TaskTp`；`PhysicalProperty::default` 与 `NewPhysicalProperty` 展示了默认根任务和显式传入两种路径（[`physical_property.rs`](physical_property.rs)）。
2. 下游以值比较选择计划分支。例如 `PhysicalProperty::GetAllPossibleChildTaskTypes` 对 root 返回单读 cop、多读 cop 和 root，对其他任务只返回自身；`IsFlashProp` 通过与 `MppTaskType` 比较识别 MPP 属性。
3. MPP 任务还会使 `PhysicalProperty::buildHashCode` 把 MPP 分区类型、分区列和可选向量检索列编入属性指纹；因此该值影响属性等价性和 Exchange 相关决策，而非仅用于日志。
4. 需要诊断或格式化时，`String` 匹配底层整数并返回静态字符串；`Display::fmt` 直接将该字符串写入 formatter。

## 数据与状态

`TaskType` 只持有一个 `i32`，没有内部缓存或隐式状态。`#[repr(transparent)]` 明确其表示遵循唯一字段，但本 crate 当前将它当作 Rust 属性值使用，文件中没有 FFI 入口。数值 0–3 是已知域；由于 tuple 字段公开，域外值也是可表示状态，其稳定行为只在本文件中定义为格式化时回退。

`PhysicalProperty::TaskTp` 是主要的外部状态承载点。其指纹使用 `TaskTp.0` 的原始整数，所以添加或重排判别值会影响属性哈希的兼容性。

## 依赖与调用关系

本文件的唯一直接依赖是标准库 `std::fmt`；`Cargo.toml` 无需为它单独引入第三方 crate。RustCodeGraph 的精确文件节点显示公开结构、四个常量、`String` 和 `Display::fmt`；同名全库 `String` 图查询含大量无关符号，因此调用侧以带路径的局部引用交叉核验。

直接上游是 [`physical_property.rs`](physical_property.rs)：它导入全部任务类型常量，由 `PhysicalProperty::TaskTp`、`wholeTaskTypes`、`IsFlashProp`、`GetAllPossibleChildTaskTypes` 和 `buildHashCode` 消费。通过 `lib.rs` 的再导出，该 API 还供上层 planner 传统 Go 实现及已移植 Rust 代码使用；例如 Go `pkg/planner/core/find_best_task.go` 用这些常量判定当前 physical task 是 root、cop 单/多读还是 MPP。

## 错误处理与边界

该模块没有 `Result`、panic 或可失败 I/O。对未知底层值，`String` 不拒绝、不修正也不报错，而是返回 `UnknownTaskType`；这只是显示容错，不意味着其他规划器分支能够安全处理未知值。底层整数为公开字段，所以如果边界需要严格验证，应在解码/构造入口完成，不应把 `String` 的回退当成验证。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源。`TaskType` 是 `Copy` 值，字符串输出均为 `&'static str`，因此没有动态分配、借用绑定资源或需要显式清理的生命周期。并发安全性由组成字段和自动 trait 推导决定；本文件没有自定义同步不变量。

## 与 Go 版本的对应关系

Go `type TaskType int` 对应 Rust `TaskType(pub i32)`；Go 的 `iota` 依次产生 0、1、2、3，Rust 显式写出相同数值。Rust 额外提供了关联常量和 `Display`，并派生了比较、哈希、排序与 `Default`；这些是 Rust API 便利性，不改变 Go 核心判别值和格式化契约。

Go 代码的 `int` 宽度随平台，Rust 固定为 `i32`；在当前 0–3 值域内没有语义差异。

## 扩展指南

新增任务类型时，至少要同步：（1）以不改动既有 0–3 值的方式新增模块常量；（2）如果保留关联常量 API，添加对应别名；（3）在 `TaskType::String` 增加稳定文本；（4）核对 `physical_property.rs` 中 `wholeTaskTypes`、`GetAllPossibleChildTaskTypes`、MPP 专用判定和指纹编码；（5）搜索 planner 中按任务类型穷举的代价与物理计划分支；（6）同步 Go 对照语义或清楚记录有意差异。

测试不应内嵌到 `task_type.rs`。应在独立的 `physical_property_test.rs` 扩展 `task_type_keeps_known_and_unknown_strings`，同时为子任务枚举、属性哈希或新执行层的计划选择增加针对性独立测试。主要兼容风险是判别值变动、遗漏穷举分支和指纹编码变化；性能风险不在这个常量封装本身，而在新任务层导致的计划空间扩张、Exchange 或远程执行决策。
