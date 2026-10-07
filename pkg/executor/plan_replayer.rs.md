# `pkg/executor/plan_replayer.rs`

源码：[`pkg/executor/plan_replayer.rs`](plan_replayer.rs)

## 文件定位

`plan_replayer.rs` 位于 `astersql-executor` crate；`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认 crate 根，`pkg/executor/lib.rs` 以 `pub mod plan_replayer` 对外公开本模块。它把 PLAN REPLAYER 的三类动作抽象成与具体会话、归档和结果集实现无关的 Rust 流程：按 digest 注册/移除 Capture 任务，导出 DUMP 现场，以及恢复 LOAD 现场。

该文件处于“执行器流程与后端能力边界”这一层，而不是完整运行时实现层。`pkg/executor/builder.rs::buildPlanReplayer` 会解析 DUMP 中的 SQL，并把计划及语句交给 `ExecutorBuilderDependencies::build_plan_replayer_executor`；但仓库生产 Rust 代码中尚无 `PlanReplayerBackend` 的实现，现有两个实现都在独立测试文件 `pkg/executor/plan_replayer_test.rs` 和 `pkg/executor/test/planreplayer/plan_replayer_test.rs`。因此当前代码已经表达并测试核心控制流，但真实会话存取、内部 SQL、ZIP 解析、统计加载和文件传输仍须由未来的生产后端接线。

## 核心职责

- `PlanReplayerBackend` 定义所有有副作用的能力：结果集写入、捕获任务持久化、文件创建与关闭、读时间戳、SQL 解析、归档读取、schema/统计/绑定恢复以及 warning 记录。主流程只编排这些能力。
- `PlanReplayerExec::Next` 实现 Capture 与 DUMP 的单次执行生命周期，并用 `end` 防止成功完成后重复产出。
- `PlanReplayerLoadExec::Next` 校验 LOAD 路径并将文件传输准备委托给后端。
- `updateLoadInfo` 固化恢复现场的依赖顺序：变量、禁用自动分析、表、TiFlash 副本、视图、统计、Binding。
- `appendPlanReplayerDumpResult` 将本地文件 token 或远端预签名 URL 转换为两列表格结果；`isPlanReplayerDownloadURL` 负责二者判别。
- `DumpSQLsFromFile`、`loadPlanReplayerForExplainExplore` 和一组薄包装函数保留与 Go `pkg/executor/plan_replayer.go` 对应的可复用入口。

本文件不直接实现 SQL 执行、ZIP 格式细节或数据库状态变更；这些都属于 `PlanReplayerBackend` 的实现责任。

## 主要符号

- `PLAN_REPLAYER_DUMP_VAR_KEY` / `PLAN_REPLAYER_LOAD_VAR_KEY`：字符串形式的会话状态键，值分别为 `plan_replayer_dump_var` 与 `plan_replayer_load_var`。当前文件只声明，生产 Rust 调用处尚未出现。
- `PlanReplayerCaptureInfo { sql_digest, plan_digest, remove }`：Capture 操作的不可变输入；`remove` 决定注册还是删除。
- `PlanReplayerDumpInfo<S, F>`：DUMP 的可变状态，包含待导出语句、`analyze`、历史统计时间戳、语句读取时间戳、外部 SQL 文件路径、打开的文件和文件名。泛型 `S`、`F` 由后端决定具体语句及文件类型。
- `PlanReplayerLoadInfo { path }`：LOAD 文件路径。与 Go 类型相比，Rust 版本不直接持有 session context，context 由函数参数传入。
- `PlanReplayerBackend`：核心端口 trait。六个关联类型隔离 context、request、statement、file、archive 和 error；其方法分成结果集操作、Capture、DUMP、LOAD/归档恢复四组。
- `PlanReplayerExec<B>`：Capture/DUMP 执行器，`capture_info` 与 `dump_info` 表示互斥模式，`end` 表示成功完成。代码依赖构建者维持“至少且仅有一个模式输入”的不变量；DUMP 缺少 `dump_info` 会触发 `expect`。
- `PlanReplayerLoadExec<B>`：LOAD 执行器；它没有 `end` 字段，是否拒绝重复准备由生产后端/会话接线负责。
- `handleLoadStats` / `handlePlanReplayerLoad`：空字节直接成功，否则分别调用后端统计加载或完整归档恢复。
- `loadPlanReplayerForExplainExplore` / `extractPlanReplayerTargetSQL`：前者读取文件、先提取目标 SQL、再恢复环境；后者只打开归档并取目标 SQL。
- `loadSetTiFlashReplica`、`loadAllBindings`、`loadBindings`、`loadVariables`、`disableAutoAnalyzeForPlanReplayerLoad`、`createSchemaAndItems`、`loadStats`、`createTable`：与 Go 函数同名或近似同名的委托入口。真正行为由 trait 方法实现；`loadBindings` 的 `_session` 当前未参与分派。
- `isNoTiFlashStoreErr`：以错误文本是否包含 `total tiflash server count: 0` 判断无 TiFlash 节点情形。

## 执行流程

### Capture / DUMP

1. `PlanReplayerExec::Next` 总是先调用 `grow_and_reset` 清空并准备结果容器；若 `end` 已为真则不再执行后端动作。
2. 若存在 `capture_info`，根据 `remove` 调用 `remove_capture_task` 或 `register_capture_task`。只有后端返回成功时才设置 `end = true`，失败保留可重试状态并直接传播错误。
3. 否则进入 DUMP，要求 `dump_info` 存在。先 `create_dump_file`，把文件与名称写入状态，再调用 `statement_read_timestamp`。读取时间戳失败会取走并关闭刚创建的文件。
4. 若 `dump.path` 非空，表示 SQL 文件传输模式：调用 `prepare_dump_file_transfer`，成功后只标记结束，不在此处执行 dump；失败则关闭文件。对应 Go `PlanReplayerExec.Next` 中由连接层稍后读取文件并调用 `DumpSQLsFromFile` 的设计。
5. 普通 DUMP 要求 `statements` 非空；为空时构造 `empty_sql_error` 并关闭文件。随后调用后端 `dump`；失败同样关闭文件。
6. DUMP 成功后，`appendPlanReplayerDumpResult` 将 token 写入 request。预签名 URL 生成 `Download URL`、`Expires in`、浏览器提示、`curl` 示例和过期提示五行；普通 token 生成单行 `File token`。最后设置 `end = true`。

`DumpSQLsFromFile` 清空旧语句，以分号切分输入，只去掉每段首尾的换行符而保留空格，跳过空段，然后逐条 `parse_sql` 并调用 `dump`。这是对 Go `strings.Trim(sql, "\n")` 的刻意对齐；任一解析失败会停止后续解析和 dump。

### LOAD / EXPLAIN EXPLORE

1. `PlanReplayerLoadExec::Next` 先重置 request；空 `path` 返回后端定义的错误，非空路径调用 `prepare_load_file_transfer`。
2. 连接层获得文件字节后可调用 `handlePlanReplayerLoad`；空文件是 no-op，非空文件进入 `updateLoadInfo`。
3. `updateLoadInfo` 每次重新打开归档，严格依次执行 `load_variables`、`disable_auto_analyze`、`create_tables`、`load_tiflash_replicas`、`create_views`、`load_statistics` 和 `load_bindings`。前六步出错立即终止；Binding 错误被转换为 warning，随后总是追加“自动分析已关闭”的 warning 并返回成功。
4. `loadPlanReplayerForExplainExplore` 拒绝仅含空白的路径，读取全部文件字节，先通过独立归档句柄取得目标 SQL，再以新的归档句柄执行 `updateLoadInfo`，最后返回 SQL。提取或恢复任一步失败均不返回 SQL。

## 数据与状态

- `PlanReplayerExec.end` 是本文件唯一的显式执行状态机：初始为 `false`，Capture、外部文件传输准备或普通 DUMP 成功后转为 `true`；错误路径保持 `false`。再次 `Next` 仍会重置结果容器，但不会重复后端操作。
- `PlanReplayerDumpInfo.file: Option<F>` 表示文件所有权。文件创建后进入 `Some`；时间戳、传输准备、空 SQL 或 dump 失败时通过 `take()` 转移所有权给 `close_dump_file`，从而同时避免二次关闭。成功路径由后端 `dump` 或后续文件传输流程负责最终资源处置，本文件不会主动清空它。
- `start_timestamp` 在创建文件后、实际 dump 前写入，用来固定语句读取视图。`historical_stats_timestamp`、`analyze`、`statements`、`file_name` 由后端消费，本文件不解释归档编码。
- `updateLoadInfo` 中 `create_tables` 返回 `HashSet<String>`，随后传给 `load_bindings`，使后端能只恢复与已创建数据库有关的 Binding。
- 本文件没有全局可变状态；两个字符串键仅是常量。会话变量、任务表、LastPlanReplayerToken、warning 列表等真实状态均隐藏在后端 context 中。

## 依赖与调用关系

上游方面，`pkg/executor/lib.rs` 导出模块；`pkg/executor/builder.rs::buildPlanReplayer` 是已找到的 Rust 构建入口，它在 DUMP 模式先用 `parse_replayer_statement` 解析 `PlanReplayerPlanData::statement_sql()`，然后把计划与语句传给依赖接口 `build_plan_replayer_executor`。仓库当前没有生产实现直接构造本文件的两个执行器，也没有生产 `PlanReplayerBackend` 实现，所以从 builder 到本文件尚不存在完整静态接线。

下游方面，本文件唯一直接标准库依赖是 `std::collections::HashSet`；其余依赖全部倒置到 `PlanReplayerBackend`。重要调用边包括：

- `PlanReplayerExec::Next` → Capture 的 `register_capture_task` / `remove_capture_task`，或 DUMP 的 `create_dump_file` → `statement_read_timestamp` → `prepare_dump_file_transfer` / `dump` → `appendPlanReplayerDumpResult`。
- `appendPlanReplayerDumpResult` → `isPlanReplayerDownloadURL`、`presigned_url_expiration`、`append_string`。
- `handlePlanReplayerLoad` 与 `loadPlanReplayerForExplainExplore` → `updateLoadInfo`。
- `updateLoadInfo` → 归档及恢复方法，并将 `create_tables` 的数据库集合传给 `load_bindings`。

`pkg/executor/Cargo.toml` 将此模块归于 `astersql-executor`，并以 `package.metadata.porting.go-package = "pkg/executor"` 声明 Go 对照包；本文件本身未直接引用 crate 依赖，也不受唯一显式 feature `nextgen` 的条件编译控制。

## 错误处理与边界

- 所有可恢复错误都使用后端关联类型 `B::Error` 原样传播，因此错误文本、堆栈和分类由具体后端定义。
- Capture 失败不会将 `end` 置真。DUMP 文件创建本身失败时没有文件可关闭；创建成功后的时间戳、传输准备、空 SQL、dump 失败均显式关闭并清空 `file`。`pkg/executor/test/planreplayer/plan_replayer_test.rs` 分别覆盖这些关闭路径。
- `dump_info.as_mut().expect(...)`、Capture helper 中的 `capture_info.as_ref().expect(...)` 是构建不变量，不是用户错误处理。错误构造执行器会 panic；生产 builder/backend 接线必须保证模式与 info 一致。
- `isPlanReplayerDownloadURL` 只接受小写 `http://` 或 `https://` 且 authority 非空；它以 `/`、`?`、`#` 截断 authority，使 `http://?signature=...` 不会误判为 URL。它不是通用 URL 解析器，国际化 host、大小写 scheme 等兼容性取决于与 Go `net/url.Parse` 的实际差异。
- `DumpSQLsFromFile` 用 `String::from_utf8_lossy`，无效 UTF-8 会被替换字符代替而非报编码错误；分号是无上下文切分，含字符串字面量分号的复杂输入是否与上游协议相容尚未由本文件验证。
- `handleLoadStats` 和 `handlePlanReplayerLoad` 将空数据视为成功 no-op。`loadPlanReplayerForExplainExplore` 则只预先校验路径是否为空白，文件读取、归档格式和目标 SQL 有效性都由后端报错。
- LOAD 中 Binding 是唯一明确的软失败：记录 warning 后继续；变量、禁用 auto-analyze、schema、TiFlash、view、statistics 均为硬失败。`append_auto_analyze_warning` 仅在主流程走到末尾时调用。
- `isNoTiFlashStoreErr` 是文本匹配工具，容易受上游错误文案变化影响；当前 `updateLoadInfo` 不直接调用它，而由后端的 `load_tiflash_replicas` 决定是否采用 Go 的 hypothetical replica 降级逻辑。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道；全部方法均为同步调用，并通过 `&mut self`、`&mut Context` 和 `&mut Archive` 串行化一次执行器/归档内的状态变更。它既未声明 `Send`/`Sync` 约束，也不保证同一 backend 可跨线程共享。

