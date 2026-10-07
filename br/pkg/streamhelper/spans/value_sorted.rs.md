# `br/pkg/streamhelper/spans/value_sorted.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-streamhelper-spans`；该包在 [`Cargo.toml`](./Cargo.toml) 中声明为以 `lib.rs` 为入口的 library，并用 `package.metadata.porting.go-package = "br/pkg/streamhelper/spans"` 标明其 Go 对照包。模块入口 [`lib.rs`](./lib.rs) 通过 `pub mod value_sorted` 加载本文件，再用 `pub use value_sorted::*` 扁平导出其公开符号。

在流备份 checkpoint 推进链中，本文件位于“区间进度存储”一层：[`sorted.rs`](./sorted.rs) 的 `ValuedFull` 负责按 `StartKey` 保存完整、互不重叠的区间图，本文件的 `ValueSortedFull` 再建立按 checkpoint 值排序的二级索引。上层 [`../advancer.rs`](../advancer.rs) 用它合并各 Region 的 flush TS，并取全局最小值作为安全推进水位。

## 核心职责

`ValueSortedFull` 同时维护两种观察顺序：

- `full: ValuedFull` 是事实主树，按区间起点排列并执行重叠切分、取较大值和相邻同值段粘连。
- `valueIdx: BTreeMap<(Value, Vec<u8>), Valued>` 是派生索引，先按 `Value`、再按 `StartKey` 排列，用于快速取得最小 checkpoint 或扫描所有 `Value < n` 的落后区间。

文件的关键不变量是两棵树表示同一组 `Valued`。`MergeAll` 因此不能只更新 `full`：它必须删除受重叠影响的旧索引项，再把 `mergeWithOverlap` 实际产生的新段写回 `valueIdx`。复合键中的 `StartKey` 也使同一 `Value` 的多个区间不会互相覆盖，并给出稳定的键序。

## 主要符号

- `pub struct ValueSortedFull`：封装键序主树和私有值序索引。字段均不公开，调用方只能通过本文件的方法维护一致性。
- `pub fn Sorted(f: ValuedFull) -> ValueSortedFull`：取得已有主树的所有权，通过 `ValuedFull::Traverse` 一次性重建 `(Value, StartKey) -> Valued` 索引。
- `ValueSortedFull::Merge(newItem)`：单项便利入口，委托 `MergeAll(vec![newItem])`。
- `ValueSortedFull::MergeAll(newItems)`：逐项查询重叠旧段、删除旧索引、调用主树合并并收集新段、插入新索引，是本文件维持一致性的核心方法。
- `ValueSortedFull::TraverseValuesLessThan(n, action)`：按值升序遍历严格小于 `n` 的项；回调返回 `false` 时立即停止。
- `ValueSortedFull::Min()`：返回值序首项的克隆；空集合返回 `None`。
- `ValueSortedFull::MinValue()`：把 `Min()` 投影为 `Option<Value>`。
- `ValueSortedFull::Traverse(m)`：直接转发主树遍历，因此顺序是 `StartKey` 序而非 `Value` 序。
- `pub fn NewSortedFull(init)`：用 `Full()` 构造全键空间、用 `NewFullWith` 赋统一初值，最后交给 `Sorted` 建索引。

本文件没有模块级常量、trait、条件编译项或独立错误类型。

## 执行流程

构造路径有两条。`Sorted` 接收任意已有 `ValuedFull`，遍历每个段并插入二级索引；`NewSortedFull` 则先构造初值覆盖全键空间的主树，再复用 `Sorted`。在生产入口 `CheckpointAdvancer::SetTask` 中，任务范围为空时使用 `Full()`，否则使用任务 spans，然后执行 `Sorted(NewFullWith(&init, 0))` 初始化 checkpoint 图。

合并一个 `Valued` 时，`Merge` 将其包装为单元素向量。`MergeAll` 对每个输入依次执行：

1. 用 `ValuedFull::overlapped` 收集与输入区间重叠的旧段。
2. 按旧段的 `(Value, StartKey)` 从 `valueIdx` 删除；这是避免旧 checkpoint 残留的必要步骤。
3. 调用 `ValuedFull::mergeWithOverlap(item, overlapped, Some(&mut inserted))`。主树会保留左右余量，对重叠主体取新旧值最大值，并粘连相邻同值段，同时把最终写回的段记录到 `inserted`。
4. 将 `inserted` 中的每个新段按复合键加入 `valueIdx`，恢复双索引一致性。

读取时，`Min` 取 `BTreeMap::values().next()`，所以得到 `(Value, StartKey)` 全序的首项；`MinValue` 只取其中的值。`TraverseValuesLessThan` 使用半开 range `..(n, Vec::new())`：所有较小 `Value` 都在范围内，而 `Value == n` 的键不小于下界复合键，因而严格排除。`Traverse` 则从主树按键序导出完整覆盖图。

