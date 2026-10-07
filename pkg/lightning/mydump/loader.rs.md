# `pkg/lightning/mydump/loader.rs`

## 文件定位

`loader.rs` 是 `astersql-lightning-mydump` 子 crate 的目录清单装载层。`pkg/lightning/mydump/lib.rs` 以 `mod loader; pub use loader::*;` 将本文件 API 暴露给嵌入方；`pkg/lightning/mydump/Cargo.toml` 则把该目录定义为独立 crate，并声明它从 Go 包 `pkg/lightning/mydump` 移植。它位于“外部存储中的文件清单”与后续 schema/region/import 处理之间：把路径经 `FileRouter` 解释为库、表、视图和数据分片，形成 `MDLoader` 元数据树。

直接生产入口可见于 `pkg/importsdk/file_scanner.rs:389`：SDK 组装 `LoaderConfig`、`Storage` 和扫描选项后调用 `NewLoaderWithStore`；同文件随后读取 `GetDatabases`、`GetStore`，并在需要时再次调用 `EstimateRealSizeForFile`。`pkg/lightning/mydump/schema_import.rs` 消费这些库表元数据来导入 schema。本文件不是数据行解析器，也不执行建库建表或数据写入。

## 核心职责

- `NewLoaderWithStore`（第 238 行）枚举并稳定排序源文件，应用扫描上限、文件路由和表过滤，计算数据文件估算大小，并组装 `MDDatabaseMeta`/`MDTableMeta`。
- `MDDatabaseMeta::GetSchema` 与 `MDTableMeta::GetSchema`（第 48、85 行）通过 `ExportStatement` 从 `Storage` 读取 schema SQL；数据库缺失/读取失败时生成带转义库名的兜底建库语句，表缺失 schema 则返回错误。
- `newAuroraFileRouter`（第 447 行）只在显式启用时识别 Aurora 原生 Parquet 目录并生成优先路由，同时拒绝不完整、混合或歧义清单。
- `should_skip_rules`/`glob_match`（第 544、573 行）实现大小写不敏感的简化 glob 包含/排除规则。
- `EstimateRealSizeForFile`/`SampleFileCompressRatio`（第 642、651 行）以压缩文件前缀采样估算解压后大小，失败时保守回退到原文件大小。
- `ParallelProcess`（第 595 行）提供保持输入顺序的固定并发 map；它是公开兼容辅助函数，但当前 `NewLoaderWithStore` 的扫描循环本身是串行的。

## 主要符号

- `MDDatabaseMeta`：一个数据库的名称、建库文件、表、视图和字符集。`NewMDDatabaseMeta` 只初始化字符集。
- `MDTableMeta`：表或视图的 schema 文件、按路由键排序的数据文件、估算总大小、索引比率和行序标志。`NewMDTableMeta` 将 `is_row_ordered` 置为 `true`。
- `SourceFileMeta`：供大小估算使用的路径、类型、压缩方式、排序键、逻辑/物理大小、扩展列及行数载体。
- `LoaderConfig`：字符集、`FileRouteRule` 列表、过滤表达式以及路由是否仍为内置默认规则。默认字符集为 `utf8mb4`，默认路由来自 `default_file_route_rules()`。
- `MDLoaderSetupConfig` 与 `MDLoaderSetupOption`：构造阶段的可变选项；`WithMaxScanFiles`、`WithSkipRealSizeEstimation`、`WithAuroraAutoMapping`、`WithFileIterator` 等返回一次性配置闭包。
- `FileIterator`、`RawFile`、`AllFileIterator`：把文件枚举抽象为 `(path, size)` 回调，使 SDK 或测试可以绕过 `Storage::list` 注入清单。
- `MDLoader`：最终持有 `databases`、共享 `store`、表 schema/数据文件的路径索引、过滤规则和 Aurora 检测标志；通过 getter 暴露只读视图或 `Arc` 克隆。
- `insert_meta`、`insertDB`、`insertTable`、`insertView`：内部/兼容装配函数。真正主流程使用 `insert_meta`；后三者是公开辅助入口。
- `constructFileInfo`、`setup`、`route` 及自由函数形式的 getter/`shouldSkip`/`IterateFiles`：兼容或薄封装 API，不应误认为存在第二套装载流程。

