# `pkg/ddl/dist_owner.rs`

## 文件定位

[`dist_owner.rs`](./dist_owner.rs) 属于 `astersql-ddl` crate；[`lib.rs`](./lib.rs) 通过 `pub mod dist_owner;` 将其公开。文件当前不是 DDL owner 选举器或分布式任务执行器，而只是把 Go 版的两个 backfill（回填）轮询周期移植成 `std::time::Duration` 全局量。

当前 Rust 接线状态必须与设计意图区分开：RustCodeGraph 对该文件报告 `used by 0 files`，全仓 Rust 精确搜索也只命中变量定义自身。因此它目前是可公开访问、但没有被 Rust 生产流程消费的配置模块；完整运行行为仍只能由 Go 对照代码证明，不能据此声称 Rust 已实现对应的 owner 监控循环。

## 核心职责

本文件仅承担两项默认时间参数的声明：

- `CheckBackfillJobFinishInterval`：默认 300 毫秒，表达检查分布式 backfill 任务是否结束、暂停或取消的频率。
- `UpdateBackfillJobRowCountInterval`：默认 3 秒，表达把分布式任务已处理行数刷新到 DDL job 进度的频率。

它不创建定时器，不提交或等待任务，不更新行数，不参与 owner 选举，也不处理 DDL job 状态。上述动作在 Go 版本由 [`index.go`](./index.go) 的分布式 backfill 等待循环完成；Rust 侧尚无对应调用者。

## 主要符号

- `use std::time::Duration`：唯一依赖，引入标准库的非负时间跨度类型；本文件不依赖 `pkg/ddl/Cargo.toml` 中的任何第三方或工作区 crate。
- `pub static mut CheckBackfillJobFinishInterval: Duration = Duration::from_millis(300)`：公开、进程级、可变的完成检查周期。名字保持 Go 导出变量的拼写，因此没有采用 Rust 通常使用的全大写静态量命名。
- `pub static mut UpdateBackfillJobRowCountInterval: Duration = Duration::from_secs(3)`：公开、进程级、可变的进度刷新周期，同样直接对应 Go 包变量。

文件没有常量、结构体、枚举、trait、函数、`impl` 或条件编译项，也没有文件内测试。

## 执行流程

Rust 当前没有执行流程：crate 装配阶段由 `lib.rs` 声明模块，两个 `Duration` 在程序静态存储期初始化，之后没有仓库内 Rust 代码读取或修改它们。

作为语义对照，Go 的真实流程位于 `index.go`：分布式 backfill 任务提交后，一个 goroutine 同时创建两个 ticker；完成检查 ticker 每次触发时调用 `checkRunnableOrHandlePauseOrCanceled`，行数 ticker 每次触发时调用 `updateDistTaskRowCount`。任务完成通道关闭时，还会最终刷新一次行数并再次检查任务状态，退出时停止两个 ticker。这个流程解释了两个周期“为何存在”，但不是本 Rust 文件当前已经具备的行为。

## 数据与状态

两个值均为 `Duration`，默认值分别是 300 毫秒和 3 秒。它们没有封装、校验或恢复机制，状态作用域是整个进程，而不是某个 DDL job、owner 任期或 worker 实例。

`static mut` 允许调用方改写配置，但 Rust 中读取或写入可变静态量都需要 `unsafe`，并且不会自动提供原子性或同步保证。当前没有调用者，所以仓库内尚未发生数据竞争；若未来多个线程在定时循环运行期间写入这些值，就必须额外定义同步和可见性协议。`Duration` 本身只表达间隔，不保存上次触发时间、任务进度或 ticker 句柄。

## 依赖与调用关系

上游装配关系是 `pkg/ddl/lib.rs` → `pub mod dist_owner`；crate 边界由 [`Cargo.toml`](./Cargo.toml) 的 `[package] name = "astersql-ddl"` 和 `[lib] path = "lib.rs"` 确认。

RustCodeGraph 的文件节点能够读取完整源码，但报告 `used by 0 files`；对两个变量的 `query`、限定名 `callers` 和 `callees` 查询均没有符号结果。`rg` 复核表明，Rust 中除定义和一段保留 Go 文本的测试辅助数据外没有引用。因而当前不存在可陈述的 Rust 上游调用者或下游被调用函数。

Go 侧直接依赖边为 `index.go` 的等待循环 → 两个周期变量 → `time.NewTicker`。测试侧，`main_test.go` 和 `tests/serial/main_test.go` 把完成检查周期缩短为 50 毫秒；`tests/indexmerge/merge_test.go` 在冲突场景中临时改为 50 毫秒，并用 `defer` 恢复旧值。

