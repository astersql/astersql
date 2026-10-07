# [`pkg/dxf/framework/proto/step.rs`](step.rs)

## 文件定位

本文件属于 `astersql-dxf-framework-proto` crate，crate 入口 `pkg/dxf/framework/proto/lib.rs` 以 `pub mod step` 声明模块并用 `pub use step::*` 重导出其公开符号。它位于 DXF（Distributed eXecution Framework）的协议层：DXF 将一个任务顺序拆成多个 step，再把每个 step 拆成可并行执行的 subtask；该整体抽象可在 `pkg/dxf/framework/doc.go` 的 “Task abstraction” 部分核对。

文件本身不调度任务，也不执行导入或回填。它定义持久化/跨层共享的 step 数值、任务类型到 step 名称的映射，以及两种合法性判断。实际推进 step 的调度器、按 step 生成物理计划的 planner、执行不同 step 的 executor 和存储层共同消费这些值。

`pkg/dxf/framework/proto/Cargo.toml` 声明此 crate 的库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/dxf/framework/proto"` 标出 Go 对照包。该 Cargo 文件列出的 `bytesize`、`chrono`、`serde`、`serde_json` 均不是 `step.rs` 的直接依赖；本文件只引用同 crate 的 `TaskType` 与三种任务类型常量。

## 核心职责

1. 以 `pub type Step = i64` 固定阶段编号的表示，与 Go 的 `type Step int64` 对齐。
2. 定义所有任务共享的框架标记步：`StepInit = -1`、`StepDone = -2`、`StepPrepared = -3`。负值与既有编码是持久化兼容契约，不能重排或复用。
3. 定义三组正数业务步：示例任务 `StepOne..StepThree`、`ImportInto` 的七个导入阶段、`Backfill` 的四个加索引阶段。不同任务类型可以复用同一个正整数，因为解释时必须同时提供 `TaskType`。
4. 由 `Step2Str` 将 `(TaskType, Step)` 转为日志、诊断及任务摘要使用的稳定字符串。
5. 由 `IsValidStep` 和 `IsValidBusinessStep` 区分“可识别的阶段”与“可识别且不是框架标记的业务阶段”。

这些职责由 `pkg/dxf/framework/proto/step_test.rs` 的映射与合法性断言直接覆盖，也由 `migration_step_and_type_match_go`（`pkg/dxf/framework/proto/migration_aster_unit_test.rs`）抽样验证 Go/Rust 迁移一致性。

## 主要符号

- `Step = i64`：阶段编号类型别名。它没有 Rust 新类型的类型隔离能力，任意 `i64` 都能传入 API，因此合法性依赖显式校验。
- `StepInit`、`StepDone`、`StepPrepared`：框架级标记。`StepPrepared` 表示 prepare 逻辑已完成，但任务仍处于 pending；成功路径分别是 `Init → 业务步 → Done`，或 prepare 模式下 `Init → Prepared → 业务步 → Done`。
- `StepOne`、`StepTwo`、`StepThree`：`TaskTypeExample` 的业务阶段 1、2、3。
- `ImportStepImport`、`ImportStepPostProcess`、`ImportStepEncodeAndSort`、`ImportStepMergeSort`、`ImportStepWriteAndIngest`、`ImportStepCollectConflicts`、`ImportStepConflictResolution`：`ImportInto` 的业务阶段 1 至 7。源码注释给出本地排序和全局排序两条路径，其中 merge-sort、冲突收集和冲突解决可以没有 subtask。
- `BackfillStepReadIndex`、`BackfillStepMergeSort`、`BackfillStepWriteAndIngest`、`BackfillStepMergeTempIndex`：`Backfill` 的业务阶段 1 至 4。merge-sort 只用于全局排序，文件重叠低时可跳过 subtask。
- `Step2Str(t: TaskType, s: Step) -> String`：公开格式化入口。先识别三个框架标记，再按 `Backfill`、`ImportInto`、`TaskTypeExample` 分派。
- `IsValidStep(t: TaskType, s: Step) -> bool`：公开合法性入口，以格式化结果是否包含私有前缀 `unknown step` 判定。
- `IsValidBusinessStep(t: TaskType, s: Step) -> bool`：先排除三个框架标记，再调用 `IsValidStep`。
- `exampleStep2Str`、`importIntoStep2Str`、`backfillStep2Str`：私有的按任务类型映射函数。
- `unknownStepStr` 与 `unknownStepPrefix`：统一生成 `unknown step <id>` 并为合法性检查提供哨兵文本。

