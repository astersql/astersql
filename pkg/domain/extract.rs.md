# `pkg/domain/extract.rs`

## 文件定位

本文件属于 `astersql-domain` crate，由 [`pkg/domain/lib.rs`](lib.rs) 以 `pub mod extract` 暴露。它实现 Plan Extract 的领域层：把语句摘要记录规范化、按 SQL digest 与 plan digest 去重，解析视图依赖，解码 binary plan，并把最终包交给可替换的数据源写出。生产入口位于 `pkg/server/extract_runtime.rs`：`CanonicalExtractRuntime::new` 用当前 `Domain` 和 `ProductionExtractSource` 构造 `ExtractHandle::new_with_domain`，随后 `ExtractRuntime::extract_task` 将 HTTP 层任务转换为本文件的 `ExtractTask` 并调用 `ExtractHandle::extract_task`。

源码第 23–591 行是整段注释掉的 Go 机械翻译草稿，不参与编译；当前可执行 Rust 实现从 `use std::collections::{BTreeMap, BTreeSet};` 开始。分析行为时应以第 592 行之后的实现为准，不能把注释草稿中的 zip API、restricted SQL 或错误分支当作已经接线的 Rust 行为。

## 核心职责

- 用 `ExtractTask` 表达 Plan Extract 的时间窗、后台标志、是否跳过统计信息及是否读取持久化历史摘要。
- 用 `ExtractSource` 隔离语句摘要查询、InfoSchema 表解析、计划解码和产物落盘；本文件负责与具体存储无关的编排与过滤。
- 在 `ExtractHandle::extract_task` 中保留 Go 的关键语义：后台任务暂不执行；历史视图依赖持久化摘要开关；收集阶段串行；无效记录丢弃；相同 `(digest, plan_digest)` 后写覆盖前写；截断 SQL 不解码计划；其余记录解码后参与表集合构建。
- 通过 `view_dependencies_from_sql` 和 `DomainAstExtractSource` 使用生产 parser AST 与当前 `Domain` 的 InfoSchema 递归展开视图依赖，并用 visited 集合终止视图环。
- 生成不重复的 zip 文件名，并把完整 `ExtractPlanPackage` 交给 `ExtractSource::dump_package`。文件自身不实现 zip 条目格式或外部存储 I/O；这些属于生产 `ExtractSource` 的职责。

## 主要符号

- 常量 `EXTRACT_META_FILE`、`EXTRACT_TASK_TYPE`、`EXTRACT_PLAN_TASK_SKIP_STATS`、`EXTRACT_TASK_DIR_NAME`：保留 Go 产物协议中的文件名、元数据键和目录名。当前文件中只有目录辅助函数直接读取这些常量；实际 dump 方可使用其余协议常量。
- `ExtractType::Plan` 与 `ExtractType::as_str`：当前唯一任务类型及其外部字符串 `"Plan"`。因为枚举只有一个变体，当前 `extract_task` 不再执行 Go 的未知类型分派。
- `ExtractTask::new_plan(begin, end)`：构造同步、包含 stats、读取当前摘要视图的默认 Plan 任务；不自行验证 `begin <= end`。
- `TableNamePair`、`StatementKey`、`StatementRecord`：分别描述库表/视图、去重键和单条摘要记录。三者的有序派生让 `BTreeSet`/`BTreeMap` 输出稳定；`StatementRecord::is_valid` 只接受大小写精确为 `Select`、非空 schema 和非空 plan digest。
- `ExtractPlanPackage`：最终交给 dump 层的有序表集合与有序记录映射。
- `ExtractSource`：`Send + Sync` 的领域边界。读取记录、查表、解析视图依赖、解码计划、dump 包和读取持久化开关均通过此 trait 注入。
- `TableDependencyVisitor` 与 `view_dependencies_from_sql`：遍历真实 parser AST，收集表名与 CTE 名；无 schema 的表使用视图所在库作为默认 schema，最终剔除名称属于 CTE 的表引用。
- `DomainAstExtractSource`：装饰已有 `ExtractSource`，仅把 `view_dependencies` 替换为基于 `Domain::stats_table` 中视图定义的递归 AST 解析，其余调用原样委托。
- `ExtractHandle`：公开同步入口，持有 `Arc<dyn ExtractSource>` 和只覆盖记录收集阶段的 `Mutex<()>`。
- `generate_extract_file_name`、`get_extract_task_dir_name`：分别生成 `extract_<hex>_<nanos>.zip` 和返回固定目录 `extract`。

