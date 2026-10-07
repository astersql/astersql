# `br/pkg/restore/ingestrec/model_stub.rs`

## 文件定位

`model_stub.rs` 属于 `astersql-br-pkg-restore-ingestrec` library crate。`br/pkg/restore/ingestrec/lib.rs` 通过 `#[path = "model_stub.rs"]` 挂载该模块，并以 `pub use model_stub::*` 扁平再导出其公开符号。该文件不是 BR 恢复流程的独立入口，而是 `foreign_key.rs` 与 `ingest_recorder.rs` 共用的本地模型适配层。

该适配层只复刻 ingest 索引录制与外键处理所需的 `meta/model`、`infoschema`、`ast.CIStr`、MySQL 主键标志和 `types.UnspecifiedLength` 子集。`Cargo.toml` 也印证了这一隔离目标：本 crate 唯一直接依赖是本地 `astersql-errors`，没有直接拉入完整元数据、解析器或存储依赖。它应被理解为迁移期的受限 stand-in，而不是这些上游包的完整替代实现。

## 核心职责

文件承担四类职责：

1. 提供 ingestrec 所需的元数据形状，包括 `CIStr`、列/索引/表/外键/库信息，以及 DDL job、finished args 和相关枚举。
2. 实现外键索引安全判断：`IsIndexPrefixCovered` 检查索引前缀，`IsIndexPrefixCoveredForForeignKey` 再检查部分索引谓词是否符合 MATCH SIMPLE 下的安全条件。
3. 用 `InfoSchema: Send + Sync` 收敛调用方需要的四个查询能力，使录制器和外键管理器可接内存替身测试，而不依赖完整 domain/kv 环境。
4. 通过 `GetFinishedModifyIndexArgs`、`GetFinishedModifyColumnArgs`、`annotatef` 和 `trace` 提供最小错误边界，保持 ingestrec 主流程的 Go API 形状。

它明确不负责解析任意 SQL 表达式、解码真实 DDL job 序列化格式、维护完整 schema 快照或执行索引恢复；这些能力仍属于 Go/canonical 模型和 ingestrec 的其他文件。

## 主要符号

- `UnspecifiedLength: isize = -1` 与 `PriKeyFlag: u32 = 1 << 1` 分别对应 Go `types.UnspecifiedLength` 和 `mysql.PriKeyFlag`；`HasPriKeyFlag` 只做按位判定。
- `CIStr { O, L }` 保存原始文本和小写文本，`CIStr::new` 同步生成 `L`。查找逻辑通常使用 `L`，生成 SQL 或展示名称时保留 `O`。
- `ColumnInfo`、`IndexColumn`、`IndexInfo`、`TableInfo`、`FKInfo`、`DBInfo`、`ReferredFKInfo` 是 ingestrec 使用的最小元数据结构。`ColumnInfo::GetFlag/GetFlen`、`IndexInfo::HasCondition`、`TableInfo::FindIndexByName` 对应 Go 侧的访问方法。
- `FindColumnInfo` 按调用方传入的小写名匹配 `ColumnInfo.Name.L`；它不会在函数内部再次规范化输入。
- `IsIndexPrefixCovered` 和 `IsIndexPrefixCoveredForForeignKey` 是本文件的主要行为函数；私有函数 `isIndexConditionCoveredByForeignKeyCols` 承担部分索引谓词检查。
- `ReorgType`、`ActionType`、`JobState` 只保留 ingestrec 会观察的变体，并用 `repr` 固定与 Go 常量相同的判别值。`ReorgType::LitMerge` 是指向 `TxnMerge` 的本地兼容别名。
- `Job` 只保存表 ID、动作、状态、重组类型和两类 finished args；`FinishedModifyIndexArgs/IndexArg` 与 `FinishedModifyColumnArgs` 只保留录制器实际读取的索引 ID。
- `InfoSchema` 定义 `GetTableReferredForeignKeys`、`TableByName`、`TableInfoByID`、`SchemaByID` 四个方法，并要求实现者同时满足 `Send + Sync`。
- `Context` 是无字段占位类型；`ColumnArg = String` 将 Go 的 `any` 收窄为当前实际传递的列名字符串。

## 执行流程

外键索引判断从 `IsIndexPrefixCoveredForForeignKey` 开始。它先调用 `IsIndexPrefixCovered`：索引列数必须不少于 FK 列数；每个前缀位置的列名小写值必须相等；`IndexColumn.Offset` 必须落在 `TableInfo.Columns` 内；若索引指定了前缀长度，该长度不能小于列的 `Flen`。任一步失败都会立即返回 `false`。

基础覆盖成立后，`isIndexConditionCoveredByForeignKeyCols` 处理部分索引。无谓词时直接安全；有谓词时，仅接受大小写无关的 `col IS NOT NULL` 或反引号形式，也允许限定名并只取最后一段列名。解析后的列名必须属于 FK 列集合，否则返回 `false`。这与 Go `pkg/meta/model/index.go` 的语义目标一致，但 Rust 版本是字符串级轻量解析，而 Go 版本通过 `ConditionExpr()` 和 AST 类型检查。

