# `pkg/util/slice/slice.rs`

## 文件定位

本文件是 `astersql-util-slice` crate 的业务实现，提供与 Go 包 `pkg/util/slice` 对齐的通用切片操作。crate 入口 `pkg/util/slice/lib.rs` 通过 `pub mod slice` 声明该模块，并以 `pub use slice::*` 将其公开项提升到 crate 根；工作区根 crate 又在 `pkg/lib.rs` 的 `util::slice` 模块中通过 `facade_util_slice::*` 再导出，因此调用方既可直接依赖 `astersql-util-slice`，也可经根 facade 的 `crate::util::slice` 访问。

`pkg/util/slice/Cargo.toml` 将库入口指定为 `lib.rs`，没有声明普通依赖或 feature；其移植元数据把 Go 来源标为 `pkg/util/slice`。本文件不是 SQL 请求主链上的状态组件，而是供 DDL、统计、规划等上层模块复用的无状态基础工具。当前 Rust 仓库中可确认的应用接线是 `pkg/lib_test.rs::independent_util_modules_are_wired` 经根 facade 调用 `AllOf`；大量业务使用仍位于 Go 文件，例如 `pkg/ddl/partition.go`、`pkg/statistics/handle/bootstrap.go` 和 `pkg/planner/**`。

## 核心职责

- `AllOf` 判断输入切片中的每个元素是否满足谓词，并保持空切片恒真的全称量化语义以及遇到首个反例即停止的短路行为。
- `Int64sToStrings` 把 `i64` 切片逐项格式化为十进制 `String`，保持长度、顺序和重复值。
- `DeepCloneItem` 抽象 Go 元素类型的 `Clone() T` 能力；其 blanket implementation 让所有实现标准 `Clone` 的 Rust 类型自动满足该约束。
- `DeepClone` 逐元素调用 `DeepCloneItem::Clone` 构造新 `Vec`，并用 `Option` 保留 Go 中 nil slice 与非 nil 空 slice 的区别。

这些 API 只负责切片遍历、转换和复制，不负责验证业务值、不缓存结果，也不持有跨调用状态。

## 主要符号

- `pub fn AllOf<T, F>(s: &[T], mut p: F) -> bool where F: FnMut(&T) -> bool`：公开泛型函数。谓词接收借用的元素，元素不需要实现 `Clone`；`FnMut` 允许谓词记录访问次数等局部状态。实现为 `!s.iter().any(|x| !p(x))`。
- `pub fn Int64sToStrings(ints: &[i64]) -> Vec<String>`：公开转换函数。对每个元素调用 `ToString::to_string` 并 `collect` 为新向量。
- `pub trait DeepCloneItem: Sized`：公开元素复制协议，只定义 `fn Clone(&self) -> Self`。方法名称为兼容 Go API 保留大写，本文件用 `#![allow(non_snake_case)]` 接受这组移植名称。
- `impl<T: Clone> DeepCloneItem for T`：标准 `Clone` 类型的统一适配层，默认把 `DeepCloneItem::Clone` 转发到 `Clone::clone`。
- `pub fn DeepClone<T: DeepCloneItem>(s: Option<&[T]>) -> Option<Vec<T>>`：公开切片复制函数。`None` 直接返回 `None`；`Some` 先按原长度预留容量，再按输入顺序复制元素。

文件没有模块级常量、结构体、枚举、宏或条件编译项。所有业务符号都是公开 API；循环、匹配和迭代器闭包均为函数内部实现。

## 执行流程

`AllOf` 的流程如下：

1. 通过 `s.iter()` 按原顺序借用元素。
2. 对每个元素计算 `!p(x)`，把“不满足谓词”转换为反例条件。
3. `Iterator::any` 在首个反例处返回 `true` 并停止迭代；外层取反得到 `false`。
4. 若没有反例（包括输入为空），`any` 返回 `false`，最终结果为 `true`。

`Int64sToStrings` 对输入建立迭代器，依次调用十进制字符串转换，再收集为独立 `Vec<String>`。没有过滤、排序或聚合步骤，因此输出与输入一一对应。

`DeepClone` 先匹配 `Option<&[T]>`：`None` 提前返回；`Some(slice)` 创建容量为 `slice.len()` 的空向量，随后顺序遍历并把每个 `item.Clone()` 的结果压入向量，最终包装为 `Some`。容量预留避免常规增长过程中的重复扩容，但长度只有在逐项压入后才增加。

## 数据与状态

三个函数都不修改输入。`AllOf` 只借用切片，不过 `FnMut` 谓词自身可以维护由调用方拥有的可变捕获状态；短路意味着首个反例之后的元素不会传给谓词。`Int64sToStrings` 为每个整数分配一个拥有所有权的字符串，并返回拥有这些字符串的新向量。

