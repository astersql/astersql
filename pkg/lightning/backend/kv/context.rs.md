# `pkg/lightning/backend/kv/context.rs`

## 文件定位

本文件属于 `astersql-lightning-backend-kv` crate；crate 入口 `pkg/lightning/backend/kv/lib.rs` 以 `mod context` 装入它，并通过 `pub use context::*` 对外再导出公开项。`pkg/lightning/backend/kv/Cargo.toml` 将该 crate 标记为 Go 包 `pkg/lightning/backend/kv` 的移植，并声明本文件直接使用的 `encode`（`Datum`）和 `tablecodec`（时区解析）路径依赖。

它位于 Lightning/IMPORT INTO 的 SQL 行到 KV 编码边界：`pkg/lightning/backend/kv/session.rs::NewSession` 从 `encode::SessionOptions` 建立 `litExprContext` 和 `litTableMutateContext`，随后 `NewBaseKVEncoder`、`NewTableKVDecoder` 等持有这个轻量 `Session`。本文件只定义求值和表变更所需的上下文状态，不负责解析输入行、生成键、提交事务或写入 TiKV。

RustCodeGraph 的文件节点显示本文件被 `pkg/lightning/backend/kv/session.rs`、三个独立 Rust 测试文件以及 `pkg/executor/importer/import.rs` 使用。图查询没有为构造器生成可展示的精确 `callers/callees` 边，因此下述调用链同时以这些使用文件和源码引用为准，不把图中缺失的边当作“不存在调用”。

## 核心职责

- `newLitExprContext` 把 SQL mode、系统变量和可选时间戳折叠为稳定的表达式求值配置，包括类型转换容错、语句错误等级、时区和函数相关会话参数。
- `litExprContext::{setUserVarVal, unsetUserVar}` 维护大小写不敏感的用户变量，供导入语句中的表达式读取。
- `newLitTableMutateContext` 把表写入相关系统变量折叠为行编码、行级校验和、mutation checker、事务断言等级和 Row ID 分片配置。
- `litTableMutateContext` 提供 Lightning 编码路径所需的轻量查询接口，并用固定返回值明确声明不支持临时表、缓存表、交换分区 DML、连接态和 restricted SQL 等普通 TiDB session 能力。
- `RowIDShardGenerator` 提供可克隆、可跨线程共享的原子伪随机状态；当前上下文保持一个有效生成器以避免未来调用方取得空对象，但实际 Lightning Row ID 仍主要由导入编码链自身生成（对应 `context.go::newLitTableMutateContext` 的说明）。

## 主要符号

- SQL mode 常量 `MODE_STRICT_TRANS_TABLES`、`MODE_STRICT_ALL_TABLES`、`MODE_NO_ZERO_IN_DATE`、`MODE_NO_ZERO_DATE`、`MODE_ERROR_FOR_DIVISION_BY_ZERO`、`MODE_ALLOW_INVALID_DATES`：仅供 `newLitExprContext` 位运算判定，不对 crate 外公开。
- `ENABLE_ROW_LEVEL_CHECKSUM: AtomicBool` 与 `SetGlobalRowLevelChecksumEnabled(bool)`：进程级开关。使用 `Relaxed` 原子序；新建表上下文时读取一次，不会使既有上下文随全局值变化。
- `ErrorLevel::{Error, Warn, Ignore}`、`ImportTypeFlags`、`StatementErrorLevels`：把 Go 的错误上下文和 import 类型标志压缩为本 crate 可直接读取的数据结构。
- `litExprContext`：公开字段承载 `SQLMode`、类型标志、错误等级、当前 Unix 秒、包大小、除法精度、周格式、加密模式、GROUP_CONCAT 上限、时区字符串和 `UserVars: HashMap<String, Datum>`。其 `Clone` 是深复制 HashMap，而不是共享可变用户变量表。
- `newLitExprContext(sqlMode, sysVars, timestamp) -> Result<litExprContext, String>`：表达式上下文唯一构造入口；系统变量名先由 `normalizedSystemVars` 转为 ASCII 小写。
- `MutateBuffers { WriteStmtBuffer }`：当前只保留写语句字节缓冲，默认空 `Vec<u8>`。
- `RowIDShardGenerator { state, shard_step }`：`GetShardStep` 返回配置，`Next` 用 xorshift（左移 13、右移 7、左移 17）和 CAS 推进共享状态。
- `litTableMutateContext`：拥有表达式上下文的克隆、两个行编码布尔值、mutation checker、断言级别、变更缓冲和 Row ID 分片生成器。
- `newLitTableMutateContext(exprCtx, sysVars) -> Result<litTableMutateContext, String>`：表变更上下文唯一构造入口，并再次执行系统变量名规范化。
- `litTableMutateContext` 的查询方法：`AlternativeAllocators`、`GetExprCtx`、`ConnectionID`、`InRestrictedSQL`、`TxnAssertionLevel`、`EnableMutationChecker`、`GetRowEncodingConfig`、`GetMutateBuffers`、`GetRowIDShardGenerator`、`GetReservedRowIDAlloc`、`GetStatisticsSupport` 及各类能力探测方法。这些是 Go `table.MutateContext` 方法集的轻量同名表示；Rust 当前未声明对应 trait 实现。

