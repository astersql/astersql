# `br/pkg/utils/pointer.rs`

## 文件定位

`pointer.rs` 是 `astersql-br-pkg-utils` 库中的无状态指针/可选值辅助模块。crate 根文件 `br/pkg/utils/lib.rs` 通过 `#[path = "pointer.rs"] pub mod pointer` 挂载它，并以 `pub use pointer::GetOrZero` 将唯一 API 提升到 crate 根。该 crate 的边界和 Go 包映射由 `br/pkg/utils/Cargo.toml` 声明：库入口为 `lib.rs`，`package.metadata.porting.go-package` 为 `br/pkg/utils`。本文件不是业务流程入口，而是把 Go 的可空指针取值惯用法映射为 Rust 的 `Option<&T>`。

## 核心职责

唯一职责是实现 `GetOrZero<T>`：调用方提供一个可选借用；存在值时得到其所有权副本，不存在值时得到类型的默认值。这样，调用方不需要自行展开 `Option`，同时保留 Go `GetOrZero` 对 `nil` 返回零值的契约。文件没有其他常量、类型、trait、`impl`、条件编译项或私有辅助函数；其 API 是公开且同步的纯函数。

## 主要符号

- `pub fn GetOrZero<T>(p: Option<&T>) -> T where T: Default + Clone`（`br/pkg/utils/pointer.rs:21`）是唯一符号。`Default` 提供 `None` 分支的值，`Clone` 把 `Some(&T)` 指向的值复制成独立的 `T`；函数不取得原值所有权，也不返回借用。
- `pointer` 模块与 crate 根再导出（`br/pkg/utils/lib.rs:98-101,152`）共同构成公开路径：既可按模块访问，也可从 `astersql_br_pkg_utils::GetOrZero` 访问。crate 根的 `#![allow(non_snake_case)]` 允许保留 Go 风格的导出名。

## 执行流程

1. 调用方将已有值借用包装为 `Some(&value)`，或以 `None::<&T>` 表示缺失值；类型 `T` 由参数或上下文推导。
2. `GetOrZero` 对 `Option` 做一次穷尽匹配（`br/pkg/utils/pointer.rs:25-28`）。
3. `None` 分支调用 `T::default()`，构造并返回一个新的 `T`。
4. `Some(value)` 分支调用 `value.clone()`，返回被借用值的所有权副本。

流程不访问外部服务、不修改输入，也没有隐藏的重试或回退路径。当前 Rust 生产源码中未检索到除 `lib.rs` 再导出之外的实际调用；已接线的 Rust 使用证据是 crate 级契约测试。Go 侧则在 `EC2Session.WaitSnapshotsCreated` 的快照失败日志中调用同名函数，将可空的 AWS `StateMessage` 安全转换为字符串值（`br/pkg/aws/ebs.go:280-282`）。

## 数据与状态

输入只有栈上的 `Option<&T>` 和共享借用 `&T`，函数自身不保存任何状态。输出总是一个拥有所有权的 `T`：`None` 时其具体“零值”由 `T::default()` 定义，`Some` 时其复制语义由 `T::clone()` 定义。因此 Rust 契约是“默认值或克隆值”，并不保证任意用户类型的 `Default`/`Clone` 与 Go 的按值零值/复制在成本或深层语义上完全相同。

对 `Copy` 标量，当前行为等价于值复制；对堆对象，克隆可能分配内存或执行用户定义逻辑。函数没有全局变量、缓存、计数器或配置读取。

## 依赖与调用关系

下游仅依赖 Rust 标准库中的 `Option`、`Default` 和 `Clone`；`br/pkg/utils/Cargo.toml` 没有为本文件引入专属外部依赖或 feature。RustCodeGraph 将 `GetOrZero` 定位在 `br/pkg/utils/pointer.rs:21`，并显示 `br/pkg/utils/lib.rs` 的导入/再导出关系。

Rust 上游直接证据为 `br/pkg/utils/parity_test.rs:26-42`：测试从 crate 根导入 `GetOrZero` 并调用两个分支。全仓 Rust 文本检索未发现其他生产调用。RustCodeGraph 还把 Go 的 `WaitSnapshotsCreated`（`br/pkg/aws/ebs.go:246`）列为调用者；这是跨语言同名调用关系，代表 Go 对照实现的生产用法，不应解读为 Rust 函数已接入 EBS Rust 主链。

## 错误处理与边界

函数签名不返回 `Result` 或 `Option`，两个匹配分支也没有显式错误路径。边界行为如下：

