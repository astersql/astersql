# `lightning/pkg/checkpoints/checkpoints.rs`

源码：[checkpoints.rs](./checkpoints.rs)；crate 入口：[lib.rs](./lib.rs)；Go 对照：[checkpoints.go](./checkpoints.go)。

## 文件定位

本文件是 `astersql-lightning-pkg-checkpoints` crate 的核心实现，crate 根 `lib.rs` 通过 `mod checkpoints; pub use checkpoints::*;` 将其公开符号提升为包级 API。它位于 Lightning 批量导入链路的恢复边界：上层以 `DB` trait 读取任务、表、engine 和 chunk 的恢复坐标，并把导入事件先合并为 `TableCheckpointDiff`，再交给选定后端持久化。

`Cargo.toml` 把该 crate 标为 Go 包 `lightning/pkg/checkpoints` 的 library 移植，直接依赖 checkpoint protobuf crate、`serde` 与 `serde_json`。当前迁移架构还通过同 crate 的 `stubs.rs` 提供 SQL、对象存储、配置、日志、校验和等边界；因此本文件是实际检查点协议实现，但 MySQL/外部存储能力的可用范围仍受这些迁移边界实现约束，不能仅凭接口齐全推断已接入所有 Go 生产依赖。

生产侧可见入口包括 `lightning/pkg/server/lightning.rs` 与 `lightning/pkg/importer/precheck.rs` 对 `OpenCheckpointsDB` 的调用、`lightning/pkg/importer/import.rs` 对 `ChunkCheckpointMerger` 的构造，以及 `lightning/pkg/server/checkpoint_control.rs` 对忽略/销毁错误检查点的管理调用。该文件不是独立进程入口，而是 Lightning 启动、恢复、导入推进和运维控制共同依赖的状态子系统。

## 核心职责

1. 定义持久化协议：`CheckpointStatus` 及其数值阶段、四张 MySQL 表名/SQL 模板、`ChunkCheckpointKey` 的稳定键格式都需要与 Go 保持兼容。
2. 定义统一运行时模型：`TaskCheckpoint`、`TableCheckpoint`、`EngineCheckpoint`、`ChunkCheckpoint` 把不同后端的数据还原成同一层级结构。
3. 聚合增量事件：`TableCheckpointMerger::MergeInto` 及四类 merger 把状态、chunk 位置、校验和和自增基值变化折叠到 `TableCheckpointDiff`，避免每次重写完整快照。
4. 抽象并选择后端：`DB` 规定生命周期、读写、清理、迁移和导出契约；`OpenCheckpointsDB` 根据配置返回 Null、MySQL 或文件实现。
5. 保证恢复语义：`TableCheckpoint::Apply` 只更新已经存在的 engine/chunk，自增基值只增不减，chunk 以“路径 + 原始偏移”定位，防止恢复时重复、遗漏或错配数据。
6. 提供运维操作：删除检查点、备份迁移、查找仍有本地中间数据的 engine、忽略失败状态、销毁失败记录，以及 MySQL 模式的 CSV 导出。

## 主要符号

