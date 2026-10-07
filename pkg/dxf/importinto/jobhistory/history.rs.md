# `pkg/dxf/importinto/jobhistory/history.rs`

## 文件定位

本文件是 `astersql-dxf-importinto-jobhistory` crate 的核心实现。crate 入口在 `pkg/dxf/importinto/jobhistory/lib.rs`：它以 `#[path = "history.rs"] pub mod jobhistory` 装入本文件，并通过 `pub use jobhistory::*` 对外再导出这里的类型与函数。crate 边界由同目录 `Cargo.toml` 定义，并由根 `Cargo.toml` 的 workspace 成员列表以及 `pkg/dxf/importinto/Cargo.toml` 的 `astersql-dxf-importinto-jobhistory` 路径依赖接入 IMPORT INTO 子系统。

它负责从已经归档的 `mysql.tidb_global_task_history` 与 `mysql.tidb_background_subtask_history` 读取单个 IMPORT INTO 作业，并转换成适合 JSON 展示的汇总信息。Go 应用中的真实入口是 `pkg/server/handler/tikvhandler/dxf.go` 的 `DXFImportIntoHistoryJobInfoHandler.ServeHTTP`：处理器校验 keyspace 和正整数 job ID，取得 DXF `TaskManager`，设置请求超时后调用 Go 同名实现 `jobhistory.GetFromHistory`，把未找到映射为 HTTP 404。当前 Rust 仓库搜索到的 `GetFromHistory` 直接调用者是独立 Rust 测试；尚未发现对应的 Rust HTTP 生产入口，因此不能把 Go 入口描述成已经由 Rust 请求链调用。

## 核心职责

- `GetFromHistory` 只查询历史表，不回退到活动任务表。它先用 keyspace 与 job ID 生成全局任务键，再限制任务类型为 `proto::ImportInto`，避免命中其他分布式任务。
- 它从全局任务历史行提取状态、并发度、节点数和任务 `meta` JSON 中的计划/汇总字段，形成 `Info` 的基础部分。
- 它按 `step + kv_group` 聚合子任务历史：只在 `ImportStepWriteAndIngest` 统计 KV 字节，将 `kv-group == "data"` 归入数据 KV，其余非空聚合行归入索引 KV。
- 它用所有有效子任务窗口的最早开始时间和最晚更新时间计算总耗时，并为每个步骤独立合并窗口，再映射到 `Duration` 的六个阶段字段。
- 它计算平均行长、整体每小时吞吐和每核每小时吞吐，并按 Go 版本使用的字符串格式输出字节与时长。

这些职责都集中在读取和派生展示数据；文件不迁移任务、不更新历史表，也不负责 HTTP 参数校验或响应码选择。

## 主要符号

- `pub struct Duration`：七个字符串字段分别表示总耗时与 encode、merge sort、ingest、collect conflicts、resolve conflicts、post process 阶段耗时。`Default` 使缺少证据的阶段保持空字符串；`serde::Serialize` 与逐字段 `serde(rename = ...)` 固定 Go 兼容 JSON 名称。
- `pub struct Info`：单个作业的展示 DTO。标识与状态字段包括 `JobID`、`Keyspace`、`TaskID`、`State`；计划维度包括任务并发、最大节点数、DistSQL 扫描并发、索引数和列数；统计维度包括文件/KV 大小、两类吞吐、行数、平均行长及嵌套 `Duration`。同样通过 `Serialize` 和显式 rename 保持 snake_case JSON 契约。
- `pub fn GetFromHistory(ctx, mgr, keyspace, jobID) -> Result<Info, Error>`：唯一业务入口，执行两轮 SQL 并聚合 `Info`。
- `pub fn formatDuration(seconds) -> String`：把非负整秒格式化为 `h/m/s` 组合；负值返回空串。
- `pub fn formatBytes(size) -> String`：非负字节值进入二进制单位格式化；负值返回空串。
- `pub fn formatBytesPerHour(...)` 与 `pub fn formatBytesPerCoreHour(...)`：整体吞吐和按 `maxNodeCount * taskConcurrency` 折算的每核吞吐；无效输入返回空串。
- `fn formatBytesValue(f64)` 与 `fn formatFourSignificantDigits(f64)`：内部格式化辅助函数，使用 `bytesize::KIB` 逐级换算 `B` 到 `YiB`，并模拟 Go `docker/go-units` 的约四位有效数字输出；大或极小数使用带符号指数的科学计数法。

