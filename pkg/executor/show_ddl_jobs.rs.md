# [`pkg/executor/show_ddl_jobs.rs`](./show_ddl_jobs.rs)

## 文件定位

本文件位于 `astersql-executor` crate，crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/lib.rs` 通过 `pub mod show_ddl_jobs;` 公开模块，并仅在测试构建中用 `#[path = "show_ddl_jobs_test.rs"]` 挂载独立测试。它承载 Go 文件 `pkg/executor/show_ddl_jobs.go` 中 `ADMIN SHOW DDL JOBS` 执行器的数据模型、分页检索和结果行/Comments 编码的 Rust 迁移。

当前生产接线状态必须与算法实现分开理解：仓库内 Rust 搜索只在本文件发现 `ShowDDLJobsBackend`、`DDLJobRetriever` 和 `ShowDDLJobsExec`，没有任何生产后端实现或执行器构造调用；Rust 独立测试也只直接覆盖 `showCommentsFromJob` 与 `showCommentsFromSubjob`。相对地，Go 版本在 `pkg/executor/builder.go` 构造 `ShowDDLJobsExec`。因此本文件目前提供公开、可实例化的抽象实现，但不能据现有证据声称已经进入 Rust SQL 执行主链。

## 核心职责

1. 用 `DDLAction`、`DDLJob`、`SubJob`、`BinlogInfo`、`ReorgMeta` 等 Rust 数据结构表达展示 DDL 作业所需的快照，而不是直接依赖其他 crate 的具体 DDL/Meta 类型。
2. 用 `ShowDDLJobsBackend`、`LastJobIterator`、`ShowDDLChunk` 和 `DDLPrivilegeChecker` 隔离系统会话、事务、运行中/历史作业、InfoSchema 名称解析、权限校验及结果 Chunk 写出。
3. `DDLJobRetriever::initial` 根据谓词决定是否跳过运行中或历史来源；`ShowDDLJobsExec::Next` 先分页输出运行中作业，再以 `jobNumber` 为历史作业上限补齐结果。
4. `DDLJobRetriever::appendJobToChunk` 恢复兼容名称、转换时间、可选地做权限过滤，并把父作业和多 schema 子作业编码为 13 列结果。
5. `showCommentsFromJob` 和 `showCommentsFromSubjob` 生成 analyze、回填方式、DXF/cloud、重组参数及 RU 标签，并保持 Go 测试所定义的标签顺序和默认值抑制规则。

## 主要符号

- `DDLAction`：动作展示名及 rename、多 schema、add index、add primary key 分类标志；`Default` 便于测试构造。
- `AnalyzeState`：`None`、`Running`、`Failed`、`Timeout` 四种统计收集状态，对应空标签或 `analyzing`、`analyze_failed`、`analyze_timeout`。
- `ReorgType`：保存具体展示字符串的 `Txn`、`Ingest`、`TxnMerge`、`Other` 以及 `None`；私有 `label` 给子作业 Comments 使用。父作业逻辑有意忽略 `Other`。
- `ReorgMeta`：包含 analyze 状态、回填方式、DXF/cloud 标志以及 concurrency、batch size、写速率、service scope、最大节点数。
- `DDLJob`：父作业展示快照，含 ID/名称/状态/时间戳、binlog 完成信息、重组元数据、子作业、查询文本和 RU。
- `DDLJobPredicates`：按列名保存字符串集合；`initial` 只解释 `state`、`db_name`、`table_name`。
- `DDLRuntimeConfig`：提供 next-gen、RU 版本和三项默认重组参数，使 Comments 逻辑可测试且不直接读取全局变量。
- `ShowDDLChunk`：列式结果写入协议；列索引 0..12 分别是 job ID、库名、表名、动作、schema state、schema ID、table ID、row count、create time、start time、finish time、job state、comments/query。
- `ShowDDLJobsBackend`：生产边界，关联 `Context/Error/Session/Transaction/Time/Role/HistoryIterator`，并要求显式实现会话、事务、数据源、配置、名称和时间转换；源码注释明确没有脱离生产环境的默认实现。
- `DDLJobRetriever<I,T,R>`：持有运行中列表、历史迭代器、全局游标、活跃角色、可复用历史缓存、时区占位和谓词。`TZLoc` 当前未被算法读取，时间转换完全委托给 backend。
- `ShowDDLJobsExec<B>`：执行器状态，包含 backend、retriever、历史条数上限和被借出的系统 session；公开生命周期方法为 `Open`、`Next`、`Close`。
- `appendCommonJobColumns`：写入前 12 列并根据时间戳是否存在写入时间或 NULL；Comments 第 13 列由调用者追加。
- `showCommentsFromJob`、`showCommentsFromSubjob`、`ts2Time`、`getSchemaName`、`getTableName`：公开辅助函数；后三者只是 backend 的薄委托/缺失值归空串。