## 执行流程

1. `NewLoader` 原样委托 `NewLoaderWithStore`。后者从默认 `MDLoaderSetupConfig` 开始，按传入顺序执行选项闭包。
2. 若启用 Aurora 自动映射却配置了非默认路由，立即返回 `MydumpError::Configuration`；Aurora 模式还强制关闭部分结果标志。
3. 构造普通 `ChainRouters`。优先使用注入的 `FileIterator`，否则调用 `Storage::list`；收集结果后按路径字典序排序，再按 `max_scan_files` 截断。Aurora 模式禁止截断完整清单。
4. `newAuroraFileRouter` 在过滤之前检查完整原始清单。若识别到形如 `<root>/<schema>/<schema>.<table>/.../part-*.parquet` 的文件，就校验单一根目录、目录前缀一致、标识符无歧义、无 glob 元字符且不存在混合的普通数据文件，然后生成转义过的正则路由并置 `aurora_source=true`。生成的 Aurora 规则排在普通规则之前。
5. 对每个文件调用 `FileRouter::Route`。未命中、`SourceType::Ignore` 或被 `should_skip_rules` 排除的文件不进入结果。
6. 构造 `FileInfo`：未压缩或显式跳过估算时 `real_size=file_size`；其余文件经 `EstimateRealSizeForFile` 采样。随后以 `BTreeMap` 按库名聚合：库/表/视图 schema 检查重复，SQL/CSV/Parquet 分片累加 `total_size` 并按 `sort_key` 排序。
7. `pruneViewPlaceholders` 移除与视图同名的表占位；每个库的表按 `total_size` 升序排列，使较小表先出现。
8. 遍历最终表集合建立 `all_files`。该索引包含表 schema 和表数据文件，不包含数据库 schema 或视图 schema。最后保存过滤规则、存储句柄与 Aurora 标志并返回 `MDLoader`。

## 数据与状态

装载期间的核心可变状态是局部 `BTreeMap<String, MDDatabaseMeta>`。选择 `BTreeMap` 使数据库输出按名称确定性排列；文件先按路径排序，数据分片再按 `sort_key` 排序，表最终按 `total_size` 排序，因此相同不可变输入可得到稳定结果。`insert_meta` 以 `(database, table name)` 聚合多个分片，并用 `real_size` 而非压缩文件大小累计 `total_size`。

`MDLoader` 构造后没有内部可变性：元数据和 `all_files` 直接拥有数据，`Storage` 以 `Arc<dyn Storage>` 共享。`GetDatabases`/`GetAllFiles` 只借用内部集合；`GetStore` 克隆 `Arc`。过滤规则被保留，供后续 `shouldSkip` 查询复用。`SourceFileMeta.extend_data`、`rows`、`index_ratio` 等字段在本文件主流程中不更新，属于与其他导入阶段共享的数据模型。

需要特别注意当前接线：`MDLoaderSetupConfig.scan_file_concurrency` 只由选项和 `setup()` 校验使用，`NewLoaderWithStore` 未调用 `ParallelProcess`；`support_partial_result` 会被选项设置，但枚举、路由或装配错误仍通过 `?` 直接返回，当前 Rust 主流程不会携带部分 `MDLoader`。这两点不能按 Go 版本能力宣称已实现。

## 依赖与调用关系

上游关系：

- `pkg/lightning/mydump/lib.rs` 公开重导出本模块，并通过 `#[path = "loader_test.rs"]` 挂载独立测试。
- `pkg/importsdk/file_scanner.rs:375-389` 把 SDK 的最大扫描数、Aurora 开关等转换为 loader 选项并调用 `NewLoaderWithStore`；`:670` 复用大小估算。
- `pkg/lightning/mydump/schema_import.rs` 读取 loader 元数据；其独立测试也直接构造 loader。

