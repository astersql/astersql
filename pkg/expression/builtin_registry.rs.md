# `pkg/expression/builtin_registry.rs`

## 文件定位

该文件属于 `astersql-expression` crate（边界见 `pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:259-260` 以私有模块名 `builtin_registry_kernel` 装配。它不是 SQL 内建函数的完整工厂实现，而是一个只依赖标准库 `HashMap` 的轻量注册表/快照工具：给定“函数名 → 任意值”的映射，生成稳定有序且独立拥有的名称列表，并提供最小的插入和查找封装。

模块没有在 `lib.rs` 中被 `pub use` 重新导出，因此其中的 `pub` 项目前仍受私有父模块限制。仓库搜索显示，生产 Rust 路径尚未使用 `BuiltinRegistry<V>`；自由函数 `registered_builtin_function_names` 的直接模块外使用位于独立测试 `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs`。因此本文件当前更准确的角色是已实现、已做局部语义回归，但尚未接入完整 Rust 函数工厂主链的基础组件。

## 核心职责

1. `registered_builtin_function_names` 从任意 `HashMap<String, V>` 克隆所有键并排序，返回不借用原映射的名称快照。
2. `BuiltinRegistry<V>` 将同样的 `HashMap<String, V>` 封装为泛型容器，集中表达注册、覆盖、按名读取和枚举名称四项操作。
3. 排序消除 `HashMap` 迭代顺序的不确定性，使输出适合快照比较、调试展示和跨语言一致性检查。

本文件不负责函数名规范化、别名解析、参数个数检查、函数实例构建、扩展函数合并或 SQL 可见函数过滤。这些完整注册语义目前在其他实现中：Rust 的工厂表主要位于 `pkg/expression/builtin.rs`，Go 的全局 `funcs` 与 `GetBuiltinList` 位于 `pkg/expression/builtin.go`。

## 主要符号

- `pub fn registered_builtin_function_names<V>(funcs: &HashMap<String, V>) -> Vec<String>`：模块级核心算法。泛型参数 `V` 没有 trait 约束，因为函数只读取键。它按 `funcs.len()` 预分配容量，克隆键，再调用 `sort_unstable`。
- `pub struct BuiltinRegistry<V>`：保存私有字段 `funcs: HashMap<String, V>`。派生 `Clone`、`Debug`、`Default`；相应能力仅在 `V` 满足派生宏生成的约束时可用。
- `BuiltinRegistry::insert(name, function) -> Option<V>`：接受任意 `Into<String>` 名称；同名插入会覆盖并返回旧值，不把重复注册当作错误。
- `BuiltinRegistry::registered_builtin_function_names(&self) -> Vec<String>`：把内部映射委托给同名模块级函数，维持单一排序实现。
- `BuiltinRegistry::get(&self, name) -> Option<&V>`：精确按字符串键查找，返回共享借用；缺失时返回 `None`。

文件没有模块级常量、trait、枚举、条件编译项或自定义错误类型。

## 执行流程

名称快照流程由 `registered_builtin_function_names` 完整定义：先用映射长度建立结果向量；随后遍历 `HashMap::keys`，对每个 `String` 执行克隆；最后用 `Vec::sort_unstable` 按 Rust 字符串的字典序升序排列并返回。因为输出拥有自己的 `String`，调用者之后修改结果不会改变注册表。

容器流程很薄：调用者先通过 `Default` 得到空注册表或通过克隆已有注册表得到副本；`insert` 将 `name` 转换为 `String` 后写入；`get` 以原样名称读取；枚举时，实例方法把 `&self.funcs` 交给模块级快照函数。源码没有自动小写化步骤，所以 `"ABS"` 与 `"abs"` 是两个不同键，调用方必须自行保证命名约定。

当前应用主链中没有找到 `BuiltinRegistry<V>` 的构造或调用边。`pkg/expression/lib.rs` 只完成模块装配；完整函数解析/构建仍由 `pkg/expression/builtin.rs` 的工厂结构承担。本文件不能单独证明某个 SQL 函数已可执行。

## 数据与状态

唯一持久状态是 `BuiltinRegistry<V>::funcs`。字段私有，外部只能经 `insert` 修改、经 `get` 读取、经快照方法枚举，因此不会直接取得底层映射的可变引用。

