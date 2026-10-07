# `pkg/executor/importer/precheck.rs`

## 文件定位

本文件属于 `astersql-executor-importer` crate，是 Rust `IMPORT INTO` 导入流程的前置条件判断层。模块由 `pkg/executor/importer/lib.rs` 声明并公开再导出，crate 根与依赖由 `pkg/executor/importer/Cargo.toml` 定义。它不负责查询数据库、访问 etcd 或创建对象存储客户端，而是通过 `ImportPrecheckService` 把这些外部动作抽象为边界接口，再对 `LoadDataController` 中的导入计划、目标表元数据和已统计文件大小执行有序校验。

当前接线状态必须分开看：`CheckImportTableTTL` 已由 `pkg/session/runtime/dispatch.rs` 的 `IMPORT INTO` 分派路径直接调用；`CheckImportDataSizeWithLimit` 已由 `pkg/dxf/importinto/scheduler.rs::prepareImportTask` 在 `InitDataFiles` 后调用。完整的 `CheckRequirements` / `CheckRequirementsBeforeInitDataFiles` 及 `ImportPrecheckService` 生产实现，在本次全仓搜索中未发现生产调用方或实现方，现有直接实现仅在 `pkg/executor/importer/precheck_test.rs::PrecheckBoundary`。因此，本文件包含完整 Rust 逻辑和测试，但不能据此声称完整 precheck 已全部接入 Rust 生产主链。

## 核心职责

- 定义 `ImportPrecheckService`，隔离活跃导入 job、目标表行探测、Starter 部署参数、PiTR/CDC 状态和 Global Sort 云存储权限等外部依赖。
- 用 `CheckImportTableTTL` 在任何外部检查或导入副作用前拒绝启用 TTL 的目标表；这一约束即使 `DisablePrecheck` 为真也不跳过。
- 为 `LoadDataController` 提供完整入口 `CheckRequirements` 和初始化文件前入口 `CheckRequirementsBeforeInitDataFiles`，共享同一检查顺序，但后者跳过尚未统计出的总文件大小并额外探测数据源可访问性。
- 对文件源检查同表活跃 job、非零文件体量和 Starter 最大真实体量；对所有数据源检查目标表为空，并按开关检查 CDC/PiTR 冲突。
- 对 Global Sort 先解析并限制云存储后端，再要求读取、列举、写入和删除对象的权限集合。
- 提供 `check_import_size_limit`、`display_bytes` 等纯函数以对齐 Go 的大小边界与错误文本；`is_supported_cloud_uri` 是当前仅由独立测试使用的轻量字符串判定辅助函数，不参与生产 URI 校验路径。

## 主要符号

