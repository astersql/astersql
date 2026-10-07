# `lightning/pkg/checkpoints/stubs.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-checkpoints` crate 的迁移边界桩，而不是检查点业务实现。crate 入口 [`lib.rs`](lib.rs) 先以 `mod stubs` / `pub use stubs::*` 导出这里的兼容类型，再导出 [`checkpoints.rs`](checkpoints.rs) 中的真实检查点模型、合并逻辑和 `CheckpointsDB` 后端。因此，业务流程仍由 `checkpoints.rs` 驱动，本文件只把 Go 版本依赖的 MySQL、配置、对象存储、日志、JSON、mydump 等外围接口压缩为可编译、可测试的本地替身。

[`Cargo.toml`](Cargo.toml) 将该 crate 标为 `lightning/pkg/checkpoints` 的 Rust library 移植，直接依赖只有已移植的 `checkpointspb`、`serde` 和 `serde_json`。这与文件头的约束一致：当前实现刻意不引入 `kv`、`domain`、`kvproto`、`grpcio`，也不提供真实 MySQL 或云对象存储连接。源文件没有条件编译项；测试由 `lib.rs` 中独立的 `*_test.rs` 模块装配，没有把测试内嵌进本文件。

## 核心职责

本文件按职责可分为四组：

1. 兼容形状：`Error`/`Result`、`context`、`build`、`config`、`model`、`importdef`、`mydump`、`verify` 保留 Go 调用点需要的类型、字段和常量。
2. 行为适配：`common` 提供标识符转义、Go 风格格式替换、标准化错误和单次执行的 `SQLWithRetry`；`json` 用 Serde 完成序列化。
3. 有状态测试替身：`sql::DB` 用共享内存保存 task/table/engine/chunk 检查点及 SQL 执行痕迹；`storeapi` 提供内存和本地文件存储。
4. 边界工具：`objstore` 解析 URL/后端并创建存储句柄，`gopath` 模拟 Go `path` 的斜杠语义，`log`/`logutil`/`zap` 和 `sqltocsv` 只维持接口连通性。

这些桩的完成标准是支撑当前 checkpoint 契约测试，而不是覆盖相应 Go 库的全部能力。尤其 `sql::QueryContext`、`Rows`、远端 `objstore::Backend` 和 `sqltocsv::Write` 都是窄实现，不能据此宣称真实数据库查询、云访问或 CSV 导出已接线。

## 主要符号

- `Error`、`Result<T>` 与 `errors`：错误保存 `msg`、`not_found`、`no_rows` 和可选 `class`；`errors_NotFoundf` 与 `NormalizedError::GenWithStackByArgs` 为调用方保留可观察的 not-found 分类。
- `context::Context` / `Background()`：零大小占位令牌，不实现取消、deadline 或值传播。
- `config::{Config, Checkpoint, Mydumper, TikvImporter, TiDB}`：承载 `OpenCheckpointsDB` 和 `Initialize` 实际读取的最小字段；`CheckpointDriverMySQL`、`CheckpointDriverFile` 是驱动分支值。
- `common::{EscapeIdentifier, UniqueTable, SprintfWithIdentifiers}`：按反引号规则转义 schema/table 名，并处理 `%s`、`%%`、`%[n]s`；生成的 SQL 文本是 Go/Rust 对齐契约的一部分。
- `common::SQLWithRetry`：把 `Exec`、`Transact`、`QueryRow` 转发给 `sql::DB`；`Retry` 当前只调用闭包一次，不具备真实重试策略。
- `mydump::{SourceFileMeta, Chunk}` 与 `verify::KVChecksum`：描述恢复文件、偏移/行号边界及 bytes/KV/checksum 三元组，是 checkpoint 持久化数据形状。
- `sql::{DB, Tx, Stmt, Rows, Row, SqlValue}`：`DB` 通过 `Arc<Mutex<DbInner>>` 共享状态；`new_memory`、`QueryRowTask`、`put_checkpoint`/`replace_checkpoint`、错误 checkpoint 管理和三个 CSV dump helper 是主要有状态入口。
- `storeapi::Storage`：定义 `FileExists`、`ReadFile`、`WriteFile`、`DeleteFile`、`Rename`；`MemoryStorage` 和 `LocalStorage` 是两个实现，`StorageHandle` 以 `Arc<dyn Storage>` 做类型擦除。
- `objstore::{RawURL, Backend, ParseRawURL, ParseBackend, New}`：负责 URL 文本、后端分类及存储句柄创建。远端后端目前映射为进程内存存储。
- `gopath::{Base, Dir}`：遵循 Go `path` 而非宿主 OS `filepath` 语义。
- `log::Logger`、`zap::Field`、`sqltocsv::Write`：无日志副作用或极简写出行为的接口占位器。

