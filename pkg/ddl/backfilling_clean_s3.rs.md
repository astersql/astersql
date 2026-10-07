# `pkg/ddl/backfilling_clean_s3.rs`

## 文件定位

该文件属于 `astersql-ddl` crate，由 `pkg/ddl/lib.rs` 以 `pub mod backfilling_clean_s3` 公开。它描述分布式 DDL 回填使用云存储全局排序后的收尾逻辑：删除中间文件、在限定条件下聚合计量数据，最后脱敏任务元数据中的云存储 URI。

当前 Rust 模块的定位是可独立测试的领域逻辑，而不是已接入分布式任务框架的运行时 cleaner：全仓 Rust 搜索中，生产代码没有构造或调用 `BackfillCleaner`，仅 `pkg/ddl/backfilling_clean_s3_test.rs` 直接调用它。对应 Go 实现则由 `pkg/ddl/ddl.go:812` 的 `scheduler.RegisterCleanerFactory(proto.Backfill, newBackfillCleaner)` 注册进生产主链。

## 核心职责

- `BackfillCleaner::clean` 按固定顺序编排清理：判空和校验 URI，清理当前 task ID 前缀，必要时清理旧版 job ID 前缀，可选上报计量，然后脱敏 URI。
- `send_meter_on_clean` 将已成功的 Read Index 子任务摘要聚合成行数和索引 KV 字节数。
- `redact_cloud_storage_uri` 复用 `astersql_parser_ast::misc::redact_url`，与 Go `ast.RedactURL` 的云存储敏感 query 参数脱敏规则对齐。
- `CleanupStorage` 和 `send_meter` 回调把外部存储、计量系统与纯编排逻辑解耦，便于独立测试。

## 主要符号

- `TaskState { Pending, Running, Succeed, Failed }`：清理所需的最小任务状态集。只有 `Succeed` 可以通过计量门禁。
- `CleanupTask { id, state, meta }`：清理输入。`id` 是新格式对象前缀，`meta` 是 `backfilling_dist_executor.rs` 定义的 `BackfillTaskMeta`，其 `job_id`、`cloud_storage_uri`、`merge_temporary_index` 和 `version` 被本文件读取或修改。
- `MeteringData { row_count, index_kv_size }`：传给计量回调的聚合值；这里没有 Go `SendRowAndSizeMeterData` 的其他位置参数。
- `CleanupStorage::cleanup_prefix(&mut self, prefix) -> Result<(), String>`：对象存储删除边界。实现者应把“删除该前缀下全部对象”作为成功契约。
- `CleanupError::{InvalidCloudStorageUri, Storage(String), Meter(String)}`：区分输入、删除和计量三类失败边界。
- `BackfillCleaner`：无字段类型；`clean` 是唯一方法，所有可变状态都由参数显式传入。
- `send_meter_on_clean(&[SubtaskSummary]) -> MeteringData`：公开聚合函数，累加 `backfilling_read_index.rs` 中的 `row_count` 和 `processed_bytes`。
- `redact_cloud_storage_uri(&str) -> String`：公开脱敏包装。
- `valid_cloud_storage_uri(&str) -> bool`：私有的最小语法检查，只要存在 `://` 且其前后均非空即通过，它不会创建或连接真实对象存储。

## 执行流程

1. `BackfillCleaner::clean` 先检查 `task.meta.cloud_storage_uri`。空 URI 代表未使用云存储，立即成功返回，不删除、不计量、不改写元数据。
2. 非空 URI 必须通过 `valid_cloud_storage_uri`；失败时返回 `InvalidCloudStorageUri`，且外部副作用尚未发生。
3. 调用 `storage.cleanup_prefix(task.id.to_string())`，删除以分布式任务 ID 为前缀的当前格式中间文件。
4. 若 `task.meta.version < BACKFILL_TASK_META_VERSION_1`，再以 `task.meta.job_id` 为前缀清理旧格式文件。顺序不可随意交换：新前缀可能已删除而旧前缀删除失败。
5. 仅当 `next_generation_kernel == true`、`task.state == Succeed` 且 `merge_temporary_index == false` 时，调用 `send_meter_on_clean` 聚合所有传入的成功 Read Index 子任务摘要，然后将结果传给 `send_meter`。
6. 前述步骤全部成功后，用 `redact_cloud_storage_uri` 就地替换 `cloud_storage_uri`，再返回 `Ok(())`。