- `pub trait ImportPrecheckService`：前置检查所需的可变外部服务边界。`ActiveJobCount`、`TableHasRows`、`PiTRTaskNames`、`RunningCDCChangefeedsMessage` 和 `CheckGlobalSortStorePrivileges` 可访问外部状态；`IsStarterDeployment` 与 `StarterMaxImportDataSize` 提供部署配置。接口统一返回 `Result<_, String>`，让控制器保持与具体 SQL、etcd、CDC 和对象存储实现解耦。
- `pub enum GlobalSortPermission`：权限意图的封闭集合，包括 `GetObject`、`ListObjects`、`PutAndDeleteObject`。`checkGlobalSortStorePrivilege` 固定按此三项一次性交给服务边界。
- `pub fn CheckImportTableTTL(&TableInfo) -> Result<(), SharedError>`：唯一返回 TiDB 类型化共享错误的独立入口。仅当 `TTLInfo` 存在且 `Enable` 为真时生成 `ErrLoadDataPreCheckFailed`。
- `LoadDataController::CheckRequirements`：完整检查入口，以 `check_total_file_size = true` 调用私有编排函数。
- `LoadDataController::CheckRequirementsBeforeInitDataFiles`：异步 prepare 的早期入口，以 `false` 执行公共检查；文件源通过后调用定义在 `import.rs` 的 `CheckDataSourceAccess`，只验证位置/存储可访问，不负责通配符匹配结果和空文件判断。
- `LoadDataController::checkRequirements`：检查顺序的核心实现。依次处理 TTL、文件源活跃 job、可选体量、空表、可选 CDC/PiTR、可选 Global Sort 权限，并在首个错误处停止。
- `CheckImportDataSize` / `CheckImportDataSizeWithLimit` / `checkStarterMaxImportDataSize`：分别从服务读取部署参数、接收显式部署参数、调用纯大小比较函数。显式参数入口使 DXF scheduler 不需要构造完整 precheck 服务。
- `checkTableEmpty`、`checkCDCPiTRTasks`、`checkGlobalSortStorePrivilege`：三个私有检查步骤，分别代理表行探测、先 PiTR 后 CDC 的冲突判断，以及 URI 解析后权限检查。
- `validate_global_sort_uri`：用 `ParseRawURL` 和 `ParseBackendFromURL` 解析并脱敏格式错误，只接受 `S3`、`Gcs`、`AzureBlobStorage` 后端。
- `check_import_size_limit`：只有同时满足 Starter 模式、上限非零、真实体量为正且超过上限时才失败。
- `is_supported_cloud_uri`：大小写不敏感地识别 `s3/gcs/gs/azure/azblob` 且要求非空 authority；它没有调用真实解析器，当前只用于测试简单 scheme/bucket 矩阵。
- `display_bytes`：以 TiB/GiB/MiB/KiB/B 格式化字节数，并尝试保留 Go `docker/go-units` 的四位有效数字表现。

## 执行流程

1. 完整入口调用 `CheckRequirements`；早期入口调用 `CheckRequirementsBeforeInitDataFiles`。两者首先进入 `checkRequirements`，区别仅在是否立即校验 `Plan.TotalFileSize`。
2. `checkRequirements` 优先从 `Plan.TableInfo` 取得表元数据，若计划未携带则退回 `self.Table.Meta()`，随后调用 `CheckImportTableTTL`。TTL 开启时立即返回，尚未调用任何 `ImportPrecheckService` 方法。
3. 若 `Plan.DataSourceType == DataSourceTypeFile`，先以数据库名和目标表名调用 `ActiveJobCount`；数量大于零即失败。完整入口随后调用 `CheckImportDataSize`，早期入口跳过此步。
4. `CheckImportDataSize` 从服务读取 Starter 标志和最大值；`CheckImportDataSizeWithLimit` 先拒绝 `TotalFileSize == 0`，再由 `check_import_size_limit` 比较 `TotalRealSize` 与最大值。非 Starter、上限为零、真实体量非正数或未超限均放行。
5. `checkTableEmpty` 调用服务的 `TableHasRows`；为真时拒绝导入。查询源虽然跳过 job 和文件体量检查，仍执行 TTL 与空表检查。
6. 若 `Plan.DisablePrecheck` 为假，`checkCDCPiTRTasks` 先读取 PiTR 任务名；存在任一任务即返回并不再探测 CDC。没有 PiTR 时才读取运行中 CDC changefeed 的可读错误消息。
7. 若 `Plan.IsGlobalSort()` 为真，先由 `validate_global_sort_uri` 完成解析、后端限制和错误脱敏，再把 URI 与三项固定权限交给 `CheckGlobalSortStorePrivileges`。
8. 早期入口在上述检查全部通过后，仅对文件源调用 `CheckDataSourceAccess`；查询源直接成功。测试证明本地明确缺失文件会失败，而通配符暂时无匹配仍可通过，因为发现文件和空文件判断留给后续异步 prepare。

生产接线还存在两条较窄路径：session SQL 分派在进入文件/查询具体执行前直接调用 `CheckImportTableTTL`；DXF `prepareImportTask` 在 `InitDataFiles` 已填充体量后调用 `CheckImportDataSizeWithLimit`，然后才计算资源和生成 chunks。

