# `pkg/importsdk/error.rs`

## 文件定位

`pkg/importsdk/error.rs` 属于 Cargo 包 `astersql-importsdk`；包入口 `pkg/importsdk/lib.rs` 以 `mod error` 装入该模块，并通过 `pub use error::*` 把其中的公开静态量提升为 crate 公共 API。这个文件不执行导入，也不定义控制流入口，而是为文件扫描、schema 创建、通配符生成、导入作业管理和 `IMPORT INTO` SQL 生成提供统一的错误分类标识。

`pkg/importsdk/Cargo.toml` 直接依赖 `astersql-errors`，本文件只使用该依赖和标准库 `std::sync::LazyLock`。文件中没有 feature gate、条件编译项、函数、trait、结构体或枚举；全部业务内容是 13 个 `pub static` 哨兵错误。

## 核心职责

本文件的职责是把导入 SDK 中需要被调用方识别的失败类别集中定义为稳定的 `astersql_errors::SharedError`：

- 扫描与建表类：`ErrNoDatabasesFound`、`ErrSchemaNotFound`、`ErrTableNotFound`、`ErrNoTableDataFiles`、`ErrWildcardNotSpecific`、`ErrParseStorageURL`、`ErrCreateExternalStorage`、`ErrCreateLoader`、`ErrCreateSchema`。
- 作业类：`ErrJobNotFound`、`ErrNoJobIDReturned`、`ErrInvalidOptions`。
- SQL 选项类：`ErrMultipleFieldsDefinedNullBy`。

这些值既提供面向人的稳定基础消息，也提供可供 `errors::ErrorEqual` 比较的根因。具体调用点可以直接克隆哨兵，也可以用 `Annotate` 附加 source、schema、table、job 等上下文，而不丢失根因分类。该职责由 `pkg/importsdk/file_scanner.rs`、`job_manager.rs`、`pattern.rs` 和 `sql_generator.rs` 的引用共同验证。

## 主要符号

所有符号的共同类型均为 `LazyLock<errors::SharedError>`，初始化闭包均调用 `errors::New(<固定消息>)`：

| 符号 | 固定基础消息 | 当前语义与直接使用位置 |
| --- | --- | --- |
| `ErrNoDatabasesFound` | `no databases found in the source path` | loader 未发现数据库；`fileScanner::CreateSchemasAndTables`、`EstimateImportDataSize` |
| `ErrSchemaNotFound` | `schema not found` | 按名创建时未找到 schema；`CreateSchemaAndTableByName` |
| `ErrTableNotFound` | `table not found` | schema 中没有目标表或按名扫描未命中；`CreateSchemaAndTableByName`、`GetTableMetaByName` |
| `ErrNoTableDataFiles` | `no data files for table` | 表文件列表为空，无法生成导入路径；`generateWildcardPath` |
| `ErrWildcardNotSpecific` | `cannot generate a unique wildcard pattern for the table's data files` | 候选模式不能“全部且仅”匹配目标表文件；`generateWildcardPath` |
| `ErrJobNotFound` | `job not found` | 单个 job 或 group 查询无行；`GetJobStatus`、`GetGroupSummary` |
| `ErrNoJobIDReturned` | `no job id returned` | 提交语句结果集为空；`SubmitJob` |
| `ErrInvalidOptions` | `invalid options` | 当前用于拒绝空 group key；`GetGroupSummary`、`GetJobsByGroup` |
| `ErrMultipleFieldsDefinedNullBy` | `IMPORT INTO only supports one FIELDS_DEFINED_NULL_BY value` | CSV 配置给出多个 NULL 标记；`buildCSVOptions` |
| `ErrParseStorageURL` | `failed to parse storage backend URL` | `ParseBackend` 失败；`NewFileScanner` |
| `ErrCreateExternalStorage` | `failed to create external storage` | 外部存储客户端创建失败；`NewFileScanner` |
| `ErrCreateLoader` | `failed to create MyDump loader` | MyDump loader 创建失败；`NewFileScanner` |
| `ErrCreateSchema` | `failed to create schemas and tables` | `SchemaImporter::Run` 失败；两个 schema 创建入口 |

这些名称沿用 Go 导出标识符的大小写。crate 根的 `#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]` 允许 Rust 公共 API 保持这种 Go 兼容命名。

## 执行流程

1. 第一次解引用某个静态量（例如 `&ErrTableNotFound` 或 `*ErrInvalidOptions`）时，`LazyLock` 执行对应闭包。
2. 闭包调用 `astersql_errors::New` 构造带栈的基础 `SharedError`，随后由 `LazyLock` 缓存；之后访问复用同一静态值。
3. 调用方有两种主要用法：
   - `job_manager.rs` 和 `sql_generator.rs` 通过 `clone()` 返回基础哨兵。`SharedError` 内部以 `Arc` 共享，克隆不会复制整个错误对象。
   - `file_scanner.rs` 和 `pattern.rs` 通过 `Annotate` 包装克隆后的哨兵，增加经脱敏的 source、schema/table 或失败原因。`Annotate` 保留 cause 链并确保错误链带有有效栈。