文件没有 trait、impl、模块级可变状态或条件编译分支。

## 执行流程

1. `GetFromHistory` 首先调用 `injectfailpoint::DXFRandomErrorWithOnePercent()`；命中时立即返回重试类错误，不访问存储。
2. 用 `taskkey::ForJobInKeyspace(keyspace, jobID)` 构造任务键，调用 `TaskManager::ExecuteSQLWithNewSession` 查询 `tidb_global_task_history`。SQL 同时要求 `t.type = proto::ImportInto`，并通过 JSON 函数读取扫描并发、索引/列数量、总文件字节和行数。
3. 若结果为空，以 `storage::ErrTaskNotFound` 为根错误，通过 `errors::Annotatef` 附加 job ID 与 keyspace。若有多行，只消费 `rows[0]`；任务键和类型组合应由存储约束保证唯一性，本函数自身不检测重复。
4. 创建默认 `Info`，填充必需列；可选 JSON 派生列仅在 `Row::IsNull` 为假时写入。文件大小用于 `FileSize`，行数大于零时以 `round(totalFileBytes / RowCount)` 计算平均行长。
5. 第二次 `ExecuteSQLWithNewSession` 使用十进制字符串 `storage_crate::TaskIDToKey(info.TaskID)` 绑定子任务历史表的 `task_key`。这是重要边界：该列存 task ID，不存第一步的业务 task-key 字符串。SQL 按步骤和 KV 分组求字节总和及有效正时间的最小/最大值。
6. 单次遍历聚合行。write-and-ingest 行累加 data/index KV 字节；缺少开始或更新时间的行仍可贡献 KV 字节，但不会进入总时长或步骤时长。
7. 有有效时间窗时，总耗时取全体步骤的 `maxUpdateTime - minStartTime`；同一步骤多行则取该步骤最小开始与最大结束。所有差值都用 `.max(0)` 防止负耗时。
8. 用总文件字节和总耗时计算吞吐，然后把已知 `proto::Step` 映射到 `Duration` 字段。未知步骤被忽略，最后返回完整 `Info`。

## 数据与状态

