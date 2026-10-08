# `pkg/ttl/cache/task.rs`

## 文件定位

`task.rs` 属于 `astersql-ttl-cache` crate，由 [`pkg/ttl/cache/lib.rs`](lib.rs) 以 `pub mod task` 公开。它描述 `mysql.tidb_ttl_task` 系统表中的扫描任务，提供 SQL 文本与绑定参数构造、扫描范围 Datum 的键编码/解码、任务状态模型以及系统表行到 `TTLTask` 的映射。

crate 的移植元数据把它归入 Go 包 `pkg/ttl/cache`（[`pkg/ttl/cache/Cargo.toml`](Cargo.toml) 的 `package.metadata.porting`）。当前 Rust 生产链没有整体调用这里的 SQL/行映射门面：精确引用显示 [`pkg/ttl/ttlworker/persistent.rs`](../ttlworker/persistent.rs) 使用 `EncodeDatums` 持久化扫描范围，[`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 使用 `DecodeDatums` 恢复扫描范围；其余公开函数和数据模型目前由同目录独立测试覆盖。因而本文件既是 Go API 的语义移植面，也是已接入生产路径的范围编解码组件。

## 核心职责

1. `selectFromTTLTask` 与 `insertIntoTTLTask` 固定系统表列顺序和 TiDB `%?` 占位符形式；`SelectFromTTLTaskWithJobID`、`SelectFromTTLTaskWithID`、`PeekWaitingTTLTask` 在基础查询上增加筛选条件和参数。
2. `EncodeDatums` / `DecodeDatums` 在 `Datum` 与可比较的 TiDB key-codec 字节之间转换，供 `scan_range_start`、`scan_range_end` 持久化和恢复。
3. `InsertIntoTTLTask` / `InsertIntoTTLTaskWithScanIndexID` 组合插入 SQL 与八个绑定值，并把扫描范围编码错误直接返回。
4. `TaskStatus`、`TTLTaskState`、`TTLTask` 表达任务状态、进度和系统表完整行；`RowToTTLTask` 按 SELECT 的十四列顺序完成解码。
5. `JsonCursor` 与 `parse_state` 在不引入 JSON crate 的情况下复现本任务状态对象所需的 `encoding/json` 行为：识别已知字段、跳过未知字段、处理转义并拒绝畸形输入。

## 主要符号

- `Datum`：本 crate 的轻量单元格/键值类型，变体为 `Null`、有符号/无符号整数、浮点、字节串、字符串和以 `i64` 表示的时间。`Row` 是 `Vec<Datum>`；二者并非仓库通用 SQL Datum 的类型别名。
- `selectFromTTLTask`：十四列 SELECT 的唯一列序定义。`RowToTTLTask` 的下标必须与此常量保持一致。
- `SelectFromTTLTaskWithJobID(job_id)`：查询一个 job 的全部扫描任务。
- `SelectFromTTLTaskWithID(job_id, scan_id)`：查询 job 下指定 scan task。
- `PeekWaitingTTLTask(heartbeat_expire)`：选择 `waiting`，或 owner 心跳早于阈值且仍为 `running` 的任务，并按 `created_time ASC` 排序。名称/Go 注释提到 limit，但当前 Rust 与 Go SQL 都没有 `LIMIT` 参数。
- `EncodeDatums(datums)`：写入与 Go `pkg/util/codec.EncodeKey` 对应的标签及有序字节编码。空切片得到空字节串。
- `DecodeDatums(input)`：逆向读取支持的编码；字符串和字节在 tag 1 下统一恢复成 `Datum::Bytes`。
- `InsertIntoTTLTask(...)`：不带扫描索引的便利入口，转发给 `InsertIntoTTLTaskWithScanIndexID(..., None)`。
- `InsertIntoTTLTaskWithScanIndexID(...)`：输出插入 SQL 和严格按列排序的八个参数；`scan_index_id` 缺失时绑定 `Datum::Null`。
- `TaskStatus`：已知状态为 `Waiting`、`Running`、`Finished`，未知字符串由 `Other(String)` 无损保留；默认值为 `Other("")`，对应 Go 字符串别名的零值。
- `TTLTaskState`：记录总行数、成功/失败行数、扫描错误和前任 owner。
- `TTLTask`：十四个系统表字段的 Rust 表示。扫描范围总是 `Vec<Datum>`，可空 state/index 用 `Option`。
- `JsonCursor` / `parse_state`：文件私有的 JSON 游标和状态对象解析器。
- `RowToTTLTask(row)`：公开行映射入口；要求至少十四列，并传播范围或 state 的解码错误。

## 执行流程

插入流程从 `InsertIntoTTLTask` 开始。它把 `scan_index_id` 设为 `None` 后调用带索引版本；后者依次将 job/table/scan 标识、`EncodeDatums(start)`、`EncodeDatums(end)`、过期时间、创建时间和可选索引装入绑定参数。任何一个扫描范围编码失败都会通过 `?` 中止，不返回半成品 SQL 参数。

任务领取查询由 `PeekWaitingTTLTask` 生成：数据库端选择尚未开始的任务，也允许重新领取心跳过期的 running 任务，并以创建时间保证先旧后新。函数只构造 SQL/参数，不执行 SQL、加锁或修改 owner；领取原子性属于上层 task manager 的职责。

行映射从 `RowToTTLTask` 开始：

1. 先检查 `row.len() >= 14`，避免后续字段悄然以零值掩盖缺列。
2. 状态列为非 NULL 空字符串/空字节时回退到 `Waiting`；三个已知字符串映射到枚举，未知字符串保留为 `Other`，NULL/类型不符保持字符串零值。
3. 起止范围仅在对应列为非空 `Datum::Bytes` 时调用 `DecodeDatums`；NULL、类型不符或空字节得到空向量。
4. state 仅接受 `Datum::String`，由 `parse_state` 解析；未知 JSON 字段跳过，已知字段类型不符或 JSON 畸形时报错。
5. `scan_index_id` 接受 `Int`，也接受能安全转为 `i64` 的 `UInt`；其他输入得到 `None`。

键解码流程按首字节 tag 分派。数值 tag 读取固定八字节；bytes tag 逐个读取八字节分组及 marker，检查尾部 padding；未知 tag、截断数据或非法 padding 均立即返回错误。

## 数据与状态

`TTLTask` 是一次查询所得的值对象，本文件没有全局可变缓存。`JobID`、`TableID`、`ScanID` 标识任务；`ScanRangeStart`/`ScanRangeEnd` 是分片边界；`ExpireTime` 决定删除截止时间；owner 字段和心跳/状态更新时间描述租约；`State` 保存进度摘要；`CreatedTime` 用于调度排序；`ScanIndexID` 指示任务采用的扫描索引。

编码不变量如下：有符号整数写 tag 3 并翻转符号位；无符号整数和 `Time` 写 tag 4；浮点写 tag 5，负数取反、非负数翻转符号位以保持字节序比较；字符串和字节写 tag 1，并采用八字节分组、零填充和 marker。解码无法区分原始 `String` 与 `Bytes`，两者都成为 `Bytes`；tag 4 也统一成为 `UInt`，所以该格式保证键值语义和顺序，不保证 Rust 枚举变体逐项往返。

状态 JSON 的缺失字段依赖 `TTLTaskState::default()`，计数为 0、字符串为空。`prev_owner` 使用 Go JSON tag，而 Rust 字段名仍为 `PreviousOwner`。解析器忽略未知字段，允许对象、数组、字符串、布尔、null 和可由 `f64` 接受的数值作为未知值。

## 依赖与调用关系

内部调用关系为：

- `InsertIntoTTLTask` → `InsertIntoTTLTaskWithScanIndexID` → `EncodeDatums`。
- `RowToTTLTask` → `string` / `int` / `bytes`，并按列调用 `DecodeDatums`、`parse_state`。
- `parse_state` → `JsonCursor::{byte,string,unsigned,skip_value,whitespace}`。
- 所有查询构造函数引用 `selectFromTTLTask`；插入构造函数引用 `insertIntoTTLTask`。

当前 Rust 生产上游的精确引用只有范围 codec：`PersistentJobStore` 在 [`pkg/ttl/ttlworker/persistent.rs`](../ttlworker/persistent.rs) 创建任务行前调用 `EncodeDatums`；`persisted_scan_ranges` 在 [`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 读取任务后调用 `DecodeDatums` 并转换为 worker session 的 Datum。SQL 构造函数、`RowToTTLTask` 和任务结构在 Rust 侧尚未被生产代码引用；[`pkg/ttl/cache/task_test.rs`](task_test.rs) 是它们当前的直接 Rust 消费者。

`Cargo.toml` 将该 crate 声明为 workspace 成员式本地 library，入口为 `lib.rs`。文件本身只依赖 `std`；manifest 中较重的 AsterSQL 依赖均位于 Windows 条件段，不能据此声称本文件在所有平台直接调用这些 crates。

## 错误处理与边界

所有可失败入口使用 `Result<_, String>`，没有自定义错误类型。`EncodeDatums` 当前覆盖所有 `Datum` 变体，接口保留错误通道主要用于让插入流程稳定传播未来编码失败；现有实现没有可达的 `Err` 分支。

`DecodeDatums` 明确拒绝短数值、短 bytes 分组、padding 大于 8、padding 区含非零字节和未知 tag。内部固定八字节转换仅在长度检查后 `unwrap()`，不会由截断输入触发 panic。

JSON 解析拒绝缺少引号/冒号/逗号/右括号、非法或短 Unicode 转义、控制字符、无符号计数字段的负数/小数、无效 UTF-8，以及根对象后的尾随内容。`\uXXXX` 当前逐个码点处理，不组合 UTF-16 surrogate pair；这与完整的 Go `encoding/json` 能力不是完全等价，扩展 Unicode 行为时需新增对照测试。

`RowToTTLTask` 对缺列严格报错，但多数列类型不符会由 `string`/`int`/`bytes` 降为零值或空值。这是当前事实，不应把它描述成完整 schema 类型校验。时间在 Rust 中只是 `i64`，没有 Go 版本的时区转换和 `time.Time` 合法性检查。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、数据库连接或事务。所有函数只读取借用的输入并返回拥有所有权的 `String`、`Vec` 或结构体，因此单次调用没有共享可变状态，天然可并发调用。

数据库任务的 owner 竞争、心跳续约、领取/重领、事务隔离和完成状态推进都不在这里实现。`PeekWaitingTTLTask` 仅表达候选条件；上层必须负责以事务或条件更新避免多个 worker 同时取得同一任务。生产写入和读取分别发生在 TTL worker/session 组件中，SQL 执行资源的开启与释放也由它们管理。

字节/JSON 解析均按输入分配新容器。`JsonCursor` 只借用 JSON 字节并维护当前位置，解析结束即释放；`RowToTTLTask` 克隆需要保留的字符串和字节内容，不持有原始 row 的引用。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/ttl/cache/task.go`](task.go)，独立测试是 [`pkg/ttl/cache/task_test.go`](task_test.go) 与 [`pkg/ttl/cache/task_test.rs`](task_test.rs)。SQL 常量、查询条件、插入字段顺序、三种已知状态、任务/state 字段以及空 status 回退语义保持一致。

主要表示差异：Go 使用 `types.Datum`、`chunk.Row`、`time.Time`、`codec.EncodeKey/Decode` 和 `encoding/json`；Rust 使用本地 `Datum`/`Row`、`i64` 时间、手写 key codec 和手写 JSON 子集。Go 的 `TaskStatus` 是字符串别名，Rust 以 `Other(String)` 保留未知值以维持兼容。Go 的 NULL 扫描范围为 nil slice，Rust 归一化为空 `Vec`。

接线差异更重要：Go 的 `job_manager.go` 使用 job 查询、插入和 `RowToTTLTask`，`task_manager.go` 使用 peek、按 ID 查询和行映射；Rust 对应生产路径目前自行执行 SQL，只复用这里的范围 codec。因此文档不能宣称整个 Rust TTL manager 经由 `TTLTask` API 运行。未来若统一接线，应同时核对现有 runtime/persistent SQL 的时间转换、Datum 类型和 state 中额外字段，不能机械替换。

测试语义方面，Rust 测试覆盖 Go key codec 的整数/bytes 代表性字节、空范围、扫描索引、状态默认/未知值、JSON 转义/畸形输入、短行和 SQL 形态；Go 测试还通过真实内部 SQL 执行检查系统表 round trip。Rust 单测是独立文件，符合源码与测试分离约束。

## 扩展指南

- 修改系统表 SELECT 列时，必须同步 `selectFromTTLTask`、`TTLTask`、`RowToTTLTask` 的列下标与 [`pkg/ttl/cache/task_test.rs`](task_test.rs) 的 `task_row`；列顺序错位可能静默产生零值。
- 新增 Datum 类型或 codec tag 时，应成对修改 `EncodeDatums`/`DecodeDatums`，补充 Go `codec.EncodeKey/Decode` 的字节级对照、边界数值、空/八字节整块、截断和非法 marker 测试，并检查 `ttlworker/persistent.rs` 与 `session/runtime/ttl_runtime.rs` 的跨类型转换。
- 扩展 `TTLTaskState` 时，应修改结构、`parse_state` 的 key 分支及 Rust 独立测试，并与 Go JSON tag 和默认值保持一致。若需求超出当前手写 JSON 子集，优先评估统一到仓库既有 JSON 依赖，避免继续扩张不完整解析器。
- 新增任务状态时，修改 `TaskStatus::{as_str,from_string}` 并保留 `Other` 的前向兼容；同步测试 NULL、空串、已知值和未知值。
- 将 SQL/行映射 API 接入 Rust 生产 task manager 前，先比较现有直接 SQL 的事务与并发语义，特别是时间的 `FROM_UNIXTIME`/时区处理、任务重领条件和 state/cursor 字段；这不是单纯替换字符串。
- 性能风险主要在每个扫描范围的重复分配和 JSON 手写解析；兼容风险主要在 Go codec 字节格式、系统表列顺序、未知状态保留和 NULL/空值区别。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/ttl/cache/task.rs`；`node --file pkg/ttl/cache/task.rs` 核对了 544 行源码；对 `RowToTTLTask`、插入/查询函数、`EncodeDatums`、`DecodeDatums`、`parse_state` 执行了 `query`、`callers`、`callees`。图确认内部边为 `InsertIntoTTLTask → InsertIntoTTLTaskWithScanIndexID`、`RowToTTLTask → DecodeDatums/parse_state`，未发现这些 SQL/映射 API 的生产调用者。
- 源码与 crate 边界：[`pkg/ttl/cache/task.rs`](task.rs)、[`pkg/ttl/cache/lib.rs`](lib.rs)、[`pkg/ttl/cache/Cargo.toml`](Cargo.toml)。
- Rust 生产调用证据：[`pkg/ttl/ttlworker/persistent.rs`](../ttlworker/persistent.rs) 对 `EncodeDatums` 的引用；[`pkg/session/runtime/ttl_runtime.rs`](../../session/runtime/ttl_runtime.rs) 对 `DecodeDatums` 的引用。对 `pkg/**/*.rs` 的精确符号检索用于确认其他 API 只出现在定义和测试中。
- Go 对照与调用证据：[`pkg/ttl/cache/task.go`](task.go)、[`pkg/ttl/ttlworker/job_manager.go`](../ttlworker/job_manager.go)、[`pkg/ttl/ttlworker/task_manager.go`](../ttlworker/task_manager.go)。
- 测试证据：[`pkg/ttl/cache/task_test.rs`](task_test.rs) 和 [`pkg/ttl/cache/task_test.go`](task_test.go)。本任务只新增文档，按计划未运行 Cargo 或代码测试；交付验证以固定章节结构、路径/链接和事实复核为准。