DUMP 文件是最关键资源：创建后由 `PlanReplayerDumpInfo.file` 持有；已覆盖的错误分支通过 `Option::take` 恰好关闭一次。成功 DUMP、成功外部传输准备和 `DumpSQLsFromFile` 后的关闭职责属于 backend/连接层，新增后端必须明确所有权转移，避免泄漏或重复关闭。

归档生命周期由每次 `open_archive` 返回的局部值限定。EXPLAIN EXPLORE 为目标 SQL 提取和环境恢复分别打开归档，避免复用已被读取/游标推进的归档状态。恢复流程顺序执行，失败不会自动回滚已经完成的变量或 schema 变更；调用方必须把 LOAD 视为可能部分生效的管理操作。自动分析被关闭后，本文件只追加提醒，不自动恢复原值，这与 Go 行为一致但具有持续的集群配置影响。

## 与 Go 版本的对应关系

直接基线是 `pkg/executor/plan_replayer.go`：Rust 的 `PlanReplayerExec`、`PlanReplayerCaptureInfo`、`PlanReplayerDumpInfo`、`PlanReplayerLoadExec`、`PlanReplayerLoadInfo` 以及多数 camelCase 函数都保留了 Go 名称和流程顺序，文件级 `#![allow(non_snake_case)]` 也服务于这种迁移对齐。