`Info` 和 `Duration` 是纯值对象，派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq` 和 `Serialize`，便于比较与 JSON 输出。缺失的可选历史列保留 Rust `Default` 的零值或空字符串，不使用 `Option` 暴露缺失状态；调用者因此必须把空字符串理解为“没有足够数据/输入无效”，而不是零字节或零时长。明确的零字节会格式化为 `0B`，明确的零秒会格式化为 `0s`，两者与缺失值可区分。

函数内状态全部是局部变量：两个 KV 累加器及其 `has*` 标志、全局时间边界及 `hasTotalDuration` 标志，以及 `HashMap<proto::Step, [i64; 2]>` 保存每一步的最小开始/最大结束。布尔标志避免把合法的零或尚未观察到的数据混为一谈。`RowLength` 使用浮点除法后四舍五入；只有 `RowCount > 0` 才计算，因此没有除零。

SQL 与代码共同定义数据契约：第一轮行至少需要 9 列，第二轮行至少需要 5 列，并要求相应列类型能被 `GetInt64`、`GetString` 读取。实现没有为列数不足或类型错误做本地防护，这些边界由 `TaskManager`/SQL 层承担。

## 依赖与调用关系

上游关系：

- `pkg/dxf/importinto/jobhistory/lib.rs` 公开本文件全部符号。
- Rust 直接证据包括 `history_test.rs`、`migration_aster_unit_test.rs` 以及 `pkg/dxf/importinto/job_testkit_test.rs::large_task_ids_isolate_history_aggregation`；它们分别验证模拟查询聚合、Go 迁移语义和大于 JavaScript 安全整数范围的相邻 task ID 隔离。
- Go 对照应用入口为 `pkg/server/handler/tikvhandler/dxf.go::DXFImportIntoHistoryJobInfoHandler.ServeHTTP`。RustCodeGraph 的 `callers` 对精确 Rust 符号未返回生产调用边，仓库 `rg` 搜索也未发现 Rust 生产调用者，所以 Rust 运行时接线状态应视为“crate 已实现并有测试，生产请求链未验证”。

下游关系：

- `taskkey_crate::taskkey::ForJobInKeyspace` 构造第一轮任务键。
- `storage_crate::TaskManager::ExecuteSQLWithNewSession` 执行两轮只读 SQL，`Row`/`Value` 提供结果访问；`TaskIDToKey` 保留大整数 task ID 的十进制精度。
- `proto_crate` 提供 IMPORT INTO 类型名、任务状态及步骤常量。
- `injectfailpoint_crate` 提供入口前的随机错误注入。
- `bytesize` 仅提供二进制单位基数；最终字符串规则由本文件实现。
- `serde` 生成 JSON 序列化实现，`serde_json` 只属于 dev-dependency，用于测试字段名。

RustCodeGraph 对 `GetFromHistory` 的 callee 结果确认了它构造 `Info` 并调用四个公开格式化函数；图索引对常见名称存在宽匹配噪声，因此 SQL 方法、任务键和错误注解等边界同时以源码、Cargo 与测试原文核验。

## 错误处理与边界

- failpoint 错误、第一轮 SQL 错误和第二轮 SQL 错误都通过 `?` 原样向上传播；若第二轮失败，不返回部分 `Info`。
- 找不到任务时保留 `storage::ErrTaskNotFound` 的错误身份并增加上下文，供 Go HTTP 层用 `errors.Is` 转成 404。Rust 测试同时检查根错误相等和附加消息。
- NULL 可选字段回落为零值/空字符串；没有有效子任务时间窗时总耗时与两类吞吐均为空。
- `formatDuration`、`formatBytes` 拒绝负输入；吞吐函数还拒绝零/负时长，每核吞吐额外拒绝零/负节点数或并发度。
- 时间边界颠倒时，持续时间被钳制为零秒而不是负数。未知步骤不报错，只是不填任何具名阶段字段。
- KV 分类严格复刻 Go：write-and-ingest 阶段中只有字面值 `data` 属于数据 KV，其他分组（包括空字符串）都属于索引 KV。改变这一规则会改变兼容行为，需同步 Go 和测试。
- `i64` task ID 在第二轮查询前转换为字符串，避免经过浮点表示；`job_testkit_test.rs` 用相邻的 `2^53` 与 `2^53+1` 验证不会串入邻接任务。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁或共享缓存。`GetFromHistory` 按顺序执行两个新 session 查询：第二轮依赖第一轮解析出的 `TaskID`，不能并行。传入的 `Context` 在第一轮查询时克隆，原值移交给第二轮；取消、超时和 session 回收语义由 `Context` 与 `TaskManager::ExecuteSQLWithNewSession` 负责。

所有查询结果、聚合 HashMap 和 DTO 都由调用栈拥有，函数返回或报错时按 Rust 所有权规则释放。函数只读历史表，没有事务性写入；但两次查询之间没有在本文件建立共同快照，因此它依赖“历史数据已经归档且稳定”的使用前提。Go HTTP 入口设置 `requestDefaultTimeout`，Rust 侧当前未发现等价生产入口，不能据此断言 Rust 调用总有超时。

## 与 Go 版本的对应关系

`pkg/dxf/importinto/jobhistory/history.go` 是逐项对照基准，`Cargo.toml` 的 `[package.metadata.porting]` 也把 Go 包声明为 `pkg/dxf/importinto/jobhistory`。Rust 保留了 Go 导出名和字段名（以 `#[allow(non_snake_case)]` 兼容），两版的两轮 SQL、NULL 处理、KV 分类、时间窗合并、步骤映射、平均行长与吞吐公式一致。

