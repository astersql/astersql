# `pkg/util/size/size.rs`

## 文件定位

`size.rs` 是 `astersql-util-size` crate 的实现文件，定义两组可在编译期求值的公共常量：二进制容量单位和 Go 常见值类型的浅层大小。crate 入口 [`lib.rs`](lib.rs) 通过 `mod size; pub use size::*;` 将这里的全部常量提升为 crate 根 API；[`Cargo.toml`](Cargo.toml) 将库入口指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/util/size"` 标明它来自同目录 Go 包。它不参与 SQL 请求处理或存储 I/O，而是被表达式、KV、规划器、类型元数据和 codec 等模块用作内存占用估算的叶子常量库。

## 核心职责

第一项职责是提供 `KB`、`MB`、`GB`、`TB`、`PB` 五个 1024 进制的字节倍率，保持 [`size.go`](size.go) 中逐级乘以 1024 的定义。这里名称沿用 Go API；虽然文档用 KiB/MiB 等二进制含义解释它们，数值仍以兼容既有调用者为准。

第二项职责是把 Go `unsafe.Sizeof` 所表达的“值本身大小”映射为 Rust 编译期常量。`SizeOfSlice`、`SizeOfString`、`SizeOfInterface`、`SizeOfMap` 等只表示 Go 值头部或引用的大小，不递归计算底层数组、字符串字节、动态值、map 桶或键值。调用者通常将这些固定开销与 `len`/`capacity` 对应的动态存储相加，例如 [`pkg/expression/schema.rs`](../../expression/schema.rs) 和 [`pkg/kv/key.rs`](../../kv/key.rs) 的内存估算。

## 主要符号

- `KB: u64 = 1024` 是容量单位基数；`MB`、`GB`、`TB`、`PB` 依次以前一个常量乘 `1024`。使用 `u64` 与 Go 的 `KB = uint64(1024)` 一致，并能容纳 `PB`。
- `SizeOfSlice: i64` 使用 `[usize; 3]` 表示 Go slice 的 data/len/cap 三个机器字；`SizeOfString` 使用 `[usize; 2]` 表示 data/len 两个机器字；`SizeOfInterface` 同样用两个机器字表示类型信息和数据指针。
- `SizeOfByte` 与 `SizeOfUint8` 都取 `u8` 的大小；`SizeOfBool`、`SizeOfFloat64`、`SizeOfUint64`、`SizeOfInt32`、`SizeOfInt64` 分别取对应 Rust 标量的 `size_of`。
- `SizeOfInt` 与 `SizeOfUint` 分别以 `isize`、`usize` 保留 Go `int`、`uint` 随目标平台字宽变化的语义。
- `SizeOfPointer` 用 `*const isize`，`SizeOfFunc` 用函数指针 `fn()`，`SizeOfMap` 用 `*const ()` 表示一个机器字的引用值。三者都只计引用或函数值本身，不计所指对象。
- 所有 `SizeOf*` 均为 `i64`，与 Go 文件显式转换为 `int64` 的公共契约一致。文件级 `#![allow(non_upper_case_globals)]` 仅用于保留 Go 导出名的大小写，不改变常量行为。

本文件没有类型、trait、函数、`impl` 或条件编译项；全部符号都是公开常量。

## 执行流程

这些常量没有运行时控制流。编译器在目标平台上计算 `std::mem::size_of::<T>()`，并折叠逐级容量乘法；`lib.rs` 再导出结果。下游执行 `MemoryUsage` 一类方法时，读取常量作为固定开销，并把容器容量、元素数量或字符串长度对应的动态开销加上。例如 [`pkg/planner/util/tablesampler/sample.rs`](../../planner/util/tablesampler/sample.rs) 将两个指针、一个 slice 头部和分区元素对应的 interface 头部相加；[`pkg/expression/schema.rs`](../../expression/schema.rs) 将 slice 头部、指针数组容量及嵌套 key 的容量组合起来。

因此正确的调用顺序不是“调用本模块后分配资源”，而是“由调用者确定自身数据结构的动态部分，再引用这里的固定 Go 布局基数完成估算”。这些值是记账模型，不会读取或改变真实分配器状态。

## 数据与状态

模块只含不可变编译期常量，没有全局可变状态。容量单位固定为 1024 进制；类型大小中机器字相关项会随编译目标变化：32 位目标上的 `usize`/指针较小，64 位目标上较大。标量项则由 Rust 对应类型布局确定。

重要不变量是浅层计数：slice 为三个机器字，string/interface 为两个机器字，pointer/function/map 为一个机器字。这个模型刻意描述 Go 值布局，而不是 Rust `Vec`、`String`、trait object 或 `HashMap` 的完整布局，也不是递归的堆内存统计。下游若需要总占用，必须自行加入元素、容量及嵌套对象成本。

## 依赖与调用关系

下游唯一实现依赖是 Rust 标准库的 `std::mem::size_of`；[`Cargo.toml`](Cargo.toml) 没有 `[dependencies]`、feature 或 build script。根 workspace 在根 [`Cargo.toml`](../../../Cargo.toml) 中包含 `pkg/util/size`，并以 `facade_util_size` 别名暴露该包；多个 crate 也通过路径依赖直接引用它，例如 `pkg/kv`、`pkg/expression/aggregation`、`pkg/planner/property`、`pkg/planner/util`、`pkg/util/codec`、`pkg/infoschema` 和 `pkg/executor`。