## 执行流程

1. Server 的 `CanonicalExtractRuntime::extract_task` 将秒级 `Timestamp` 转为 `SystemTime`，用 `ExtractTask::new_plan` 构造任务并复制后台、skip-stats、history-view 三个开关，然后调用 `ExtractHandle::extract_task`。
2. `extract_task` 先处理两个前置分支：后台任务直接返回 `Ok(None)`；请求 history view 而 `persistent_statement_summary_enabled` 为假时，返回与 Go 相同的配置错误。
3. 进入 `serial` 锁保护的收集阶段，调用 `ExtractSource::statement_records`。每条记录的每个原始表名都经 `ExtractSource::table` 解析为当前表元数据；命中内部库或缺失表会清空结果并丢弃整条记录。`schema_name` 取解析后最后一张表的 database。
4. 记录还必须通过 `StatementRecord::is_valid` 且至少包含一张表。合格记录以 `StatementKey { digest, plan_digest }` 插入 `BTreeMap`；相同键的后记录覆盖前记录。
5. 锁在收集结束后释放。对每条去重记录，若 SQL 包含 `"(len:"`，设置 `skipped = true` 并跳过计划解码和表集合加入；否则调用 `decode_binary_plan`，去掉首尾换行，把记录涉及的表加入 package。
6. 从 package 表集合筛出 `is_view` 的条目，逐个调用 `view_dependencies` 并合并结果。生产构造器使用 `DomainAstExtractSource`：它从当前 InfoSchema 读取视图定义，解析 AST，递归解析嵌套视图，并靠 `visited` 防止环路无限递归。
7. `generate_extract_file_name` 生成名字，`dump_package` 完成具体文件写出。成功返回 `Ok(Some(name))`；任何 source、解析、锁或 dump 错误立即向上返回。

## 数据与状态

`ExtractHandle` 的持久状态只有共享数据源和串行锁；任务、记录和 package 都是单次调用内的值。锁仅包围 `statement_records`、表解析、过滤和去重，不覆盖 binary plan 解码、视图展开或 dump，这与源码注释所述 Go `collectRecords` 锁范围一致。

`BTreeMap<StatementKey, StatementRecord>` 的键同时包含 SQL digest 与 plan digest；只有两者都相同才覆盖。`BTreeSet<TableNamePair>` 的身份还包含 `is_view`，因此数据库名、表名相同但 view 标志不同会被视为不同值。截断记录仍保留在 `records` 中并带 `skipped` 标记，但不会把其表加入 `package.tables`。

视图递归有两套集合：每次生产 `view_dependencies` 调用新建 `visited`，用于阻止同一依赖链中的视图环；`dependencies` 汇总该根视图解析出的所有直接和间接依赖。AST visitor 自身还维护 `cte_names`，用来排除 CTE 名被误报成真实表。文件名生成使用进程内 `AtomicU64` 序列号（`Relaxed`）混合当前纳秒时间；它提供并发调用下的区分度，但不是安全随机标识。

## 依赖与调用关系

上游生产链为 `pkg/server/http_status.rs` 的状态服务路由 → `pkg/server/extract.rs` 的 HTTP 适配 → `pkg/server/extract_runtime.rs::CanonicalExtractRuntime` → 本文件 `ExtractHandle::extract_task`。RustCodeGraph 将 `pkg/domain/extract.rs` 识别为被 10 个文件使用；精确源码检索确认生产构造点是 `pkg/server/extract_runtime.rs:444`，生产执行点是同文件 `:477`，其余直接调用主要位于独立测试。

