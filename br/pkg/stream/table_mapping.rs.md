# `br/pkg/stream/table_mapping.rs`

## 文件定位

本文件属于 `astersql-br-pkg-stream` library crate。`br/pkg/stream/Cargo.toml` 将 crate 根设为 `lib.rs`，而 `br/pkg/stream/lib.rs` 通过 `#[path = "table_mapping.rs"] pub mod table_mapping` 装配本模块，并在 crate 根执行 `pub use table_mapping::*`。它是 BR 日志备份/PiTR 迁移中的库、表、分区上下游 ID 映射实现：解析 TiDB meta KV，先为日志中发现的对象分配负临时下游 ID，再合并快照或已有 PITR 映射，最后按需批量替换成正式全局 ID。

当前 Rust 接线需要特别区分两层事实。目标文件已提供完整的 canonical `TableMappingManager` API，并被 `table_mapping_test.rs`、`parity_test.rs`、`rewrite_meta_rawkv_test.rs` 等测试直接调用；但 `br/pkg/restore/log_client/batch_meta_processor.rs` 的恢复编排目前导入的是 `crate::stubs::stream::TableMappingManager`，该桩的 `ParseMetaKvAndUpdateIdMapping` 和 `CleanTempKV` 不执行本文件逻辑。因此，Go 版的生产恢复主链是本文件的设计归属，但 Rust 跨 crate 生产接线尚未完成，不能把测试可用等同于恢复流程已经使用 canonical 实现。

## 核心职责

1. `ParseMetaKvAndUpdateIdMapping` 识别事务 meta key 中的数据库、表、自增 ID、序列和 auto-random ID 字段，并创建 `DBReplace`、`TableReplace` 与分区映射。
2. 对 DefaultCF 和 WriteCF 进行事务配对：DefaultCF 暂存 JSON 元信息，WriteCF 根据 `start_ts` 消费暂存，或直接解析 short value；删除和回滚只清理暂存，非 Put 类型被忽略。
3. 维护上游 ID 到下游 ID 的全局一致性。`globalIdMap` 使同一上游表/分区 ID 在不同上下文中复用同一下游 ID，初次发现时由 `generateTempID` 生成 `-1、-2、...`。
4. 把快照恢复或既有 PITR 的正式映射合并进日志扫描结果，并执行过滤、数据库 ID 复用、临时 ID 正式化及 protobuf 往返。
5. 通过 `MetaInfoCollector` 把已提交的库名、表名、分区列表和提交时间戳交给历史信息收集器；映射管理器本身不负责历史版本取舍。

## 主要符号

- `InitialTempId = 0`：临时 ID 计数起点；`generateTempID` 先减一再返回，所以首个临时 ID 是 `-1`。
- `tableMetaKey(dbId, tableId, ts)`、`dbMetaKey(dbId, ts)`：DefaultCF 暂存键，`ts` 是事务 `start_ts`。对应 value 保存解析后的名称/`TableSimpleInfo` 及重复计数。
- `TableMappingManager`：核心可变状态。公开结果是 `DBReplaceMap`；`globalIdMap` 与 `tempIDCounter` 协调 ID 唯一性；两个 `tempDefaultKV*Map` 完成 CF 配对；`noDefaultKVErrorMap` 按 commit timestamp 延迟保存缺失 DefaultCF 的错误；`PreallocatedRange` 记录外部提供的 `[start,end)` 区间。
- `NewTableMappingManager`：创建空管理器，所有 map 为空、PITR 来源标志为 false、计数器和预分配区间归零。
- `ParseMetaKvAndUpdateIdMapping`：单条 meta KV 的公开解析入口。它使用 `ParseTxnMetaKeyFrom` 解码 key，再调用 `meta::Is*Key`/`Parse*Key` 分类。
- `getOrCreateDBReplace`、`getOrCreateTableReplace`：懒创建内部入口；新对象使用负临时 ID，表 ID 优先复用 `globalIdMap`。
- `MergeBaseDBReplace`：三阶段合并已有正式映射：先建立全局 ID 表，再回填当前条目，最后补入当前映射缺少的库、表和分区。
- `ApplyFilterToDBReplaceMap`、`ReuseExistingDatabaseIDs`：分别通过 `PiTRIdTrackerLookup` 单调设置 `FilteredOut`，以及通过 `DatabaseSchemaLookup` 按名称复用现有下游数据库 ID。
- `ReplaceTemporaryIDs`：收集仍被使用的负 ID，检查一对一约束，排序、批量申请正式 ID并回写。
- `ToProto`、`FromDBMapProto`：在 `DBReplaceMap` 与 `PitrDBMap`/`PitrTableMap` 之间转换，保留库表名称、上下游 ID、分区映射及两级 `FilteredOut`。
- `MetaInfoCollector`：解析提交值后的回调边界；`PiTRIdTrackerLookup` 和 `DatabaseSchemaLookup` 是对 Go 依赖的最小适配 trait。

