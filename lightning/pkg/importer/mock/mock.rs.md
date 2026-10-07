# `lightning/pkg/importer/mock/mock.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-importer-mock` crate 的主要行为实现，由同目录 `lib.rs` 通过 `#[path = "mock.rs"] mod mock` 装入并整体 `pub use`。它对应 Go 包 `lightning/pkg/importer/mock/mock.go`，专供 importer 相关测试构造两类可控替身：源端的 `ImportSource` 和目标端的 `TargetInfo`。

该 crate 的 `Cargo.toml` 仅直接依赖 `astersql-lightning-pkg-importer-opts`；mydump、对象存储、PD HTTP、模型、错误等类型来自同 crate 的 `stubs.rs`。因此它是无真实 TiDB、TiKV、PD、数据库或网络依赖的测试基建，而不是生产导入链上的真实客户端。RustCodeGraph 显示本文件直接被 `lightning/pkg/importer/mock/mock_test.rs` 使用，并被索引为 `lightning/pkg/importer/get_pre_info_test.rs` 的依赖之一；精确符号调用则主要落在本 crate 的单元与 parity 测试中。

## 核心职责

1. `NewImportSource` 把测试手写的数据库—表—文件树转换为 mydump 风格的 `MDDatabaseMeta`/`MDTableMeta`，同时把 schema 和数据字节写入共享的 `MemStorage`。
2. `ImportSource` 同时保留原始输入、派生元数据和存储句柄，并通过 getter 提供不同观察视图。
3. `TargetInfo` 记录系统变量、表结构与行数、store 容量、副本数和空 region 数量，再以 importer 预检所需的方法形状返回这些信息。
4. 保持 Go mock 的关键语义，包括文件后缀识别、默认副本数、缺库错误文案、缺表视为空表、Go `nil` 到 Rust `Option` 的映射，以及负 region 数量触发 panic。

需要特别区分“方法形状”和“trait 接线”：这些方法注明对应 `TargetInfoGetter`，但本 crate 为保持轻量没有依赖 importer crate，也没有为 `TargetInfo` 写 `impl TargetInfoGetter`。`mock_test.rs::test_mock_target_info_basic` 明确记录了这一差异。因此当前实现适合作为独立测试替身，不能仅凭方法同名就断言它可直接作为 `Arc<dyn TargetInfoGetter>` 注入真实 Rust 预检链。

## 主要符号

- `SourceFile { FileName, Data, TotalSize }`：单个虚拟文件。`TotalSize == 0` 时使用 `Data.len()`；非零值（包括负值）原样参与元数据大小计算，以保持 Go `int` 契约。
- `TableSourceData { DBName, TableName, SchemaFile, DataFiles }`：表级输入描述。实际构建时数据库名和表名取外层 `HashMap` 的键；结构体里的 `DBName`、`TableName` 仅随原始输入保存在 `dbSrcDataMap` 中。
- `DBSourceData { Name, Tables }`：数据库级输入描述。同样，派生元数据的数据库名取顶层 map 键。
- `ImportSource { dbSrcDataMap, dbFileMetaMap, srcStorage }`：同时持有原始树、派生 mydump 元数据和内存对象存储。三个字段均为私有。
- `NewImportSource(...) -> Result<Box<ImportSource>>`：源端唯一构造入口，负责校验文件类型、生成元数据和写入文件内容。
- `ImportSource::{GetStorage, GetDBMetaMap, GetAllDBFileMetas, src_storage}`：分别返回共享存储句柄、元数据 map 借用、元数据值的扁平借用集合，以及供同 crate 测试直接读取存储的借用。
- `StorageInfo`：单 store 的总量、已用量、可用量和 region 数；前三项为 `u64`，`RegionCount` 为与 Go `int` 对齐的 `isize`。
- `TableInfo { RowCount, TableModel }`：表行数与可选模型。`None` 表达 Go map 中存在键但值为 `nil`。
- `TargetInfo`：公开字段承载副本、空 region 和 store 信息；私有 map 承载系统变量与按 schema/table 分层的表信息。
- `NewTargetInfo() -> Box<TargetInfo>`：创建空 store、空系统变量和空表信息的目标端替身，其余字段取默认值。
- `SetSysVar`、`SetTableInfo`：测试配置入口，均为覆盖写入。
- `FetchRemoteDBModels`、`FetchRemoteTableModels`、`GetTargetSysVariablesForImport`、`GetMaxReplica`、`GetStorageInfo`、`GetEmptyRegionsInfo`、`IsTableEmpty`、`CheckVersionRequirements`：目标端查询方法面。
- `_error_ty`：仅用于压制 `Error` 类型导入未使用告警，无运行时职责。

