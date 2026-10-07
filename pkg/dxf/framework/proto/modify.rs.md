# `pkg/dxf/framework/proto/modify.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dxf-framework-proto`；crate 入口 `pkg/dxf/framework/proto/lib.rs` 通过 `pub mod modify` 声明模块，并用 `pub use modify::*` 将其公开类型和常量提升到 crate 根。该 crate 的边界由 `pkg/dxf/framework/proto/Cargo.toml` 定义，Go 对照包记录为 `pkg/dxf/framework/proto`。

它是 DXF（分布式执行框架）的“运行中任务修改”协议层：描述要改什么、目标值是多少、修改前任务处于什么状态，并提供与 Go 日志格式一致的字符串展示。它不负责校验目标值、持久化、状态迁移、调度器重建或修改任务元数据；这些行为分别位于 storage、scheduler 及具体任务的 extension 中。

## 核心职责

- 用 `ModificationType = &'static str` 表示修改类型，并固定四个兼容性线名：`ModifyRequiredSlots`、`ModifyMaxNodeCount`、`ModifyBatchSize`、`ModifyMaxWriteSpeed`。
- 用 `Modification { Type, To }` 表示一项修改，用 `ModifyParam { PrevState, Modifications }` 将修改前状态和有序修改列表打包。
- 通过 `ModificationTypeExt::String`、`Modification::String` 和 `ModifyParam::String` 生成 Go `fmt.Stringer` 风格的诊断文本。
- 保持历史线协议：required slots 已与旧 concurrency 概念分离，但 `ModifyRequiredSlots` 的值仍是 `"modify_concurrency"`；消费者和持久化数据依赖这个字符串，不能仅因 Rust 命名而改线值。

## 主要符号

- `pub type ModificationType = &'static str`：修改种类的轻量别名。它没有封闭枚举的穷尽性，具体消费者必须处理未知字符串。
- `pub trait ModificationTypeExt`：为 `ModificationType` 补充大写命名的 `String(&self) -> String`，对齐 Go 方法名；实现只是复制静态字符串。
- `ModifyRequiredSlots = "modify_concurrency"`：修改任务所需 slot。历史 wire name 是重要兼容约束。
- `ModifyMaxNodeCount = "modify_max_node_count"`：修改任务可使用的最大节点数。
- `ModifyBatchSize = "modify_batch_size"`、`ModifyMaxWriteSpeed = "modify_max_write_speed"`：add-index 元数据修改，由具体调度扩展解释。
- `pub struct ModifyParam`：`PrevState: TaskState` 是乐观状态门禁和修改完成后的恢复目标；`Modifications: Vec<Modification>` 保留提交顺序。
- `ModifyParam::String`：逐项调用 `Modification::String`，用单个空格连接，再生成 `{prev_state: ..., modifications: [...]}`。
- `pub struct Modification`：`Type` 指明修改类别，`To: i64` 保留 Go `int64` 的目标值域。
- `Modification::String`：生成 `{type: ..., to: ...}`，不做值合法性判断。

这些 API 均为公开符号。文件没有模块级可变状态、条件编译项、异步函数或内部私有辅助函数。

## 执行流程

1. 上游构造 `ModifyParam`，把当前任务状态写入 `PrevState`，把一个或多个目标变化写入 `Modifications`。Go 的 DDL 动态参数循环可同时生成 required slots、batch size 和 max write speed 修改（`pkg/ddl/index.go` 的修改收集与 `ModifyTaskByID` 调用）。
2. storage 的 `TaskManager::ModifyTaskByID`（`pkg/dxf/framework/storage/task_state.rs`）先调用 `TaskStateExt::CanMoveToModifying`，再确认数据库中的当前状态仍等于 `PrevState`；成功后把任务状态改为 `modifying` 并持久化修改参数。
3. storage converter 的 `parse_modify_param`（`pkg/dxf/framework/storage/converter.rs`）从持久化 JSON 重建 `PrevState`、`Type` 和 `To`；`scheduler/storage_adapter.rs` 再把 proto 对象转换成 scheduler 内部的 `previous_state` 与 `modifications`。
4. `BaseScheduler::schedule_once` 遇到 `TASK_STATE_MODIFYING` 时进入 `on_modifying`（`pkg/dxf/framework/scheduler/scheduler.rs`）。`modify_concurrency` 更新 required slots 并可能要求重建调度器；正数 `modify_max_node_count` 更新节点上限；其余类型转交 `Extension::modify_meta`。
5. storage 的 `ModifiedTask` 持久化新并发度、节点上限和 meta，将任务恢复到 `PrevState`，清空 `modify_params`，并同步 pending/running/paused 子任务的并发度。调度器内存快照随后也恢复原状态并清空修改列表。

