# `br/pkg/restore/misc.rs`

## 文件定位

本文件属于 `astersql-br-pkg-restore` library crate。crate 根 [`lib.rs`](./lib.rs) 以 `#[path = "misc.rs"] pub mod misc` 挂载本模块，并通过 `pub use misc::*` 将公开符号扁平再导出；[`Cargo.toml`](./Cargo.toml) 说明该 crate 对应 Go 包 `br/pkg/restore`，直接依赖 restore-utils、`base64`、`sha2`、`kvproto` 和 `grpcio`。本文件不是一个单一算法模块，而是与 Go [`misc.go`](./misc.go) 对齐的恢复辅助集合，横跨 PiTR 黑名单文件、集群元数据检查、PD 时间戳、Region 扫描以及重叠 SST 分组。

当前 Rust 生产主链中最清晰的直接入口是 [`restorer.rs`](./restorer.rs) 的 `BatchRestorer::GoRestore`：它把待恢复文件集展平后调用 `GroupOverlappedBackupFileSetsIter`，再把每个批次交给 worker 导入。黑名单和通用检查 API 已公开并由独立测试覆盖，但在仓库 Rust 生产文件中未找到 `MarshalLogRestoreTableIDsBlocklistFile`、`CheckTableTrackerContainsTableIDsFromBlocklistFiles`、`TruncateLogRestoreTableIDsBlocklistFiles`、`AssertUserDBsEmpty` 或本文件 `GetTSWithRetry` 的直接调用；因此这些符号是迁移后的可用库接口，不能仅依据 Go 调用链宣称 Rust 上层已经完整接线。`HasRestoreIDColumn` 的相邻 `log_client` 调用目前经过该 crate 自己的 `stubs::restore_misc`，并非直接链接本模块实现。

## 核心职责

1. 定义 `LogRestoreTableIDsBlocklistFile`，生成固定命名的 PiTR 表/库 ID 黑名单文件，以 SHA-256 校验内容，并手工实现与 Go gogo protobuf 字段编号兼容的编码和解码。
2. 通过 `fastWalkLogRestoreTableIDsBlocklistFile` 并发读取对象存储中的黑名单文件；在此之上实现时间窗冲突检查和按提交 TS 截断删除。
3. 提供小型恢复工具：布尔值到 `ON`/`OFF`、InfoSchema 表查询、fresh-cluster 用户库检查、PD TSO 获取和重试、`restore_id` 列能力探测。
4. 维护一个可刷新前缀的 Region 缓存，并判断给定键区间是否落在同一 Region。
5. 对 rewrite 后键范围排序，合并重叠 `BackupFileSet`，验证其 Table ID 与 rewrite rules 一致，并在 Region 边界上切分可导入批次。

本文件不负责写入黑名单对象、执行 SST 导入、创建真实 PD/InfoSchema/Storage 客户端或管理外部连接；这些资源均由调用者注入。`FineGrained`/`CoarseGrained` 只是兼容字符串标记，本文件不解释或执行拆分策略。

## 主要符号

- `Granularity = String`、`FineGrained`、`CoarseGrained`：与 Go 已弃用粒度参数对齐的字符串接口。
- `logRestoreTableIDBlocklistFilePrefix` 与公开别名 `LogRestoreTableIDBlocklistFilePrefix`：黑名单对象目录 `v1/log_restore_tables_blocklists`；公开别名主要供跨 crate/测试使用。
- `LogRestoreTableIDsBlocklistFile`：包含 `RestoreCommitTs`、`RestoreStartTs`、`RewriteTs`、`TableIds`、`DbIds`、`Checksum`。`filename` 生成 `R{commit:016X}_S{start:016X}.meta`；checksum 按三个 TS、表 ID、库 ID 的顺序，以小端 64 位字节计算 SHA-256。
- `parseLogRestoreTableIDsBlocklistFileName` 及公开包装 `ParseLogRestoreTableIDsBlocklistFileName`：只解析 basename，验证 `.meta`、`R`、固定宽度十六进制段和 `_S` 分隔符，失败统一返回 `(0, 0, false)`。
- `MarshalLogRestoreTableIDsBlocklistFile` / `UnmarshalLogRestoreTableIDsBlocklistFile`：黑名单构造、checksum 写入、protobuf 编解码及完整性检查入口。
- `fastWalkLogRestoreTableIDsBlocklistFile`：私有并发遍历骨架；接受文件名/内容双阶段过滤闭包和业务执行闭包。
- `CheckTableTrackerContainsTableIDsFromBlocklistFiles`：将有效时间窗内的表、分区和库 ID 与 `PiTRIdTracker` 比较；冲突时报带 BackupTS/RestoreTS 建议的错误，非冲突项可触发 lost 警告和 `clean_error(rewrite_ts)`。
- `TruncateLogRestoreTableIDsBlocklistFiles`：删除 `RestoreCommitTs <= until_ts` 的对象。
- `UniqueTableName`、`TransferBoolToValue`、`GetTableSchema`、`AssertUserDBsEmpty`、`GetTS`、`GetTSWithRetry`、`HasRestoreIDColumn`：恢复编排所需的小型数据类型、元数据和时间戳辅助函数。
- `regionScanner` / `ArcSplitClient` / `NewRegionScanner`：拥有 `Arc<dyn SplitClient>`、Region 缓存和扫描上限；`locateRegionFromRemote`、`locateRegionFromCache`、`IsKeyRangeInOneRegion` 组成定位 API。`regionScanner` 类型本身未公开，但构造函数和方法允许调用者通过推断类型使用。
- `BackupFileSetWithKeyRange`：持有文件集及 rewrite 后的最小/最大键，是排序和合并的中间状态。
- `GroupOverlappedBackupFileSetsIter` 与 `getKeyRangeForBackupFileSet`：生产主链使用的分组入口及键范围计算器。
- `encode_*`、`decode_*`、`proto_marshal`、`proto_unmarshal`：仅服务黑名单消息的私有 protobuf codec；字段 2 故意空缺，1/3/4/5/6/7 必须保持 Go struct tag 兼容。

