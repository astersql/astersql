# `br/pkg/restore/ingestrec/foreign_key.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-ingestrec`。包边界由同目录 `Cargo.toml` 定义，库入口是 `lib.rs`；`lib.rs` 以 `#[path = "foreign_key.rs"]` 挂载本模块，并通过 `pub use foreign_key::*` 将公开符号扁平再导出。该 crate 当前只声明直接依赖 `astersql-errors`，表、索引、外键和 InfoSchema 接口均来自同 crate 的 `model_stub.rs`，而不是直接链接完整 TiDB 元数据与 InfoSchema crate。

它位于 BR ingest 索引恢复链中：`ingest_recorder.rs` 的 `IngestRecorder::UpdateIndexInfo` 在把 DDL job 中的索引 ID 补全为最新表结构时，同时调用这里的外键收集与过滤逻辑。最终保留下来的 `ForeignKeyRecord` 由 `IngestRecorder::IterateForeignKeys` 输出，供修复索引前先处理相关外键约束。文件不是门面或生成代码，包含实际的记录、过滤和合并逻辑；模块门面是同目录 `lib.rs`。

## 核心职责

本文件解决“重建 ingest 索引前，哪些外键仍依赖将被修复的索引”这一元数据归集问题，职责分三层：

1. `NewForeignKeyRecordManagerForTables` 从单表视角同时收集该表声明的外键，以及其他子表指向该表的 referred 外键。
2. `TableForeignKeyRecordManager::RemoveForeignKeys` 按索引前缀覆盖关系删除已经由非待修复索引保障的记录；自身外键检查 `FKInfo.Cols`，referred 外键检查 `FKInfo.RefCols`。
3. `ForeignKeyRecordManager::Merge` 将各表剩余记录按“子库、子表、外键名”三元组去重汇总，得到恢复阶段需要处理的全局集合。

这里记录的是约束元数据，不执行 SQL、不删除外键，也不负责索引重建。具体修复动作在上层遍历接口的消费者中完成。

## 主要符号

- `ForeignKeyRecordKey { ChildSchemaNameO, ChildTableNameO, FKNameO }`：`HashMap` 键。三个字段都使用 `CIStr.O` 原始拼写；派生 `Eq`、`Hash` 和 `PartialEq` 后用于精确去重。相同子库、子表和外键名的正向记录与反向记录会落到同一键。
- `ForeignKeyRecord { FKInfo, ChildSchemaNameO, ChildTableNameO }`：保存完整、克隆后的 `FKInfo` 以及子表位置。与 Go 的匿名嵌入 `model.FKInfo` 不同，Rust 通过显式字段 `FKInfo` 访问。
- `newForeignKeyRecordKey(...)`：私有构造器，一次生成匹配的 key/value，避免两处收集路径在命名字段上漂移。
- `ForeignKeyRecordManager { fkRecordMap }`：跨表的最终集合。`New` 方法和包级 `NewForeignKeyRecordManager` 都构造空 map；`Merge` 先写入单表自身外键，再写入 referred 外键。
- `TableForeignKeyRecordManager { fkRecordMap, referredFKRecordMap }`：单表临时集合。两个 map 分开保存“本表是子表”和“本表是父表”两种方向，便于使用不同的列集合过滤。
- `NewForeignKeyRecordManagerForTables(ctx, infoSchema, dbName, tableInfo)`：主要采集入口，返回 `Result<TableForeignKeyRecordManager, SharedError>`。当前 `_ctx` 仅为与 Go API 形状对齐，函数体不读取它。
- `TableForeignKeyRecordManager::RemoveForeignKeys(tableInfo, indexInfo)`：原地过滤两个 map，无返回值。

文件没有模块级常量、trait、枚举或条件编译项。字段当前公开，测试侧另由 `export_test.rs` 提供与 Go `export_test.go` 对齐的只读访问器。

## 执行流程

单表收集流程如下：

1. 创建两个空 map。
2. 遍历 `tableInfo.ForeignKeys`。若表是 `PKIsHandle`、外键只有一列、该列能在 `tableInfo.Columns` 中找到且带 `PriKeyFlag`，则主键句柄本身已提供覆盖，该外键不进入待处理集合；否则以当前数据库原名、当前表原名和外键原名构造记录，写入 `fkRecordMap`。
3. 调用 `InfoSchema::GetTableReferredForeignKeys(dbName.L, tableInfo.Name.L)`，以小写规范名查询所有指向当前表的外键摘要。
4. 对每条摘要应用同样的 PK-handle 单列跳过规则。未跳过时，调用 `InfoSchema::TableByName` 定位子表，再在子表的 `ForeignKeys` 中按 `ChildFKName.O == FKInfo.Name.O` 找到完整定义。
5. 找到后，以摘要给出的子库和子表原名构造记录，写入 `referredFKRecordMap`，随后 `break`；找不到同名定义时不写入也不报错。
6. 返回单表管理器。

