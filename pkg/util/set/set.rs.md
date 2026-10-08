# `pkg/util/set/set.rs`

## 文件定位

本文件是 `astersql-util-set` crate 中的通用泛型集合实现，源码由 [`pkg/util/set/lib.rs`](./lib.rs) 以 `pub mod set` 挂载并通过 `pub use set::*` 重导出。它与 `int_set.rs`、`float64_set.rs`、`string_set.rs` 等面向具体标量类型的集合并列，负责需要由调用者自定义成员身份的场景：元素实现 `Key`，集合以 `Key::Key()` 返回的字符串作为唯一标识。

crate 边界由 [`pkg/util/set/Cargo.toml`](./Cargo.toml) 定义，包名为 `astersql-util-set`，迁移元数据指向 Go 包 `pkg/util/set`。本文件自身只使用标准库 `std::collections::HashMap`，不直接使用该 manifest 中的 `hack-crate`、`memory-crate` 或 `types-crate`；这些依赖服务于同 crate 的其他集合实现。

RustCodeGraph 能定位本文件 26 个符号，并确认 `CombSet` 到 `combSetIterate` 的内部调用边。仓库文本检索显示，当前泛型 API 的实际 Rust 调用集中在同 crate 的 `set_test.rs` 与 `migration_aster_unit_test.rs`；若干生产 crate 声明了对 `astersql-util-set` 的依赖，但可见调用主要使用该 crate 的其他专用集合类型。因此不能仅凭 Cargo 依赖断言本文件已经进入某条 SQL 执行主链。

## 核心职责

- 用 `Key` trait 把任意可克隆元素映射到稳定字符串键，并以键而不是完整值判断成员相等性。
- 用 `Set<T>` trait 暴露批量添加、查询、删除、有序列表、大小、克隆和稳定字符串化能力。
- 用 `setImpl<T>` 提供默认实现，并通过 `Option<HashMap<String, T>>` 保留 Go 版本“首次写入时才创建 map”的行为。
- 提供列表转集合以及并集、交集、差集运算；所有结果都是新集合，不把输入集合本身作为结果返回。
- 用 `CombSet`/`combSetIterate` 按稳定输入顺序回溯枚举固定大小组合。

集合身份只由字符串键决定。两个不同值若返回同一个键，后添加的值会覆盖先添加的值；`migration_aster_unit_test.rs::generic_set_crud_sorting_clone_and_key_replacement_match_go` 明确验证了这一不变量。

## 主要符号

- `pub trait Key { fn Key(&self) -> String; }`：元素身份协议。每次查询、插入、删除或排序都可能调用它，调用者应保证同一元素的键稳定。
- `pub trait Set<T: Key + Clone>`：对象安全的集合接口。`Add` 接收切片以对应 Go 变参；`Contains`/`Remove` 接收元素引用；`Clone` 返回 `Box<dyn Set<T>>`。
- `struct setImpl<T>`：私有默认实现，唯一字段 `s: Option<HashMap<String, T>>` 保存键和值。
- `NewSet<T>()`：构造内部 map 尚未分配的空集合，返回 trait object；`T: 'static` 来自装箱动态对象的生命周期要求。
- `ListToSet<T>(items)`：逐项调用 `Add` 构造集合。
- `UnionSet<T>(ss)`：合并所有输入；零输入返回空集合，单输入返回克隆，多输入按输入顺序写入，因此同键冲突时后面的集合获胜。
- `AndSet<T>(ss)`：遍历第一个集合的元素，保留被其余所有集合包含的成员；零输入为空，单输入为克隆。
- `DiffSet<T>(s1, s2)`：保留 `s1` 中键不在 `s2` 的元素。
- `CombSet<T>(s, numberOfItems)`：把集合先转为稳定列表，再建立可变工作集并进入递归。
- `combSetIterate<T>(itemList, currSet, depth, numberOfItems)`：私有回溯助手；选择当前元素、递归、删除恢复状态，再递归不选择分支。

这些 API 沿用 Go 风格的导出名，crate 根的 `#![allow(non_snake_case, ...)]` 允许这种命名，以便迁移代码和对照审阅。

## 执行流程

基础操作从 `NewSet` 开始。集合初始为 `s: None`；第一次 `Add` 通过 `get_or_insert_with(HashMap::new)` 分配存储，然后对每个元素计算键并克隆值写入。`Contains` 和 `Remove` 也只计算键；前者在未初始化时返回 `false`，后者在未初始化时不做任何事。`ToList` 克隆所有值并按 `Key()` 字典序排序，因此不会暴露 `HashMap` 的随机迭代顺序。`String` 独立收集并排序键，再生成 `{k1, k2}` 格式。

集合代数的流程如下：

1. `UnionSet` 对每个输入调用 `ToList`，再批量 `Add` 到结果；后遍历集合的同键值会覆盖先前值。
2. `AndSet` 只枚举第一个输入的有序成员，并逐一询问其余输入的 `Contains`；第一次不包含即早停。
3. `DiffSet` 枚举 `s1.ToList()`，仅在 `s2.Contains` 为假时加入结果。