下游抽象依赖由 `ExtractSource` 明确列出。生产 `ProductionExtractSource` 位于 `pkg/server/extract_runtime.rs`，负责实际摘要/InfoSchema/plan 解码/dump 行为；本文件的 `DomainAstExtractSource` 额外依赖 `crate::domain::Domain::stats_table`、`astersql-parser` 的 `New`/`ParseOneStmt`，以及 `astersql-parser-ast` 的 `Node`、`SelectStmt`、`CreateViewStmt`、`TableName`、`Walk` 和 `InPlaceVisitor`。`pkg/domain/Cargo.toml` 将本 crate 定义为 `astersql-domain`，并显式依赖 `astersql-parser`、`astersql-parser-ast`；`zip` 依赖存在于 crate 边界，但本文件的可执行实现并不直接调用它。

## 错误处理与边界

- 所有领域边界错误统一为 `Result<_, String>`。source 查询、查表、计划解码、dump 以及 parser 错误都用 `?` 原样传播；parser 错误额外加前缀 `parse view definition:`。
- `serial.lock()` 中毒会转为固定错误 `extract worker lock poisoned`，不会 panic。
- history view 未开启持久化摘要时，在读取记录前失败，错误文本与 Go 保持一致。
- 内部库判断不区分大小写，范围固定为 `performance_schema`、`information_schema`、`metrics_schema` 和 `mysql`。这个过滤发生在摘要记录初始表解析阶段；递归视图依赖没有再次调用 `is_internal_schema`，测试明确期望嵌套视图引用的 `mysql.user` 出现在最终表集合中。
- 表缺失会丢弃整条摘要记录；但递归展开期间视图本身消失、条目不是视图或依赖表消失会使整个任务报错，而不是只跳过该依赖。
- 反向时间窗不会在本文件本地拒绝，而是照常传给 source；`reversed_window_is_forwarded_like_go_instead_of_rejected_locally` 固定了这一兼容行为。
- AST CTE 排除按规范化后的表名与 CTE 名比较。该实现不在本地验证空 SQL、空 digest 或空 binary plan；其中只有 plan digest、schema 和语句类型属于 `is_valid` 条件，其余交给 source 或后续步骤处理。
- 后台任务当前仅返回 `Ok(None)`，不会入队；server 层随后用 `unwrap_or_default` 将它转换为空文件名。

## 并发与资源生命周期

`ExtractSource: Send + Sync` 且由 `Arc` 持有，允许 handle 被并发调用或共享生产资源。`Mutex<()>` 只串行化收集/过滤/去重区间，guard 在块结束时自动释放；耗时的解码、递归解析和 dump 不占锁。该范围应谨慎保持，否则扩大锁会降低并发度，缩小锁则可能偏离 Go worker 对摘要收集的互斥语义。

