# `pkg/util/disjointset/int_set.rs`

## 文件定位

本文件属于独立 crate `astersql-util-disjointset`，crate 边界由 `pkg/util/disjointset/Cargo.toml` 定义，入口 `pkg/util/disjointset/lib.rs` 以 `mod int_set` 加载本文件并用 `pub use int_set::*` 公开其 API。根 facade 又在 `pkg/lib.rs` 的 `util::disjointset` 中再导出该 crate；`pkg/util/chunk/internal/group1/lib.rs` 也提供一层同名再导出。因此这里是“连续整数”并查集的可复用实现，而不是 SQL 请求链上的独立服务或有状态组件。

仓库限定搜索目前没有找到非测试 Rust 代码直接构造这里的 `SimpleIntSet`；可见的 Rust 使用是 `pkg/lib_test.rs::independent_util_modules_are_wired` 以及本 crate 的独立测试。尤其是 `pkg/expression/constant_propagation.rs` 当前定义了自己的私有 `SimpleIntSet`，不能把它的运行时调用误认为本文件的调用。Go 对照实现则已用于表达式常量传播与 planner 投影索引修正，详见后文。

## 核心职责

- `SimpleIntSet` 用长度固定或阶段性重置的 `Vec<usize>` 表示元素 `0..len` 的等价类；元素值同时就是数组下标，适合稠密、连续的非负整数。
- `NewIntSet` 建立每个元素各自为根的森林；`Union` 合并两棵树；`FindRoot` 查找代表元并压缩访问路径。
- `Clear` 清空逻辑元素，`GrowNewIntSet` 将对象重置为恰好 `n` 个彼此独立的元素，以便复用已有分配。
- 本实现不保存集合数量、秩或树大小，也不支持稀疏键。非连续或非整数元素应使用同 crate 的 `Set`（`pkg/util/disjointset/set.rs`）。

## 主要符号

- `pub struct SimpleIntSet { pub(super) parent: Vec<usize> }`：公开类型，但父数组只对父模块可见。核心不变量是每个有效 `i` 都满足 `parent[i] < parent.len()`，根节点满足 `parent[root] == root`。
- `pub fn NewIntSet(size: usize) -> SimpleIntSet`：以 `(0..size).collect()` 构造身份映射。与 Go 返回指针不同，Rust 返回拥有所有权的值；调用者需要用可变绑定执行后续操作。
- `pub fn SimpleIntSet::Union(&mut self, a: usize, b: usize)`：分别调用 `FindRoot(a)`、`FindRoot(b)`，随后令 `parent[root_a] = root_b`。因此若两者原本不同，`b` 所在树的根成为合并后代表元；同组或自合并是幂等写入。
- `pub fn SimpleIntSet::FindRoot(&mut self, a: usize) -> usize`：先迭代追踪父指针找到根，再迭代回走原路径，将沿途父指针直接改为根。它需要 `&mut self`，因为查询会做路径压缩。
- `pub fn SimpleIntSet::Clear(&mut self)`：调用 `Vec::clear` 将长度置零。它删除逻辑元素但通常保留已分配容量，不保证归还堆内存。
- `pub fn SimpleIntSet::GrowNewIntSet(&mut self, n: usize)`：清空、`reserve(n)`，再写入 `0..n`。虽然沿用 Go 名称与注释中的 “grow”，实际语义是重置为**恰好** `n` 个独立元素；原连通关系全部丢失，也允许从更大长度重置到更小长度。

本文件没有 trait、枚举、模块级常量、条件编译项或错误类型。`#![allow(non_snake_case)]` 用于保留 Go 风格公开名称。

## 执行流程

1. 调用 `NewIntSet(size)` 后，`parent` 为 `[0, 1, ..., size - 1]`，每个元素单独成组。
2. `Union(a, b)` 先通过两次 `FindRoot` 获得真实根，避免把非根节点直接连接而破坏集合语义；然后把 `a` 的根挂到 `b` 的根。
3. `FindRoot(a)` 的第一段循环沿 `parent` 链找到自指根；第二段循环从 `a` 沿原链前进，把每个经过节点改为直接指向根，最后返回根下标。
4. 重复查询同一路径时，压缩后的节点通常一步即可到根。实现没有按秩/大小合并，所以合并顺序仍会影响压缩发生前的树高；源码注释将典型并查集操作描述为接近常数的逆 Ackermann 级别，但该文件实际只实现路径压缩。
5. 需要复用对象时可先 `Clear`，此时不得再查询旧下标；随后 `GrowNewIntSet(n)` 重建身份映射，才重新拥有 `0..n` 的有效元素。