下游关系（RustCodeGraph 的 `callees` 与源码一致）：

- 路由：`NewFileRouter`、`ChainRouters::Route`、`FileRouteRule`、`RouteResult`，实现在 `pkg/lightning/mydump/router.rs`。
- 存储与文件模型：`Storage::list/open`、`FileMeta`、`FileInfo`、`ExportStatement`，类型主要位于 `common.rs`。
- 并发辅助：标准库 `thread::scope`、`Arc<Mutex<VecDeque<_>>>`。
- 外部 crate：`regex` 用于 Aurora 识别和规则安全转义；Cargo 还声明 `encoding_rs`、`hex`、`libm`、`percent-encoding`、`thiserror`，但它们不是本文件的直接 import。

## 错误处理与边界

`NewLoaderWithStore` 返回 `Result<MDLoader, MydumpError>`。配置不兼容产生 `Configuration`；Aurora 清单截断或存储枚举问题产生/传播 `Io`；路径歧义、混合源、多个根或 glob 元字符产生 `Routing`；重复库、表、视图 schema 产生 `Schema`。路由构造、路由执行、迭代器及存储列表错误均向上传播。

数据库 `GetSchema` 有意吞掉读取/解码失败并生成 `CREATE DATABASE IF NOT EXISTS`，同时用双反引号转义库名；表 `GetSchema` 更严格，缺文件或读取失败都会返回错误。两者使用 `String::from_utf8_lossy`，非法 UTF-8 被替换而非报错。

压缩估算是非关键优化：`SampleFileCompressRatio` 的打开/读取错误对直接调用者可见，但 `EstimateRealSizeForFile` 将任何采样错误回退为 `file_size`；比率下限为 `1.0`，分母通过 `max(1)` 防止零大小除零。过滤器为空表示不过滤；非空时默认排除，匹配规则按出现顺序更新状态，`!` 表示排除，后匹配者覆盖先前结果。

## 并发与资源生命周期

主装载流程当前同步执行。文件枚举借用回调，返回后原始清单由局部 `Vec` 拥有；读取 schema 或采样时，`Storage::open` 返回的 reader 随函数退出释放。`MDLoader` 通过 `Arc` 延长底层存储寿命，克隆 getter 不复制存储实例。

`ParallelProcess` 为每次调用创建共享队列、输出和错误槽，线程数量至少为 1。工作线程在取任务前观察错误；某个失败写入共享错误后，其余线程在下一次循环停止。成功结果携带输入下标，join 后排序以恢复输入顺序。已经开始的闭包不会被强制取消，因此错误返回前仍会等待 scoped threads 结束；若多个任务近同时失败，最后取得锁的写入可能覆盖先前错误。闭包或互斥锁中毒导致的 panic 会通过 `unwrap`/scope 传播，而不是转换为 `MydumpError`。

## 与 Go 版本的对应关系

Go 对照为 `pkg/lightning/mydump/loader.go`。Rust 保留了 Go 的主要名词和入口：`MDLoader`、配置 option、`NewLoaderWithStore`、`ParallelProcess`、文件路由、占位视图清理、表/分片稳定排序以及压缩大小估算。`pkg/lightning/mydump/Cargo.toml` 的 `package.metadata.porting.go-package` 明确记录了这一来源。

当前并非逐能力等价：

- Go `mdLoaderSetup.setup` 以 `ParallelProcess` 按 `ScanFileConcurrency` 并发构造文件信息，并可在配置允许时保留部分扫描结果；Rust 主流程串行，相关配置字段尚未接线。
- Go `ParallelProcess` 接收 `context.Context`，用 `errgroup` 传播取消；Rust 版无取消上下文，只在共享错误出现后停止领取新任务。
- Go 对 Parquet 按表采样行大小/压缩率并估算行数；Rust 统一走 `Storage::open(path, compression)` 的前缀采样，不维护按表 Parquet 统计。
- Go 压缩比采样用两次读取寻找有效压缩边界；Rust 依赖 `Storage::open` 的解压 reader，读取最多 4096 个解压字节，再除以 `min(file_size, 4096)`。两者目标相同但算法并不完全相同。
- Go 支持更完整的表路由、日志、指标、上下文错误包装和 failpoint；Rust 本文件没有这些接线。Rust 的 Aurora 检查和基本重复/排序语义由独立测试覆盖，不能据此推断上述 Go 能力均已移植。