本文件没有 trait、struct、enum、宏或条件编译项。

## 执行流程

`Step2Str` 的执行顺序具有语义意义：

1. 先匹配 `StepInit`、`StepDone`、`StepPrepared`，直接返回 `init`、`done`、`prepared`。因此框架标记对任意任务类型都可识别，不要求 `TaskType` 已注册。
2. 非框架 step 再匹配任务类型。`Backfill`、`ImportInto`、`TaskTypeExample` 分别进入对应私有映射函数。
3. 类型已知但编号未登记时，私有映射调用 `unknownStepStr`，返回 `unknown step <id>`。
4. 类型未知时，顶层直接返回 `unknown type <type>`，不会进入编号映射。

`IsValidStep` 调用 `Step2Str`，只检查结果中是否包含 `unknown step`。所以三个框架标记和已登记业务步返回 `true`，已知类型下未登记编号返回 `false`。一个需要特别保留的 Go 兼容边界是：未知任务类型得到 `unknown type ...`，不含 `unknown step`，因而 `IsValidStep` 会返回 `true`；当前实现没有把“未知类型”纳入拒绝条件。

`IsValidBusinessStep` 在上述逻辑前增加框架标记过滤。它对 `Init`、`Done`、`Prepared` 固定返回 `false`，其他值再交给 `IsValidStep`；因此它同样继承未知任务类型的兼容行为。

导入流程的声明顺序不是由本文件主动驱动，而是由业务 scheduler/planner 根据这些常量选择。例如 `pkg/dxf/importinto/scheduler.rs` 处理 `StepInit | StepPrepared` 以及冲突收集等分支，`pkg/dxf/importinto/planner.rs` 用 `Step2Str` 生成诊断信息并按 `ImportStepCollectConflicts` 生成规格。代码图确认的直接调用链还包括 `TaskBase::String → Step2Str`（`pkg/dxf/framework/proto/task.rs`）。

## 数据与状态

所有数据都是编译期常量或函数局部值；本文件不持有可变全局状态。

数值空间按约定分为两类：负数是跨任务类型共享的框架标记，正数由各任务类型在自己的命名空间中解释。例如数值 `1` 在 `TaskTypeExample`、`ImportInto`、`Backfill` 下分别表示 `one`、`import`、`read-index`。因此持久化或传输时必须同时保存任务类型和 step，不能只凭数值恢复业务含义。

字符串是对外可观察的诊断契约：已知映射使用固定短名称；未知编号是 `unknown step <十进制编号>`；未知类型是 `unknown type <原任务类型字符串>`。`Step2Str` 每次创建一个拥有所有权的 `String`，没有缓存或驻留状态。

最重要的不变量是已有常量值不得修改。源码与 Go 对照都明确指出，改值会破坏向后兼容性；原因是任务/子任务状态会跨进程和版本保留，消费者按既有数字解释阶段。

## 依赖与调用关系

下游依赖很小：

- `TaskType` 来自 `pkg/dxf/framework/proto/task.rs`，实际为 `&'static str`。
- `Backfill`、`ImportInto`、`TaskTypeExample` 来自 `pkg/dxf/framework/proto/type.rs`，值分别是 `backfill`、`ImportInto`、`Example`。
- 标准库的模式匹配、`String` 分配与 `format!` 完成全部逻辑；没有直接使用 Cargo 第三方依赖。

