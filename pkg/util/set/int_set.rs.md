# `pkg/util/set/int_set.rs`

## 文件定位

该文件属于 `astersql-util-set` crate；`pkg/util/set/Cargo.toml` 将 `lib.rs` 设为 crate 根，`pkg/util/set/lib.rs` 通过 `pub mod int_set` 装入本文件，并用 `pub use int_set::*` 在 crate 根公开其 API。它提供两种只保存整数键的基础集合：平台位宽的 `IntSet` 与固定 64 位的 `Int64Set`。RustCodeGraph 对目标文件报告 11 个符号，并显示该文件没有被其他文件直接引用；仓库内 Rust 引用搜索只找到同 crate 的独立测试。因此可以确认它是公开基础 API，但目前没有证据表明它已被生产 SQL、存储或 DDL 主链直接调用。

## 核心职责

- `IntSet` 用 `HashSet<isize>` 表示 Go `map[int]struct{}`：只记录成员是否存在，不附带值。
- `Int64Set` 用 `HashSet<i64>` 表示 Go `map[int64]struct{}`，避免平台位宽影响需要严格 64 位的标识或数值。
- `NewIntSet`、`NewInt64Set` 从切片批量初始化集合；`Insert`、`Exist`、`Count` 分别提供写入、查询和计数。
- 集合自动去重。`pkg/util/set/int_set_test.rs::TestIntSet`、`TestInt64Set` 和 `pkg/util/set/migration_aster_unit_test.rs::primitive_sets_match_go_membership_and_float_key_semantics` 都验证了重复插入不会增加计数。

本文件不实现删除、清空、迭代或集合代数；需要这些能力时不能假定当前类型已经支持，应在本文件及独立测试中显式扩展，或选用同 crate 的其他集合实现。

## 主要符号

- `pub struct IntSet { inner: HashSet<isize> }`：平台位宽整数集合。派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`；`inner` 私有，外部只能通过公开方法访问。
- `pub fn NewIntSet(is: &[isize]) -> IntSet`：按输入长度预分配，逐项调用 `IntSet::Insert`。空切片得到已初始化的空集合。
- `IntSet::Exist(&self, val: isize) -> bool`：委托 `HashSet::contains` 查询成员。
- `IntSet::Insert(&mut self, val: isize)`：委托 `HashSet::insert`；忽略其“是否新插入”的布尔返回值，因此 API 只表达确保成员存在。
- `IntSet::Count(&self) -> usize`：返回 `HashSet::len`。
- `pub struct Int64Set { inner: HashSet<i64> }`：固定 64 位整数集合，派生项和封装方式与 `IntSet` 相同。
- `pub fn NewInt64Set(xs: &[i64]) -> Int64Set`：按切片长度预分配并逐项插入。
- `Int64Set::{Exist, Insert, Count}`：分别对应 `contains`、`insert`、`len`，仅元素类型改为 `i64`。

文件没有模块级常量、trait、类型别名、错误类型或条件编译项。

## 执行流程

构造流程以 `NewIntSet` 为例：先用 `HashSet::with_capacity(is.len())` 创建底层集合，再按切片顺序遍历每个值并调用 `Insert`，最后返回拥有该集合的 `IntSet`。`NewInt64Set` 的流程完全相同。预分配容量只减少可能的扩容，不改变重复值去重或成员语义。

运行期操作没有额外控制层：`Insert` 取得 `&mut self` 后写入底层哈希表；重复键由 `HashSet` 合并；`Exist` 通过共享借用执行哈希查找；`Count` 读取当前长度。典型序列“构造 → 多次插入 → 计数/查询”由 `pkg/util/set/int_set_test.rs` 的两个测试覆盖。接口不承诺遍历顺序，且本文件没有暴露迭代器。

## 数据与状态

两种类型的全部可变状态都是私有字段 `inner`。成员值自身就是哈希键，不存在独立 value，也没有缓存、全局变量或派生计数；`Count` 始终直接读取底层长度。`Clone` 复制独立的 `HashSet`，`Eq`/`PartialEq` 按成员相等性比较，`Default` 产生空集合。

`isize` 的位宽随目标平台变化，对应 Go `int` 的平台相关意图；`i64` 始终为 64 位。`usize` 是 Rust 容器长度的自然返回类型，而 Go `Count` 返回 `int`。哈希表容量属于内部性能状态，调用方不可观察，也不等同于 `Count`。

## 依赖与调用关系

下游依赖只有标准库 `std::collections::HashSet`；目标文件没有使用 `pkg/util/set/Cargo.toml` 中声明的 `hack-crate`、`memory-crate` 或 `types-crate`。`NewIntSet` 调用 `IntSet::Insert`，后者调用 `HashSet::insert`；`Exist` 调用 `HashSet::contains`；`Count` 调用 `HashSet::len`。`Int64Set` 具有对应的三条调用关系。

上游装配来自 `pkg/util/set/lib.rs`：它声明 `int_set` 模块并把全部公开符号重导出到 crate 根。RustCodeGraph 的精确查询找到了本文件的 `NewIntSet` 和 `NewInt64Set`，但 callers/callees 查询未给出跨文件生产调用边；全仓 Rust 文本引用仅确认 `pkg/util/set/int_set_test.rs` 与 `pkg/util/set/migration_aster_unit_test.rs` 使用这些 API。因而“可由其他 crate 使用”由公开导出成立，“已经进入完整应用的生产主链”则未验证。

## 错误处理与边界

所有公开函数都是无 `Result`/`Option` 的直接操作，没有业务错误分支。空切片可正常构造空集合；负数可作为普通成员，迁移测试以 `-3` 验证；`Int64Set` 接受 `i64::MIN` 与 `i64::MAX`，迁移测试验证极值与重复值共存。重复插入是幂等的，且因为 `Insert` 丢弃底层返回值，调用方无法从本方法判断该值先前是否存在，需在写入前调用 `Exist` 才能区分。

资源分配失败等标准库级不可恢复情况没有在本文件转换为错误。哈希表的平均查询/插入复杂度通常为常数级，但本文件不声明复杂度或哈希顺序契约。由于 `inner` 私有，不能像 Go map 那样直接取长度或直接索引；必须使用 `Count`、`Exist` 和 `Insert`。

## 并发与资源生命周期

集合拥有其 `HashSet`，离开作用域时由 Rust 自动释放；没有文件句柄、网络连接、异步任务、锁、通道或事务。读取方法使用 `&self`，写入使用 `&mut self`，借用规则阻止同一实例在安全 Rust 中无同步地并发读写。本文件没有内部锁，也不提供共享并发容器；跨线程共享和同步策略必须由调用方通过适当的所有权或同步原语建立。

构造函数返回完全拥有状态的值，没有对输入切片的借用，因此输入切片随后可释放或修改而不影响集合。派生 `Clone` 会复制成员，克隆后的生命周期和后续修改相互独立。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/set/int_set.go`。两边都有 `IntSet`、`Int64Set`、两个构造函数及各自的 `Exist`、`Insert`、`Count`，控制流均为“按输入逐项插入”。Go 用 `map[int]struct{}`/`map[int64]struct{}`，Rust 用 `HashSet<isize>`/`HashSet<i64>`；两者都以键存在表示成员并自动去重。

