# `br/pkg/restore/snap_client/tikv_sender.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore-snap-client` library crate；crate 入口 `br/pkg/restore/snap_client/lib.rs` 通过 `pub mod tikv_sender` 装载它，并以 `pub use tikv_sender::*` 重导出公开符号。它对应 Go 包中的 `br/pkg/restore/snap_client/tikv_sender.go`，位于快照恢复的“已建表元数据/备份文件 → 下游物理表顺序 → region split 提示与 SST 文件组 → TiKV 导入”阶段。

当前 Rust crate 的 `Cargo.toml` 明确标注为 arm64 darwin 可用的 local traits/stubs 边界；源码也依赖同 crate 的 `stubs.rs`，而不是直接依赖真实 PD、TiKV、kvproto 或 gRPC crate。因此本文所称“发送”和“恢复”首先描述该 Rust API 的控制流契约，不能等同于 Go 主程序已经具备的完整线上能力。RustCodeGraph 显示该文件由独立测试模块使用；仓库 Rust 源码文本搜索未发现 `RestoreTables` 的应用级调用者，Go 对应方法则由 `br/pkg/task/restore.go` 的 `runSnapshotRestore` 调用。

## 核心职责

1. `getSortedPhysicalTables` 把每个 `CreatedTable` 展开为普通表和所有分区对应的 `PhysicalTable`，并按下游 `NewPhysicalID` 排序，建立确定性的处理顺序。
2. `SortAndValidateFileRanges` 校验每个文件与 rewrite rules 的匹配关系，调用 `MergeAndRewriteFileRanges` 合并、改写范围，然后按字节数、KV 数、文件数量阈值和 `split_on_table` 策略生成 split key 与 `BatchBackupFileSet`。
3. `filterOutFiles` 在 split key 已经选定之后按 checkpoint 排除已导入范围，同时把跳过的 KV/字节计入 summary；这个顺序保证重试时 split key 不受 checkpoint 状态影响。
4. `SnapClient::RestoreTables` 编排 placement rule、范围整理、split、强制分区范围保护、SST 恢复和保护范围移除，并保证主流程结束后尝试重置 placement rules。
5. `sendRequestToStore`、`compactAndCheckSSTRange` 和 `removeForcePartitionRange` 定义向所有可用 TiKV store 广播保护范围请求的边界；`RestoreSSTFiles` 把文件组交给 `SstRestorer`。

本文件没有定义网络协议实现、PD store 枚举算法、文件下载/ingest 细节或 rewrite 算法本体；这些能力分别来自 `ImporterClient`、`GetAllTiKVStoresWithRetry`、`SstRestorer` 和 `MergeAndRewriteFileRanges` 边界。

## 主要符号

- `getSortedPhysicalTables(&[CreatedTable]) -> Vec<PhysicalTable>`：公开自由函数。主表使用 `OldTable.Info.ID → Table.ID`，分区使用 `GetPartitionIDMap` 的旧/新 ID 映射；文件从 `FilesOfPhysicals[old_id]` 获取，不存在时使用空向量；所有条目共享对应 `RewriteRule` 的 clone。
- `filterOutFiles(&HashSet<String>, &[backuppb::File]) -> Vec<backuppb::File>`：公开自由函数。用 `getFileRangeKey(file.Name)` 去掉最后一个下划线后的 CF 后缀形成 checkpoint key，命中者不进入返回值，并累计 `TotalKvs`、`TotalBytes` 及两项 checkpoint-skip summary。
- `MergedRangeCountThreshold: usize = 1536`：跨范围合并的文件数量保护阈值，与 Go 常量一致。判断条件使用 `merged_range_count > 1536`，即达到 1536 本身不会立即切组，修改时不要擅自改成 `>=`。
- `SortAndValidateFileRanges(...) -> Result<(Vec<Vec<u8>>, Vec<BatchBackupFileSet>)>`：核心纯数据路径。返回确定性的已改写 split keys，以及按恢复批次组织的 `(TableID, SSTFiles, RewriteRules)` 文件组。
- `RestoreTablesContext`：一次表恢复的值对象，包含日志进度开关、两种 split 阈值、是否按表切分、online 模式、已创建表、按新物理表 ID 建索引的 checkpoint 集，以及 compact protect 的起止 key。Rust 结构体没有 Go 版本的 `Glue` 字段。
- `SnapClient::RestoreTables(&mut self, &Context, RestoreTablesContext)`：本文件的编排入口。
- `SnapClient::SplitPoints`：当前 Rust 精简实现只调用一次 `on_progress(sorted_split_keys.len())` 并记录 split-key 数量，不创建 region splitter，也不读取 `Context`/`is_raw_kv`。
- `SnapClient::sendRequestToStore`：枚举非 TiFlash store，跳过无 status address 或非 `Up` store；存在 `import_client` 时逐 store 同步执行回调。
- `SnapClient::compactAndCheckSSTRange` / `removeForcePartitionRange`：分别广播 TTL 为 7200 秒的 `AddPartitionRangeRequest` 和对应的 `RemovePartitionRangeRequest`；仅把 code 为 `Unimplemented` 的错误降级为告警。
- `SnapClient::RestoreSSTFiles`：取得 `GetRestorer()`，先 `GoRestore`，成功后 `WaitUntilFinish`。
- `getFileRangeKey(&str) -> String`：取最后一个 `_` 之前的前缀；没有 `_` 时 panic，输入契约来自备份文件名 `{store_id}_{region_id}_{epoch_version}_{key}_{ts}_{cf}.sst`。