本文件的 `String` 方法不参与上述状态迁移，仅服务于日志、断言和诊断展示。

## 数据与状态

`ModifyParam` 是一次修改请求的快照。`PrevState` 同时承担两项语义：提交时检测陈旧请求，完成后决定回到 pending、running 或 paused 中的哪一个状态。允许进入 modifying 的状态集合定义在 `pkg/dxf/framework/proto/task.rs` 的 `TaskStateExt::CanMoveToModifying`，不是本文件自行决定。

`Modifications` 使用 `Vec`，因此顺序和重复项都会被保留。本文件不合并同类型修改，也不声明原子应用到哪一层；scheduler 按顺序遍历，内置字段可能被后项覆盖，而元数据项按原顺序交给 extension。新增消费者不能假设列表已去重。

`ModificationType` 是 `&'static str`，四个常量无需分配；从数据库解析出的动态类型则由 converter 的 `intern` 转为静态引用。`To` 是无类型的 `i64` 载荷，其单位和范围由 `Type` 的消费者解释：例如 slots/节点数会转成 scheduler 字段，写速度和 batch size 则进入任务 meta。

## 依赖与调用关系

直接源码依赖只有 `super::task::TaskState`。`pkg/dxf/framework/proto/Cargo.toml` 声明 crate 依赖 `bytesize`、`chrono`、`serde`、`serde_json`，但 `modify.rs` 本身没有直接调用这些 crate；JSON 编解码由相邻 storage 适配层承担，当前结构体上的 JSON 名称只以注释记录。

RustCodeGraph 将 `ModifyParam` 的构造者定位到 handle 测试、framework 修改集成测试、mock/execute 迁移测试、import-into 代码等，并显示 `task.rs` 导入它作为 `Task.ModifyParam`。关键下游边为：

- `Task.ModifyParam`（`pkg/dxf/framework/proto/task.rs`）持有协议对象；
- `TaskManager::ModifyTaskByID` / `ModifiedTask`（`pkg/dxf/framework/storage/task_state.rs`）负责进入和退出 modifying；
- `parse_modify_param`（`pkg/dxf/framework/storage/converter.rs`）负责持久化数据恢复；
- `from_task` / `to_task`（`pkg/dxf/framework/scheduler/storage_adapter.rs`）负责 proto 与 scheduler 内部模型互转；
- `BaseScheduler::on_modifying`（`pkg/dxf/framework/scheduler/scheduler.rs`）消费修改类型；
- Go 侧 `pkg/ddl/backfilling_dist_scheduler.go::ModifyMeta` 证明 batch size 与 max write speed 属于具体任务元数据语义，而不是本协议文件中的通用字段更新。

## 错误处理与边界

本文件所有格式化方法都是不可失败的普通函数，没有 `Result`、错误转换或显式 panic。空 `Modifications` 会显示为空方括号；负数、零、重复类型和未知类型也可以被结构体表达，因为协议层不验证业务合法性。

真正的边界位于消费者：`ModifyTaskByID` 拒绝 failed 等不能进入 modifying 的状态，并在数据库状态已变化时返回 task-changed 错误；Rust scheduler 仅接受正数 max-node-count，未识别的类型交给 extension。集成测试 `stale_or_terminal_modification_is_rejected_without_side_effects` 验证陈旧 `PrevState` 和终态请求不会改变任务；storage 测试验证非法状态不会发 SQL，合法请求会写入 `modify_params`。

由于 `ModificationType` 不是 enum，添加拼写错误的字符串仍可编译。由于 `To` 会在消费者中转换为 `i32` 等更窄类型，范围检查必须发生在入口或消费端；本文件目前不提供这类保护。另一个边界是当前 `ModifyParam` / `Modification` 未在此文件派生 `Serialize`、`Deserialize`、`Clone` 或 `PartialEq`，不要假定可以直接以 serde 编解码或任意复制比较。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或外部句柄。`String` 方法只借用 `self`；临时字符串和 `Vec<String>` 在调用结束时释放，`ModifyParam` 拥有其 `Vec<Modification>`。

并发正确性由跨层生命周期保障：提交端把 `PrevState` 当作比较条件，storage 在事务中再次读取并以 `where ... state = PrevState` 更新，避免陈旧修改覆盖并发状态变化；修改完成时只允许从 `modifying` 更新。调度器处理的是 storage 快照，完成后清空修改列表。扩展本文件时必须保持这个“旧状态校验 → modifying → 应用 → 恢复旧状态/清空参数”的生命周期，不能把 `PrevState` 降格为仅用于展示的字段。