## 执行流程

MySQL 检查点路径从 [`checkpoints.rs`](checkpoints.rs) 的 `NewMySQLCheckpointsDB`、`Initialize`、`TaskCheckpoint` 等入口进入。主实现用 `common::SprintfWithIdentifiers` 生成建库建表和读写 SQL，再由 `SQLWithRetry` 调用 `sql::DB`。`DB::Exec` 只识别当前测试需要的少量 DDL 并记录文本；`Tx::PrepareContext` 创建 `Stmt`；`Stmt::ExecContext` 把 task 数据写入共享状态。表级检查点则由主实现通过 `put_checkpoint`、`replace_checkpoint` 等窄接口维护。`QueryRowTask` 在有 task 时复制结果，无结果时返回带 `no_rows` 标志的错误，随后 `checkpoints.rs::TaskCheckpoint` 用 `sql::is_no_rows` 转为 `Ok(None)`。

错误 checkpoint 管理同样由主实现发起：`MySQLCheckpointsDB::IgnoreErrorCheckpoint` 调用 `DB::ignore_error_checkpoint`，把无效 table/engine 状态复位到 `CheckpointStatusLoaded`；`DestroyErrorCheckpoint` 调用 `destroy_error_checkpoints`，仅删除无效表并返回 engine ID 范围；`DumpTables`、`DumpEngines`、`DumpChunks` 使用稳定排序后的 CSV 字符串写入调用方 writer。

文件检查点路径由 `separateCompletePath` 调用 `objstore::ParseRawURL` 与 `gopath::{Base, Dir}` 拆出文件名和目录，再由 `objstore::ParseBackend` / `New` 取得 `StorageHandle`。[`checkpoints.rs`](checkpoints.rs) 的 `FileCheckpointsDB` 使用该句柄读取、写入、重命名或删除 protobuf 快照；内存存储服务无环境测试，本地存储产生真实文件系统副作用，远端 scheme 当前仍退化为内存存储。

## 数据与状态

`sql::DbInner` 是 SQL 桩的共享状态核心：`closed` 记录关闭动作，`schemas` 记录少量 DDL 结果，`task` 保存任务级扫描值，`checkpoints` 保存完整 `crate::TableCheckpoint`，`exec_log` 保存执行文本。另有 `tables`、`engines`、`chunks` 兼容行级形状，但当前读取主路径主要依赖 `checkpoints` map；不能把这些字段的存在等同于通用 SQL 表实现。

`DB` 克隆只克隆 `Arc`，多个句柄观察同一 `Mutex<DbInner>`。`MemoryStorage` 同理以 `Arc<Mutex<HashMap<String, Vec<u8>>>>` 共享文件字节；`StorageHandle` 再以 `Arc<dyn Storage>` 隐藏具体介质。`LocalStorage` 只保存根目录，并在每次操作时解析相对/绝对路径。`RawURL` 保存 scheme、host、path、query 和原始文本；`Backend` 只区分 `Local`、`Remote`、`Noop`。

配置、模型、mydump 和 checksum 类型多为值对象。重要不变量包括：表名通过 `UniqueTable` 形成两个已转义标识符；`KVChecksum` 的三个统计量独立保存；chunk 的 `Offset`/`RealOffset`/`EndOffset` 和行号边界不能互换；CSV dump 在输出前排序，以避免 `HashMap` 遍历顺序造成非确定结果。

## 依赖与调用关系

上游直接调用者是同 crate 的 [`checkpoints.rs`](checkpoints.rs)，crate 外调用者通常通过 [`lib.rs`](lib.rs) 的重导出使用这些类型。已核对的关键边包括：