视图递归全部在调用栈内同步完成；`visited` 的生命周期限定在单个根视图的 `view_dependencies` 调用，环路会返回已有依赖而不无限递归。本文件不创建线程、异步任务或通道，也不直接打开 zip、文件或外部存储句柄；资源关闭责任属于 `ExtractSource::dump_package` 的实现。`AtomicU64` 是唯一进程全局可变状态，使用 `Relaxed` 只要求原子唯一递增，不承担跨字段同步。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/domain/extract.go`，对应测试为 `pkg/domain/extract_test.go`。Rust 保留了 `ExtractPlanType`、任务默认值、history-view 配置门槛、`collectRecords` 的串行与 digest-pair 去重、`checkRecordValid` 三项过滤、截断 SQL 路由、binary plan 换行裁剪、视图依赖扩展以及产物目录/文件名用途。

主要结构差异是 Rust 将 Go 中绑定 session/domain/外部存储的 worker 拆为 `ExtractSource` trait，并把具体 I/O 放在 server 的生产 source 中；`DomainAstExtractSource` 则专门保证生产视图定义来自当前 Domain InfoSchema。Go 通过 `tableNameExtractor` 与 restricted SQL parser 展开视图；Rust 直接调用 `astersql-parser` 并遍历真实 AST，独立测试进一步覆盖嵌套子查询、普通 CTE、递归 CTE、嵌套视图和视图环。

Rust 当前 `ExtractType` 只有 `Plan`，没有 Go `taskTypeToString` 的 `Unknown` 分支，也没有运行时未知任务类型错误。Go 的随机文件名使用 16 个随机字节加纳秒时间；Rust 使用纳秒时间与原子序列混合，目标是并发唯一性而非密码学随机性。Go 文件直接创建 zip 并 dump config/meta/schema/stats/SQL；Rust 本文件只编排并调用 `dump_package`，因此不能从本文件单独推断具体 zip 条目已完整实现，必须继续查看生产 `ExtractSource`。

## 扩展指南

- 新增任务类型时，应同时扩展 `ExtractType`、`as_str`、任务构造器和 `ExtractHandle::extract_task` 的显式分派；不能依赖当前“只有 Plan”的隐含路径。同步在 `pkg/domain/extract_test.rs` 建立非 Plan 分支测试，并核对 Go 对应增量。
- 修改摘要过滤或去重时，优先改 `StatementRecord::is_valid` 或收集块；保持 `(digest, plan_digest)` 键与后写覆盖语义，测试应继续独立放在 `pkg/domain/extract_test.rs`，重点覆盖内部库、缺失表、空 schema/plan digest 和重复键。
- 修改视图语义时，入口是 `view_dependencies_from_sql`、`TableDependencyVisitor` 与 `DomainAstExtractSource::collect_view_dependencies`。必须同步覆盖嵌套查询、未限定 schema、CTE 遮蔽、递归 CTE、嵌套视图、循环依赖、视图/依赖消失和 parser 错误；注意当前内部库只在初始记录过滤，改变递归依赖过滤会产生兼容差异。
- 修改并发策略时，明确评估锁范围、source 的 `Send + Sync` 契约及 dump 是否允许并发。不要把文件/zip 资源生命周期搬入本文件而没有为失败关闭路径增加测试。
- 修改产物格式或 stats 开关时，应实现于生产 `ExtractSource::dump_package`，本文件只负责传递 `ExtractTask::skip_stats` 和 package；同时核对 `EXTRACT_*` 协议常量、server 生产 source 及 Go zip 格式。
- 修改文件名算法时，保留并发唯一性并同步测试 `generate_extract_file_name`；若产物名成为安全边界，当前时间戳加 `Relaxed` 序列方案不足，需引入明确的安全随机源并评估外部兼容性。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/domain` 确认目标文件有 46 个符号；`node --file pkg/domain/extract.rs --offset 1/501` 读取了完整 993 行目标实现；`node extract.rs::ExtractHandle` 确认类型位置及来自 `extract_test.rs` 的引用。精确 callees 命令发生名称歧义，因此没有把其泛化结果作为调用事实。
- Rust 源与入口：`pkg/domain/extract.rs`、`pkg/domain/lib.rs`、`pkg/server/extract_runtime.rs:434-479`、`pkg/server/extract.rs:16-58`、`pkg/server/http_status.rs:16-29`。
- Crate 边界：`pkg/domain/Cargo.toml` 的 `[package]`、`[lib]` 与 `astersql-parser` / `astersql-parser-ast` 依赖。
- Rust 独立测试：`pkg/domain/extract_test.rs`，覆盖真实 AST 的嵌套查询与 CTE、Domain InfoSchema 的递归视图/环、Go 过滤条件、截断与重复记录顺序、反向时间窗及 history-view 开关。
- Go 对照：`pkg/domain/extract.go`；Go 集成行为证据：`pkg/domain/extract_test.go` 的当前摘要、持久化历史摘要配置错误和成功导出场景。
- 本任务是纯文档分析，未修改 Rust/Go/Cargo，按任务约束未运行 Cargo。交付前另运行任务指定的 11 章节结构验证，并人工确认本文区分了注释草稿、领域编排与生产 source 的真实职责。
