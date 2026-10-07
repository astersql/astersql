# `pkg/dxf/framework/proto/subtask.rs`

## 文件定位

本文件属于 Cargo crate `astersql-dxf-framework-proto`。`pkg/dxf/framework/proto/lib.rs` 以 `pub mod subtask` 声明模块，并通过 `pub use subtask::*` 将其公开协议类型提升到 crate 根；`pkg/dxf/framework/proto/Cargo.toml` 则把 Go 对照包登记为 `pkg/dxf/framework/proto`。文件直接依赖相邻模块的 `Step`、`TaskType`，以及标准库原子整数、时间和解引用 trait；`bytesize::KIB` 仅用于资源摘要格式化。

在 DXF 主链中，它同时定义两组共享模型：一组是调度器、存储层和执行器交换的子任务状态/载荷；另一组是执行器用来描述和无锁核算 CPU、内存配额的 `Allocatable` 与 `StepResource`。包级契约 `pkg/dxf/framework/doc.go` 说明，一个 task 的 step 顺序执行、每个 step 的多个 subtask 可跨节点并行，而同一任务的 subtask 不会在同一节点并发运行；本文件把这套抽象落为协议数据，但不负责持久化、调度或执行。

## 核心职责

- 定义六个子任务状态常量及字符串适配，保持数据库/Go 线值稳定。
- 用轻量的 `SubtaskBase` 承载调度常用字段，避免列表和调度路径总是加载可能很大的 `Meta`；用 `Subtask` 叠加元数据、更新时间和摘要。
- 提供 `NewSubtask` 的统一初始值，以及 `SubtaskBase::IsDone` 的终态判断。
- 用 `Deref`/`DerefMut` 模拟 Go 匿名嵌入字段，让 `Subtask` 可直接访问和修改基础字段。
- 用 `Allocatable` 的原子 CAS 循环实现共享配额占用/释放；用 `StepResource` 汇总 CPU、内存容量并计算每核内存。
- 私有的 `bytes_size`、`format_go_general_4` 对齐 Go `docker/go-units.BytesSize` 的二进制单位和四位有效数字展示。

本文件不校验状态迁移、不保证 `Meta` 的业务格式、不分配数据库 ID，也不把子任务写入存储；这些属于调用方职责。

## 主要符号

- `SubtaskState = &'static str` 与 `SubtaskStateExt::String`：状态类型仍是字符串别名，不是封闭 enum。公开常量为 `pending`、`running`、`succeed`、`failed`、`canceled`、`paused`。
- `SubtaskBase`：公开字段包括 `ID`、`Step`、`Type`、`TaskID`、`State`、`Concurrency`、`ExecID`、`CreateTime`、`StartTime`、`Ordinal`。`String` 输出固定字段子集；`IsDone` 只把成功、失败、取消视为完成。
- `Subtask`：拥有 `SubtaskBase`、`UpdateTime`、`Meta: Vec<u8>`、`Summary`。`Meta` 是具体 step 的输入/结果载荷，正常执行路径应只读，约定的完成回调可以改写后再由框架持久化。
- `NewSubtask(...) -> Box<Subtask>`：构造堆拥有对象；ID 为 0、状态和摘要为空、三个时间字段为 `SystemTime::UNIX_EPOCH`，其余参数原样写入。
- `Deref` / `DerefMut for Subtask`：把字段访问投影到内嵌的 `SubtaskBase`，不复制数据。
- `Allocatable { capacity, used }`：容量创建后不可变，已用量为私有 `AtomicI64`。公开方法为 `Capacity`、`Used`、`Alloc`、`Free`。
- `StepResource { CPU, Mem }`：拥有两个 `Allocatable`；`String` 输出容量摘要，`MemoryPerCore` 在 CPU 容量正数时执行整数除法，否则返回全部内存。
- `bytes_size`、`format_go_general_4`：文件私有格式化函数。文件没有条件编译项、异步函数或模块级可变状态。

## 执行流程

