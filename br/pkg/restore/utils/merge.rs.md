# `br/pkg/restore/utils/merge.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-utils`。包入口 [`lib.rs`](./lib.rs) 以 `pub mod merge` 挂载本模块，再通过 `pub use merge::*` 扁平导出其公共 API；[`Cargo.toml`](./Cargo.toml) 将该包标为 Go 包 `br/pkg/restore/utils` 的 Rust 移植，并声明对 `astersql-br-pkg-errors`、`astersql-br-pkg-rtree` 和 `astersql-errors` 的直接依赖。

它处在 BR 快照恢复的“备份 SST 元数据 → 可用于 Region 切分/散布的有序范围”转换位置：把同一原始键区间的 write/default CF 文件组成一个逻辑范围，先应用目标表键重写，再让 rtree 按大小和键数阈值合并相邻小范围。这样做的业务目的可由 Go 版本 [`merge.go`](./merge.go) 的函数注释核对：减少大量小 Region 恢复时的 split/scatter 开销。

当前 Rust 接线需要与设计归属分开看待。仓库文本调用搜索表明，此文件的 `MergeAndRewriteFileRanges` 目前由本 crate 的 [`merge_test.rs`](./merge_test.rs) 和 [`parity_test.rs`](./parity_test.rs) 直接调用；Rust [`../snap_client/tikv_sender.rs`](../snap_client/tikv_sender.rs) 虽有同名调用，但从其 `crate::stubs` 导入，实际落到 [`../snap_client/stubs.rs`](../snap_client/stubs.rs) 的同名副本，而不是本文件。Go 生产链则由 [`../snap_client/tikv_sender.go`](../snap_client/tikv_sender.go)、`br/pkg/task/restore_raw.go` 和 `br/pkg/task/restore_txn.go` 调用本路径的 Go 实现。因此，本文件是 canonical restore-utils Rust API，但尚不能据现有调用证据声称已接入 Rust 快照恢复生产链。

## 核心职责

本文件只承担一条聚焦的数据整理流水线，核心入口是 `MergeAndRewriteFileRanges`：

1. 空输入直接返回空范围和全零统计。
2. 按 `StartKey` 将备份文件分组，并要求同组文件拥有相同 `EndKey`。
3. 识别 write/default CF，累计文件数、字节数、KV 数和逻辑 Region 数。
4. 将每组 `backuppb::File` 转换为 rtree 的 `Range`/`File`，通过 `RewriteRange` 把旧表键空间改写到目标键空间。
5. 把改写后的范围插入 `RangeStatsTree`，拒绝重复起始键。
6. 调用 `MergedRanges` 按 `splitSizeBytes` 与 `splitKeyCount` 合并可相邻合并的小范围，并返回合并结果及前后统计。

函数不读取 SST 内容，也不访问 PD、TiKV、网络或磁盘；它操作的是已加载到内存的文件元数据。范围是否能相邻合并、是否跨表/跨索引等更深层规则由 `astersql-br-pkg-rtree` 的 `RangeStatsTree::MergedRanges`/`NeedsMerge` 决定，本文件负责准备有序树所需的范围和权重。

## 主要符号

- `fn br_err(err: &'static astersql_errors::Error) -> SharedError`：内部适配器，把 BR 包的静态错误克隆为 `SharedError`，以便传给 `Annotatef`。它不公开，也不改变错误分类。
- `pub struct MergeRangesStat`：合并过程的值类型统计。`TotalFiles` 是输入文件总数；`TotalWriteCFFile`/`TotalDefaultCFFile` 是识别出的 CF 文件数；`TotalRegions` 取两类 CF 文件数的较大值，以兼容 write+default 成对的 TxnKV 和只有 default 的 RawKV；`Region*Avg` 是合并前平均值；`MergedRegions` 与 `MergedRegion*Avg` 描述合并结果。类型派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，字段均为公开的 `i32`。
- `fn to_rtree_files(files: &[backuppb::File]) -> Vec<RtreeFile>`：内部逐项复制适配器，只把 `Name`、`Cf`、起止键、字节数、KV 数和 CRC 字段投影到 rtree 文件类型。它不改变键，不聚合统计。
- `pub fn MergeAndRewriteFileRanges(files, rewriteRules, splitSizeBytes, splitKeyCount) -> Result<(Vec<RangeStats>, MergeRangesStat), SharedError>`：唯一公共行为入口。它取得输入 `Vec` 的所有权，重写规则只读借用，成功时返回按起始键排序并可能合并的范围及统计。

文件没有模块级常量、trait、impl、条件编译项或异步入口。非 snake_case 命名来自 Go API 对齐，包入口的 crate 级 `allow` 负责兼容这种命名风格。

## 执行流程

`MergeAndRewriteFileRanges` 的具体控制流如下：