## 执行流程

典型日志扫描流程始于 `NewTableMappingManager`，随后按 DefaultCF 在前、WriteCF 在后的顺序反复调用 `ParseMetaKvAndUpdateIdMapping`。非 meta DB key 立即成功返回。数据库字段先解析上游 DB ID 并确保 `DBReplace` 存在；表、auto-increment、auto-table、sequence、auto-random 字段则确保相应 `TableReplace` 存在。无法识别的字段被忽略，已识别的库/表值只接受 `DefaultCF` 或 `WriteCF`。

DefaultCF 路径把 DBInfo 或 TableInfo JSON 简化成名称/`TableSimpleInfo`，用对象 ID 与 `start_ts` 暂存；同键重复到达时只增加 `count`。WriteCF 首先由 `RawWriteCFValue::ParseFrom` 解码：Delete/Rollback 删除相应暂存，非 Put 不产生映射更新；Put 若带 short value 就直接解析，否则以其 `start_ts` 查找 DefaultCF 暂存。命中后 `count` 减一并调用落地函数，缺失时把包含对象 ID 和 base64 WriteCF value 的错误放入 `noDefaultKVErrorMap[commitTs]`，本次调用仍返回成功，等待 `CleanError` 或 `ReportIfError` 决定最终结果。

库值落地时，仅以非空名称覆盖 `DBReplace.Name`，然后触发 `OnDatabaseInfo`。表值落地时，确保库表映射存在，为尚未出现的分区按 `globalIdMap` 复用或分配临时 ID，仅以非空名称覆盖表名，最后触发 `OnTableInfo`。扫描后可依次合并基线映射、应用选择过滤、复用已有数据库 ID，并调用 `ReplaceTemporaryIDs`；后者按临时 ID 降序（`-1、-2、...`）与申请结果逐项配对并回写库、表、分区，成功后重置计数器。

## 数据与状态

`DBReplaceMap` 的 key 永远是上游 DB ID，value 中的 `DbID` 是下游 ID；其 `TableMap` 和 `PartitionMap` 遵循同一“上游作 key、下游作 value/字段”的约定。负下游 ID 表示尚未正式分配，正 ID 表示来自快照、既有数据库或全局 ID 分配器。`globalIdMap` 跨库共享命名空间，主要用于 exchange partition 等同一物理 ID 在表/分区角色间再次出现时保持下游映射一致。

两个 DefaultCF 暂存 map 与错误 map 都只属于一次可变管理器生命周期。`CleanTempKV` 清空尚未配对的值，但不会清除映射或错误；`CleanError(rewriteTs)` 只移除指定提交时间的延迟错误。`fromPitrIdMap` 由 `SetFromPiTRIDMap` 单向设置，在本文件内不改变算法分支，供外部策略读取。`PreallocatedRange` 也只被记录，不参与 `ReplaceTemporaryIDs` 的分配校验；正式 ID 的数量和内容完全由传入的 `genGlobalIDs` 函数决定。

`HashMap` 迭代顺序未定义，因此 `ToProto` 的库、表、分区输出顺序不稳定，调用者应按集合语义消费。`ReportIfError` 同样只保证返回某一条未清理错误，不保证固定选择哪条。临时 ID 在申请前显式排序，从而使正式 ID 配对顺序稳定。

## 依赖与调用关系

下游依赖包括：`meta_kv::{ParseTxnMetaKeyFrom, RawWriteCFValue}` 负责事务 key/write value 解码；`stubs::meta` 负责识别和解析 TiDB meta 字段；`stubs::model` 从 JSON 提取 DB 名称和 `TableSimpleInfo`；`stubs::{DBReplace, TableReplace, NewDBReplace, NewTableReplace}` 承载替换结果；`stubs::backuppb` 提供 PITR 映射结构；`astersql-br-pkg-utils-consts` 提供 `DefaultCF`/`WriteCF`；`base64` 只用于错误上下文编码。Cargo 直接声明了 utils consts 和 base64，其他类型来自 crate 内模块。

RustCodeGraph 将 `ParseMetaKvAndUpdateIdMapping` 指向同文件的 key 分类、CF 配对、映射创建路径；`ReplaceTemporaryIDs` 指向负 ID 收集、排序、外部分配及回写路径。图与源码搜索显示 canonical API 的直接 Rust 调用者集中在 `br/pkg/stream/table_mapping_test.rs`、`br/pkg/stream/parity_test.rs` 和 `br/pkg/stream/rewrite_meta_rawkv_test.rs`。`br/pkg/stream/lib.rs` 对外再导出全部符号。

