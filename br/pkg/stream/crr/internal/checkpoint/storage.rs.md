# `br/pkg/stream/crr/internal/checkpoint/storage.rs`

## 文件定位

本文件属于 crate `astersql-br-pkg-stream-crr-internal-checkpoint`，由同目录 `lib.rs` 以 `pub mod storage` 挂载。它位于 CRR（跨区域复制）安全检查点计算器的“上游对象发现与元数据加载”边界：从上游对象存储的 `v1/backupmeta` 前缀增量列举 backupmeta，把文件名转换为轻量的 `parsedMetaFile`，再读取非空 meta 的 JSON 内容形成 `loadedMetaFile`。真正的检查点轮次调度、并发读取和下游同步等待分别位于 `progress.rs` 的 `Calculator::plan_round` 与 `Calculator::wait_object_sync`，本文件不直接推进 `synced_ts`。

同目录 `Cargo.toml` 将该目录声明为独立 library crate，直接依赖 `astersql-br-pkg-stream-backupmetas`（解析 backupmeta 文件名）、`astersql-br-pkg-streamhelper`、`serde` 和 `serde_json`。模块入口为 `lib.rs`；包级算法约束还记录在 `doc.go`/`doc.rs`。

## 核心职责

1. `Calculator::new_meta_file_iter`/`collect_meta_files` 根据当前 `state.synced_ts` 构造 `WalkOption`，只列举 `v1/backupmeta` 下、游标之后且 `flush_ts > synced_ts` 的 `.meta` 对象。
2. `load_meta_file` 对空 meta 走无 I/O 快路径；对普通 meta 调用 `UpstreamStorageReader::ReadFile`、解析 JSON、交叉校验文件名与内容中的 store ID，并提取后续必须确认同步的数据对象路径。
3. `validate_incremental_meta_scan_storage` 在 `CalculatorDeps::validate` 阶段拒绝不保证 `StartAfter` 语义的 URI scheme，避免增量列举漏对象。
4. `meta_scan_start_after` 生成与 backupmeta 大写十六进制命名顺序一致的排他游标，使已完成水位及同一 `flush_ts` 的所有 store 条目都落在游标之前。
5. `walk_dir_seq` 将回调式 `WalkDir` 适配为可提前停止的收集过程，并区分内部停止哨兵与真实存储错误。

该文件只负责“发现和解释上游 meta”，不负责 PD checkpoint 获取、下游对象存在性判断、轮询等待或全局水位聚合。

## 主要符号

- 常量 `META_SUFFIX`、`MAX_STORE_ID_SUFFIX`、`STREAM_BACKUP_META_PREFIX`：分别固定 `.meta` 过滤条件、最大 store 游标尾缀 `FFFFFFFFFFFFFFFF~` 和对象前缀 `v1/backupmeta`。
- `parsedMetaFile { path, flush_ts, store_id, empty }`：只从对象名获得的轻量记录，尚未读取内容；可见性为 `pub(crate)`。
- `loadedMetaFile { path, flush_ts, store_id, empty, data_file_paths }`：完成内容解析与 store ID 校验后的轮次输入。
- `BackupMetadata`、`BackupDataFileGroup`、`BackupDataFileInfo`：只映射当前逻辑所需的落盘 JSON 字段；`MetaVersion` 使用 `serde_json::Value` 兼容字符串和数值版本，`StoreId` 兼容 `store_id` 别名。
- `walk_dir_seq`：包装 `UpstreamStorageReader::WalkDir`。yield 返回 `false` 时用文本为 `stop walk iteration` 的内部 `Error` 中断底层遍历，再将该错误归一化为成功。
- `Calculator::new_meta_file_iter`：对外提供本轮 `Iterator<Item = Result<parsedMetaFile, Error>>`；实现先调用 `collect_meta_files` 物化 `Vec`，所以迭代期间不持有底层 Walk 状态。
- `load_meta_file`：加载单个 meta，返回 `(loadedMetaFile, ignored)`；第二个布尔值仅在“空 meta 且文件名 store ID 为 0”时为 `true`。
- `parse_backup_metadata`：JSON 反序列化，并在 V1 且 `FileGroups` 为空时把顶层 `Files` 包装成一个匿名 group。
- `resolve_store_id`：要求内容 store ID 为正；文件名 store ID 非零时还要求两者一致。
- `extract_data_file_paths`：每个 group 优先采用组级 `Path`，否则展开其 `DataFilesInfo[*].Path`，跳过空路径。
- `validate_incremental_meta_scan_storage`：允许 `s3`、`oss`、`file`、`gcs`，拒绝空 scheme 和其他 scheme。
- `meta_scan_start_after`：`synced_ts == 0` 返回空串；否则返回 `v1/backupmeta/{synced_ts:016X}FFFFFFFFFFFFFFFF~`。