## 数据与状态

本文件不持有全局可变状态，也不缓存检查结果。主要只读输入来自 `LoadDataController`：`Plan.DBName`、`Plan.DataSourceType`、`Plan.TableInfo`、`Plan.DisablePrecheck`、`Plan.CloudStorageURI`、`Plan.TotalFileSize`，以及控制器的 `TotalRealSize` 和 `Table.Meta()`。检查结果只通过立即返回的 `Result` 表达。

文件体量存在两个不同含义：`TotalFileSize` 是匹配文件的原始总大小，零被视为未匹配或全空；`TotalRealSize` 是解压/展开后的估算真实体量，只在 Starter 限额比较中使用。真实体量小于等于零时不触发限额错误，这是与 Go 版一致的保护边界，不代表文件源有效；文件有效性仍由 `TotalFileSize != 0` 单独保证。

`ImportPrecheckService` 以 `&mut dyn` 传入，因为 SQL/etcd/对象存储实现可能维护会话或调用状态；大小配置方法只需 `&self`。独立测试的 `PrecheckBoundary.calls` 记录调用顺序，证明 TTL 失败前没有外部调用，并证明文件源的基本顺序为 job 后空表、查询源只有空表（测试设置 `DisablePrecheck = true`）。

## 依赖与调用关系

模块公开关系为 `lib.rs -> mod precheck -> pub use precheck::*`。目标源码直接使用同 crate 的 `DataSourceTypeFile` 与 `LoadDataController`；表元数据来自 `astersql-meta-model`，TTL 错误来自 `astersql-util-dbterror` 和 `astersql-util-dbterror-exeerrors`，数据源访问上下文来自 `astersql-objstore-storeapi`，URI 解析与后端枚举来自 `astersql-objstore`。这些均在 `pkg/executor/importer/Cargo.toml` 的 `[dependencies]` 中声明；独立测试所用 `astersql-session`、`astersql-testkit` 等位于 `[dev-dependencies]`。

RustCodeGraph 确认的内部调用链包括 `CheckRequirementsBeforeInitDataFiles -> checkRequirements -> CheckImportTableTTL`，以及 `checkRequirements` 到 `ActiveJobCount`、`CheckImportDataSize`、`checkTableEmpty`、`checkCDCPiTRTasks`、`checkGlobalSortStorePrivilege` 的分支边。各私有步骤再分别调用 trait 方法或 URI/大小辅助函数。

已确认生产上游有 `pkg/session/runtime/dispatch.rs` 对 `CheckImportTableTTL` 的直接调用，以及 `pkg/dxf/importinto/scheduler.rs::prepareImportTask` 对 `CheckImportDataSizeWithLimit` 的直接调用。图的文件级关系也显示 `precheck.rs` 被 `precheck_test.rs` 与 session dispatch 使用。全仓精确搜索没有找到生产 `impl ImportPrecheckService`，也没有找到生产代码对 `CheckRequirements` 或 `CheckRequirementsBeforeInitDataFiles` 的调用；所以完整编排目前是可测试 API，而非已证实的生产调用链。

## 错误处理与边界

`CheckImportTableTTL` 保留类型化 `SharedError`，错误码语义为 `ErrLoadDataPreCheckFailed`；控制器编排会把它字符串化，因此 `CheckRequirements` 的公开结果类型仍是 `Result<(), String>`。trait 方法错误均通过 `?` 原样传播，普通业务拒绝则构造稳定字符串。由于编排严格短路，最先失败的条件决定最终错误，也避免在已知不安全后继续发起外部访问。

TTL 是不可由 `DisablePrecheck` 绕过的硬边界；该开关只跳过 CDC/PiTR。注释与 Go 对照说明原因是异步 TTL job 可能与 import mode 切换竞态，造成保护表错误或校验和不一致。文件源的活跃 job 检查也不受 `DisablePrecheck` 影响；Global Sort 权限检查同样始终执行。