## 执行流程

`Open` 的顺序是：调用 `open_base`；当 `jobNumber == 0` 时读取默认历史条数；取得系统 session 并保存在 `sess`；在该 session 上创建新事务、取得 active transaction、设置 in-transaction 标志；最后把 transaction 与 session 交给 `DDLJobRetriever::initial`。

`initial` 首先解析谓词。若存在 `state` 集合，它先假设两个来源都跳过：遇到 `cancelled` 或 `synced`（不区分大小写）即保留历史来源，遇到任何其他状态即保留运行中来源。空 state 集合会同时跳过两者。`db_name` 和 `table_name` 集合只传给历史迭代器；运行中作业始终由 backend 返回后在上层继续处理。随后按上述决定调用 `running_jobs` 和/或 `history_iterator`，并把游标归零。

每次 `Next` 先把请求 Chunk 扩到 backend 的最大尺寸并清空。若已消费的历史数量 `cursor - running_count`（Rust 用 `saturating_sub` 防止无符号下溢）达到 `jobNumber`，立即返回空批次。否则：

1. 若游标仍位于运行中列表，最多写入 Chunk capacity 条，并推进全局游标；运行中作业不计入 `jobNumber` 历史限额。
2. Chunk 尚有空间且历史迭代器存在时，按剩余 Chunk 容量和剩余历史限额的较小值调用 `get_last_jobs`。旧 `cacheJobs` 通过 `mem::take` 交给迭代器复用，再逐条编码并按实际返回数推进游标。
3. 单条编码先按 binlog 最终信息、rename 新表名、多表名列表、作业自身名称、最后 InfoSchema ID 回查的优先级恢复库表名；再可选进行小写库表名权限校验；随后写公共列与 Comments/query。

多 schema 作业还会对每个 `SubJob` 写一行，动作后缀为 ` /* subjob */`，父作业的 create/finish time 与 schema/table ID 被复用，start time、row count、schema/job state 来自子作业。必须注意当前 Rust 的实际追加顺序：先写父作业前 12 列，再写所有子作业的 13 列，最后才追加父作业第 13 列；Go 版本先补全父行再追加子行。对列式 Chunk 来说，这会令 Comments 列与其他列的行序存在错位风险，现有 Rust 测试没有覆盖该路径。

`Close` 从 `sess` 中 `take` 出系统 session 并释放，然后调用 `close_base`。重复调用时不会重复释放 session，但仍会再次调用 `close_base`。

## 数据与状态

