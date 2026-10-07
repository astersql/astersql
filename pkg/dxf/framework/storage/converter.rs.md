# `pkg/dxf/framework/storage/converter.rs`

## 文件定位

本文件是 `astersql-dxf-framework-storage` crate 的数据库行转换层；对应源码为 [`converter.rs`](converter.rs)。它把 storage 查询返回的 `chunk::Row` 按约定列序转换成 `proto::TaskBase`、`proto::Task`、`proto::SubtaskBase` 和 `proto::Subtask`，并把数值任务 ID 转成子任务表使用的十进制字符串键。crate 根通过 `include!("converter.rs")` 将其直接并入根模块，因此这里的 `pub` 函数就是 crate 根 API，而不是一个独立子模块（`pkg/dxf/framework/storage/lib.rs:1246-1256`）。

crate 清单将库入口指定为 `lib.rs`、关闭自动测试发现，并把 `converter_1_aster_unit_test.rs` 注册为独立测试目标；直接依赖中与本文件最相关的是 `serde` 和 `serde_json`，`nextgen` feature 不改变本文件的条件编译路径（`pkg/dxf/framework/storage/Cargo.toml`）。源文件没有 `cfg` 条件、trait 或 impl；全部逻辑由模块级函数组成。

## 核心职责

1. 按 task 查询的固定 12/18 列布局构造基础或完整任务对象（`row2TaskBasic`、`Row2Task`，`converter.rs:216-283`）。
2. 按 subtask 查询的固定 10/13 列布局构造基础或完整子任务对象，并兼容以 `VARCHAR` 保存的 task ID、以 `BIGINT` 秒数保存的时间（`row2BasicSubTask`、`Row2SubTask`，`converter.rs:295-358`）。
3. 将数据库中的任务类型、任务状态和子任务状态映射到 proto 常量；未知字符串不丢失，而是通过 `intern` 原样保留（`converter.rs:25-89`）。
4. 以 Go `pingcap/errors` 的规范化错误 JSON 行为解析任务错误，解析失败时保留原始字节的有损 UTF-8 文本（`row2TaskError`，`converter.rs:124-211`）。
5. 解析 `extra_params` 和 `modify_params` JSON，同时维持 Go 版本“记录错误但继续返回行对象”的容错契约（`converter.rs:92-121,216-283`）。
6. 用 `TaskIDToKey` 生成完整的有符号十进制字符串，避免 SQL 绑定过程中把大整数经浮点表示而混淆相邻 ID（`converter.rs:286-290`）。

## 主要符号

- `intern(String) -> &'static str`（crate 内可见）：用进程级 `OnceLock<RwLock<HashMap<...>>>` 去重动态字符串，并以 `Box::leak` 取得稳定的静态引用（`converter.rs:25-47`）。它也是同 crate 的若干状态聚合转换的共享辅助函数。
- `task_type`、`task_state`、`subtask_state`（私有）：把已知数据库字符串映射到 proto 常量，未知值交给 `intern`。已知 task 类型为 `Example`、`ImportInto`、`backfill`；任务和子任务状态集合分别见 `converter.rs:50-89`。
- `parse_modify_param(&[u8]) -> Result<proto::ModifyParam, serde_json::Error>`（私有）：读取 `prev_state` 与 `modifications[]`；缺字段或字段类型不匹配时使用空字符串、0 或空列表，只有顶层 JSON 语法错误会返回 `Err`（`converter.rs:92-121`）。
- `row2TaskError(&chunk::Row, usize) -> Option<Error>`（私有）：处理 NULL、规范错误对象、旧 terror class 到 RFC code 的补全，以及非对象/非法 JSON 的原文回退（`converter.rs:124-211`）。历史任务摘要也调用它。
- `row2TaskBasic(chunk::Row) -> proto::TaskBase`（公开）：消费一份 Row 值并读取列 0–11；调用方包括任务列表、按 ID/Key 查询、执行信息查询和历史摘要转换。
- `Row2Task(chunk::Row) -> proto::Task`（公开）：复用基础转换，再读取列 12–17；调用方主要是当前/历史任务的按 ID、Key、状态和清理查询。
- `TaskIDToKey(i64) -> String`（公开）：返回 `i64::to_string()`；调用面横跨子任务读写、状态切换、历史搬迁以及 importinto 作业查询。
- `row2BasicSubTask(chunk::Row) -> proto::SubtaskBase`（公开）：读取列 0–9；除 `Row2SubTask` 外，基础子任务列表也直接复用它。
- `Row2SubTask(chunk::Row) -> proto::Subtask`（公开）：在基础对象上读取列 10–12；由按 step/state、executor 和历史联合查询使用。