- `None` 不报错，直接返回 `T::default()`；对 `i32`，契约测试确认结果为 `0`（`br/pkg/utils/parity_test.rs:42`）。
- `Some(&value)` 返回克隆值；契约测试确认 `Some(&7_i32)` 返回 `7`（`br/pkg/utils/parity_test.rs:41`）。
- 不接受 `T: !Default` 或 `T: !Clone`，这类用法会在编译期被 trait bound 拒绝；这是相对 Go `T any` 更窄的类型边界。
- 本函数不捕获 `Default::default` 或 `Clone::clone` 的 panic；若调用方类型的实现会 panic，panic 将正常向上传播。

Go 的 EBS 测试包含“snapshot failed”以及 `StateMessage: nil` 的失败样例（`br/pkg/aws/ebs_test.go:129-192`），说明同名 Go 辅助函数的关键现实边界是可空错误消息；该测试的专用模拟函数没有直接调用 `GetOrZero`，所以它只能作为 Go 场景证据，不能替代 Rust 函数测试。

## 并发与资源生命周期

`GetOrZero` 没有锁、原子变量、通道、任务、文件句柄、网络连接或事务，也不要求 `T: Send + Sync`。共享借用只在调用期间有效；返回值拥有自身生命周期，不依赖输入引用继续存活。并发安全性因此由调用点对输入的合法共享借用以及 `T` 的 `Clone`/`Default` 实现决定，而不是由本函数进行同步。

资源成本集中在 `T::default()` 或 `T::clone()`：简单标量通常成本固定，复杂容器可能分配或复制大量数据。扩展时不应在这个薄辅助函数内引入全局状态、阻塞 I/O 或隐式锁，否则会破坏当前可预测的纯函数性质。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/utils/pointer.go`。Go 签名 `GetOrZero[T any](p *T) T` 在 `p == nil` 时返回 `var zero T`，否则返回 `*p`；Rust 版本用 `Option<&T>` 表达 `*T` 的可空性，用模式匹配表达 nil 检查。两个版本对标量和常见值类型都实现“缺失返回零/默认值，存在返回值副本”。

差异必须保留在迁移判断中：Go 接受任意 `T`，语言自动提供零值且按值复制；Rust 要求 `T: Default + Clone`，而 `Default` 是类型作者定义的约定，未必等同于业务意义上的零值，`Clone` 也可能是深复制并带来额外成本。Go 的当前生产调用位于 `br/pkg/aws/ebs.go:281`；当前 Rust 版本只有公开导出和契约测试证据，尚不能声称已替换该 Go 调用。

## 扩展指南

- 若只需增加 `GetOrZero` 的类型覆盖，优先在独立测试文件 `br/pkg/utils/parity_test.rs` 增加案例，不要把测试内嵌到 `pointer.rs`；至少覆盖目标类型的 `None` 默认值、`Some` 克隆结果，以及必要时验证原值未被取得所有权。
- 若要放宽 `Clone` 成本，可新增语义明确的借用型 API，而不要悄悄改变现有返回所有权的契约；同时评估 crate 根 `pub use` 是否需要新增导出。
- 若要为特定类型定义“零值”，先确认其 `Default` 与 Go 零值一致。不能为了通过迁移测试而对所有类型假定二者相同。
- 修改函数名、可见性、trait bound 或返回类型时，需要同步 `br/pkg/utils/lib.rs:152` 和 `br/pkg/utils/parity_test.rs:26-42`，并检查所有下游 crate 对根级再导出的依赖。
- 对大对象或昂贵 `Clone` 的新调用点，应评估复制开销；对可能 panic 的自定义 `Default`/`Clone`，应由调用方决定是否隔离 panic，而不是在这里静默吞掉。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/utils` 确认 `pointer.rs`、`pointer.go`、`lib.rs` 和 `parity_test.rs` 均被索引。
- RustCodeGraph `node --file br/pkg/utils/pointer.rs` 与 `node GetOrZero`：确认文件共 29 行、唯一函数位于第 21 行，函数体只有两个匹配分支。
- RustCodeGraph `query GetOrZero`：确认函数定义、`lib.rs` 模块导入和 `parity_test.rs` 测试导入；调用轨迹还指出 Go 的 `WaitSnapshotsCreated`，已按跨语言证据解释。
- 已读源码/配置：`br/pkg/utils/pointer.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`、`br/pkg/utils/pointer.go`、`br/pkg/aws/ebs.go`。
- 已读测试：`br/pkg/utils/parity_test.rs` 与 `br/pkg/aws/ebs_test.go`。前者直接验证 Rust 的 `Some`/`None` 两分支；后者验证 Go 生产场景中的快照失败和缺失状态消息边界，但其测试辅助函数不直接调用 `GetOrZero`。
- 文档完成后以任务规定的 `rg -c` 命令校验固定的十一个二级章节；本任务是纯文档分析，按计划不运行 Cargo。
