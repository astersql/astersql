# `br/pkg/streamhelper/spans/sorted.rs`

## 文件定位

本文件属于独立 crate `astersql-br-pkg-streamhelper-spans`；crate 入口是同目录的 [`lib.rs`](lib.rs)，其通过 `#[path = "sorted.rs"]` 声明本模块并将公开符号扁平再导出。该 crate 在 [`Cargo.toml`](Cargo.toml) 中声明为库，移植元数据指向 Go 包 `br/pkg/streamhelper/spans`，上层 `astersql-br-pkg-streamhelper` 再以路径依赖接入它。

它实现按键起点排序的“区间 + 进度值”主树，是流式备份 checkpoint 数据结构的底层。应用主链中的直接证据是 `br/pkg/streamhelper/advancer.rs::SetTask`：任务范围为空时使用 `Full()`，否则使用显式范围，随后调用 `Sorted(NewFullWith(&init, 0))` 初始化 checkpoint 树。值序二级索引由同目录 `value_sorted.rs` 包装，本文件只负责键序覆盖和合并。

## 核心职责

- 用 `BTreeMap<Vec<u8>, Valued>` 按 `StartKey` 保存互不重叠的半开区间 `[StartKey, EndKey)`。
- 用 `NewFullWith` 先折叠初始化范围，再为每个范围赋统一初值，建立主树不重叠的不变量。
- 用 `Merge` 将新进度合入现有覆盖：只修改与输入相交的既有范围，重叠部分取两值最大值，未覆盖的左右余量保持原值。
- 合并过程中把首尾相接且值相同的区间粘连，控制长期推进造成的分片数量。
- 用 `Traverse` 提供稳定的键序遍历和回调早停能力；`value_sorted.rs::Sorted` 依赖它建立值序索引。

这里的 `ValuedFull` 名称沿用 Go。实际覆盖域由 `initSpans` 的并集决定，并不必然是整个键空间；`sorted_test.rs::test_sub_range` 明确验证了多个子窗口之间的空洞不会被填充。

## 主要符号

- `type Value = u64`：区间携带的单调进度值，流备份中通常是 checkpoint TS。
- `fn join(a, b) -> Value`：内部合并策略，返回 `max(a, b)`，保证已推进水位不会因较小的新值回退。
- `struct Span { StartKey, EndKey }`：字节键的半开区间。空 `EndKey` 通过 `CompareBytesExt`/`Overlaps` 解释为正无穷；`Full()` 使用起止均为空的默认值表示全键空间。
- `struct Valued { Key, Value }`：将一个 `Span` 与进度绑定。`Valued::Less` 只比较 `StartKey`，对应 Go `btree.Item.Less` 的排序依据。
- `struct ValuedFull`：持有私有 `inner: BTreeMap<Vec<u8>, Valued>`。映射键复制自 `Valued.Key.StartKey`，因此所有写回路径必须保持二者一致。
- `NewFullWith(initSpans, init)`：公开构造函数。调用 `Collapse` 排序并合并重叠或相邻的初始化范围，再逐段插入。
- `ValuedFull::Merge(val)`：公开单条合并入口，依次调用 `overlapped` 和 `mergeWithOverlap`。
- `ValuedFull::Traverse(callback)`：按 `StartKey` 升序克隆每个 `Valued` 交给回调；回调返回 `false` 时停止。
- `overlapped(k, result)`：crate 内可见的重叠查询，供本文件和 `value_sorted.rs::MergeAll` 共用。
- `mergeWithOverlap(val, overlapped, newItems)`：crate 内可见的核心改写函数；可选的 `newItems` 用来把实际写回的新段报告给值序索引。

## 执行流程

初始化流程如下：

1. `NewFullWith` 调用 `Collapse(initSpans)`，把无序、重叠或相邻的输入整理为按起点有序且互不交叠的覆盖段。
2. 每个折叠后的 `Span` 包装成值为 `init` 的 `Valued`，以其 `StartKey` 插入 `BTreeMap`。
3. 上层通常立即调用 `value_sorted.rs::Sorted`；后者通过 `Traverse` 扫描主树，建立 `(Value, StartKey)` 值序索引。

一次 `Merge(val)` 的流程如下：