## 执行流程

任务行的主流程如下：

1. `Row2Task` 克隆输入行并调用 `row2TaskBasic`。后者读取 ID、Key、Type、State、Step、优先级、槽位、创建时间、作用域、最大节点数、额外参数和 keyspace（列 0–11）。
2. `row2TaskBasic` 总是读取创建时间；`extra_params` 仅在列 10 非 NULL 时解析，失败时打印诊断并保留 `ExtraParams::default()`。
3. `Row2Task` 先以 UNIX epoch、空向量、空字符串、无错误和空 `ModifyParam` 建立完整对象，再按 NULL 情况填充 start/update 时间（12、13），并直接复制 meta（14）和 scheduler ID（15）。
4. 错误列 16 为 NULL 时得到 `None`；否则 `row2TaskError` 解析错误字段，`Row2Task` 最终保存其格式化字符串。modify 列 17 非 NULL 时调用 `parse_modify_param`；语法错误只记录，不覆盖默认值。

子任务行的主流程如下：

1. `Row2SubTask` 克隆行并调用 `row2BasicSubTask`。后者将列 2 的字符串 task ID 解析为 `i64`，失败时记录并回退 0；列 3 通过 `proto::Int2Type` 映射，未知整数映射为空类型（`pkg/dxf/framework/proto/type.rs:46-53`）。
2. ordinal（8）为 NULL 时保持 0；start time（9）为 NULL 时保持 epoch，否则按秒调用 `time::Unix`。其余基本列 0–7直接填入对象。
3. `Row2SubTask` 对 update time（10）采用同样的 epoch/秒数规则，然后复制 meta（11）及 JSON summary 的字符串表示（12）。

## 数据与状态

转换函数自身不访问数据库，也不修改输入行背后的持久化状态；它们依赖上游 SQL 严格维持列顺序和可读取类型。`row2TaskBasic`、`Row2Task`、`row2BasicSubTask`、`Row2SubTask` 中的数字下标就是 storage 查询与 proto 对象之间的隐式 schema，调整 SELECT 列时必须同步调整这些位置。

NULL 的语义按字段分别处理：可选时间回退 `UNIX_EPOCH`，ordinal 回退 0，task error 回退 `None`，可选 JSON 保留目标类型默认值。meta 和 summary 没有 NULL 分支，调用者必须提供符合 `chunk::Row` getter 契约的列。

唯一的进程级可变状态是 `intern` 的字符串表。已知 enum 值复用静态常量；未知 enum、修改类型和动态错误文本会成为表项或静态泄漏。表只增不减，生命周期与进程一致，换取 proto 中 `&'static str` 字段的稳定引用。

## 依赖与调用关系

上游方面，RustCodeGraph 显示：