1. `files.is_empty()` 时短路，既不检查 CF，也不构造树，返回 `Vec::new()` 和 `MergeRangesStat::default()`。
2. 保存 `files.len()`，遍历输入并以克隆的 `StartKey: Vec<u8>` 作为 `HashMap` 键。每次 push 后比较桶内首文件和末文件的 `EndKey`；不一致会调用 `crate::stubs::log::Panic`，这是数据不变量失败而非普通 `Result` 错误。
3. CF 分类优先比较 `file.Cf` 与 `WriteCFName`/`DefaultCFName`；若字段不匹配，再以文件名是否包含对应字符串作为历史备份兼容回退。无法识别的单个文件不会立即报错，但如果整个输入中 write/default 计数都为零，则返回 `ErrRestoreInvalidBackup`。
4. `TotalRegions = max(defaultCFFile, writeCFFile)`。这使一对起止键相同的 write/default 文件计为一个逻辑 Region，也允许 RawKV 仅凭 default CF 得到非零 Region 数。
5. 对每个 `HashMap` 桶累加 `TotalBytes`/`TotalKvs`，用首文件的起止键构造 `Range`，并用 `to_rtree_files` 保留桶内所有文件。虽然 `HashMap::into_values()` 的遍历顺序不稳定，后续 `RangeStatsTree` 使用按 `StartKey` 排序的树，因此最终输出顺序由树而不是哈希顺序决定。
6. `RewriteRange(&mut rg, rewriteRules)` 在无规则时返回原范围克隆；有规则时改写起止键。任何重写错误在这里统一包装为 `ErrInvalidRange`，并附上该组文件上下文。
7. `RangeStatsTree::InsertRange` 以改写后的 `StartKey` 为键。若返回旧值，说明已有相同起始键，函数以 `ErrInvalidRange` 结束；因此键重写可能把原本不同的组映射成重复范围，这也会被拒绝。
8. `MergedRanges` 按起始键顺序扫描。rtree 负责判断相邻范围是否属于可合并的同类键空间且累加后不越过大小/键数阈值；可合并时扩展终点、累加 `Size`/`Count` 并拼接文件列表。
9. 使用输入总字节/KV 数分别除以合并前 `TotalRegions` 和合并后范围数，构造最终统计。合并结果为空时，Rust 实现把合并后均值显式置零。

## 数据与状态

- 输入 `backuppb::File` 是本 crate [`stubs.rs`](./stubs.rs) 暴露的备份文件元数据。函数消费输入向量，但会克隆名称、CF 和键到 rtree 文件对象；调用完成后不存在对输入的借用。
- `filesMap: HashMap<Vec<u8>, Vec<backuppb::File>>` 是第一阶段临时状态。键只使用 `StartKey`，而 `EndKey` 一致性通过显式不变量检查保证。
- `totalBytes`、`totalKvs` 统计所有输入文件，包括未被识别为 write/default 的文件；CF 计数只统计识别成功者。因此混合“合法 CF + 未知 CF”的输入不会触发全局非法备份错误，未知文件仍会进入范围树并计入字节/KV 总量。这与当前 Go 实现一致，是扩展 CF 识别时需要特别留意的兼容行为。
- 每个组的 `rangeSize`/`rangeCount` 是组内所有 CF 文件的总和，并作为 `RangeStatsTree` 的合并权重；`Range.Files` 则保留组成该逻辑范围的每个文件。
- 统计字段最终从 `usize`/`u64` 转为 `i32`。当前代码使用 `as i32`，超出 `i32` 的文件数、Region 数或平均值会按 Rust 强制转换语义截断，而不会返回溢出错误。
- 关键不变量是：空输入安全；非空且至少识别一个合法 CF 才进入平均值计算；同一 `StartKey` 的所有文件必须具有相同 `EndKey`；改写后的起始键不能重复。

## 依赖与调用关系

上游与暴露关系：

- [`lib.rs`](./lib.rs) 声明并重新导出本模块，所以外部 crate 可通过 `astersql_br_pkg_restore_utils::MergeAndRewriteFileRanges` 使用入口。
- RustCodeGraph 对本目录建立了 17 个文件的索引，并识别本文件 5 个符号；文件级结果显示 [`parity_test.rs`](./parity_test.rs) 使用本文件。补充文本搜索确认 [`merge_test.rs`](./merge_test.rs) 也直接调用公共入口。
- 当前 Rust `snap_client` 没有在 [`../snap_client/Cargo.toml`](../snap_client/Cargo.toml) 中依赖 `astersql-br-pkg-restore-utils`，其 `tikv_sender.rs` 使用本地 stubs 同名实现；这是现有迁移边界，不应把名字相同误判为调用本文件。
- Go 侧 [`../snap_client/tikv_sender.go`](../snap_client/tikv_sender.go) 在 `SortAndValidateFileRanges` 中先验证每个文件的 rewrite rule，再调用 Go `restoreutils.MergeAndRewriteFileRanges`，消费返回统计并继续生成稳定 split keys/file groups。`br/pkg/task/restore_raw.go` 与 `br/pkg/task/restore_txn.go` 也消费 Go 入口。

