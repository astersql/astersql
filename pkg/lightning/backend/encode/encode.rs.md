# `pkg/lightning/backend/encode/encode.rs`

## 文件定位

本文件是 `astersql-lightning-backend-encode` crate 的核心协议文件。crate 入口 `pkg/lightning/backend/encode/lib.rs` 将本文件的全部公开项重新导出；`pkg/lightning/backend/encode/Cargo.toml` 则把 crate 映射到 Go 包 `pkg/lightning/backend/encode`，并只直接依赖 `astersql-lightning-verification` 来取得 `KVChecksum`。

它位于 Lightning “解析后的 SQL 行”与“具体后端可写行”之间：调用方通过 `EncodingBuilder` 创建 `Encoder`，再把 `Datum` 序列编码成不透明的 `Row`；后端通过 `Row::ClassifyAndAppend` 把结果并入 `Rows` 缓冲并累计校验和。本文件只定义边界、配置和值类型，不实现实际 KV/SQL 编码。已确认的具体实现位于 `pkg/lightning/backend/kv/sql2kv.rs` 和 `pkg/lightning/backend/tidb/tidb.rs`。

## 核心职责

1. 用 `EncodingBuilder`、`Encoder`、`Rows`、`Row` 四个 trait 隔离编码调用方与后端表示，使 KV 后端和 TiDB SQL 后端共享同一生命周期。
2. 用 `EncodingConfig` 与 `SessionOptions` 携带表、输入路径、日志字段和 SQL 会话属性；这些结构本身不解释或执行配置。
3. 用 `Datum`、`ColumnType`、`Column`、`Table` 提供 Rust 侧独立、可动态分发的输入值和表元数据模型。
4. 用 `Context` 表达立即取消、超时和外部取消回调，用 `EncodeError` 统一 trait 边界上的人类可读错误。
5. 通过 `Any` 下转型入口保留具体后端的测试、诊断和桥接能力；下转型规则由实现方和调用方共同约定。

## 主要符号

- `Context { cancelled, cancel_check, deadline }`：可克隆的轻量取消上下文。`cancelled()` 创建永久取消实例；`with_timeout(Duration)` 保存绝对截止时刻；`with_cancel_check(Arc<dyn Fn() -> bool + Send + Sync>)` 接入共享回调；`is_cancelled()` 对三种来源做逻辑或。自定义 `Debug` 只输出求值后的 `cancelled`，不会泄露回调或截止时刻。
- `Logger { fields }`：日志键值字段载体。本文件不产生日志。
- `Datum`：列值枚举，覆盖空值、上下界哨兵、有符号/无符号整数、浮点、字节、字符串、JSON、二进制字面量、BIT、ENUM、SET、十进制、时间戳和时长；默认值为 `Null`。
- `Table`：要求实现方可跨线程共享，并提供 `name()`、`columns()` 与 `as_any()`。`as_any()` 用于具体实现下转型。
- `ColumnType`：canonical 行/索引 codec 所需的存储类型提示；`Auto` 是默认兼容模式，由具体编码器按 `Datum` 和字符集判断。
- `Column`：保存名称、字符集、存储类型、ENUM/SET 元素，以及生成列、自增、AUTO_RANDOM、主键标记。
- `EncodingConfig`：组合 `SessionOptions`、源文件 `Path`、可选 `Table`、`Logger` 和 `UseIdentityAutoRowID`。`Table` 使用 `Arc<dyn Table>`，允许配置与编码器共享元数据所有权。
- `EncodingBuilder`：`NewEncoder(&Context, &EncodingConfig)` 创建后端编码器，`MakeEmptyRows()` 创建与该后端匹配的空缓冲；trait 要求 `Send + Sync`。
- `Encoder`：`Encode(&[Datum], rowID, &[i32], offset)` 编码一行，`Close()` 结束资源生命周期，`as_any()` 暴露只读下转型；trait 要求 `Send`。
- `SessionOptions`：携带 `SQLMode`、语句时间戳、系统变量、逻辑导入预处理开关、AUTO_RANDOM 种子、重复检测索引 ID 和最小提交时间戳。
- `Rows`：批量结果缓冲。`Clear(self: Box<Self>)` 消耗旧句柄并返回可复用容量的空缓冲；两个 `Any` 方法分别支持只读和可变下转型。
- `Row`：单行结果。`ClassifyAndAppend` 同时接收数据/索引缓冲及各自校验和，`Size()` 返回总 KV 字节量，`Any` 方法允许具体类型操作。
- `EncodeError(String)`：可克隆、可比较的字符串错误，实现 `Display` 与标准 `Error`，不保留结构化错误源。