子任务对象的典型流程是：上游根据 step、父任务、目标 executor、并发度、序号和业务 meta 调用 `NewSubtask`；存储侧随后分配 ID、写入初始状态和时间；调度/执行路径通过 `DerefMut` 更新基础字段，业务 executor 读取 `Meta` 并在完成阶段按约定写回结果；观察或清理路径可用 `IsDone` 判定三个终态。RustCodeGraph 当前明确记录 `NewSubtask` 被 proto 单测、mock 迁移测试和 import-into 冲突处理生命周期测试调用；其他运行时路径也会直接构造 `Subtask`/`SubtaskBase`，例如 `pkg/session/runtime/import_file.rs::execute_import_file` 和 `pkg/session/runtime/modify_column_dist_backfill.rs::subtask`。

资源核算流程是：调用方以 `NewAllocatable(capacity)` 创建 CPU/内存池并装入 `StepResource`；`Alloc(n)` 读取 `used`，用回绕加法计算候选值，超过容量立即失败，否则以顺序一致性 CAS 抢占，竞争失败便重新读取和重试；任务释放资源时 `Free(n)` 用原子减法归还。执行器收到资源变更后读取 `Capacity` 或 `MemoryPerCore` 调整内部并发/缓存，例如 RustCodeGraph 定位到 `pkg/dxf/importinto/task_executor.rs` 的多个 `ResourceModified` 实现会构造或消费 `StepResource`。

格式化时，`StepResource::String` 读取两项总容量；内存值经 `bytes_size` 按 1024 逐级换算，再由 `format_go_general_4` 选择普通或科学计数法，从而保持 Go 诊断文本兼容。该展示不读取当前 `used`。

## 数据与状态

`SubtaskBase` 是调度视图，`Subtask` 是包含业务载荷的完整视图。`TaskID + Step + Ordinal` 表达子任务在父任务阶段内的位置；源码约定 `Ordinal` 从 1 开始且同 task/step 唯一，但构造器不会检查。`ExecID` 当前约定为 `IP:PORT`。`Concurrency` 初始通常等于任务 required slots，源码明确标记其尚未全面使用，轻量 step 可将其调低；不能把它误写成已经生效的统一执行限流器。

状态机依据 `pkg/dxf/framework/doc.go` 包含 pending、running、paused 及三个终态；节点被判死时还可能把 running 重置为 pending。`IsDone` 因此故意排除 pending、running、paused。由于 `SubtaskState` 是字符串别名，空值和未知值都能存在；`NewSubtask` 就以空字符串初始化，真正状态由持久化/调度路径设置。

`Meta` 由 `Vec<u8>` 独占，避免 Go `[]byte` 那样默认共享底层切片；同 step 的不同 subtask 按协议应具有不同 meta。时间字段使用 `SystemTime`，构造时以 Unix epoch 表示“尚未由框架填写”。`Allocatable.capacity` 不变、`used` 可原子变化；其 API 忠实保留 Go 的有符号整数回绕和无下界保护，因此数学意义上的 `0 <= used <= capacity` 只在调用者传入非负且成对的分配/释放量时成立。

## 依赖与调用关系

直接源码依赖为 `super::step::Step`、`super::task::TaskType`、`std::sync::atomic::{AtomicI64, Ordering}`、`SystemTime`、`Deref`/`DerefMut` 和 `bytesize::KIB`。crate manifest 还声明 `chrono`、`serde`、`serde_json`，但本文件不直接使用后三者；序列化与数据库转换发生在相邻 storage 层。

RustCodeGraph 的关键边包括：

- `NewSubtask -> Subtask -> SubtaskBase`：构造和拥有关系；当前图中直接调用者包括 `subtask_test.rs::test_subtask_is_done`、mock 迁移测试与 import-into 冲突处理测试。
- `Subtask`/`SubtaskBase` 的构造者分布于 example、framework integration、import-into 与 session runtime，说明该类型是框架与具体业务 step 的公共边界。
- `Alloc -> AtomicI64::{load, compare_exchange}`、`Free -> fetch_sub`；`MemoryPerCore -> Capacity`。
- `StepResource` 被 `pkg/dxf/importinto/task_executor.rs` 的多种 executor 及其 `ResourceModified` 路径构造/消费；框架 taskexecutor 和 session runtime 也以同一类型表达动态资源。
- `pkg/dxf/framework/proto/lib.rs` 对外再导出所有符号，调用方通常从 crate 根或 `proto` 模块引用，而不必显式写 `subtask` 子模块。