- `row2TaskBasic` 被 `task_table.rs` 的 `GetAllTasks`、`GetTaskBaseByIDWithHistory`、`GetTaskBaseByKeyWithHistory`、`GetTaskBasesInStates`、`GetTaskExecInfoByExecID`、`getTaskBaseByID`、`getTopTasks` 以及 `history.rs::row2HistoryTaskSummary` 调用。
- `Row2Task` 被 `task_table.rs` 的当前/历史任务按 ID、Key、状态及清理查询调用。
- `row2BasicSubTask` 被 `GetActiveSubtasks`、`GetAllSubtasks` 和 `Row2SubTask` 调用；`Row2SubTask` 被按 step/state、executor 及 history 联合查询调用。
- `TaskIDToKey` 有广泛调用面，包括 `subtask_state.rs` 的取消、失败、暂停、恢复，`task_state.rs` 的任务修改/错误暂停，`task_table.rs` 的子任务查询和 step 切换，`history.rs` 的历史搬迁，以及 `pkg/dxf/importinto/job.rs` 与 `jobhistory/history.rs`。

下游方面，本文件依赖 crate 根提供的 `chunk::Row`、`Cell` getter 语义、`Error`、`time` 兼容层和 `proto` 门面；JSON 解析由 `serde_json` 完成。`proto::TaskBase`、`Task`、`SubtaskBase`、`Subtask`、`ModifyParam` 是输出边界，其中 `ModifyParam` 明确包含 `PrevState` 与 `Vec<Modification>`（`pkg/dxf/framework/proto/modify.rs:61-66`）。

## 错误处理与边界

- 所有转换函数都返回对象而非 `Result`。这与 Go 版本一致：持久化数据的 JSON 或 task ID 异常不会阻断整行读取，而是记录到标准错误并采用默认值/原文回退。
- `row2TaskError` 只接受 JSON 对象或 `null`。字段同时接受小写名和 Go 导出的首字母大写名；缺失 class/code 默认为 0。旧 class 1–27 可补成 `<class>:<code>`，越界 class 不补名称。展示文本为 `[RFCCode]message`，无 RFCCode 时为 `[code]message`；非法 JSON、数组或字段类型错误则用原始字节的 lossy 字符串。
- `parse_modify_param` 对合法 JSON 内部的缺失/错类型字段采取默认值，但顶层语法错误会返回 `Err`。因此扩展 JSON schema 时要区分“兼容忽略”与“必须拒绝”的字段。
- 固定列 getter 没有长度与类型检查；列数不足或类型不符合 `chunk::Row` 实现时，风险由调用方承担。整数到 `i32` 的 `as` 转换也不做范围校验。
- `intern` 的锁若 poisoned 会通过 `expect` panic；未知值数量无界时会永久增长。它不适合承载高基数、攻击者可控且无限变化的数据，除非同时调整 proto 的所有权模型。

## 并发与资源生命周期

`row2TaskBasic`、`Row2Task`、`TaskIDToKey`、`row2BasicSubTask` 和 `Row2SubTask` 除字符串驻留外都是同步、无异步任务、无通道、无事务的纯行转换；SQL session 和事务生命周期属于上游 storage 查询函数。

`intern` 通过 `OnceLock` 延迟创建全局表，通过 `RwLock` 允许并发命中读取。读锁未命中后获取写锁，并在写锁下做第二次检查，避免两个线程为同一字符串重复泄漏。插入后的 `&'static str` 永不失效，代价是相应分配永不回收。`chunk::Row` 在组合转换中会被克隆一次，使基础转换与后续列读取各自拥有可用句柄；文档不能假定 clone 一定是零成本，性能敏感修改应核对 `Row` 实现。

## 与 Go 版本的对应关系

直接对照文件为 [`converter.go`](converter.go)。Rust 保留了 Go 的四段转换结构、列下标、NULL 回退、秒级 bigint 时间、字符串 task key、额外参数/修改参数解析失败继续执行，以及非法子任务 ID 归零等语义。

主要表示差异如下：