在上层流程中，`CheckpointAdvancer::tryAdvance` 和 `optionalTick` 的成功钩子调用 `Merge` 写入 Region flush TS；`importantTick` 在 `WithCheckpoints` 的互斥锁保护下调用 `MinValue` 和 `Min`，再上传任务全局 checkpoint 并更新 GC safe point。

## 数据与状态

`Value` 在 [`sorted.rs`](./sorted.rs) 中是 `u64`，在该调用链中表示 checkpoint TS。`Valued` 由半开区间 `Span { StartKey, EndKey }` 和 `Value` 组成；主树合并使用 `max`，使同一区域的进度单调不减。

`valueIdx` 的键是 `(u64, Vec<u8>)`，Rust 元组采用字典序，因此排序规则与 Go 的 `sortedByValueThenStartKey.Less` 一致：先比较值，不同值时较小 checkpoint 优先；值相同时按 `StartKey` 排序。索引值保存完整 `Valued`，对外遍历和最小值读取会克隆它，因此不会暴露内部可变引用。

`Sorted` 的时间复杂度约为对主树 `N` 个段执行 `N` 次 `BTreeMap` 插入，即 `O(N log N)`。每次合并的成本由重叠段数 `K`、主树重写和索引删插组成，约为 `O(K log N)`；`Min`/`MinValue` 为树首项读取，`TraverseValuesLessThan` 为 `O(log N + M)`，其中 `M` 是被访问的结果数。结构保存同一组区间的两份索引，空间复杂度为 `O(N)`，但 `Valued` 及键字节会发生克隆。

## 依赖与调用关系

直接标准库依赖只有 `std::collections::BTreeMap`。crate 内依赖为：

- `sorted::{NewFullWith, Value, Valued, ValuedFull}`：定义主树、区间值类型、初始化函数以及本文件调用的 `Traverse`、`overlapped`、`mergeWithOverlap`。
- `utils::Full`：产生全键空间 span，供 `NewSortedFull` 使用。

RustCodeGraph 显示目标文件被 [`../advancer.rs`](../advancer.rs)、`advancer_test.rs`、`subscription_test.rs` 和本目录 `parity_test.rs` 等文件引用。生产侧关键边为 `CheckpointAdvancer::SetTask -> Sorted/NewFullWith`、成功钩子 `-> ValueSortedFull::Merge`、`CheckpointAdvancer::importantTick -> MinValue/Min`。`lib.rs` 的公开再导出使上层通常从 crate 根引入这些符号。

测试侧，[`value_sorted_test.rs`](./value_sorted_test.rs) 直接覆盖 `Sorted`、`Merge` 和 `TraverseValuesLessThan`；[`parity_test.rs`](./parity_test.rs) 还覆盖 `NewSortedFull`、`MergeAll`、`Min`、`MinValue`、严格阈值、同值键序与提前停止。

## 错误处理与边界

本文件 API 不返回 `Result`，也不主动 panic。空主树由 Rust API 显式表示：`Min` 和 `MinValue` 返回 `None`；这比 Go 版本对空 `btree.Min()` 做类型断言更安全。生产 `importantTick` 将缺失最小值降级为 `0` 并跳过推进。

边界语义包括：

- `TraverseValuesLessThan` 是严格小于，`Value == n` 不会进入回调；`parity_test.rs` 和 Go/Rust 独立测试均锁定此语义。
- 回调返回 `false` 必须停止遍历；该行为由 `parity_test.rs` 验证。
- `MergeAll` 按输入顺序逐项更新。由于底层对重叠值取 `max`，较小的新值不会倒退已覆盖区域，但每一步仍可能切分或重新粘连区间。
- `mergeWithOverlap` 在没有重叠段时不写入任何内容；正常生产构造维护覆盖范围，调用方若自行用不完整 `ValuedFull` 包装，需理解这一底层前提。
- `(Value, StartKey)` 假定同一起点在主树中唯一；若以后改变主树唯一性或索引排序规则，必须同步调整删除键与 Go 对照逻辑。

## 并发与资源生命周期

`ValueSortedFull` 自身没有锁、线程、异步任务、通道、I/O 或事务；所有修改都要求 `&mut self`，单个实例内部的双索引更新是同步顺序执行的。它也没有显式资源释放逻辑，`BTreeMap` 和克隆的字节向量随所有者离开作用域自动释放。

并发控制由上层承担。`CheckpointAdvancer` 把实例放在 `Mutex<Option<ValueSortedFull>>` 中，`WithCheckpoints` 持锁执行闭包；采集成功钩子也先取得同一互斥锁再调用 `Merge`。因此 `importantTick` 读取最小水位时不会与区间合并并发观察到半更新状态。若未来在别处共享该类型，必须沿用外部互斥或所有权串行化，不能把一次 `MergeAll` 拆成可并发观察的“先删旧索引、后写主树、再插新索引”阶段。

