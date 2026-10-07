# `lightning/pkg/importer/dup_detect.rs`

## 文件定位

本文件属于 `astersql-lightning-pkg-importer` library crate；crate 入口 `lightning/pkg/importer/lib.rs` 以 `mod dup_detect` 装配并通过 `pub use dup_detect::*` 再导出其符号。它处在 Lightning 单表导入的预去重阶段：`lightning/pkg/importer/table_import.rs` 的 `TableImporter::preDeduplicate` 构造 `dupDetector`，再调用 `dupDetector::run`。本文件不负责把最终 KV 导入 TiKV，而是重新读取 checkpoint 中的数据 chunk，编码出可能受唯一约束影响的 KV，找出重复项，并按冲突策略报错或生成“应忽略行”记录。

`lightning/pkg/importer/Cargo.toml` 表明该 crate 对齐 Go package `lightning/pkg/importer`，直接依赖 checkpoint crate `astersql-lightning-pkg-checkpoints`；解析、编码、重复检测、外排、日志和表模型接口则由 crate 内模块提供。当前 Rust 上游仅搜索到 `TableImporter::preDeduplicate` 这一处生产构造入口；该方法本身未在 Rust 生产代码中搜索到调用点，因此本文只确认该局部接线，不推断完整 Rust 导入主链已经启用。

## 核心职责

1. `dupDetector::run` 建立临时磁盘 sorter 和 `duplicate::Detector`，先收集全部待检查键，再按 `Conflict.Strategy` 执行重复检测，并记录重复数量。
2. `addKeys` / `addKeysByChunk` 遍历 table checkpoint 的数据 engine 和 chunk，重放解析与编码，将每个编码行展开出的 KV key 与 RowID 交给 `duplicate::KeyAdder`。
3. `makeDupHandlerConstructor` 把 `ErrorOnDup` 与 `ReplaceOnDup` 映射为两种 `duplicate::Handler`：前者立即形成规范化重复键错误，后者把除每组最后一个 RowID 外的旧 RowID 写入 ignore sorter。
4. `simplifyTable` 仅保留主键/唯一索引及其所需列，减少去重编码需要处理的数据；`decodeIndexID` 将 record-key 冲突归类为 handle 冲突，将 index-key 冲突还原为具体索引 ID。

这些职责由 `dup_detect_test.rs` 的 handler、engine 跳过/数据 chunk、表简化测试，以及 Go 对照 `dup_detect.go`、`dup_detect_test.go` 共同佐证。

## 主要符号

- `pub struct dupDetector { tr, rc, cp, logger }`：一次单表预去重所需的表导入器、全局导入控制器、表 checkpoint 快照和日志上下文。四个字段均由 `TableImporter::preDeduplicate` 注入。
- `dupDetector::run(ctx, workingDir, ignoreRows) -> Result<()>`：本文件的顶层执行入口。`workingDir` 用于打开检测过程的磁盘 sorter；`ignoreRows` 是 replace 策略的输出 sorter。
- `dupDetector::addKeys`：从 `cp.Engines` 生成 chunk 工作项，忽略负 engine ID，并按 `RegionConcurrency.max(1)` 分批创建 scoped threads。
- `dupDetector::addKeysByChunk`：单 chunk 的解析、列映射、扩展列拼接、编码和 key 收集流水线。
- `makeDupHandlerConstructor(...) -> duplicate::HandlerConstructor`：策略工厂。`ErrorOnDup` 返回默认 `errorOnDup`；`ReplaceOnDup` 为每个 handler 从传入 sorter 新建 writer；其他枚举值直接 panic。
- `ERR_DUPLICATE_KEY_MSG` / `ErrDuplicateKey()`：分别是错误消息模板和 RFC code 为 `Lightning:PreDedup:ErrDuplicateKey`、class 为 `ErrDuplicateKey` 的规范化错误构造器。
- `errorOnDup`：保存当前冲突索引 ID 和最多两个 RowID。`Begin` 解码索引类别，`Append` 截断诊断样本，`End` 总是返回重复键错误，`Close` 无操作。
- `replaceOnDup`：保存 writer、上一 RowID 和 varint 编码的索引 ID。每次 `Append` 先写出“上一项”，再把当前项设为候选，因此一组重复项的最后一个 RowID 被保留、此前 RowID 被标记为忽略；`Close` 关闭 writer。
- `simplifyTable(&TableInfo, &[i32])`：克隆表元数据并返回调整后的列置换，不修改输入。
- `CONFLICT_ON_HANDLE = -1` / `decodeIndexID`：以 `-1` 区分 record handle 冲突；index key 则经 `tablecodec::DecodeIndexKey` 返回真实 index ID。

