# `pkg/util/disjointset/set.rs`

## 文件定位

本文件实现 `astersql-util-disjointset` crate 的通用、稀疏并查集 `Set<T>`。它面向任意满足 `Eq + Hash + Clone` 的键：先把原值映射成连续的内部 `usize` 下标，再在 `parent` 数组上执行查找、路径压缩和合并。连续整数且元素全集已知时，应优先使用同 crate 的 `SimpleIntSet`，以省去两张 `HashMap` 的开销（`set.rs:27-44`、`int_set.rs`）。

crate 入口 `pkg/util/disjointset/lib.rs` 将本模块私有挂载为 `mod set`，再用 `pub use set::*` 导出 `Set` 和 `NewSet`。工作区根通过 `facade_util_disjointset` 依赖并在 `pkg/lib.rs` 的 `util::disjointset` 门面重导出；`pkg/util/chunk/Cargo.toml` 也以 `disjointset-crate` 直接依赖该 crate。

当前可见的生产调用点是 `pkg/util/chunk/chunk_util.rs:382-419` 的 `ColumnSwapHelper::mergeInputIdxToOutputIdxes`：它把引用同一底层列的输入列下标合并为一组，再按代表列汇总输出列映射。本文件本身不属于 SQL 请求主链，只是该列引用归并流程使用的底层数据结构。

## 核心职责

- 通过 `NewSet(size)` 创建仅预留容量、尚未含元素的通用并查集。
- 在第一次查询或合并某个值时，由 `findRootOriginalVal` 惰性注册该值，并建立值与内部下标的双向映射。
- 由 `findRootInternal` 沿父指针寻找根并迭代执行路径压缩，避免深链查找使用递归栈。
- 由 `Union(a, b)` 合并两个连通分量，并保证合并后采用 `a` 所在分量的根作为代表根。
- 由 `InSameGroup` 查询连通性，由 `FindRoot` 暴露内部根下标，由 `FindVal` 将任意有效内部下标解析为当前根代表的原值。

它不负责删除元素、拆分集合、按秩/大小合并、枚举分组、持久化或跨线程同步；这些能力不能从现有 API 推断为已支持。

## 主要符号

- `pub struct Set<T>`（`set.rs:33-45`）：通用并查集容器。泛型约束为 `T: Eq + Hash + Clone`，比 Go `comparable` 多出 `Clone`，用于同时保存正向和反向映射以及返回拥有所有权的代表值。
- `parent: Vec<usize>`：内部下标到父下标的数组；仅对同模块可见，测试模块借助父模块作用域检查长度。每个已注册值恰好对应一个有效位置。
- `val2Idx: HashMap<T, usize>`：原值到首次分配下标的稳定映射。
- `idx2Val: HashMap<usize, T>`：内部下标到原值的反向映射；根下标对应的原值就是该组当前代表值。
- `tailIdx: usize`：下一个分配下标，同时等于已注册元素数和 `parent.len()`。
- `pub fn NewSet<T>(size: usize) -> Set<T>`（`set.rs:49-59`）：以 `size` 作为三个容器的容量提示；不预先注册元素。
- `fn findRootOriginalVal(&mut self, a: T) -> usize`（`set.rs:66-77`）：查已注册值的根；未注册时同步写入三个存储并返回新下标。
- `fn findRootInternal(&mut self, a: usize) -> usize`（`set.rs:81-96`）：先迭代找到自指父节点，再把查找路径上的节点直接改挂到根。
- `pub fn InSameGroup(&mut self, a: T, b: T) -> bool`（`set.rs:100-102`）：比较两个值的根；查询未知值会产生注册副作用。
- `pub fn Union(&mut self, a: T, b: T)`（`set.rs:106-114`）：把 `b` 的根挂到 `a` 的根；同组时不写父指针。
- `pub fn FindRoot(&mut self, a: T) -> usize`（`set.rs:118-122`）：返回内部根下标；未知值会先成为单元素集合。
- `pub fn FindVal(&mut self, idx: usize) -> (T, bool)`（`set.rs:126-131`）：压缩 `idx` 的路径并返回根代表值。对有效下标第二项恒为 `true`。

文件没有模块级常量、trait、枚举、条件编译项或自定义错误类型。

## 执行流程

1. `NewSet(size)` 创建空 `parent`、`val2Idx`、`idx2Val`，三者仅预留容量，并把 `tailIdx` 置零。
2. 公共查询或合并把原值交给 `findRootOriginalVal`。若值不存在，则用当前 `tailIdx` 分配下标 `idx`，依次追加 `parent[idx] = idx`、记录双向映射并递增 `tailIdx`；新值因此自成一组。
3. 已存在的值进入 `findRootInternal(idx)`。第一轮沿 `parent` 前进，直到找到满足 `parent[root] == root` 的根；第二轮重新从 `idx` 出发，把路径上每个父指针都改为 `root`。
4. `Union(a, b)` 分别完成上述查根过程。若根不同，只执行 `parent[root_b] = root_a`，所以新的代表值来自 `a` 原先所在分量；未使用按秩或按大小启发式。
5. `InSameGroup(a, b)` 比较两个根。因此即使返回 `false`，此前未知的 `a`、`b` 也已经写入集合。
6. `FindRoot(a)` 直接返回根的内部下标。`FindVal(idx)` 再从任意有效下标查到当前根，并通过 `idx2Val[root]` 克隆代表原值。