## 错误处理与边界

本文件没有 `Result` 或显式业务错误。`NewSubtask` 不拒绝空 `ExecID`、空 meta、零/负并发、零序号或未知 step/type/state；协议合法性必须由创建者和存储层保证。`IsDone` 对所有未知字符串返回 false，这会把损坏或新增但未同步的终态当作未完成，因此新增状态时必须同步此方法和状态机测试。

`Alloc(n)` 只检查回绕后的 `next > capacity`：负数分配、整数溢出以及已经为负的 `used` 不会被拒绝；`Free(n)` 也不检查释放是否超过已用量。`test_allocatable_integer_boundary_matches_go` 明确把这种二补数回绕视为 Go 兼容行为，而不是安全验证。生产调用者必须传递语义正确的非负值并保持 Alloc/Free 成对，否则资源统计可为负或回绕。CAS 循环在持续竞争下没有公平性承诺。

`MemoryPerCore` 对 CPU 容量小于等于 0 返回总内存，避免除零；正数 CPU 使用截断整数除法。资源摘要把 `i64` 转为 `f64`，极大整数可能失去低位精度，但当前测试锁定的是 Go 展示兼容而非精确可逆。私有格式化器内部对自己生成的有限小数执行 `parse::<f64>().unwrap()`；当前输入来自 `i64 -> f64`，不会产生无法解析的字符串。

## 并发与资源生命周期

`Subtask`、`SubtaskBase` 和 `StepResource` 本身不含锁、任务、通道或外部句柄；所有权随普通 Rust 值移动并在离开作用域时释放。`DerefMut` 要求独占可变借用，防止同一 `SubtaskBase` 被安全 Rust 同时写入。文件不启动线程，也不持有数据库事务。

`Allocatable` 是唯一明确的并发原语。`used` 的读、CAS、减法全部使用 `Ordering::SeqCst`，为所有线程提供单一全序；`capacity` 创建后只读，因此共享 `&Allocatable` 即可操作。`pkg/dxf/framework/proto/subtask_test.rs::test_allocatable` 用 `Arc` 在 10 个线程中各执行 10,000 次确定性 Alloc/Free，并验证 join 后 used 回到 0。类型没有 RAII permit：成功分配后若调用路径提前返回、panic 或忘记 `Free`，容量不会自动归还；扩展时可在上层封装 guard，但不能无意改变现有手工配对语义。

子任务资源生命周期与包级约束相连：同一 task 的同一节点不会同时执行多个 subtask，所以 `StepResource` 同时可表示 step 上限和单个 subtask 上限；跨节点的 subtask 仍可并行。本文件只表达该配额，不负责 executor 停止、资源重分配或 meta 持久化的时序。

## 与 Go 版本的对应关系

权威对照为 `pkg/dxf/framework/proto/subtask.go`，状态常量、字段集合、字符串输出、完成态集合、CAS 分配算法和每核内存算法均逐项对齐。独立测试 `subtask_test.go` 与 `subtask_test.rs` 都覆盖六状态终态判断、容量边界及并发分配归零；Rust 测试另外锁定整数回绕和 `docker/go-units.BytesSize` 的显示细节。

已确认的语言差异与迁移边界如下：