`DeepClone` 返回的新向量拥有每个克隆元素。它保证调用 `DeepCloneItem::Clone`，但“深”的层级由元素实现决定：blanket implementation 与标准 `Clone` 的语义完全一致，不能额外保证自定义引用计数、句柄或内部共享资源变为物理独立。`Option` 是重要状态编码：`None` 对应 Go nil slice，`Some(&[])` 对应非 nil 空 slice，并分别产生 `None` 与 `Some(Vec::new())`。

本文件没有全局变量、缓存、锁、通道、事务或后台任务。

## 依赖与调用关系

下游依赖全部来自 Rust 标准库：`AllOf` 使用切片迭代器和 `Iterator::any`，`Int64sToStrings` 使用整数的 `ToString` 实现和 `Iterator::collect`，`DeepClone` 使用 `Option`、`Vec::with_capacity`、迭代以及 `Vec::push`。`pkg/util/slice/Cargo.toml` 未列出外部 crate，因而这里没有第三方错误类型或运行时耦合。

上游装配链为 `pkg/util/slice/lib.rs` → `astersql-util-slice` crate → 根 `Cargo.toml` 的 `facade_util_slice` 路径依赖 → `pkg/lib.rs::util::slice` 再导出。RustCodeGraph 的文件查询识别出 `slice.rs` 的 7 个符号，并将该文件关联到 13 个使用文件；由于索引同时包含 Go/Rust，调用关系需要按语言区分。已核实的 Rust 调用/引用包括：

- `pkg/lib_test.rs::independent_util_modules_are_wired` 通过 `crate::util::slice::AllOf` 验证 facade 接线。
- `pkg/planner/core/generator/plan_cache/plan_clone_generator.rs` 生成包含 `sliceutil.DeepClone(...)` 的 Go 源代码文本；它不是对 Rust `DeepClone` 的运行时调用。
- `pkg/planner/core/operator/physicalop/plan_clone_generated.rs` 中的命中位于保留的 Go 源码注释，同样不是 Rust 调用边。

Go 生产调用展示了该工具包的业务位置：`AllOf` 用于 DDL 参数类型检查和 workload repository 条件判断；`Int64sToStrings` 用于统计 bootstrap 中拼接表 ID；`DeepClone` 广泛用于 planner 路径、range、column、sort item 等结构的复制。当前证据不支持宣称这些 Go 业务调用已全部迁移为 Rust 运行时调用。

## 错误处理与边界

API 不返回 `Result`，也没有显式错误分支。`AllOf` 对空输入返回 `true`；谓词副作用只会发生到首个反例为止。若谓词 panic，panic 原样向上传播。`Int64sToStrings` 能表示 `i64::MIN`、零和 `i64::MAX`，负号与十进制格式由标准库保证；它不会解析字符串，也不存在非法整数输入。

`DeepClone(None)` 返回 `None`，不会分配或调用元素复制；`DeepClone(Some(&[]))` 返回 `Some` 空向量，保留与 nil 的区别。元素复制发生 panic 时，没有恢复逻辑：已构造的局部向量按 Rust 栈展开规则析构，panic 继续传播。极大输入的内存分配失败属于标准分配器行为，本文件不将其转换为业务错误。

需要特别注意 blanket implementation 的一致性规则：由于所有 `T: Clone` 已自动实现 `DeepCloneItem`，调用方不能再为同一 `Clone` 类型提供另一个冲突实现。若某类型需要区别于标准 `Clone` 的深拷贝语义，应避免为该类型实现标准 `Clone`，再显式实现 `DeepCloneItem`，或先重新评估 API 设计。

## 并发与资源生命周期

实现没有共享可变状态，因此函数本身不引入锁竞争、线程、异步任务或通道。能否跨线程调用取决于输入元素、谓词捕获和返回类型自身的 `Send`/`Sync` 属性；本文件没有额外添加这些约束。