下游依赖：

- `crate::misc::{WriteCFName, DefaultCFName}`：统一 CF 名称。
- `crate::rewrite_rule::{RewriteRange, RewriteRules}`：执行目标键空间前缀重写。`RewriteRange` 无规则时保持原值；规则下起止表 ID 不同会返回表 ID 不匹配错误，缺少匹配规则则记录警告并继续。
- `crate::stubs::backuppb`：提供 Rust 侧备份文件元数据类型；`crate::stubs::log::Panic` 承载不变量失败。
- `astersql_br_pkg_rtree::{Range, RangeStats, RangeStatsTree}`：有序插入、重复起始键检测和相邻范围合并。`InsertRange` 的重复判定基于 `StartKey`，`MergedRanges` 的实际合并约束在 rtree crate 内。
- `astersql_br_pkg_errors::{ErrInvalidRange, ErrRestoreInvalidBackup}` 与 `astersql_errors::{Annotatef, SharedError}`：保留可按错误身份判断的 BR 错误并补充上下文。

## 错误处理与边界

- 空输入成功返回，不产生除零，也不要求 rewrite rules。
- 所有文件的 CF 都无法通过字段或名称识别时，返回带固定说明的 `ErrRestoreInvalidBackup`。文案中的 `Wrtie` 拼写沿用 Go 版本，不应依赖文案做错误分类；[`merge_test.rs`](./merge_test.rs) 用 `ErrRestoreInvalidBackup.Equal` 验证错误身份。
- 同一 `StartKey` 对应不同 `EndKey` 时走 `log::Panic`。该行为对齐 Go 的 `log.Panic`，调用方不能靠 `Result` 恢复；输入元数据进入本函数前应保证成对 CF 范围一致。
- `RewriteRange` 的具体错误被丢弃后重标为 `ErrInvalidRange`，上下文包含整组文件。若调用方需要区分表 ID 不匹配等底层原因，当前 API 不保留该错误链细节。
- 插入树时发现相同改写起始键会返回 `ErrInvalidRange`。当前 rtree `InsertRange` 实际只以 `StartKey` 检测替换，并不在本层显式比较任意交叠区间；文档和新代码不应把它扩大解释为完整的区间重叠检测。
- `splitSizeBytes` 或 `splitKeyCount` 为零的精确行为委托给 rtree 的 `NeedsMerge`；本文件不校验阈值合法性。
- Rust 版本对 `sortedRanges.is_empty()` 做零均值保护，而 Go 当前实现直接以 `len(sortedRanges)` 为除数。这是防御性差异；正常非空合法输入预计至少产生一个树节点，但扩展过滤/合并逻辑时应保留 Rust 的除零保护。

## 并发与资源生命周期

函数是同步、单线程、无共享可变状态的纯内存计算。它不创建线程、异步任务、锁、channel、事务、文件句柄或网络连接；错误返回或 panic 时，局部 `HashMap`、范围树和克隆的文件元数据由 Rust 所有权机制自动释放。

并发调用之间没有本文件级状态竞争：每次调用各自拥有输入 `Vec` 和临时树，`RewriteRules` 只读借用。是否可跨线程调用最终取决于输入/依赖类型的 `Send`/`Sync`，本文件没有额外同步约束。

资源成本主要来自内存和复制：分组持有原 `File`，`to_rtree_files` 再克隆名称、CF 和键；`RewriteRange` 返回克隆；rtree 合并时还会克隆 `RangeStats` 并追加克隆的文件列表。排序成本由 rtree 的 `BTreeMap` 插入承担。Go 基准覆盖 100 至 100000 个文件，Rust 独立测试把 100、1000、10000 规模改为单次正确性测试；在扩大数据规模或减少克隆时，应以行为对齐和内存峰值为主要性能风险。

## 与 Go 版本的对应关系

Rust [`merge.rs`](./merge.rs) 与 Go [`merge.go`](./merge.go) 在公开数据模型和主流程上逐段对应：统计字段一致；空输入短路一致；按 `StartKey` 分组并断言 `EndKey` 一致；CF 字段优先、文件名回退；`TotalRegions` 取 write/default 较大值；先 rewrite 后插树；重复范围返回 `ErrInvalidRange`；最终由 rtree 按两个阈值合并。

已确认的实现形态差异包括：