本文件没有模块级常量、trait 实现、异步函数或条件编译项。

## 执行流程

`NewImportSource` 的主流程如下：

1. 创建后台 `Context`、空的数据库元数据 map 和一个 `MemStorage`。
2. 遍历顶层数据库 map，为每个数据库创建 `SourceType::SchemaSchema` 的数据库 schema `FileInfo`，并以字符集 `binary` 初始化 `MDDatabaseMeta`。
3. 遍历数据库的表 map；要求 `SchemaFile` 必须为 `Some`，否则以 `expect` 直接 panic。创建 `MDTableMeta`，同样使用 `binary`，并根据 schema 文件是否以 `.gz` 结尾设置压缩格式。
4. 把 schema 文件字节写入 `MemStorage`。写失败时经 `errors::Trace` 原样向上传播。
5. 逐个处理数据文件：`TotalSize == 0` 时退回真实字节长度，否则采用声明值；所有值用 `wrapping_add` 汇总为表总大小。
6. 最多剥离一个 `.gz` 后缀，再按剩余文件名末尾识别 `.csv`、`.sql` 或 `.parquet`。未知后缀立即返回包含原文件名的错误。
7. 记录 `Path`、`FileSize`、`RealSize`、`Compression` 和 `SourceType`，把数据字节写入同一存储；最后把表、数据库元数据逐层装配进 `ImportSource`。

`TargetInfo` 的使用流程是“先录入，后回放”：测试先用公开字段、`SetSysVar` 和 `SetTableInfo` 写入所需场景，再调用查询方法。数据库枚举从 `dbTblInfoMap` 的键生成最小 `DBInfo`；表查询只返回请求列表中确实存在的键；容量查询按 `StorageInfos` 顺序产生从 1 开始的 store ID；空 region 查询把每个 store 的计数展开为相应数量、各含一个 peer 的 `RegionInfo`。

## 数据与状态

`ImportSource` 在构造完成后没有内部可变入口。`dbSrcDataMap` 保留输入所有权，但当前文件没有 getter；`dbFileMetaMap` 是预先计算的缓存，后续 getter 不再做 IO 或重新派生；`srcStorage` 的克隆共享底层文件表。`GetAllDBFileMetas` 从 `HashMap::values()` 收集借用，数量稳定但跨运行顺序不保证，调用者不应依赖数据库枚举顺序。

表元数据中的 `DB`、`Name` 以及文件的 `TableName` 来自 map 键，而非 `DBSourceData.Name`、`TableSourceData.DBName` 或 `TableSourceData.TableName`。扩展输入校验时必须注意这一现有事实，避免误以为这些重复字段会参与一致性检查。

`TargetInfo` 是普通可变结构，没有内部锁。系统变量 getter 克隆整个 map，调用方修改返回值不会污染内部状态；表模型查询也克隆 `Option<Box<model::TableInfo>>`。`SetTableInfo` 会自动创建 schema 的二级 map，并覆盖同名表。`CheckVersionRequirements` 当前恒成功；它构造一次默认 `pdhttp::StoresInfo` 仅为保持依赖面，没有实际检查状态。

大小和计数存在刻意的 Go 对齐：公开 Go `int` 字段映射到 Rust `isize`；数据文件大小再转为 `i64`；总大小使用 `wrapping_add`。store 容量字符串由 `units::BytesSize` 生成，store 状态固定为 `Up`。

## 依赖与调用关系

上游方面，RustCodeGraph 对 `NewImportSource` 的精确索引为 `mock.rs::NewImportSource`，调用者包括 `mock_test.rs::test_mock_import_source_basic`、`test_import_source_strips_only_one_gzip_suffix`，以及 `parity_test.rs` 的正常、边界、错误和生命周期契约。`NewTargetInfo` 及其方法同样由这两份独立测试驱动。crate 入口 `lib.rs` 把全部符号重新导出，使测试可用 `crate::*` 模拟 Go 同包访问。