Go 生产侧的上游是日志恢复 meta 扫描；Rust 的同名编排位于 `br/pkg/restore/log_client/batch_meta_processor.rs::MetaKVInfoProcessor::ProcessBatch`，但它当前调用该 crate 自己的 `stubs::stream::TableMappingManager`，而不是本文件。`br/pkg/restore/log_client/Cargo.toml` 虽已依赖 `astersql-br-pkg-stream`，当前 canonical table mapping 仍未沿这条依赖接线。这一缺口应作为后续迁移任务处理，不能在本文档任务中扩展修复。

## 错误处理与边界

key、JSON 或 WriteCF 解码失败直接以 `Error` 返回；已识别 DB/表字段使用非 DefaultCF/WriteCF 时返回 `unsupported column family`。`FromDBReplaceMap` 只允许对空管理器装载，防止覆盖既有映射，并以 `ErrRestoreInvalidRewrite` 分类错误。未识别 meta 字段、非 meta DB key、非 Put WriteCF 都是成功忽略。

WriteCF Put 缺少对应 DefaultCF 时采取延迟报错：错误被 commit timestamp 覆盖式存入 map，`ParseMetaKvAndUpdateIdMapping` 返回 `Ok(())`，之后可针对已被重写/过滤的时间点调用 `CleanError`，最终由 `ReportIfError` 返回剩余错误。与 Go 实现相比，Rust 在暂存 `count` 变负时没有日志告警，也不会在计数归零时删除暂存；`count` 当前用于检测/记录重复次数而非控制清理，因此长生命周期扫描应在批次结束调用 `CleanTempKV`。

`ReplaceTemporaryIDs` 拒绝“同一个负下游 ID 对应不同上游 ID”，并标注 `ErrRestoreInvalidRewrite`；同一上下游对重复出现允许通过。它假设 `genGlobalIDs(n)` 成功时恰好返回 `n` 个 ID，源码直接按索引读取，未验证返回长度；适配真实分配器时必须维持这一契约，否则存在越界 panic 风险。`FromDBMapProto` 同样假设输入结构字段已经完整有效，不做重复 ID 或业务一致性校验。

## 并发与资源生命周期

管理器的操作都要求 `&mut self`，内部使用普通 `HashMap`，没有锁、原子变量、线程、异步任务或 channel；本文件不提供跨线程共享保证。合理生命周期是：单一扫描/恢复协调者创建管理器，按 CF 顺序串行喂入 KV，扫描阶段结束清理临时 CF 值，再由同一所有者完成合并、过滤与正式 ID 替换。

`MetaInfoCollector` 也以 `&mut dyn` 传入，回调与映射更新在同一调用栈同步完成；若回调返回前发生 panic，函数没有事务回滚机制。`genGlobalIDs` 是同步函数指针，Rust 版本没有 Go `context.Context` 的取消传播。所有 map、字符串和 protobuf 结构由 Rust 所有权管理，离开管理器或返回值作用域后自动释放；唯一需要显式管理的是何时调用 `CleanTempKV`/`CleanError`，以限定扫描中间态和延迟错误的生命周期。

## 与 Go 版本的对应关系

主体实现逐段对应 `br/pkg/stream/table_mapping.go`：管理器字段、meta key 分类、DefaultCF/WriteCF 配对、short value、Delete/Rollback/非 Put 分支、全局 ID 复用、三阶段基线合并、过滤、数据库 ID 复用、临时 ID 排序替换、proto 往返及预分配区间语义保持一致。Rust 用 trait 替代 Go 的 `MetaInfoCollector`、`*utils.PiTRIdTracker` 和 `infoschema.InfoSchema`，用拥有值的 `HashMap`/protobuf 结构替代 Go 指针 map，并把 Go 的 `context.Context + genGenGlobalIDs` 收窄为同步函数指针。

已确认的差异包括：Rust 未实现 Go 文件末尾的 `UpdateDownstreamIds`，该方法在 Go 中从 snapshot restore 的数据库/建表结果构造基线后调用 `MergeBaseDBReplace`；Rust 的 `SetPreallocatedRange` 不写日志；Rust 缺少 Go 在 CF 数量异常和缺失 DefaultCF 时的结构化 warning；Rust 的恢复 log-client 仍使用本地空桩而非本实现。`ReuseExistingDatabaseIDs` 还显式跳过空名称，语义上避免无意义查询；Go 通过 `SchemaByName("")` 自然不命中。上述差异均应在后续接线/移植任务中单独核验，本文档不把它们描述为已支持。