## 执行流程

1. `pkg/lightning/backend/kv/base.rs::NewBaseKVEncoder` 或 `kv2sql.rs::NewTableKVDecoder` 调用 `session.rs::NewSession`；后者只收集 `KNOWN` 列表中的系统变量。
2. `NewSession` 先调用 `newLitExprContext`。构造器把变量名小写化，依据 strict mode 计算 `ImportTypeFlags`，再分别确定截断、NULL、缺省值和除零错误等级。
3. 构造器解析系统变量。缺失时使用本文件默认值；提供值时检查数值范围、时区是否合法，以及 block encryption mode 是否属于 12 个 AES 模式之一。`timestamp > 0` 直接采用调用者给出的 Unix 秒，否则在构造时读取系统时间一次。
4. `NewSession` 再调用 `newLitTableMutateContext(&exprCtx, &sysVars)`。构造器解析行格式版本、mutation checker、断言等级和 shard step，用当前纳秒时间的低 64 位（强制最低位为 1）初始化分片生成器。
5. 行级校验和只有在 `tidb_row_format_version == "2"` 且进程级 `ENABLE_ROW_LEVEL_CHECKSUM` 为真时启用。结果存进上下文字段，之后不再动态读取全局原子量。
6. 编码期间，`base.rs::AddRecord` 经 `Session::GetTableCtx` 读取 `RowEncodingEnabled` 来选择 canonical row 格式；导入表达式侧的 `pkg/executor/importer/import.rs::CreateColAssignSimpleExprs` 把 `litExprContext` 交给 `ColAssignExpressionBuilder::Build`。用户变量由 `Session::{SetUserVarVal, UnsetUserVar}` 转发到本文件的方法。

## 数据与状态

`litExprContext` 和 `litTableMutateContext` 都由 `Session` 按值拥有。建表上下文时会克隆表达式上下文，因此 `Session.exprCtx` 后续用户变量修改不会同步到 `Session.tblCtx.exprCtx`；当前已检索到的生产调用通过 `Session::GetExprCtx` 使用前者，文档不能假定两份状态自动共享。

表达式默认值是：`max_allowed_packet = 64 MiB`、`div_precision_increment = 4`、`default_week_format = "0"`、`block_encryption_mode = "aes-128-ecb"`、`group_concat_max_len = 1024`、`time_zone = "SYSTEM"`、空用户变量表。`CurrentTimestamp` 是构造时确定的 `i64` 秒值，不会在每次读取时重新取时间。

表变更默认值是：旧行格式、关闭行级校验和、关闭 mutation checker、断言等级 `OFF`、空写缓冲、`shard_step = i64::MAX`。`RowIDShardGenerator::Clone` 共享同一个 `Arc<AtomicU64>`，因此克隆生成器会推进同一序列；克隆整个表上下文时生成器状态仍共享，而缓冲和表达式上下文被复制。

系统变量名使用 ASCII 小写规范化。用户变量的插入和删除也执行 ASCII 小写化，因此 `Example` 与 `EXAMPLE` 指向同一个键。值本身除特定枚举的大小写转换外不做空白修剪。

## 依赖与调用关系

上游直接链路为：

`NewBaseKVEncoder` / `NewTableKVDecoder` → `session.rs::NewSession` → `newLitExprContext` → `newLitTableMutateContext`。

`pkg/lightning/backend/kv/lib.rs` 的公开再导出使 `litExprContext` 能被 `pkg/executor/importer/import.rs` 引用；该文件的 `ColAssignExpressionBuilder::Build` 和 `CreateColAssignSimpleExprs` 把它作为编译 SET 赋值表达式的上下文。`session.rs` 则直接导入两个 context 类型和两个构造器，并转发用户变量操作。