## 执行流程

`RestoreTables` 的顺序如下：

1. 用 `pd_store_meta`、`meta_client` 和 `Online` 创建 `PlacementRuleManager`，随后调用 `SetPlacementRule`。这两步失败会立即返回；此时尚未进入需要重置规则的闭包。
2. `SortAndValidateFileRanges` 先展开并按新物理 ID 排序所有表/分区。对每张物理表，先逐文件执行 `ValidateFileRewriteRule`，再让 `MergeAndRewriteFileRanges` 按 size/count 阈值产生已排序、已改写的范围和 CF 统计。
3. 每个合并范围先参与 split 决策：若当前组加上该范围后超过 size/count 阈值，或此前累计文件数已经大于 1536，则提交上一 split key 和上一文件组并从当前范围重新累计；否则继续并入当前组。`last_key` 始终更新为当前范围的 `EndKey`。
4. 在 split 决策之后才调用 `filterOutFiles`。因此 checkpoint 会改变待恢复文件组，却不会改变 `group_size`、`group_count`、`merged_range_count` 或 split key 序列。非空文件会追加到当前组；下游 table ID 变化时在同一批中新增 `BackupFileSet` 条目。
5. 若 `split_on_table` 为真，表结束时清空跨表累计值、丢弃该表最后的候选 key（表边界已提供切分语义），并提交已有文件组。循环结束后再提交剩余 key/文件组，记录 default/write CF 文件数。
6. `RestoreTables` 把 split keys 交给当前精简 `SplitPoints`。保护范围满足 `start < end` 时，广播 add-force-partition-range；否则只告警并跳过。
7. 调用 `RestoreSSTFiles`，即 `GoRestore` 后 `WaitUntilFinish`。之后在有效保护范围上广播 remove-force-partition-range；无效范围同样只告警。
8. 无论闭包内步骤成功或失败，函数都会尝试 `ResetPlacementRules`；重置失败只告警，最终返回闭包的原始结果。

## 数据与状态

核心算法的临时状态全部局限于 `SortAndValidateFileRanges`：`group_size`/`group_count` 是当前跨范围批次的累计量，`last_key` 是尚未提交的最后范围尾键，`merged_range_count` 统计 checkpoint 过滤前的文件数，`last_files_group` 保存尚未提交的恢复批次。`total_write_cf_file` 与 `total_default_cf_file` 仅用于 summary。

`checkpoint_set_with_table_id` 以**下游** `NewPhysicalID` 查找集合；集合元素不是完整文件名，而是 `getFileRangeKey` 生成的去 CF 后缀范围键，所以同一范围的 write/default CF 文件会一并视为已完成。文件输出使用 clone，不修改 `CreatedTable` 或原始文件列表。

跨调用的可变状态位于 `SnapClient`：placement rule manager 使用其 PD/meta 边界；`sendRequestToStore` 读取 `import_client`；`RestoreSSTFiles` 通过 `GetRestorer` 延迟初始化并复用 `self.restorer`。当前默认 restorer 是 `stubs.rs::SimpleRestorer`，它把每个 group clone 到内存 `restored` 列表、逐组回调进度，`WaitUntilFinish` 直接成功。

## 依赖与调用关系

RustCodeGraph 的直接调用边为：`RestoreTables → SortAndValidateFileRanges → getSortedPhysicalTables / filterOutFiles → getFileRangeKey`；`RestoreTables → SplitPoints`；`RestoreTables → compactAndCheckSSTRange / removeForcePartitionRange → sendRequestToStore`；`RestoreTables → RestoreSSTFiles`。独立测试 `test_get_sorted_physical_tables` 调用排序入口，`test_sort_and_validate_file_ranges` 调用范围入口。

主要下游依赖如下：