独立 Rust 测试 `br/pkg/stream/table_mapping_test.rs` 对应 Go `table_mapping_test.go` 的核心场景：proto 往返与 FilteredOut、空/非空基线合并、过滤、负 ID 替换及重复冲突、DefaultCF+WriteCF 的 DB meta 解析、延迟错误清理、已有数据库 ID 复用。`parity_test.rs` 提供跨模块公开契约冒烟，`rewrite_meta_rawkv_test.rs` 验证映射与 schema rewrite 的组合。Rust 测试覆盖比 1710 行 Go 测试更聚焦，不能据此声称所有 Go 边界均已移植。

## 扩展指南

新增一种 meta 字段时，应在 `ParseMetaKvAndUpdateIdMapping` 的分类链中接入正确的 `meta::Is*Key`/解析函数；若字段包含名称或分区等值信息，应同时补齐 DefaultCF 暂存、WriteCF short/default 配对和 `MetaInfoCollector` 语义。新增状态必须明确其清理时机，并保持测试逻辑位于独立的 `br/pkg/stream/table_mapping_test.rs`，不要内嵌到生产文件。

调整 ID 分配时必须维持三项不变量：`DBReplaceMap`/`TableMap`/`PartitionMap` 的 key 是上游 ID；同一上游物理 ID 经 `globalIdMap` 获得一致下游 ID；不同上游 ID 不得共享同一负临时 ID。若允许异步或可取消分配，需要重新设计当前函数指针接口和失败后的部分更新边界。若使用 `PreallocatedRange` 约束分配，还需增加范围、长度和唯一性校验，不能只依赖当前存储字段。

完成生产接线时，优先让 `br/pkg/restore/log_client` 直接使用 `astersql_br_pkg_stream::TableMappingManager` 及真实 `LogBackupTableHistoryManager` 适配，而不是扩写其 `stubs::stream`；同时补齐跨 crate 类型转换和 Go `UpdateDownstreamIds` 所需的快照恢复接口。此类改动应同步更新 `batch_meta_processor` 的独立测试，并验证 DefaultCF 必须早于 WriteCF、批次残留、`CleanTempKV` 收尾和 `ReportIfError` 的调用位置。

性能方面，解析路径每个对象至少进行一次 `HashMap` 查询，分区更新会先收集待插入项以满足借用规则；大表分区数、长时间未清理的 DefaultCF 暂存和一次性 `ToProto` 克隆是主要内存关注点。若引入稳定输出排序，会增加 `O(n log n)` 成本，但可改善持久化和测试的确定性。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/stream/table_mapping.rs` 确认目标文件已索引且包含 53 个符号；`explore "br/pkg/stream/table_mapping.rs table mapping"` 给出了主要方法的内部调用、Go 对照和 Rust 测试调用关系；精确 `query` 定位 `TableMappingManager`、`ParseMetaKvAndUpdateIdMapping`、`MergeBaseDBReplace`、`ReplaceTemporaryIDs`。
- 生产源码：`br/pkg/stream/table_mapping.rs`（完整 809 行），重点核验 `TableMappingManager`、CF 配对辅助函数、`MergeBaseDBReplace`、`ReplaceTemporaryIDs`、三个适配 trait、`ToProto`/`FromDBMapProto`。
- crate 边界：`br/pkg/stream/Cargo.toml`、`br/pkg/stream/lib.rs`；恢复侧接线：`br/pkg/restore/log_client/Cargo.toml`、`batch_meta_processor.rs`、`stubs.rs`。
- Go 对照：`br/pkg/stream/table_mapping.go`（完整 900 行），包括 Go 生产用途说明和 Rust 尚缺的 `UpdateDownstreamIds`；测试对照为 `br/pkg/stream/table_mapping_test.go`。
- Rust 独立测试：`br/pkg/stream/table_mapping_test.rs`（完整 445 行）、`br/pkg/stream/parity_test.rs`、`br/pkg/stream/rewrite_meta_rawkv_test.rs`。本任务按约束不运行 Cargo，测试文件仅用于事实和边界核验。
- 结构验证命令：`test -f br/pkg/stream/table_mapping.rs.md && test "$(rg -c '^## (文件定位|核心职责|主要符号|执行流程|数据与状态|依赖与调用关系|错误处理与边界|并发与资源生命周期|与 Go 版本的对应关系|扩展指南|验证依据)$' br/pkg/stream/table_mapping.rs.md)" -eq 11`。此外人工复核了 11 个固定章节、真实符号、直接调用证据、Go 差异、测试位置和当前未接线限制。