- `CheckpointStatus = u8` 与 `CheckpointStatus*`：数值范围本身是协议。`Missing=0`，`MaxInvalid=25` 划定失败/无效区间，正常阶段从 `Loaded=30` 依次推进到 `Analyzed=210`；`MetricName` 将细粒度阶段折叠为低基数指标标签。
- `WholeTableEngineID = i32::MAX`：借用 engine 形状表示整表状态更新；普通 engine ID 不应无条件污染表级状态。
- `ChunkCheckpointKey { Path, Offset }`：`String` 生成 `path:offset` 键；`compare`/`less` 提供与 Go 一致的路径优先、原始偏移次优先排序，供文件恢复排序和 `Apply` 的二分定位使用。
- `ChunkCheckpoint`：组合源文件元数据、列映射、当前/真实偏移、行号边界、校验和与时间戳。`UnfinishedSize`、`TotalSize`、`FinishedSize` 对未压缩文件使用逻辑 offset，对压缩文件使用文件大小/真实 offset。
- `EngineCheckpoint` 与 `TableCheckpoint`：分别保存 engine 的阶段/chunk，以及整表阶段、engine map、表结构、校验和和三类自增基值。`DeepCopy` 按 Go 语义故意不复制 `TableInfo`；`CountChunks` 汇总所有 engine 的 chunk 数。
- `TableCheckpointDiff`、`engineCheckpointDiff`、`chunkCheckpointDiff`：增量提交协议；`hasStatus`、`hasRebase`、`hasChecksum` 用来区分“本次未更新”与“写入默认值”。同一 engine/chunk 的后写值覆盖先写值。
- `TableCheckpointMerger`：事件到 diff 的接口。`StatusCheckpointMerger` 处理阶段及 `SetInvalid` 的除十编码；`ChunkCheckpointMerger` 写位置/行号/校验和/列映射；`TableChecksumMerger` 写整表校验和；`RebaseCheckpointMerger` 以最大值合并三类 base。
- `DB: Send`：统一定义 `Initialize`、`TaskCheckpoint`、`Get`、`InsertEngineCheckpoints`、`Update`、`RemoveCheckpoint`、`MoveCheckpoints`、错误管理及导出方法。
- `OpenCheckpointsDB` / `IsCheckpointsDBExists`：后端工厂和存在性探测；未知 driver 返回 `ErrUnknownCheckpointDriver`，MySQL 构造失败时主动关闭已创建的 DB 句柄。
- `NullCheckpointsDB`：禁用 checkpoint 时的空对象。初始化/读取默认态/写进度是无副作用成功，删除、迁移、错误管理和导出明确返回 `errCannotManageNullDB`。
- `MySQLCheckpointsDB`：用 task/table/engine/chunk 四张版本化表和事务表达检查点协议；当前 Rust 实现还维护 `sql::DB` 边界内的 checkpoint 行镜像，`Get` 执行查询形状后返回该镜像。
- `FileCheckpointsDB`：用 `checkpointspb::CheckpointsModel` 保存整份快照，以 `Mutex<()>` 串行保护内存修改和写回；`file_cp_save` 是统一序列化提交点。
- `separateCompletePath` / `createExstorageByCompletePath`：把本地路径或带 scheme URL 拆成存储根与文件名；`uppercasePercentEncoding` 修正 URL 百分号转义大小写以匹配 Go 的外部表示。

## 执行流程

初始化与打开流程如下：

1. `OpenCheckpointsDB(ctx, cfg)` 先检查 `cfg.Checkpoint.Enable`；关闭时返回 `NewNullCheckpointsDB()`。
2. MySQL driver 通过配置参数或 DSN 建立 `sql::DB`，`NewMySQLCheckpointsDB` 依次创建 schema 及四张检查点表；任一步失败都会关闭句柄并传播错误。
3. 文件 driver 经 `NewFileCheckpointsDB`、`createExstorageByCompletePath` 拆解路径并创建 `StorageHandle`；已有文件会读取并反序列化 protobuf，不存在则保留空模型。目录型 DSN 因文件名为空而报错。
4. `DB::Initialize` 写任务头和各目标表的 Loaded 壳。MySQL 后端在事务中写 task/table 行并同步内存镜像；文件后端在锁内补充模型并整份保存，已存在的表不会被覆盖。

导入推进流程如下：

1. 上层为一次事件构造 merger，并调用 `MergeInto(&mut TableCheckpointDiff)`；多个事件可在内存中合并，同键 chunk 只保留最后一次位置。
2. `DB::InsertEngineCheckpoints` 首次登记 engine/chunk。两种真实后端都会保持 chunk 按 `ChunkCheckpointKey` 有序，确保后续 `partition_point` 查找成立。
3. `DB::Update` 持久化 diff。MySQL 在单个事务中按表级 status、rebase/checksum、engine status、chunk 位置的层次写 SQL，然后对镜像调用 `TableCheckpoint::Apply`；文件后端持锁修改 protobuf 模型后调用 `file_cp_save`。
4. 恢复时 `DB::Get` 返回 `TableCheckpoint`。文件后端重建 `TableInfo`、`KVChecksum`、engine/chunk 层级并排序；MySQL 后端保留查询行为，同时从 SQL 边界的行镜像取得统一对象。
5. `TableCheckpoint::Apply` 先应用表状态和单调 rebase，再只修改已存在 engine 和已存在 chunk；diff 中未知的 ID/key 会被忽略，而不会凭空扩展恢复拓扑。