- `pipeline_items.rs::PhysicalTable` 承载展开后的新旧物理 ID、rewrite rules 和文件。
- `stubs.rs::GetPartitionIDMap`、`ValidateFileRewriteRule`、`MergeAndRewriteFileRanges` 提供 ID 映射、规则校验和范围合并/改写。
- `placement_rule_manager.rs::NewPlacementRuleManager` 负责恢复前设置、恢复后重置 placement rules。
- `stubs.rs::GetAllTiKVStoresWithRetry` 调用 `StoreMeta::GetAllStores` 并过滤 TiFlash；本文件进一步检查 address 和 `Up` 状态。
- `stubs.rs::ImporterClient` 定义 add/remove force partition range RPC 边界；`stubs.rs::SstRestorer` 定义 `GoRestore`/`WaitUntilFinish` 生命周期。
- crate 入口把本文件 API 重导出，但当前 Rust 仓库中除测试外未找到 `RestoreTables` 调用；Go 主链的对应边是 `br/pkg/task/restore.go::runSnapshotRestore → SnapClient.RestoreTables`。

`Cargo.toml` 的直接依赖包括 restore/utils/errors 等 workspace crate 以及 serde/sha2，但本文件实际通过 crate-local 模块和 stubs 消费这些能力；它没有 feature 条件或 `cfg` 分支。

## 错误处理与边界

- rewrite rule 校验或范围合并失败通过 `?` 原样终止，且不会返回部分 split keys/文件组。
- `NewPlacementRuleManager` 或 `SetPlacementRule` 失败会在恢复闭包建立前返回；尤其 `SetPlacementRule` 部分成功后的外部回滚语义取决于 manager 自身，本文件不会调用 `ResetPlacementRules`。
- 闭包开始后，split、add-range、restore、remove-range 的第一个错误会短路后续步骤；之后仍尝试 reset placement rules。reset 的错误只告警，不覆盖主错误，也不会让原本成功的恢复变为失败。
- add/remove RPC 的 `Unimplemented` 被视为兼容旧 TiKV 的成功降级；其他错误以 `Error::Trace` 返回。store 枚举失败直接返回。
- `import_client == None` 时，`sendRequestToStore` 对每个 store 都不调用回调并最终返回成功。这是当前 Rust 边界的静默 no-op，调用方不能据此证明 RPC 已发送。
- 空表/空范围会自然得到空 split keys 和空恢复组；默认 `SimpleRestorer` 对空组成功。无效 compact protect 区间也只跳过 add/remove 请求。
- `getFileRangeKey` 对不含 `_` 的文件名会 panic，而不是返回 `Result`；调用者必须保证备份数据文件命名契约。

## 并发与资源生命周期

当前 Rust 文件自身不创建线程、任务、通道或锁。`sendRequestToStore` 按 store 顺序同步调用闭包，遇到首个非兼容错误即停止；这与 Go 版本使用 `errgroup` 加按 store 数量建立的 worker pool 并发广播不同。`SplitPoints` 也没有 Go 版本 region splitter 的异步 split/scatter 行为。

`RestoreSSTFiles` 保留 `GoRestore → WaitUntilFinish` 两阶段接口，因此注入的真实 `SstRestorer` 可以在 `GoRestore` 内启动后台工作，并由 `WaitUntilFinish` 负责汇合；默认 `SimpleRestorer` 则完全同步。该方法不调用 `Close`，restorer 的长期释放责任属于 `SnapClient` 的更高层生命周期。

force partition range 的资源生命周期理想顺序为 add → restore → remove；但若 `RestoreSSTFiles` 失败，闭包会在 remove 之前返回，因此本函数不会清理已经添加的范围。placement rules 与之不同，闭包后总会尝试 reset。TTL 7200 秒为 add-range 遗留提供服务端失效上限，但 Rust stub 本身不实现 TTL。

## 与 Go 版本的对应关系

算法上，Rust 保留了 Go 的表/分区展开与按下游 ID 排序、rewrite 校验、跨表范围合并、1536 文件阈值、checkpoint 在 split 选择之后过滤、按表切分、CF 统计以及 add/remove 请求对 `Unimplemented` 的兼容策略。`tikv_sender_test.rs` 的主要矩阵对照 Go 测试，覆盖不同阈值、是否跨表合并、是否启用 checkpoint，以及非均匀 size/count 统计。

仍存在明确差异：