注册表不维护额外索引、版本号、容量策略或名称元数据。`insert` 遵循 `HashMap::insert` 的替换语义；注册表长度只在新键写入时增长。快照的空间复杂度为所有键副本加一个 `Vec`，时间复杂度主要是克隆键的线性成本和排序的 `O(n log n)` 成本。`sort_unstable` 不承诺相等元素的相对顺序，但映射键天然唯一，因此不会影响结果。

`Clone` 会复制整个映射及其值，`Default` 创建空映射，`Debug` 暴露字段内容用于调试；这些派生行为不增加全局状态。文件也没有静态变量或进程级单例。

## 依赖与调用关系

- 下游依赖仅为 `std::collections::HashMap` 及 `Vec`/`String` 的标准库操作；`pkg/expression/Cargo.toml` 没有为本文件引入专属外部 crate 或 feature。
- 模块入口为 `pkg/expression/lib.rs` 的 `#[path = "builtin_registry.rs"] mod builtin_registry_kernel;`，模块对 crate 外不可见。
- 实例方法 `BuiltinRegistry::registered_builtin_function_names` 调用同文件的自由函数；`insert` 和 `get` 分别下沉到 `HashMap::insert` 与 `HashMap::get`。
- `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs` 导入自由函数，并用两项映射验证排序、拥有权和源映射不变性。
- 对完整注册表的更广回归在 `pkg/expression/builtin_registry_aster_unit_test.rs`，但它使用的是 `crate::formal_registry::{funcs, GetBuiltinList, ...}`，并不调用本文件的 `BuiltinRegistry<V>`。这组测试证明完整迁移注册表的行为，不应被误记成本容器的直接调用边。

RustCodeGraph 能定位本文件的 `BuiltinRegistry` 与两个同名快照函数节点；对这些节点没有给出生产调用者。仓库级 `rg` 搜索进一步确认 `BuiltinRegistry` 类型目前只出现在定义处，自由函数只在上述测试中被模块外引用。

## 错误处理与边界

所有 API 都是非 `Result` 接口。缺失键由 `get` 返回 `None`；重复键由 `insert` 返回 `Some(old_value)` 并保留新值。调用者若要禁止覆盖，必须在本层之外先检查或把返回的旧值视为冲突。

空映射会产生空向量。键允许空字符串、大小写混合或任意合法 UTF-8，源码没有校验。查找为大小写敏感的精确匹配，排序使用 Rust `String`/`str` 的默认顺序，不应用 SQL collation、语言区域或 ASCII 大小写折叠。