- Go 的转换函数返回 proto 指针；Rust 返回拥有所有权的结构体，并在完整转换前克隆 `chunk::Row`。
- Go 的 `TaskType`/`TaskState` 是字符串别名，可直接转换任意字符串；Rust proto 字段使用静态字符串，因此已知值走常量，未知值由 `intern` 保真。
- Go `row2TaskError` 返回 `error`，借助 `errors.Normalize("").UnmarshalJSON`；Rust 在局部重建 class/code/message/RFCCode 规则，再由 `Row2Task` 保存格式化字符串。`converter_test.rs` 对 null、规范 JSON、旧 class、显式 RFC code、非法对象与非 JSON 回退进行了针对性覆盖。
- Go 用结构体 JSON unmarshal 填充 `ModifyParam`；Rust 的手工解析对缺失或错类型的内部字段更明确地使用默认值。任何新字段都应同时核对两端 serde/encoding-json 行为。
- Go 使用日志组件，Rust 当前使用 `eprintln!`；两者都不把这些兼容性异常向调用者返回。

未发现专门命名为 `converter_test.go` 的 Go 文件。Go 语义依据来自 `converter.go` 本身及 storage 包的调用/测试面；Rust 的专门回归位于 `converter_test.rs` 和独立目标 `converter_1_aster_unit_test.rs`。

## 扩展指南

- 增删 task/subtask SELECT 列时，先修改拥有查询的 `task_table.rs`/`history.rs`，再同步本文件的固定下标和 Go `converter.go`；为 NULL、零值、类型转换和历史表路径各补用例。
- 新增任务类型或状态时，同时更新 `task_type`、`task_state` 或 `subtask_state` 及 proto 常量/映射。要保留未知值兼容路径，不能把默认分支改成拒绝或空字符串而没有迁移方案。
- 修改 error JSON 时同时覆盖小写/大写字段别名、null、非对象、错误字段类型、legacy class、RFCCode 和原始文本回退；独立测试应继续放在 `pkg/dxf/framework/storage/converter_test.rs` 或注册的 `converter_1_aster_unit_test.rs`，不要把测试嵌入生产源文件。
- 扩展 `ModifyParam` 或 `ExtraParams` 时同步 Go JSON 标签、Rust serde 定义和失败后的默认对象语义，避免旧行因新字段而无法读取。
- 修改 `TaskIDToKey` 时必须保持完整有符号十进制域，特别是 `2^53` 相邻值、`i64::MIN` 和 `i64::MAX`；字符串绑定是历史 `VARCHAR task_key` schema 的兼容要求。
- 若要消除静态泄漏，应先把 proto 中相关静态字符串字段改为拥有型值，并审计 `intern` 在 `task_table.rs` 的共享调用者；仅替换本文件会破坏类型与生命周期契约。

## 验证依据

- Rust 源与装配：`pkg/dxf/framework/storage/converter.rs:25-359`、`pkg/dxf/framework/storage/lib.rs:1246-1256`。
- crate 边界：`pkg/dxf/framework/storage/Cargo.toml` 的 `[lib]`、`[[test]]`、`[features]`、`[dependencies]` 和 porting metadata。
- Go 对照：`pkg/dxf/framework/storage/converter.go:29-155`。
- proto 对照：`pkg/dxf/framework/proto/type.rs:46-53`、`pkg/dxf/framework/proto/modify.rs:61-66`。
- Rust 专项测试：`pkg/dxf/framework/storage/converter_test.rs:6-80` 覆盖错误规范化与完整 i64 key；`pkg/dxf/framework/storage/converter_1_aster_unit_test.rs:37-162` 覆盖 task 列映射、JSON、错误回退、非法 task ID、类型映射、NULL ordinal、秒级时间和 summary。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/dxf/framework/storage` 确认目标及对照文件已索引；`query/node/explore` 核对了上述符号签名、源码、上下游调用者和测试入口。精确 `callers/callees` 子命令未返回独立文本，调用边采用同一索引的 `explore` blast-radius 结果复核，并与符号源码相互印证。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核本说明未把当前容错行为、日志行为或调用范围写成未经证实的理想设计。