- Go 输入是 `[]*backuppb.File`，返回 `[]rtree.RangeStats`、`*MergeRangesStat` 和 `error`；Rust 消费 `Vec<backuppb::File>`，返回元组值和 `SharedError`。
- Rust 需要 `to_rtree_files` 把本 crate 的 stub 文件类型投影为 rtree 文件类型；Go 两层直接共享 kvproto 文件指针。
- Go 通过 `bytes.Equal` 比较键并用字符串键建 map；Rust 用 `Vec<u8>` 的值相等和哈希实现相同语义。
- Rust 在合并后范围数为零时把平均值设为零，避免 Go 版本潜在的除零路径。
- Rust 的整数统计固定为 `i32`，Go 的 `int` 宽度取决于平台；极大输入下边界并不完全相同。

测试对照也保持独立文件：[`merge_test.rs`](./merge_test.rs) 镜像 [`merge_test.go`](./merge_test.go) 的空输入、阈值边界、同表合并、跨表/跨索引隔离、乱序输入、RawKV default-only、未知 CF 和规模路径。Rust 规模测试只保留 100/1k/10k 的单次执行，没有移植 Go 50k/100k benchmark；这属于测试形态/覆盖规模差异，不应据此宣称生产性能已经等价。`parity_test.rs` 另提供空、非法 CF、正常 write 的门面级冒烟覆盖。

## 扩展指南

- 增加或改变 CF 识别时，修改 `MergeAndRewriteFileRanges` 的分类分支，并同步考虑“字段匹配”和“文件名包含”两条兼容路径、RawKV 只有 default CF 的语义、未知 CF 混入合法输入时的现状。对应回归应写入独立的 [`merge_test.rs`](./merge_test.rs)，并与 [`merge_test.go`](./merge_test.go) 的意图保持一致，不把测试嵌入源文件。
- 改变范围分组键或 write/default 配对规则时，必须维护“同组起止键一致”和 `TotalRegions` 的含义；重点补充同起点异终点、单 CF、双 CF、多文件同组以及改写后碰撞测试。
- 改变键重写行为应优先落在 [`rewrite_rule.rs`](./rewrite_rule.rs) 的 `RewriteRange`，本文件只负责调用和错误上下文化。需要保留原始错误分类时，应明确调整错误链并同步验证现有 `ErrInvalidRange` 契约。
- 改变相邻范围能否合并或阈值边界时，应修改 `astersql-br-pkg-rtree` 的 `NeedsMerge`/`MergedRanges` 并同步其独立测试；本文件只传递 `rangeSize`、`rangeCount` 和阈值。跨表、跨索引以及“恰好等于阈值”的用例是兼容关键点。
- 若把 Rust 快照恢复生产链切换到本 canonical crate，应先移除或收敛 `snap_client/stubs.rs` 的同名副本，在 `snap_client/Cargo.toml` 增加明确依赖，并核对两边的 `backuppb::File`、`RewriteRules`、`RangeStats` 类型是否同源。仅替换 import 名称不足以证明接线完成。
- 优化复制或整数类型时，风险分别是所有权生命周期变化、文件列表顺序/内容变化以及统计溢出语义变化。至少同步 `merge_test.rs`、`parity_test.rs`，并在生产接线后补充 `snap_client` 侧调用测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/utils` 列出本目录 17 个索引文件；`node --file br/pkg/restore/utils/merge.rs --offset 1 --limit 400` 读取目标文件 195 行并报告 5 个符号；`query MergeAndRewriteFileRanges --kind function --json` 区分了 Go 实现、本文件实现和 `snap_client/stubs.rs` 同名实现。精确 `callers/callees` 未产生可用输出，因此调用边用下述源码搜索补齐，没有把缺失图边当成事实。
- 目标与模块：[`merge.rs`](./merge.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)。
- 下游实现：[`rewrite_rule.rs`](./rewrite_rule.rs) 的 `RewriteRange`；`br/pkg/rtree/rtree.rs` 的 `RangeStatsTree::InsertRange`、`RangeStatsTree::MergedRanges` 与 `NeedsMerge`；`br/pkg/rtree/Cargo.toml`。
- Rust 调用/接线：[`merge_test.rs`](./merge_test.rs)、[`parity_test.rs`](./parity_test.rs)、[`../snap_client/tikv_sender.rs`](../snap_client/tikv_sender.rs)、[`../snap_client/stubs.rs`](../snap_client/stubs.rs) 和 [`../snap_client/Cargo.toml`](../snap_client/Cargo.toml)。
- Go 对照与调用：[`merge.go`](./merge.go)、[`merge_test.go`](./merge_test.go)、[`../snap_client/tikv_sender.go`](../snap_client/tikv_sender.go)、`br/pkg/task/restore_raw.go`、`br/pkg/task/restore_txn.go`。
- 人工复核结论：本文能够回答文件存在目的、从分组到重写再到有序合并的运行过程、错误/不变量、当前 Rust 未接入生产发送链的限制，以及扩展时应修改的符号和独立测试位置。任务是纯文档分析，按计划不运行 Cargo。
