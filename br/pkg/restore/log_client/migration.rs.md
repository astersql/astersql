# `br/pkg/restore/log_client/migration.rs`

## 文件定位

`migration.rs` 属于 `astersql-br-pkg-restore-log-client` library crate。`br/pkg/restore/log_client/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定 crate 根；`lib.rs` 以 `#[path = "migration.rs"] pub mod migration` 挂载本文件，并通过 `pub use migration::*` 将公开类型和函数扁平再导出。Cargo 清单没有控制本模块的 feature，因此它随该 crate 正常编译；直接依赖包括本地迭代器 crate `astersql-br-pkg-utils-iter`，以及用于解析压缩注释的 `serde`/`serde_json`。

在 PITR 日志恢复链路中，本文件位于“读取 migration 元数据”与“枚举实际恢复文件”之间：它把 `Migration` 中的元数据删除、compaction 产物目录和 ingested SST 元数据路径汇总成 `WithMigrations`，再把过滤规则套到 `LogFileManager` 的 meta → physical group → logical file 迭代链上。它不负责发现 migration 文件、不直接导入 SST，也不解析普通日志 backup meta；这些职责分别由上层客户端、`log_file_manager.rs` 及 stream/storage 辅助层承担。

目标包目录没有 `doc.go`。最近的模块契约来自 `lib.rs`，生产接线可在 `client.rs` 与 `log_file_manager.rs` 中核对，独立 Rust 测试由 `lib.rs` 的 `#[cfg(test)] #[path = "migration_test.rs"]` 挂载。

## 核心职责

本文件提供四组连续职责：

1. 用 `MetaSkipMap` 表达 meta 路径、物理文件路径和逻辑 `RangeOffset` 三层删除，并用 `skipMeta`、`skipPhysical`、`skipLogical` 保持“父层整删后不再记录子层细删”的不变量。
2. `WithMigrationsBuilder` 按恢复时间窗粗过滤 migration，将未过滤项的 `EditMeta`、`Compactions[].Artifacts` 与 `IngestedSstPaths` 汇总为一个不可变的遍历视图。
3. 为 retain-latest-MVCC 模式解析 compact-log-backup JSON 注释，验证 `cal-shift-ts`、`minimal-compaction-size`、时间覆盖和分片完整性。
4. 通过 `WithMigrations::Metas`、`MetaWithMigrations::Physicals`、`PhysicalWithMigrations::Logicals`、`Compactions` 和 `IngestedSSTs` 构造惰性迭代器，在消费时过滤被迁移删除或超出恢复窗口的对象。

该文件是过滤与视图构建层，不修改外部存储，也不回写 migration。`Build` 只聚合内存状态；真正的存储读取发生在后续消费 `Compactions`/`IngestedSSTs` 迭代器时。

## 主要符号