## 执行流程

`TableImporter::preDeduplicate` 先创建 ignore sorter 和 `dupDetector`，再进入 `run`：

1. 以 `rc.cfg.App.RegionConcurrency` 打开检测用 `DiskSorter`，创建 `duplicate::Detector`。
2. `addKeys` 遍历 `cp.Engines`。负 engine ID 代表 index engine，被显式跳过；每个非负 engine 的每个 chunk 绑定一个 `detector.KeyAdder`。工作项按并发度切片，每批启动 scoped threads，批内全部 join 后才开始下一批。
3. 每个 worker 调用 `addKeysByChunk`。成功时 `KeyAdder::Flush`；失败时尝试 `KeyAdder::Close` 并向上返回原错误；thread panic 被转换成 `duplicate key worker panicked`。
4. 单 chunk 先由 `openParser` 打开解析器并读取首行。空文件的首个 `EOF` 直接成功；否则取得源列名、忽略列配置和 `createColumnPermutation` 结果。
5. 将 checkpoint 中 extend columns 的值纳入行数据，并把对应目标列的 permutation 改为首行原始长度之后的扩展位置。
6. 调用 `simplifyTable` 取得缩减后的列置换；随后用 SQL mode、chunk timestamp、sysvars、`PrevRowIDMax` 自动随机种子、文件路径、`encTable` 和 logger 创建 encoder。注意当前 Rust 只使用 `simplifyTable` 返回的 permutation，丢弃其返回的简化 `TableInfo`。
7. 循环取 `LastRow`，追加 extend values，调用 `Encode`，用 `kv::Row2KvPairs` 展开 KV，并逐项 `adder.Add(key, RowID)`；随后清理编码行、回收解析行并读取下一行。`EOF` 正常结束，其他错误透传。无论循环结果如何，函数最后都尝试 `parser.Close()`。
8. 键收集完成后，`run` 依据冲突策略构造 handler，并调用 `Detector::Detect`。成功取得 `numDups` 后清理检测 sorter；最后日志任务以错误（若有）和重复数量结束。

## 数据与状态

- checkpoint 是扫描范围的权威来源：只遍历 `TableCheckpoint.Engines` 内非负 ID 的 `Chunks`。`dup_detect_test.rs::test_add_keys_processes_data_chunks_and_skips_index_engine` 用同一个缺失文件 chunk 证明负 engine 被跳过、非负 engine 会实际尝试打开文件。
- `column_permutation` 将目标表列映射到解析行位置；被忽略列、extend columns、隐藏 auto-row-id 都会影响其形状。`simplifyTable` 在无生成列时同步缩减列与 permutation，并重写保留索引列的 offset。
- 若存在任意 generated column，`simplifyTable` 保留全部列和原 permutation，以免删除生成表达式可能依赖的列；它仍会删除非主键/非唯一索引。
- `errorOnDup.keyIDs` 最多保存两个克隆后的 RowID，只用于错误诊断，不随更多重复项增长。
- `replaceOnDup.keyID` 是“尚未写出”的最后候选。收到下一 RowID 时才将前一个写入 sorter，所以每组冲突保留最后一项。writer value 是 `codec::EncodeVarint` 编码的 index ID；record handle 使用 `-1`。
- encoder 的 `AutoRandomSeed` 来自 `chunk.Chunk.PrevRowIDMax`，保持 checkpoint 恢复后再次编码的自动随机值稳定；这一意图与 Go `addKeysByChunk` 的同字段一致。

## 依赖与调用关系

上游局部调用边为 `TableImporter::preDeduplicate` → `dupDetector::run`。crate 根 `lib.rs` 再导出本模块，但仓库内未发现其他 Rust 生产调用者。Go 主链另有 `table_import.go` 中导入阶段调用 `tr.preDeduplicate(...)`，不能据此宣称 Rust 主链已完成同样接线。

关键下游边如下：

