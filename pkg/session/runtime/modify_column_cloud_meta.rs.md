# `pkg/session/runtime/modify_column_cloud_meta.rs`

源文件：[`modify_column_cloud_meta.rs`](./modify_column_cloud_meta.rs)

## 文件定位

本文件位于 `astersql-session` crate 的 `runtime` 私有模块中，由 `pkg/session/runtime.rs` 以 `mod modify_column_cloud_meta;` 装配；所有导出项均为 `pub(super)`，只服务于 session runtime 内部的分布式 `MODIFY COLUMN`/索引回填云存储路径，不构成 crate 的公共 API。`pkg/session/Cargo.toml` 表明它直接依赖 `base64`、`serde`、`serde_json`、`astersql-ingestor-globalsort` 与 `astersql-ingestor-simplesst`。

它是运行时对象与 Go JSON 协议之间的“线格式适配层”：将原始二进制键编码为 Go `encoding/json` 对 `[]byte` 使用的标准 Base64 字符串，将较大的全局排序字段放入对象存储，并让持久化的子任务行只保留内部字段和 `ExternalPath` 指针。上游直接使用者包括 `modify_column_dist_backfill.rs`、`modify_column_pipeline.rs`、`modify_column_cloud_planner.rs` 和 `modify_column_cloud_executor.rs`；模块本身不调度任务、不执行扫描或导入。

## 核心职责

1. `encode`/`decode` 固化 Go `[]byte` JSON 的标准 Base64 表示，避免调用方直接比较编码文本。
2. `FilesStat`、`Conflict`、`SortedMeta` 和 `ExternalFields` 精确描述云回填元数据的 JSON 字段名、空值兼容与省略规则。
3. `SortedMeta::from_simple` 和 `SortedMeta::from_merge` 把两类 writer summary 转成统一的持久化结构；`sort_files` 再把持久化的文件对还原为全局排序 range splitter 所需类型。
4. `SortedMeta::merge` 聚合多个子任务或 writer 的键范围、KV 规模、文件统计和冲突信息。
5. `read` 根据内部 JSON 中的 `ExternalPath` 读取并合并外部 JSON；`write` 先发布外部字段，再把指针写回内部 JSON 并返回可持久化字节。

本文件刻意不负责 Go `decodeBackfillSubTaskMeta` 的全部兼容收尾。例如“`meta_groups` 为空时使用内嵌旧版 `SortedKVMeta`”由 `modify_column_cloud_planner.rs::groups` 和 `modify_column_cloud_executor.rs::run_import` 在消费点实施。

## 主要符号

- `encode(key: &[u8]) -> String` / `decode(key: &str) -> Result<Vec<u8>, String>`：使用 `base64::engine::general_purpose::STANDARD`。解码错误被降为字符串并向上传播。
- `FilesStat`：保存一组文件的最小/最大键、`[data_file, stat_file]` 文件对和最大重叠数。Serde 将字段映射为 `min-key`、`max-key`、`filenames`、`max-overlapping-num`。
- `Conflict`：保存冲突 KV 数量和冲突文件列表；零数量与空文件列表序列化时省略。
- `null_vec`：允许 JSON 数组字段为缺失、数组或显式 `null`，统一反序列化为空 `Vec`。这对 Go 零值 slice 编码出的 `null` 很关键。
- `SortedMeta`：对应 Go `globalsort.SortedKVMeta`，键范围为 `[start, end)`；同时记录总字节数、KV 数量、多文件统计和冲突信息。
- `SortedMeta::merge`：忽略完全空范围；接收端为空时整体克隆；否则解码 Base64 后按原始字节序取最小起点、最大终点，再追加统计并以 wrapping 加法累计无符号计数。
- `SortedMeta::from_simple`：对应 Go `NewSortedKVMeta`，从 `simplesst::writer::WriterSummary` 构造元数据，把最大键追加一个零字节形成排他上界。
- `SortedMeta::from_merge`：从 globalsort merge writer 的 summary 构造同一线格式；文件对转换为 `sort::FilePair` 的数据/统计路径，持久化重叠数设为 `1`。
- `SortedMeta::sort_files`：为 range splitter 恢复 `sort::MultipleFilesStat`；持久化格式不保存 file properties，因此恢复出的 `properties` 为空。
- `ExternalFields`：对应 Go `BackfillSubTaskMeta` 上标记 `external:"true"` 的字段；`legacy` 通过 `#[serde(flatten)]` 承接 v7.5 单索引内嵌 `SortedKVMeta`。
- `read` / `write`：对象存储指针协议的读写入口，均接收 `dyn sort::Storage`，错误统一返回 `String`。