组合枚举由 `CombSet` 调用 `s.ToList()` 固定元素顺序。`combSetIterate` 在工作集大小等于目标数时克隆快照并返回；到达列表末尾或工作集已超过目标数时返回空结果。普通节点依次执行“选择当前元素”和“不选择当前元素”两个分支，选择分支结束后必须 `Remove` 当前元素以恢复工作集。该顺序使测试中的组合排列稳定，例如四选二依次产生 `{q1, q2}`、`{q1, q3}`、`{q1, q4}`、`{q2, q3}`、`{q2, q4}`、`{q3, q4}`。

## 数据与状态

核心状态是 `Option<HashMap<String, T>>`：`None` 与已分配但为空的 map 对外都表现为大小 0、列表为空、字符串为 `{}`，但前者避免空集合立即分配。集合中每个键只保存一个 `T`，重复键插入会替换值而不增加 `Size`。

`Clone` 并不复制 trait object 的具体内存布局，而是调用 `ToList` 后把克隆出的元素加入新集合。结果拥有独立的 `HashMap`，之后对原集合做 `Remove` 不影响副本；这由 `set_test.rs::TestSetBasic` 和迁移回归测试共同验证。对 `T` 的复制深度取决于 `T::clone()`，本文件不承诺元素内部引用资源的深拷贝。

稳定性来自按键排序，而不是 `HashMap` 顺序。`ToList` 返回值的顺序、`String` 的文本顺序以及 `CombSet` 的组合顺序均以键的字典序为基础。若 `Key()` 随元素状态变化，map 中已存键与当前返回键可能不一致，查询、删除和排序语义会失真；因此“入集合期间键保持稳定”是调用者必须维护的不变量。

## 依赖与调用关系

直接下游依赖只有标准库 `HashMap` 和本文件自己的 trait 方法。RustCodeGraph 的精确图查询给出：

- `CombSet` 调用 `Set::ToList` 与 `combSetIterate`。
- `combSetIterate` 调用 `Set::Size`、`Set::Clone`、`Set::Add`、`Set::Remove`，并递归调用自身；索引会同时列出 trait 声明和 `setImpl` 实现对应的目标。
- `UnionSet`、`AndSet`、`DiffSet` 分别组合 `NewSet`、`ToList`、`Add`、`Contains` 与 `Clone` 完成运算，没有 I/O 或跨 crate 回调。

模块入口 `lib.rs` 把所有公开符号重导出到 `astersql_util_set` crate 根。`pkg/infoschema`、`pkg/executor`、`pkg/ddl`、`pkg/planner/indexadvisor` 等 manifest 声明该 crate 依赖，但对 Rust 源码的直接检索没有找到这些生产模块调用本文件泛型符号的有效代码；已确认的上游是 `set_test.rs` 与 `migration_aster_unit_test.rs`。这说明本文件目前更接近已移植并有回归覆盖的通用能力，而不是已经验证接入具体生产流程的组件。

## 错误处理与边界

所有 API 都返回普通值，不定义 `Result` 或自有错误类型；内存分配失败和用户 `Key`/`Clone` 实现中的 panic 仍按 Rust 运行时行为传播。

- 空集合：`Contains` 为 `false`，`Size` 为 0，`ToList` 为空，`String` 为 `{}`，`Remove` 是无操作。
- 空输入集合列表：`UnionSet([])` 与 `AndSet([])` 都返回新空集合。
- 单输入集合列表：并集和交集都返回独立克隆，不共享内部 map。
- 键冲突：`Add` 静默覆盖旧值；这是设计语义，不报告重复错误。
- `CombSet` 的目标数为 0 时立即返回一个空集合组合；目标数为负数或大于成员数时返回空 `Vec`。迁移回归测试覆盖了 0、-1 和超界三种情况，基础对照测试覆盖了 1 到成员数及成员数加一。
- 由于 Rust 使用引用调用接口，无法表示 Go 的 nil `*setImpl` 接收者。Go 的 `ToList`/`Size` 对 nil receiver 有显式分支，而 Rust 仅保留“内部 map 尚未初始化”的等价正常状态。

复杂度方面，基础哈希操作平均为 O(1)，但要加上生成字符串键的成本；`ToList` 和 `String` 为 O(n log n) 排序。并集还会反复排序每个输入的 `ToList`；交集约为 O(n₀ × (m-1)) 次成员查询并含首集合排序；组合枚举输出规模为 C(n, k)，递归探索最坏呈指数增长，且每个命中组合都会克隆当前集合。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务。可变操作要求 `&mut self`，Rust 借用规则阻止同一集合在安全代码中并发读写；trait 没有声明 `Send` 或 `Sync`，是否能跨线程还取决于具体 `T` 与 trait object 的边界，调用者不能假定它天然线程安全。