下游依赖很窄：标准库提供 HashMap、原子量和系统时间；`encode::Datum` 是用户变量值；`tablecodec::time::Location` 仅用于验证非 `SYSTEM` 时区字符串；`crate::Allocators` 只出现在 `AlternativeAllocators` 的返回类型。`Cargo.toml` 还声明 `verification`，但本文件没有直接使用它。

数据消费证据包括 `base.rs::AddRecord` 读取 `RowEncodingEnabled`，`session_internal_test.rs` 经 `NewSession` 验证字段装载和用户变量，`sql2kv_test.rs` 验证编码器/解码器 session 的时间戳及行格式。本文件本身不持有 KV、表元数据或外部 I/O 句柄。

## 错误处理与边界

两个构造器用 `Result<_, String>` 原样向 `NewSession` 传播配置错误。`newLitExprContext` 拒绝：不能解析为无符号整数的数值变量、范围不在 `1024..=1_073_741_824` 的 `max_allowed_packet`、大于 30 的除法精度、大于 7 的周格式、小于 4 的 GROUP_CONCAT 上限、无效时区和不支持的 AES 模式。读取系统时间早于 Unix epoch 也会返回底层错误字符串。

`newLitTableMutateContext` 只接受布尔值 `1`/`ON`/`0`/`OFF`（字母不区分大小写），只接受行格式 `1` 或 `2`，只接受断言等级 `OFF`/`FAST`/`STRICT`，并要求 shard step 至少为 1。它不会忽略传入 map 中的无关键；它只读取已知键，因此直接调用时无关键自然无效。但经 `NewSession` 的生产路径会先过滤变量，且当前 `session.rs::KNOWN` 没有 `tidb_shard_allocate_step`，所以该变量在直接构造器测试中有效，在 `NewSession` 路径中会被过滤并落到 `i64::MAX`；扩展时必须同时核对两处列表。

固定能力返回值是有意边界：连接 ID 为 0、非 restricted SQL、没有替代 allocator、缓存表/临时表/交换分区 DML 均不支持；统计支持返回真但 `UpdatePhysicalTableDelta` 为空操作；reserved row ID 返回 `(0, true)`。这些返回值只适用于轻量导入上下文，不能据此推断普通 SQL session 行为。

## 并发与资源生命周期

全局行级校验和开关和 Row ID 状态都用 `Ordering::Relaxed`，只保证单个原子值的无数据竞争访问，不建立跨字段 happens-before 关系。这里的需求分别是读取独立布尔配置和原子推进伪随机状态，不依赖其他内存同步。

`RowIDShardGenerator::Next` 使用 `compare_exchange_weak` 循环；竞争或伪失败时以实际状态重算，成功后返回新值。初始 seed 强制为奇数，避免 xorshift 从零开始永远停在零。算法不提供密码学随机性，也未承诺跨进程可复现。

普通 `litExprContext::UserVars` 和 `MutateBuffers::WriteStmtBuffer` 没有锁；它们依靠可变借用或上层会话串行访问。只有 Row ID 原子状态可通过 `Arc` 安全共享。构造器不启动任务、不打开文件/网络连接，也没有显式清理动作；所有集合和 Arc 随上下文 drop 自动释放。