代码图对本文件给出的内部调用边为：`IsValidBusinessStep → IsValidStep → Step2Str`；`Step2Str → exampleStep2Str/importIntoStep2Str/backfillStep2Str`；三个类型专用映射的默认分支均调用 `unknownStepStr`。

上游分为两类：

- 协议展示：`TaskBase::String` 在 `pkg/dxf/framework/proto/task.rs` 中调用 `Step2Str(self.Type, self.Step)`，将阶段名称写入任务的人类可读摘要。
- 业务控制与持久化：`pkg/dxf/importinto/scheduler.rs`、`planner.rs`、`task_executor.rs`、`job.rs`、`jobhistory/history.rs` 等按 `ImportStep*` 常量选择推进、计划生成、执行和统计分支；框架存储与调度模块则使用 `StepInit`、`StepPrepared`、`StepDone` 表达生命周期。RustCodeGraph 的文件关系还显示 `step.rs` 被同 crate 的 `task.rs` 直接引用，crate 根再将符号公开给其他 crate。

本文件定义的是共享词汇而不是完整状态机。流程能否跳过某步、何时推进、怎样写回存储，必须到对应 scheduler/planner/storage 实现中核对，不能从常量排列推断。

## 错误处理与边界

这些 API 不返回 `Result`、不 panic，也不做 I/O。无法识别时用字符串兜底，而不是抛错：已知类型的未知编号走 `unknownStepStr`，未知类型走 `unknown type`。

主要边界如下：

- 框架标记优先于任务类型判断；即使任务类型未知，`StepInit` 等仍有稳定名称。
- `IsValidStep` 是基于字符串哨兵的兼容实现，不是任务类型和编号的独立注册表。它会拒绝已知类型的未知编号，但不会拒绝未知类型；扩展时不能假设它等价于“类型与编号都已登记”。
- `Step` 只是 `i64` 别名，没有编译期范围限制、枚举穷尽检查或反序列化校验。
- `ImportStepMergeSort`、`ImportStepCollectConflicts`、`ImportStepConflictResolution` 以及某些 backfill 阶段允许产生零个 subtask；零 subtask 不等同于非法 step。
- `ImportStepCollectConflicts` 被刻意与冲突解决分开：其不修改下游数据、可重试并维持正确 checksum；冲突解决若混入同一步，中途重试会破坏 checksum 计算。多唯一索引冲突去重当前在内存中完成，冲突过多时后续 checksum 可能被跳过。

这些边界来自 `step.rs` 注释，并由 `step_test.rs` 对未知编号、未知类型和框架标记合法性进行验证。

## 并发与资源生命周期

本文件没有锁、原子变量、异步任务、线程、通道、事务、文件句柄或网络连接。所有常量不可变，函数只读输入并返回新字符串，因此自身可重入，资源生命周期仅限调用栈和返回 `String` 的堆分配。

并发语义存在于消费者而非这里：DXF owner 依序推进 task step，同一 step 下的多个 subtask 可在节点间并行执行（见 `pkg/dxf/framework/doc.go`）。本文件只保证这些参与持久化和跨组件判断的编号/名称一致。对 `ImportStepCollectConflicts` 的幂等说明是业务执行阶段的重试约束，不表示本文件执行任何去重或 checksum 工作。

性能上，`Step2Str` 与两个合法性函数都是常数时间的小型匹配；每次格式化都会分配 `String`。若在高频路径优化分配，必须同时维持公开返回类型、未知值格式及 Go 对齐，不能仅局部改成不同的字符串协议。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/dxf/framework/proto/step.go`。Rust 版本逐项保留了 Go 的 `Step int64`、三个负数框架标记、三组业务常量、字符串映射、未知值格式和合法性逻辑。Go 的 `switch` 对应 Rust 的 `match`；Go 的 `fmt.Sprintf` 对应 Rust 的 `format!`；Go 的 `strings.Contains` 对应 Rust `str::contains`。

当前可见差异主要是语言表达，不是业务差异：Rust 的 `TaskType` 是同 crate 中的 `&'static str` 别名，返回拥有所有权的 `String`；模块通过 `lib.rs` 重导出；命名保留 Go 风格并由 crate 根的 lint allow 支持。`pkg/dxf/framework/proto/Cargo.toml` 的 porting metadata 明确将整个 crate 对应到 Go 包。