- `run` → `extsort::OpenDiskSorter` → `duplicate::NewDetector` → `addKeys` → `duplicate::Detector::Detect`。
- `addKeys` → `Detector::KeyAdder` → `addKeysByChunk` → `KeyAdder::{Add, Flush, Close}`。
- `addKeysByChunk` → `openParser`、ignore-column 配置、`createColumnPermutation`、`filterColumns`、`simplifyTable`、`EncodingBuilder::NewEncoder`、`Encoder::Encode`、`kv::Row2KvPairs`。
- 两个 handler → `decodeIndexID` → `tablecodec::{IsRecordKey, IsIndexKey, DecodeIndexKey}`；replace handler 还依赖 `Writer::{Put, Close}` 和 varint 编码。
- `simplifyTable` → `TableInfo::{Clone, GetPkColInfo}`、`ColumnInfo::IsGenerated`、`common_ext::TableHasAutoRowID`。

RustCodeGraph 的精确文件读取确认源码 1–466 行；符号查询同时定位到 Rust/Go 的 `dupDetector`、`makeDupHandlerConstructor`、`simplifyTable`、`decodeIndexID`。图查询还识别到 `addKeysByChunk` → `simplifyTable`，但当前索引对该文件的部分 callers/callees 查询没有返回完整边，因此上述完整局部关系以源码和仓库调用搜索交叉核验，不将缺失图边解释为“无调用”。

## 错误处理与边界

- 磁盘 sorter 打开、KeyAdder 创建、parser 打开/读取、忽略列解析、列置换、encoder 创建/编码、KV 添加、检测和 writer 操作的错误都通过 `Result` 向上返回，多个边界使用 `errors::Trace` 保留错误链。
- 首读或循环读遇到 root cause class 为 `EOF` 时视为正常结束；其他 parser 错误保留。这里依赖错误 class 字符串，而 Go 对照使用 `errors.Cause(err) == io.EOF`。
- `decodeIndexID` 只接受 record key 或 index key；其他字节返回包含十六进制 key 的明确错误。index key 解码失败会透传 trace。
- `makeDupHandlerConstructor` 对 `ErrorOnDup`、`ReplaceOnDup` 之外的策略 panic，调用方必须保证配置已归一到这两个预去重策略之一。
- worker panic 被 join 逻辑转换成普通错误；某批任一 worker 返回错误后，join 循环会提前返回，其他已启动 worker仍由 scoped-thread 作用域保证在离开作用域前结束。
- `run` 中 `sorter.CloseAndCleanup()` 位于所有带 `?` 的操作之后；若 `addKeys` 或 `Detect` 提前返回错误，该清理语句不会执行。Go 版本以 `defer` 覆盖错误路径，这是当前 Rust/Go 的资源清理差异，扩展或修复时不能忽略。
- `parser.Close()` 和日志 `task.EndWith(...)` 的返回值被有意忽略；日志结束覆盖成功和失败路径，但 parser close 失败不会替换主结果。

## 并发与资源生命周期

并发上限取 `RegionConcurrency.max(1)`，避免配置为零时 `chunks_mut(0)`。Rust 实现不是持续 worker pool：它预先创建全部 `(chunk, KeyAdder)` 工作项，再以固定大小批次启动线程，并在批次间形成 barrier。共享的 `TableImporter`、`Controller`、配置、存储、encoder builder 与 logger 以不可变引用或 `Arc` 跨 scoped threads 使用；每个 chunk 独占可变 `KeyAdder` 和 parser/encoder 状态。

这与 Go 的 `errgroup.WithContext` + `SetLimit` 有可观察的调度差异：Go 在一个任务失败后取消派生 context，Rust 只是把克隆的原 context 传给所有任务，没有在首错时生成取消信号；Rust 还会按 batch 等待，而不是某个 slot 释放后立即补位。文档不能假设二者取消/吞吐语义完全一致。

资源所有权边界为：检测 sorter 由 `run` 创建并在成功路径 `CloseAndCleanup`；ignore sorter 由上游创建并通过 `Arc<dyn ExternalSorter>` 共享，replace handler 只创建和关闭自己的 writer；parser 由单 chunk 创建并在函数尾关闭；KeyAdder 成功时 flush、处理失败时 close；handler writer 由 detector 在 handler 生命周期结束时调用 `Close`。`std::thread::scope` 保证 worker 不会越过 `addKeys` 借用的配置与表元数据生命周期。

## 与 Go 版本的对应关系

`lightning/pkg/importer/dup_detect.go` 是逐函数对照来源，主要结构一致：`run` 建 detector、`addKeys` 重放 chunk、两种 handler 分别报错/记录旧行、`simplifyTable` 缩减唯一约束相关结构、`decodeIndexID` 区分 handle 与 index。`dup_detect_test.go` 的 `TestErrorOnDup`、`TestReplaceOnDup`、`TestSimplifyTable` 在 Rust `dup_detect_test.rs` 中均有对应测试；Rust 额外测试了负 engine 跳过和数据 engine 错误路径。