`pkg/util/disjointset/int_set_test.rs::test_int_disjoint_set` 以 10 个元素形成 `{0,1,3,5}`、`{2,4,6,9}`、`{7,8}` 三个连通分量，并验证同组合并、自合并、清空和重建。`migration_aster_unit_test.rs::simple_int_set_compresses_a_deep_go_style_chain_without_stack_overflow` 还验证十万元素链使用迭代查找不会因递归栈溢出。

## 数据与状态

全部持久状态只有 `parent: Vec<usize>`。该向量同时编码元素域、树边和根：有效元素域就是当前 `0..parent.len()`，没有额外容量字段参与语义。`Vec` 的 capacity 仅是分配优化；`Clear` 后长度为零而 capacity 可能保留，`GrowNewIntSet` 可复用或扩充分配。

`Union` 和 `FindRoot` 都可能修改父数组；`NewIntSet`、`Clear`、`GrowNewIntSet` 则建立或销毁整个有效元素域。调用者不能持有内部父指针的公开引用，因为字段为 `pub(super)`，跨 crate 只能通过方法维护不变量。类型未自定义 `Clone`、`Default`、序列化或内存计量能力。

## 依赖与调用关系

下游依赖仅为 Rust 标准库的 `Vec`、区间迭代器和集合构造，没有第三方依赖；`pkg/util/disjointset/Cargo.toml` 也未声明 `[dependencies]` 或 feature。RustCodeGraph 对本文件确认的关键内部调用边是 `Union → FindRoot`，其余方法没有项目内下游调用。

上游接线分三层：`pkg/util/disjointset/lib.rs` 公开本文件全部公开项；工作区根 `Cargo.toml` 登记成员并以 `facade_util_disjointset` 引入；`pkg/lib.rs` 将其暴露为 `crate::util::disjointset::*`。此外 `pkg/util/chunk/Cargo.toml` 以 `disjointset-crate` 依赖它，`pkg/util/chunk/internal/group1/lib.rs` 再导出整个 crate。限定 `.rs` 搜索未发现这些 facade 之外的生产 Rust 调用，只有 `pkg/lib_test.rs` 和本 crate 测试使用 `NewIntSet`。

Go 生产调用提供该抽象在完整 TiDB 架构中的意图证据：`pkg/expression/constant_propagation.go::basePropConstSolver.unionSet` 用它维护列相等关系，并在 `propagateColumnEQ` 中按条件列数重建；`pkg/planner/core/resolve_indices.go::refine4NeighbourProj` 用它把引用同一输入列的多个投影输出下标归为一组。当前 Rust 的 `pkg/expression/constant_propagation.rs` 使用私有同类结构，planner 对应 Rust 接线未由本文件调用图确认。

## 错误处理与边界

API 不返回 `Result` 或 `Option`，也不主动校验输入。`a` 或 `b` 若不在 `0..parent.len()`，`FindRoot` 的数组索引会 panic；因此 `NewIntSet(0)` 或 `Clear()` 后可以安全持有空对象，但在重建前不能查找或合并任何元素。

只要 `parent` 不变量由本文件的方法维护，寻根循环会终止。跨 crate 调用者不能直接篡改数组；父模块与本文件测试因 `pub(super)` 可访问它，若未来在同一父模块新增代码直接写入越界父下标或环，可能分别触发 panic 或无限循环。`usize` 还意味着负整数在类型层面不可表示；极大 `size` 或 `n` 的分配失败沿用 `Vec` 的标准失败行为，而非转化为本 crate 错误。

`GrowNewIntSet` 不是保留原集合的增量扩容，误用会静默清除全部连通关系。`Clear` 也不是安全擦除或强制释放容量。代表元只保证组内一致，不保证最小、最大或最早元素；由于 `Union` 固定把左根挂到右根，改变参数顺序可能改变代表元，但不改变连通性。

## 并发与资源生命周期

该类型没有锁、原子变量、任务、通道、文件句柄或事务。修改操作要求独占的 `&mut self`，Rust 借用规则会阻止同一实例在安全代码中被多个线程无同步地并发修改；若调用者需要共享，应在外层选择合适的互斥机制。