## 与 Go 版本的对应关系

权威对照为 `pkg/dxf/framework/proto/modify.go`：四个常量的字符串、`ModifyParam`/`Modification` 字段以及三个字符串方法的输出均逐项对应。`pkg/dxf/framework/proto/migration_aster_unit_test.rs::migration_subtask_and_modification_match_go` 验证单项输出；`pkg/dxf/framework/integrationtests/modify_test.rs::concurrency_and_node_count_modifications_keep_wire_names` 验证多项顺序和历史 wire name。

已确认的语言差异如下：

- Go 的 `ModificationType` 是独立的 `string` 定义；Rust 是 `&'static str` 类型别名，类型隔离更弱，并通过扩展 trait 模拟 `String` 方法。
- Go 字段有 `json:"prev_state"`、`json:"modifications"`、`json:"type"`、`json:"to"` 标签；Rust 文件只保留标签注释，实际持久化转换在 storage 中完成。
- Go 的 `ModifyParam.String` 是指针接收者，Rust 使用共享借用；两者都不修改参数。
- Go 的 `%v` 依次调用元素的 `String`；Rust 显式 map、collect、join，产生相同的单空格分隔格式。
- Go 的调度实现对非内置类型调用 `ModifyMeta`，Rust scheduler 也把其余修改汇总给 extension；两侧都将 required slots 和 max-node-count 作为框架内置字段处理。

## 扩展指南

新增修改类型时，首先在本文件增加具有稳定 wire value 的常量，然后同步 `pkg/dxf/framework/proto/modify.go`。再明确它属于框架内置字段还是任务 meta：前者需修改 Rust/Go scheduler 的 modifying 分支、storage 持久化字段和必要的执行器传播；后者需修改对应 extension 的 `modify_meta` / Go `ModifyMeta`。仅增加常量不会让功能生效。

测试应保持独立于生产文件。至少同步：

- `pkg/dxf/framework/proto/migration_aster_unit_test.rs`：常量线值及 `String` 格式；
- `pkg/dxf/framework/integrationtests/modify_test.rs`：状态门禁、运行中传播、未知类型或值边界；
- `pkg/dxf/framework/storage/task_state_test.rs` 和 `converter_1_aster_unit_test.rs`：JSON/状态事务/活跃子任务更新；
- 具体任务的 scheduler/extension 测试，以及对应 Go 测试（当前 Go 主覆盖包括 `pkg/dxf/framework/integrationtests/modify_test.go`、`pkg/dxf/framework/storage/task_state_test.go` 和 `pkg/ddl/index_nokit_test.go`）。

兼容性风险主要是改动 wire string、JSON 字段名或展示格式；正确性风险主要是遗漏消费者、接受溢出/非法 `To`、破坏 `PrevState` 并发门禁；性能风险通常较小，但修改 required slots 会触发 scheduler 重建，max-node-count 会影响重平衡，修改列表过大还会增加持久化与日志开销。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/dxf/framework/proto` 覆盖 `modify.rs`、Go 对照和相关独立测试。
- RustCodeGraph 源码/符号查询：`modify.rs`（101 行）、`ModifyParam`、`ModificationTypeExt`、`Modification`；图中 `ModifyParam` 的构造边包括 framework integration tests、handle tests、mock tests 和 import-into 代码。
- crate 与模块：`pkg/dxf/framework/proto/Cargo.toml`、`pkg/dxf/framework/proto/lib.rs`。
- Rust 直接运行链：`pkg/dxf/framework/proto/task.rs`、`pkg/dxf/framework/storage/task_state.rs`、`pkg/dxf/framework/storage/converter.rs`、`pkg/dxf/framework/scheduler/storage_adapter.rs`、`pkg/dxf/framework/scheduler/scheduler.rs`。
- Go 对照与真实生产入口：`pkg/dxf/framework/proto/modify.go`、`pkg/dxf/framework/storage/task_state.go`、`pkg/dxf/framework/scheduler/scheduler.go`、`pkg/ddl/index.go`、`pkg/ddl/backfilling_dist_scheduler.go`。
- Rust 测试证据：`pkg/dxf/framework/proto/migration_aster_unit_test.rs`、`pkg/dxf/framework/integrationtests/modify_test.rs`、`pkg/dxf/framework/storage/task_state_test.rs`、`pkg/dxf/framework/storage/converter_1_aster_unit_test.rs`。
- 本任务为纯文档分析，按计划未运行 Cargo；最终仅执行任务文件规定的 11 章节结构验证，并人工复核上述路径、符号和边界陈述。