## 执行流程

典型流程由 trait 合同规定，具体行为由后端实现：

1. 上层准备 `SessionOptions`、源路径、表元数据与日志字段，组装 `EncodingConfig`。
2. 上层从后端获得 `dyn EncodingBuilder`，调用 `NewEncoder`；构造失败以 `EncodeError` 返回。`pkg/lightning/backend/tidb/tidb.rs::encodingBuilder::NewEncoder` 会读取 SQL mode、列、路径及预处理开关，`pkg/lightning/backend/kv/sql2kv.rs::NewTableKVEncoder` 会继续调用 `NewBaseKVEncoder`。
3. 对每个输入行调用 `Encoder::Encode`。参数中的 `columnPermutation` 把表列映射到输入列，负数通常代表源数据缺列；`rowID` 参与 handle、自增或 AUTO_RANDOM 计算；`offset` 用于实现层定位输入错误。本文件不强制这些参数的具体算法。
4. 调用方用 `MakeEmptyRows` 分别建立数据与索引缓冲。每个编码结果再调用 `Row::ClassifyAndAppend`，由后端决定如何分类并更新两个 `KVChecksum`。
5. 批次写出后以 `rows = rows.Clear()` 的所有权模式清空并复用缓冲。完成或错误退出时调用 `Encoder::Close`；trait 没有 `Drop` 兜底，因此调用方必须显式遵守该生命周期。

KV 实现的实证流程是：`tableKVEncoder::Encode` 按置换填列、处理缺省/自增值、求值生成列、必要时 rebase RowID、调用 `Record2KV`，再将可比较编码的 RowID 写入 `Pairs`；`Pairs::ClassifyAndAppend` 通过记录键判别将 KV 分流并对相应校验和调用 `UpdateOne`。TiDB 实现则把 `tidbRow` 全部追加到数据缓冲，并用其大小和一条 KV 的计数更新数据校验和，不使用索引缓冲。

## 数据与状态

- 本文件没有全局可变状态。`Context` 的截止时间基于 `Instant`，取消回调放在 `Arc` 中，因此克隆上下文会共享回调、复制同一截止时刻；公开布尔位则按值复制。
- `EncodingConfig::Table` 和 `Context::cancel_check` 使用 `Arc` 管理共享所有权。其他配置（包括 `HashMap`、字符串和日志字段）在克隆时做值复制。
- `Datum` 与 `Column` 是面向编码边界的自有数据，避免返回借用跨越编码器调用；`f64` 使 `Datum` 只能实现 `PartialEq`，不能实现 `Eq`。
- `Rows::Clear` 消耗 `Box<Self>` 是重要不变量：调用方不能继续使用旧句柄，具体实现可以原地清空后返回同一分配。`pkg/lightning/backend/tidb/tidb.rs::tidbRows::Clear` 即采用这种方式。
- `ClassifyAndAppend` 同步修改四个外部对象；方法本身只借用 `Row`，允许同一个已编码行在不改变自身的情况下被读取。具体 KV 实现会克隆 pair 后追加。
- `SessionOptions::MinCommitTS` 在本层仅是载荷。Go 对照把它定义为重复检测过滤阈值；本文件不会自行保证事务提交时间戳，也不能仅凭字段注释推断下游已使用该值。

## 依赖与调用关系

- crate 内部：`lib.rs` 的 `mod encode; pub use encode::*;` 使本文件成为公开 API；唯一直接 crate 依赖是 `verification::KVChecksum`。
- 具体实现：`pkg/lightning/backend/kv/sql2kv.rs` 实现 `Encoder for tableKVEncoder`、`Row for Pairs`、`Rows for Pairs/GroupedPairs`；`pkg/lightning/backend/tidb/tidb.rs` 实现 `EncodingBuilder for encodingBuilder`、`Encoder for tidbEncoder`、`Row for tidbRow`、`Rows for tidbRows`。
- KV 构造链：`NewTableKVEncoder` → `pkg/lightning/backend/kv/base.rs::NewBaseKVEncoder` → 会话/表元数据准备；后者读取 `UseIdentityAutoRowID`、AUTO_RANDOM 种子和表分片位来选择恒等或混洗 RowID 映射。
- RustCodeGraph 精确查询确认 `EncodingBuilder::NewEncoder`、`Encoder::Encode`、`Row::ClassifyAndAppend` 为 trait 方法，并识别上述具体实现。对 trait 声明执行 callers/callees 没有返回静态边，这是动态分发限制；实现与消费关系由具体实现节点、Cargo 依赖和源码引用共同核验。
- Cargo 直接消费者包括 `pkg/lightning/backend`、`backend/kv`、`backend/tidb`，并延伸到 `pkg/executor/importer`、`pkg/dxf/importinto`、`pkg/ddl/ingest`、`pkg/ingestor`、`pkg/session` 等 crate。存在同名的 `lightning/pkg/importer/stubs.rs::encode` 协议，不能把它的调用边误认为本 crate 的直接调用边。