已明确对齐的语义包括：

- `Next` 首先 Grow/Reset 结果；Capture 优先于 DUMP；成功后设置结束标志。
- 创建 DUMP 文件后读取 statement read TS；其后的错误路径关闭文件。
- 外部 SQL 文件模式只登记/准备传输，由连接层稍后读取；普通模式立即 dump 并返回 token。
- 多 SQL 输入只 trim 换行，保留空格；测试 `dump_sql_file_only_trims_newlines_like_go` 专门锁定这一点。
- 预签名 URL 返回五行下载说明；无 host 的 HTTP 字符串仍按文件 token 处理。
- LOAD 顺序为 variables → disable auto-analyze → tables → TiFlash → views → stats → bindings；Binding 错误降级为 warning。
- EXPLAIN EXPLORE 先提取 `sql/sql0.sql` 目标 SQL，再恢复重放环境。

Rust 当前是能力压缩后的抽象，而不是 Go 实现的逐函数完整落地。Go `PlanReplayerBackend` 等价逻辑分散在 session、domain、ZIP、parser、statistics 等真实组件中；Rust 将它们集中为 trait 方法。特别是 Go `PlanReplayerLoadInfo.createTable` 临时关闭外键检查、忽略 placement 并在 defer 中恢复，`loadSetTiFlashReplica` 在无 TiFlash 时建立 hypothetical replica，以及 `loadStats` 对无统计/null JSON 与 warning 的处理，都必须由未来 backend 保留，本文件本身无法证明已经实现。Go 的 typed session keys 和 `FileTransInConnHandlers` 映射在 Rust 这里只剩字符串常量与处理函数，生产连接层注册尚未找到。