上层 `IngestRecorder::UpdateIndexInfo` 对每个录制过 ingest 索引的表调用该入口。遍历该表所有现存索引时，待修复集合中不存在的索引会传给 `RemoveForeignKeys`：若索引安全覆盖外键列，就从临时集合移除对应约束。完成单表处理后调用 `ForeignKeyRecordManager::Merge`。所有表处理结束，最终管理器保存到 recorder，之后 `IterateForeignKeys` 遍历剩余记录。

## 数据与状态

两个管理器均只持有内存 `HashMap`，没有静态全局状态。`ForeignKeyRecord` 和 key 都拥有字符串与 `FKInfo` 克隆，因此不借用传入的 `TableInfo` 或 InfoSchema 返回对象；函数返回后记录生命周期独立于输入。

关键不变量是：

- map 键始终描述外键所属的子表，即使记录是在父表的 referred 路径发现。
- 正向和反向扫描得到的同一外键具有同一个三元组键，跨表 `Merge` 后只保留一份。
- `Merge` 的覆盖顺序是 `fkRecordMap` 后 `referredFKRecordMap`；后写入的同键值覆盖先前值。正常情况下两者代表同一完整子表 FK 定义。
- `RemoveForeignKeys` 只删除满足 `IsIndexPrefixCoveredForForeignKey` 的记录，不修改 `TableInfo`、`IndexInfo` 或 `FKInfo`。
- 自身方向必须用子表外键列 `Cols` 判断，referred 方向必须用父表被引用列 `RefCols` 判断，二者不可互换。

`HashMap` 不保证迭代顺序，因此最终外键遍历顺序不应作为外部契约。测试只断言集合长度或按名称收集，不依赖顺序。

## 依赖与调用关系

直接依赖均可由源码和 Cargo 声明确认：

- `std::collections::HashMap` 提供去重集合。
- `astersql_errors::SharedError` 是唯一跨 crate 的直接类型依赖；`Cargo.toml` 将其绑定到工作区 `pkg/errors`。
- `model_stub.rs` 提供 `CIStr`、`Context`、`FKInfo`、`IndexInfo`、`TableInfo`、`InfoSchema`、`FindColumnInfo`、`HasPriKeyFlag`、`IsIndexPrefixCoveredForForeignKey` 和 `trace`。

RustCodeGraph 将 `NewForeignKeyRecordManagerForTables -> newForeignKeyRecordKey` 识别为直接调用边。图的 callers 查询未给出跨方法边，源码精确检索补充确认生产上游为 `ingest_recorder.rs::IngestRecorder::UpdateIndexInfo`：它调用 `NewForeignKeyRecordManagerForTables`、`RemoveForeignKeys` 和 `Merge`。`foreign_key_test.rs` 是直接单元测试，`ingest_recorder_test.rs` 与 `parity_test.rs` 进一步覆盖 recorder 集成和公开可观察行为；`export_test.rs` 仅在测试配置下增加 map 访问器。

## 错误处理与边界

唯一显式错误路径发生在 referred 扫描中：`InfoSchema::TableByName` 找不到或无法读取子表时，错误经 `trace` 转换后立即返回；此前构建的局部 `tm` 随返回而丢弃，不会产生部分成功结果。正向扫描、map 插入、合并和移除本身不返回错误。

边界行为包括：

- `ForeignKeys` 或 referred 列表为空时正常返回空管理器。
- PK-handle 跳过必须同时满足三项：`PKIsHandle`、恰好一列、能找到且带主键标志；列找不到或标志不符时继续记录。
- referred 摘要能定位子表但找不到同名完整 FK 时静默忽略。当前实现假定 InfoSchema 的摘要与子表元数据一致；若需要把不一致视为损坏，必须显式改变此契约并补错误测试。
- 相同 key 重复插入会覆盖旧值，不会报重复定义错误。
- `RemoveForeignKeys` 的部分索引安全性完全委托给 `IsIndexPrefixCoveredForForeignKey`；`foreign_key_test.rs` 验证谓词落在无关 `marker` 列时不能移除，谓词落在 FK 列时才可移除。
- 当前函数不检查外键启用状态、引用动作或 SQL 执行语义；它按传入元数据记录和过滤。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。所有操作都是调用线程内的同步内存变更。可变操作要求 `&mut self`，Rust 借用规则阻止对同一管理器的无同步并发写；类型没有额外声明线程安全保证。

