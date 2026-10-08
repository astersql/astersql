# `pkg/util/set/string_set.rs`

源文件：[`string_set.rs`](./string_set.rs)

## 文件定位

本文件属于 Cargo crate `astersql-util-set`（见 [`Cargo.toml`](./Cargo.toml)），实现与 Go `pkg/util/set/string_set.go` 对应的字符串集合。crate 入口 [`lib.rs`](./lib.rs) 将 `string_set` 声明为公开模块，并把其中的公开项重导出，因此使用者既可从 `astersql_util_set::string_set` 访问，也可从 crate 根访问 `StringSet` 和 `NewStringSet`。

它是一个通用工具层组件，不参与 SQL 解析或执行调度；当前源码确认的生产接线是 `pkg/util/stmtsummary/lib.rs` 再次重导出该类型，`pkg/util/stmtsummary/reader.rs::stmtSummaryChecker` 用它保存允许读取的 statement digest，并在 `isDigestValid` 中调用 `Exist` 做白名单判断。`pkg/executor/test/oomtest/oom_test.rs::OomCapture` 也直接从 crate 根导入它，用作测试日志消息过滤集合。

## 核心职责

- 用 `HashSet<String>` 表达“只关心字符串是否存在”的集合，并隐藏内部容器，见 `StringSet::inner`。
- 提供构造、成员查询、插入、计数、判空、清空和回调遍历等 Go 风格 API：`NewStringSet`、`Exist`、`Insert`、`Count`、`Empty`、`Clear`、`IterateWith`。
- 提供精确交集 `Intersection`，以及按 Go 字符串大小写规则匹配的 `IntersectionWithLower`。
- 在 Rust Unicode 大小写转换可能扩展为多个 Unicode 标量时，通过 `go_simple_uppercase` 和 `go_simple_lowercase` 保持 Go `strings.ToUpper`/`strings.ToLower` 的逐 rune 简单映射语义。

本文件不负责内存计量；需要跟踪内存用量时，应使用同 crate 的 `StringSetWithMemoryUsage`，而不是给此类型附加 tracker 行为。

## 主要符号

- `fn go_simple_uppercase(value: &str) -> String`：私有辅助函数。逐字符调用 Rust `to_uppercase`；若结果只有一个标量则采用该标量，若发生多标量扩展则保留原字符，以贴近 Go 的简单映射。例如 `ß` 不在这里扩展成 `SS`。
- `fn go_simple_lowercase(value: &str) -> String`：私有辅助函数。逐字符取 `to_lowercase` 的第一个标量，丢弃 Rust 完整映射可能产生的组合后缀；相关测试以 `İ` 验证该边界。
- `pub struct StringSet { inner: HashSet<String> }`：公开集合类型，内部字段私有；派生 `Clone`、`Debug`、`Default`、`Eq` 和 `PartialEq`。相等性只比较成员，不受哈希遍历顺序影响。
- `pub fn NewStringSet(ss: &[&str]) -> StringSet`：按输入长度预分配容量，再逐项调用 `Insert`；重复项自然去重，空切片得到空集。
- `Exist(&self, val: &str) -> bool`：借用字符串做成员查询，不产生新所有权。
- `Insert(&mut self, val: String)`：取得字符串所有权并插入；重复插入不会增加 `Count`。
- `Intersection(&self, rhs: &StringSet) -> StringSet`：遍历左集合，在右集合中精确查询，返回新的独立集合，并保留左侧字符串值。
- `IntersectionWithLower(&self, rhs: &StringSet, toLower: bool) -> StringSet`：遍历 `rhs`；先将右侧成员转为小写（`true`）或大写（`false`），再到 `self` 中查询；命中时写入结果的是 `rhs` 原文，而不是转换后的文本。
- `Count`、`Empty`、`Clear`：分别委托给 `HashSet::len`、`is_empty`、`clear`；`Count` 返回 Rust 容器使用的 `usize`。
- `IterateWith<F: FnMut(String)>`：按未定义顺序遍历，克隆每个成员并把拥有所有权的 `String` 传给回调。

## 执行流程

构造流程从 `NewStringSet` 开始：按切片长度创建 `HashSet`，随后逐个复制 `&str` 为 `String` 并调用 `Insert`。集合去重由 `HashSet::insert` 完成，构造函数不另设重复分支。

精确交集 `Intersection` 创建空结果集，遍历 `self.inner`，对每个成员调用 `rhs.Exist`；命中后克隆左侧成员并插入结果。结果与两侧输入没有共享可变状态。

大小写折叠交集 `IntersectionWithLower` 的方向是关键不变量：它遍历 `rhs.inner`，按 `toLower` 选择 `go_simple_lowercase` 或 `go_simple_uppercase`，用转换值查询 `self`，但将未转换的 `origElt` 克隆进结果。因此 `self` 应预先采用目标规范形式，而返回值保留调用者提供的右侧拼写。该函数并非一般意义上将两边都大小写折叠，也不执行 Unicode normalization。