可见差异如下：Go 构造函数接收变参，Rust 接收切片；Go map 类型允许调用方直接 `len` 和索引，Rust 把 `inner` 封装为私有字段；Go `Count` 返回 `int`，Rust 返回 `usize`；Go map 是引用语义，而 Rust 的 `Clone` 是独立深拷贝，且修改必须持有 `&mut self`。`pkg/util/set/int_set_test.go` 与 `pkg/util/set/int_set_test.rs` 保留了去重、存在/不存在和批量构造的相同测试意图；Rust 迁移测试还补充了负数和 `i64` 两端极值。

## 扩展指南

新增成员操作时，应分别评估 `IntSet` 与 `Int64Set` 是否都需要同构 API，并在 `pkg/util/set/int_set_test.rs` 中补充独立测试；跨迁移契约可同步扩展 `pkg/util/set/migration_aster_unit_test.rs`。若目标是继续对齐 Go，先修改或核对 `pkg/util/set/int_set.go` 及 `pkg/util/set/int_set_test.go`，保持命名、边界和返回语义一致。不要把 Rust 测试嵌入本源文件。

若增加删除或清空，可直接在对应 `impl` 中封装 `HashSet::remove`/`clear`，但需明确返回值是否暴露；若增加迭代，必须避免承诺稳定顺序，除非显式排序并承担分配成本。若增加从迭代器构造或集合代数，应考虑复用标准 trait，并确认不会与同 crate 的 `set.rs` 职责重叠。改变 `isize`、`i64` 或 `usize` 会影响 Go 类型对应和跨平台兼容；暴露 `inner` 会扩大 API 并破坏封装。性能改动应保留构造时按输入长度预分配的行为，并用重复值、大集合和极值测试验证。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/util/set` 确认目标文件含 11 个符号；`node --file pkg/util/set/int_set.rs --offset 1 --limit 260` 读取完整 125 行实现；`query NewIntSet --kind function` 与 `query NewInt64Set --kind function` 精确定位本文件及 Go 对照符号；callers/callees 未返回本文件的跨文件生产调用边。
- 源码与装配：`pkg/util/set/int_set.rs`、`pkg/util/set/lib.rs`、`pkg/util/set/Cargo.toml`。
- Go 对照：`pkg/util/set/int_set.go`、`pkg/util/set/int_set_test.go`。
- Rust 独立测试：`pkg/util/set/int_set_test.rs`；补充迁移边界测试：`pkg/util/set/migration_aster_unit_test.rs::primitive_sets_match_go_membership_and_float_key_semantics`。
- 全仓 Rust 引用搜索：排除本源文件和主测试后，仅迁移测试直接引用本文件构造函数；未据此推断不存在外部消费者，只记录当前仓库与当前索引能证明的范围。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行任务规定的 11 章节结构校验，并人工复核文件定位、运行流程、安全扩展点和未验证边界。