独立测试 `context_test.rs` 为修改进程级 checksum 开关使用 `ROW_CHECKSUM_TEST_LOCK`，并在断言后恢复为 false，说明调用方测试必须避免并行污染该全局状态。生产构造器只在创建时拍摄开关值，既有上下文无需锁定全局量。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/backend/kv/context.go`，测试对照是同目录 `context_test.go`。类型和构造器一一对应于 Go 的 `litExprContext`、`newLitExprContext`、`litTableMutateContext` 和 `newLitTableMutateContext`；SQL mode 分支、主要系统变量、时间戳语义、用户变量大小写不敏感、row format、全局 row checksum、mutation checker、断言等级及 shard step 是当前 Rust 测试刻意覆盖的共同语义。

Rust 是轻量重建而非完整框架对象移植。Go 表达式上下文嵌入 `exprstatic.ExprContext`，包含共享 warning handler、plan-cache tracker、RNG、可选属性、location 对象等；Rust 仅保存编码链目前读取的字段，没有 `setNewCollationEnabled`，也没有完整 `exprctx.ExprContext` trait。Go 表上下文实际实现 `table.MutateContext`/`StatisticsSupport`，持有 `tblctx.RowEncodingConfig`、真实 mutate buffers 和 `ReservedRowIDAlloc`；Rust 用同名方法和简化结构表达所需子集。

还有两项重要所有权差异：Go 表上下文保存同一个 `*litExprContext` 指针，Rust 构造器克隆它；Go 的 `RowIDShardGenerator` 用 `math/rand` 源，Rust 使用共享原子 xorshift。若新增功能依赖 Go 对象身份、动态警告状态、精确随机序列或真实 table trait，不能把当前轻量字段视作等价实现，必须先补齐对应抽象和独立测试。

Go 测试覆盖比 Rust 更广，包括 warning 共享、CurrentTime 重复读取、plan cache、RNG、可选属性和接口返回对象。Rust 的 `context_test.rs` 覆盖当前实现拥有的配置与验证分支；未在 Rust 类型中出现的 Go 能力应记为尚未移植，而不是由测试缺失推断为隐式支持。

## 扩展指南

- 新增表达式系统变量：同时修改 `newLitExprContext` 的解析/默认值/边界、`litExprContext` 字段、`session.rs::NewSession::KNOWN`，并在独立 `context_test.rs` 和必要的 `session_internal_test.rs` 添加成功、默认和非法值案例。
- 新增表变更系统变量：修改 `newLitTableMutateContext`，并特别检查 `NewSession::KNOWN` 是否会把它传入；`tidb_shard_allocate_step` 当前的过滤差异是一个现成风险点。
- 改变 SQL mode 映射：以 `context.go::newLitExprContext` 和 `context_test.go::TestLitExprContext` 为语义基准，覆盖 strict/non-strict、两个 zero-date 位、allow-invalid-dates 和 division-by-zero 的组合，避免只测单一模式。
- 扩充 Go interface 对齐能力：不要只增加固定返回方法；先确定消费方是否需要对象身份、可变共享状态或真实 trait，然后在生产文件实现，在 `context_test.rs`（而非本源文件）写测试，并补调用链集成用例。
- 改变全局 checksum 或 Row ID 并发策略：保持全局测试串行化与状态恢复，明确所需内存序；如要求确定性，给生成器注入 seed，而不是在测试中依赖系统纳秒。
- 调整行编码开关时同步检查 `base.rs::AddRecord`、编码/解码测试和 Go 的 `tblctx.RowEncodingConfig`。该开关直接改变落盘行值格式，存在数据兼容和性能风险。
- Rust 测试必须继续放在独立的 `context_test.rs` 或相邻集成测试文件，不能嵌入 `context.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/lightning/backend/kv` 确认 Rust/Go 源与独立测试均已索引；`node --file pkg/lightning/backend/kv/context.rs --offset 1 --limit 500` 返回完整 372 行、46 个符号并列出 5 个使用文件。
- RustCodeGraph 精确查询：`query newLitExprContext`、`query newLitTableMutateContext`、`query SetGlobalRowLevelChecksumEnabled`、`query setUserVarVal`、`query unsetUserVar`、`query GetRowEncodingConfig` 确认 Rust 定义、Go 对照及测试候选；对应 `callers/callees` 命令未输出静态边，故调用关系再由源码引用交叉验证。
- 生产源码：`pkg/lightning/backend/kv/context.rs`（所有本文符号）、`lib.rs`（模块装配与再导出）、`session.rs`（构造、过滤和用户变量转发）、`base.rs`（行格式消费）、`kv2sql.rs`（解码 session 构造）、`pkg/executor/importer/import.rs`（表达式上下文消费）。
- crate 边界：`pkg/lightning/backend/kv/Cargo.toml` 的 package、lib path、路径依赖和 Go package 元数据。
- 语义对照：`pkg/lightning/backend/kv/context.go` 与 `context_test.go`。
- Rust 独立测试：`pkg/lightning/backend/kv/context_test.rs` 直接覆盖 SQL mode、配置范围、用户变量、默认值、checksum、行格式、mutation checker、断言等级和 shard generator；`session_internal_test.rs`、`sql2kv_test.rs` 覆盖经 Session/编码链的装载与消费。
- 本任务是只增说明文档的静态分析，按计划不运行 Cargo；最终以固定 11 章节结构命令和人工证据复核验收。