下游方面，`NewImportSource` 调用/组合 `context::Background`、`objstore::NewMemStorage`、`MemStorage::WriteFile`、`mydump::{NewMDDatabaseMeta, NewMDTableMeta}`、`filter::Table` 和 `errors::{Trace, Errorf}`。`TargetInfo` 使用 `ast::NewCIStr` 构造大小写感知名称，用 `dbterror::ClassSchema`、`errno::ErrBadDB` 生成缺库错误，用 `units::BytesSize` 格式化容量，并装配 `pdhttp` stub 的 store/region 类型。

与完整应用的关系是契约模拟而非直接生产接线：Go `TargetInfo` 满足 `lightning/pkg/importer/get_pre_info.go::TargetInfoGetter` 并用于 importer 测试；Rust 真实 trait 位于 `lightning/pkg/importer/get_pre_info.rs::TargetInfoGetter`，当前 mock crate 没有实现它。若未来需要把此 mock 注入 Rust precheck，必须在不形成 crate 依赖环的前提下增加适配层，并以 trait 签名为准处理当前方法参数/返回值差异。

## 错误处理与边界

- 缺少表 schema 文件不是 `Result::Err`，而是 `expect("SchemaFile required like Go non-nil *SourceFile")` panic；这是对 Go nil 解引用前提的显式表达。
- 内存存储写失败经 `errors::Trace` 返回；当前 `MemStorage` 正常路径通常不会失败，但传播逻辑仍是契约的一部分。
- 数据文件只接受 CSV、SQL、Parquet，可在类型后附一层 `.gz`。`.csv.gz.gz` 只剥一层后仍不匹配，返回 `unsupported file type: <原路径>`。
- schema 文件只依据是否以 `.gz` 结尾设置压缩，不验证去压缩后的 schema 扩展名。
- `TotalSize == 0` 才表示“使用真实长度”；负值不会被拒绝，parity 测试要求其按 Go `int` 语义透传。
- `FetchRemoteTableModels` 对缺 schema 返回带 HTTP 外壳、`Unknown database`/1049 信息的错误；存在 schema 但缺表时跳过该键；表存在但模型为空时返回 `Some(key -> None)`。
- `GetMaxReplica` 对零或负值回退为 1。
- `GetEmptyRegionsInfo` 将负计数转 `usize` 时以 `expect("makeslice: len out of range")` panic，与 Go `make` 负长度一致；大计数还可能造成大量分配。
- `IsTableEmpty` 对缺库或缺表返回 `true`，存在表时仅判断 `RowCount == 0`，负行数因此会被视为非空。
- `CheckVersionRequirements` 不模拟版本不兼容错误，任何需要版本失败分支的测试不能依赖此实现。

## 并发与资源生命周期

本文件不创建线程、任务、通道、事务或取消流程；传入的 `Context` 在目标端方法中均未使用，`NewImportSource` 只创建一个同步后台 context 用于内存写入。

`MemStorage` 的克隆句柄共享底层所有权。`parity_test.rs::contract_resource_cleanup` 证明：通过 `GetStorage` 取得句柄后，即使 `ImportSource` 被 drop，句柄仍能读取已写文件。因此新增资源清理逻辑时不能把 `GetStorage` 改成依赖源对象生命周期的弱引用或临时借用。

`TargetInfo` 自身没有 `Send`/`Sync` 接线、锁或原子变量；并发读写必须由外部同步。查询通常返回拥有所有权的克隆或新装配结果，避免调用方持有内部可变借用。大文件数据、表模型和系统变量的 clone 会产生相应内存成本，但作为测试 mock，当前实现优先保证隔离与简单生命周期。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `lightning/pkg/importer/mock/mock.go`：核心结构、构造顺序、`binary` 字符集、单层 `.gz` 剥离、三种数据类型、size 回退、store ID、`Up` 状态、空 region 展开、缺库错误和空表判断均保持一致。`mock_test.rs` 对应 Go 的 `TestMockImportSourceBasic` 与 `TestMockTargetInfoBasic`；`parity_test.rs` 进一步锁定 Go/Rust 公共契约。

