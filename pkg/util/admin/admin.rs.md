# `pkg/util/admin/admin.rs`

## 文件定位

本文件是 `astersql-util-admin` crate 的业务实现，crate 入口 `pkg/util/admin/lib.rs` 通过 `mod admin; pub use admin::*;` 将其 API 全量导出。`pkg/util/admin/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/util/admin`，运行时依赖只有 `thiserror`；数据库会话、KV、表和索引能力均被压缩成文件内 trait，由调用方注入。

它实现两类管理校验：`CheckIndicesCount` 在同一快照上比较表与索引的 `COUNT(*)`；`CheckRecordAndIndex` 扫描记录 KV、解码索引列并逐条探测索引。工作区的 `pkg/executor/Cargo.toml` 已声明对 `astersql-util-admin` 的依赖，但当前 Rust `pkg/executor/check_table_index.rs` 使用独立的 `CheckTableRuntime::{CheckIndicesCount, CheckRecordAndIndex}` 边界，仓库搜索未发现把本文件函数接入该 trait 的实现。因此，本文件当前有 crate 内实现与独立测试证据，但不能据此声称它已经成为 Rust `ADMIN CHECK TABLE` 的生产运行时实现。

## 核心职责

1. `CheckIndicesCount` 暂时开启会话的不可见索引开关，选择事务或显式快照，在相同快照上取得表计数和每个指定索引的计数，返回首个不一致索引的位置及哪一侧更多。
2. `CheckRecordAndIndex` 根据 `IndexMeta::column_offsets` 选出索引列，从最小整型 handle 对应的记录键开始扫描；缺失列按 `Datum::Null` 表示，并在探测索引前执行 NOT NULL/原始默认值规则。
3. `iterRecords` 负责限定表记录前缀、解码 handle 与行值、构造全局索引所需的分区 handle，并通过 `RecordIterFunc` 把每行交给上层策略。
4. `encode_record_key`、`decode_row_key`、`prefix_next` 和 `row_key_prefix` 提供该简化实现使用的有序记录键及范围扫描辅助逻辑。
5. `AdminError` 将 SQL 形状错误、计数不一致、行/索引不一致、解码、存储和 SQL 执行错误统一到一个返回类型。

## 主要符号

- 值与元数据：`Datum` 只覆盖 `Null`、`Int`、`Bytes`、`String`；`Handle` 支持普通整型和 `{ partition_id, handle }` 分区句柄；`RecordData` 保存报错所需的 handle 与索引列值；`Column`、`IndexMeta`、`KvPair` 是校验所需的最小元数据/KV 模型。
- 依赖边界：`RestrictedSqlExecutor::exec_restricted_sql` 执行带快照的受限 SQL；`SessionContext` 暴露不可见索引开关、事务/显式快照和 SQL 执行器；`Retriever::iter` 返回范围内 KV；`Table` 提供列、记录前缀、行解码和可选的分区 ID 解码；`Index::exists` 探测索引。
- 索引探测结果：`IndexLookup::{Found, Missing, Duplicate(Handle)}` 分别表示精确命中、索引缺项、相同值指向另一 handle。
- 计数入口：私有 `getCount` 要求受限 SQL 恰好返回一个 `CountRow`；公开 `CheckIndicesCount` 返回 `(greater, index_offset, Result)`；`TblCntGreater=1`、`IdxCntGreater=2` 编码不一致方向。
- 记录入口：公开 `CheckRecordAndIndex` 组装索引列和回调；公开 `iterRecords` 扫描并解码；`RecordIterFunc` 以布尔值控制是否继续。
- 解码辅助：`RowDecoder::decode` 委托 `Table::decode_row`；`makeRowDecoder` 仅包装 `Table`；`encode_record_key`/`decode_row_key` 通过翻转 `i64` 符号位维持字典序；私有 `prefix_next` 计算半开区间上界。
- 错误辅助：`ErrAdminCheckTable` 构造 `AdminError::Inconsistent`。这是函数而非 Go 版的全局标准错误对象。

## 执行流程

`CheckIndicesCount` 的流程如下：