## 错误处理与边界

- 只有 `NewEncoder` 和 `Encode` 在协议层返回 `EncodeError`；`Close`、`Clear`、`ClassifyAndAppend`、`Size` 均不能返回错误。实现若在这些无错误返回值的方法中遇到类型不匹配，当前实现通常会 `panic!`。
- `EncodeError` 只包装字符串，没有错误分类、错误源或输入位置字段；实现应在转换前把表列、路径或偏移等诊断上下文写入消息。
- `EncodingConfig::Table` 是 `Option`，协议允许缺表；但 KV 实现 `NewBaseKVEncoder` 要求它能下转型为 `TableDefinition`，否则返回 `encoding table must be a TableDefinition`。TiDB 实现在缺表时使用空列集合。
- `as_any`/`as_any_mut` 是刻意保留的逃生口，但错误具体类型会导致 `expect`/`unwrap` 失败。例如 `Pairs::ClassifyAndAppend` 要求两个缓冲均为 `Pairs`，`tidbRow` 要求数据缓冲为 `tidbRows`。
- `ColumnType::Auto` 和字符串形式的 Decimal/Timestamp/Duration 延迟了规范化与校验；严格模式、字符集、ENUM/SET 数值恢复等必须由具体编码器处理。
- `Context::is_cancelled` 只报告状态，不会中断 `Encode`；trait 实现或上层循环必须主动轮询。回调若耗时或发生 panic，会直接影响调用线程。
- `GroupedPairs::Clear` 在当前 KV 实现中明确为 `panic!("not implemented")`，相关独立测试将该限制固定下来；不能假定所有 `Rows` 实现都可清空。

## 并发与资源生命周期

- `EncodingBuilder: Send + Sync` 可被多个线程共享；`Encoder: Send`、`Rows: Send`、`Row: Send` 可以在线程间转移，但没有 `Sync` 保证，不能据此并发共享同一可变编码器或缓冲。
- `Table: Send + Sync` 配合 `Arc<dyn Table>` 支持跨线程共享只读元数据。`Context` 的取消回调也要求 `Send + Sync`，但本文件不提供原子取消令牌；公开 `cancelled` 字段只是普通布尔值。
- `Encoder::Encode` 需要 `&mut self`，允许实现安全复用行缓存。`pkg/lightning/backend/kv/base.rs::BaseKVEncoder::recordCache` 即为此类复用状态，所以同一 encoder 的行编码天然串行。
- `Close(&mut self)` 是显式清理点。KV 实现关闭内部 Session 并记录 closed，后续 `Encode` 返回错误；协议没有规定重复关闭、遗漏关闭或析构行为，实现与调用方需单独约定。
- 校验和与行缓冲由调用方独占可变借用，`ClassifyAndAppend` 的签名阻止同一作用域中的数据竞争，但不提供跨任务批次排序、背压或事务原子性。

## 与 Go 版本的对应关系

Go 基准文件是 `pkg/lightning/backend/encode/encode.go`。Rust 保留了 `EncodingConfig`、`EncodingBuilder`、`Encoder`、`SessionOptions`、`Rows`、`Row` 的整体分层和 Go 风格方法名，也保留了 `Clear` 的“返回新 Rows、可共享旧容量”合同、`ClassifyAndAppend` 的双缓冲/双校验和合同，以及 `Size() uint64` 的语义。

主要适配差异如下：