语言层面的显式差异包括：Go 指针和 nil 用 `Box`/`Option<Box<_>>` 表达；Go `int` 用 `isize` 表达；Go `maps.Clone` 用 `HashMap::clone` 表达；Go 返回 storage 接口，Rust 返回具体 `MemStorage`；Go 同包可访问 `srcStorage`，Rust 增加 `src_storage()`；Go 的 map 迭代和 Rust `HashMap` 迭代都不承诺业务顺序。

最重要的迁移缺口是 trait：Go 测试含 `var _ importer.TargetInfoGetter = ti` 的编译期检查，Rust 测试明确省略，因为本 crate 为避免 importer/kv/domain 等依赖保持 slim。除此以外，Rust `_error_ty` 和 `CheckVersionRequirements` 内的默认值构造属于编译/依赖占位，不对应额外业务行为。

## 扩展指南

- 新增源文件格式：修改 `NewImportSource` 的后缀判定与 `SourceType` 投影，并在独立的 `mock_test.rs` 或 `parity_test.rs` 增加正常、`.gz`、未知后缀及大小字段断言；同时核对 Go `mock.go`，不能只扩 Rust。
- 新增元数据字段：在构建 `FileInfo`、`MDTableMeta` 或 `MDDatabaseMeta` 的位置赋值，并验证 getter 观察到的派生结果。先明确字段应取 map 键还是输入结构体中的重复名称。
- 新增目标端预检查询：优先在 `TargetInfo` 添加最小状态与回放方法，再评估是否需要在 importer crate 提供适配器实现真实 `TargetInfoGetter`；不要为通过测试把整个生产依赖树复制进 mock crate。
- 改动错误行为：同步更新 Go 对照和 `parity_test.rs::contract_error`。缺库错误的外壳与文件名信息被测试当作兼容信号。
- 改动资源所有权：保留 `GetStorage` 在 `ImportSource` drop 后仍可用、系统变量返回独立副本两项契约。
- 改动计数类型或默认值：参考 `go_int_fields_keep_signed_pointer_width`、`negative_empty_region_count_panics_like_go` 和 `contract_boundary`；不得把 `isize` 随意收窄或改成无符号类型。
- 测试必须继续放在独立文件 `mock_test.rs`/`parity_test.rs`，不要内嵌进生产 `mock.rs`。本文件是 Go 复刻实现，修改时应尽可能同步 Go 逻辑而非简化版本。

兼容风险主要是 trait 方法形状、错误文本、nil/`Option` 语义与默认值；性能风险主要是克隆大 map/模型、保存全部文件字节，以及按空 region 计数逐项分配。作为 mock 这些通常可接受，但构造大规模压力场景时应显式评估内存。

## 验证依据

- 源码：`lightning/pkg/importer/mock/mock.rs`，核对全部 459 行及其中 38 个索引符号。
- crate 边界：`lightning/pkg/importer/mock/Cargo.toml`、`lib.rs`；确认 crate 名、唯一直接依赖、stub/实现装配和测试模块位置。
- Go 对照：`lightning/pkg/importer/mock/mock.go`，核对全部 337 行；`BUILD.bazel` 用于确认 Go 包的真实生产依赖与 mock 测试目标。
- 独立测试：`lightning/pkg/importer/mock/mock_test.rs`，覆盖基本源端投影、单层 gzip 剥离、系统变量、副本数、store、region、表模型和空表判断；`mock_test.go` 提供原始 Go 用例意图。
- 契约测试：`lightning/pkg/importer/mock/parity_test.rs`，覆盖正常、边界、错误、克隆/生命周期、Go `int` 宽度和负 region 计数 panic。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter lightning/pkg/importer/mock` 确认该目录 7 个索引源文件；`query NewImportSource --kind function` 定位 Go/Rust 两个实现；`query NewTargetInfo --kind function`、`query FetchRemoteTableModels --kind method` 区分 mock 与真实 getter；`node --file` 核对目标、Go 对照及两份 Rust 测试；`explore` 确认 `NewImportSource` 的直接 Rust 调用来自 `mock_test.rs`，并确认真实 `TargetInfoGetter` 位于 `get_pre_info.rs`。
- 人工复核结论：本文件存在是为了用纯内存、可编程状态复刻 importer 测试所需的源端与目标端边界；运行方式是先构造状态再通过 getter/查询方法回放；安全扩展必须同时维护 Go 语义、独立测试、错误与生命周期契约，并正视当前未实现真实 Rust trait 的边界。