- `runningJobs` 是 `Open/initial` 阶段拍下的运行中作业列表；`historyJobIter` 是基于当前 transaction 创建的惰性历史来源。
- `cursor` 是跨 `Next` 调用的单一游标：前 `runningJobs.len()` 个位置对应运行中列表，之后的位置表示已经拉取的历史作业数量。
- `jobNumber` 仅限制历史作业；默认值由 `default_history_job_count` 给出。显式值 0 会被解释成“使用默认值”，不能表达“不要历史作业”。
- `cacheJobs` 用作迭代器输入/输出缓存，避免每批都丢弃 Vec 分配；错误发生时，因旧 Vec 已被 `mem::take` 移出，retriever 中会暂时保留空 Vec。
- 时间戳为 `u64`：`start_ts` 总是转换并写入 create time；`real_start_ts == 0` 和 `finished_ts == 0` 分别产生 NULL。转换精度、时区和零时间行为由 backend 的 `timestamp_to_time` 决定。
- 名称缺失最终归为空串；多表名用逗号连接且不插入空格。
- Comments 标签具有稳定顺序：analyze → reorg 类型 → DXF → cloud → 非默认重组参数 → RU。RU 仅在 next-gen、RU v2、状态为 synced（ASCII 大小写无关）且 `ru > 0` 时以两位小数输出。
- 文件没有条件编译项；crate 的 `nextgen` feature 在 `Cargo.toml` 中只转发给 `astersql-dxf-importinto/nextgen`，本文件自身用运行时 `DDLRuntimeConfig.next_gen` 分支。

## 依赖与调用关系

本文件的直接语言依赖只有 `std::collections::{HashMap, HashSet}`；数据库能力均经本文件 trait 注入。Cargo 层面它属于依赖 DDL、Meta、InfoSchema、KV、session/sessiontxn、privilege、planner、types、chunk 等众多内部 crate 的 `astersql-executor`，但当前文件没有直接 import 这些具体 crate，说明它是一个尚待生产适配器落地的隔离边界。

RustCodeGraph 对精确 Rust 定义给出的关键下游边包括：

- `initial` → `ShowDDLJobsBackend::running_jobs`、`history_iterator`；
- `appendJobToChunk` → `request_verification`、`runtime_config`、`appendCommonJobColumns`、`showCommentsFromJob`、`showCommentsFromSubjob`、`ts2Time`、`getSchemaName`、`getTableName`；
- `appendCommonJobColumns` → `ShowDDLChunk::{append_i64, append_string, append_time, append_null}`；
- `showCommentsFromJob`/`showCommentsFromSubjob` → `ReorgType::label`（图索引对通用字符串方法还产生跨文件误配，不能据此推导业务依赖）。

上游方面，`pkg/executor/lib.rs` 是模块装配入口，`pkg/executor/show_ddl_jobs_test.rs` 是已找到的唯一 Rust 调用者；全仓 Rust 文本搜索未发现 production backend 实现或执行器构造。Go 的真实上游是 `pkg/executor/builder.go`，真实下游则包括 session transaction、DDL 运行中查询、Meta 历史迭代器、InfoSchema 和 Chunk。

## 错误处理与边界

- `Open`、`Next`、`Close` 和 `initial` 原样传播 backend/迭代器的统一关联错误类型，不包装上下文。
- `Open` 在取得 session 后的 `new_transaction`、`transaction` 或 `initial` 失败时不会在本方法中释放 session，也没有 `Drop` 兜底；调用框架必须保证失败路径仍执行 `Close`，否则存在系统 session 生命周期泄漏风险。现有 Rust 代码与测试未证明该保证。
- `Close` 先释放 session，再执行 `close_base`；`release_system_session` 无返回值，因此无法表达释放失败。它也没有显式清除 session 的 in-transaction 标志，是否由释放流程重置属于 backend 契约，当前未验证。
- `appendJobToChunk` 的权限校验是可选参数；`Next` 明确传 `None`，所以 `ADMIN SHOW DDL JOBS` 路径不在此处过滤。供其他展示路径复用时，校验失败是静默跳过而非错误。
- Rust `DDLPrivilegeChecker` 只接收角色、库名、表名；Go 的调用还传入空列名及 `mysql.AllPrivMask`。生产适配时必须确认 Rust trait 的实现确实等价于 Go 的全权限掩码语义。
- `expect("the acquired system session is retained until Close")` 位于刚写入 `Some` 之后，正常控制流不会 panic；若以后在两者之间增加会清空 `sess` 的逻辑，该不变量需同步维护。
- 历史上限、Chunk 容量、游标差值均使用 `min`/`saturating_sub`，避免整数下溢；但若迭代器在请求数为 0 时仍被调用，其行为由实现者负责。
- 旧作业可能没有名称，按 ID 回查失败时输出空串而非报错，这是与 Go 兼容的展示降级策略。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、原子变量或通道。所有状态通过 `&mut self` 串行推进，类型自身也未声明额外的 `Send`/`Sync` 保证；调用者不应并发调用同一个执行器实例。