文件没有条件编译项、全局变量、trait 或自定义错误类型。

## 执行流程

写路径通常从 `modify_column_dist_backfill.rs::ReadIndex::RunSubtask` 或云 planner/executor 开始。调用方先收集 `SortedMeta` 和元素 ID，构造 `ExternalFields` 与仍包含 `physical_table_id`、`row_start`/`row_end`、`ts` 等内部字段的 `serde_json::Value`。`write` 将外部字段序列化并写到给定对象路径；写成功后才在内部对象插入 `ExternalPath`，最后序列化并返回较小的持久化行。该顺序确保不会发布一个指向尚未成功写入对象的指针。

读路径由 `modify_column_cloud_planner.rs::groups`、`modify_column_cloud_executor.rs::run_merge`/`run_import` 使用。`read` 先解析持久化行；没有或空 `ExternalPath` 时直接返回原对象。存在路径时，它同步读取对象、要求外部与内部 JSON 都是 object，并以外部字段覆盖/补充同名内部键。调用方随后把合并结果反序列化为 `ExternalFields`，并按步骤进行合并、切分、merge sort 或导入。

统计路径中，pipeline writer 关闭时把 simplesst summary 经 `from_simple` 转换并在调用方持有的 `Mutex<Vec<SortedMeta>>` 中合并；merge executor 的回调则用 `from_merge` 产生元数据。planner 汇总多个前序 subtask 时逐组调用 `merge`，再用 `sort_files` 生成 range splitter 输入。

## 数据与状态

模块自身没有进程级或线程局部状态，所有状态都通过值、可变引用和 `Storage` 边界传递。关键不变量如下：

- `SortedMeta.start`/`end` 是标准 Base64 文本包装的原始键；逻辑排序必须先 `decode`，因为 Base64 文本字典序不等价于字节序。
- 非空 summary 的 `end` 是最大已写键后追加 `0` 的排他上界，与 Go `summary.Max.Clone().Next()` 在现有键模型中的线格式约定一致；测试明确验证 `b` 变为 `['b', 0]`。
- `meta_groups[i]` 与 `ele_ids[i]` 按 Go `BackfillSubTaskMeta` 注释保持同序；本文件只保存该关系，不检查长度，planner/executor 负责按步骤消费。
- `FilesStat.filenames` 的每项固定为两个字符串，位置分别表示 data file 和 stat file；类型本身阻止缺项或多项。
- `range_job_keys`、`range_split_keys` 是 Base64 字符串列表；`data_files`、`stat_files` 和 conflict files 是对象路径。
- `legacy` 展平在外部 JSON 顶层，用于旧任务的单组字段；新格式优先使用 `meta_groups`。
- `u64` 累加使用 `wrapping_add`，与 Go 无符号整数溢出回绕一致；`modify_column_cloud_meta_test.rs` 以 `u64::MAX` 覆盖该行为。

## 依赖与调用关系

上游关系由 RustCodeGraph 文件索引和仓库引用检索共同确认：

- `runtime.rs` 声明生产模块，并在 `#[cfg(test)]` 下挂载独立的 `modify_column_cloud_meta_test.rs`。
- `modify_column_pipeline.rs` 持有每个索引组的 `SortedMeta`，writer 关闭时调用 `from_simple` 与 `merge`。
- `modify_column_dist_backfill.rs` 为读取索引步骤初始化组摘要，完成后调用 `write` 发布 `meta_groups`/`ele_ids`。
- `modify_column_cloud_planner.rs` 调用 `read` 汇总前序 subtask，用 `merge` 合并范围，用 `decode`、`sort_files` 构造 merge/import 计划，并调用 `write` 发布下一步计划元数据。
- `modify_column_cloud_executor.rs` 调用 `read` 装载计划；merge 步骤用 `from_merge`/`merge` 汇总真实输出后 `write`；import 步骤解码边界键并消费 legacy fallback。