- `LogicalSkipMap = HashMap<u64, ()>`：记录应跳过的逻辑文件 offset；值是占位单元，集合语义由键承担。
- `LogicalFileSkipMap { skipmap, skip }`：一个物理文件的删除状态。`skip=true` 表示整物理文件删除，此时 offset 子表不再有意义。
- `PhysicalSkipMap = HashMap<String, LogicalFileSkipMap>` 与 `PhysicalFileSkipMap { skipmap, skip }`：按物理路径组织子删除；外层 `skip=true` 表示整份 meta 删除。
- `MetaSkipMap = HashMap<String, PhysicalFileSkipMap>`：`WithMigrations` 的核心索引，第一层键必须与 `MetaName.name` 一致。
- `skipMeta`、`skipPhysical`、`skipLogical`：分别写入三级删除。后两者遇到已整删的父层会短路；`skipLogical` 遇到已整删的物理文件也会短路。
- `NeedSkip`：直接查询三级表，meta、physical 或 offset 任一层命中删除即返回 `true`；不存在的键返回 `false`。生产遍历主要使用三个包装迭代器，而该函数为直接查询与 parity 测试提供接口。
- `WithMigrationsBuilder { shiftStartTS, startTS, restoredTS }`：构建期时间配置。`new` 令 `shiftStartTS=startTS`，`SetShiftStartTS` 可在构建前调整压缩输入的过滤下界。
- `updateSkipMap`：把 `MetaEdit` 的 `DestructSelf`、`DeletePhysicalFiles`、`DeleteLogicalFiles[].Spans[].Offset` 合并到三级表；`DestructSelf` 优先并跳过该 edit 的子项。
- `coarseGrainedFilter`：检查 `Migration.Compactions`。只要存在一个 `InputMinTs/InputMaxTs` 均非零且完全落在 `[shiftStartTS, restoredTS]` 外的 compaction，就返回 `true` 并让 `Build` 丢弃整条 migration；零边界被视为旧格式/未知范围，不触发粗过滤。
- `Build`：汇总未被粗过滤 migration 的 skip map、`Artifacts` 与 `IngestedSstPaths`，生成 `WithMigrations`。
- `CompactLogBackupComment*`：`serde` 私有 DTO，映射注释 JSON 的 `config`、`from-ts`、`until-ts`、`cal-shift-ts`、`minimal-compaction-size` 与 `shard`。
- `RetainLatestMVCCCompactionInterval`：公开的覆盖区间和分片坐标；`compactLogBackupCompactionIntervalForRetainLatestMVCC` 负责解析和校验单个 compaction。
- `hasCompleteShardCoverage`、`retainLatestMVCCCompactionsCover`：前者判断一个子区间是否被某个相同 `shardTotal` 的完整分片集合覆盖，后者按所有交界点切分恢复窗口并逐段检查。
- `ValidateRetainLatestMVCCCompactionCoverage`：聚合所有可参与验证的 interval，要求完整覆盖 `[startTS, restoredTS]`。
- `MetaWithMigrations`、`PhysicalWithMigrations`：把对应层的数据与下一层 skip map 绑在一起，供逐层惰性过滤。
- `WithMigrations`：构建结果，保存三级 skip map、compaction 目录、ingested SST 路径及三个时间戳；其 `Metas`、`Compactions`、`IngestedSSTs` 是消费入口。

本文件没有模块级常量、条件编译项、异步函数或自定义 trait。

## 执行流程

常规生产主链如下：

1. `client.rs::InstallLogFileManager` 用恢复的 `startTS`、`restoreTS` 创建 `WithMigrationsBuilder`，先对空 migration 列表调用 `Build`，再把 builder 与初始 `WithMigrations` 一并交给 `CreateLogFileManager`。
2. 上层获得真实 migration 后，`LogFileManager::BuildMigrations` 调用同一 builder 的 `Build`。每条 migration 先经过 `coarseGrainedFilter`；一旦其中任一有效 compaction 完全越界，该 migration 的 meta edit、所有 compaction 目录和 ingested SST 路径都会整体忽略。
3. 对保留项，`updateSkipMap` 先应用 meta/physical/logical 删除，随后 `Build` 追加每个 compaction 的 `Artifacts` 和所有 `IngestedSstPaths`。结果保留 builder 的三个时间戳供消费阶段继续过滤。
4. `LogFileManager::FilterDataFiles` 先调用 `WithMigrations::Metas` 剔除整 meta 删除项，再对每个幸存项调用 `MetaWithMigrations::Physicals` 剔除整物理文件删除项，最后调用 `PhysicalWithMigrations::Logicals` 按 `DataFileInfo.RangeOffset` 剔除逻辑 span。其后 `log_file_manager.rs` 还会叠加 `IsMeta` 与 TS 过滤，并把结果转换为 `LogDataFileInfo`。
5. `LogFileManager::GetCompactionIter` 调用 `WithMigrations::Compactions`。后者为每个 `compactionDirs` 元素创建 `Subcompactions(ctx, dir, storage, shiftStartTS, restoredTS)`，再用 `ConcatAll` 串接；实际目录扫描、解码和 input TS 过滤由 `Subcompactions` 承担。
6. `LogFileManager::GetIngestedSSTs` 调用 `WithMigrations::IngestedSSTs`。后者通过 `stream::LoadIngestedSSTs` 按 `fullBackups` 加载并按 backup UUID 聚合，丢弃未完成组和 `GroupTS` 不在闭区间 `[startTS, restoredTS]` 内的组，再把组内 `PathedIngestedSSTs` 展平成 `IngestedSSTs`。

retain-latest-MVCC 验证是独立入口：`LogFileManager::ValidateRetainLatestMVCCCompactionCoverage` 委托 builder 遍历全部 compaction。单项解析仅接纳显式 `cal-shift-ts=true` 且 `minimal-compaction-size=0` 的注释；缺少 `from-ts`/`until-ts` 时回退到 protobuf 字段；缺少 shard 时视为 `1/1`。聚合器收集有效 interval，按恢复窗口、interval 起止点排序去重形成连续子段，并要求每个子段至少存在一套完整分片。