快照不会过滤内部伪函数或未实现函数。这个边界区别于 Go `builtin.go::GetBuiltinList`：后者排除 `row`、`istrue_with_null` 和以 `"'tidb`.(` 开头的字面量函数，还合并扩展函数；本文件对应的不是该展示列表，而是 `builtin_registry.go::RegisteredBuiltinFunctionNames` 的原始全量键快照。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、任务、通道、事务、文件句柄或网络资源。注册表所有权由普通 Rust 值语义管理，离开作用域时映射和值按 RAII 释放。

共享读取是否跨线程安全由 `V` 以及调用方采用的共享方式决定；本类型自身不提供内部同步。修改需要 `&mut self`，因此安全 Rust 会在编译期阻止同一实例同时进行未同步的读写。返回的 `&V` 生命周期绑定到 `&self`，而名称快照与注册表生命周期解耦。若未来放入全局并发路径，应在外层选择 `Mutex`/`RwLock`/不可变初始化等策略，而不是假定当前类型已有 Go 全局 map 的并发生命周期。

## 与 Go 版本的对应关系

直接 Go 对照文件是 `pkg/expression/builtin_registry.go`。其 `RegisteredBuiltinFunctionNames()` 遍历包级 `funcs`、复制所有名称并调用 `slices.Sort`；Rust 自由函数保留了“全量键、排序、独立快照、不修改源表”四个语义，但将 Go 隐式访问的包级全局 map 显式改为 `&HashMap<String, V>` 参数。

存在三点迁移差异：第一，Rust 函数对值类型泛型化，不依赖 Go 的 `functionClass`；第二，Rust 文件额外提供 `BuiltinRegistry<V>` 容器，而 Go 对照文件没有同名容器；第三，Rust 容器尚未接到完整的 `formal_registry::funcs` 或 SQL 函数构建路径，因此不能替代 Go 包级 `funcs` 的现有作用。

`pkg/expression/builtin.go::GetBuiltinList` 是另一种面向用户展示的列表：它有过滤、扩展函数合并和排序逻辑。`pkg/expression/builtin_registry_aster_unit_test.rs::builtin_listing_and_operator_display_follow_go_filters` 针对的是这套完整列表语义，不是本文件的无过滤快照。Go 的 `pkg/planner/util/null_misc_test.go::TestNullRejectBuiltinRegistrySnapshot` 则调用 `RegisteredBuiltinFunctionNames` 并校验哈希与集合覆盖，说明全量快照的实际用途是检测函数注册集合漂移。

## 扩展指南

若只需新增一种存储值类型，无需修改本文件，直接实例化 `BuiltinRegistry<NewType>` 即可。若要增加删除、迭代或冲突拒绝能力，应在 `impl<V>` 中新增最小 API，并保持字段私有；重复注册策略发生变化时必须明确兼容影响，因为当前 `insert` 的返回旧值/覆盖新值语义是可观察行为。

若要把此容器接入生产函数注册主链，应先核对 `pkg/expression/builtin.rs` 中现有工厂注册、动态扩展和锁策略，避免建立第二份会漂移的注册源；同时决定名称是否统一小写、是否允许别名、是否需要扩展函数，以及快照究竟对应全量 `RegisteredBuiltinFunctionNames` 还是过滤后的 `GetBuiltinList`。这属于接线工作，当前文件没有实现。

测试必须放在独立 Rust 测试文件，不应内嵌到 `builtin_registry.rs`。直接语义应扩展 `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs` 中的快照测试，覆盖空表、重复覆盖、缺失查找和大小写精确匹配；若接入完整注册表，还应同步 `pkg/expression/builtin_registry_aster_unit_test.rs`、其聚合入口 `pkg/expression/builtin_test.rs`，并评估 Go 的 `pkg/planner/util/null_misc_test.go` 快照哈希和 `pkg/expression/function_traits_test.go` 列表约束。性能上需关注频繁全量克隆/排序；兼容性上需避免误把无过滤快照替换为用户展示列表。

## 验证依据

- 目标源码：`pkg/expression/builtin_registry.rs`，核对了自由函数、结构体、三个实例方法、派生能力以及无条件编译项的完整文件。
- crate 与模块边界：`pkg/expression/Cargo.toml`（crate 名、`lib.rs` 入口、无本文件专属依赖）和 `pkg/expression/lib.rs:259-260,409-410`（私有生产模块与独立测试模块装配）。
- Go 对照：`pkg/expression/builtin_registry.go::RegisteredBuiltinFunctionNames`；辅助区分 `pkg/expression/builtin.go::{funcs, GetBuiltinList}`。
- Rust 测试：`pkg/expression/builtin_regexp_util_23_aster_unit_test.rs::registry_snapshot_is_sorted_owned_and_does_not_mutate_source`；完整工厂注册回归参考 `pkg/expression/builtin_registry_aster_unit_test.rs` 与 `pkg/expression/builtin_test.rs`。
- Go 使用与测试：`pkg/planner/util/null_misc_test.go::TestNullRejectBuiltinRegistrySnapshot`、`pkg/expression/function_traits_test.go::TestIllegalFunctions4GeneratedColumns`。
- RustCodeGraph：`status` 显示索引包含 `pkg/expression/builtin_registry.rs`（6 个符号）；`query BuiltinRegistry --json` 定位结构体，`query registered_builtin_function_names --json` 定位自由函数和实例方法。图查询未返回生产调用边，随后用 `rg` 核对引用范围。
- 人工复核结论：该文件存在于“稳定枚举注册名称”的边界；运行时算法是克隆键后排序；安全扩展的关键是保持快照拥有权、区分全量与展示列表、并在接入生产主链时避免双注册源。