测试也成对存在：`pkg/dxf/framework/proto/step_test.rs` 对应 `step_test.go`，两者覆盖 Backfill、ImportInto、Example、未知编号、未知类型、框架标记和业务步判断。Rust 测试另外显式断言未知任务类型分支；`migration_aster_unit_test.rs` 再覆盖 `BackfillStepMergeTempIndex`、冲突解决阶段和 prepare 合法性，防止迁移漂移。

## 扩展指南

新增任务类型或阶段时，应按以下边界接入：

1. 若新增 `TaskType`，先在 `pkg/dxf/framework/proto/type.rs` 及其 Go 对照中定义；随后在 `Step2Str` 增加任务类型分支，并添加一个私有的该类型 step 映射函数。只增加常量而不接入 `Step2Str` 会使已知业务阶段被报告为未知。
2. 若给现有类型新增业务 step，追加一个从未使用的正数常量，并同步对应私有映射。不得改动或复用已发布编号；还要核对存储、scheduler、planner、executor、历史统计和回滚路径是否都理解新阶段。
3. 若新增框架标记步，需要在 `Step2Str` 的类型分派之前处理，并明确它是否应由 `IsValidBusinessStep` 排除；同时更新默认流/prepare 流相关调度代码。负数编号同样必须保持持久化兼容。
4. 若想收紧未知任务类型的合法性，必须把它作为显式兼容变更处理：当前 Go/Rust 都只搜索 `unknown step`，直接更改会影响调用者观察，不能当作内部重构。
5. 同步扩展独立测试 `pkg/dxf/framework/proto/step_test.rs`，不要把测试嵌入生产源文件；同时更新 `step_test.go` 或至少核对 Go 行为。迁移契约发生变化时还应补充 `migration_aster_unit_test.rs`。

兼容风险集中在持久化数字和公开字符串；正确性风险集中在遗漏某个 scheduler/planner/executor 分支；性能风险通常很低，但新增基于大表或动态注册的映射时应避免把锁或线性扫描引入高频格式化路径。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；本次查询时目标文件已被索引。
- RustCodeGraph `files --filter pkg/dxf/framework/proto`：确认 `step.rs`、crate 入口、Go 对照和 Rust/Go 独立测试均位于同一协议包边界。
- RustCodeGraph `node --file pkg/dxf/framework/proto/step.rs`：逐行核对全部 226 行，确认类型别名、17 个公开 step 常量（3 个框架标记、3 个示例阶段、7 个 ImportInto 阶段、4 个 Backfill 阶段）、3 个公开函数、4 个私有函数、1 个私有字符串常量，以及不存在条件编译项。
- RustCodeGraph `explore "Step2Str IsValidBusinessStep ..."`：确认内部调用边 `IsValidBusinessStep → IsValidStep → Step2Str`、三类映射分派、`unknownStepStr` 汇合，以及 `TaskBase::String → Step2Str` 的直接上游。
- 已读源码/配置：`pkg/dxf/framework/proto/step.rs`、`lib.rs`、`task.rs`、`type.rs`、`Cargo.toml`，以及框架契约 `pkg/dxf/framework/doc.go`。
- 已读对照与测试：`pkg/dxf/framework/proto/step.go`、`step_test.go`、`step_test.rs`、`migration_aster_unit_test.rs`。
- 使用 `rg` 核对未由代码图完整展开的消费点，确认 import-into 的 scheduler、planner、executor、job 与 history 模块直接引用 `ImportStep*`/`Step2Str`。本任务是纯文档分析，按计划不运行 Cargo。