遍历流程 `IterateWith` 直接走 `HashSet` 迭代器。哈希集合没有稳定顺序；若消费者需要确定顺序，应像 `migration_aster_unit_test.rs` 那样先收集再排序，而不能依赖回调到达次序。

## 数据与状态

唯一持久状态是 `StringSet::inner: HashSet<String>`。成员字符串由集合拥有；查询接受借用，插入和回调输出使用拥有所有权的 `String`。`Clone` 会复制整个集合及字符串，克隆体与原对象后续互不影响；`Default` 等价于空 `HashSet`。

`Intersection` 和 `IntersectionWithLower` 总是分配新集合。`Clear` 删除所有成员，但 `HashSet::clear` 的容量保留行为属于标准库实现细节，调用者不应把它当作释放全部已分配内存的保证。`NewStringSet` 的 `with_capacity(ss.len())` 只是容量优化，不影响可观察的成员语义。

集合没有保存插入顺序、重复次数或原始容量。`Eq`/`PartialEq` 的测试因此可以直接比较两个集合，即使它们的插入顺序不同。

## 依赖与调用关系

下游依赖仅为 Rust 标准库 `std::collections::HashSet`；本文件没有使用 `Cargo.toml` 中声明的 `hack-crate`、`memory-crate` 或 `types-crate`，这些依赖服务于同 crate 的其他模块。

文件内部的已核实调用边为：

- `NewStringSet -> StringSet/HashSet::with_capacity -> Insert`。
- `Intersection -> NewStringSet -> Exist/Insert`。
- `IntersectionWithLower -> NewStringSet -> go_simple_lowercase|go_simple_uppercase -> Exist/Insert`。
- `Count`、`Empty`、`Clear`、`IterateWith` 分别直接调用对应的 `HashSet` 操作。

crate 边界由 `pkg/util/set/lib.rs` 建立：它声明 `pub mod string_set` 并 `pub use string_set::*`。工作区根 `Cargo.toml` 将 `pkg/util/set` 纳入成员并以 `facade_util_set` 建立工作区依赖别名。源码确认的直接消费链包括 `pkg/util/stmtsummary/lib.rs -> set_dependency::string_set::{NewStringSet, StringSet}`，随后由 `reader.rs::stmtSummaryChecker::isDigestValid -> StringSet::Exist` 完成 digest 过滤；测试消费链包括 `pkg/executor/test/oomtest/oom_test.rs::OomCapture -> NewStringSet/Insert/Clear/Exist`。

RustCodeGraph 的文件节点报告该文件被多个文件使用，但对 `StringSet`、`Insert` 等常见名称的 `explore`/`impact` 输出混入了 Go 符号和其他同名类型。因此上面的跨文件调用关系只采用经精确源码搜索确认的接线，不把图中的同名候选当作事实。

## 错误处理与边界

这些 API 不返回 `Result`，正常集合操作没有业务错误分支。两个大小写辅助函数对每个字符的映射调用 `expect("case mapping is never empty")`；其前提是 Rust 字符大小写迭代器至少产生一个标量。若标准库违反此前提会 panic，但当前实现没有可恢复错误路径。

重要边界如下：空输入构造空集；重复插入幂等；查询不存在的成员返回 `false`；与空集求交得到空集；`Clear` 后 `Empty` 为真且 `Count` 为零。所有匹配都是完整字符串匹配，没有前缀、locale、排序规则或 Unicode normalization 语义。

`IntersectionWithLower` 只转换右侧再查左侧，因此若左侧没有按所选大小写形式准备，视觉上近似的字符串也可能不匹配。它保留 `rhs` 原文，这一点对返回拼写有意依赖的调用者是兼容性约束。

## 并发与资源生命周期

`StringSet` 内部不含锁、原子、线程、任务、通道、文件句柄或事务。只读方法可通过共享引用调用；`Insert` 和 `Clear` 需要独占可变引用，Rust 借用规则在编译期阻止同一实例的无同步并发写。跨线程共享并写时，调用者必须自行使用 `Mutex`、`RwLock` 等同步容器；例如 OOM 测试把包含 `StringSet` 的 `OomCapture` 放入 `Mutex`，同步责任不在本文件。

新集合在函数返回时转移给调用者，离开作用域后由 Rust 自动释放；交集的临时转换字符串和结果成员也遵循普通所有权生命周期。`IterateWith` 在回调执行前克隆成员，因此回调拿到的字符串可独立存活，但每个成员都会产生一次分配/复制成本。回调是同步的 `FnMut`，本方法不会派生异步任务。

## 与 Go 版本的对应关系

Go 文件 `pkg/util/set/string_set.go` 用 `type StringSet map[string]struct{}`；Rust 用带私有字段的 `HashSet<String>`。两者都表达唯一键集合，并保持无序遍历、重复插入幂等、原地清空和新建交集的语义。

