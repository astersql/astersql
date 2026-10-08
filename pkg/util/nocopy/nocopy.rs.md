# `pkg/util/nocopy/nocopy.rs`

## 文件定位

本文件是 workspace 成员 crate `astersql-util-nocopy` 的唯一行为实现文件，源码入口为 [`pkg/util/nocopy/lib.rs`](./lib.rs)：入口声明 `pub mod nocopy`，随后用 `pub use nocopy::*` 再导出本文件的公开 API。crate 由 [`pkg/util/nocopy/Cargo.toml`](./Cargo.toml) 定义，根 [`Cargo.toml`](../../../Cargo.toml) 将它列为 workspace 成员，并以 `facade_util_nocopy` 依赖名接入根 `pkg` crate；[`pkg/lib.rs`](../../lib.rs) 又通过 `pkg::util::nocopy` 门面再导出。

它位于通用工具层，不参与 SQL 解析、规划、执行或存储请求的运行时主链。RustCodeGraph 对本文件的精确查询只识别出 `NoCopy` 及四个方法，且未找到生产调用边；仓库文本检索也只找到模块声明、门面再导出和独立测试。因此，当前 Rust 代码的实际角色是提供已接线但尚无生产 Rust 使用者的兼容标记 API，而不是一个运行时锁实现。

## 核心职责

- `NoCopy` 用零大小类型表达“拥有者不应被复制”的设计意图。
- 类型只派生 `Default`，刻意不实现 `Copy` 或 `Clone`；这使 Rust 调用者不能通过这两个标准 trait 显式复制标记值。
- `lock`/`unlock` 提供符合 Rust 命名习惯的兼容空操作，`Lock`/`Unlock` 保留 Go 方法拼写，便于移植代码维持接口形状。
- 四个方法都不取得互斥锁，也不改变任何状态；它们的存在是静态语义和迁移兼容需要，不能用于线程同步。

需要注意，Rust 类型本身默认就不可 `Copy`/`Clone`。把 `NoCopy` 字段嵌入另一个 Rust 结构，只有在该结构的复制实现受字段 trait 约束时才形成编译期限制；手写复制逻辑仍可绕过这种意图。因此它表达的是约束和审查信号，不是不可绕过的所有权机制。

## 主要符号

- `pub struct NoCopy;`：公开的单元结构体，也是零大小类型（ZST）。它没有字段、堆分配或运行时状态，并派生 `Default` 以便通过 `NoCopy::default()` 构造。
- `pub fn lock(&self)`：借用标记后立即返回的小写空操作。签名只需要共享引用，不会独占对象。
- `pub fn unlock(&self)`：与 `lock` 配对的空操作，同样立即返回。
- `pub fn Lock(&self)`：保留 Go `sync.Locker` 方法名的兼容入口；`#[allow(non_snake_case)]` 只为此命名例外服务。
- `pub fn Unlock(&self)`：与 `Lock` 配对的兼容入口，也显式放宽 snake_case lint。

本文件没有模块级常量、trait、枚举、类型别名、条件编译项或私有辅助函数。条件编译仅存在于相邻的 `lib.rs`：`#[cfg(test)]` 把独立文件 `migration_aster_unit_test.rs` 接入测试构建。

## 执行流程

典型 Rust 路径非常短：调用者构造或在结构中持有 `NoCopy`，再按需要调用某个兼容方法；方法接收 `&self` 后直接返回。由于结构体无字段，构造不需要初始化业务状态；由于方法体为空，调用过程中没有分支、循环、系统调用、锁操作或下游函数调用。

当前仓库内唯一已验证的执行实例在 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)：测试以 `NoCopy::default()` 构造标记，检查其大小为零，并连续调用两次 `lock` 和两次 `unlock`，证明小写方法允许重复、无配对状态要求且不产生可观察副作用。大写兼容方法当前没有调用者或独立运行时断言。

## 数据与状态

`NoCopy` 是无字段单元结构体，因此 `std::mem::size_of_val` 的结果为 `0`，不会为每个实例增加有效载荷，也没有需要序列化、持久化或清理的数据。`Default` 构造与直接写 `NoCopy` 等价，均不读取全局配置。

类型没有内部可变性、引用计数、生命周期参数或泛型参数。四个方法只接受 `&self`，既不消费实例也不产生返回值。独立测试用 `static_assertions::assert_not_impl_any!(NoCopy: Clone, Copy)` 在编译期锁定“不实现 `Clone`/`Copy`”这一不变量；若未来添加这两个 trait，会直接破坏本工具的核心语义。

## 依赖与调用关系

下游依赖为空：本文件只使用 Rust 语言内建的 `Default` 派生和 lint 属性，不调用其他 crate 或仓库函数。[`Cargo.toml`](./Cargo.toml) 没有普通依赖，仅在 `dev-dependencies` 中声明 `static_assertions = "1"`，供独立测试做负 trait 断言。

上游接线链为：`pkg/util/nocopy/lib.rs` 声明并再导出本模块，根 `Cargo.toml` 以 `facade_util_nocopy` 引入该 crate，`pkg/lib.rs` 再把它暴露为根门面的 `util::nocopy`。RustCodeGraph 的限定 `callers`/`callees` 查询没有给 `NoCopy` 或四个方法返回调用边；`rg` 也未发现该独立测试以外的 Rust `NoCopy` 使用。因此不能声称 Rust 生产流程已经依赖此标记。