资源完全由所有权管理：`Box<dyn Set<T>>` 拥有具体集合，`HashMap` 和元素在集合释放时随之释放。组合递归借用同一个 `currSet`；每次选择分支后的 `Remove` 是状态回滚，不是外部资源清理。达到目标大小时先 `Clone` 快照，保证之后的回滚不会修改已收集结果。递归深度最多随输入成员数增长，超大集合可能带来栈深与指数级内存风险。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/set/set.go`](./set.go)，Rust 基本保持其控制流与边界：`Key`/`Set` 接口、`setImpl` 懒初始化、同键覆盖、有序 `ToList`/`String`、空与单输入的并交集分支、差集过滤，以及组合递归的 Add/Remove 回溯均一一对应。

语言映射上的差异是：

- Go `map[string]T` 的 nil 状态映射为 `Option<HashMap<String, T>>`；Go 接口返回值映射为 `Box<dyn Set<T>>`。
- Go 变参 `...T`/`...Set[T]` 映射为切片，调用点需要显式借用或构造单元素切片。
- Go `int` 大小映射为 `usize`（集合大小与递归深度）及 `isize`（允许组合目标为负数）。
- Go 的 nil slice 无结果映射为空 `Vec`；外部观察到的迭代结果相同，但 nil 与空的表示差异不再存在。
- Rust 需要 `T: Clone + 'static`；Go 版本没有等价的显式克隆约束，而是按值存入 map。
- Go 可以对 nil `*setImpl` 调用 `ToList`/`Size`，Rust trait object 不存在 nil receiver；正常构造的空集合行为保持一致。

[`pkg/util/set/set_test.go`](./set_test.go) 与 [`pkg/util/set/set_test.rs`](./set_test.rs) 使用相同的 q1–q4 案例验证 CRUD、克隆独立性、并交差和组合顺序。`migration_aster_unit_test.rs` 额外验证同键 payload 覆盖以及 `CombSet` 的 0、负数和超界边界。

## 扩展指南

新增通用集合操作时，优先在 `Set<T>` trait 判断它是否属于所有实现必须提供的能力；若只是由现有原语组合出的代数运算，可像 `UnionSet`/`DiffSet` 一样新增自由函数，避免扩大 trait 实现负担。改变 trait 方法会影响所有 `Set<T>` 实现与 trait object 调用点，应同步检查 `setImpl`、构造函数和独立测试。

若修改成员身份或覆盖策略，应从 `Key`、`setImpl::Add`、`Contains`、`Remove` 一起审视，并保持 Go `set.go` 的语义；尤其不能把完整值相等误当成成员相等。若改变输出顺序，需同时评估 `ToList`、`String` 和 `CombSet`，因为组合结果的确定性依赖前两者的排序约定。

若优化集合代数性能，可以避免不必要的 `ToList` 排序或选择较小集合驱动交集，但必须保留稳定输出、同键值来源和 Go 对照行为。若优化 `CombSet`，必须保持选择/不选择的输出顺序、命中时的独立快照以及 k=0/负数/超界边界。组合规模天然可能爆炸，新增面向不可信大小的调用时应在上层设置限制，而不是悄悄截断这里的结果。

测试逻辑必须继续放在独立文件中：常规对照用 `pkg/util/set/set_test.rs`，更全面的迁移边界可放在 `pkg/util/set/migration_aster_unit_test.rs`，并同步参考 Go 的 `pkg/util/set/set_test.go`。不要把 `#[cfg(test)]` 测试内嵌回本生产源文件。

## 验证依据

- 源码事实：`pkg/util/set/set.rs` 的 `Key`、`Set`、`setImpl`、`NewSet`、`ListToSet`、`UnionSet`、`AndSet`、`DiffSet`、`CombSet` 与 `combSetIterate`。
- crate 与模块边界：`pkg/util/set/Cargo.toml`、`pkg/util/set/lib.rs`；另以各消费方 `Cargo.toml` 核对 crate 依赖声明。
- Go 对照：`pkg/util/set/set.go`；对应测试为 `pkg/util/set/set_test.go`。
- Rust 测试：`pkg/util/set/set_test.rs` 验证基础行为和 k=1..5 的组合；`pkg/util/set/migration_aster_unit_test.rs` 验证键覆盖、稳定顺序、克隆独立性及 k=0、负数、超界。
- RustCodeGraph：`status` 显示索引包含本文件；`files --filter pkg/util/set` 确认模块文件集合；`node --file` 读取目标、Go 对照和测试；`query` 精确定位公开函数；`callees CombSet --file pkg/util/set/set.rs` 确认 `ToList` 与 `combSetIterate` 调用边；`callees combSetIterate --file ...` 确认 `Add`、`Remove`、`Size`、`Clone` 调用边。
- 上游核验：对 Rust 源码检索本文件公开符号，目前只发现同 crate 的两个测试文件有有效调用；因此文档没有声称已接入未被代码证实的生产主链。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行固定 11 章节结构验证并人工复核链接、符号和边界描述。