核心资源生命周期是 `Open` 获取系统 session 和 transaction，多个 `Next` 共享该快照/历史迭代器，`Close` 释放 session 并关闭基执行器。transaction 未作为字段保存，而是被移动进 `history_iterator`；其实际持有期与提交/回滚策略由 `B::Transaction`、`B::HistoryIterator` 的具体实现决定。由于当前没有 production backend，无法从本文件验证事务终止语义。

运行中作业列表在 `Open` 时一次性加载，后续 `Next` 不刷新；历史迭代器也绑定同一打开阶段建立的事务。这提供分页期间的一致资源边界，但运行中部分是否严格快照取决于 backend 返回对象是否为独立值；当前 `Vec<DDLJob>` 模型本身是所有权快照。

## 与 Go 版本的对应关系

主要一一对应关系为：Go `ShowDDLJobsExec` ↔ Rust `ShowDDLJobsExec<B>`，Go `DDLJobRetriever` ↔ Rust 泛型 retriever，Go `model.Job`/`DDLReorgMeta`/`SubJob` ↔ Rust 本地展示模型，Go `chunk.Chunk` ↔ `ShowDDLChunk`，Go session/Meta/InfoSchema 调用 ↔ `ShowDDLJobsBackend`。

已经由独立 Rust 测试对齐的语义包括：RU 的 next-gen + RU v2 + synced + 正值门槛和两位小数；analyze 标签；txn/txn-merge/ingest、DXF、cloud 标签；非默认线程数、batch size、写速率、service scope、最大节点数；next-gen 下 add-index 父作业只保留 analyze/RU、子作业 Comments 为空。对应证据是 `pkg/executor/show_ddl_jobs_test.rs` 与 Go 的 `pkg/executor/show_ddl_jobs_test.go`，Go 集成覆盖还见 `pkg/executor/test/admintest/show_ddl_jobs_test.go`。

仍未完全对齐或未验证的点：

- Go 已在 `builder.go` 生产接线并实现 `exec.Executor`，Rust 未发现 backend 实现、builder 构造或统一 Executor trait 实现。
- Go `ddlJobRUEnabled` 从 Domain/内核模式取得开关；Rust 要求 backend 直接提供完整 `DDLRuntimeConfig`。
- Go `ts2Time` 明确执行 TSO 转换、按默认 FSP 截断和时区转换；Rust 把全部细节下放给 backend，当前无实现可验证。
- Go 在父作业 Comments 写完后才写子作业；Rust 当前先写子作业 Comments，存在列式行错位风险。
- Go rename 参数解析可能返回错误并静默回退；Rust 模型把已解析的新表名作为 `Option<String>` 输入，不承担参数解码。
- Go 的 `showCommentsFromSubjob` 会展示所有非 None `ReorgTp.String()`；Rust `ReorgType::Other(label)` 同样可经 `label` 展示。父作业则只识别 Txn/Ingest/TxnMerge。
- Rust 测试覆盖 Comments 纯函数，但没有覆盖 `Open/Next/Close`、谓词裁剪、名称回退、权限过滤、分页、错误清理或多 schema 行布局。

## 扩展指南

