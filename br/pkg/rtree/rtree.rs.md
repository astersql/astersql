# `br/pkg/rtree/rtree.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-rtree` 的区间算法主体，真实源码见 [`rtree.rs`](./rtree.rs)。包入口 `br/pkg/rtree/lib.rs` 以 `pub mod rtree` 挂载它，并通过 `pub use rtree::*` 扁平导出公开类型和函数；`br/pkg/rtree/Cargo.toml` 声明这是对应 Go 包 `br/pkg/rtree` 的 library crate。源码直接对照 `br/pkg/rtree/rtree.go`，使用 `BTreeMap<Vec<u8>, _>` 承担 Go `google/btree` 的有序索引、同起点替换和升序遍历职责。

当前生产接线中，`br/pkg/restore/utils/merge.rs::MergeAndRewriteFileRanges` 直接依赖本 crate，并调用 `NewRangeStatsTree`、`RangeStatsTree::InsertRange` 和 `RangeStatsTree::MergedRanges` 合并恢复文件区间。`br/pkg/backup/client.rs` 虽有同名 `ProgressRangeTree` 调用链，但其 `rtree` 来自备份 crate 自己的 `stubs`，不能据此认定该备份主链已经接到本文件；本文件的进度树公开面目前主要由本 crate 的独立测试验证。

## 核心职责

文件将职责分成三层：

1. `KeyRange`、`Range` 和 `RangeStats` 定义半开键区间 `[StartKey, EndKey)`、关联文件及恢复合并统计；空 `EndKey` 统一表示正无穷。
2. `RangeStatsTree` 按 `StartKey` 排序恢复区间，并在大小、键数及表/索引编码均允许时合并相邻项；核心判定是 `NeedsMerge`。
3. `RangeTree` 维护“不重叠的已完成区间”，负责覆盖写入和计算请求范围内的未覆盖空洞；`ProgressRangeTree` 再以原始请求区间为单位汇总多个 `RangeTree`，在请求完成时写出文件元数据、触发回调并按物理表聚合 checksum。

因此它既提供恢复侧的文件区间合并，也保存了备份侧进度算法的 Go 对齐实现；两部分共享相同的字节序区间语义，但当前 Rust 生产调用范围并不相同。

## 主要符号

- `KeyRange { StartKey, EndKey }`：公共半开区间。`Contains` 判断单键且排除有限上界，`ContainsRange` 判断整个子区间，`Intersect` 返回交集及是否相交。
- `Range { KeyRange, Files }`：一次备份响应或恢复文件组。它通过 `Deref<Target = KeyRange>` 暴露区间字段；`BytesAndKeys` 汇总文件的 `TotalBytes`/`TotalKvs`，`Less` 按起点排序。
- `RangeStats { Range, Size, Count }`：恢复合并单元。`Size`、`Count` 是调用者提供的区间统计，`NeedsMerge` 的阈值判断则读取 `Range::BytesAndKeys` 的文件统计；合并时两套统计都会分别累计。
- `RangeStatsTree`、`NewRangeStatsTree`：`StartKey → RangeStats` 的有序映射。`InsertRange` 对同起点执行替换并返回旧值；`MergedRanges` 顺序扫描并构造合并结果。
- `NeedsMerge`：先处理右侧零字节特例，再检查合并后的文件字节数和键数是否超过阈值，最后经 `parse_inner_key` 限制为同表记录或同表同索引。
- `RangeTree { BTreeG, PhysicalID }`：不重叠完成区间集合。`Get`/`Delete`/`ReplaceOrInsert` 是按起点的基础操作，`Find` 是 floor 查询后再做包含校验，`Update`/`Put`/`PutForce` 通过 `updateForce` 处理重叠，`GetIncompleteRange` 计算空洞。
- `NewRangeTreeWithFreeListG`：保留 Go 构造签名和 `PhysicalID`，但 Rust 的 `FreeListG` 参数不参与节点复用。
- `ProgressRange { Res, Origin }`：一个原始请求及其已完成子区间树。
- `ProgressRangeTree`：保存进度项、`checksumMap`、`skipChecksum`、可选 `MetaWriter` 和完成回调。公开入口包括 `Insert`、`FindContained`、`GetIncompleteRanges`、`SetCallBack`、`GetChecksumMap` 和 `UpdateChecksum`。
- `DeletedRange`：`GetIncompleteRanges` 两阶段处理中的暂存项，保存稍后删除的起点键与预计算 checksum。

