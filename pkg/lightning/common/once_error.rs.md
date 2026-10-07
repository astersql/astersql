# `pkg/lightning/common/once_error.rs`

## 文件定位

该文件属于 `astersql-lightning-common` crate。crate 入口 `pkg/lightning/common/lib.rs` 以 `mod once_error;` 装入模块，再通过 `pub use once_error::*;` 将 `OnceError` 暴露给下游。`pkg/lightning/common/Cargo.toml` 指定库入口为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/lightning/common"` 标明它对齐 Go 包 `pkg/lightning/common`。

当前 Rust 仓库中，`OnceError` 的直接使用点只有独立测试 `pkg/lightning/common/once_error_test.rs`；未检索到生产 Rust 调用者。它是已经公开的 Lightning 公共能力，但 Go 导入主链中已有的生产用途尚未在 Rust 主链中出现。Go 侧的直接入口位于 `lightning/pkg/importer/table_import.go` 和 `lightning/pkg/importer/import.go`，用于并发 engine、chunk 和 table 任务的首错汇总。

## 核心职责

`OnceError` 是一个可克隆、可在线程间共享的一次性错误槽：初始为空，忽略 `None`，并在多个非空写入中仅保留第一个取得锁且观察到空槽的 `CommonError`。读取者通过 `Get` 获得已保存值的副本，而不是内部引用。

这个抽象把“首错胜出”与互斥同步集中在一个类型中。Go 调用现场会在 worker 报错时调用 `Set` 并取消上下文，等待所有 worker 退出后再调用 `Get` 返回首错；Rust 文件本身只负责保存与读取，不负责取消任务、等待线程、记录日志或归一化错误。

## 主要符号

- `pub struct OnceError(Arc<Mutex<Option<CommonError>>>)`：唯一公开类型。元组字段私有，调用者不能绕过 API 替换内部状态。`Clone` 克隆 `Arc`，因此克隆实例共享同一个槽；`Default` 构造 `None` 状态。
- `pub fn Set(&self, error: Option<CommonError>)`：公开写入口。`None` 在加锁前直接返回；`Some` 获取互斥锁，仅当槽仍为 `None` 时移动写入。
- `pub fn Get(&self) -> Option<CommonError>`：公开读入口。持锁克隆整个 `Option<CommonError>` 后返回；因此要求 `CommonError: Clone`，也避免锁保护数据的引用逸出。

文件没有模块级常量、trait、自由函数、条件编译项或自定义 `Drop` 实现。方法名保留 Go 风格的 `Set`/`Get`；crate 根的 `#![allow(non_snake_case)]` 允许这种命名。

## 执行流程

写入路径如下：

1. `Set` 对参数做模式匹配；参数为 `None` 时立即结束，既不加锁也不清空既有错误。
2. 参数为 `Some(error)` 时，对内部 `Mutex` 加锁。锁若已中毒，`expect("once error lock poisoned")` 触发 panic。
3. 锁内检查 `stored.is_none()`；为空则写入该错误，非空则丢弃本次参数。
4. 方法结束时互斥锁守卫离开作用域并自动解锁。

读取路径更短：`Get` 加锁、克隆当前 `Option<CommonError>`、随后释放锁并返回。`pkg/lightning/common/once_error_test.rs::test_once_error` 覆盖初始空值、`Set(None)`、首次 `Some` 生效、后续 `Some` 被忽略、已有值不被 `None` 清除，以及克隆实例在线程中访问相同状态。

## 数据与状态

内部状态只有两态：`None` 表示尚未记录错误，`Some(CommonError)` 表示槽已永久占用。公开 API 不提供清空、替换或取出操作，因此正常执行中只允许 `None -> Some` 一次单向转换。

`CommonError` 定义在 `pkg/lightning/common/errors.rs`，包含错误 ID、消息、种类、状态码、原因链和栈信息，并实现 `Clone`。`Get` 会克隆这些字段及其向量，而不是返回共享对象身份；读取成本随已保存错误内容大小增长。`OnceError::clone` 则只增加 `Arc` 引用计数，不复制槽内错误。

## 依赖与调用关系

文件的直接下游依赖只有 `crate::CommonError` 与标准库 `std::sync::{Arc, Mutex}`。`CommonError` 经 `pkg/lightning/common/lib.rs` 从 `errors` 模块再导出；本文件不直接使用 `pkg/lightning/common/Cargo.toml` 中的 `astersql-lightning-log` 或 `libc` 依赖，也没有 feature 分支。

模块装配边为 `lib.rs -> once_error.rs`，公开边为 `lib.rs -> pub use once_error::*`，测试边为 `lib.rs` 在 `cfg(test)` 下装入 `once_error_test.rs -> OnceError::{Set, Get}`。RustCodeGraph 能定位 `pkg/lightning/common/once_error.rs::OnceError` 及其实现源码，但 `callers`/`callees` 未返回调用边；仓库级 Rust 搜索同样未发现测试之外的 `OnceError` 使用点。因此不能把 Go 侧 importer 调用误写成已经接通的 Rust 调用链。

作为设计对照，Go 生产链在 `lightning/pkg/importer/table_import.go` 用 `engineErr`/`chunkErr` 收集并发任务首错并配合 context cancel，在等待 goroutine 完成后读取；`lightning/pkg/importer/import.go` 用 `restoreErr` 汇总 table 恢复与 post-process 错误。