## 执行流程

黑名单写入与读取流程：

1. `MarshalLogRestoreTableIDsBlocklistFile` 填充消息，按固定字段顺序计算 checksum，生成包含 commit/start TS 的路径，再调用 `proto_marshal`。
2. codec 对 proto3 零值字段不编码；表/库 ID 使用 packed repeated int64，checksum 使用 length-delimited 字段。
3. `unmarshalLogRestoreTableIDsBlocklistFile` 先解析 wire 格式，再从除 `Checksum` 外的字段重算摘要；不一致则以 base64 展示计算值和记录值并报损坏错误。
4. `fastWalkLogRestoreTableIDsBlocklistFile` 先 `WalkDir` 收集路径。文件名能解析且被 TS 谓词过滤时不读取；不能解析的路径仍会进入读取并最终由内容解码决定成败。
5. Rust 在调用线程依次 `ReadFile`，随后把拥有所有权的 `(filename, data)` 投递给大小为 8 的 worker pool。worker 再按内容中的 TS 过滤并执行回调；首个 `ErrorGroup` 错误出现后停止投递新任务，最后由 `Wait` 汇总错误。

冲突检查使用条件 `start_ts >= restore_commit_ts || restored_ts < restore_start_ts` 跳过无关文件。对剩余文件，表 ID 同时检查 `ContainsTableId` 与 `ContainsPartitionId`，库 ID 检查 `ContainsDB`；lost 回调只产生警告，不阻止恢复。只有文件内全部 ID 检查通过后才调用 `clean_error(rewrite_ts)`。截断流程复用遍历器收集路径，再在 worker 阶段结束后串行 `DeleteFile`。

元数据与 TS 流程：`GetTableSchema` 委托 `Domain::InfoSchema().TableByName`；`AssertUserDBsEmpty` 遍历所有 schema，跳过系统/内存库和空 `test` 库，经 `MetaReader::ListSimpleTables` 收集最多十个名称，超限追加 `...` 并返回 `ErrRestoreNotFreshCluster`。`GetTS` 调用 PD 后以 `ComposeTS(physical, logical)` 合成 TSO；`GetTSWithRetry` 交给 `WithRetryAggressive` 重试，成功返回最终 TS，失败优先传播最后一次 `GetTS` 错误。`HasRestoreIDColumn` 在目标表不存在或查询失败时返回 `false`，否则大小写规范化后查找 `restore_id`。

SST 分组流程：

1. `getKeyRangeForBackupFileSet` 对每个 SST 调用 `GetRewriteRawKeys`，取全组最小 start 和最大 end；空端点使用默认空字节串。
2. `GroupOverlappedBackupFileSetsIter` 按 `(startKey, endKey)` 排序并创建缓存上限 64 的 scanner。
3. 若 `last_end_key < next.startKey`，当前窗口结束：先把旧集合放入 batch，再以新文件集开启窗口；若两窗口间隙跨 Region，则先回调输出已积累 batch。
4. 否则视为重叠，追加 SST；Table ID 或 `RewriteRules::Equal` 不一致立即报错，防止把不同重写语义静默合并。
5. 循环结束后加入最后一个集合并 flush 非空 batch。输入为空时不扫描 Region，也不调用回调。