1. `overlapped` 先在 `..=val.StartKey` 中找 floor 节点。若该前驱跨过输入起点，则从前驱开始；否则从输入起点开始向右扫描。
2. 扫描按键有序进行，遇到首个不重叠节点即停止，所得 `overlapped` 保持起点升序。
3. 若没有重叠，`mergeWithOverlap` 直接返回；它不会在初始化覆盖域的空洞中创建新段。
4. 有重叠时，先从主树删除所有旧段，避免旧键与重写段冲突。
5. 若首段从输入左侧跨入，切出左余量并保留旧值；若末段越过输入右端，暂存右余量并保留旧值。
6. 对裁剪后的每个重叠体计算 `join(val.Value, old.Value)`；左右余量以 `standalone = true` 绕过 `join`。
7. `emitToCollected` 将相邻且同值的段扩展为一段，否则先 `flushCollected` 写回累计段，再开始新段。
8. 最后依次处理右余量并刷新末段。若传入 `newItems`，每次实际写回也会克隆追加到该列表，供 `ValueSortedFull::MergeAll` 重建受影响的值序索引。

## 数据与状态

主状态全部位于 `ValuedFull::inner`。正常构造和合并后应维持以下不变量：

- 映射键等于节点的 `Key.StartKey`，且起点唯一。
- 节点按起点有序，覆盖段互不重叠；初始化覆盖域之外的空洞保持为空洞。
- 合并只改变输入与既有覆盖的交集，左右未覆盖部分保留原值。
- 重叠部分的值等于新旧值最大值，因此连续合并对每个键点单调不减。
- 本次改写中相邻同值段会被合并；测试比较还使用 `ValuedSetEquals` 容忍语义等价但切分不同的表示。

方法通过克隆 `Vec<u8>` 和 `Valued` 转移或复制所有权，没有借用外部键缓冲区。`Traverse` 同样向回调传递克隆值，所以回调无法直接破坏树内不变量，代价是每个访问节点都会复制两个边界字节向量。

查找 floor 节点和定位扫描起点为 `BTreeMap` 的对数操作；之后成本与实际扫描/重写的重叠段数相关。输入范围被切得很碎时，合并与克隆成本会随受影响段数增长，相邻同值粘连是主要的碎片控制机制。

## 依赖与调用关系

直接下游依赖均来自同 crate：

- `utils::Collapse`：规范化初始化范围。
- `utils::Overlaps`：按半开区间及空右端正无穷语义判断相交。
- `utils::CompareBytesExt`：比较有限/无限端点，并判断区间是否首尾相接。
- 标准库 `BTreeMap`：提供 floor 查询、范围扫描与键序遍历。

已由 RustCodeGraph 核对的主要上游边包括：

- `br/pkg/streamhelper/advancer.rs::SetTask -> NewFullWith`，建立任务 checkpoint 覆盖。
- `br/pkg/streamhelper/spans/value_sorted.rs::Sorted -> ValuedFull::Traverse`，建立值序索引。
- `ValueSortedFull::MergeAll -> ValuedFull::overlapped/mergeWithOverlap`，在更新主树的同时精确删除旧索引并登记新段。
- `br/pkg/streamhelper/subscription_test.rs` 中的辅助路径也构造 `NewFullWith`/`Sorted`，验证订阅场景。

因此本文件并不负责计算全局最小 checkpoint；该职责属于 `value_sorted.rs::MinValue`。它也不负责 Region 扫描、锁解析或定时推进，这些位于上层 `advancer.rs`。

## 错误处理与边界

本文件没有 `Result` 返回值或显式 I/O 错误：操作是纯内存、确定性的。关键边界行为如下：

- 空初始化列表产生空树；后续 `Merge` 因找不到重叠而保持空树。`parity_test.rs` 进一步验证空包装树的最小值以 `Option::None` 表示。
- 输入区间完全落在初始化覆盖域之外，或只落在子范围空洞中时，不创建节点。
- 半开区间仅端点相接不算重叠；空 `EndKey` 按正无穷处理。空起止的默认 `Span` 是全键空间约定，不是普通的零长度区间。
- `mergeWithOverlap` 在访问 `overlapped[0]` 前先检查空列表，因此正常入口不会因无重叠而越界。
- `Traverse` 必须尊重回调的 `false`；`parity_test.rs::remaining_go_contract_edges_match` 断言两段树只访问一段即停止。

调用方仍需提供语义有效的区间并维护“重叠列表来自当前树”的内部契约。`mergeWithOverlap` 是 `pub(crate)` 而非公开 API；若绕开 `overlapped` 传入乱序、重复或不属于当前树的节点，可能破坏覆盖不变量。此处没有运行时校验或错误返回来修复这种误用。

## 并发与资源生命周期

`ValuedFull` 内部没有锁、原子量或异步任务；所有修改方法需要 `&mut self`，Rust 借用规则阻止同一实例在安全代码中并发写入。上层 `advancer.rs` 将 `ValueSortedFull` 放在互斥锁保护的 checkpoint 状态中，并通过 `WithCheckpoints` 在持锁期间访问，这是应用层的并发边界，而不是本文件提供的能力。