## 执行流程

恢复合并路径从 `MergeAndRewriteFileRanges` 开始：调用者先按原始 `StartKey` 将 write/default CF 文件分组并改写键，再将每组包装成 `Range` 传给 `RangeStatsTree::InsertRange`。`MergedRanges` 按起点升序取项：第一项建立目标；后续项若 `NeedsMerge` 为真，就把右端点改为当前项的 `EndKey`，以 wrapping 加法累计 `Size`/`Count` 并追加 `Files`；否则开始新的目标段。`NeedsMerge` 会先拒绝超大小或超键数的组合，再解码起点中的 table/index 头，防止跨表、跨索引或记录/索引混合合并。

完成区间更新路径由 `RangeTree::Put` 或 `Update` 构造/接收 `Range` 后进入 `updateForce`。它先用 `Find` 找到可能从查询起点左侧延伸进来的区间，再向右收集起点小于新范围有限上界的所有项。非强制写入遇到任一重叠即返回 `false` 且不修改树；强制写入删除全部收集项，再以新范围的 `StartKey` 插入。邻接区间因半开语义不算重叠。

空洞扫描由 `GetIncompleteRange(startKey, endKey)` 完成。非空且相等的端点直接代表空请求；否则它从包含请求起点的既有项（若有）开始升序扫描，用 `lastEndKey` 记录已覆盖前沿。当前项起点在前沿右侧时，通过 `requestRange.Intersect` 裁剪并输出中间空洞；到达请求上界后停止；最后再判断是否需要补尾部空洞。空树或找不到扫描项时会返回整个请求区间。

进度汇总时，`ProgressRangeTree::GetIncompleteRanges` 对每个 `Origin` 调用其 `Res.GetIncompleteRange`。仍有空洞就追加到返回值；完全覆盖则调用 `collectRangeFiles`，依次 `SummaryFiles`、`MetaWriter::Send(..., AppendDataFile)`，缓存 checksum 并触发完成回调。遍历结束后才删除已完成项；未开启 `skipChecksum` 时，再以 `Res.PhysicalID` 调用 `UpdateChecksum` 聚合结果。

## 数据与状态

所有排序均采用 `Vec<u8>` 的字典序，与 Go `bytes.Compare` 对应。三个树都把起点克隆为 `BTreeMap` 的键，因此同一 `StartKey` 只能保存一个值；基础 `InsertRange`/`ReplaceOrInsert` 的“重叠”返回值实际仅表示同起点替换，更广义的区间重叠由 `RangeTree::getOverlaps` 处理。

`RangeTree` 的核心不变量是存储区间互不重叠，但 `InsertRange` 明确不清理重叠，调用者必须自行维护不变量；`Put` 和 `Update` 则总是强制覆盖。覆盖不是区间切割：只要旧项与新项重叠，旧项整体删除，旧项未被新项覆盖的左右残片不会保留。这一点由 `rtree_test.rs::test_range_tree` 和 `test_range_tree_put_force` 的长度、空洞与文件名断言固定。

`RangeStatsTree::MergedRanges` 不修改原树，而是克隆出新的 `Vec<RangeStats>`。文件字节/键统计、合并后的 `Size`/`Count` 以及 checksum 的计数使用 `wrapping_add`，显式保持 Go `uint64` 模 2^64 的溢出语义；CRC 使用异或。

`ProgressRangeTree` 的 `BTreeG` 以 `Origin.StartKey` 为键。已完成项在一次 `GetIncompleteRanges` 调用成功结束时被移出，因此完成回调对该项只触发一次；`checksumMap` 留存累计结果。`metaWriter=None` 时完成项仍会删除并回调，但 `collectRangeFiles` 返回零统计，不发送文件。

## 依赖与调用关系

本文件只直接使用标准库 `BTreeMap`、`Deref`/`DerefMut`，以及 `crate::stubs` 中的 `File`、`RpcKeyRange`、`ChecksumStats`、`MetaWriter`、`FreeListG`、`SummaryFiles`、`DecodeKeyHead`、`redact_key` 和 `AppendDataFile`。这些桩隔离了 kvproto、metautil、tablecodec 等完整 Go 依赖；`Cargo.toml` 虽声明 `astersql-br-pkg-logutil`，本文件本身没有直接引用它。