## 执行流程

构造计算器时，`calculator.rs` 的 `CalculatorDeps::validate` 先把 `Upstream.URI()` 交给 `validate_incremental_meta_scan_storage`。不支持增量游标的后端在任何轮次开始前就失败。

一次成功推进中，`Calculator::ComputeNextCheckpoint` 在取得上游 checkpoint 和存活 store 后调用 `progress.rs` 的 `plan_round`：

1. `plan_round` 创建可取消的轮次上下文，并遍历 `self.new_meta_file_iter(&plan_ctx)`。
2. `collect_meta_files` 设置 `WalkOption.SubDir = "v1/backupmeta"`；若 `synced_ts != 0`，同时设置由 `meta_scan_start_after` 生成的 `StartAfter`。
3. `walk_dir_seq` 调用上游 `WalkDir`。回调忽略非 `.meta` 对象；对文件 basename 去掉后缀后调用 `backupmetas::ParseName`。
4. 文件名解析失败会把带路径的错误放入结果并立即停止 Walk；成功但 `FlushTS <= synced_ts` 的条目被跳过；其余条目转换为 `parsedMetaFile`。
5. `plan_round` 再按 `state.synced_by_store` 过滤每个 store 已处理的旧条目，把剩余 meta 交给受 `MetaReadConcurrency` 限制的 scoped threads。
6. 每个线程调用 `load_meta_file`。空 meta 不读取对象内容；普通 meta 读取、反序列化、校验 store ID 并提取 data paths。首个加载错误写入共享错误槽并取消同轮兄弟任务。
7. 未被 `ignored` 标记的 `loadedMetaFile` 由 `roundPlan::record_loaded_meta` 汇入轮次；其 meta 路径和数据路径最终进入 `pending_paths`，供 `wait_object_sync` 逐个确认。只有这一等待成功后，外层才更新按 store 的同步进度和安全检查点。

## 数据与状态

本文件自身不保存跨轮状态；长期状态在 `Calculator.state` 中。读取的关键输入是全局 `synced_ts`，它同时决定对象存储 `StartAfter` 和扫描后额外的 `FlushTS` 过滤。额外过滤是防御边界项，并不替代存储端游标语义。

`parsedMetaFile.store_id` 来自文件名，可以为 0；`loadedMetaFile.store_id` 对非空 meta 来自内容且必须为正。空 meta 不含需要加载的 JSON：文件名带非零 store ID 时保留该 ID并参与轮次；文件名 store ID 为 0 时无法归属 store，返回 `ignored = true`，只产生警告文本且不进入轮次计划。

`BackupMetadata` 是面向所需字段的兼容视图，不是完整 protobuf 模型。V1 的顶层 `Files` 只有在 `FileGroups` 为空时才被转换；已有 V2 风格 groups 不会被覆盖。提取结果保持 group/成员遍历顺序，不去重；组级路径存在时，其成员路径被视为已被该组对象覆盖。

`walkEntry.size` 随 Walk 回调保存，但当前扫描逻辑不消费该字段。`collect_meta_files` 会把全轮扫描结果物化到内存，空间使用量随候选 meta 数量线性增长。

## 依赖与调用关系

上游调用关系：

- `calculator.rs::CalculatorDeps::validate` → `validate_incremental_meta_scan_storage`，发生在 `NewCalculator` 构造阶段。
- `progress.rs::Calculator::plan_round` → `Calculator::new_meta_file_iter` → `collect_meta_files` → `walk_dir_seq`/`backupmetas::ParseName`。
- `progress.rs::Calculator::plan_round` 的并发加载线程 → `load_meta_file` → `UpstreamStorageReader::ReadFile`、`parse_backup_metadata`、`resolve_store_id`、`extract_data_file_paths`。
- `progress.rs::roundPlan::record_loaded_meta` 消费 `loadedMetaFile`，随后 `wait_object_sync` 消费形成的 pending paths。

下游与外部边界：