在 `ColumnSwapHelper::mergeInputIdxToOutputIdxes` 中，流程是：比较输入列是否共享引用，调用 `Union(i, j)` 合并同源列；随后对每个待映射输入列调用 `FindRoot` 和 `FindVal`，把多个输入列的输出目标归入代表原值对应的同一桶（`pkg/util/chunk/chunk_util.rs:387-412`）。

## 数据与状态

核心不变量来自 `findRootOriginalVal` 的同步写入：对每个已注册值，`val2Idx[value] = idx`、`idx2Val[idx] = value`、`idx < parent.len()`；同时 `tailIdx == parent.len() == val2Idx.len() == idx2Val.len()`。每个父指针必须落在 `0..parent.len()` 内，且每棵树最终到达一个自指根。

下标按值的首次访问顺序从零单调分配，之后不会复用。`Union` 只改根节点的父指针，不改双向映射，因此所有值的原始下标稳定；但某个下标所解析到的代表值可随后续合并改变。特别地，`Union(a, b)` 保留 `a` 侧根，这一方向性被 `set_test.rs:66-72` 和 `migration_aster_unit_test.rs:58-65` 验证。

路径压缩会在只读语义的查询中修改 `parent`，所以 `InSameGroup`、`FindRoot`、`FindVal` 都要求 `&mut self`。空间随不同值数量线性增长，集合没有清空或删除 API。源码注释把合并复杂度描述为接近常数的反阿克曼复杂度；实现确定具备路径压缩，但没有按秩/大小合并，性能判断应以当前实现和实际负载为准，不应把注释扩展成更强保证。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::HashMap` 和 `std::hash::Hash`；`pkg/util/disjointset/Cargo.toml` 没有第三方依赖或 feature 声明，crate 使用工作区统一的版本、edition 和发布设置，并以 `lib.rs` 为库入口。

内部调用边经 RustCodeGraph 和源码核对如下：

- `InSameGroup` → `findRootOriginalVal`（两次）。
- `Union` → `findRootOriginalVal`（两次）。
- `FindRoot` → `findRootOriginalVal`。
- `FindVal` → `findRootInternal`。
- `findRootOriginalVal` → `findRootInternal`（仅已注册分支）。

生产上游为 `ColumnSwapHelper::mergeInputIdxToOutputIdxes`：`NewSet::<usize>` → `Union` → `FindRoot` → `FindVal`。公共 API 还由 `pkg/util/disjointset/set_test.rs` 和 `migration_aster_unit_test.rs` 直接覆盖。RustCodeGraph 对常见泛型方法名的反向解析存在歧义，因此生产上游又用限定 Rust 搜索核验；未发现本 crate 之外的其他 Rust 生产调用点。

## 错误处理与边界

API 不返回 `Result`，正常路径没有显式错误。未知原值不是错误，而是被惰性注册；这意味着仅想“检查但不插入”的调用方不能使用 `InSameGroup` 或 `FindRoot` 而避免状态变化。

`FindVal(idx)` 的 `idx` 必须来自同一个 `Set` 的 `FindRoot`、首次注册下标或其他已验证有效下标。若 `idx >= parent.len()`，`findRootInternal` 的数组索引会 panic；若内部不变量被破坏，父指针索引或 `idx2Val[&root]` 也会 panic。当前第二返回值不能表达非法下标，因为在执行到返回前非法下标已经 panic，而合法下标又必有反向映射并恒返回 `true`。

`NewSet(size)` 把外部值直接作为容量提示传给 `Vec`/`HashMap`；极大容量可能导致分配失败。`tailIdx += 1` 也没有显式溢出处理，但在可实际分配的元素数量范围内通常先受地址空间限制。哈希冲突由标准 `HashMap` 处理，不影响相等性语义，但键在存入后必须继续遵守 `Eq`/`Hash` 一致性。

## 并发与资源生命周期

`Set` 不含锁、原子、任务、通道、事务或外部句柄。所有会查根的 API 都获取 `&mut self`，同一实例的正常 Rust 调用因此被独占借用；需要跨线程共享时，调用方必须自行提供同步与所有权管理，且 `T` 是否满足 `Send`/`Sync` 会继续限制容器的可传递性。

资源完全由 `Vec` 和两张 `HashMap` 拥有，`Set` 离开作用域时由 Rust 自动释放。惰性注册只增长容器；路径压缩原地更新父指针，不额外分配。`FindVal` 会克隆根代表值，克隆成本由 `T` 决定。该结构不持有 `ColumnSwapHelper` 的原子指针；调用方在完成局部归并后才把结果发布，二者生命周期彼此独立（`chunk_util.rs:403-418`）。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `pkg/util/disjointset/set.go`：`Set` 的四个字段、惰性下标分配、双向映射、`InSameGroup`/`Union`/`FindRoot`/`FindVal` 的语义和“`b` 根挂到 `a` 根”的方向均保持一致。Rust 用 `usize` 对应 Go `int`，构造函数返回拥有值的 `Set<T>` 而非指针；调用方通过可变绑定获得与 Go 指针接收器相同的原地修改效果。

主要实现差异有三点：

- Go 泛型只要求 `comparable`；Rust 要求 `Eq + Hash + Clone`，因为哈希键和拥有权模型需要这些约束。
- Go `findRootInternal` 递归压缩路径；Rust 使用两段迭代循环，保持结果一致，并由 `migration_aster_unit_test.rs:82-94` 的十万节点深链用例验证不会因递归栈溢出失败。
- Go `FindVal` 用 map 查询返回真实的 `ok`，内部下标不存在时可返回零值和 `false`；Rust 在查 map 前先索引 `parent[idx]`，非法下标会 panic，而有效下标返回 `(value, true)`。因此 Rust 的布尔值目前仅为 API 对齐保留，不能视为越界保护。

Go `set_test.go` 覆盖连通性与组间合并；Rust `set_test.rs` 保留这些断言，并额外验证根不变性和 `FindVal`。Go 的 `main_test.go` 只建立通用测试环境和 goroutine 泄漏检查，没有对应本数据结构的并发行为。

## 扩展指南

- 增加不产生注册副作用的查询时，应在 `val2Idx` 层先区分缺失值，不要复用当前会插入的 `findRootOriginalVal`；同步在独立的 `set_test.rs` 增加“查询后长度不变”测试。
- 若要让 `FindVal` 安全处理任意下标，应在调用 `findRootInternal` 前检查 `idx < parent.len()`，并保留 Go 的 `(value, ok)` 契约；测试必须覆盖等于长度和远大于长度的输入。不要把测试内嵌到 `set.rs`。
- 若引入按秩/大小合并，需要新增并维护元数据，同时明确是否仍保证 `Union(a, b)` 选择 `a` 侧代表值。生产调用 `ColumnSwapHelper` 依赖 `FindVal` 得到组代表，改变代表选择可能影响汇总键和迭代顺序。
- 若增加删除、清空或下标复用，必须同步维护 `parent`、`val2Idx`、`idx2Val`、`tailIdx` 四者不变量，并考虑已向调用方暴露的旧下标是否失效。
- 性能改动应同时评估哈希/克隆成本、深链行为和路径压缩效果；至少更新 `set_test.rs` 与 `migration_aster_unit_test.rs`，并核对 Go `set.go`/`set_test.go` 是否需要保持同样语义。
- 源码修改完成后应按仓库协议保留 PingCAP 版权并保留顶部 AsterSQL 标记，运行 `cargo fmt --all`；本说明任务不修改源码，也不运行 Cargo。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `pkg/util/disjointset` 的 11 个文件；`files --filter pkg/util/disjointset` 确认 Rust/Go 实现与测试；`node --file pkg/util/disjointset/set.rs` 读取全部 132 行并识别 9 个符号；对 `NewSet`、`InSameGroup`、`Union`、`FindRoot`、`FindVal`、两个内部查根函数查询 callers/callees，确认本文件内部调用链。泛型方法反向边存在名称歧义，故未把歧义输出当作生产调用证据。
- 源文件与入口：`pkg/util/disjointset/set.rs`、`pkg/util/disjointset/lib.rs`、`pkg/util/disjointset/Cargo.toml`、工作区 `Cargo.toml:1079`、`pkg/lib.rs:1850-1851`、`pkg/util/chunk/Cargo.toml:19`。
- 生产调用：RustCodeGraph `node --file pkg/util/chunk/chunk_util.rs --offset 340 --limit 95`，重点为 `ColumnSwapHelper::mergeInputIdxToOutputIdxes` 的 `NewSet`、`Union`、`FindRoot`、`FindVal` 链；限定 `*.rs` 搜索用于补足图未可靠解析的泛型反向调用。
- Go 对照：`pkg/util/disjointset/set.go`、`set_test.go`、`main_test.go`，以及同一生产逻辑 `pkg/util/chunk/chunk_util.go:373-393`。
- Rust 测试：`pkg/util/disjointset/set_test.rs`；RustCodeGraph 读取 `migration_aster_unit_test.rs:48-94`，覆盖惰性注册、代表值和十万元素深链。测试均在独立文件，由 `lib.rs` 的 `#[cfg(test)]` 模块装配。
- 未运行 Cargo 或运行时测试：任务是纯文档分析，计划明确禁止 Cargo；本次以源码、调用图、Go 对照、既有测试和文档结构检查作为验证证据。