另有一个需谨慎的签名差异：Go `loadBindings` 根据 `isSession` 生成 SESSION/GLOBAL Binding，Rust `loadBindings` 的 `_session` 参数当前被忽略并统一调用 `backend.load_bindings`；后端接口也没有 session 参数。若生产后端无法从归档自身区分两类 Binding，这会成为迁移缺口，不能仅靠现有 mock 测试判定已对齐。

## 扩展指南

1. 接入生产运行时应首先实现 `PlanReplayerBackend`，并在 `ExecutorBuilderDependencies::build_plan_replayer_executor` 的真实实现中按 Load/Capture/Remove/Dump 模式构造正确执行器。必须同步接通会话状态键、连接层文件传输、LastPlanReplayerToken 与 EXPLAIN EXPLORE hook；不能只让代码编译。
2. 新增 DUMP 阶段时，应在 `PlanReplayerExec::Next` 保持“创建文件 → 固定 read TS → 传输或 dump”的顺序，并为新错误分支证明文件恰好关闭一次。相应测试放在独立的 `pkg/executor/plan_replayer_test.rs` 或 `pkg/executor/test/planreplayer/plan_replayer_test.rs`，不要内嵌到生产文件。
3. 扩展 LOAD 内容时，在 `updateLoadInfo` 中根据依赖关系选择插入点，并决定硬失败还是 warning。schema 依赖项必须早于 view/statistics/binding；可能改写统计的后台动作必须晚于禁用 auto-analyze，或显式说明新不变量。
4. 修改归档格式或目标 SQL 位置时，同时更新 backend 的 `target_sql`/`open_archive`、Go `extractPlanReplayerTargetSQL` 和跨语言测试。应保持路径正规性检查、空目标 SQL、损坏 ZIP、读取/关闭错误等边界。
5. 修改 URL 判别或输出文案时同步 `appendPlanReplayerDumpResult`、`isPlanReplayerDownloadURL` 及 Go 同名函数；现有集成 Rust 测试覆盖 URL、有 scheme 无 host、普通 token 和过期文案。
6. 完善 Binding 对齐前，应明确调整 `PlanReplayerBackend::load_bindings` 的输入，使 SESSION/GLOBAL 信息不会丢失，并新增分别验证两类归档的测试。
7. 性能上，当前文件读取与归档接口使用完整 `Vec<u8>`，多 SQL 解析也把所有语句保存在 `Vec`；若支持超大重放包，应在不破坏 Go 行为和错误顺序的前提下评估流式接口，而不是局部替换造成生命周期不一致。