1. 读取并保存 `SessionContext::optimizer_use_invisible_indexes`，随后设为 `true`，确保指定的不可见索引也能参与查询。
2. 调用 `transaction_start_ts`，无有效事务时以 `0` 为默认；若 `snapshot_ts()!=0`，显式快照覆盖事务起始时间。
3. 用 `getCount` 执行 `SELECT COUNT(*) FROM %n.%n USE INDEX()`，取得强制不走二级索引的表计数。
4. 按输入次序执行 `SELECT COUNT(*) FROM %n.%n USE INDEX(%n)`。计数相同则继续；首个不同立即返回方向常量、索引下标和 `CountMismatch`。索引查询错误也保留下标。
5. 闭包结束后恢复原不可见索引开关；事务时间戳、表查询或其他外层错误被整理为 `(0, 0, Err(...))`。

`CheckRecordAndIndex` 的流程如下：

1. 用 `IndexMeta::column_offsets` 从 `Table::columns` 克隆索引列；越界偏移会触发 Rust 下标 panic，而不是 `AdminError`，调用方必须保证元数据一致。
2. 从 `encode_record_key(record_prefix, Handle::Int(i64::MIN))` 开始，调用 `iterRecords`。
3. `iterRecords` 以 `prefix_next(record_prefix)` 为上界一次性取得 KV 列表；空列表直接成功。每个表前缀内的条目经 `decode_row_key` 得到原始 handle；全局索引再调用 `partition_id_from_key` 组成 `Handle::Partition`。
4. `RowDecoder` 委托表实现解码整行，再按索引列 ID 取值；行映射缺列时写入 `Datum::Null`。
5. 回调遍历列值：NULL 且 NOT NULL、又无 `origin_default` 时返回 `MissingNotNullValue`；存在原始默认值时以其替换 NULL。
6. `Index::exists` 命中则继续；缺失时产生没有 `index_record` 的 `Inconsistent`；重复 handle 时同时报告当前记录和索引指向的另一记录。
7. 回调返回 `false` 或任何错误会立即停止；正常完成则返回 `Ok(())`。扫描循环用当前记录键前缀跳过共享此前缀的后续辅助 KV。

## 数据与状态

- `CheckIndicesCount` 唯一修改的外部状态是会话的不可见索引开关。原值在正常返回和 `Result` 错误路径上都会恢复；它不是 RAII guard，因此若依赖实现 panic，恢复语句不会执行。
- 快照选择满足“非零显式 `snapshot_ts` 优先于事务 `start_ts`”；同一次调用的表计数和所有索引计数使用同一数值。
- `Retriever::iter` 返回拥有所有权的 `Vec<KvPair>`，所以本实现不是流式扫描：范围内结果在处理前整体驻留内存。`position` 是函数内局部游标。
- `BTreeMap<i64, Datum>` 以列 ID 索引解码值；输出给索引的 `Vec<Datum>` 严格遵循 `IndexMeta::column_offsets` 的顺序。
- 全局索引的 `Handle::Partition` 保存分区 ID，但 `Handle::int_value` 和记录键编码只使用内层 handle；分区身份来自实际记录键的 `Table::partition_id_from_key`。
- 本文件没有全局可变状态、缓存、后台任务或持久资源。trait 对象均为借用；`RowDecoder` 的生命周期不超过所借用的 `Table`。

## 依赖与调用关系

RustCodeGraph 的精确符号轨迹给出以下 crate 内调用边：

- `CheckIndicesCount → getCount → RestrictedSqlExecutor::exec_restricted_sql`，并调用 `SessionContext` 的开关、时间戳和执行器方法。
- `CheckRecordAndIndex → encode_record_key`，随后 `CheckRecordAndIndex → iterRecords`；回调下游为 `Index::exists`。
- `iterRecords → Retriever::iter / Table::record_prefix / prefix_next / makeRowDecoder / decode_row_key / row_key_prefix`；全局索引分支还调用 `Table::partition_id_from_key`。
- `iterRecords → RowDecoder::decode → Table::decode_row`。

已确认的直接 Rust 调用者仅来自 `pkg/util/admin/main_test.rs`（`CheckIndicesCount`）和 `pkg/util/admin/admin_integration_test.rs`（`CheckRecordAndIndex`）；`iterRecords` 的直接生产调用者是同文件的 `CheckRecordAndIndex`。`pkg/executor/check_table_index.rs` 中相同名字属于 `CheckTableRuntime` trait，并非对本函数的直接调用。Cargo 层面，工作区根清单包含该 crate，`pkg/executor/Cargo.toml` 声明了路径依赖，但源码搜索没有发现对应 crate 导入或适配实现。