## 数据与状态

清理器自身无状态；一次调用的状态分布在 `CleanupTask`、`CleanupStorage` 实现和 `send_meter` 闭包中。`CleanupTask.meta` 复用 `BackfillTaskMeta`，其版本常量 `BACKFILL_TASK_META_VERSION_1 == 1` 来自 `pkg/ddl/backfilling_dist_executor.rs`。

唯一内存内写入是成功尾声对 `task.meta.cloud_storage_uri` 的脱敏。云存储删除和计量回调则是不可由本函数回滚的外部副作用。因此返回错误不表示“什么都没发生”：例如计量失败时文件已删除，而 URI 仍保留未脱敏值。

`send_meter_on_clean` 信任调用者仅传入成功的 Read Index 子任务；Rust 类型本身不带子任务状态，该函数也不做过滤。空切片将产生两个零值计数。

## 依赖与调用关系

上游方面，`pkg/ddl/lib.rs` 公开模块，`pkg/ddl/backfilling_clean_s3_test.rs` 是当前唯一个 Rust 直接调用者。RustCodeGraph 的 `BackfillCleaner` 节点可展示结构体及 `clean` 实现；仓库搜索未找到 Rust 生产构造、scheduler cleaner factory 注册或其他生产调用边。因此不能将 Go 主链的接线状态推导为 Rust 已接线。

下游方面：

- `crate::backfilling_dist_executor::{BackfillTaskMeta, BACKFILL_TASK_META_VERSION_1}` 提供任务元数据与旧版分支界限。
- `crate::backfilling_read_index::SubtaskSummary` 提供 `row_count` 和 `processed_bytes`。
- `astersql_parser_ast::misc::redact_url` 执行真正的 URI query 脱敏。`pkg/ddl/Cargo.toml` 通过路径依赖 `astersql-parser-ast = ../parser/ast` 声明该 crate 边界。
- 存储删除与计量发送都由调用者注入；本文件不直接依赖 Rust 对象存储、DXF storage 或 metering crate。

Go 生产对照链为 `pkg/ddl/ddl.go:812` 注册 factory → `newBackfillCleaner` → `(*BackfillCleaner).Clean`；Go `Clean` 内部直接依赖 `objstore`、`globalsort`、DXF task manager 和 metering handle。

## 错误处理与边界

- 空 URI 是合法的“无需清理”分支，不是错误。
- `valid_cloud_storage_uri` 是轻量检查，不等价于 Go `objstore.ParseBackend`；它可能接受实际存储后端不支持的 scheme 或 authority 形式，真实合法性仍需由未来的存储适配层保证。
- 任一 `cleanup_prefix` 失败都立即包装为 `CleanupError::Storage` 并短路；旧版第二次删除失败时，第一次删除不会回滚。
- `send_meter` 失败被包装为 `CleanupError::Meter`；此时删除已经成功，URI 因短路而尚未脱敏。
- URI 脱敏函数返回 `String` 而不返回 `Result`；其对非支持协议或无法解析输入的具体保留规则由 parser AST 实现决定，并由独立测试锁定。
- API 用 `String` 携带底层错误，没有结构化 source chain、运行时 context 或日志；这些是未来生产接线时需明确的边界。

## 并发与资源生命周期

本文件不启动线程、异步任务或通道，也不持有锁。`clean` 是同步、顺序的单次操作；`&mut CleanupTask` 与 `&mut dyn CleanupStorage` 使同一调用期间的任务和存储句柄受 Rust 独占借用保护，但不保证多次调用之间的分布式互斥。

对象存储连接的创建、关闭、重试和超时完全属于 `CleanupStorage` 实现者。计量端的连接和重试属于 `send_meter` 闭包。本流程的幂等性依赖“重复删除已经不存在的前缀也成功”的存储契约，trait 签名本身没有强制这一点。

从 DDL 作业模型看，这是分布式回填任务结束后的资源回收，不会推进 schema state、更新 schema version 或操作 DDL job 持久化。这一边界是清理代码和 DDL worker 状态机之间的重要分工。

## 与 Go 版本的对应关系

`pkg/ddl/backfilling_clean_s3.go` 是直接语义来源。两版共同保留了如下顺序与规则：空 URI 早返回；先按 task ID 清理；版本 1 之前再按 job ID 清理；只对 next-generation kernel 下成功且非 merge-temp-index 任务计量；计量聚合成功 Read Index 子任务的行数与 processed/index-KV 字节数；最后脱敏 URI。