下游依赖是标准 Base64、Serde JSON、simplesst writer summary、globalsort writer summary/文件统计与 `sort::Storage::{read, write}`。调用图查询对目标文件给出了“被 15 个文件使用”的索引信息，但精确 `callers/callees` 名称查询没有返回边，因此本文没有把未解析的图边当作事实，而以以上直接符号引用补证。

## 错误处理与边界

所有可失败入口使用 `Result<_, String>`，保留底层错误文本但不保留结构化错误类型或调用栈。`decode` 拒绝非法 Base64；因此一个损坏的非空 `SortedMeta` 会使范围合并或计划生成停止，不会静默按文本排序。空范围判断发生在解码前，所以默认/旧版空元数据可安全跳过。

`read` 依次区分内部 JSON 解析失败、对象存储读取失败、外部 JSON 解析失败、外部值非 object、内部值非 object。外部键通过 `row.extend` 覆盖内部同名键；协议依赖 Go 的 internal/external 字段划分避免冲突，新增字段时若两侧重名必须明确覆盖语义。

`write` 要求 `internal` 是 JSON object。它先写外部对象，再检查/修改内部 object；若调用者传入非 object，会返回错误但可能留下尚未被 durable row 引用的外部对象。对象写入与 SQL 行更新也不是跨存储事务，崩溃时可能产生孤儿对象；本模块保证的仅是“指针不会在对象写成功前生成”。调用方只有在整个步骤成功后才发布返回的 subtask metadata；`modify_column_cloud_executor_test.rs` 验证失败或取消的 merge 不发布成功元数据。

`from_simple`/`from_merge` 把“最小键和最大键同时为空”视为空 summary。`sort_files` 无法恢复未持久化的 properties；若 splitter 将来依赖 properties，必须先扩展线格式，而不能继续填空数组。

## 并发与资源生命周期

本文件不创建线程、任务、锁或通道，也不缓存 `Storage`。`read` 和 `write` 都是同步调用，借用的 store 仅在函数调用期间有效；读出的 JSON 和 summary 转换结果均为拥有所有权的值。

并发聚合由调用方控制：`modify_column_pipeline.rs` 和 `modify_column_cloud_executor.rs` 将 `SortedMeta` 放入 `Arc<Mutex<_>>`，取得互斥锁后才调用 `merge`。因此 `merge(&mut self, ...)` 本身无需内部同步，也不能在未加锁的共享可变状态上并发调用。对象路径由任务 ID、subtask ID 或 plan 序号构成，避免正常流程中的 writer 相互覆盖；路径唯一性不是 `write` 自身验证的。

对象生命周期跨越进程和步骤：外部 JSON 必须至少存活到所有后续 planner/executor 读取完成。该文件没有删除或回收接口；清理由更高层任务/对象存储生命周期负责。失败发生在外部写成功、内部行发布前时，可能需要上层按任务前缀回收孤儿对象。

## 与 Go 版本的对应关系

主要对照位于 `pkg/ddl/backfilling_dist_executor.go` 与 `pkg/ingestor/globalsort/util.go`：

- `ExternalFields` 镜像 `BackfillSubTaskMeta` 中带 `external:"true"` 的字段；内部字段 `PhysicalTableID`、`RowStart`、`RowEnd`、`TS` 和 `BaseExternalMeta.ExternalPath` 留在 SQL 行。Serde 的 kebab-case 特例与 Go JSON tag 保持一致。
- `read`/`write` 对应 `BaseExternalMeta.ReadJSONFromExternalStorage`、`WriteJSONToExternalStorage`、`Marshal` 和反射实现的 `marshalInternalFields`/`marshalExternalFields`。Rust 不用反射剥离字段，而是显式建模外部字段，并由调用方提供内部 JSON。
- `SortedMeta`、`from_simple` 与 `merge` 对应 Go `SortedKVMeta`、`NewSortedKVMeta`、`Merge`/`MergeSummary`：空摘要处理、排他上界、字节序 min/max、文件追加、冲突追加和 `uint64` 回绕均保持一致。
- Go JSON 将 `[]byte` 编为 Base64；Rust 用 `Option<String>` 和字符串列表保存同一线格式。`null_vec` 特别兼容 Go nil slice 的 `null`，同时接受缺失字段。
- Go `decodeBackfillSubTaskMeta` 还会在 `RowStart` 为空时回退到 legacy 范围，并在 `MetaGroups` 为空时构造单组；Rust 当前在 planner/import executor 对 `legacy` 做单组回退，但本文件不实现 `RowStart` 回退，不能把 Go 解码函数的全部职责归于此处。
- `from_merge` 面向 Rust globalsort 的 summary 结构：其文件统计只携带文件对，所以用 summary 总范围填充每组 min/max，并将 overlapping 设为 `1`。这不是从 Go `simplesst.MultipleFilesStat` 完整反序列化，扩展时需维持 splitter 所需语义。