URI 解析错误通过 `ErrLoadDataInvalidURI.GenWithStackByArgs` 生成，并依赖对象存储解析器对凭证参数脱敏；独立测试验证 access key、secret key 和 session token 不会出现在错误中。不支持的、但能被解析的 backend 返回普通字符串 `unsupported cloud storage uri scheme`。真实权限检查失败的错误形态由 `ImportPrecheckService` 实现决定，本文件不再包装。

`is_supported_cloud_uri` 只做字符串级判断，不能代替 `validate_global_sort_uri`：它不解析查询参数、endpoint 或后端配置。当前生产路径不调用它，扩展时不应误用为安全校验。`display_bytes` 使用浮点数格式化极大字节值，现有测试覆盖常用边界，但没有证明所有 `u64` 值与 Go 格式逐位等价。

## 并发与资源生命周期

本文件自身不启动线程、异步任务或 channel，也不持锁；所有检查按调用线程串行、短路执行。它借用 `LoadDataController` 和服务，不取得服务资源所有权。trait 设计没有显式 `Close`，因此 SQL 结果集、etcd client、对象存储 probe 等资源的创建与释放必须由具体服务实现负责；当前仓库没有生产实现可供验证该生命周期。

TTL 检查特别处理的是本文件之外的并发风险：TTL job 异步运行，可能与进入 import mode 竞态，因此必须在导入副作用前拒绝。session dispatch 在选择 `IMPORT INTO ... SELECT` 或文件路径前再次调用该函数，形成较早的保护点；控制器完整入口中也把它放在一切外部检查之前。

`CheckRequirementsBeforeInitDataFiles` 的数据源访问只进行一次同步探测。它不会保留 storage handle，也不会枚举通配符结果；`CheckDataSourceAccess` 的打开/关闭细节属于 `import.rs`。DXF 大小检查发生在 `InitDataFiles` 完成之后，此时控制器已经持有统计结果，但本文件仍只读取数值，不管理数据存储生命周期。

## 与 Go 版本的对应关系

同路径 `pkg/executor/importer/precheck.go` 是主要语义基准。两版的完整顺序一致：TTL；文件源活跃 job 与可选文件大小；空表；未禁用时的 PiTR/CDC；Global Sort URI 与权限。两版早期入口都跳过总文件大小并在文件源上执行 `CheckDataSourceAccess`；两版都返回首个错误。

Rust 将 Go 对 session、SQL executor、etcd、部署全局配置和对象存储构造器的直接访问抽成 `ImportPrecheckService`。这提升了纯逻辑可测试性，但当前仓库只看到测试实现，尚未证实完整 Go 会话边界已经生产移植。Rust 另提供 `CheckImportDataSizeWithLimit`，供 DXF scheduler 显式传入 `deploymode::IsStarter()` 与全局上限；Go `CheckImportDataSize` 则在方法内部直接读取部署模式和全局配置。

Go 的 `checkTableEmpty` 自行构造 `SELECT 1 ... USE INDEX() LIMIT 1` 并关闭结果集，`checkCDCPiTRTasks` 自行创建和关闭 etcd client，`checkGlobalSortStorePrivilege` 自行创建带权限选项的对象存储。Rust 把这些副作用交给 trait；因此 Go 的资源清理证据不能自动证明未来 Rust 服务实现正确清理。

错误类型并非完全等价：Go 大多数业务拒绝直接返回 `ErrLoadDataPreCheckFailed`；Rust 只有独立 TTL 函数保留类型化错误，其余编排多为字符串。大小错误文本通过 `display_bytes` 对齐 Go `units.BytesSize`，Rust 独立测试覆盖 `0B`、`KiB`、`MiB` 与四位有效数字示例。Go 测试 `TestCheckRequirements` 还覆盖真实 SQL、etcd PiTR/CDC、Global Sort 后端与权限；对应 Rust 测试目前重点覆盖纯辅助函数、TTL 顺序及早期数据源访问，并未覆盖生产服务实现。