4. 上层可展示完整上下文消息，也可用 `ErrorEqual` 下钻到根因后比较；`file_scanner_test.rs` 和 `job_manager_test.rs` 展示了这种分类检查。

本文件本身没有循环、分支或 I/O；真正的失败判定发生在上述四个调用模块中。

## 数据与状态

唯一持久状态是 13 个进程级 `LazyLock<SharedError>`。每个槽位在首次访问前未初始化，首次访问后持有一个可共享错误值，生命周期持续到进程结束。

`SharedError` 在 `pkg/errors/core.rs` 中定义为持有 `Arc<dyn Error + Send + Sync + 'static>` 的可克隆值，因此这些静态错误可以跨所有权边界和线程安全地传播。它们不包含导入任务 ID、路径、配置或数据库连接等可变业务状态；动态上下文由调用方包装，避免改变全局哨兵。固定消息是兼容面的一部分，因为 `ErrorEqual` 对非规范化错误在指针不同时还会回退到展示文本比较。

## 依赖与调用关系

- 上游装配：`pkg/importsdk/lib.rs` 声明 `mod error` 并 `pub use error::*`；crate 内模块可经 `crate::Err...` 使用，外部 crate 可经 `astersql_importsdk::Err...` 使用。
- 下游构造：`std::sync::LazyLock` 提供一次性、线程安全初始化；`astersql_errors::New` 创建基础错误。
- 扫描链：`NewFileScanner` 使用 URL/存储/loader 三个创建错误；`fileScanner` 的 schema 与 metadata 方法使用数据库、schema、table 和建表错误。
- 路径链：`generateWildcardPath` 使用无数据文件和模式不唯一错误。
- 作业链：`JobManagerImpl::{SubmitJob, GetJobStatus, GetGroupSummary, GetJobsByGroup}` 使用提交空结果、未找到和非法选项错误。
- SQL 链：`buildCSVOptions` 使用多 NULL 标记错误，最终由 SQL 生成入口向上传播。

RustCodeGraph 的 `files --filter pkg/importsdk` 确认 `error.rs` 位于该模块的生产文件集合中，`node --file pkg/importsdk/error.rs` 确认完整源码和文件级引用。当前索引把该文件记录为一个文件级符号，没有为这些 `pub static` 暴露可单独查询的节点；所以上述精确静态量调用边由对 Rust 标识符的仓库搜索补齐，并由相邻实现源码复核。

## 错误处理与边界

- 这里定义的是错误类别，不负责决定是否恢复、跳过或重试。比如 `GetTableMetas` 是否在 `skip_invalid_files` 下跳过坏表，由 `file_scanner.rs` 决定。
- `ErrJobNotFound` 只用于期望单行的 `GetJobStatus` 和 `GetGroupSummary`；`GetJobsByGroup` 的空结果是合法空向量，不能误改为该哨兵。
- `ErrInvalidOptions` 当前只覆盖空 group key，不代表所有导入配置校验都会映射到它。
- `ErrTableNotFound` 同时服务按名创建和按名获取 metadata；附加上下文不同，但根因应保持一致。
- URL 错误上下文必须继续遵守 `file_scanner.rs` 的脱敏规则：解析失败且无法安全脱敏时使用占位路径，不能把密钥写入注解。
- `ErrNoTableDataFiles` 与 `ErrWildcardNotSpecific` 是两个不同边界：前者在任何模式推导前判空，后者表示存在多个文件但所有候选模式均不具备唯一性。
- 直接返回时必须克隆 `SharedError`，包装时必须把克隆值作为 cause；不要尝试移动静态值，也不要丢弃 cause 后仅拼接字符串，否则 `ErrorEqual` 的根因语义会被破坏。

## 并发与资源生命周期

`LazyLock` 保证每个哨兵在并发首次访问时只初始化一次；`SharedError` 的 `Arc` 以及其 `Send + Sync + 'static` 动态错误边界允许错误安全地跨线程共享。克隆哨兵只增加共享所有权，不会修改静态对象。