Go 证据测试包括 `pkg/ingestor/globalsort/util_test.go::TestSortedKVMeta`、`TestReadWriteJSON` 及 marshal internal/external 的表驱动用例；Rust 对应测试位于独立文件 `pkg/session/runtime/modify_column_cloud_meta_test.rs`。

## 扩展指南

新增外部元数据字段时，应先确认 Go `BackfillSubTaskMeta` 的 JSON tag 与 `external:"true"` 分类，再把字段加入 `ExternalFields`，选择正确的 `rename`、`default`、`skip_serializing_if` 和 `null_vec` 策略；同时补充 `modify_column_cloud_meta_test.rs` 的 Go 兼容 JSON round-trip。新增内部字段通常不应加入 `ExternalFields`，而应由 planner/executor 的 `internal` JSON 保留，并测试 `read` 合并后仍存在、`write` 返回行中仍存在。

改变键范围时必须同步检查 `SortedMeta::{from_simple, from_merge, merge}`、planner 中的 `decode`/range splitter 逻辑，以及 Go `SortedKVMeta` 的排他上界契约。不要改成比较 Base64 字符串。改变计数溢出策略会破坏 Go 兼容，需要显式版本化而非悄然改用 checked/saturating 加法。

如果 globalsort 开始依赖 per-file properties 或真实重叠数，应扩展 `FilesStat` 和 `from_merge`/`sort_files` 的双向映射，并在独立测试中证明序列化前后信息不丢失。对象发布语义若需原子性，应在上层引入临时路径、提交标记或清理协议；仅修改 `write` 的顺序不足以提供跨对象存储与 SQL 的事务。

测试必须保持独立文件，不得嵌入生产 `.rs`。至少同步：`modify_column_cloud_meta_test.rs`（线格式、空/null、非法 Base64、覆盖与失败路径），涉及 merge/import 发布行为时再同步 `modify_column_cloud_executor_test.rs`；Go 协议变化还应对照 `pkg/ingestor/globalsort/util_test.go` 和相关 DDL 回填测试。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/session/runtime/modify_column_cloud_meta.rs` 确认目标文件已索引；`node --file ... --offset 1 --limit 260` 与 `--offset 260 --limit 80` 读取了完整 278 行、17 个符号，并报告该文件被 15 个文件使用；`query SortedMeta --kind struct`、`query ExternalFields --kind struct` 定位到本文件。精确 `callers/callees` 查询无输出，故调用边另由直接引用核验。
- Rust 源与装配：`pkg/session/runtime/modify_column_cloud_meta.rs`、`pkg/session/runtime.rs`、`pkg/session/Cargo.toml`。
- Rust 直接调用方：`pkg/session/runtime/modify_column_pipeline.rs`、`modify_column_dist_backfill.rs`、`modify_column_cloud_planner.rs`、`modify_column_cloud_executor.rs`。
- Rust 测试：`pkg/session/runtime/modify_column_cloud_meta_test.rs` 验证 Go 指针/外部 JSON round-trip、内部字段保留、文件对恢复、二进制键比较与 `u64` 回绕；`modify_column_cloud_executor_test.rs` 验证成功发布及失败/取消不发布。
- Go 对照：`pkg/ddl/backfilling_dist_executor.go` 的 `BackfillSubTaskMeta`、解码与外部写入；`pkg/ingestor/globalsort/util.go` 的 external field marshal、`BaseExternalMeta`、`SortedKVMeta`；`pkg/ingestor/engineapi/engine.go` 的 `ConflictInfo`。
- Go 测试：`pkg/ingestor/globalsort/util_test.go` 的 `TestSortedKVMeta`、internal/external marshal 用例与 `TestReadWriteJSON`。
- 人工复核结论：本文描述的是当前已接线实现；没有把 RustCodeGraph 未返回的调用边、未在本文件实现的 Go row-bound fallback，或对象存储/SQL 跨系统原子性写成已支持能力。
