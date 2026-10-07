# `pkg/dxf/framework/proto/type.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-proto` crate，crate 根由 [`lib.rs`](./lib.rs) 以 `pub mod r#type` 声明模块，并通过 `pub use r#type::*` 将本文件的公开项同时暴露到 crate 根。之所以写成原始标识符 `r#type`，是因为 `type` 是 Rust 关键字；调用方既可以使用 `astersql_dxf_framework_proto::r#type::ImportInto`，也可以使用根级再导出的 `astersql_dxf_framework_proto::ImportInto`。

该文件是 DXF（Distributed eXecution Framework）任务类型的协议常量与紧凑整数编码表。它不负责创建、调度或执行任务，而是给这些层提供稳定的类型标识。字符串类型本身定义在 [`task.rs`](./task.rs) 的 `pub type TaskType = &'static str`；本文件通过 `use super::task::TaskType` 复用该协议类型。

[`Cargo.toml`](./Cargo.toml) 将 crate 的 Go 对照包声明为 `pkg/dxf/framework/proto`，并把 `lib.rs` 指定为库入口。`type.rs` 自身只依赖同 crate 的 `TaskType`，没有直接使用该 manifest 中的 `bytesize`、`chrono`、`serde` 或 `serde_json` 外部依赖。

## 核心职责

本文件承担两项职责：

1. 定义框架内置任务类型的规范字符串：测试/示例任务 `"Example"`、IMPORT INTO 任务 `"ImportInto"`、加索引回填任务 `"backfill"`。字符串的大小写是协议的一部分，不能按 Rust 命名习惯自行改写。
2. 在这三个规范字符串与固定 `i32` 编码 `1`、`2`、`3` 之间转换；未知字符串编码为 `0`，未知整数解码为空字符串。这一兜底行为与 [`type.go`](./type.go) 一致。

它刻意不是封闭枚举：`TaskType` 是 `&'static str`，因此框架其他位置可以携带未在本文件登记的静态任务类型。代价是整数转换只认识本文件的显式映射，未知值会丢失原值，不能无损往返。

## 主要符号

- `pub const TaskTypeExample: TaskType = "Example"`：示例和测试任务的类型标识。它在框架集成测试、handle 测试和 mock 中用于构造通用任务，例如 `pkg/dxf/framework/integrationtests/framework_test.rs`。
- `pub const ImportInto: TaskType = "ImportInto"`：分布式 IMPORT INTO 任务标识。生产调用方包括 `pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/clean_up.rs` 与 `pkg/dxf/importinto/scheduler.rs`；`step.rs::Step2Str` 也用它选择导入任务的 step 名称表。
- `pub const Backfill: TaskType = "backfill"`：分布式加索引 backfill 任务标识。`step.rs::Step2Str` 用它选择 backfill step 名称表，`pkg/dxf/framework/dxfmetric/migration_aster_unit_test.rs` 还验证它作为指标标签的任务类型。
- `pub fn Type2Int(t: TaskType) -> i32`：纯匹配转换。三个已知常量依次返回 `1`、`2`、`3`，其余任何字符串返回 `0`。
- `pub fn Int2Type(i: i32) -> TaskType`：反向纯匹配转换。`1`、`2`、`3` 返回相应静态常量，其余整数返回空字符串 `""`。

本文件没有 trait、struct、enum、宏、可变静态量、条件编译项或私有辅助函数。所有五个符号都是公开 API；crate 根的再导出进一步使它们成为 crate 根 API。

## 执行流程

编码路径 `Type2Int` 的流程只有一次 `match`：接收一个静态字符串引用，按内容与三个规范常量比较，命中后返回固定编号，否则进入 `_ => 0`。它不分配内存、不修改输入，也不调用其他函数。

解码路径 `Int2Type` 同样只有一次 `match`：按整数选择已编译进程序的静态字符串，未知编号统一返回空字符串。返回值是 `&'static str`，因此不存在临时字符串所有权转移。