- 接入生产执行链时，优先新增独立 adapter/backend 文件实现 `ShowDDLJobsBackend`、Chunk/权限/历史迭代器 trait，并在 builder 中构造 `ShowDDLJobsExec`；不要把具体 session、Meta、InfoSchema 类型重新耦合回本文件。同步新增独立 `*_test.rs`，不要把测试内嵌进生产源文件。
- 修改分页或谓词规则时，重点覆盖 state 仅含终态、仅含运行态、混合集合、空集合、无 extractor，及 running 数量超过 Chunk 容量/历史限额等情形；Go 基准逻辑在 `DDLJobRetriever.initial` 和 `ShowDDLJobsExec.Next`。
- 修改结果列时，必须同时更新 `appendCommonJobColumns`、父/子 Comments 追加位置以及消费端 schema；首先应增加列式 Chunk fake 的回归测试，确认多 schema 父行和每个子行的第 13 列对齐，再决定是否修正当前顺序。
- 修改 Comments 时保持标签顺序和默认值抑制，扩展 `pkg/executor/show_ddl_jobs_test.rs` 的矩阵，并与 `pkg/executor/show_ddl_jobs_test.go`、`pkg/executor/test/admintest/show_ddl_jobs_test.go` 对照。尤其要覆盖 AnalyzeState::Failed/Timeout、非 add-index 作业、负 RU、`Other` reorg type。
- 修改资源生命周期时，应测试 `open_base`、获取 session、建事务、取 transaction、初始化各阶段失败后的关闭/释放次数，以及重复 `Close`。若加入自动清理，需避免与显式 `Close` 双重释放。
- 若实现 `timestamp_to_time` 或权限适配器，必须分别复刻 Go 的 FSP/时区规则和 `AllPrivMask` 语义，并通过端到端结果测试，而不能只以 trait 可编译作为完成证据。
- 性能方面，保留运行中列表批量写出、历史迭代器按需拉取和 `cacheJobs` 容量复用；不要为每个历史作业重新开启事务或全量加载历史表。

## 验证依据

- Rust 源码：`pkg/executor/show_ddl_jobs.rs`（604 行），核对了全部结构体、枚举、trait、impl、函数和实际分支；文件无 `cfg` 条件编译项。
- crate/模块：`pkg/executor/Cargo.toml` 的 package、lib、`nextgen` feature 与 porting 元数据；`pkg/executor/lib.rs` 的公开模块声明和独立测试挂载。
- Rust 测试：`pkg/executor/show_ddl_jobs_test.rs` 的 4 个测试，覆盖 RU、父作业标签矩阵、analyze/next-gen 和子作业 DXF/cloud。
- Go 对照：`pkg/executor/show_ddl_jobs.go` 的 `Open`/`Next`/`Close`、`initial`、`appendJobToChunk` 及 Comments/时间/名称辅助；`pkg/executor/builder.go` 的两个构造位置；`pkg/executor/show_ddl_jobs_test.go` 的 3 个测试；`pkg/executor/test/admintest/show_ddl_jobs_test.go` 的 next-gen RU 集成测试。
- RustCodeGraph：索引状态为 11,467 files / 307,296 nodes / 1,848,419 edges；使用 `node --file pkg/executor/show_ddl_jobs.rs` 读取 1..604 行，并查询了 `ShowDDLJobsExec`、`DDLJobRetriever`、`showCommentsFromJob`、`showCommentsFromSubjob`、`appendJobToChunk`、`initial`、`appendCommonJobColumns` 的节点和 callers/callees。通用 `Open`/`Next`/`Close` 因全仓同名符号过多无法可靠消歧，所以生命周期调用关系以精确文件节点为依据，并明确没有把噪声图边当成事实。
- 接线搜索：全仓 `*.rs` 对 `ShowDDLJobsExec|ShowDDLJobsBackend|DDLJobRetriever|showCommentsFromJob|showCommentsFromSubjob` 的扫描仅命中本源文件和独立测试；Go 搜索命中 `pkg/executor/builder.go` 与本 Go 对照文件。
- 结构验收使用任务指定命令，要求目标存在且恰有本页 11 个固定二级标题；本任务按计划不运行 Cargo。