资源生命周期完全由 `Vec` 所有权管理：构造时分配或创建空向量，路径压缩与合并原地修改；`Clear` 保留可复用容量，`GrowNewIntSet` 可能扩容；`SimpleIntSet` 离开作用域时由 `Vec` 自动释放内存。深链测试表明寻根是迭代实现，不随树深消耗调用栈，但仍会在压缩前线性遍历该条路径。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/disjointset/int_set.go`。两版具有同名的 `SimpleIntSet`、`NewIntSet`、`Union`、`FindRoot`、`Clear`、`GrowNewIntSet`，父数组初始化、左根挂右根、清空和重建语义一致。`pkg/util/disjointset/int_set_test.go::TestIntDisjointSet` 的主要合并序列与 Rust 的 `int_set_test.rs::test_int_disjoint_set` 一致。

实现差异如下：Go 元素和父下标类型为 `int`，构造函数返回 `*SimpleIntSet`；Rust 使用 `usize` 并按值返回。Go `FindRoot` 递归压缩路径，Rust 分成寻根与回写两个迭代循环，避免深链递归栈风险。Go `GrowNewIntSet` 使用 `slices.Grow` 保证容量后追加身份映射，Rust 用 `reserve` 后 `extend(0..n)`；两者都保留可复用容量且最终长度为 `n`。Rust 独立测试比 Go 同名测试额外覆盖同组/自合并、`Clear`/`GrowNewIntSet`，迁移补充测试还覆盖十万级深链。

Go 当前生产接线不等同于 Rust 当前生产接线：Go 的表达式常量传播直接依赖本包，Rust 对应文件保留了一份私有实现。若要消除重复，必须单独评估 crate 依赖方向、可见性与测试，而不能仅凭接口相似就宣称迁移完成。

## 扩展指南

- 若增加只读观察 API（例如元素数），优先从 `parent.len()` 推导，避免新增可漂移状态；若方法需要压缩路径，应继续使用 `&mut self` 并明确副作用。
- 若增加“保留现有关系并扩容”，不要改变 `GrowNewIntSet` 的重置语义；应新增明确命名的方法，并覆盖扩容前后旧根保持、新元素自成一组、缩容策略和零长度边界。
- 若引入按秩/大小合并，需要新增对应向量并同步维护 `NewIntSet`、`Clear`、重建和所有合并路径；代表元选择可能变化，调用者只能依赖同组一致性，测试也不应固定未承诺的根值。
- 若需要可恢复的越界错误，需设计新的 checked API；直接改变现有方法的 panic 行为或签名会影响 facade 兼容性。性能改动应继续覆盖长链、重复查找和重复合并。
- Rust 测试逻辑必须放在独立文件。主要同步位置是 `pkg/util/disjointset/int_set_test.rs`；Go 语义变化还应核对 `int_set.go` 与 `int_set_test.go`，迁移/压力边界核对 `migration_aster_unit_test.rs`，根 facade 可访问性核对 `pkg/lib_test.rs`。
- 若让 `pkg/expression/constant_propagation.rs` 改用本 crate，应先确认依赖图不会形成环，并验证常量传播的完整等价类行为；该接线属于调用方任务，不应通过在本文件中复制业务逻辑完成。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件；`files --filter pkg/util/disjointset` 列出本 crate 的 Rust/Go 源与独立测试。
- RustCodeGraph `node --file pkg/util/disjointset/int_set.rs --offset 1 --limit 260`：读取本文件 83 行全貌，确认 1 个结构体、1 个自由函数、4 个方法及无条件编译项；调用图确认 `Union` 调用 `FindRoot`。
- RustCodeGraph 精确 `query`：确认 Rust/Go 两侧 `SimpleIntSet`、`NewIntSet`、`GrowNewIntSet` 定义。对 `Union`、`FindRoot` 等常见名称的 callers/callees 消歧会混入无关符号，精确目标 callers 为空，因此未据此声称存在生产 Rust 调用。
- 读取的边界与接线路径：`pkg/util/disjointset/Cargo.toml`、`pkg/util/disjointset/lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/util/chunk/Cargo.toml`、`pkg/util/chunk/internal/group1/lib.rs`、`pkg/lib_test.rs`。
- 读取的 Go 对照与生产证据：`pkg/util/disjointset/int_set.go`、`pkg/expression/constant_propagation.go`、`pkg/planner/core/resolve_indices.go`；读取的 Rust现状对照：`pkg/expression/constant_propagation.rs`。
- 读取的测试证据：`pkg/util/disjointset/int_set_test.rs`、`pkg/util/disjointset/int_set_test.go`、`pkg/util/disjointset/migration_aster_unit_test.rs`。
- 限定路径 `rg` 核对了 `NewIntSet`、`SimpleIntSet`、`GrowNewIntSet`、crate 别名及 facade 的引用；结论仅覆盖当前检出的仓库状态。按任务约束未运行 Cargo 或代码测试，最终以固定十一章节结构检查和人工事实复核验收。
