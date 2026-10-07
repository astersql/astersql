# `br/pkg/restore/split/sum_sorted.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-restore-split`。该 crate 的入口是同目录 `lib.rs`；入口以 `#[path = "sum_sorted.rs"] pub mod sum_sorted` 装配本模块，并通过 `pub use sum_sorted::*` 将其 API 扁平导出。`Cargo.toml` 的 `package.metadata.porting.go-package` 指向 Go 包 `br/pkg/restore/split`，因此这里是同路径 `sum_sorted.go` 的 Rust 移植，而不是一套独立算法。

在恢复主链中，本文件处在“文件范围统计”和“实际 Region 分裂”之间：`br/pkg/restore/log_client/log_split_strategy.rs::Accumulate` 与 `compacted_file_strategy.rs::Accumulate` 把日志文件或 SST 的键范围及规模合并进 `SplitHelper`；`br/pkg/restore/split/splitter.rs::RewriteSplitter` 持有该结构，`SplitHelperIterator::Traverse` 按表读取其片段，最终由 `SplitPoint` 将非零片段映射到 TiKV Region 并驱动分裂。本文件只维护有序、互不重叠的带权键区间，不访问 PD/TiKV，也不执行 key rewrite 或 split RPC。

## 核心职责

- 用 `Value { Size, Number }` 表达一个键区间的近似字节量和近似条目数，并用 `join` 做分量相加。
- 用 `Valued { Key, Value }` 把半开键区间 `[StartKey, EndKey)` 与统计量绑定；空 `EndKey` 在比较逻辑中表示正无穷。
- 用 `SplitHelper` 的 `BTreeMap<Vec<u8>, Valued>` 按起始键维护全键空间的分段表示。`NewSplitHelper` 先插入空起止键、零权重的全空间哨兵，后续每次 `Merge` 都把命中的旧段裁切并重新写回。
- 在新范围跨越多个旧段时，将新增 `Value` 按旧段数量做整数均摊；在新范围切开旧段边界时，再对原值做二分或单段三分近似。该数据用于选择 split point，语义是与 Go 一致的估算，不是守恒的精确直方图。
- 提供升序、可提前终止的 `Traverse`，供内存统计与 `splitter.rs` 的 Region 分裂主链消费。

## 主要符号

- `Value { Size: u64, Number: i64 }`：片段权重。`Size` 是近似字节数，`Number` 是近似键/条目数。`Default` 为零值。
- `join(a, b) -> Value`：分别用 `wrapping_add` 累加两个字段，显式复现 Go 无符号/有符号整数溢出的回绕行为；`sum_sorted_test.rs::test_go_integer_overflow_semantics` 锁定此契约。
- `Span = KeyRange`：`crate::stubs::KeyRange` 的别名，表示相邻子键空间。
- `Valued`：带权半开区间。`NewValued` 是构造器；`MemSize` 估算两端 key、两个 `Vec` 元数据和两个数值字段的内存；`GetStartKey`/`GetEndKey` 返回克隆，并通过 `AppliedFile` trait 让 restore-utils 的通用范围逻辑可读取它；`Display`/`String` 输出区间、MB 与条目数。
- `SplitHelper { inner }`：核心有序分段结构。字段私有，外部只能经构造、合并和遍历 API 操作。
- `NewSplitHelper() -> SplitHelper`：建立仅含全空间零值哨兵的初态。空 `Vec` 同时作为最小起始键和无穷结束键的编码，具体含义由字段位置决定。
- `SplitHelper::Merge`：公开写入口；拒绝空起点或空终点的输入，收集所有重叠旧段后调用 `mergeWithOverlap`。
- `SplitHelper::Traverse`：按 `BTreeMap` 起始键顺序把克隆后的 `Valued` 交给回调；回调返回 `false` 时立即停止。
- `SplitHelper::overlapped`：从不晚于新范围起点的最右节点开始向右扫描，依靠有序性在首次不重叠时停止。
- `SplitHelper::mergeWithOverlap`：删除旧段、生成左右 trail、计算新增份额、合并权重并按起始键重新插入，是文件的核心算法。
- `checkOverlaps(a, ap) -> bool`：半开区间相交判断；第二个参数应是有限范围，首个范围允许空 `EndKey` 表示正无穷。