## 错误处理与边界

本文件没有返回值或错误类型，也不会自行报告非法配置。默认构造不会失败。若未来调用方把值改为 `Duration::ZERO`，本文件不会阻止它；实际定时器 API 是否接受零周期必须由消费端处理，不能由当前代码推断。

周期只影响检查或进度可见性的时延，不代表任务超时：300 毫秒不是 backfill 完成期限，3 秒也不是行数更新必须成功的期限。Go 对照流程中的暂停、取消、任务完成和行数更新错误边界属于消费端 `index.go`，不属于这两个声明本身。

## 并发与资源生命周期

两个静态量与进程同生命周期，没有析构工作，也不拥有线程、channel、锁、ticker 或任务句柄。当前 Rust 无消费者，因此没有由本文件创建或释放的运行时资源。

未来接线时，直接照搬 Go 的“测试可改全局变量”会引入 Rust 特有风险：`static mut` 的并发读写需要 `unsafe`，并行测试还可能互相污染。消费端还应确保 ticker/异步任务在完成、错误和取消路径都被释放。Go `index.go` 用 `defer ...Stop()` 和任务完成通道体现了这一生命周期要求；Rust 实现应优先把周期注入 owner/worker 实例，或使用同步配置容器，而不是在运行中无保护地改写这两个静态量。

## 与 Go 版本的对应关系

[`dist_owner.go`](./dist_owner.go) 同样只声明两个包变量，默认值与 Rust 完全一致：`300 * time.Millisecond` 对应 `Duration::from_millis(300)`，`3 * time.Second` 对应 `Duration::from_secs(3)`。名称和注释意图也保持一致。

差异在于语言语义与接线完成度。Go 包变量可直接并发读写（尽管仍可能产生数据竞争），而 Rust `static mut` 的访问必须进入 `unsafe`。更重要的是，Go `index.go` 已用两者创建 ticker 并驱动状态检查与行数更新；Rust `index.rs` 没有引用它们，所以当前只是声明层移植。Rust `main_test.rs` 用局部不可变 `DdlTestEnvironment.backfill_finish_interval` 验证 50 毫秒的测试配置意图，没有修改本模块的全局量；`tests/serial/main_test.rs` 中的相关内容只是返回 Go 源码片段的字符串，也不构成行为测试。

## 扩展指南

若要完成 Rust 接线，最可能修改的是消费分布式 backfill 任务的等待逻辑，而不是继续扩充本文件。实现应把“完成/暂停/取消检查”和“进度刷新”分别绑定到这两个周期，并覆盖完成通道优先退出、退出前最后一次刷新、错误传播和定时资源释放。

建议不要让新代码长期依赖裸 `static mut`：可将两个周期组合为配置结构并在 worker/owner 构造时注入；若必须支持运行期全局修改，则使用有明确内存序或锁语义的同步容器。任何修改都要保持 Go 默认值和可观测节奏兼容，并评估更短周期带来的状态查询、元数据写入和调度开销。

测试逻辑应放在独立测试文件，不能内嵌进 `dist_owner.rs`。可扩展现有 [`main_test.rs`](./main_test.rs) 验证默认/注入配置；真正接线时还需要在相应独立 worker/index 测试中用可控时钟或短周期验证两类 tick、完成退出、取消/暂停错误以及最终行数刷新。Go 回归意图可参考 `main_test.go`、`tests/serial/main_test.go` 和 `tests/indexmerge/merge_test.go`。

## 验证依据

- 源码与装配：`pkg/ddl/dist_owner.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Go 对照与真实消费端：`pkg/ddl/dist_owner.go`、`pkg/ddl/index.go`。
- 测试证据：`pkg/ddl/main_test.go`、`pkg/ddl/main_test.rs`、`pkg/ddl/tests/serial/main_test.go`、`pkg/ddl/tests/serial/main_test.rs`、`pkg/ddl/tests/indexmerge/merge_test.go`。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`explore` 和 `node --file pkg/ddl/dist_owner.rs` 核对了完整定义，文件节点报告 `used by 0 files`；变量 `query/callers/callees` 没有结果。
- 文本复核：对 `CheckBackfillJobFinishInterval`、`UpdateBackfillJobRowCountInterval` 和 `dist_owner` 的限定搜索确认 Rust 无运行时引用，并定位了上述 Go 消费端与测试。
- 按任务约束未运行 Cargo；本次只新增说明文档，结构由固定标题检查验证。
