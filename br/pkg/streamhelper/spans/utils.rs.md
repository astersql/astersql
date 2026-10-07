# `br/pkg/streamhelper/spans/utils.rs`

## 文件定位

本文件是 `astersql-br-pkg-streamhelper-spans` library crate 的区间工具层。`br/pkg/streamhelper/spans/Cargo.toml` 将 `lib.rs` 声明为 crate 入口，`lib.rs` 通过 `#[path = "utils.rs"] pub mod utils` 挂载本模块，再用 `pub use utils::*` 将符号扁平导出。crate 的 porting 元数据指向 Go 包 `br/pkg/streamhelper/spans`；对应实现是同路径 `utils.go`。

它不是独立数据结构，而是 `sorted.rs` 和 `value_sorted.rs` 共享的几何与校验基础：定义半开区间的重叠判定、空键的无穷端点比较、范围并集折叠、全键空间占位，以及带值区间集的覆盖等价判定。这些符号进一步被 `br/pkg/streamhelper/advancer.rs` 用于检查点范围初始化和采集前去重。

## 核心职责

本文件有四组职责：

1. 统一 `Span` 的半开区间语义：`Overlaps` 将有限区间解释为 `[StartKey, EndKey)`，因此仅端点相接不算重叠；空 `EndKey` 表示向右无穷延伸。
2. 为其他区间算法提供可显式指定无穷语义的字节比较 `CompareBytesExt`。空切片只有在对应 `*_inf` 为 `true` 时才是 `+∞`，否则仍是普通的有限空键。
3. 将任意顺序的 `Span` 折叠为按起点排序的不相交覆盖，并提供全键空间占位 `Full`。
4. 比较带值区间：`Valued::Equals` 执行单元素严格相等，`ValuedSetEquals` 则忽略同值区间的切分方式，比较整体键空间覆盖与取值是否等价。`Debug` 输出键序主树与值序索引两种视图，用于人工对照状态。

## 主要符号

- `pub fn Overlaps(a: &Span, b: &Span) -> bool`：判定两个半开区间是否有非空交集。两端有限时使用 `a.start < b.end && b.start < a.end`；任一右端为空时按 `+∞` 分支处理。
- `pub fn CompareBytesExt(a: &[u8], a_inf: bool, b: &[u8], b_inf: bool) -> i32`：返回 `-1/0/1`。仅“切片为空且对应 inf 标志为真”时视为无穷；其余情况使用 Rust 切片字典序。
- `pub fn Debug(full: &ValueSortedFull)`：分别通过 `Traverse` 和 `TraverseValuesLessThan(u64::MAX, ...)` 收集全部 `Valued`，再用 `println!` 输出两份调试视图。
- `pub fn Collapse(spans: &[Span]) -> Vec<Span>`：克隆输入，先按 `StartKey`、再按将空 `EndKey` 视为 `+∞` 的右端排序，然后合并重叠或相邻的范围。
- `pub fn Full() -> Vec<Span>`：返回只含 `Span::default()` 的向量。结合本 crate 的端点规约，空起止键表示整个键空间。
- `impl Valued::Equals(&self, y: &Valued) -> bool`：要求 `Value`、`StartKey` 和 `EndKey` 全部相同，不做切分归一化。
- `pub fn ValuedSetEquals(mut xs: Vec<Valued>, mut ys: Vec<Valued>) -> bool`：消费并就地排序两份输入，用双指针允许一侧的单段对应另一侧多个同值且首尾相接的分段。

文件不定义新 struct、enum、trait、模块级常量或条件编译项。`Span`、`Valued` 来自 `sorted.rs`，`ValueSortedFull` 来自 `value_sorted.rs`。

## 执行流程

`Collapse` 是主要的区间归一化流程：