外部 Rust 依赖仅为 `thiserror::Error` 派生宏。其余依赖由本地 trait 反转，不直接依赖 session、KV、tablecodec 或 expression crate。

## 错误处理与边界

- `getCount` 透传 `AdminError::Sql` 等执行器错误；结果不是恰好一行时返回 `InvalidCountRows(rows.len())`。`CountRow` 只建模一个 `i64`，未检查真实 SQL 行的列数或类型，这由执行器适配层负责。
- `CheckIndicesCount` 只报告首个不一致索引。表 COUNT 失败或取事务时间戳失败时下标为 `0`；某个索引 COUNT 失败时返回该索引下标；成功时 `(0, 0, Ok(()))`。
- `CheckRecordAndIndex` 将缺失、重复和 NOT NULL 缺值分开报告。任意 `Table`/`Retriever`/`Index` 实现返回的 `Decode`、`Storage`、`Sql` 错误都通过 `?` 原样传播。
- `decode_row_key` 只验证键至少有 8 字节，然后把末 8 字节当作 handle；它不验证表前缀或记录标记。扫描范围和 `starts_with(prefix)` 提供外层约束。
- `row_key_prefix` 对合法键返回完整键；因此跳过规则只跳过“以完整当前键开头”的后续 KV。它是对 Go `EncodeRecordKey + RowKeyPrefixFilter` 的简化表达，是否覆盖实际存储布局取决于 `Retriever`/键编码适配，当前测试没有覆盖辅助 KV。
- `prefix_next` 对末尾非 `0xff` 字节加一并清零溢出尾部；全为 `0xff` 时追加 `0`。该行为只在本简化 KV 边界内有证据，文档不推断其与所有真实 TiKV key helper 完全等价。
- `column_offsets` 越界、测试桩内部 `unwrap` 或 trait 实现 panic 不会转成 `AdminError`。安全接入时必须在边界校验元数据，并考虑用 guard 保证会话标志在 unwind 时恢复。

## 并发与资源生命周期

本文件本身是同步、单线程逻辑，没有线程、锁、channel 或 async task。`CheckIndicesCount` 以 `&mut dyn SessionContext` 独占会话状态修改窗口；其 SQL 执行器以共享借用访问。调用方不得在同一会话上并发改变不可见索引开关，否则恢复语义没有隔离保证。

Go 的 `iterRecords` 持有存储迭代器并用 `defer it.Close()` 释放；Rust 的 `Retriever::iter` 直接返回 `Vec<KvPair>`，没有需要显式关闭的迭代器资源，代价是一次性内存占用。`RowDecoder` 只借用 `Table`，在 `iterRecords` 返回时自然销毁。回调借用仅持续到单行调用结束，返回 `false` 可以提前终止扫描。

## 与 Go 版本的对应关系

对应源文件为 `pkg/util/admin/admin.go`，主要流程保持如下对应：

- 两版 `CheckIndicesCount` 都临时启用不可见索引、让显式快照覆盖事务 start TS、用 `%n` 参数执行表/索引 COUNT，并返回首个不一致方向和下标。Rust 的 `main_test.rs` 进一步验证了 snapshot=20 覆盖 txn_ts=10、第二个索引下标为 1，以及成功/错误路径均恢复标志。
- 两版 `CheckRecordAndIndex` 都按索引列偏移取列，从最小 handle 开始扫描，把 NULL 视作原始默认值，并区分索引缺失与唯一键指向另一 handle。Rust `admin_integration_test.rs::TestAdminCheckTableCorrupted` 复现 Go `admin_integration_test.go::TestAdminCheckTableCorrupted` 的末字节篡改场景；Rust 另测了默认值回填。
- 两版全局索引都把物理分区 ID 与行 handle 组合成分区 handle；两版都允许回调提前停止，并跳过当前行关联的辅助 KV。

当前 Rust 是有意收窄的移植边界，而非 Go 实现的完整类型复用：