- `UpstreamStorageReader`、`WalkOption`、`Context`、`Error`、`Calculator` 均定义在同 crate 的 `calculator.rs`。
- `astersql_br_pkg_stream_backupmetas::ParseName` 是文件名格式的权威解析器，本文件不重复实现 flush/store/empty 标签语法。
- `serde_json` 只用于 meta 内容；扫描阶段只解析对象名，避免为已过滤对象产生读取放大。
- `lib.rs` 将 `storage` 声明为公开模块，但主要业务类型和函数保持 `pub(crate)`，外部 crate 通常通过扁平导出的 `Calculator` API 间接使用它们。

RustCodeGraph 对目标文件给出 385 行完整源码；精确符号查询定位 `load_meta_file`、`validate_incremental_meta_scan_storage` 和 `meta_scan_start_after`。图的通用名称检索噪声较大，因此同 crate 的直接调用点又以精确符号文本核对，得到上述 `calculator.rs`/`progress.rs` 调用链。

## 错误处理与边界

- `walk_dir_seq` 将底层 Walk 错误包装为 `walk upstream backupmeta prefix: ...`；仅内部主动停止哨兵被吞掉。当前哨兵通过错误消息文本识别，因此新增后端错误若恰好使用相同文本会被误判，扩展时应谨慎。
- backupmeta 文件名解析失败会产生 `parse backupmeta name <path>: ...` 并停止本轮扫描，不能静默跨过未知对象，否则可能错误推进安全点。
- `ReadFile` 与 JSON 解析错误都附加 meta 路径；`resolve_store_id` 分别拒绝非正内容 ID 和文件名/内容 ID 不一致。
- serde 字段均允许缺省；这提高了历史精简格式兼容性，但缺失 `StoreId` 最终会在 `resolve_store_id` 被拒绝。未知 JSON 字段被忽略。
- URI 解析仅按 `file://` 或第一个 `://` 前缀提取 scheme；无 scheme 明确报错，未知 scheme 明确拒绝。它不执行 Go `net/url.Parse` 的完整 URL 语法校验。
- 空 meta 快路径不会调用 `ReadFile`。带 store ID 的空 meta 仍形成有效进度条目；store ID 为 0 的空 meta 被忽略，避免将无法归属 store 的进度计入全局最小值。
- `meta_scan_start_after(0)` 为空，表示全量扫描；非零时必须保持 16 位大写十六进制和最大 store 后缀，否则可能因对象存储字典序漏扫真实大写名称。

## 并发与资源生命周期

扫描本身同步执行：`walk_dir_seq` 中的 `AtomicBool` 只充当跨回调可见的停止标志，使用 `SeqCst`；函数返回后标志即销毁。`new_meta_file_iter` 在返回迭代器前已完成 Walk 并物化全部结果，因此不会把存储回调、借用或连接生命周期泄漏给调用者。

内容加载的并发不在本文件创建，而由 `progress.rs::plan_round` 使用 `thread::scope` 和 `ConcurrencyLimiter` 管理。每个线程共享只读 `UpstreamStorageReader` 引用，将成功结果写入 `Mutex<roundPlan>`；首错通过 `Mutex<Option<Error>>` 保存并触发轮次取消。scoped threads 在 `plan_round` 返回前全部结束，所以 `parsedMetaFile`、上下文和 storage 借用不会逃逸。

本文件不持有锁、事务、异步任务或长期句柄。主要资源风险是候选 meta 的整轮物化与每个普通 meta 的完整字节读取；若未来改成流式/异步实现，必须保留“坏名立即停止”“首错取消同轮加载”和 `MetaReadConcurrency` 上限。

## 与 Go 版本的对应关系

直接对照文件是同目录 `storage.go`，Rust 基本保持以下语义：`.meta` 过滤、`StartAfter` 游标、`backupmetas.ParseName`、`flushTS` 过滤、空 meta 快路径、内容 store ID 为权威值、组级路径优先、允许的 URI scheme，以及错误中携带对象路径。

需要明确的实现差异：

- Go `newMetaFileSeq` 返回惰性的 `iter.Seq2`；Rust `new_meta_file_iter` 先把 Walk 结果收集到 `Vec` 再迭代，内存与错误出现时机不同，但本轮短路语义保持。
- Go 用 `errStopWalkIteration` 的错误身份识别主动停止；Rust 当前比较错误消息文本。
- Go 在扫描前有 `failpoint.InjectCall("before-list-meta")`；Rust 文件没有对应 failpoint 注入点。
- Go `parseBackupMetadata` 调用 protobuf `stream.MetadataHelper.ParseToMetadata`；Rust 用局部 serde 结构解析 JSON，并显式模拟 V1 顶层 `Files` 到匿名 group 的转换。它只保证当前字段子集，不应被描述为完整 protobuf 兼容层。
- Go 用结构化 `log.Warn`；Rust 对零 store ID 空 meta 使用 `eprintln!`。
- Go `url.Parse` 负责 URI 解析；Rust `parse_scheme` 是较窄的 scheme 字符串提取器。
- Go Walk 序列是惰性的；Rust 为了借用安全增加了 `AtomicBool` 停止适配和 `Vec` 物化。