借用只在函数调用期间存在。`AllOf` 和 `Int64sToStrings` 不把输入引用保存到返回值中；`DeepClone` 也在返回前把每个元素复制为拥有所有权的值。`DeepClone` 预留的向量以及已经复制的元素在提前 panic 时自动释放。对于文件句柄、连接、`Arc` 等资源，释放或共享规则仍由具体元素的 `Clone` 和 `Drop` 实现决定，不能从“DeepClone”名称推导额外生命周期保证。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/util/slice/slice.go`。三组 API 的控制流保持一致：Go `AllOf` 以 `!slices.ContainsFunc(s, !p)` 表达“找不到反例”，Rust 使用 `!iter().any(!p)`；两者都对空切片返回真并短路。Go 谓词按值接收 `T`，Rust 为避免不必要复制而接收 `&T`，这是调用签名差异而非判定语义变化。

Go `Int64sToStrings` 先按长度创建 `[]string` 再按索引写入 `strconv.FormatInt(v, 10)`；Rust 使用 `map`/`collect` 和 `i64::to_string()`。二者都生成十进制表示并保持顺序。Go 的 nil `[]int64` 会得到长度为零但非 nil 的结果切片；Rust 输入没有 nil 切片表示，因此只覆盖空切片结果。

Go `DeepClone` 的约束是 `interface{ Clone() T }`，nil 输入返回 nil，随后以输入长度为容量逐项调用 `Clone()`。Rust 以 `DeepCloneItem` 表达该协议，以 `Option<&[T]>` 显式表达 nil，并为 `T: Clone` 提供适配。`pkg/util/slice/slice_test.rs` 覆盖显式 `DeepCloneItem` 实现，`migration_aster_unit_test.rs` 还覆盖标准 `Clone` 类型；这比现有 Go `slice_test.go`（仅直接测试 `AllOf`）的本地覆盖更广，但并不改变 Go API 的语义基线。

## 扩展指南

新增通用切片操作时，应先判断它是否属于无状态、与元素业务类型无关的能力；若是，在 `slice.rs` 中新增公开函数，并由现有 `lib.rs` 自动再导出。API 名称和 nil/空切片、顺序、短路、所有权语义应先与 `pkg/util/slice/slice.go` 对照；不能直接对应时，应在签名和文档中明确 Rust 的表示选择。

行为测试应放在独立的 `pkg/util/slice/slice_test.rs`，迁移语义的补充回归可延续 `pkg/util/slice/migration_aster_unit_test.rs`，不要把测试嵌入生产源文件。至少覆盖空输入、边界值、顺序/重复值和可观察的短路；涉及复制时还需验证 nil 与空集合的区别，并根据元素类型验证所有权或地址独立性。若改变公开面，还要确认 `pkg/util/slice/lib.rs` 与根 `pkg/lib.rs::util::slice` 的 facade 仍可访问，必要时扩展 `pkg/lib_test.rs` 的接线烟测。

修改 `DeepCloneItem` 或 blanket implementation 风险最高：可能造成 trait 实现冲突、改变方法解析，或让调用者误判资源是否真正独立。性能修改应保留单次线性遍历和合理预分配；不要为了复用而引入中间集合。与 Go 版本共同维护时，应同步检查 `slice.go`、`slice_test.go` 及真实 Go 调用点，避免只让 Rust 单测通过却偏离原始业务语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件且目标目录已收录；`files --filter pkg/util/slice` 列出 `slice.rs`、crate 入口、Go 对照及独立测试；`node --file pkg/util/slice/slice.rs --offset 1 --limit 240` 展示完整 81 行和 7 个符号；`query` 分别确认 `AllOf`、`Int64sToStrings`、`DeepClone`、`DeepCloneItem` 的定义。`callers AllOf`/聚合 callers、callees 查询在本次环境中未在 30 秒内返回，因此未据此虚构精确 Rust 调用边，改用索引文件关联结果与下列直接源码搜索交叉核验。
- 实现与装配：`pkg/util/slice/slice.rs`、`pkg/util/slice/lib.rs`、`pkg/util/slice/Cargo.toml`、根 `Cargo.toml` 的 `facade_util_slice` 路径依赖、`pkg/lib.rs::util::slice`。
- Rust 测试：`pkg/util/slice/slice_test.rs` 验证空切片全称真、非 `Clone` 元素、整数边界、自定义深拷贝和 nil/空区别；`pkg/util/slice/migration_aster_unit_test.rs` 验证短路访问序列、十进制极值和标准 `Clone` 类型的独立堆对象；`pkg/lib_test.rs` 验证根 facade 接线。`pkg/util/slice/main_test.rs` 说明该 crate 没有额外后台资源生命周期。
- Go 对照：`pkg/util/slice/slice.go`、`pkg/util/slice/slice_test.go`、`pkg/util/slice/main_test.go`。生产用途通过 `pkg/ddl/partition.go`、`pkg/statistics/handle/bootstrap.go`、`pkg/util/workloadrepo/table.go`、`pkg/planner/util/path.go` 和 `pkg/planner/core/operator/physicalop/*.go` 的直接搜索核验。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定十一个二级标题的结构命令以及人工事实复核作为验收证据。