1. 将输入 `&[Span]` 克隆为可排序向量，不修改调用方数据。
2. 先比较 `StartKey`；起点相同时用 `CompareBytesExt(..., true, ..., true)` 比较右端，因而有限右端排在无穷右端之前。
3. 从当前段开始向右扫描。只有当当前右端有限且下一起点严格大于它时才结束本组；因此右端恰好等于下一起点会被当作相邻并合并。
4. 对组内每段扩张右端：有限右端取较大者，遇到空 `EndKey` 则变为 `+∞`。将完成的并集段放入结果。

`ValuedSetEquals` 的流程不会先合并整个集合，而是保留切分后做双指针对齐：

1. 任一侧为空时，仅在另一侧也为空时返回真。
2. 两侧各自按起点和无穷右端规则排序。每组覆盖的首段起点必须相同。
3. 比较当前两段的 `Value`；不同立即返回假。
4. 比较右端。相等则两侧同时前进；一侧较短则只前进该侧，并检查它的旧右端与新段起点完全相接。不相接表示有空洞，立即返回假。
5. 任一侧耗尽时，只有另一侧也同时耗尽才相等。

生产主链上，`advancer.rs::SetTask` 将任务 `KeyRange` 转成 `Span`；无显式范围时用 `Full()`，再经 `NewFullWith` 和 `Sorted` 初始化检查点树。`advancer.rs::tryAdvance` 则在遍历 Region 之前调用 `Collapse` 合并任务范围，避免重复采集。

## 数据与状态

`Span` 由两个 `Vec<u8>` 组成，本文件对键使用原始字节字典序，不解析 TiKV key 编码。空 `StartKey` 在普通字典序中是最小键；空 `EndKey` 需在右端上显式以 `inf=true` 传给 `CompareBytesExt`，才表示 `+∞`。`Full()` 借助这两条约定用空起止键表达全空间。

`Collapse` 保留每个并集组排序后首段的 `StartKey`，只扩张其 `EndKey`。它会克隆全部输入 `Span`，时间主要由排序决定，为 `O(n log n)`；排序后扫描为 `O(n)`，额外空间为 `O(n)` 加键字节克隆成本。

`ValuedSetEquals` 按值接收两个 `Vec<Valued>`，所以排序不会改变调用者手中的向量；调用者如果需要保留原集合，必须像测试一样克隆后传入。内层循环为避免借用冲突会克隆当前 `Valued`；复杂度同样由两次排序主导。

本文件没有全局可变状态、缓存或持久化。`Debug` 的两个临时向量是唯一的调试状态，函数返回后即释放。

## 依赖与调用关系

- crate 内下游：`utils.rs` 导入 `sorted::{Span, Valued}` 与 `value_sorted::ValueSortedFull`；`Debug` 调用 `ValueSortedFull::{Traverse, TraverseValuesLessThan}`。其他逻辑只使用标准库比较、排序、向量和输出。Cargo manifest 没有声明外部依赖。
- crate 内上游：`sorted.rs::NewFullWith` 调用 `Collapse`；`ValuedFull::mergeWithOverlap` 使用 `CompareBytesExt` 判定相邻与右余量；`ValuedFull::overlapped` 使用 `Overlaps` 限定 BTreeMap 扫描范围。`value_sorted.rs::NewSortedFull` 通过 `Full()` 建立全键空间。
- 生产调用者：`br/pkg/streamhelper/advancer.rs::SetTask` 使用 `Full`，`tryAdvance` 使用 `Collapse`；`br/pkg/streamhelper/basic_lib_for_test.rs` 的模拟 Region 遍历使用 `Overlaps`。`br/pkg/utiltest/fakecluster/core.rs` 通过该 crate 使用 `CompareBytesExt` 和 `Overlaps` 实现测试集群的键范围查询。
- crate 边界：`br/pkg/streamhelper/Cargo.toml` 以路径依赖引入本 crate；`br/pkg/utiltest/fakecluster/Cargo.toml` 也直接依赖它。`lib.rs` 的扁平再导出使上述调用可以使用 crate 根符号。
- 测试与调试：`spans/utils_test.rs` 直接覆盖 `ValuedSetEquals`；`spans/parity_test.rs` 覆盖 `Overlaps`、`Collapse`、`Full` 与 `Valued::Equals`；`sorted_test.rs` 和 `value_sorted_test.rs` 通过整体结构间接覆盖，后者还在调试分支调用 `Debug`。