## 扩展指南

- 新增文件类型或更改聚合语义时，修改 `NewLoaderWithStore` 的 `SourceType` 分派和 `insert_meta`，并同步检查 `router.rs` 的类型解析、`all_files` 收录范围与 `total_size` 含义。
- 接通并发扫描时，应让 `scan_file_concurrency` 真正驱动文件信息构造，同时保持当前“路径排序后确定性输出”、首错语义和 `Storage` 的线程安全契约；测试仍放在独立的 `loader_test.rs`，不要嵌入生产文件。
- 实现部分结果时必须先定义 Rust API 如何同时返回 loader 与错误；现有 `Result<MDLoader, MydumpError>` 无法表达 Go 的“结果加错误”，不能只读取 `support_partial_result` 后吞错。
- 扩展过滤器时要确认是否仍兼容当前大小写不敏感的 `*`/`?`、顺序覆盖和 `!` 排除语义；若需完整 TiDB table-filter 语法，应替换或封装 `should_skip_rules`，而不是悄悄改变 glob。
- 修改 Aurora 目录识别时必须保留“过滤前验证完整清单”的安全属性，并更新 `aurora_auto_mapping_*` 测试覆盖根目录、特殊字符、混合源和扫描上限。
- 修改压缩估算时同步验证无压缩、读取失败、零大小和不同压缩格式；性能风险主要是构造 loader 时对每个压缩文件执行一次 `open`。
- 修改 schema 获取时分别保留数据库宽松兜底与表严格报错的契约，关注标识符转义、字符集转换和 lossy UTF-8 行为。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/lightning/mydump/loader.rs`（62 个符号）；`node --file ... --offset 1 --limit 1000` 阅读了完整 777 行；对 `NewLoaderWithStore`、`ParallelProcess`、`SampleFileCompressRatio`、`newAuroraFileRouter`、`pruneViewPlaceholders` 执行了 `query` 与 `callers/callees`。图确认 `NewLoaderWithStore` 调用 `NewFileRouter`、`insert_meta`、`newAuroraFileRouter`、`should_skip_rules`、`EstimateRealSizeForFile` 和 `pruneViewPlaceholders`；同名 Go/Rust 符号使 callers 结果不完整，因此用定点文本搜索补充上游证据。
- 源与边界：完整阅读 `pkg/lightning/mydump/loader.rs`；读取 `pkg/lightning/mydump/lib.rs` 和 `pkg/lightning/mydump/Cargo.toml`；定点阅读 `pkg/importsdk/file_scanner.rs` 的调用与估算路径。
- Go 对照：阅读 `pkg/lightning/mydump/loader.go` 的配置、构造、`ParallelProcess`、`setup`、文件信息构造、过滤、视图清理及压缩估算段落。
- 独立 Rust 测试：完整阅读 `pkg/lightning/mydump/loader_test.rs`。测试覆盖基本装载、空库、重复库/表、缺 schema、大小与分片排序、过滤和自定义路由、特殊字符、选项与迭代器、压缩估算、Aurora 正反例、扫描截断，以及并行结果顺序和错误。
- Go 测试定位：`pkg/lightning/mydump/loader_test.go` 覆盖同源装载、选项、压缩采样和并行处理；Rust 文档只将其作为移植语义对照，不把未移植分支写成当前能力。
- 本任务是纯文档分析，按任务约束不运行 Cargo；最终以固定 11 个二级标题的结构命令验证。