## 数据与状态

黑名单文件的兼容性由三个互相绑定的不变量构成：路径中的 commit/start TS、消息中的同名字段以及 checksum 覆盖的字段值。checksum 输入顺序和小端编码不能调整；`TableIds`/`DbIds` 的顺序也参与摘要。codec 允许同一 repeated 字段拆成多个 packed 段，也接受非 packed 单值编码；这与 Go/gogo 追加元素的行为一致。

`AssertUserDBsEmpty` 的 `user_tables` 只用于错误摘要，容量为 11：十个真实条目加 `...`。空的非 `test` 用户库以 `db.` 形式计入；系统库不计入。`GetTSWithRetry` 的 `start_ts` 和 `get_ts_err` 是调用内状态，不跨调用缓存。

`regionScanner.region_cache` 必须按 Region `StartKey` 有序。命中缓存第 `i` 项后会 drain `0..i`，使 `cache[0]` 始终是当前命中 Region；key 到达或越过非空的末 Region `EndKey` 时整批远程刷新。空 `EndKey` 表示无上界。`IsKeyRangeInOneRegion` 使用严格的 `end_key < region.EndKey`，因此恰好等于 Region 终点的范围被视为跨 Region。

`GroupOverlappedBackupFileSetsIter` 在当前调用中拥有输入 `Vec`、排序中间结构和 batch；输出通过 `FnMut` 回调转移。合并只克隆 SST 列表与 rewrite rules，不修改原始客户端或外部状态。`last_end_key` 记录当前合并窗口最大上界，而不是最近单个文件的上界。

## 依赖与调用关系

上游直接证据：

- [`restorer.rs`](./restorer.rs) 的 `BatchRestorer::GoRestore -> GroupOverlappedBackupFileSetsIter`；回调中把 batch 提交到 worker pool，执行 `FileImporter::Import`、可选 checkpoint 记录和进度累计。这是本文件在 Rust 恢复生产链中的明确接线。
- [`misc_test.rs`](./misc_test.rs) 和 [`parity_test.rs`](./parity_test.rs) 直接调用黑名单、空库、TS、Region 与分组接口，验证 Rust/Go 行为对齐。
- [`export_test.rs`](./export_test.rs) 再导出文件名前缀、解析和 unmarshal 等测试接口。
- 仓库 Rust 搜索未发现其余公开函数的非测试生产调用；[`log_client/id_map.rs`](./log_client/id_map.rs) 虽调用同名 `restore_misc::HasRestoreIDColumn`，其 import 指向 [`log_client/stubs.rs`](./log_client/stubs.rs) 的局部模块，应视为相邻迁移桩而非本模块调用边。

下游直接依赖：

- `crate::stubs` 提供 `Storage`、`Context`、`ErrorGroup`、`NewWorkerPool`、`PiTRIdTracker`、`Domain`、`PdClient`、`SplitClient`、`RegionInfo`、重试策略和错误类型。这些是移植期本地抽象，不能等同于已接入所有 Go 真实实现。
- [`restorer.rs`](./restorer.rs) 提供 `BackupFileSet` 与 `BatchBackupFileSet`；restore-utils 提供 `GetRewriteRawKeys` 和 `RewriteRules`。
- `base64` 只用于 checksum 错误展示，`sha2` 用于完整性摘要；它们不提供备份数据加密。
- `kvproto`/`grpcio` 是 crate 依赖，但本文件通过 restore-utils 的 `backuppb` 和本地 traits 间接使用相关类型。

## 错误处理与边界