录制 job 时，`ingest_recorder.rs::TryAddJob` 根据 `Job.ReorgMeta`、`ActionType` 与 `JobState` 过滤输入；加索引/主键调用 `GetFinishedModifyIndexArgs`，改列调用 `GetFinishedModifyColumnArgs`。本文件的 getter 克隆预先装入 `Job` 的参数，缺失时返回错误。之后 `UpdateIndexInfo` 通过 `InfoSchema` 获取表和库，使用本文件的元数据形状构造列列表，并把表交给外键管理器。

外键侧，`foreign_key.rs::NewForeignKeyRecordManagerForTables` 使用 `FindColumnInfo` 和 `HasPriKeyFlag` 跳过 PK-handle 特例，通过 `InfoSchema` 查 referred FK 与子表；`TableForeignKeyRecordManager::RemoveForeignKeys` 再用 `IsIndexPrefixCoveredForForeignKey` 判断某个索引是否安全覆盖约束列。

## 数据与状态

所有模型数据都是拥有所有权的值：字符串使用 `String`，集合使用 `Vec`，可选 finished args 使用 `Option`。大多数结构实现 `Clone` 和 `Default`，便于录制器保存快照及测试构造夹具；这里没有全局可变状态或内部缓存。

`CIStr` 的不变量是经 `new` 构造时 `L == O.to_lowercase()`；但字段公开且 `Default` 也可产生空值，调用方仍可能手工构造不一致的 `O/L`。索引覆盖还依赖 `IndexColumn.Offset` 指向 `TableInfo.Columns`，本文件只在 `IsIndexPrefixCovered` 中检查越界；其他使用者（例如 `UpdateIndexInfo`）直接按 offset 索引，调用方必须提供一致元数据。

枚举 `Default` 分别落到 `ReorgType::None`、`ActionType::Other`、`JobState::None`，对应 Go 零值。`Job.finished_index_args` 和 `finished_column_args` 是 crate 内字段而非公开 API；测试和同 crate 录制逻辑可装配它们，外部 crate 不能直接模拟完整解码过程。

## 依赖与调用关系

直接下游只有 `astersql_errors::{Annotate, SharedError, Trace}`。`annotatef` 和 `trace` 将具体错误库调用封装在本地，使其余 ingestrec 代码使用统一的 `SharedError`。

模块上游关系如下：

- `lib.rs` 声明并公开再导出 `model_stub`。
- `ingest_recorder.rs` 直接导入 `ActionType`、`Job`、finished-args getter、`InfoSchema`、`Context`、错误包装函数以及列/索引基础类型。
- `foreign_key.rs` 直接导入 `FindColumnInfo`、`HasPriKeyFlag`、`IsIndexPrefixCoveredForForeignKey`、`InfoSchema` 与表/外键结构。
- `parity_test.rs`、`model_stub_test.rs`、`foreign_key_test.rs` 和 `ingest_recorder_test.rs` 使用这些公开形状构造内存模型和 `InfoSchema` 替身。

RustCodeGraph 确认文件已被索引（58 个符号），并给出关键内部调用边 `IsIndexPrefixCoveredForForeignKey -> IsIndexPrefixCovered -> ColumnInfo::GetFlen`，以及 `IsIndexPrefixCoveredForForeignKey -> isIndexConditionCoveredByForeignKeyCols -> IndexInfo::HasCondition`。由于常见符号名会产生跨仓库同名噪声，具体 ingestrec 上游关系以模块导入和相邻调用点交叉核验。

## 错误处理与边界

`GetFinishedModifyIndexArgs` 与 `GetFinishedModifyColumnArgs` 在参数缺失时分别生成明确的 `SharedError`，从而使 `TryAddJob` 失败而不是静默遗漏索引。与 Go 的真实 getter 不同，它们不区分 JobVersion、不反序列化 job args、也不处理 rollback/drop-index 分支；这些是该 stand-in 的刻意边界。

`annotatef` 与 `trace` 假设底层 `Annotate(Some(err), ...)` 和 `Trace(Some(err))` 对已有错误一定返回 `Some`，因此使用 `expect`。传入的是非空 `SharedError`，正常路径不会触发 panic；若错误库契约改变，这两个包装点需要同步调整。

索引覆盖函数采用保守拒绝：索引列不足、列名不一致、offset 越界、前缀过短、空谓词列、非 `IS NOT NULL` 表达式或非 FK 列谓词均返回 `false`。但 `IndexColumn.Offset` 是 `i32`，代码在检查前转换为 `usize`；负数会成为大正数并由越界判断拒绝。轻量谓词解析不等价于完整 SQL 解析器，复杂等价表达式即使理论安全也会被拒绝。

## 并发与资源生命周期

该文件不创建线程、异步任务、通道、锁、事务、文件句柄或网络连接；所有函数均为同步、短生命周期的纯数据操作或错误包装。`IsIndexPrefixCovered*` 只借用输入，getter 通过克隆返回独立数据，不保留外部引用。