API 对应关系为：Go 可变参数 `NewStringSet(ss ...string)` 对应 Rust `NewStringSet(&[&str])`；Go 的 `int` 计数对应 Rust `usize`；Go map 可直接取 `len` 和写键，而 Rust 通过公开方法封装私有容器；Go 回调接收字符串值，Rust `IterateWith` 通过克隆传递拥有所有权的 `String`。

大小写行为是移植中的非直译点。Go `strings.ToUpper`/`ToLower` 基于逐 rune 简单映射；Rust `char::to_uppercase`/`to_lowercase` 暴露可能包含多个标量的完整映射。因此 Rust 使用两个辅助函数抑制扩展，`string_set_test.rs::intersection_with_lower_uses_go_simple_unicode_case_mapping` 以 `ß` 和 `İ` 锁定兼容行为。

Go 的 `string_set_test.go::TestStringSet` 覆盖去重、存在性、构造和连续交集；Rust `string_set_test.rs::TestStringSet` 保持相同场景。Rust 另有上述 Unicode 回归测试，并在 `migration_aster_unit_test.rs::string_set_operations_case_conversion_clear_and_iteration_match_go` 覆盖 Go 原测试未覆盖的大小写交集、遍历、清空与判空。

## 扩展指南

新增集合操作时，应优先在 `impl StringSet` 中实现并保持 `inner` 私有，避免调用者绕过所有权和不变量。若新增操作在 Go 已存在，应逐项对照 `pkg/util/set/string_set.go` 的遍历方向、返回元素来源和大小写规则，不要仅凭数学上等价就改变可观察的原始拼写或分配行为。

修改 `IntersectionWithLower` 时必须保留三项契约：只转换 `rhs`、在 `self` 中查询、结果保留 `rhs` 原文。涉及 Unicode 的改动应扩充 `string_set_test.rs` 的独立测试，至少包含会发生多标量完整映射的字符、空串、ASCII 和非 ASCII；不要把测试内嵌回生产文件。若 Go 对照增加方法，应同步 `string_set.go`/其测试所表达的真实语义，而不是自行简化 Rust 版本。

若需要借用式遍历以避免克隆，可以新增显式接受 `&str` 的 API，但这是公开签名和生命周期契约变化，需保留现有 `IterateWith(FnMut(String))` 的兼容性并评估所有调用者。若需要内存计量，应扩展 `set_with_memory_usage.rs` 对应类型，不应让此轻量类型隐式依赖 tracker。任何依赖遍历顺序的功能都应在调用方排序，或另建有序集合类型，不能改变当前 `HashSet` 契约。

同步验证位置为 `pkg/util/set/string_set_test.rs`、`pkg/util/set/migration_aster_unit_test.rs` 和 Go 对照 `pkg/util/set/string_set_test.go`。跨 crate 行为变化还应检查 `pkg/util/stmtsummary/reader.rs` 的 digest 过滤，以及直接使用集合的 OOM 测试辅助逻辑。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/util/set/string_set.rs`；`files --filter pkg/util/set` 确认源、Go 对照及独立测试均被索引；`node --file pkg/util/set/string_set.rs --offset 1 --limit 240` 读取全部 171 行及文件使用概览；`node --file pkg/util/set/string_set_test.rs --offset 1 --limit 240` 读取全部 84 行测试。
- RustCodeGraph 符号查询：`query StringSet --kind struct --json`、`query NewStringSet --kind function --json`、`query IntersectionWithLower --json`、`query IterateWith --json` 确认目标符号与签名；`callees string_set.rs::NewStringSet` 输出确认本文件内 `NewStringSet -> Insert`、`Intersection -> NewStringSet/Exist/Insert`、`IntersectionWithLower -> 大小写辅助函数/NewStringSet/Exist/Insert`。图对常见名称存在过匹配，跨文件结论另以精确源码搜索复核。
- 已读实现与边界文件：`pkg/util/set/string_set.rs`、`pkg/util/set/Cargo.toml`、`pkg/util/set/lib.rs`、工作区根 `Cargo.toml`。
- 已读 Go 对照与测试：`pkg/util/set/string_set.go`、`pkg/util/set/string_set_test.go`。
- 已读 Rust 测试：`pkg/util/set/string_set_test.rs`、`pkg/util/set/migration_aster_unit_test.rs`。
- 已读直接调用证据：`pkg/util/stmtsummary/lib.rs`、`pkg/util/stmtsummary/reader.rs`、`pkg/executor/test/oomtest/oom_test.rs`；并用 `rg` 对 `NewStringSet`、`StringSet`、`IntersectionWithLower`、`IterateWith` 及依赖别名做限定搜索。
- 本任务只新增说明文档，未修改 Rust、Go、Cargo 或总计划；按任务要求不运行 Cargo。结构验收命令及限定 diff 自审在交付前执行。