- Go `SubtaskState` 是独立的 `string` 类型，Rust 是 `&'static str` 别名并用扩展 trait 模拟 `String`；Rust 的类型隔离更弱且不能自然承载运行时拥有的字符串。
- Go 匿名嵌入 `SubtaskBase`，Rust 以命名字段配合 `Deref`/`DerefMut` 模拟访问；显式解构/序列化时仍需处理 `SubtaskBase` 字段。
- Go 构造器返回指针、`NewAllocatable` 返回指针、`StepResource` 保存两个指针；Rust 分别返回 `Box<Subtask>`、值类型 `Allocatable`，并由 `StepResource` 直接拥有两个池。需要共享时由调用方加 `Arc`。
- Go 零值 `time.Time{}` 是公历 1 年，Rust 构造器使用 1970-01-01 Unix epoch；二者都承担“未填写”哨兵语义，但绝不是相同时间戳，跨语言编码时不能直接等同。
- Go `[]byte` 可与调用方共享底层数组，Rust `Vec<u8>` 被移动进 `Subtask`；这改变别名方式，但保留载荷字节内容。
- Go 原子有符号运算与 Rust 的 `wrapping_add`/`fetch_sub` 都按二补数回绕；Rust 显式写 `wrapping_add` 是为了避免 debug 构建溢出 panic。

## 扩展指南

新增子任务状态时，应同时修改本文件与 `subtask.go` 的稳定字符串常量，明确其状态迁移和是否终态，并同步 `SubtaskBase::IsDone`、`pkg/dxf/framework/doc.go` 状态图、storage converter/查询条件及独立 Rust/Go 测试。不要仅添加常量，因为未知终态目前会被视作未完成。

增加 `SubtaskBase` 或 `Subtask` 字段时，应检查构造器、直接 struct literal、storage 映射和序列化边界；RustCodeGraph 已显示 session runtime、import-into、example 和测试中存在直接构造，新增必填字段会形成广泛编译影响。若字段属于大载荷，应继续留在 `Subtask` 而不是 `SubtaskBase`，以维持轻量查询设计。修改 `Meta` 语义还要同步具体 `StepExecutor` 的完成回写和相应独立测试。

资源扩展应优先接入 `Allocatable::{Alloc,Free}` 和 `StepResource`，并保持 Go 的边界语义，或明确进行跨语言一致的破坏性修正。若要增加负数/溢出保护、RAII permit 或弱化内存序，必须先补并发、异常退出和边界测试，并评估所有 `ResourceModified` 消费者。测试逻辑继续放在 `pkg/dxf/framework/proto/subtask_test.rs`，不要内嵌到生产文件；Go 对应测试为 `subtask_test.go`。

兼容性风险集中在状态线值、日志字符串和时间哨兵；正确性风险集中在新增终态遗漏、meta 只读/回写约定、整数回绕及资源泄漏；性能风险集中在 `SeqCst` 热点竞争和大 meta 被误放入基础视图。当前实现的 CAS 自旋适合短临界操作，但高争用下应以基准和真实调用证据再决定优化。

## 验证依据

- 包级与 crate 边界：`pkg/dxf/framework/doc.go`、`pkg/dxf/framework/proto/Cargo.toml`、`pkg/dxf/framework/proto/lib.rs`。
- RustCodeGraph 索引：项目状态显示 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/dxf/framework/proto` 覆盖目标、Go 对照与独立测试。
- RustCodeGraph 源码/符号：完整读取 `subtask.rs` 326 行；查询 `Subtask`、`SubtaskBase`、`NewSubtask`、`NewAllocatable`、`Alloc`、`Free`、`StepResource`、`MemoryPerCore`、`IsDone` 及其图中调用/构造轨迹。
- 生产使用证据：`pkg/dxf/importinto/task_executor.rs` 的多个 `ResourceModified`、`pkg/session/runtime/import_file.rs::execute_import_file`、`pkg/session/runtime/modify_column_dist_backfill.rs::subtask`。
- Go 对照：`pkg/dxf/framework/proto/subtask.go`；Go 独立测试：`pkg/dxf/framework/proto/subtask_test.go`。
- Rust 独立测试：`pkg/dxf/framework/proto/subtask_test.rs`，覆盖六状态判断、容量上限、并发 CAS、整数回绕和内存格式；`pkg/dxf/framework/proto/lib.rs` 通过 `#[path = "subtask_test.rs"]` 在测试配置下装配它。
- 本任务是纯文档分析，按计划未运行 Cargo。交付检查仅执行指定的 11 章节结构命令，并人工复核文档能回答文件存在原因、运行流程和安全扩展入口。