已确认的差异包括：

- Go `addKeys` 使用带取消传播的 `errgroup`，Rust 使用分批 scoped threads。
- Go 在每个 goroutine 内按需创建 KeyAdder；Rust 在启动线程前为全部 chunk 创建 KeyAdder。
- Go `addKeysByChunk` 用简化后的 `TableInfo` 调用 `tables.TableFromMeta` 新建编码表；Rust 丢弃简化后的表，只把简化 permutation 交给基于 `tr.encTable` 的 encoder。其实际编码等价性没有由本文件单独证明，应视为当前移植差异。
- Go 对检测 sorter 使用 defer 清理，Rust 只在检测成功到达清理语句时清理。
- Go 错误参数保留结构化的 `(idxID, keyIDs)`；Rust `errorOnDup::End` 当前把两者先格式化为一个字符串参数。Rust 测试允许较宽松的错误文本匹配，而 Go 测试直接断言结构化 Args；两端诊断形状并非完全等价。
- Rust 当前 crate 注释说明部分后端/PD/TiKV/SQL/encode/mydump/IO 边界仍由 local stubs 承担；因此独立测试验证的是当前 slim port 契约，不能替代真实后端集成证据。

## 扩展指南

- 新增冲突策略时，首先修改 `makeDupHandlerConstructor`，定义 handler 的 `Begin/Append/End/Close` 状态机，并在独立的 `dup_detect_test.rs` 增加策略、writer 关闭和错误传播测试；同时核对 Go 同名工厂，避免把未知策略继续落到 panic。
- 调整 chunk 并发时应修改 `addKeys`，明确是否要保持 Go 的动态限流与首错取消语义，并测试并发度 0、KeyAdder 创建失败、worker panic、部分 worker 失败和资源关闭。不要把测试嵌入生产 `.rs`。
- 调整编码范围或列裁剪时应同时审查 `addKeysByChunk` 与 `simplifyTable`，覆盖 generated columns、PK-is-handle、隐藏 auto-row-id、extend columns、ignored columns、复合唯一索引和 checkpoint 恢复种子。尤其要决定 Rust 是否应像 Go 一样使用简化后的 `TableInfo` 构造编码表。
- 调整错误诊断时应同步 `ERR_DUPLICATE_KEY_MSG`、`ErrDuplicateKey`、`errorOnDup` 和 `TableImporter::preDeduplicate` 的消费逻辑，并用独立测试固定 RFC code、index ID 与 RowID 参数形状。
- 修正资源生命周期时，优先让检测 sorter 在所有退出路径清理，并验证主错误与 close/cleanup 错误的优先级；同时检查 KeyAdder 在预创建后尚未启动 worker就发生错误时的释放行为。
- 性能风险集中在全量重放输入、KV 展开、磁盘排序、预创建全部 KeyAdder、batch barrier 和 clone（sysvars、extend values、RowID）上；兼容风险集中在 key 编码、列 permutation、保留最后一行策略和规范化错误形状上。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`node --file lightning/pkg/importer/dup_detect.rs --offset 1 --limit 500` 完整读取 466 行；`query` 精确定位 Rust/Go 的 `dupDetector`、`makeDupHandlerConstructor`、`simplifyTable`、`decodeIndexID`；`explore` 给出 `addKeysByChunk` 调用 `simplifyTable` 的图证据。部分精确 callers/callees 未产生完整输出，已用下列源码搜索补证。
- 生产源码：`lightning/pkg/importer/dup_detect.rs`、`lightning/pkg/importer/table_import.rs`、`lightning/pkg/importer/lib.rs`。
- crate 边界：`lightning/pkg/importer/Cargo.toml`。
- Go 对照：`lightning/pkg/importer/dup_detect.go`、`lightning/pkg/importer/table_import.go`。
- 独立测试：`lightning/pkg/importer/dup_detect_test.rs`、`lightning/pkg/importer/dup_detect_test.go`；另以 `lightning/pkg/importer/parity_test.rs` 的 decode、simplify 和 handler 构造引用作为补充调用证据。
- 仓库搜索：确认 Rust `TableImporter::preDeduplicate` 构造并调用 `dupDetector::run`，以及模块由 `lib.rs` 装配；未发现 Rust 生产代码继续调用 `preDeduplicate`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核所有结论均指向上述符号/文件，未把 Go 主链接线或 stub 测试推断为 Rust 已完整支持。