已核实的本文件内部调用边包括：`MergedRanges → NeedsMerge → Range::BytesAndKeys/parse_inner_key → stubs::DecodeKeyHead`；`Put/PutForce/Update → updateForce → getOverlaps → Find → KeyRange::Contains`；`GetIncompleteRange → Find/KeyRange::Intersect`；`GetIncompleteRanges → RangeTree::GetIncompleteRange/collectRangeFiles/UpdateChecksum`；`collectRangeFiles → stubs::SummaryFiles/MetaWriter::Send`。

已核实的 Rust 生产上游是 `br/pkg/restore/utils/merge.rs::MergeAndRewriteFileRanges`，其 Cargo manifest 通过路径依赖 `../../rtree` 引用本 crate。`br/pkg/rtree/lib.rs` 是本 crate 的公开门面。`br/pkg/backup/client.rs` 当前使用自身 `stubs` 中的同名 API，不是本文件的直接生产上游；若以后替换该桩，需要单独处理其 `Arc`、可变性和文件类型适配，不能仅改 import。

## 错误处理与边界

纯区间操作不返回错误：`Intersect` 用布尔值区分无交集，`Find`/`Get`/`Delete` 用 `Option`，`PutForce(false)` 用 `false` 表示拒绝重叠。空 `EndKey` 是正无穷而不是空集合；仅 `startKey == endKey && !startKey.is_empty()` 被 `GetIncompleteRange` 当作空请求，`([], [])` 则表示全键空间请求。

`NeedsMerge` 的 key head 解码失败会保守返回 `false`；与 Go 版相比，Rust 当前不会记录 warning。右侧文件总字节为零时在解码和阈值判断前直接允许合并，这是需要保持的既有优先级。API V2 键仅在首字节为 `x` 且至少四字节时尝试剥除四字节 keyspace 前缀；剥除后解码失败会回退到原键解码。

`ProgressRangeTree::Insert` 只通过 floor 项检查“新起点落入已有 Origin”的冲突；它依赖已有进度区间互不重叠这一前置条件。`FindContained` 找不到包含起点的项返回 `Ok(None)`，找到但不能完整包含请求终点才返回 `Err(String)`。错误文本里的键由 `redact_key` 处理，唯有 region 起点在一处仍以调试格式输出。

`collectRangeFiles` 在任一 `MetaWriter::Send` 失败时立即返回错误；`GetIncompleteRanges` 随即停止，不删除本轮已收集的完成项，也不更新其 checksum，但在错误发生前已执行的 writer 发送和完成回调属于不可回滚副作用。内部对“从当前树收集的键仍存在”使用 `expect`，其成立依赖遍历阶段没有并发修改。

## 并发与资源生命周期

这些树没有内部锁，也没有 `Send`/`Sync` 包装；所有修改接口都需要 `&mut self`，并发协调应由调用者负责。`completeCallBack` 要求 `Fn() + Send`，`MetaWriter` 是 trait object，但本文件不会创建线程或异步任务，回调和 `Send` 均在调用 `GetIncompleteRanges` 的线程上同步执行。

`GetIncompleteRanges` 先克隆键列表，再以只读借用检查每项，最后统一删除，避免在 `BTreeMap` 迭代期间修改结构。完成项的生命周期因此是“检测完整 → 写出文件并计算统计 → 回调 → 遍历成功后删除 → 可选聚合 checksum”。若写出中途失败，树仍保留完成项，后续重试可能再次发送此前已经成功发送的文件，调用方或 writer 需考虑幂等性。

Rust 的 `NewRangeTreeWithFreeListG` 接受但忽略 `FreeListG`；与 Go 通过 free list 复用 B-tree 节点相比，这里没有对应的内存池生命周期或分配优化。`MergedRanges` 和 `getOverlaps` 会克隆区间及文件向量，在大文件列表上可能增加内存和复制成本。

## 与 Go 版本的对应关系

类型和主算法逐项对应 `br/pkg/rtree/rtree.go`：`KeyRange`、`Range`、`RangeStatsTree`、`NeedsMerge`、`RangeTree`、`ProgressRangeTree` 及方法命名均保留 Go 风格，`lib.rs` 也允许非 snake case。`BTreeMap` 取代泛型 `btree.BTreeG`；floor 查询由 `range(..=key).next_back()` 实现，升序遍历由 map value 顺序实现。