## 与 Go 版本的对应关系

直接对照文件是 [`value_sorted.go`](./value_sorted.go)。结构与主要方法一一对应：Go 的嵌入式 `*ValuedFull` 对应 Rust 私有字段 `full`；Go 的 `*btree.BTree` 对应 Rust `BTreeMap<(Value, StartKey), Valued>`；`Sorted`、`Merge`、`MergeAll`、`TraverseValuesLessThan`、`Min` 和 `MinValue` 保留相同职责与排序规则。

Rust 的主要可见差异是：

- Rust `Min`/`MinValue` 返回 `Option`，定义了空集合行为；Go 返回非可选值，并假定树非空。
- Go `ValueSortedFull` 嵌入主树，因此自动暴露其方法；Rust 采用组合，只显式转发 `Traverse`，减少绕过二级索引直接修改主树的机会。
- Go `MergeAll` 复用循环外的 `overlapped`/`inserted` 缓冲；Rust 每项新建两个 `Vec`，语义一致但存在额外分配的性能差异。
- Go 在主树 merge 后删除旧索引，Rust 在调用 `mergeWithOverlap` 前删除；整个 Rust 方法需要独占 `&mut self`，正常返回后的最终状态相同。本文件没有可失败分支或回调穿插在这两个阶段。
- Rust 额外提供 `NewSortedFull`，将 Go 侧常见的 `Sorted(NewFullWith(Full(), init))` 组合封装为构造函数。

[`value_sorted_test.rs`](./value_sorted_test.rs) 与 [`value_sorted_test.go`](./value_sorted_test.go) 使用相同四组输入和期望结果，验证多次覆盖后值序索引与键序主树一致。Rust 的 `parity_test.rs` 进一步验证空集合 `Option` 行为和公开契约。

## 扩展指南

新增改变区间内容的操作时，应把入口放在 `ValueSortedFull` 的 `impl` 内，并像 `MergeAll` 一样同时维护 `full` 与 `valueIdx`；不要向调用方暴露 `&mut ValuedFull`。若新增按值范围查询，可复用复合键 range，但必须明确端点是开区间还是闭区间，并覆盖相同 `Value`、不同 `StartKey` 的顺序。

修改合并规则、索引键或空集合语义时，需要同步核对：

- [`sorted.rs`](./sorted.rs) 的 `join`、`mergeWithOverlap` 和主树不变量；
- Go [`value_sorted.go`](./value_sorted.go) 以及 [`sorted.go`](./sorted.go) 的对应行为；
- 独立 Rust 测试 [`value_sorted_test.rs`](./value_sorted_test.rs)，不得把测试内嵌回生产文件；
- Go 测试 [`value_sorted_test.go`](./value_sorted_test.go) 和 Rust 契约测试 [`parity_test.rs`](./parity_test.rs)；
- 上层 [`../advancer.rs`](../advancer.rs) 对最小值、锁粒度和 checkpoint 单调性的依赖。

性能敏感的扩展应避免全量重建索引，优先收集实际删除/插入段；若尝试复用缓冲区或减少 `Vec<u8>` 克隆，应先证明复合键生命周期和双索引一致性不受影响。兼容性上，任何从严格 `< n` 改为 `<= n`、改变同值排序或允许 checkpoint 回退的修改都会改变推进器选择落后区间的行为。

## 验证依据

本说明依据以下本地事实完成：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper/spans` 确认目标、模块入口、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file br/pkg/streamhelper/spans/value_sorted.rs`：核对 `ValueSortedFull`、两个构造函数、六个方法及全部实现；`query ValueSortedFull`、`query NewSortedFull` 用于消除 Go/Rust 同名符号。
- RustCodeGraph 对 `sorted.rs`、`lib.rs`、`advancer.rs` 的文件节点：核对底层合并不变量、公开再导出，以及 `SetTask`、成功钩子、`importantTick` 的生产调用链。
- RustCodeGraph 对 `value_sorted.go`、`value_sorted_test.rs`、`value_sorted_test.go`、`parity_test.rs` 的文件节点：核对 Go/Rust API 差异、四组对齐用例、严格阈值、提前停止、最小值和空集合语义。
- [`Cargo.toml`](./Cargo.toml)：核对 crate 名称、library 入口、Go 包映射和无额外 feature/依赖声明。

本任务是纯文档分析，按计划未运行 Cargo。交付结构检查要求本文恰有“文件定位”至“验证依据”共 11 个固定二级标题；代码行为结论来自上述源码、调用边和测试，未以编译成功或零测试替代证据。