## 执行流程

1. 调用方先用 `NewSplitHelper` 建立状态。树内唯一节点是 `[空, 空)` 的零值哨兵；在本算法中它代表全键空间，因此任意合法的有限输入都会命中至少一个节点。
2. `Merge(val)` 要求 `val.Key.StartKey` 与 `EndKey` 都非空。任一为空便直接返回，既不报错也不改变哨兵。
3. `overlapped` 在 `BTreeMap` 中寻找 `StartKey <= val.StartKey` 的最后一个节点，并从那里升序扫描。对每个节点调用 `checkOverlaps`，首次不相交即停止，得到有序的旧片段列表。
4. `mergeWithOverlap` 先按旧片段起点从树中删除全部命中项。若列表为空则保持原状返回。
5. 新区间的权重按 `overlapped.len()` 做整数除法，形成 `appendValue`。除法余数被截断，这是 Go 实现的既有近似语义。
6. 若最左旧段从新区间左侧开始，算法保留 `[old.Start, val.Start)` 为 `leftTrail`，并把重叠主体起点推进到 `val.Start`。若最右旧段越过新区间终点，则保留 `[val.End, old.End)` 为 `rightTrail`，并截短主体终点。
7. 若同一个旧段同时被左右裁切，旧权重需要近似分给左、中、右三段。实现先把三者都调整为原值的 `2/3`（乘法使用 wrapping 语义），随后统一 `emit` 时各自再除以二，得到与 Go 算法一致的近似三等分。
8. `emit` 对 trail 或与 trail 相邻的主体做二分；对重叠主体再执行 `join(appendValue, oldShare)`。结果以片段 `StartKey` 为键重新插入树。
9. 消费方通过 `Traverse` 升序读取结果。`splitter.rs::SplitHelperIterator::Traverse` 进一步按表包装这些片段；`SplitPoint` 会跳过 `Size == 0` 或 `Number == 0` 的哨兵/零权重段，然后进行 key rewrite、Region 扫描与拆分点计算。

## 数据与状态

`SplitHelper::inner` 是全部持久状态；没有额外索引或缓存。其关键不变量是：键等于对应 `Valued.Key.StartKey`，节点按起始键有序，合并完成后的片段互不重叠，并且哨兵及其被切分后的边缘片段共同覆盖全键空间。算法会先收集、再删除、最后插入，避免在遍历 `BTreeMap` 时修改同一容器。

权重处理是有意的近似算法。跨 `n` 个片段的新增权重按 `n` 均分，而不是按键空间宽度分配；旧段被一条边界切开时按二分，同一旧段被两条边界切开时按三分。所有除法都是整数除法，因此会丢弃余数。`Size` 的加法与特殊三分前的乘法使用 wrapping 运算，`Number` 同样保留 Go 两补码回绕效果；这避免 Rust debug 构建因溢出 panic，但也意味着极端输入不会返回饱和值或错误。

`Valued::MemSize` 是估算值：计算两个 key 当前长度、两个 `Vec<u8>` 结构体大小及两个八字节数值，不包含分配器额外容量、`BTreeMap` 节点开销或树本身。`log_split_strategy.rs::maybeUpdateMemUsage` 遍历所有片段累加该值后上报指标，因此它适合趋势监测，不是精确堆内存计量。

## 依赖与调用关系

上游直接生产者有两条：

- `br/pkg/restore/log_client/log_split_strategy.rs::Accumulate` 按 `TableId` 懒创建 `SplitHelper`，把大于 `splitFileThreshold` 的日志文件 `StartKey`/`EndKey`、`Length` 和 `NumberOfEntries` 写入树；同文件 `maybeUpdateMemUsage` 还用 `Traverse` 与 `MemSize` 汇总指标。
- `br/pkg/restore/log_client/compacted_file_strategy.rs::Accumulate` 按重写后的有效表 ID 建树，从 SST 取得或改写键界，过滤空文件，将经 `impactFactor` 稀释且至少为一的大小/条目数写入树。