## 错误处理与边界

业务错误作为数据保存在槽中，不由 `Set` 或 `Get` 传播为 Rust `Result`。`None` 明确表示“无错误”，不会占用槽；后续 `Some` 仍可成为首错。槽一旦为 `Some`，所有后续值（包括不同错误）都不会替换它。

同步层面的失败采用 panic 策略：若持锁线程在临界区 panic 导致 mutex poisoned，之后的 `Set` 与 `Get` 都会在 `expect` 处 panic，模块没有恢复中毒锁的分支。该文件也不对 `CommonError` 做日志脱敏、包装、归一化或重试分类，这些职责位于 `errors.rs` 等其他模块。

“第一个”由竞争线程成功取得互斥锁并看到空槽的顺序决定，不保证对应外部事件的时间戳顺序。API 也没有返回布尔值来告诉调用者本次写入是否胜出；需要这一信息的扩展必须显式设计兼容接口。

## 并发与资源生命周期

`Arc` 管理共享槽生命周期：每个 `OnceError` 克隆持有一个强引用，最后一个克隆销毁时才释放 `Mutex` 及其内部 `CommonError`。`Mutex` 将检查与写入包在同一临界区内，保证两个并发 `Some` 不会同时覆盖状态；`Get` 使用同一把锁，因此不会观察到部分写入。

方法不持有跨调用的锁守卫，也不创建线程、任务或通道。独立测试通过 `std::thread::spawn` 和同步通道确认克隆值可被移动到另一线程且共享状态，但该测试中的并发操作是 `Set(None)`；它没有构造多个并发 `Some` 来断言具体胜者。具体胜者本来也不应被依赖，只应依赖“恰有一个值被保留”的不变量。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/common/once_error.go`，测试对照是 `pkg/lightning/common/once_error_test.go`。两版都具备默认空状态、忽略空错误、互斥保护、首次非空写入生效和读取首错的核心语义；Rust 测试逐项复现了 Go 测试的断言与跨执行单元访问场景。

表示方式存在明确差异：Go 用结构体内嵌 `sync.Mutex` 与 `error` 接口，零值可直接使用，通常以指针接收者共享；Rust 用 `Arc<Mutex<Option<CommonError>>>`，通过 `Default` 获得空值，并通过 `Clone` 显式共享。Go `Get` 返回原错误接口值，Rust `Get` 返回 `CommonError` 的深层字段副本。Rust 版本因此只接受该 crate 的具体 `CommonError`，不像 Go 的 `error` 接口可容纳任意错误实现。

Go 生产调用点证明该抽象在 Lightning importer 中的目标角色，但当前 Rust 搜索没有对应生产调用，故迁移状态应描述为“公共容器和独立测试已移植，生产接线未在 Rust 侧找到”，而非宣称 Rust 导入流程已经使用它。

## 扩展指南

若要改变一次性写入语义，应优先修改 `OnceError::Set`，同时在独立文件 `pkg/lightning/common/once_error_test.rs` 增加确定性测试；不要把测试嵌入生产源文件。若增加查看、尝试写入或消费错误的 API，应维持检查与状态转换在同一次加锁内完成，并明确返回值是否暴露“本次写入胜出”。

若要把该类型接入 Rust Lightning 并发导入流程，应从真实 worker 汇合点调用 `Set(Some(error))`，在发起取消后仍等待共享资源使用者退出，再调用 `Get` 返回首错；Go 的 `table_import.go` 中 engine/chunk 流程可作为语义参考，但 Rust 入口必须通过当时的实际调用图确认。

兼容风险包括把 `Option<CommonError>` 泛化后改变公开签名、改变 Go 风格方法名、或把 `Get` 改为消费值而破坏重复读取。正确性风险集中在锁中毒策略和竞争胜者不可预测；性能风险集中在每次读取都加互斥锁并克隆完整错误。新增并发测试应只断言单次占用和所有克隆观察一致，不应断言某个线程固定获胜。

## 验证依据

- 源码与装配：[once_error.rs](once_error.rs)、[lib.rs](lib.rs)、[errors.rs](errors.rs)。
- crate 边界：[Cargo.toml](Cargo.toml)；工作区继承的版本、edition 和 publish 设置来自[根 Cargo.toml](../../../Cargo.toml)。
- Rust 独立测试：[once_error_test.rs](once_error_test.rs) 中的 `test_once_error`。
- Go 对照与测试：[once_error.go](once_error.go)、[once_error_test.go](once_error_test.go) 中的 `TestOnceError`。
- Go 生产调用证据：[table_import.go](../../../lightning/pkg/importer/table_import.go) 中 `engineErr`、`chunkErr`，以及 [import.go](../../../lightning/pkg/importer/import.go) 中 `restoreErr`。
- RustCodeGraph：`status` 显示本仓库索引可用；`query OnceError --kind struct` 和 `node pkg/lightning/common/once_error.rs::OnceError` 定位类型及实现；对精确目标执行 `callers`/`callees` 未得到调用边。随后用 `rg` 核验 Rust 使用点，仅发现 crate 装配与独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用任务文件给定的十一章节结构命令，并人工复核上述事实、限制和扩展入口。