管理流程中，`RemoveCheckpoint("all")` 删除整个 schema/文件，单表删除则清理对应层级；`MoveCheckpoints` 将 MySQL 四张表迁至 `<schema>.<taskID>.bak`，或把文件重命名为 `<file>.<taskID>.bak`；`IgnoreErrorCheckpoint` 只把 `<= MaxInvalid` 的状态重置到 Loaded；`DestroyErrorCheckpoint` 只移除失败表并返回表名及 engine ID 范围摘要。

## 数据与状态

检查点采用四层数据模型：任务级记录导入源、后端、 TiDB/PD 地址与版本；表级记录阶段、表 ID/结构、整表 KV 校验和与 auto-random/auto-increment/row-id 基值；engine 级记录局部阶段；chunk 级记录可恢复的文件读取坐标与累积校验信息。

重要不变量包括：

- 状态值的顺序不可随意调整；失败态通过正常状态除以十落入 `<=25` 区间，错误管理逻辑直接依赖该范围。
- chunk 身份使用原始 `Key.Offset`，当前进度写在 `Chunk.Offset`；二者混用会破坏稳定定位。
- engine 的 `Chunks` 在读取/插入后必须按 key 排序，否则 `TableCheckpoint::Apply` 的 `partition_point` 可能命中错误位置。
- 三类 base 用 `max` 合并和应用，恢复过程中不能回退，否则可能与已导入的 ID 重叠。
- `has*` 标记决定字段是否参与持久化；零值本身不是“没有更新”的可靠表达。
- `GetLocalStoringTables` 只选择表状态位于 `(MaxInvalid, IndexImported)`、engine 状态位于 `(MaxInvalid, Imported)` 且至少一个 chunk 已满足 `pos > offset` 的对象。
- 文件 protobuf 的 chunk map key 是 `ChunkCheckpointKey::String()`；读回后会从模型字段重建强类型 key，而非依赖解析字符串。

## 依赖与调用关系

上游主要分为四类：

- 启动装配：`lightning/pkg/server/lightning.rs` 打开 checkpoint DB，并把 trait object 交给导入流程。
- 预检：`lightning/pkg/importer/precheck.rs` 使用 `OpenCheckpointsDB` 检查既有任务状态。
- 导入推进：`lightning/pkg/importer/import.rs` 持有 `Box<dyn TableCheckpointMerger>`，其中 chunk 完成事件构造 `ChunkCheckpointMerger`；其他阶段通过相应 merger 汇入 diff。
- 运维控制：`lightning/pkg/server/checkpoint_control.rs` 打开 DB，调用 `IgnoreErrorCheckpoint` 或 `DestroyErrorCheckpoint` 管理失败记录。

主要下游依赖为：`checkpointspb` 负责文件模型的 marshal/unmarshal；`common::SQLWithRetry`、`sql::DB/Tx/Stmt` 负责 MySQL 重试与事务形状；`objstore`、`storeapi::StorageHandle` 负责本地/远端文件；`json` 转换 `TableInfo` 和列映射；`verify::KVChecksum` 提供校验聚合；`config::Config` 决定后端及初始化内容；`errors` 和 `common` 提供带语义的错误。

RustCodeGraph 对 `OpenCheckpointsDB` 的 callee 边确认其调用 `NewNullCheckpointsDB`、`NewMySQLCheckpointsDB`、`NewFileCheckpointsDB`、配置连接与失败清理；对 MySQL `Update` 的边确认其调用 `Transact`、`TableCheckpoint::Apply`、校验和读取和镜像替换；对文件 `Update` 的边确认其调用 key 字符化、校验和读取及 `file_cp_save`。调用者检索对 trait 方法存在名称歧义，故上游落点另以仓库 `rg` 精确核验。

## 错误处理与边界