本文件不持有文件、网络连接、游标、异步任务、锁、通道或事务，因此没有显式清理路径。资源失败发生在调用方：例如 `NewFileScanner` 创建 loader 失败时先关闭 store，再用 `ErrCreateLoader` 包装返回；这类生命周期约束不能下沉到本文件。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/importsdk/error.go`。Go 与 Rust 均定义同名的 13 个包级错误，固定消息逐项一致；Rust 用 `LazyLock<SharedError>` 表达 Go 包初始化时由 `errors.New` 创建的变量。

主要语义对应如下：Go 的 `errors.Annotate/Annotatef` 对应 Rust `astersql_errors::Annotate`（部分调用点用局部 helper 统一 unwrap）；Go 的 `errors.Is`/测试中的 `require.ErrorIs` 意图，在 Rust 测试中由 `errors::ErrorEqual` 或稳定消息断言表达。`file_scanner.go`、`job_manager.go`、`pattern.go`、`sql_generator.go` 与同名 Rust 文件保持相同错误分支；例如 Go 和 Rust 都把空 job 单行查询映射为相应哨兵，同时把 group job 列表的空结果保留为合法空列表。

差异主要是语言所有权：Go 错误接口可直接返回包级变量；Rust 必须克隆 `SharedError` 才能返回拥有值。该克隆是 `Arc` 克隆，不是语义简化。另一个细微差异是 Rust 的 `ErrWildcardNotSpecific` 注解消息不带 Go 版本末尾的句点，但根哨兵消息本身一致，分类语义不受影响。

## 扩展指南

新增可识别错误类别时，应同时完成以下最小接线：

1. 在本文件新增 `pub static ErrXxx: LazyLock<errors::SharedError>`，使用稳定、无敏感信息的基础消息；继续保持 Go 对照命名和消息，除非明确改变跨语言契约。
2. 在真正判定边界的生产模块返回该哨兵的 clone，或用 `Annotate` 增加经过脱敏的动态上下文；不要在本文件加入业务判断或资源操作。
3. 在同目录独立测试文件中覆盖触发条件和根因识别。扫描类同步 `file_scanner_test.rs`，作业类同步 `job_manager_test.rs`，模式类同步 `pattern_test.rs`，SQL 选项类同步 `sql_generator_test.rs`；Rust 测试逻辑应尽量与对应 Go 测试保持一致，不能把测试嵌进 `error.rs`。
4. 若错误需要被 crate 外使用，确认 `lib.rs` 的现有 `pub use error::*` 足够；通常无需额外再导出。若改动基础消息，需审查 `ErrorEqual` 的文本回退以及所有消息断言，兼容风险高于普通文案调整。
5. 若新增错误来自新的外部能力，应在 `Cargo.toml` 明确依赖；当前文件仅需已有的 `astersql-errors`，不应为错误目录引入与业务实现耦合的依赖。

性能风险很低，主要是首次初始化和 `Arc` clone；正确性风险集中在错误分类错用、cause 丢失和敏感上下文泄漏；兼容风险集中在符号名与固定消息变化。

## 验证依据

- 目标源码：`pkg/importsdk/error.rs`，确认 13 个公开 `LazyLock<SharedError>`、固定消息、无条件编译和无其他逻辑。
- crate 边界：`pkg/importsdk/Cargo.toml`、`pkg/importsdk/lib.rs`，确认包名、`astersql-errors` 依赖、模块装配与公开再导出。
- Rust 调用点：`pkg/importsdk/file_scanner.rs`、`job_manager.rs`、`pattern.rs`、`sql_generator.rs`。
- Rust 独立测试：`pkg/importsdk/file_scanner_test.rs`（table/schema 分支与 `ErrorEqual`）、`job_manager_test.rs`（无 job ID、job/group 未找到、空 group key）、`pattern_test.rs`（空文件列表）、`sql_generator_test.rs`（多个 `FIELDS_DEFINED_NULL_BY`）。未发现同名 `error_test.rs`，测试按消费模块分布。
- Go 对照：`pkg/importsdk/error.go` 及同目录 `file_scanner.go`、`job_manager.go`、`pattern.go`、`sql_generator.go`；相关测试为 `file_scanner_test.go`、`job_manager_test.go`、`pattern_test.go`、`sql_generator_test.go`。
- 错误机制：`pkg/errors/core.rs` 的 `SharedError`/`New`，`pkg/errors/adaptor.rs` 的 `Annotate`，`pkg/errors/normalize.rs` 的 `ErrorEqual`。
- RustCodeGraph：运行 `status`（索引包含 11,467 个文件，目标目录已索引）、`files --filter pkg/importsdk`、`node --file pkg/importsdk/error.rs`，并对目标错误名执行 `explore`；精确 `pub static` 节点未被当前索引单独暴露，调用边以 `rg` 的精确标识符结果补证。
- 本任务仅新增说明文档，不改变运行时代码；按计划不运行 Cargo。交付前使用任务给定命令验证 11 个固定二级标题，并人工复核所有结论均能回指上述源码、调用点或测试。