主要语义保持一致：半开区间和无限上界、强制覆盖整项、空洞裁剪、相同表记录/相同表索引的合并限制、完成后 meta 写出与 checksum 聚合，以及 Go `uint64` wrapping 行为。`rtree_test.rs` 明确对照 Go `rtree_test.go`，覆盖空树和全空间、覆盖与邻接、交集、普通/V2 表键合并、进度回调、writer/checksum；`parity_test.rs::go_rust_public_contract_matches` 额外串联公开 API。

当前差异包括：Rust `Range.Files` 持有值而非 Go 指针；`FreeListG` 被忽略；Go 解码失败会写 warning，Rust 只返回 `false`；Go `DeletedRange` 保存进度项指针，Rust 保存起点键并在第二阶段重新 remove；Go 使用具体 `metautil.MetaWriter`，Rust 使用本地 `MetaWriter` trait；Rust 通过 wrapping 运算显式复现 release/debug 均一致的 Go 溢出行为。另一个重要迁移状态差异是备份 Rust crate 尚使用自己的同名桩实现，而恢复合并已直接消费本 crate。

## 扩展指南

新增区间几何行为时，应优先修改 `KeyRange::{Contains, ContainsRange, Intersect}`，同时在独立的 `br/pkg/rtree/rtree_test.rs` 增加有限/无限端点、相等端点和邻接边界用例；不要把测试写回生产文件。调整覆盖策略时，应集中修改 `RangeTree::getOverlaps`/`updateForce`，并明确是否仍采用“整项删除”而非切割保留残片，否则会改变空洞重试语义。

调整恢复合并规则时，应修改 `NeedsMerge` 或 `parse_inner_key`，同步验证记录/索引、跨 table/index、阈值等于/超过、右侧零字节、解码失败和 API V2 keyspace 键；还需检查 `br/pkg/restore/utils/merge.rs` 的统计与错误契约。若阈值要改为使用 `RangeStats.Size/Count`，必须先确认 Go 上游，因为当前判定使用文件的 `TotalBytes/TotalKvs`，而 `Size/Count` 只在产出合并项时累计。

扩展进度完成副作用时，应保持 `GetIncompleteRanges` 的两阶段删除，清楚定义 writer 失败后的重试/幂等规则，并在 `rtree_test.rs` 使用独立 `MetaWriter` 替身覆盖成功和失败。若把 `br/pkg/backup/client.rs` 从本地桩切换到此 crate，需要额外设计共享可变访问（当前备份接口多处从共享引用修改）、`Arc<dyn MetaWriter>` 与 `Box<dyn MetaWriter>` 的所有权适配，以及 backuppb 文件类型转换。

性能扩展应重点评估 `BTreeMap` key/value 克隆、`getOverlaps` 的整段克隆、`MergedRanges` 的文件向量复制，以及忽略 free list 的影响；任何优化都需保留字节排序、回调提前停止和同起点替换语义。

## 验证依据

- RustCodeGraph 索引状态：项目含 7,032 个 Rust 文件；以 `node --file` 完整读取 `br/pkg/rtree/rtree.rs`（731 行）、`br/pkg/rtree/lib.rs`、`br/pkg/rtree/rtree_test.rs` 和 `br/pkg/rtree/parity_test.rs`，并以 `explore` 核对主要符号及调用关系。
- crate 边界：`br/pkg/rtree/Cargo.toml` 的 package 名、library 入口和 Go package metadata；根 `Cargo.toml` workspace 成员 `br/pkg/rtree`；`br/pkg/rtree/lib.rs` 的模块挂载、公开再导出和独立测试挂载。
- Go 对照：`br/pkg/rtree/rtree.go` 中对应的全部类型和算法；测试意图由同路径 `br/pkg/rtree/rtree_test.go` 的 Rust 对照文件注释及断言交叉确认。
- Rust 测试：`br/pkg/rtree/rtree_test.rs::{test_range_tree, test_range_tree_put_force, test_range_intersect, test_range_tree_merge, test_progress_range_tree, test_progress_range_tree_call_back, test_progress_range_tree_call_back2, test_uint64_accumulators_match_go_wrapping_semantics}`；`br/pkg/rtree/parity_test.rs::go_rust_public_contract_matches`。
- 生产上游：`br/pkg/restore/utils/merge.rs::MergeAndRewriteFileRanges` 及 `br/pkg/restore/utils/Cargo.toml`；以仓库搜索确认备份 crate 的同名调用来自 `br/pkg/backup/stubs.rs`，未将其误列为本文件直接调用边。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅执行任务指定的 11 章节结构验证，并人工复核文档未把未接线进度树写成已接线生产主链。