- 文件名解析对短名、错误扩展名、错误前缀/分隔符和非十六进制段返回 `false`，不会像 Go 固定切片那样因短字符串 panic；但长度大于等于 35 且前 35 字节合法时不会要求 `.meta` 紧邻第二个 TS 的第 35 字节，解析规则应与测试共同维护。
- `proto_unmarshal` 对截断 varint、varint 溢出、截断已知 packed 字段、截断 checksum 和不支持的 wire type 返回错误。对未知 wire 0/1/2/5 会跳过；当前 fixed32/fixed64/length 跳过路径直接增加索引，缺少统一的“跳过长度不得超过输入”校验，因而可能静默接受截断的未知字段，扩展 codec 时应先补防御性验证。
- checksum 缺失等价于空值，通常会与重算摘要不符并报错；错误不会返回部分消息。
- `fastWalkLogRestoreTableIDsBlocklistFile` 在启动 worker 前先完成所有文件读取，因此任一读取失败会阻止执行阶段开始；worker 已产生错误后只保证停止投递后续任务，已经投递的任务仍可能运行。
- 截断条件包含等号：提交 TS 恰等于 `until_ts` 的文件会被删除。删除按已收集顺序串行进行，遇错立即返回，可能留下“部分已删除”状态，不具备事务原子性。
- `AssertUserDBsEmpty` 的表枚举错误带 database ID 上下文；任何用户库/表导致分类错误 `ErrRestoreNotFreshCluster`。`HasRestoreIDColumn` 刻意吞掉表查询错误并降级为 `false`。
- `locateRegionFromRemote` 对空扫描结果返回 `no region found`，对缺失 `Region` 的缓存项会回退远端或在最终使用时报 `region missing`。
- 重叠 SST 的 Table ID/rewrite rules 不一致时，函数可能已在内存中追加 SST，但在任何该窗口 batch 回调前返回错误；调用者不会收到这个无效合并结果。

## 并发与资源生命周期

黑名单遍历是本文件唯一主动创建并发工作的路径。它使用固定 8 worker 的 `NewWorkerPool` 和 `ErrorGroup::with_context`；过滤器、执行器通过 `Arc` 共享并要求 `Send + Sync + 'static`。每个 job 拥有文件名和字节缓冲，因此不会借用 `Storage`；`Storage` 的 walk/read/delete 生命周期仍由调用者保证。本文件不关闭 storage，也不创建长期后台任务。

`TruncateLogRestoreTableIDsBlocklistFiles` 用 `Arc<Mutex<Vec<String>>>` 收集 worker 结果，等待全部任务后 `take` 出路径并串行删除。这是 Rust 为避免把 `&dyn Storage` 捕获进 `'static` worker 的两阶段实现，和 Go 在 worker 中直接删除存在时序差异。

`regionScanner` 需要 `&mut self` 才能定位和更新缓存，因此单个实例不会在无外部同步时被并发使用；底层 `SplitClient` 由 `Arc` 共享。`GroupOverlappedBackupFileSetsIter` 自身同步执行，但其回调可以像 `BatchRestorer::GoRestore` 那样把拥有所有权的 batch 交给其他 worker。回调不能返回错误，所以回调内部的异步失败由上层自己的 `ErrorGroup` 汇总，而非由分组函数直接传播。

## 与 Go 版本的对应关系

同路径 [`misc.go`](./misc.go) 是语义基准。Rust 已对齐：粒度常量；黑名单字段与 protobuf 编号；文件名格式；小端 checksum；时间窗过滤；表/分区/库冲突错误；lost 只告警；commit TS 截断条件；fresh-cluster 的系统库与空 `test` 例外；PD TSO 合成与 aggressive retry；`restore_id` 探测；Region 缓存刷新；rewrite 后键范围排序、重叠合并和 Region 批切分。

当前可见差异与迁移边界：

- Go 使用 gogo `proto.Marshal/Unmarshal`，Rust 使用本文件私有 codec。Rust 兼容 packed/non-packed repeated 和常见未知 wire 类型，但不是通用 protobuf 实现；字段演进必须用跨语言样本验证。
- Go 的 `fastWalk` 在 worker 内读取 storage；Rust 先串行读完全部对象再并发解码。结果语义接近，但 Rust 峰值内存与首批处理延迟更高，且读取失败发生在任何业务回调之前。
- Go 截断在 worker 内并发删除；Rust 等遍历/解码完成后串行删除。Rust 避免借用生命周期问题，但吞吐与部分失败时序不同。
- Go `GetTSWithRetry` 包含 `get-ts-error` failpoint，并通过通用 `WithRetry` 加 aggressive 策略；Rust 调用 `WithRetryAggressive`，未复刻该 failpoint。Rust 代码中的 `retry` 只递增且不参与分支。
- Go Region 缓存用二分搜索；Rust用线性 `position`。默认缓存只有 64 项，正确性相同，但复杂度不同。
- Go `locateRegionFromRemote` 直接索引第一个结果；Rust 对空结果返回明确错误。Rust 还允许 `RewriteRules` 两边同时为 `None` 时相等，而 Go 路径通常假定规则对象可调用 `Equal`。
- Rust codec 和局部 stubs 证明的是当前 crate 的移植接口与测试行为，不证明真实对象存储、PD、Domain 和所有 PiTR 上层流程已完整接入。

独立 Rust 测试位于 [`misc_test.rs`](./misc_test.rs)，Go 对照测试位于 [`misc_test.go`](./misc_test.go)；生产逻辑与测试逻辑没有混放。