对已知集合，满足 `Int2Type(Type2Int(x)) == x`；[`type_test.rs`](./type_test.rs) 覆盖三个已知常量和空字符串。对未知集合不满足一般意义的双射：例如任意未知任务类型会先压缩成 `0`，再解码为 `""`；任意不在 `1..=3` 的整数也都会解码为同一个空字符串。[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 额外验证 `ImportInto` 的往返和 `Int2Type(0) == ""`。

## 数据与状态

三个任务类型都是编译期常量，底层数据为只读静态字符串。`TaskType` 不携带判别值、所有权或运行时注册信息；相等判断就是字符串内容相等。

整数映射 `0/1/2/3` 没有单独的数据结构保存，而是直接写在两个函数的 `match` 分支中。`0` 是“未知/空”的哨兵值，不是一个具名任务类型；`1..=3` 才代表当前登记的类型。新增映射时必须保持两个函数同步，否则会产生只能编码或只能解码的单向协议。

本文件没有进程内可变状态、缓存、配置项或持久化操作。整数值的实际持久化语义由调用它们的存储层决定；当前仓库的 `pkg/dxf/framework/storage/lib.rs` 内还存在独立的同名兼容映射，必须与本文件保持一致，但那组函数不是本文件函数的直接调用者。

## 依赖与调用关系

下游依赖只有 [`task.rs`](./task.rs) 的 `TaskType` 类型别名。RustCodeGraph 对 `Type2Int` 和 `Int2Type` 均报告“无 callee”，与源码中的纯 `match` 实现一致。

上游装配关系为 `lib.rs -> r#type`，随后 `lib.rs` 将全部符号再导出。常量的直接使用面比转换函数更广：

- [`step.rs`](./step.rs) 读取 `Backfill`、`ImportInto`、`TaskTypeExample`，在 `Step2Str` 中按任务类型选择不同 step 转换表。
- `pkg/dxf/importinto/task_executor.rs` 用 `ImportInto` 构造任务和子任务，`pkg/dxf/importinto/clean_up.rs` 与 `scheduler.rs` 用它筛选或定位导入任务逻辑。
- DXF handle、integrationtests、mock 和 dxfmetric 的代码/测试使用这些常量构造任务或形成指标标签。

RustCodeGraph 对同名符号的全仓搜索会同时命中 Go 实现以及 `pkg/dxf/framework/storage/lib.rs` 中 storage crate 自己的兼容 `proto` 模块。原始引用搜索确认，storage 的 `task_table.rs::insertSubtasks` 与 `converter.rs::row2BasicSubTask` 调用的是 storage crate 自有的 `proto::Type2Int`/`proto::Int2Type`；不能把这两条边当成本文件函数的直接调用边。当前本文件两个转换函数的可确认 Rust 调用者是 [`type_test.rs`](./type_test.rs) 和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。

## 错误处理与边界

两个转换函数都不返回 `Result`、不 panic，也不记录日志。未知输入被设计为正常兜底：字符串转 `0`，整数转空字符串。这与 Go 的 `switch default` 语义相同，但会静默丢失未知输入信息，因此调用方如果需要区分不同未知值，必须在转换前自行校验，不能依赖往返结果。

边界包括负数、`0`、大于 `3` 的整数以及不等于三个常量的任意静态字符串；它们全部进入默认分支。大小写敏感，例如 `"Backfill"` 不等于规范值 `"backfill"`，会被编码为 `0`。

类型层面的另一边界是 `TaskType = &'static str`：运行时拥有的普通 `String` 不能直接作为参数，必须先由上层映射到静态常量或采用其自身的驻留策略。此约束来自 `task.rs::TaskType`，不是本文件执行时施加的检查。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务、I/O 或堆资源。常量与函数都是无状态且只读的，可由任意线程并发调用。

`Int2Type` 返回的是静态常量或静态空字符串，生命周期覆盖整个程序；`Type2Int` 只借用同样具有 `'static` 生命周期的输入。两条路径都不分配内存，因此调用成本主要是少量字符串比较或整数分支，资源释放和取消语义均不适用。

## 与 Go 版本的对应关系

直接对照文件是 [`type.go`](./type.go)，对应测试是 [`type_test.go`](./type_test.go)。Rust 保留了 Go 的全部三个常量、精确字符串值、编号顺序以及默认分支：`Example/ImportInto/backfill <-> 1/2/3`，未知值分别落到 `0` 和空任务类型。

可见差异主要来自语言类型：Go 的 `TaskType` 是具名字符串类型、转换函数使用平台相关宽度的 `int`；Rust 的 `TaskType` 是 `&'static str` 别名、整数固定为 `i32`。在当前仅使用 `0..=3` 的协议范围内，整数宽度差异不改变结果。Rust 的函数和常量保留 Go 风格大写命名，并由 `lib.rs` 的 crate 级 `allow(non_snake_case, non_upper_case_globals)` 接受，以降低移植接口差异。

Rust 独立测试 [`type_test.rs`](./type_test.rs) 与 Go `TestTaskType` 使用同一张四行用例表，分别验证两个方向；迁移测试还覆盖跨函数往返。当前 Rust 实现不是桩，也没有删减 Go 分支。

## 扩展指南

新增内置任务类型时，应在本文件完成一个原子协议变更：增加 `TaskType` 常量，为 `Type2Int` 分配从未使用的固定整数，并在 `Int2Type` 增加完全对称的反向分支。不要复用既有编号或改变 `1/2/3` 的含义，因为编号可能进入存储格式；同时应同步 Go 的 [`type.go`](./type.go) 以及 storage crate 当前维护的兼容映射 `pkg/dxf/framework/storage/lib.rs`，避免不同边界采用不一致的编码表。

测试必须放在独立文件 [`type_test.rs`](./type_test.rs)，为新类型补充正向、反向与往返断言；若属于跨模块迁移契约，还应更新 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。Go 对照测试 [`type_test.go`](./type_test.go) 也应同步。不要把 Rust 测试内嵌到 `type.rs`。

若新任务有自己的业务 step，需要同步检查 [`step.rs`](./step.rs) 中的 `Step2Str`、合法性判断及独立 `step_test.rs`。若仅需要字符串任务类型而不需要持久化整数编码，应先确认是否应该进入本映射；随意加入编号会扩大兼容承诺。主要风险是持久化兼容和 Go/Rust 双实现漂移，性能风险很低，因为映射规模小且为常数分支。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件、307,296 个节点和 1,848,419 条边；用 `node --file pkg/dxf/framework/proto/type.rs` 读取了目标文件全部 54 行。
- 符号查询：`query Type2Int --kind function`、`query Int2Type --kind function` 区分了 Go、proto Rust 与 storage Rust 的同名实现；`node TaskType --file pkg/dxf/framework/proto/task.rs` 确认别名为 `&'static str`。
- 调用查询：对 `type.rs` 中 `Type2Int`、`Int2Type` 执行 `callers`/`callees`；两者均无下游调用，caller 图未给出目标限定结果。随后以精确 Rust 引用搜索核验实际调用点，仅目标 crate 的 `type_test.rs` 与 `migration_aster_unit_test.rs` 直接调用本文件函数；storage 调用落到其自有同名实现。
- 已读生产/装配文件：[`type.rs`](./type.rs)、[`task.rs`](./task.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)、[`step.rs`](./step.rs) 的相关引用，以及 storage 的 `lib.rs`、`task_table.rs::insertSubtasks`、`converter.rs::row2BasicSubTask` 作为同名边界辨析证据。
- 已读对照与测试：[`type.go`](./type.go)、[`type_test.go`](./type_test.go)、[`type_test.rs`](./type_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。这些文件共同证明已知映射、未知兜底和 Go/Rust 对齐关系。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `rg` 命令验证本文恰好包含十一个固定二级章节，并人工复核本文回答了文件存在目的、执行方式、直接依赖、真实调用边界和安全扩展方式。