所有后端以 crate 的 `Result<T>` 返回错误，并通过 `errors::Trace`、`NotFoundf` 或领域错误保留上下文。MySQL `TaskCheckpoint` 把 `sql::ErrNoRows` 视为尚未初始化并返回 `Ok(None)`；表/engine/chunk 缺失通常是错误。文件 `TaskCheckpoint` 还把 `TaskId == 0` 当作无任务记录。

边界行为包括：未知 checkpoint driver 报 `ErrUnknownCheckpointDriver`；完整路径为空可拆为默认存储加空文件名，但真正构造文件 DB 时空文件名被拒绝；已有文件反序列化失败目前只生成日志字段并保留模型，而不是把错误返回给调用方，这是继续增强错误可观测性时需谨慎评估的现状。

MySQL 修改以 `SQLWithRetry::Transact` 保持多层写入的事务形状；事务失败不会进入后续镜像更新。文件后端则先修改内存模型再写整份文件，若 `WriteFile` 失败，方法返回错误，但已修改的内存状态仍留在对象中，重试/调用者必须认识这一差异。

`IgnoreErrorCheckpoint` 的单表模式对不存在的表返回 `ErrCheckpointTableNotFound`；成功时仅修复失败区间，不覆盖正常进度。`DestroyErrorCheckpoint` 对正常态目标返回空删除集，对缺失目标报错。Null 后端允许主导入路径继续运行，但所有管理/导出动作都明确失败，防止把“禁用持久化”误认为“存在可管理的数据”。文件后端不支持 CSV 导出，错误提示要求直接复制原检查点文件。

## 并发与资源生命周期

`DB` 要求 `Send`，允许 trait object 跨线程所有权边界传递，但没有要求 `Sync`；绝大多数修改方法接收 `&mut self`，调用方仍需串行化同一实例的可变访问。

`FileCheckpointsDB` 为所有读写和显式 `save` 使用同一把 `std::sync::Mutex<()>`。锁覆盖模型访问及 `file_cp_save`，保证同一实例内不会出现一方读取半更新模型、另一方同时写文件的情况。锁中毒通过 `unwrap()` 触发 panic，而不是业务错误；长时间对象存储 I/O 也发生在锁内，这换取一致性但限制并发吞吐。

MySQL 后端把多语句原子性委托给事务，prepared statement 在使用后显式 `Close`；`OpenCheckpointsDB` 在构造失败时关闭 DB，`DB::Close` 负责正常关闭。`MoveCheckpoints` 成功后清空 MySQL 边界中的 checkpoint 镜像；文件后端重命名当前对象，后续若继续使用同一实例，其内存模型仍存在，调用生命周期应按“迁移后结束当前 checkpoint 会话”理解。

文件后端启动时读取完整文件到内存，更新时序列化并覆盖完整对象，没有后台任务、channel 或异步刷新线程。`Close` 会再次保存当前模型；对象存储句柄生命周期由 `StorageHandle` 所有权管理。

## 与 Go 版本的对应关系

Rust 文件头明确声明移植自 `lightning/pkg/checkpoints/checkpoints.go`，两者的公开结构和方法基本逐项对应：状态常量与表版本名、四层 checkpoint 类型、diff/merger、`DB` 方法集、三种后端、路径拆分、错误管理和 CSV 导出均保留相同命名与总体控制流。

关键语义对齐点包括：`SetInvalid` 除十编码；普通 engine 状态仅写 engine diff，而整表 ID 或失败态会影响表状态；同一 chunk key 后写覆盖前写；rebase 取最大值；`Apply` 忽略未知 engine/chunk；`DeepCopy` 不复制 `TableInfo`；压缩/非压缩文件使用不同 size 计算；URL 百分号转义采用大写十六进制；全量操作以 `common::AllTables`（实际字符串 `"all"`）为哨兵。

实现层面存在迁移形态差异。Go MySQL `Get` 从真实行扫描重建对象，而当前 Rust 版本执行对应查询形状后返回 `sql::DB` 边界维护的 checkpoint 镜像；crate 的外部能力也由 `stubs.rs` 汇入。文件后端仍保留 Go 的 protobuf 整体快照语义，但 Rust 使用 `Mutex` 和拥有所有权的 `HashMap/Vec` 表达内存安全。文档所称“支持”仅指本 crate 当前代码与测试覆盖的行为，不扩张为所有外部生产依赖已经完成迁移。