- Go 直接使用 `sessionctx.Context`、`kv.Transaction`、`table.Table/Index`、`tablecodec`、表达式 schema、真实 `RowDecoder` 和 `consistency.Reporter`；Rust 使用本文件最小 trait/数据类型，`makeRowDecoder` 不构建表达式 schema，只把解码职责交给 `Table::decode_row`。
- Go 不一致错误通过 `consistency.Reporter` 生成，可携带表/索引元数据、编码键、日志脱敏与存储上下文；Rust 仅返回内存中的 `AdminError::Inconsistent`，不生成报告，也未接入标准 `dbterror` 错误码。
- Go 迭代器是流式且显式关闭；Rust 先收集 `Vec<KvPair>`。Go 记录调试日志，Rust 不记录日志。
- Go 的默认值可能通过 `table.GetColOriginDefaultValue` 求值并返回错误；Rust 的 `Column::origin_default` 已是 `Option<Datum>`，没有表达式求值或时区语义。
- Go 集成测试通过真实 testkit SQL 执行 `admin check table`；Rust 集成测试仅用内存 trait 桩直接调用本文件函数。故 Rust 证据覆盖算法分支，不等同于端到端 SQL 接线覆盖。

## 扩展指南

- 接入 Rust executor：最可能需要新增一个 `CheckTableRuntime` 适配器，把 executor 的表/索引/session/transaction 类型转换到本文件 trait，或统一两套边界；同时在 `pkg/executor` 的独立测试中证明 `CheckTableExec::Next/checkTableRecord` 真正调用本 crate。不能仅凭 Cargo 依赖认为接线完成。
- 支持真实数据类型与行解码：扩展 `Datum`、`Column` 和 `Table::decode_row`，并同步 Go 的时区、collation、生成列/表达式默认值语义。相关测试应放在独立的 `pkg/util/admin/*_test.rs`，不要内嵌到 `admin.rs`。
- 改进大表扫描：若把 `Retriever::iter` 改为流式/分页接口，应保持 `[start_key, prefix_next(prefix))` 边界、回调提前停止、错误传播和资源释放，并添加空表、跨前缀、辅助 KV、大数据量的独立测试。
- 扩展全局索引：修改 `Handle` 或键编码前，应验证分区 ID 解析、负/极值 handle 的顺序、普通与全局索引对同一行的探测行为；重点同步 `admin_integration_test.rs` 和 Go 对照测试。
- 改进诊断：如对齐 Go `consistency.Reporter`，应保留 `Inconsistent` 的当前记录/索引记录语义，并明确脱敏、错误码、编码失败和日志 I/O 的处理，避免把报告失败吞成普通缺失。
- 强化状态恢复：若会话实现可能 panic，可用作用域 guard 恢复 invisible-index 标志；需新增独立 panic/unwind 测试。正常错误路径已有 `main_test.rs` 覆盖。
- 兼容性风险集中在快照一致性、索引列顺序、默认值求值、全局索引 handle 和标准错误映射；性能风险集中在 `Vec<KvPair>` 全量物化和每行 `BTreeMap`/值克隆。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、`pkg/util/admin/admin.rs` 有 42 个符号；通过 `node --file` 阅读完整 440 行，并用 `query/node` 核对 `CheckIndicesCount`、`getCount`、`CheckRecordAndIndex`、`iterRecords`、`makeRowDecoder`、`encode_record_key`、`decode_row_key` 的定义及上述调用轨迹。
- Rust 源与边界：`pkg/util/admin/admin.rs`、`pkg/util/admin/lib.rs`、`pkg/util/admin/Cargo.toml`、工作区根 `Cargo.toml`、`pkg/executor/Cargo.toml`、`pkg/executor/check_table_index.rs`。
- Rust 独立测试：`pkg/util/admin/main_test.rs` 覆盖计数方向、索引下标、快照优先级、COUNT 空结果和开关恢复；`pkg/util/admin/admin_integration_test.rs` 覆盖损坏 handle 导致重复句柄不一致、NULL 的 origin default 回填。
- Go 对照：`pkg/util/admin/admin.go`、`pkg/util/admin/admin_integration_test.go`、`pkg/util/admin/main_test.go`。其中 `main_test.go` 只配置测试环境和 goroutine 泄漏检查，不直接验证 admin 算法。
- 接线核查：对所有 Rust 源搜索 `CheckIndicesCount|CheckRecordAndIndex|iterRecords|makeRowDecoder|ErrAdminCheckTable`，并搜索 `astersql-util-admin|astersql_util_admin|facade_util_admin`；结果支持“测试直接调用、同文件内部调用、executor 仅有独立 trait 同名方法、manifest 有依赖但无适配源码引用”的结论。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅运行任务规定的 11 章节结构命令，并人工复核没有把未接线能力描述为已上线行为。