## 扩展指南

- 若要把完整 precheck 接入 Rust 生产路径，应先在合适的 session/runtime 边界实现 `ImportPrecheckService`，逐项复刻 Go 的 SQL、etcd 与对象存储资源管理，再在导入主链明确调用 `CheckRequirements` 或早期入口。必须在独立测试文件补充真实调用顺序、资源关闭和错误类型回归，不能只依赖现有测试 mock。
- 新增检查项时，修改 `checkRequirements` 的顺序前先确定它是否为不可跳过的安全约束、是否只适用于文件源、以及是否受 `DisablePrecheck` 控制。把无副作用且最便宜的检查放在外部访问之前，并在 `precheck_test.rs` 用调用记录证明短路顺序。
- 修改 TTL 规则时，应同步检查 `CheckImportTableTTL` 的两个现有位置：控制器编排与 `pkg/session/runtime/dispatch.rs`。回归必须覆盖文件/查询源、TTL 开启/关闭/不存在、`DisablePrecheck` 开启，以及失败时外部服务零调用。
- 扩展云存储后端时，应更新 `validate_global_sort_uri` 的 `StorageBackend` 匹配和 `GlobalSortPermission` 需求；只有测试辅助需要时才同步 `is_supported_cloud_uri`。要新增脱敏、缺 bucket、不支持 scheme 和权限拒绝测试，避免把轻量字符串判断当成真实解析。
- 修改 Starter 体量规则或单位格式时，应同时审查 `CheckImportDataSizeWithLimit`、`check_import_size_limit`、`display_bytes`、DXF scheduler 调用点和 Go `checkStarterMaxImportDataSize`。正确性风险是错误放行超限数据或把未知真实体量误判为超限，兼容风险是改变用户可见错误文本，性能影响主要来自错误地放行过大导入。
- 所有 Rust 回归继续放在同目录独立文件 `pkg/executor/importer/precheck_test.rs`，不要内嵌到生产源；涉及 Go 对齐时同步核对 `precheck_test.go` 的现有边界。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件且目标文件已索引；`files --filter pkg/executor/importer` 确认目标、Go 对照和独立测试；`node --file pkg/executor/importer/precheck.rs` 读取 274 行全貌；`query` 核对 `ImportPrecheckService`、`CheckImportTableTTL`、`validate_global_sort_uri`、`check_import_size_limit`、`display_bytes` 与 `is_supported_cloud_uri`；精确 `explore` 核对内部调用链和 trait 方法边。单独 `callers/callees` 命令未返回可用文本，因此生产调用点以图的文件级 `used by`、精确 `explore` 和相邻源码补证。
- Rust 源与接线：`pkg/executor/importer/precheck.rs`、`lib.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/dxf/importinto/scheduler.rs`；分别核对全部逻辑、模块再导出、SQL 分派 TTL 入口和 DXF 文件体量入口。全仓 `rg` 精确搜索用于确认没有其他 `ImportPrecheckService` 实现和完整入口调用。
- crate 声明：`pkg/executor/importer/Cargo.toml`，核对 crate 名、`lib.rs` 根、运行时依赖与测试依赖；目标目录没有 `doc.go`。
- Rust 测试：`pkg/executor/importer/precheck_test.rs` 共 450 行，覆盖 Starter 大小边界、Go 单位格式、支持/拒绝 URI、凭证脱敏、TTL 在两入口和两数据源上的优先级，以及早期文件访问不负责通配符匹配结果。
- Go 对照：`pkg/executor/importer/precheck.go` 与 `precheck_test.go`，核对完整检查顺序、SQL/etcd/对象存储副作用、TTL、空表、活跃 job、PiTR/CDC、Starter 上限和 Global Sort 行为。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构验证，并人工复核只新增本说明文件、没有把测试覆盖误写成生产接线、没有修改只读 `plan.md`。