下游主消费者是 `br/pkg/restore/split/splitter.rs`：`RewriteSplitter` 把表级 rewrite 规则与一个 `SplitHelper` 绑定；`SplitHelperIterator::Traverse` 逐表调用本文件的 `Traverse`；`SplitPoint` 过滤零权重、执行 `GetRewriteEncodedKeys`、扫描 Region，并把相交片段交给具体 split 回调。RustCodeGraph 的 flow 证据直接显示 `splitter.rs:198` 的 `Traverse` 调用 `sum_sorted.rs:151` 的 `Traverse`。

直接类型依赖包括：标准库 `BTreeMap` 与 `fmt`；`astersql-br-pkg-restore-utils::AppliedFile`；以及本 crate `stubs` 中的 `KeyRange`、`CompareBytesExt`、`logutil`。`Cargo.toml` 声明 restore-utils 为路径依赖；本文件不直接依赖异步运行时、网络客户端或错误库。

## 错误处理与边界

本文件没有 `Result` 返回值，也不主动记录错误。非法或退化输入以确定性分支处理：`Merge` 对空起点或空终点静默忽略；`mergeWithOverlap` 对空重叠集合静默返回；`Traverse` 用布尔返回值支持调用方提前结束。

区间采用半开语义：`[a,b)` 与 `[b,c)` 相邻但不重叠。`checkOverlaps` 对普通有限范围使用 `a.Start < ap.End && ap.Start < a.End`；当 `a.EndKey` 为空时仅检查 `ap.End > a.Start`，把 `a` 视为延伸到正无穷。函数注释要求 `ap` 有限，当前公开 `Merge` 的非空终点检查保证内部调用满足这一前提。公开构造器本身不校验 `start < end`，调用方若传入逆序或空区间，通常会因找不到重叠而不改变树；这不是显式错误契约，扩展时不应把它误写成已验证输入。

`overlapped` 和 `mergeWithOverlap` 内部索引依赖“`NewSplitHelper` 建立哨兵且树不被外部直接破坏”。`inner` 私有阻止普通调用方绕过该不变量；但若将来新增反序列化或批量替换入口，必须保证非空树和有序、不重叠覆盖，否则 `overlapped[0]`、末元素索引及均分除数的安全前提会失效。

## 并发与资源生命周期

`SplitHelper` 不包含 `Mutex`、原子变量、任务或通道，也没有 `Send`/`Sync` 的定制实现。`Merge` 需要 `&mut self`，`Traverse` 只借用 `&self`，并发协调由所有者负责；当前上游策略在自己的累积阶段顺序修改表级 helper，随后在拆分阶段只读遍历。若未来跨线程共享，应在外层加锁或在阶段切换时转移所有权，不能假定本类型内部提供并发保护。

每次 `Merge` 都会克隆重叠片段和边界 key，删除旧 `BTreeMap` 节点后分配并插入新节点；生命周期完全由 Rust 所有权管理，没有显式 close/cleanup。`Traverse` 也会为每次回调克隆一个完整 `Valued`，这让回调不能修改树内状态，但大量片段或长 key 会带来分配与复制成本。回调返回 `false` 只停止本次遍历，不清理或改变 helper。

## 与 Go 版本的对应关系

Rust `Value`、`Span`、`Valued`、`SplitHelper` 以及 `NewValued`、`NewSplitHelper`、`Merge`、`Traverse`、`mergeWithOverlap`、`overlapped`、`checkOverlaps` 均与 `br/pkg/restore/split/sum_sorted.go` 同名或一一对应。Go 使用 `google/btree.BTree` 和 `Valued.Less` 排序；Rust 用 `BTreeMap<StartKey, Valued>` 把比较键显式放在容器键位。Go 的 `ReplaceOrInsert` 对应 Rust `insert`，二者都以起始键唯一标识片段。

Rust 为保持 Go 算术行为，在 `join` 以及单段三分前的乘法中使用 `wrapping_*`；这是 Rust 版本额外显式化的兼容点，并由独立 Rust 测试覆盖。Rust 的 `GetStartKey`/`GetEndKey` 必须克隆 `Vec<u8>`，而 Go 返回 slice；可观察的键内容一致，但 Rust 调用会产生复制。`MemSize` 中 Rust 用 `size_of::<Vec<u8>>()`，Go 用 slice header 的反射大小，意图相同，平台结构体尺寸由各语言 ABI 决定。