- Go 直接使用 `context.Context`、`types.Datum`、`table.Table`、`log.Logger` 和 `mysql.SQLMode`；Rust 在本 crate 内定义轻量 `Context`、`Datum`、`Table`/`Column`、`Logger`，并用 `u64` 表示 SQL mode。
- Rust 为 trait object 增加 `Send`/`Sync` 约束、`Box<dyn ...>` 所有权和 `Any` 下转型方法；这些是 Rust 动态分发与所有权适配，不是 Go API 中的业务能力。
- Rust `columnPermutation` 使用 `i32`，Go 使用平台宽度的 `int`；扩展时必须继续把负数当作缺列哨兵，并检查转换边界。
- Rust `EncodingConfig::Table` 可为空，Go 接口字段不是显式 Optional；具体 Rust 后端对空值的处理并不一致。
- Rust 增加了显式 `ColumnType` 与 ENUM/SET 元素等元数据，以支持其 canonical codec 恢复 Go `types.Datum`/表元数据隐含的类型信息。
- Go 对 `MinCommitTS` 的注释是“重复检测只考虑大于该提交时间戳的记录”；因此 Rust 使用者应按重复检测阈值理解它，而不是把本文件现有的事务可见性说明视作已实现保证。

## 扩展指南

- 新增后端时，应成套实现 `EncodingBuilder`、`Encoder`、`Rows`、`Row`，保证 `MakeEmptyRows` 返回的具体类型与 `ClassifyAndAppend` 的下转型一致，并在独立 `*_test.rs` 中覆盖构造、编码、分类、校验和、清空和关闭后行为。不要把测试放入本生产文件。
- 新增 `Datum` 或 `ColumnType` 分支时，必须同步检查 `pkg/lightning/backend/kv` 的 canonical codec、TiDB SQL 字面量/预处理参数编码、Go 对照类型和全部穷尽匹配；关注排序编码、字符集、溢出、时区和向后兼容风险。
- 修改 `EncodingConfig`/`SessionOptions` 字段时，应追踪所有结构体字面量和 Cargo 消费者；新增必填字段会造成广泛编译影响，新增默认字段则需验证默认值是否与 Go 零值一致。
- 修改取消语义时，优先保持 `Context::is_cancelled` 轻量且无副作用，并为立即取消、零/已过期超时、回调翻转和 `Debug` 输出增加同目录独立测试文件；当前目标 crate 尚无自己的 `encode_test.rs`。
- 修改 `Clear` 或 `ClassifyAndAppend` 合同时，要同步 `pkg/lightning/backend/backend_test.rs`、`pkg/lightning/backend/kv/sql2kv_test.rs`、`pkg/lightning/backend/tidb/tidb_test.rs` 及相应 Go 测试。尤其不要默默把当前明确不支持的 `GroupedPairs::Clear` 当作通用可用能力。
- 若要引入结构化错误，需同时调整所有 `EncodeError(...)` 构造和字符串断言，并决定是否保留 Go 侧可观察的错误文本；若要自动清理编码器，应先审计现有显式 `Close` 顺序和 Session 副作用。

## 验证依据

- 目标与 crate 边界：`pkg/lightning/backend/encode/encode.rs`、`pkg/lightning/backend/encode/lib.rs`、`pkg/lightning/backend/encode/Cargo.toml`、根 `Cargo.toml` 以及各直接消费者的 Cargo manifest。
- Go 对照：`pkg/lightning/backend/encode/encode.go`；其接口、字段及注释用于核对迁移语义，而非推测 Rust 实现已支持的能力。
- 具体实现：`pkg/lightning/backend/kv/sql2kv.rs`、`pkg/lightning/backend/kv/base.rs`、`pkg/lightning/backend/tidb/tidb.rs`。
- 独立 Rust 测试：`pkg/lightning/backend/backend_test.rs::TestMakeEmptyRows`、`TestNewEncoder`；`pkg/lightning/backend/kv/sql2kv_test.rs::TestClassifyAndAppend`、`classify_does_not_treat_embedded_record_marker_as_a_record_key`、`grouped_pairs_*_remains_unsupported`；更完整的后端语义覆盖位于 `pkg/lightning/backend/tidb/tidb_test.rs`。Go 对照测试包括 `pkg/lightning/backend/backend_test.go`、`pkg/lightning/backend/kv/sql2kv_test.go` 和 `pkg/lightning/backend/tidb/tidb_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/lightning/backend/encode` 找到 `encode.rs`、`lib.rs` 和 Go 对照；对目标文件执行 `node --file` 完整读取 251 行；`query` 确认 `EncodingBuilder`、`Encoder`、`Rows`、`Row` 及 `NewEncoder`、`ClassifyAndAppend` 的声明和具体实现。trait callers/callees 查询无静态输出，已用具体实现、Cargo 引用和测试补证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令验证目标文件存在且固定二级标题恰好为 11 个，并人工检查没有把 stub、未实现分支或字段载荷描述成已接线能力。