Go 上游不同：[`pkg/sessionctx/stmtctx/stmtctx.go`](../../sessionctx/stmtctx/stmtctx.go) 在 `StatementContext` 中以空白字段 `_ nocopy.NoCopy` 嵌入标记，注释说明复制上下文会令复制后的 `TypeCtx` 指向错误的 `AppendWarnings` 函数。这是当前仓库中能确认的实际业务动机。

## 错误处理与边界

所有构造和方法都是不可失败的：没有 `Result`、`Option`、panic 分支或错误转换。调用 `lock` 后不调用 `unlock`，或反向、重复调用，都不会触发状态错误，因为实现根本不记录锁状态。

关键边界是名称可能造成误解：`lock`/`Lock` 不提供互斥、内存屏障、公平性或阻塞语义，不能替代 `Mutex`、`RwLock` 等同步原语。另一个边界是 Rust 没有 Go `go vet -copylocks` 的同构机制；当前保证仅来自不实现 `Copy`/`Clone` 以及外层结构对字段 trait 的自然继承，不能阻止调用者重新构造等价值或手工实现复制逻辑。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、文件描述符或堆资源，也没有 `Drop` 实现。`NoCopy` 的生命周期完全由拥有它的外层值决定，销毁时无需清理。

空方法仅共享借用 `&self` 且不访问状态，所以就本实现而言可被不同调用路径重复调用；但这种“可调用”不代表提供同步。`NoCopy` 是否自动具备 `Send`/`Sync` 以及外层结构是否可跨线程，还应由 Rust 自动 trait 规则和外层字段共同决定，扩展时不应把本标记当成并发安全证明。

## 与 Go 版本的对应关系

Go 对照文件是 [`nocopy.go`](./nocopy.go)：同样定义空结构 `NoCopy`，并用指针接收者实现空的 `Lock` 与 `Unlock`。这样该类型满足 `sync.Locker` 的方法集合，Go 工具链可通过 `go vet -copylocks` 识别包含它的值被意外复制；`StatementContext` 的空白字段就是这一模式的实际使用点。

Rust 版本保留了零大小、无运行时副作用及大写方法名这三部分形状，同时增加符合 Rust 风格的小写方法和 `Default`。语义差异在于 Rust 没有实现一个对应 `sync.Locker` 的 trait，也没有仓库证据表明 lint 会按 Go 的方式检查包含 `NoCopy` 的外层值。Rust 侧以“不实现 `Clone`/`Copy`”近似表达意图，独立测试明确验证这一点；因此文档不能把两边的静态检查能力描述为完全等价。

## 扩展指南

- 若只需在新的 Rust 结构中表达禁止普通复制的意图，可通过已公开的 `NoCopy` 字段接入，但应同时确认外层结构没有手工实现 `Copy`/`Clone`，且没有自定义复制函数绕开字段约束。
- 若要改变复制约束，最可能修改的符号是 `NoCopy` 的派生/trait 实现；必须同步更新 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 中的 `assert_not_impl_any!`，并把新增测试继续放在独立测试文件，不要内嵌到生产源码。
- 若要调整方法兼容性，应同时审查 `lock`、`unlock`、`Lock`、`Unlock` 四个入口。删除大写入口可能破坏机械移植或遗留调用代码；给空方法增加真实锁状态则会改变 ZST、可重复调用和资源生命周期等既有不变量。
- 若未来出现生产 Rust 使用者，应为实际外层类型增加回归测试，而不能只依赖本 crate 的零大小测试。尤其需要验证外层类型的复制 API、线程边界和性能布局是否符合预期。
- 不应为了获得 Go `copylocks` 效果而把本类型改造成真实互斥锁；若需求是并发同步，应在调用方显式选择标准或项目既有同步原语，并单独验证死锁与生命周期。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 `nocopy.rs` 被识别为含 6 个符号的 Rust 文件。
- RustCodeGraph `node --file pkg/util/nocopy/nocopy.rs`：核对 `NoCopy`、`impl` 和四个公开方法的完整实现；精确 `query NoCopy` 区分了 Go/Rust 同名符号。
- RustCodeGraph 对 `pkg/util/nocopy/nocopy.rs::NoCopy`、`lock`、`unlock`、`Lock`、`Unlock` 的 `callers`/`callees` 查询：未返回调用边；随后用限定 Rust 文件的 `rg` 复核，未发现独立测试之外的使用者。
- [`pkg/util/nocopy/Cargo.toml`](./Cargo.toml)、[`pkg/util/nocopy/lib.rs`](./lib.rs)、根 [`Cargo.toml`](../../../Cargo.toml) 与 [`pkg/lib.rs`](../../lib.rs)：核对 crate 名、workspace 成员、门面依赖、模块再导出及唯一测试依赖。
- [`pkg/util/nocopy/nocopy.go`](./nocopy.go) 与 [`pkg/sessionctx/stmtctx/stmtctx.go`](../../sessionctx/stmtctx/stmtctx.go)：核对 Go 的 Locker 方法形状及 `StatementContext` 中禁止复制的真实原因。
- [`pkg/util/nocopy/migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)：核对 ZST、非 `Clone`/`Copy` 和小写空操作可重复调用；未发现对大写兼容方法的现有测试。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付结构通过任务指定的 11 章节命令验证，并另行检查变更范围只包含本文档与任务文件删除。