## 数据与状态

skip map 的关键不变量是“整删覆盖细删”。`skipMeta` 会用 `skip=true` 的新值替换已有 meta 条目并清空子表；随后针对该 meta 的 physical/logical 删除会短路。`skipPhysical` 同样用 `skip=true` 的物理条目替换其旧 offset 表；后续 logical 删除不再写入。这使 migration 顺序在“父层删除晚于子层删除”时仍收敛到父层整删状态。反方向上，父层整删之后不会被子层操作重新打开。

`Build` 每次创建新的 `HashMap` 和路径 `Vec`，不会复用旧 `WithMigrations` 的状态。路径和子 map 在构建/包装时使用拥有值或克隆，因此返回的惰性迭代器不借用 builder 或 `WithMigrations`；代价是 `Metas`、`Physicals`、`Logicals` 每次创建管道都会克隆相应 skip map。`compactionDirs` 与 `fullBackups` 保持输入 migration 的遍历顺序，文件本身不在 `Build` 时读取。

三个时间戳用途不同：`shiftStartTS` 是 compaction 粗过滤和 `Subcompactions` 的下界；`startTS` 是 ingested SST 与 retain-latest-MVCC 覆盖的恢复起点；`restoredTS` 是三条路径共享的上界。`new(startTS, restoredTS)` 只提供两参数便利构造，测试需要三者不同值时通过 `export_test.rs::NewMigrationBuilder` 直接填字段。

覆盖算法将区间视为含端点，但实现通过相邻边界子段检查连续覆盖。`startTS >= restoredTS` 被视为空窗口并直接成功。`hasCompleteShardCoverage` 按 `shardTotal` 分组、用 `HashSet` 去重 index；某一 total 的不同 interval 只要都完整覆盖当前子段且收齐 `1..=total` 数量即可通过。解析函数已拒绝 0 和大于 total 的 index，因此“集合长度等于 total”足以代表完整集合。

## 依赖与调用关系

上游调用关系：

- `lib.rs` 声明并再导出模块，也在 `cfg(test)` 下挂载 `migration_test.rs`、`export_test.rs` 和 `parity_test.rs`。
- `client.rs::InstallLogFileManager` 创建 builder 和空的初始视图。
- `log_file_manager.rs::BuildMigrations` 是真实 migration 汇总入口；`ValidateRetainLatestMVCCCompactionCoverage` 是覆盖校验的门面。
- `log_file_manager.rs::FilterDataFiles` 消费 `Metas → Physicals → Logicals`；`GetCompactionIter` 与 `GetIngestedSSTs` 分别消费另外两条产物链。

下游依赖关系：

- `astersql_br_pkg_utils_iter` 的 `MapFilter`、`FilterOut`、`FlatMap`、`Map`、`FromSlice`、`ConcatAll` 和 `TryNextor` 组成同步惰性迭代管道；谓词返回 `true` 代表过滤掉元素。
- `log_file_manager.rs` 提供 `MetaName`、`GroupIndex`、`FileIndex` 及三种迭代器别名，并提供 `Subcompactions` 读取 compaction 产物。
- `stubs::backuppb` 提供本地迁移协议模型；当前 Rust `Migration` 仅有 `EditMeta`、`Compactions`、`IngestedSstPaths` 三组字段。
- `stubs::storeapi::Storage` 是外部存储边界；`Context`、`Error`、`Result` 与 `berrors::ErrInvalidArgument` 承担错误表示。
- `stubs::stream::{LoadIngestedSSTs, IngestedSSTsGroupExt}` 负责 ingested 元数据读取、分组及 `GroupFinished`/`GroupTS` 解释。
- `serde_json` 仅用于 compact-log-backup `Comments` 的 JSON 反序列化。

RustCodeGraph 的文件级索引把 `client.rs`、`log_file_manager.rs`、`migration_test.rs`、`export_test.rs`、`log_file_manager_test.rs` 等列为使用方；精确符号检索确认上述生产调用位置。图的 `callers/explore` 查询在本地超时，因此调用链结论以已索引源码和精确引用为准。

## 错误处理与边界