RustCodeGraph 的文件查询将 `size.rs` 标记为被 35 个文件使用，但索引没有把本文件的大多数常量拆成可单独查询的定义，因此精确使用点由源码搜索补足。当前有行为的代表性调用边包括：

- [`pkg/expression/column.rs`](../../expression/column.rs)、[`expression.rs`](../../expression/expression.rs)、[`schema.rs`](../../expression/schema.rs) 和聚合实现用 `SizeOfPointer`、`SizeOfInterface`、`SizeOfSlice`、`SizeOfString` 估算表达式对象。
- [`pkg/kv/key.rs`](../../kv/key.rs) 用 `SizeOfInt64`、`SizeOfMap`、`SizeOfString`、`SizeOfInterface` 估算 key 与分区映射。
- [`pkg/planner/property/physical_property.rs`](../../planner/property/physical_property.rs) 以及 `pkg/planner/util` 下的 `byitem.rs`、`handle_cols.rs`、`tablesampler/sample.rs` 用这些常量组合计划属性的内存用量。
- [`pkg/types/field_name.rs`](../../types/field_name.rs) 和 [`pkg/util/codec/codec.rs`](../codec/codec.rs) 分别用于字段名与编码缓冲相关的浅层估算。

这些边均为常量读取；本模块不反向调用业务模块，也不会形成调用循环。

## 错误处理与边界

模块没有可失败操作、返回值或错误类型，因此不存在运行时错误传播。边界风险主要是语义误用：把 `SizeOfSlice` 当作元素数组总大小、把 `SizeOfString` 当作字符串内容长度、把 `SizeOfMap` 当作桶和键值总内存，都会低估内存；把这些 Go 布局常量当作 Rust容器真实 `size_of` 也可能得到错误结论。

容量乘法在当前最大值 `PB = 1024^5` 下远低于 `u64::MAX`。类型大小转换为 `i64` 对这些固定小值是安全的。平台相关值必须在目标平台上解释，不能把迁移测试在 64 位机器上的具体字节数硬编码为跨平台契约；独立测试对这些项使用 `size_of::<usize/isize>()` 计算期望值。

## 并发与资源生命周期

所有常量不可变且在编译期生成，可以被任意线程无锁读取；模块没有原子变量、锁、通道、异步任务或线程局部状态。它也不分配、持有或释放堆内存、文件描述符、网络连接与事务。资源生命周期完全属于下游被估算的数据结构，本模块只提供计算基数。

## 与 Go 版本的对应关系

[`size.go`](size.go) 是逐项对照基准。Rust 保留了全部五个容量常量和十五个 `SizeOf*` 名称、公开性及整数类型意图：容量为 `u64`，大小为 `i64`。标量使用直接对应的 Rust 类型；Go slice/string/interface 的运行时头部用固定数量的 `usize` 表示；Go pointer/map/function 值用一个 Rust 指针宽度表示。

这是一种 Go 兼容记账模型，而非声称 Rust 与 Go 的所有 ABI 布局相同。尤其是 Rust 原生 slice 引用、trait object、函数对象和集合可能具有不同表示；这里选择的占位类型只为重现 `unsafe.Sizeof` 在 Go 文件中的结果。独立测试 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 验证二进制单位的精确数值、标量对应、三/二/一机器字头部关系，并通过 `include_str!("size.rs")` 检查 AsterSQL 版权行。搜索未发现同目录 Go 单元测试；仓库中的 Go 调用者测试只间接消费 `pkg/util/size`。

## 扩展指南

新增常量时，应先确认它是 Go `pkg/util/size` 的现有/新增公共契约，还是 Rust 独有需求。Go 对齐项应在 `size.rs` 保持原名、符号类型和浅层/深层含义，并在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 增加等价断言；不要把测试内嵌回生产文件。若常量描述容器，文档和测试必须明确是否包含底层存储，避免下游重复或遗漏计数。

修改机器字模型时需要同时检查 32 位与 64 位语义，以及所有使用 `capacity() * SizeOf*` 的调用点；修改容量单位会影响配置阈值或测试期望，不能把二进制单位悄然改成十进制。若要计算 Rust 数据结构的真实布局，宜建立语义清楚的新 API，而不是改变既有 Go 兼容常量。性能风险很低，因为读取常量没有运行时成本；主要兼容风险是改变公共数值后使内存追踪、限额或缓存估算发生系统性偏差。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/util/size` 找到 `lib.rs`、`size.rs`、`size.go` 与独立迁移测试；`node --file pkg/util/size/size.rs --offset 1 --limit 260` 读取完整 129 行实现并报告 35 个使用文件。对 `SizeOfSlice`、`SizeOfMap`、`PB` 执行 `query/callers` 时，索引未能解析本文件的常量定义，这一覆盖限制已用精确源码搜索补充，未把同名符号误当成调用边。
- 源与 crate 边界：完整核对 [`size.rs`](size.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml) 以及根 workspace 的成员/依赖声明。
- Go 对照：逐项核对 [`size.go`](size.go) 的 `uint64` 容量链与 `unsafe.Sizeof` 定义。
- 测试证据：读取 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `binary_capacity_units_match_go_constants`、`scalar_sizes_match_go_unsafe_sizeof`、`go_runtime_header_sizes_exclude_backing_storage` 和编译期版权断言；精确搜索没有发现同目录 Go 测试。
- 调用证据：用 `rg` 核对 `SizeOf*` 在表达式、KV、planner、types 与 codec Rust 文件中的真实读取，以及各下游 `Cargo.toml` 对 `astersql-util-size` 的路径依赖。按任务约束未运行 Cargo；文档结构另以任务给定命令验证。