构造函数创建并拥有整棵 `BTreeMap`；删除重叠段时旧 `Valued` 在离开局部向量后释放，写回段由树取得所有权。`newItems` 只是写回段的克隆快照，其生命周期由调用者管理。文件不持有文件句柄、网络连接、通道或后台任务，也没有显式清理步骤；`ValuedFull` 离开作用域时由 Rust 自动释放所有键和节点。

## 与 Go 版本的对应关系

直接对照文件是 [`sorted.go`](sorted.go)。主要映射为：Go `btree.BTree` 对应 Rust `BTreeMap`，`btree.Item.Less` 对应 `Valued::Less`，`DescendLessOrEqual`/`AscendGreaterOrEqual` 对应 Rust 的 floor 范围查询和向右 range 扫描，`ReplaceOrInsert` 对应 `insert`。

算法结构与 Go 保持一致：`join` 取最大值；`Merge` 先收集重叠段；`mergeWithOverlap` 删除旧段、切左右余量、合并重叠体并粘连同值相邻段；`overlapped` 先检查可能跨入起点的前驱。`sorted_test.rs` 的 `test_basic` 与 `test_sub_range` 复刻 Go [`sorted_test.go`](sorted_test.go) 的核心用例，覆盖覆盖写、较小值不回退、无限端点、子窗口裁剪和空洞隔离。

可见差异包括：

- Go `Span` 是 `kv.KeyRange` 类型别名；Rust 在本文件定义等价字段的自有结构。
- Go `Valued` 实现 `String()` 供格式化；当前 Rust 只派生 `Debug`，没有同名字符串方法。
- Go 树用运行时类型断言取出 `btree.Item`；Rust `BTreeMap` 在编译期固定键和值类型。
- Rust 的 `Traverse` 克隆节点后传值；Go 也按值断言出 `Valued`，但切片字段仍共享底层数组，而 Rust 的 `Vec<u8>` 克隆是深复制。
- Rust 的核心辅助方法为 `pub(crate)`，以支持同 crate 的 `value_sorted.rs`；Go 则依靠包内未导出方法实现相同协作。

## 扩展指南

- 修改合并策略时，应首先修改并解释 `join`，同时检查单调 checkpoint 的安全前提；不能把 `max` 改成覆盖赋值而不评估水位回退风险。
- 修改重叠或无限端点语义时，需要同步审查 `utils.rs::{Overlaps, CompareBytesExt, Collapse, Full}`，避免构造、查询和切分采用不同边界规则。
- 新增会改变树内容的操作时，必须同步维护 `value_sorted.rs::ValueSortedFull::valueIdx`。优先沿用“先收集旧重叠段、删除旧索引、由 `newItems` 登记写回段”的接线方式。
- 保持 Rust 测试与源文件分离。合并行为用例应加入 [`sorted_test.rs`](sorted_test.rs)，公共 Go/Rust 契约加入 [`parity_test.rs`](parity_test.rs)；如更改 Go 对应语义，还应对照 [`sorted_test.go`](sorted_test.go)。
- 增加区间表示或批量 API 时，应测试：空树、全键空间、有限/无限右端、仅端点相接、跨多个旧段、较小值合并、子范围空洞、回调早停，以及相邻同值段能否粘连。
- 性能改动应关注受影响段数、边界 `Vec<u8>` 克隆和碎片数量，不能只验证最终最小值；主树和值序索引必须在每次操作后保持一致。

## 验证依据

本说明基于以下直接证据：

- Rust 源码：[`sorted.rs`](sorted.rs) 中的 `join`、`Span`、`Valued`、`ValuedFull`、`NewFullWith`、`Merge`、`Traverse`、`mergeWithOverlap`、`overlapped`。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)；上层路径依赖见 `br/pkg/streamhelper/Cargo.toml`。
- 下游工具与索引：[`utils.rs`](utils.rs) 和 [`value_sorted.rs`](value_sorted.rs)。
- 应用入口：`br/pkg/streamhelper/advancer.rs::SetTask`、`WithCheckpoints`。
- Go 对照：[`sorted.go`](sorted.go) 与 [`sorted_test.go`](sorted_test.go)。
- 独立 Rust 测试：[`sorted_test.rs`](sorted_test.rs) 的 `test_basic`、`test_sub_range`，以及 [`parity_test.rs`](parity_test.rs) 的公开契约与早停检查。
- RustCodeGraph：索引状态为 7032 个 Rust 文件；查询确认 `SetTask -> NewFullWith -> Sorted/Traverse` 主链，以及 `ValueSortedFull::MergeAll -> overlapped/mergeWithOverlap` 的索引维护关系。

本任务是纯文档分析，按计划不运行 Cargo。结构验证应确认本文恰好包含任务要求的十一个二级章节。