- `Build`、三级 skip 写入和三层文件过滤都是无错误返回的内存操作。空 migration、空 edit、空目录列表均生成合法空视图。
- `coarseGrainedFilter` 的粒度是整条 migration，不是单个 compaction。实现采用“任一有效 compaction 越界即过滤整条”的语义；扩展时不能误写成仅删除那个 compaction，也不能把源注释中“所有”理解为实现事实。
- `InputMinTs == 0` 或 `InputMaxTs == 0` 使该 compaction 的范围无效，因此不会仅凭它触发粗过滤；`migration_test.rs::test_filter_out` 直接覆盖这一旧格式兼容边界。
- compact 注释为空、缺 `config`、未启用 `cal-shift-ts` 或 `minimal-compaction-size != 0` 时返回 `ok=false`，不是解析错误；它们不参与覆盖，最终可能由整体校验报“coverage incomplete”。
- 非法 JSON、`fromTS > untilTS`、分片 index/total 为零或 index 大于 total 会立即返回带 `ErrInvalidArgument` 根因的注解错误。整体覆盖不足同样返回 `ErrInvalidArgument`，消息包含要求和目标窗口。
- 注释省略 `from-ts`/`until-ts` 时会回退到 `LogFileCompaction` 字段；两处都为零时形成 `[0,0]` 的合法 interval，是否有用由目标窗口决定。
- `Compactions` 和 `IngestedSSTs` 返回惰性 `TryNextor`，存储遍历、读取或解码错误在消费 `TryNext` 时传播，而不是在构造迭代器时发生。Rust 测试验证损坏的 compaction JSON 和 ingested 元数据错误不会被静默吞掉。
- `NeedSkip` 对缺失键返回 `false`。同路径 Go `migration.go` 当前把 `exists` 条件写反，随后可能解引用 nil；Rust 有意采用 `LogFilesSkipMapExt` 的正常语义并由 `parity_test.rs` 验证，不应为了逐字翻译恢复该缺陷。
- Go `Build` 留有 `TruncatedTo`、`DestructPrefix` 的 TODO；当前 Rust stub `Migration` 也没有这些字段。因此文档只能确认三类已接线数据，不能声称支持前缀销毁或截断语义。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务，全部 API 为同步调用。builder 与 `WithMigrations` 都拥有自己的时间戳和集合；迭代器通过克隆捕获过滤状态，因此创建后不依赖调用者继续持有原对象，也不会回写原对象。

`Storage` 以共享 trait 引用 `&dyn Storage` 传入，所有权和连接关闭由 `LogFileManager`/具体 storage 实现管理。`Compactions` 先为每个目录构造子迭代器并由 `ConcatAll` 顺序消费；`IngestedSSTs` 同样是串行组合管道。本文件没有并发读取保证以外的同步机制，若未来并行化，必须保持输出错误传播、同 backup UUID 的完成态聚合和确定的过滤边界。

资源消耗主要来自 map/path 克隆、覆盖验证的边界排序，以及按每个子段扫描全部 interval。现有覆盖算法大致为排序加 `O(B×I)` 检查（`B` 为去重边界数，`I` 为 interval 数）；一般 compaction 数量较小。若优化算法，需保持“同一子段必须由同一个 shardTotal 的完整分片组覆盖”，不能把不同 total 的分片混合计数。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `migration.go`：三级 skip map、`WithMigrationsBuilder`、粗过滤、retain-latest-MVCC 注释模型与覆盖算法、三层迭代包装、compaction 目录串接和 ingested SST 分组过滤都保留了相同的数据流。`migration_test.rs` 也按 `migration_test.go` 的 `TestMigrations`、`TestFilterOut`、`TestRetainLatestMVCCCompactionCoverage` 及 ingested SST 用例组织。

主要语言适配包括：Go 的 map/pointer/nil 由 Rust `HashMap`、拥有值与 `Option` 表达；Go 的多返回值 `(interval, bool, error)` 变为 `Result<(interval, bool)>`；Go 迭代器接口映射为 `Box<dyn TryNextor<T>>`；JSON 指针字段映射为 `Option<T>`。Rust `MetaWithMigrations` 额外保存 `name`，因为过滤键需要保留外部 meta 路径；Go 类型只保存内部 `meta` 与 skip map。

有三点差异必须显式保留：