RustCodeGraph 的文件节点标记 `utils.rs` 被 9 个文件使用，但批量 `callers/callees` 查询未返回可用的方法边；上述具体调用点因此由 RustCodeGraph 文件源码与全仓 Rust 引用搜索交叉确认，不将图边缺失解释为“无调用者”。

## 错误处理与边界

本文件的公开函数都不返回 `Result` 或 `Option`，也不主动校验 `StartKey <= EndKey`、输入区间是否自相矛盾，或同一侧的 valued 分段是否互不重叠。调用者应提供符合 spans 不变量的区间；否则算法只会按字典序执行，结果不代表对非法范围的拒绝。

`Overlaps` 的关键边界是半开语义：`[a,b)` 与 `[b,c)` 不重叠；而 `Collapse` 有意将它们当作相邻范围合并。两者判断不同是 API 语义，不是矛盾。空 `EndKey` 是 `+∞`；空起点则按最小有限字节键处理，与 `Full()` 的全空间表达配合。

`Collapse(&[])` 返回空向量。`ValuedSetEquals([], [])` 返回真，仅一侧为空返回假。对非空集合，它会拒绝起点不对齐、覆盖右端不一致、对应切片值不同、一侧分段之间存在空洞，以及两侧不同时耗尽。`utils_test.rs` 专门锁定了这些边界。

`Debug` 唯一可观察副作用是向标准输出打印；它没有错误通道，也不保证日志框架级别、结构化字段或生产环境可用性。

## 并发与资源生命周期

`Overlaps`、`CompareBytesExt`、`Collapse`、`Full`、`Valued::Equals` 和 `ValuedSetEquals` 都是同步计算，不启动线程、异步任务或定时器，不持有锁、通道、文件、网络连接或事务。除了返回的向量，所有临时数据在函数返回时释放，不会跨调用保留。

`Debug` 只通过共享引用读取 `ValueSortedFull`，不修改树或索引；但标准输出可能与其他线程打印交错。是否能在多线程中共享 `ValueSortedFull` 由类型及外层同步决定；本文件不添加 `Mutex` 或 `Arc`。生产调用中 `CheckpointAdvancer` 将检查点树放在 `Mutex<Option<ValueSortedFull>>` 内，该锁生命周期属于 `advancer.rs`，不属于本模块。

