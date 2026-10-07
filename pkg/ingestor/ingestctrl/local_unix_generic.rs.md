# `pkg/ingestor/ingestctrl/local_unix_generic.rs`

## 文件定位

本文件位于 `astersql-ingestor-ingestctrl` crate 内；crate 入口 `pkg/ingestor/ingestctrl/lib.rs` 以 `pub mod local_unix_generic` 公开该模块。它是从 Go 文件 `pkg/ingestor/ingestctrl/local_unix_generic.go` 移植而来的 rlimit 平台适配片段，为非 FreeBSD 的 Unix 语义提供资源限制数值类型，并保留结构化日志字段的兼容接口。

需要注意当前 Rust 接线与 Go build tag 并不等价：Go 文件由 `//go:build !freebsd && !windows` 限定平台，而 Rust 模块声明和本文件均没有 `#[cfg(...)]`。因此当前事实是该 Rust 模块会随 crate 一起声明；FreeBSD 和 Windows 的对应实现则分别存在于独立模块 `local_freebsd.rs`、`local_windows.rs`，不是编译期同名替换。

## 核心职责

- 用 `pub type RlimT = u64` 固定通用 Unix rlimit 值的 Rust 表示，使 `local_unix.rs` 的 `maxRLimit`、`GetSystemRLimit` 和 `VerifyRLimit` 共用同一数值类型。
- 用 `RLimitLogField` 表示一对结构化日志键值，并由 `zapRlimT` 将借用的键名复制成自有 `String`。
- 保留 Go 版本 `RlimT`/`zapRlimT` 的命名和大致 API 轮廓，方便逐步移植和对照。

本文件不读取或修改操作系统 rlimit，不发日志，也不执行校验；真正的 `getrlimit`/`setrlimit` 流程位于 `pkg/ingestor/ingestctrl/local_unix.rs`。

## 主要符号

- `pub type RlimT = u64`：公开类型别名。它没有新的运行时表示或转换成本；`local_unix.rs` 直接把它用于请求值、返回值及 `maxRLimit` 常量。
- `pub struct RLimitLogField { pub key: String, pub value: u64 }`：自有键名和 rlimit 数值的简单数据对象。派生的 `Clone`、`Debug`、`Eq`、`PartialEq` 便于复制、诊断和精确比较，但本仓库当前没有直接使用或测试这些能力。
- `pub fn zapRlimT(key: &str, value: RlimT) -> RLimitLogField`：公开构造函数。它通过 `key.to_owned()` 取得键名所有权，原样保存 `value`，没有校验、截断或格式化。

三个符号都公开；文件内没有常量、trait、impl、宏、异步函数、unsafe 代码或条件编译项。

## 执行流程

`zapRlimT` 的完整执行只有两步：先把调用方的 `&str` 复制成 `String`，再连同 `u64` 值构造并返回 `RLimitLogField`。该函数是纯构造逻辑，不会记录日志或触发 I/O。

应用侧真正的 rlimit 主链是：`local.rs::NewBackend` 调整 `BackendConfig` 后，调用 `local_unix.rs::VerifyRLimit(config.max_open_files as u64)`；后者把请求截断至 `maxRLimit`，读取当前限制，必要时调用 `setrlimit`，然后复读确认。此链只使用本文件的 `RlimT`。RustCodeGraph 与仓库文本搜索均未发现 Rust 代码调用 `zapRlimT`，所以不能把 Go 版本成功设置后的 old/new 日志行为视为已经接入 Rust 主链。

## 数据与状态

`RlimT` 对应通用 Unix Go 实现的 `uint64`，可表示 `0..=u64::MAX`；本文件本身不施加 `maxRLimit = 1_000_000` 的业务上限，该上限由 `local_unix.rs::VerifyRLimit` 执行。

`RLimitLogField` 完全拥有 `key`，因此返回值不借用输入字符串；`value` 是按值复制的 `u64`。文件没有全局可变状态、缓存、句柄或环境依赖。构造相同键值会得到可用 `Eq`/`PartialEq` 判等的相同数据。

## 依赖与调用关系

crate 边界由 `pkg/ingestor/ingestctrl/Cargo.toml` 确认：包名为 `astersql-ingestor-ingestctrl`，库入口是 `lib.rs`，根 workspace 在 `Cargo.toml` 中纳入 `pkg/ingestor/ingestctrl`。本文件只使用 Rust 标准库的 `String`、`str::to_owned` 和派生 trait，不直接依赖 Cargo 中列出的外部 crate，也没有 feature 声明控制它。

直接下游关系是 `local_unix.rs` 导入 `crate::local_unix_generic::RlimT`。再上游，`local.rs::NewBackend` 调用 `local_unix.rs::VerifyRLimit`，使该类型间接位于 Lightning local backend 创建前的系统要求检查路径。`RLimitLogField` 和 `zapRlimT` 当前没有 Rust 调用者；RustCodeGraph 对 `zapRlimT` 也未给出被调用函数，因为其函数体只是字段构造与标准字符串复制。

## 错误处理与边界