1. Rust `NeedSkip` 修复了 Go 当前的 `exists` 条件反转，缺失 meta/physical 时安全返回 `false`。
2. Go 的公开/私有性由包边界控制；Rust 为跨模块和测试适配公开了更多类型/字段。公开不代表调用方应绕过 builder 破坏时间戳或 skip-map 不变量。
3. Rust `Migration` 是本 crate `stubs.rs` 中的迁移模型，当前不包含 Go TODO 提到的 `TruncatedTo`/`DestructPrefix`；因此这是已移植子集，不是完整协议能力证明。

Rust 测试还增加了 `compactions_loads_storage_artifacts_and_filters_by_input_ts` 与 `compactions_propagates_artifact_decode_errors`，直接证明 `Compactions` 已接到 storage/解码路径；Go 测试则用真实 local storage 与 failpoint 覆盖 ingested SST 错误。两边共同支持过滤语义，但运行环境和错误注入机制不同。

## 扩展指南

- 增加 migration 编辑类型时，先在 `updateSkipMap` 或 `Build` 明确其覆盖优先级，再同步本地 `stubs::backuppb::Migration`/`MetaEdit` 与真实协议接线；不要把 Go 的 TODO 直接描述成已实现。测试必须放在独立 `migration_test.rs`，并对照 `migration_test.go` 补充父删/子删顺序和重复 edit 用例。
- 修改粗过滤规则时，同时复核 `coarseGrainedFilter`、`Build` 以及 `test_filter_out`。尤其要决定过滤粒度是 compaction 还是整条 migration，因为当前整条丢弃会连带移除 edit 和 ingested 路径，兼容性影响远大于目录筛选。
- 扩展 compact-log-backup 注释时，在私有 serde DTO 与 `compactLogBackupCompactionIntervalForRetainLatestMVCC` 接入。新增可选字段应区分“不参与覆盖”和“配置非法”，并在 Rust/Go 独立测试中同时覆盖缺失、非法值和 fallback。
- 改动覆盖算法时保持三个不变量：窗口无空洞、每个子段分片齐全、不同 `shardTotal` 不混用。大规模 interval 优化前应增加重叠区间、重复 shard、交错 total 和边界相接的回归测试。
- 修改三层过滤键时，要联动 `MetaName.name`、`DataFileGroup.Path` 和 `DataFileInfo.RangeOffset` 的生产者；路径规范化或 offset 语义变化会导致静默漏过滤，应通过 `FilterDataFiles` 的端到端迭代断言验证。
- 为 `Compactions`/`IngestedSSTs` 引入并行或缓存时，保持惰性错误可观察、storage 生命周期不被迭代器越界借用，并评估路径/map 克隆与输出顺序的兼容性。
- 新增测试继续由 `lib.rs` 以独立 `*_test.rs` 文件挂载，不要把测试模块嵌入 `migration.rs`。任何有意偏离 Go 的行为都应像 `NeedSkip` 一样记录原因和直接测试证据。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件，其中 Rust 7,032 个；索引可读取目标文件。
- RustCodeGraph `files --filter br/pkg/restore/log_client`：确认目录内 Rust/Go 对照、独立测试与相邻生产模块集合。
- RustCodeGraph `node --file br/pkg/restore/log_client/migration.rs --offset 1 --limit 500` 及 `--offset 490 --limit 180`：完整核对目标文件 635 行源码、44 个符号和文件级使用方。
- RustCodeGraph `query WithMigrationsBuilder --kind struct --json`、`query ValidateRetainLatestMVCCCompactionCoverage --kind function --json`、`query Metas --kind function --json`：核对 Rust/Go 同名符号及其文件位置。`explore`/`callers` 在本地索引上超时且未返回调用边，故未将其当作完成证据。
- 直接读取 `br/pkg/restore/log_client/Cargo.toml`、`lib.rs`、`migration.go`、`migration_test.rs`、`migration_test.go`、`export_test.rs`、`parity_test.rs`、`stubs.rs`，并读取 `client.rs`、`log_file_manager.rs` 的直接调用段；包目录不存在 `doc.go`。
- 精确符号检索核对 `BuildMigrations`、`ValidateRetainLatestMVCCCompactionCoverage`、`Metas`、`Physicals`、`Logicals`、`Compactions`、`IngestedSSTs` 和 `NeedSkip` 的生产/测试引用，确认本文件不是未接线门面。
- 未运行 Cargo 或代码测试：本任务只新增分析文档，任务计划明确禁止 Cargo。交付采用 Ready 文档范围，仅执行固定 11 章节结构验证、Markdown/差异检查和人工事实复核。