`Collapse` 与 `ValuedSetEquals` 都不在共享输入上就地修改：前者克隆切片，后者获取向量所有权。因此函数内排序不需要外部锁，但大量键或分段时的分配、克隆和排序开销仍需由调用者考虑。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/streamhelper/spans/utils.go`。Rust 的 `Overlaps`、`Debug`、`Collapse`、`Full`、`Valued::Equals` 和 `ValuedSetEquals` 都保留了 Go 命名与主要分支结构，crate 级 `allow(non_snake_case)` 为这种迁移命名提供兼容。

主要语义是逐项对齐的：

- 两版 `Overlaps` 均将空 `EndKey` 视为右无穷，有限区间使用严格不等式保留半开边界。
- Go `Collapse(length, getRange)` 通过长度和回调抽取输入；Rust 为更自然的所有权 API 改为 `Collapse(&[Span])`。排序、相邻/重叠合并、空右端扩张逻辑相同。
- Go 使用 `br/pkg/utils.CompareBytesExt`；Rust 在本文件内提供同语义的 `CompareBytesExt`，供 spans crate 内部与测试集群使用。
- Go `ValuedSetEquals` 原地排序传入的 slice；Rust 按值消费 `Vec`，因而将排序副作用限定在函数所有的向量内。双指针、值相等、相邻性与同时耗尽判定对齐。
- Go `Debug` 使用 `fmt.Printf` 和 `math.MaxUint64`；Rust 使用 `println!` 与 `u64::MAX`，都遍历键序主树和全部值序索引。具体字符串格式不完全一致，不应将输出文本当作稳定协议。

`utils_test.rs` 对应 Go `utils_test.go::TestValuedEquals`，保留 9 组数据与双向对称断言。`parity_test.rs` 另外锁定了 Go 公开契约中的重叠、相邻折叠、空输入、无穷右端和严格 `Equals` 语义。

## 扩展指南

新增或修改区间工具时，应先确定三个共享契约：范围是否仍为半开、空 `EndKey` 是否仍为 `+∞`、相邻范围是否应合并。修改 `Overlaps` 会直接影响 `ValuedFull::overlapped` 的扫描停止点；修改 `CompareBytesExt` 会同时影响折叠排序、Merge 切分与 valued 等价判定，必须审查 `sorted.rs` 和 `value_sorted.rs` 的不变量。

若要扩展 `Collapse`，应保留输出按起点排序、互不相交且覆盖与输入并集一致的不变量。若出于性能改为迭代器或可变输入 API，需同时调整 `NewFullWith`、`advancer.rs::tryAdvance` 和公开再导出调用面，并明确原 API 的所有权兼容性。

若要扩展 `ValuedSetEquals`，不要把“单个 `Valued` 严格相等”与“集合覆盖等价”混合。任何对重叠分段、重复段或非法反向范围的新规则都应先在 API 文档中定义，不应仅凭当前排序顺序推测。

测试必须继续放在独立 Rust 文件中：`utils_test.rs` 用于对齐 Go 同名集合等价测试，`parity_test.rs` 用于公开契约边界，涉及树 Merge 则同步扩展 `sorted_test.rs`，涉及值索引则扩展 `value_sorted_test.rs`。不要将 `#[cfg(test)]` 测试嵌入 `utils.rs`。每次语义变更都应对照 `utils.go` 和 `utils_test.go`；有意的 Rust 差异应在 parity 测试和本文档中说明。

## 验证依据

- RustCodeGraph 索引：`status` 报告 7032 个 Rust 文件和 4415 个 Go 文件；`files --filter br/pkg/streamhelper/spans` 列出 14 个目标、对照与测试文件；`node --file br/pkg/streamhelper/spans/utils.rs` 确认 187 行源码、8 个索引符号及 9 个使用文件。
- 主要符号查询：`query` 确认 Rust/Go `Overlaps`、`Collapse`、`CompareBytesExt` 和 `ValuedSetEquals` 的精确文件与签名。批量 `callers/callees` 查询超时且无可用输出，因此调用边用 RustCodeGraph 文件节点和 `rg` 引用结果交叉复核。
- Rust 源码与入口：`br/pkg/streamhelper/spans/utils.rs`、`lib.rs`、`sorted.rs`、`value_sorted.rs`、`br/pkg/streamhelper/advancer.rs`、`basic_lib_for_test.rs` 以及 `br/pkg/utiltest/fakecluster/core.rs`。
- crate 声明：`br/pkg/streamhelper/spans/Cargo.toml` 确认 library 边界和 Go 包对应；`br/pkg/streamhelper/Cargo.toml` 与 `br/pkg/utiltest/fakecluster/Cargo.toml` 确认直接路径依赖。
- Go 对照：`br/pkg/streamhelper/spans/utils.go` 的六组公开行为和内部比较，以及 `utils_test.go::TestValuedEquals` 的 9 组数据。
- 独立 Rust 测试：`br/pkg/streamhelper/spans/utils_test.rs`、`parity_test.rs`、`sorted_test.rs` 和 `value_sorted_test.rs`。其中直接边界证据包括空集、乱序输入、相邻折叠、有限端点相接不重叠、空右端无穷、同值不同切分等价、取值/覆盖不一致与中间空洞。
- 本任务仅新增文档，按总计划不运行 Cargo。交付前使用任务指定的结构命令确认文件存在且恰好含有 11 个固定二级章节。