本文件没有 `Result`、显式失败分支或 panic 路径。对空键、重复键、`0` 和 `u64::MAX` 都会照原值构造字段；是否是有效日志键或有效 rlimit 请求不由这里判断。

实际系统调用错误、请求上限、硬限制提升和二次读取确认均由 `local_unix.rs` 处理。尤其不能因为本文件存在 `zapRlimT` 就推断 Rust 会在成功调整后写日志：当前 Rust `VerifyRLimit` 没有调用它。另一个平台边界是模块缺少与 Go `!freebsd && !windows` 相同的 cfg；若以后要恢复编译期平台替换，必须同时审查 `lib.rs` 的模块声明、`local_unix.rs` 的导入及各平台文件，而不能只改本文件。

## 并发与资源生命周期

所有数据都在调用栈或返回值中按值管理；没有锁、原子变量、channel、任务、线程、事务或文件描述符生命周期。`zapRlimT` 只分配键名 `String`，返回值离开作用域时由 Rust 自动释放。`RlimT` 是 `u64` 别名，可按值复制；`RLimitLogField` 的派生 trait 没有引入共享可变状态，因此构造函数可被并发调用而无需同步。

操作系统资源限制属于进程级状态，但它由 `local_unix.rs` 中的 FFI 逻辑管理，不是本文件拥有的资源。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/ingestor/ingestctrl/local_unix_generic.go` 在 `!freebsd && !windows` 下把 `RlimT` 定义为 `uint64`，Rust 的 `u64` 在位宽和无符号语义上与之对齐。FreeBSD 对照文件使用有符号 `int64`，Rust 的 `local_freebsd.rs` 也使用 `i64`。

`zapRlimT` 的语义尚未完全对齐：Go 版本调用 `zap.Uint64(key, val)`，返回日志库可直接消费的 `zap.Field`；Go `local_unix.go::VerifyRLimit` 在设置成功后用它记录 `old` 和 `new`。Rust 版本返回仓库自定义的 `RLimitLogField`，`local_unix.rs::VerifyRLimit` 未调用该函数，也没有对应日志发射，因此它目前只是兼容数据门面。Rust 的 rlimit 行为测试集中在 `local_unix_test.rs`，验证的是 `verify_rlimit_with` 的截断、读写次数、硬限制和错误传播，不直接覆盖本文件的字段构造。

## 扩展指南

- 若只需改变通用 Unix rlimit 的数值表示，先核对目标平台 C `rlim_t`、`local_unix.rs::RawRLimit` 的字段布局及 FreeBSD/Windows 分支，避免仅改别名造成 FFI 布局或转换不一致。
- 若要恢复 Go 的成功日志行为，应优先决定 Rust 日志 API 的真实字段类型和消费位置，再修改 `local_unix.rs::VerifyRLimit` 接入；不要把当前 `RLimitLogField` 当作已经兼容 `log`/`zap` 的字段。相应单元测试应放在独立的 `local_unix_test.rs` 或新增独立测试文件中，不要嵌入生产源文件。
- 若要恢复 Go build-tag 等价的平台选择，应在 `lib.rs` 上为 `local_unix_generic`、`local_freebsd`、`local_windows` 设计互斥 `#[cfg]`，并同步调整引用；当前三个模块是不同命名空间，不能假设会自动替换。
- 为 `zapRlimT` 增加行为时，至少补充空键、普通值和边界值测试，并确认是否允许额外分配。对日志接线的修改还要验证成功设置才记录、失败路径不误报。

兼容性风险主要是公共类型/返回类型变化影响未来调用方；性能风险仅在日志热路径频繁复制键名时出现；正确性风险集中在平台 cfg 与 FFI 类型不一致。任何扩展都应继续保持 Rust 逻辑和独立测试与 Go 版本的真实意图对齐。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；查询目标目录确认 `local_unix_generic.rs` 已索引。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/local_unix_generic.rs`：核对本文件 39 行源码及三个核心符号；`query RlimT`、`query RLimitLogField`、`query zapRlimT` 核对平台同名实现。
- RustCodeGraph `node`：读取 `local_unix.rs`、`lib.rs`、`local.rs`、`local_unix_test.rs`、`local_freebsd.rs`、`local_unix_generic.go` 和 `local_unix.go`，核对模块声明、主链、平台差异、Go 日志行为和测试覆盖。
- RustCodeGraph `callers zapRlimT` 没有返回 Rust 调用边；`callees zapRlimT` 对目标 Rust 定义报告无被调函数。仓库 `rg` 复核仅在本文件定义 Rust `zapRlimT`/`RLimitLogField`，仅 `local_unix.rs` 导入 Rust `RlimT`。
- `pkg/ingestor/ingestctrl/Cargo.toml` 与根 `Cargo.toml`：核对 crate 名称、库入口、workspace 成员和依赖边界；目标包没有 `doc.go`。
- 相关独立测试：`pkg/ingestor/ingestctrl/local_unix_test.rs`。它覆盖 rlimit 校验主流程，但没有直接覆盖本文件的日志字段构造；Go 同目录也不存在名为 `local_unix_test.go` 的文件，Go rlimit failpoint 测试需从其他测试文件按符号检索。