- `NewMySQLCheckpointsDB` / `MySQLCheckpointsDB::*` → `common::SQLWithRetry`、`sql::DB`、`json`、`verify`、`logutil`/`zap`。
- `TaskCheckpoint` → `SQLWithRetry::QueryRow` → `DB::QueryRowTask`；无记录错误再由 `sql::is_no_rows` 分类。
- MySQL 管理与 dump 方法 → `DB::{ignore_error_checkpoint, destroy_error_checkpoints, dump_*_csv}`。
- `newExternalCheckpointStorage` / `separateCompletePath` → `objstore::{ParseRawURL, ParseBackend, New}`、`gopath::{Base, Dir}` → `StorageHandle`。
- `FileCheckpointsDB` → `StorageHandle::{FileExists, ReadFile, WriteFile, DeleteFile, Rename}` → `MemoryStorage` 或 `LocalStorage`。

下游外部依赖仅为标准库、Serde/Serde JSON 和同 crate 的 `TableCheckpoint`/状态常量；checkpoint protobuf 由真实 `checkpoints.rs` 使用。RustCodeGraph 将本文件识别为 211 个符号，精确查询确认 `ParseBackend` 位于 1641 行、`SprintfWithIdentifiers` 位于 259 行；图的 `callers/callees` 命令本次未在时限内返回，因此上述调用边又用 crate 内精确引用搜索和源码核对确认，而不是采用自然语言查询产生的跨仓库同名结果。

## 错误处理与边界

本文件统一返回自定义 `Error`。JSON 和本地文件 I/O 把底层错误转为文本；空 JSON 特判为 `unexpected end of JSON input`。`MemoryStorage::ReadFile` 对缺失键报 `file not found`，而删除缺失键和内存 rename 缺失源目前静默成功。`LocalStorage::WriteFile`/`Rename` 会尝试创建父目录，但创建目录的错误被忽略，后续文件操作才决定是否返回失败。

SQL 边界尤其有限：`DB::Exec` 对未识别语句通常返回成功，`Tx::Commit` 没有回滚/隔离语义，`QueryContext` 返回空 `Rows`，`Rows::Next` 恒为 `false`，`Row::Scan` 恒返回 no-rows。`SQLWithRetry::Retry` 不重试，`context` 不取消。锁中毒处统一 `unwrap`，会 panic 而不是转换为 `Error`。

`objstore::ParseBackend` 拒绝空字符串与未知 scheme；`s3`、`gcs`、`hdfs` 等虽然能被分类，但 `New` 对它们只创建独立内存存储，没有认证、网络、持久化或跨句柄全局共享保证。`sqltocsv::Write` 只写一个换行，不能验证真实 CSV 格式。这些限制都是扩展时必须显式处理的边界。

## 并发与资源生命周期

并发共享集中在两个 `Arc<Mutex<...>>`：SQL 状态和内存文件 map。每个公开操作在短临界区内加锁，当前没有后台任务、channel、异步 runtime 或显式锁顺序，因此没有跨锁事务保证。`Storage` 要求 `Send + Sync`，使 `StorageHandle` 可跨线程共享；但业务级原子性仍由 [`checkpoints.rs`](checkpoints.rs) 的 `FileCheckpointsDB::lock` 保护，本文件的单次文件操作本身不能组成多步原子事务。

`DB::Close` 仅把 `closed` 置为 `true`，后续方法没有检查该标志；`Stmt::Close`、`Rows::Close` 是空操作。`LocalStorage` 的文件由每次方法调用即时打开/关闭。`objstore::New` 创建本地根目录；内存/远端替身的内容随最后一个共享句柄释放而消失。独立测试 [`parity_test.rs`](parity_test.rs) 验证文件 checkpoint 的 `Close` 会保留已刷盘文件，而 `RemoveCheckpoint("all")` 会删除文件；这部分生命周期行为由真实 `FileCheckpointsDB` 调度、由本文件的存储接口兑现。

## 与 Go 版本的对应关系

Go 对照文件 [`checkpoints.go`](checkpoints.go) 直接依赖 `context`、`database/sql`、`encoding/json`、`sqltocsv`、PingCAP errors/build/common/config/importdef/log/mydump/verification/model、objstore/storeapi、logutil 和 zap。本文件按这些包边界建立同名或近似同名模块，使 [`checkpoints.rs`](checkpoints.rs) 能尽量保留 Go 的调用结构和字段映射。

保留的关键语义包括：反引号标识符、`all` 保留表名、task/checkpoint 数据形状、no-rows 与 not-found 分流、Go `path` 的 slash 规则、URL 中 `+` 的处理、校验和三元组以及文件存储的读写/改名/删除接口。独立 Rust 测试 [`checkpoints_sql_test.rs`](checkpoints_sql_test.rs)、[`checkpoints_file_test.rs`](checkpoints_file_test.rs)、[`checkpoints_test.rs`](checkpoints_test.rs) 和 [`parity_test.rs`](parity_test.rs) 分别对应同目录 Go 测试并覆盖这些契约。