`br/pkg/restore/split/sum_sorted_test.go::TestSumSorted` 与 Rust `sum_sorted_test.rs::test_sum_sorted` 使用同一组重叠、相邻、嵌套和边界对齐用例，核对遍历后的 MB/条目序列及字符串格式。Rust 另外有 `test_go_integer_overflow_semantics`，专门证明回绕加法和三分路径与 Go 一致。当前 Rust 测试比 Go 测试多了遍历数量断言，仍保持同一行为契约。

## 扩展指南

- 若修改合并/均摊规则，应优先改 `SplitHelper::mergeWithOverlap`，并同步独立文件 `br/pkg/restore/split/sum_sorted_test.rs` 及 Go 对照 `sum_sorted_test.go` 的表驱动期望。要覆盖左 trail、右 trail、同段双 trail、跨多段、相邻不重叠、整数截断与溢出回绕。
- 若修改区间边界语义，应同时检查 `checkOverlaps`、`overlapped`、`Merge` 的有限范围前提，以及 `splitter.rs::SplitPoint` 对 key rewrite 和 Region 半开边界的处理；空 `EndKey` 的正无穷含义不能与普通空输入混淆。
- 若增加输入校验或错误返回，会改变当前静默忽略契约，并向 `log_split_strategy.rs`、`compacted_file_strategy.rs` 的 `Accumulate` 传播 API 改动；应先与 Go 版本对齐，而不是只在 Rust 侧收紧。
- 若优化性能，可考虑减少 `Traverse` 与重叠收集中的克隆，但必须保持回调不能破坏树内不变量、不能在遍历期间可变更新容器。任何改为借用的 API 都会影响 `SplitHelperIterator` 与 `SplitPoint` 的闭包生命周期。
- 若修改 `MemSize`，需明确它是指标估算，并同步检查 `log_split_strategy.rs::maybeUpdateMemUsage`；不要在未计入树节点、容量和分配器开销时声称为精确内存。
- 新增测试逻辑继续放在独立的 `sum_sorted_test.rs`，不要嵌回生产源文件；跨 Go/Rust 公共契约还可同步 `parity_test.rs`，但本文件的核心算法回归应保留在最近的独立测试中。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标文件被索引。
- RustCodeGraph `node --file br/pkg/restore/split/sum_sorted.rs --offset 1 --limit 360`：读取目标文件全部 281 行，核对所有类型、函数、方法、内部算法和注释。
- RustCodeGraph `explore "br/pkg/restore/split/sum_sorted.rs SumSorted NewSumSortedTree Traverse GetSplitKeys"`：确认 `splitter.rs::Traverse → sum_sorted.rs::Traverse` 调用边，并发现 `sum_sorted_test.rs`、`parity_test.rs` 的测试引用。精确 `callers` 子命令本次未输出文本，因此调用关系又用已索引调用方源码与仓库搜索交叉核对。
- RustCodeGraph `node`：读取 `br/pkg/restore/split/lib.rs`，确认模块装配、公开再导出和独立测试挂载；读取 `splitter.rs` 的 `RewriteSplitter`、`SplitHelperIterator::Traverse`、`PipelineRegionsSplitterImpl::ExecuteRegions` 与 `SplitPoint`；读取 `log_split_strategy.rs::Accumulate/maybeUpdateMemUsage` 和 `compacted_file_strategy.rs::Accumulate`。
- 直接读取 `br/pkg/restore/split/Cargo.toml`，确认 crate 名、`lib.rs` 入口、Go 包映射和 restore-utils 路径依赖。
- 直接读取 `br/pkg/restore/split/sum_sorted.go`、`sum_sorted_test.go` 与独立 Rust 测试 `sum_sorted_test.rs`，核对 API、合并算法、半开区间、字符串格式、表驱动期望和 Rust 的 Go 溢出兼容测试。
- 本任务仅产出文档，按计划不运行 Cargo；交付前使用任务指定命令验证文档存在且固定二级标题恰好为 11 个，并人工复核未把估算权重、内存估算或调用方约束描述成更强保证。