相关独立测试包括：Rust `storage_internal_test.rs`（大写 StartAfter、增量扫描、空 meta 快路径与零 store ID 忽略）、Rust `storage_test.rs`（V1 顶层 Files 兼容），以及 Go `storage_internal_test.go` 和 `checkpoint_calculator_test.go`。更完整的轮次行为由 Rust `checkpoint_calculator_test.rs`、`integration_test.rs`、`parity_test.rs` 和 `randomized_integration_test.rs` 间接覆盖。

## 扩展指南

- 新增对象存储 scheme：先确认其 `WalkDir.StartAfter` 是严格排他且按完整对象键字典序实现，再同时修改 `validate_incremental_meta_scan_storage` 和构造校验测试；不能仅因为后端可列举就放行。
- 修改 backupmeta 命名或游标：优先修改权威 `backupmetas::ParseName`；同步审查 `META_SUFFIX`、`MAX_STORE_ID_SUFFIX`、`meta_scan_start_after`，并在 `storage_internal_test.rs` 添加大小写、同 flush 多 store、extra tags 和边界水位用例。
- 扩展元数据字段/版本：修改 `BackupMetadata` 及其私有子结构，明确 serde 别名和缺省策略；同步 `storage_test.rs`，并与 Go `stream.MetadataHelper.ParseToMetadata` 的输出核对。不要把测试写回生产文件。
- 改变 store ID 规则：以 `resolve_store_id` 为唯一校验入口，并增加负数、零、文件名为零、名称/内容冲突的独立测试；错误会阻断整轮，因此属于安全兼容性变更。
- 改变 data path 展开：修改 `extract_data_file_paths`，同时验证组路径优先、空路径、重复路径和 V1/V2 布局。路径集合决定下游等待范围，漏项会导致过早推进，额外项会造成不必要阻塞。
- 优化大规模扫描：可考虑惰性流或有界通道，但必须与 `plan_round` 的 scoped 并发、取消和首错顺序共同设计，不能绕过坏名停止和水位过滤。
- 修改错误模型：若将停止哨兵改为可判别类型，应同步 `walk_dir_seq` 及存储测试，避免吞掉真实后端错误。

兼容性风险集中在落盘 JSON/文件名格式和 scheme 行为；正确性风险集中在漏扫、漏提取路径和错误 store 归属；性能风险集中在全轮 `Vec` 物化、完整 meta 读取与重复路径导致的额外同步检查。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file br/pkg/stream/crr/internal/checkpoint/storage.rs` 读取目标文件完整 385 行；`query` 精确定位 `load_meta_file`、`validate_incremental_meta_scan_storage`、`meta_scan_start_after`；`node` 读取 `lib.rs`、`calculator.rs`、`progress.rs`、`storage_internal_test.rs` 和 `checkpoint_calculator_test.rs` 的相关定义与调用段。
- 源码：`br/pkg/stream/crr/internal/checkpoint/storage.rs`；直接入口/调用方：同目录 `lib.rs`、`calculator.rs`、`progress.rs`；包级契约：同目录 `doc.go`。
- crate 边界：`br/pkg/stream/crr/internal/checkpoint/Cargo.toml`。
- Go 对照：`br/pkg/stream/crr/internal/checkpoint/storage.go`；Go 边界测试：`storage_internal_test.go`、`checkpoint_calculator_test.go`。
- Rust 独立测试：`storage_internal_test.rs`、`storage_test.rs`；轮次级间接验证：`checkpoint_calculator_test.rs`、`integration_test.rs`、`parity_test.rs`、`randomized_integration_test.rs`。
- 人工复核结论：文件存在是为了把对象存储增量列举和落盘格式兼容隔离在计算器边界；运行路径从构造期 scheme 校验，经扫描/加载进入 round plan，再由下游同步等待决定能否推进；安全扩展必须同步独立测试并保护 StartAfter、store 归属和 data path 完整性三项不变量。