差异也必须保留在认知中：Go MySQL 后端由 `database/sql` 与 sqlmock/真实 driver 表达，Rust 是定制内存状态机；Go 对象存储可接真实后端，Rust 远端 scheme 只进内存；Go context、重试、日志、CSV 库具有完整语义，Rust 这里只保留接口形状。测试中的 `test_normal_operations` 也明确接受内存桩不返回通用 chunk 查询行这一已知边界。

## 扩展指南

扩展前先确定需求来自哪条真实 `checkpoints.rs` 调用路径，避免把本文件演变成通用 SQL/对象存储框架。

- 增加 MySQL checkpoint 行为时，优先修改 `sql::DB` 的明确状态转换和相应 `Tx`/`Stmt` 入口；同步更新独立 [`checkpoints_sql_test.rs`](checkpoints_sql_test.rs)，并与 [`checkpoints_sql_test.go`](checkpoints_sql_test.go) 的断言意图对齐。若需要通用查询，不能继续依赖 `Rows::Next == false` 的占位实现。
- 增加文件/URL 行为时，修改 `storeapi::{Storage, MemoryStorage, LocalStorage}` 或 `objstore`/`gopath`；同步更新 [`checkpoints_file_test.rs`](checkpoints_file_test.rs) 与 [`parity_test.rs`](parity_test.rs)，覆盖本地路径、对象 URL、缺失文件、rename/delete 和 round-trip。
- 增加错误分支时，保护 `not_found`、`no_rows`、`class` 及稳定文本片段；不要把所有错误简化成无分类字符串。
- 替换真实依赖时，保持 `lib.rs` 对外契约和 Go 对照语义，逐步删除已无调用的桩，而不是同时维护两套含糊实现。真实远端存储还需考虑认证、持久化、并发一致性和错误映射；真实 SQL 还需考虑事务回滚、隔离、连接关闭与重试幂等性。
- Rust 测试继续放在独立 `*_test.rs` 文件，不在 `stubs.rs` 内加入 `#[cfg(test)]` 测试模块。

兼容性风险主要是 Go 格式化/path/URL/错误文本细节漂移；正确性风险集中在窄 SQL 状态机遗漏新调用；性能目前不是主要目标，但把大 checkpoint clone 或 CSV 拼接用于生产规模前需要重新评估内存占用与锁持有时间。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件（Rust 7,032），`files --filter lightning/pkg/checkpoints` 确认目标及四个独立 Rust 测试均已索引；目标 `stubs.rs` 有 211 个符号。通过 `node --file` 分段核对全文，并用 `query` 精确定位 `ParseBackend`、`StorageHandle`、`SprintfWithIdentifiers`。`callers/callees` 两次在 30 秒内未返回输出，故调用边改由下述本地引用证据验证。
- Rust 源码：[`stubs.rs`](stubs.rs)、[`lib.rs`](lib.rs)、[`checkpoints.rs`](checkpoints.rs)；精确引用搜索核对了 `SprintfWithIdentifiers`、`QueryRowTask`、错误 checkpoint 管理、CSV dump、`StorageHandle`、`ParseBackend` 和 `ParseRawURL` 的调用位置。
- crate 边界：[`Cargo.toml`](Cargo.toml)，确认 library 路径、Go package 元数据和三个直接依赖。
- Go 对照：[`checkpoints.go`](checkpoints.go)、[`checkpoints_sql_test.go`](checkpoints_sql_test.go)、[`checkpoints_file_test.go`](checkpoints_file_test.go)、[`checkpoints_test.go`](checkpoints_test.go)。
- Rust 独立测试：[`checkpoints_sql_test.rs`](checkpoints_sql_test.rs) 覆盖内存 SQL 初始化、task/表状态、错误管理和关闭；[`checkpoints_file_test.rs`](checkpoints_file_test.rs) 覆盖文件后端；[`checkpoints_test.rs`](checkpoints_test.rs) 覆盖公共模型并最小引用 `StorageHandle`；[`parity_test.rs`](parity_test.rs) 覆盖 round-trip、路径拆分、错误文本与资源清理。
- 本任务是只读分析加 Markdown 文档，不运行 Cargo；结构检查要求本文恰好具有上述十一个固定二级标题。