Go 测试 `checkpoints_test.go`、`checkpoints_file_test.go`、`checkpoints_sql_test.go` 分别对应 Rust 的同名独立测试文件；`parity_test.rs` 进一步核对公共契约、边界错误和资源清理。测试未内嵌在生产源文件，符合本仓库测试分离约束。

## 扩展指南

新增状态阶段时，应同时审查状态数值顺序、`MetricName` 聚合、MySQL/文件序列化字段、错误区间判断，并在 `checkpoints_test.rs` 与 `parity_test.rs` 增加 Go 对齐断言；不要只增加常量。更改 MySQL schema 时必须更新版本化表名、建表/读写/导出 SQL 和 Go 对照，避免旧数据被新代码按错误结构读取。

新增一种导入事件时，优先实现新的 `TableCheckpointMerger`，把事件转换为现有或明确扩展后的 diff，而不要让上层直接依赖具体后端。若扩展 `TableCheckpointDiff`，须同步 `TableCheckpoint::Apply`、MySQL `Update`、文件 `Update`、protobuf 模型与两类后端测试。

更改 chunk 身份或排序时，必须同步 `ChunkCheckpointKey::{String,compare,less}`、MySQL 主键字段、文件 map key、插入后的排序和 `Apply` 查找，并增加乱序、多路径、相同路径不同原始偏移的回归用例。性能优化文件后端时，应保持“锁内模型与成功写回一致”的可观察契约；若改为临时文件原子替换或批量延迟刷新，还需定义写失败后的内存回滚/重试语义。

新增后端需要实现完整 `DB` trait，并接入 `OpenCheckpointsDB`、`IsCheckpointsDBExists`、配置 driver 校验和生命周期清理。建议复用 `checkpoints_file_test.rs`/`checkpoints_sql_test.rs` 的共同场景：初始化与 round trip、AddIndexBySQL、更新、单表/全量删除、忽略与销毁错误、迁移、导出能力及缺失对象错误。

任何代码修改都应继续把 Rust 生产实现放在本文件、测试放在独立测试文件；修复真实 Rust 逻辑后保留顶部 `// Copyright 2026 AsterSQL.` 和 PingCAP Apache License。由于本说明只分析现状，本任务不修改 Rust、Go、Cargo 或 protobuf。

## 验证依据

- RustCodeGraph：`status` 确认本地索引可用；`files --filter lightning/pkg/checkpoints` 确认目标与相关测试已索引；`node --file lightning/pkg/checkpoints/checkpoints.rs` 分段核对全部 2300 行；`node --file lightning/pkg/checkpoints/checkpoints_test.rs` 核对核心 merger/apply 测试；`callees OpenCheckpointsDB` 与 `callees Update` 核对后端构造、事务、apply 和文件保存边。
- crate/模块证据：`lightning/pkg/checkpoints/Cargo.toml`、`lightning/pkg/checkpoints/lib.rs`、`lightning/pkg/checkpoints/stubs.rs`。
- Go 对照：`lightning/pkg/checkpoints/checkpoints.go`；相关 Go 测试为 `checkpoints_test.go`、`checkpoints_file_test.go`、`checkpoints_sql_test.go`。
- Rust 独立测试：`checkpoints_test.rs` 覆盖状态/失败状态/chunk/rebase 合并、diff 应用、序列化与路径拆分；`checkpoints_file_test.rs` 覆盖文件后端读取、删除、忽略/销毁错误；`checkpoints_sql_test.rs` 覆盖 MySQL round trip、更新、管理、导出与迁移；`parity_test.rs` 覆盖 Go/Rust 公共契约、边界、错误和资源清理。
- 生产调用证据：`lightning/pkg/server/lightning.rs`、`lightning/pkg/importer/precheck.rs`、`lightning/pkg/importer/import.rs`、`lightning/pkg/server/checkpoint_control.rs` 的精确符号搜索。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；结构验证使用任务文件给定命令，结果在交付时报告。