`InfoSchema: Send + Sync` 允许调用方跨线程共享具体实现，但 trait 本身不规定锁策略、快照一致性或事务生命周期；这些责任属于实现者。当前 Rust 测试用 `HashMap` 驱动的不可变 `MockIS`，并不证明真实生产 infoschema 的并发语义。

性能上，覆盖判断对 FK 列数线性扫描，部分谓词再线性扫描 FK 列；`FindColumnInfo` 和 `FindIndexByName` 也是切片线性查找。典型元数据集合较小，文件没有额外索引或缓存。`CIStr::new`、谓词规范化和按名查找会分配小写字符串，扩展热路径时需评估这一成本。

## 与 Go 版本的对应关系

本文件没有同名 Go 文件；它聚合了多个 Go 包的最小投影。业务主流程对照文件是 `br/pkg/restore/ingestrec/foreign_key.go` 与 `ingest_recorder.go`，canonical 模型算法与常量来自 `pkg/meta/model/index.go`、`job.go`、`job_args.go`、`reorg.go`，名称类型和标志还分别对应 `pkg/parser/ast`、`pkg/parser/mysql` 与 `pkg/types`。

已核对的语义包括：`ReorgType` 的 0..3 顺序；`ActionAddIndex=7`、`ActionDropIndex=8`、`ActionModifyColumn=12`、`ActionAddPrimaryKey=32`；`JobStateRollbackDone=3`、`Done=4`、`Synced=6`；索引前缀覆盖的列数、名称、offset 和 Flen 判定；以及 MATCH SIMPLE 下只有 FK 列的 `IS NOT NULL` 谓词可安全使用。

差异必须保留在设计认知中：Go 使用真实 AST 解析部分索引谓词，Rust 只接受有限字符串形态；Go finished-args getter 支持版本化解码和更多动作状态，Rust 只读取预装入的两类字段；Go `context.Context` 可携带取消和调用值，Rust `Context` 当前为空；Go `ColumnArgs` 是 `[]any`，Rust 限定为 `Vec<String>`；完整 `infoschema.InfoSchema` 远大于这里的四方法 trait。因此扩展时不能假设该文件已经覆盖上游全部兼容面。

## 扩展指南

新增 ingestrec 所需字段或动作时，应先从 `foreign_key.go` / `ingest_recorder.go` 的实际增量确定最小模型面，再修改对应结构、getter 或枚举，不要递归复制完整 `meta/model`。若增加枚举变体，必须核对 Go 数值并扩展 `model_stub_test.rs::model_enum_discriminants_match_go`；若增加 finished args，需同时覆盖成功、缺参和错误传播测试。

扩展部分索引支持时，首选接入 canonical parser/AST 能力；若仍保持轻量解析，必须明确可接受语法并以保守拒绝为默认，测试限定名、反引号、大小写、复合 FK、非 FK 列、比较表达式和非法文本。任何行为变化都应同步 `model_stub_test.rs` 与 `foreign_key_test.rs`，并对照 `pkg/meta/model/index_test.go` 的 canonical 用例。

扩展 `InfoSchema` 时只添加调用方确实需要的方法，并在 `parity_test.rs`、`foreign_key_test.rs`、`ingest_recorder_test.rs` 的独立 `MockIS` 实现中同步。若 `Context` 开始承载取消或资源生命周期，也必须把错误/取消传播接入直接调用者，而不能继续把它当作纯 API 占位。

修改元数据 offset 或公开字段时，应复核所有直接索引点，尤其是 `ingest_recorder.rs::UpdateIndexInfo`；本文件的覆盖函数会安全拒绝越界，但其他路径可能 panic。测试逻辑继续放在独立 `*_test.rs` 文件中，不内嵌回生产源文件。

## 验证依据

事实取证使用 RustCodeGraph 状态和文件节点确认索引覆盖，并查询了 `IsIndexPrefixCovered`、`IsIndexPrefixCoveredForForeignKey`、`InfoSchema`、finished-args getter、错误包装函数的调用关系。直接读取并交叉核验的 Rust 文件为：`model_stub.rs`、`lib.rs`、`foreign_key.rs`、`ingest_recorder.rs`、`model_stub_test.rs`、`foreign_key_test.rs`、`ingest_recorder_test.rs`、`parity_test.rs`，crate 边界依据 `br/pkg/restore/ingestrec/Cargo.toml`。

Go 对照依据为 `br/pkg/restore/ingestrec/foreign_key.go`、`ingest_recorder.go`，以及 `pkg/meta/model/index.go`、`job.go`、`job_args.go`、`reorg.go`。Rust 独立测试证据包括：枚举判别值和限定部分索引列测试；外键管理器对普通/部分索引及 PK-handle 的场景；录制器对 finished args、schema 查询和列参数的覆盖；`parity_test.rs` 串联 `TryAddJob -> RewriteTableID -> UpdateIndexInfo -> Iterate/IterateForeignKeys`。

本任务仅做静态文档分析，按计划不运行 Cargo。交付结构检查要求目标文件存在，并且上述十一个固定二级标题各出现一次；人工复核还需确认文档没有把本地桩描述成完整生产实现，也没有建议把测试写回源文件。