可见语言差异包括：Go 返回 `*Info`，Rust 返回拥有所有权的 `Info`；Go 的 `int` 在 Rust 中固定为 `i32`，历史表值从 `i64` 转换时使用 `as i32`，超出范围会截断而非返回错误；Go 用 `time.Duration.String` 和 `docker/go-units.BytesSize`，Rust 以 `formatDuration`、`formatBytesValue`、`formatFourSignificantDigits` 手工复现当前测试覆盖的输出；Go 使用 `math.Round`，Rust 使用 `f64::round`。

验证语义来自两组独立 Rust 测试与 Go 测试：`history_test.rs` 覆盖主聚合和未找到，`migration_aster_unit_test.rs` 进一步覆盖重复步骤边界、所有步骤映射、NULL 零值、格式化边界及 JSON 名称；`history_test.go` 在真实测试表迁移后验证相邻大 task ID 不污染目标聚合。Rust 测试用规范 `TaskManager` 的注入结果验证业务聚合，但没有完整执行 Go 测试所用的真实 JSON SQL 函数链，相关限制在 `job_testkit_test.rs` 注释中明确记录。

## 扩展指南

- 新增展示字段时，先确认数据来自全局任务行还是子任务聚合；同步修改 `Info`、对应 SQL 投影/列下标、赋值逻辑、serde 字段名，以及 Go 的 `Info` 和 `GetFromHistory`。列下标是位置契约，插入列后必须逐一复核后续索引。
- 新增 IMPORT INTO 步骤时，若需展示独立耗时，应同时扩展 `Duration`、步骤 `match`、Go `switch`、JSON 契约和 `migration_combines_repeated_step_bounds_and_maps_every_go_step`。若不展示，保留未知步骤忽略行为并用测试说明意图。
- 改变 KV 分组规则或吞吐定义时，必须评估历史元数据兼容性、空/NULL 行为和面向用户的字符串稳定性；同步 `history_test.rs`、`migration_aster_unit_test.rs`、`history_test.go`，必要时扩展 `job_testkit_test.rs` 的持久化边界验证。
- 格式化变更应以 Go 库在边界值上的真实输出为基准，特别关注四位有效数字、科学计数、单位跃迁、零值和极大值。不能只让现有样例通过而缩减 Go 语义。
- 若接入 Rust HTTP 生产链，应在调用层负责方法、keyspace/job ID 校验、超时与 `ErrTaskNotFound` 到 404 的映射；不要把这些传输层职责塞入本文件。同时增加独立集成测试，而不是把测试写进 `history.rs`。
- 性能方面，当前第二轮 SQL 在数据库内按 `step, kv_group` 聚合，内存规模与分组数而非子任务数成正比。扩展 group-by 维度或取消 SQL 聚合会扩大返回集与内存/网络成本，应先评估。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/dxf/importinto/jobhistory` 列出本 crate 的 6 个 Rust/Go 实现与测试文件；`node --file pkg/dxf/importinto/jobhistory/history.rs` 读取了 395 行完整源码；`query GetFromHistory` 区分出 Go 与 Rust 同名实现；`callees history.rs::GetFromHistory` 确认 `Info` 构造和四个格式化调用。精确 `callers` 没有返回边，且 `callees` 存在宽匹配噪声，因此调用方用仓库文本搜索复核。
- 源码与配置：`pkg/dxf/importinto/jobhistory/history.rs`、`lib.rs`、`Cargo.toml`，以及 `pkg/dxf/importinto/Cargo.toml`、根 `Cargo.toml`/`Cargo.lock` 的 crate 接线。
- Go 对照与应用入口：`pkg/dxf/importinto/jobhistory/history.go`、`history_test.go`、`pkg/server/handler/tikvhandler/dxf.go::DXFImportIntoHistoryJobInfoHandler.ServeHTTP`。
- Rust 独立测试：`pkg/dxf/importinto/jobhistory/history_test.rs`、`migration_aster_unit_test.rs`、`pkg/dxf/importinto/job_testkit_test.rs::large_task_ids_isolate_history_aggregation`。
- 本任务是纯文档分析，按计划不运行 Cargo。仓库指令引用的 `.agents/skills/tidb-verify-profile/SKILL.md` 在当前检出中不存在，无法加载其 Ready 配置；交付验证使用任务文件规定的 11 章节结构命令，并人工复核本文没有把未验证的 Rust 生产接线写成既成事实。