单表管理器是 `UpdateIndexInfo` 每轮表处理中的短生命周期值，经过索引过滤后立即合并。全局管理器跨表累计，最后移动到 `IngestRecorder.foreignKeyRecordManager`。记录拥有克隆数据，合并同样克隆 key/value；这简化了生命周期，但外键很多时会产生与记录数成正比的分配和复制成本。`retain` 原地过滤，不另建 map。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/ingestrec/foreign_key.go`，主要算法、分支顺序和 key 组成保持一致：两侧都先处理表自身 FK，再处理 referred FK；都跳过单列 PK-handle 特例；都通过子表和 `ChildFKName.O` 还原完整 FK；都分别以 `Cols` 和 `RefCols` 做索引覆盖过滤；全局合并均以后写覆盖同键。

实现层差异如下：

- Go 管理器持有 `map[ForeignKeyRecordKey]*ForeignKeyRecord`，Rust 持有拥有所有权的值并在构造、合并时克隆。
- Go `ForeignKeyRecord` 匿名嵌入 `model.FKInfo`，Rust 使用显式 `FKInfo` 字段。
- Go 构造器返回指针，Rust 返回值；Rust 同时保留 `ForeignKeyRecordManager::New` 和包级 `NewForeignKeyRecordManager` 两个入口。
- Go `TableByName` 接收 `context.Context`，Rust 本地 `InfoSchema` trait 的 `TableByName` 不接收上下文，因此参数命名为 `_ctx` 且当前未使用。
- Go 用 `maps.Copy` 合并、循环 `delete` 过滤；Rust分别用 `HashMap::insert` 和 `retain` 表达相同行为。
- 当前 Rust crate 依赖 `model_stub.rs` 的本地模型边界。测试以手工 `TableInfo`/`MockIS` 复现 Go testkit 建表结果，而 Go 测试使用真实测试 schema 与 InfoSchema。

`foreign_key_test.rs` 对齐 Go 的 `TestForeignKeyRecordManager`、`TestForeignKeyRecordManagerForPK1` 和 `TestForeignKeyRecordManagerForPK2`，并保留普通父子表、单侧/双侧索引移除、部分索引和 PK handle 的断言意图。

## 扩展指南

新增或修改行为时应保持改动聚焦在相应层次：

- 改变记录身份或大小写规则：修改 `ForeignKeyRecordKey` 与 `newForeignKeyRecordKey`，同时检查正向和 referred 路径是否仍生成同键，并在独立的 `foreign_key_test.rs` 增加重复/大小写场景。
- 改变收集规则：修改 `NewForeignKeyRecordManagerForTables`，同步核对 Go `foreign_key.go` 的增量；涉及 InfoSchema 能力时优先扩展 `model_stub.rs` trait 与独立测试夹具，不要把测试写入生产文件。
- 改变索引覆盖判断：优先检查 `model_stub.rs::IsIndexPrefixCoveredForForeignKey` 的契约；本文件只负责为两种方向选择 `Cols` 或 `RefCols`。必须保留 `foreign_key_test.rs` 中 unsafe/safe 部分索引回归，并补复合列、前缀长度或表达式索引边界。
- 改变全局汇总：修改 `ForeignKeyRecordManager::Merge` 时要定义冲突和顺序语义，同时检查 `IngestRecorder::UpdateIndexInfo` 与 `IterateForeignKeys` 的消费者假设。
- 把 `_ctx` 接入真实调用或替换本地 stub：需要同步更新 `InfoSchema` trait、所有实现和调用测试，并评估取消/超时错误如何传播。

兼容性风险主要在 key 大小写、静默忽略不一致摘要、PK-handle 跳过条件和部分索引安全判定；这些变化会直接改变恢复前被删除的外键集合。性能风险集中在按 referred 摘要逐次 `TableByName`、扫描子表全部外键，以及跨表合并的克隆成本。任何优化都应维持集合语义，不应依赖 `HashMap` 顺序。

## 验证依据

事实来源与检查记录：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/ingestrec` 确认目标源、模块入口、Go 对照及独立测试均已索引。
- RustCodeGraph `node --file br/pkg/restore/ingestrec/foreign_key.rs --offset 1 --limit 260`：读取目标文件完整 174 行，并确认 11 个符号及被其他文件使用的文件级关系。
- RustCodeGraph `query`：核对 `NewForeignKeyRecordManager`、`NewForeignKeyRecordManagerForTables`、`RemoveForeignKeys`、`Merge`、`newForeignKeyRecordKey`、`InfoSchema`、`IsIndexPrefixCoveredForForeignKey`、`FindColumnInfo`、`HasPriKeyFlag`、`FKInfo` 与 `ReferredFKInfo` 的定义位置。
- RustCodeGraph `callees NewForeignKeyRecordManagerForTables`：确认 Rust 入口调用同文件 `newForeignKeyRecordKey`。`callers` 未返回跨方法调用边，因此以精确源码检索补证 `ingest_recorder.rs::IngestRecorder::UpdateIndexInfo` 的三条上游调用。
- 已读生产与边界文件：`br/pkg/restore/ingestrec/foreign_key.rs`、`Cargo.toml`、`lib.rs`、`model_stub.rs` 中相关符号、`ingest_recorder.rs`。
- 已读对照与测试：`foreign_key.go`、`foreign_key_test.go`、`foreign_key_test.rs`，并检索 `ingest_recorder_test.rs`、`parity_test.rs`、`export_test.rs` 的直接使用点。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，验证文件存在且恰好包含上述 11 个固定二级标题。