- Go `RestoreTablesContext` 带 `Glue`，用两个 `glue.WithProgress` 阶段包装 split/scatter 与 download/ingest；Rust 丢弃 `LogProgress`，传入的是空回调。
- Go `SplitPoints` 构造真实 split client/region splitter，支持 raw KV、最大 split key 数、region index step 和 coarse scatter；Rust 只汇报 key 数。
- Go `sendRequestToStore` 并发广播并由 errgroup 汇合；Rust 串行执行，而且缺失 importer 时静默成功。
- Go `RestoreSSTFiles` 带 failpoint、checkpoint runner，并把批次展开给完整 restorer；Rust 没有 failpoint，使用当前 `GetRestorer()` 边界，默认只是内存记录。
- Go 记录真实阶段耗时；Rust对 merge duration 固定记录 0 秒，split 仅记数量。

因此可以认为范围整理算法已被移植和测试，但真实 region split/scatter、并发 store RPC、下载/ingest 与应用主链接线仍未由本文件证明完成。

## 扩展指南

- 扩展排序/分组策略时优先修改 `getSortedPhysicalTables` 或 `SortAndValidateFileRanges`，并同步扩展独立文件 `tikv_sender_test.rs`；必须保留“先选 split key、后 checkpoint 过滤”的重试确定性，以及按 `NewPhysicalID` 排序的不变量。
- 修改 checkpoint key 规则时同步检查 `getFileRangeKey`、checkpoint 写入方和 Go 的 `getFileRangeKey`。若要消除 panic，应先确认所有调用者和持久化格式，再把签名升级为 `Result`，不能只在此处容错造成键不一致。
- 接入真实 split/scatter 应替换 `SplitPoints` 的精简体，并从 `SnapClient` 接入 Go 同等的 PD/HTTP/TLS、`maxSplitKeysOnce`、store count、region index step、coarse scatter 和 raw-KV 选项；需要新增独立 Rust 测试验证取消、部分 split 失败、进度次数及空 key。
- 接入真实 TiKV 请求时，应让 importer 缺失成为显式初始化错误，并评估是否复刻 Go 的并发/取消语义；测试至少覆盖 TiFlash/离线/空地址过滤、首错传播、`Unimplemented` 降级、多 store 并发和 context 取消。
- 扩展 restorer 生命周期时修改 `RestoreSSTFiles` 和 `SnapClient::GetRestorer` 的实现边界，测试 `GoRestore` 失败时不等待、等待失败传播、后台任务汇合和 Close 所有权。测试逻辑继续放在 `tikv_sender_test.rs` 或其他独立 `*_test.rs`，不要内嵌进生产文件。
- 调整 add/remove 清理策略时重点处理 restore 失败后的 remove 是否应执行、remove 错误是否应与主错误合并，以及 7200 秒 TTL 的兼容性；任何变化都要与 Go 行为和旧 TiKV 的 `Unimplemented` 约定对齐。

## 验证依据

- 生产源：`br/pkg/restore/snap_client/tikv_sender.rs`，核对了 1–452 行全部符号与控制流；直接结构来源还包括 `client.rs::SnapClient/GetRestorer`、`pipeline_items.rs::PhysicalTable`、`placement_rule_manager.rs` 和 `stubs.rs::ImporterClient/SstRestorer/SimpleRestorer/GetAllTiKVStoresWithRetry`。
- crate 边界：`br/pkg/restore/snap_client/Cargo.toml` 与 `lib.rs`，确认 library 路径、porting 元数据、local stubs 约束、模块装载及公开重导出。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/snap_client` 定位了 39 个相关 Go/Rust 文件；`query/node/explore` 核对了本文件符号、源码和直接调用边。图中关键边包括 `RestoreTables → SortAndValidateFileRanges/SplitPoints/compactAndCheckSSTRange/RestoreSSTFiles/removeForcePartitionRange`、`SortAndValidateFileRanges → getSortedPhysicalTables/filterOutFiles`、`filterOutFiles → getFileRangeKey`。
- Rust 测试：`br/pkg/restore/snap_client/tikv_sender_test.rs`；确认 `test_get_sorted_physical_tables` 的分区展开排序断言，以及 `test_sort_and_validate_file_ranges` 对 threshold、split-on-table、checkpoint、非均匀 size/count 的矩阵覆盖。测试明确不发真实 TiKV RPC。
- Go 对照：`br/pkg/restore/snap_client/tikv_sender.go` 与 `tikv_sender_test.go`，核对完整 split/scatter、并发 store 广播、进度、failpoint、restorer/checkpoint 和文件名契约；RustCodeGraph 还给出 Go 主链 `br/pkg/task/restore.go::runSnapshotRestore → RestoreTables`。
- 接线限制：RustCodeGraph 将 `tikv_sender.rs` 标为由 `tikv_sender_test.rs` 使用；`rg -n 'RestoreTables\\(' --glob '*.rs'` 除定义外无结果。因此“Rust 应用主链尚未发现调用”是当前仓库证据，不推断未来接线状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文未把 stub 行为描述成完整生产能力。