当前 Rust 实现缩小了基础设施边界，但不应被误读为 Go 运行时的完整替代：

- Go `Clean` 从 `proto.Task.Meta` JSON 反序列化 `BackfillTaskMeta`；Rust 直接接收已类型化的 `CleanupTask`。
- Go 通过 `objstore.ParseBackend` 和 `objstore.NewWithDefaultOpt` 创建外部存储，并调用 `globalsort.CleanUpFiles`；Rust 只面向 `CleanupStorage` trait。
- Go `sendMeterOnClean` 从 DXF task manager 查询成功子任务、解析 JSON summary，并直接发送计量；Rust 要求调用者先筛选并构造 `&[SubtaskSummary]`，再注入发送回调。
- Go `redactCloudStorageURI` 将脱敏后的 meta 重新 JSON 序列化回 `task.Meta`，marshal 失败只记录警告；Rust 仅改写内存中的 `BackfillTaskMeta.cloud_storage_uri`，不承担持久化。
- Go 已实现 `scheduler.Cleaner` 并注册；Rust `BackfillCleaner` 没有相应 framework trait 实现或 factory 注册。

## 扩展指南

- 若接入 Rust 生产主链，首先在 DXF scheduler 边界增加 adapter：负责 task meta 解码、对象存储创建、成功 Read Index 子任务查询、计量发送、脱敏 meta 的持久化以及 cleaner factory 注册；不要只在本文件中模拟这些边界就声称已接线。
- 增加元数据版本时，同步检查 `BackfillTaskMeta` 编解码、对象前缀兼容规则、Go `BackfillTaskMetaVersion*` 和 `clean` 的版本分支；必须保留旧任务可清理性。
- 改变计量口径时，同步修改 `send_meter_on_clean`、`SubtaskSummary` 数据生产端、Go `sendMeterOnClean` 和 `pkg/ddl/backfilling_clean_s3_test.rs`；重点覆盖空 summary、多 summary 聚合和所有计量门禁。
- 改变操作顺序前，明确部分失败的重试语义；特别是“已删文件、计量失败、URI 未脱敏”和“新前缀成功、旧前缀失败”两种状态。
- 扩展 URI 校验或脱敏规则时，优先与 `astersql-parser-ast` 的 `redact_url` 及 Go `ast.RedactURL` 对齐，并在独立测试文件中增加协议、重复 query key、非云 URL 和非法输入用例；不要把测试内嵌进生产 `.rs` 文件。
- 存储适配器应明确前缀删除的幂等、重试、超时和部分成功契约；性能风险主要来自前缀列举/批量删除和子任务 summary 的收集，而不是本文件内的简单遍历。

## 验证依据

- RustCodeGraph 索引状态：`11467` 个文件、`307296` 个节点、`1848419` 条边；`node --file pkg/ddl/backfilling_clean_s3.rs` 读取了全部 165 行，`node BackfillCleaner` 核对了 Rust 结构体和 `clean` 方法。
- RustCodeGraph 精确查询确认了 `BackfillCleaner`、`send_meter_on_clean`、`redact_cloud_storage_uri`、`valid_cloud_storage_uri`、`BackfillTaskMeta` 和 `SubtaskSummary` 的定义；`callers send_meter_on_clean` 在当前索引上长时间无返回而中止，因此调用者结论另用全仓精确符号搜索复核，没有把超时解释为零调用边。
- 已读生产与装配证据：`pkg/ddl/backfilling_clean_s3.rs`、`pkg/ddl/backfilling_dist_executor.rs`、`pkg/ddl/backfilling_read_index.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- 已读 Go 对照与接线证据：`pkg/ddl/backfilling_clean_s3.go` 和 `pkg/ddl/ddl.go:802-812`。
- 已读独立 Rust 测试 `pkg/ddl/backfilling_clean_s3_test.rs`：覆盖敏感 query key 脱敏、重复 key 归一化、新旧前缀顺序、多 summary 聚合、全部计量门禁、空/非法 URI、存储错误与计量错误的短路和脱敏时机。
- 本任务为纯文档分析，按计划不运行 Cargo；文档结构由任务文件指定的 11 章命令校验。