## 验证依据

- 生产源码：`pkg/executor/plan_replayer.rs`（495 行）完整读取；符号清单、DUMP/LOAD 顺序、错误关闭和 trait 边界均来自该文件。
- crate 与模块：`pkg/executor/Cargo.toml` 的 `[package] name = "astersql-executor"`、`[lib] path = "lib.rs"`、`package.metadata.porting.go-package = "pkg/executor"`；`pkg/executor/lib.rs` 的 `pub mod plan_replayer` 与独立 `mod plan_replayer_test`。
- Rust 上游：`pkg/executor/builder.rs::buildPlanReplayer`、`ExecutorBuilderDependencies::{parse_replayer_statement, build_plan_replayer_executor}`。全仓 `rg` 未找到生产 `PlanReplayerBackend` 实现，只有两处测试实现，因此“尚未完成生产接线”是当前代码事实。
- 单元测试：`pkg/executor/plan_replayer_test.rs` 验证仅 trim 换行、LOAD 先重置结果、Capture helper 成功后结束。
- 集成语义测试：`pkg/executor/test/planreplayer/plan_replayer_test.rs` 验证 Capture 生命周期、DUMP token、时间戳/prepare/dump/空 SQL 失败时关闭文件、预签名 URL、50 条 SQL、LOAD 顺序与软失败、建库错误优先、EXPLAIN EXPLORE、空路径和无效 SQL。
- Go 对照：`pkg/executor/plan_replayer.go` 完整函数列表及关键实现，尤其 `PlanReplayerExec.Next`、`DumpSQLsFromFile`、`PlanReplayerLoadInfo.Update`、`createTable`、`loadSetTiFlashReplica`、`loadBindings`、`loadStats`；`pkg/executor/builder.go::buildPlanReplayer` 验证实际 Go 模式构造。
- RustCodeGraph：执行 `status` 确认索引可用（11,467 个文件），但 `files --filter pkg/executor/plan_replayer` 返回无匹配，精确目标未被索引；因此没有采用宽泛 `explore` 返回的无关符号，调用边改由上述 `rg` 与源码核验。该限制只影响图索引证据，不影响直接源码事实。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以结构检查和人工事实复核验收。