## 扩展指南

- 修改黑名单字段或 protobuf 编号时，必须同步 [`misc.go`](./misc.go) 的 struct tag/生成协议、`checksumLogRestoreTableIDsBlocklistFile` 的字段顺序和 [`misc_test.rs`](./misc_test.rs) 的跨段 packed、空数组、损坏 checksum、未知字段测试。旧文件可读性比新增字段本身更重要。
- 扩展文件名格式时，集中修改 `filename` 与 `parseLogRestoreTableIDsBlocklistFileName`，同时覆盖短名、额外后缀、大小写、错误分隔符、最大 `u64` 和带目录路径；不要让 parser 重新引入字符串切片 panic。
- 调整黑名单时间窗时，同步检查 `CheckTableTrackerContainsTableIDsFromBlocklistFiles` 和 `TruncateLogRestoreTableIDsBlocklistFiles` 的相等边界。前者的 `>=`/`<` 与后者的 `<=` 承担不同语义，不应机械统一。
- 若把读取恢复成真正流式并发，需保持 worker 拥有缓冲、首错取消、过滤两阶段一致，并评估 storage trait 的线程安全；新增独立测试覆盖读取失败、解码失败、回调失败、取消和大量文件的内存上限。
- 修改 fresh-cluster 判定时应在 `AssertUserDBsEmpty` 集中处理，并同步测试系统库、空 `test`、空普通库、超过十张表和 `ListSimpleTables` 错误；不要把测试逻辑放回源文件。
- 修改 SST 分组时，保持 rewrite 后键排序、半开区间 Region 边界、同 Table ID/规则约束与稳定回调顺序。新增场景应写入 [`misc_test.rs`](./misc_test.rs)，至少覆盖空输入、嵌套范围、相等端点、跨 Region 间隙、不同规则错误和 scanner 空返回。
- 若性能分析表明线性 Region 查找或全量黑名单预读成为瓶颈，可分别恢复二分查找和有界流水线；优化必须保留错误传播及确定性排序，不能以减少校验为代价。
- 新增生产调用时应优先复用本 crate 扁平导出，并明确替换相邻 crate 的同名 stubs；否则同名函数会让代码搜索产生“已接线”的假象。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/restore` 显示目标 `misc.rs` 有 58 个符号，并列出 Rust/Go 源与独立测试。
- RustCodeGraph `node --file br/pkg/restore/misc.rs --offset 1 --limit 500` 与 `--offset 500 --limit 500`：完整读取目标 1–934 行；索引报告该文件被 15 个文件使用。`query` 分别定位了 Rust/Go 同名 `MarshalLogRestoreTableIDsBlocklistFile`、`CheckTableTrackerContainsTableIDsFromBlocklistFiles`、`TruncateLogRestoreTableIDsBlocklistFiles`、`AssertUserDBsEmpty`、`GetTSWithRetry`、`HasRestoreIDColumn` 和 `GroupOverlappedBackupFileSetsIter`；`callees` 进一步确认 marshal 到 `filename`/checksum/`proto_marshal`，冲突检查到 `fastWalk` 的边。
- RustCodeGraph 的部分 `callers` 查询没有输出，故按技能回退到 `rg` 核验直接使用：确认 [`restorer.rs`](./restorer.rs) 的生产调用、[`misc_test.rs`](./misc_test.rs) 与 [`parity_test.rs`](./parity_test.rs) 的测试调用，以及 [`log_client/id_map.rs`](./log_client/id_map.rs) 实际指向局部 stub 的同名调用。
- 已核对 crate 边界文件 [`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)，确认 library 身份、模块挂载、公开再导出、依赖以及独立测试文件。
- 已核对 Go 对照 [`misc.go`](./misc.go) 的黑名单、元数据/TS、Region scanner 和 SST 分组实现，以及 Go 测试 [`misc_test.go`](./misc_test.go) 的时间窗、截断、PD 重试和 Region 场景。
- 已核对 Rust 独立测试 [`misc_test.rs`](./misc_test.rs)：覆盖布尔转换、表查询、fresh-cluster、PD 健康/失败/切主、文件名、protobuf round-trip 与拆分 packed 字段、冲突时间窗、截断、scanner/分组、边界过滤、空数组和非法文件名；[`parity_test.rs`](./parity_test.rs) 提供补充 parity 覆盖。
- 本任务仅生成文档，按任务要求未运行 Cargo。交付前使用任务指定的 `test -f ... && test "$(rg -c ...)" -eq 11` 做结构验证，并人工复核本文未把 Go 调用链或本地 stubs 描述成 Rust 已完成生产接线。
