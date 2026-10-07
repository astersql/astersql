# `pkg/ingestor/ingestctrl/local_freebsd.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；该 crate 的入口是 [`lib.rs`](lib.rs)，Cargo 边界由 [`Cargo.toml`](Cargo.toml) 定义。`lib.rs` 通过 `pub mod local_freebsd;` 无条件公开本模块，因此其三个公开符号可以由 crate 使用者按 `local_freebsd` 路径访问。

它是 FreeBSD 的 rlimit 数值与结构化日志字段适配层，对照 Go 的 [`local_freebsd.go`](local_freebsd.go)。不过当前 Rust 模块没有 `#[cfg(target_os = "freebsd")]`，也没有被 [`local_unix.rs`](local_unix.rs) 选作平台实现；后者明确导入 `crate::local_unix_generic::RlimT`。因此，本文件目前是可编译的公开兼容辅助，不在实际 rlimit 查询或提升流程中。

## 核心职责

- 用 `RlimT = i64` 表达 FreeBSD/Go 对照中的 rlimit 值类型。
- 用 `RLimitLogField` 保存结构化日志所需的字段名和值。
- 用 `zapRlimT` 将借用的键名复制为自有 `String`，并连同数值构造成日志字段。

职责仅限类型和数据转换。本文件不读取或修改操作系统资源限制，不执行日志输出，也不参与错误处理。打开文件数限制的查询和提升逻辑位于 `local_unix.rs`。

## 主要符号

- `pub type RlimT = i64`：公开类型别名，对应 Go `local_freebsd.go` 的 `type RlimT = int64`。别名不会创建新的运行时类型，调用者仍按 `i64` 的值语义传递数据。
- `pub struct RLimitLogField { pub key: String, pub value: i64 }`：公开、拥有数据的日志字段。派生的 `Clone`、`Debug`、`Eq` 和 `PartialEq` 支持复制、诊断输出和精确比较；两个字段均公开。
- `pub fn zapRlimT(key: &str, value: RlimT) -> RLimitLogField`：本文件唯一函数。它用 `key.to_owned()` 获取键名所有权，并原样保存 `value`。命名沿用 Go 的 `zapRlimT`；crate 根允许 `non_snake_case`，所以该名称不会触发本 crate 的命名警告。

文件没有常量、trait、`impl`、条件编译项或私有辅助函数。

## 执行流程

若调用 `zapRlimT(key, value)`，执行过程只有两步：

1. 将输入 `&str` 复制成新的 `String`，使返回值不再借用调用者的键名。
2. 构造并返回 `RLimitLogField { key, value }`；数值不做截断、符号转换、范围校验或格式化。

当前应用主链不会自然进入此流程：`local_unix.rs` 的 `GetSystemRLimit`、`verify_rlimit_with` 和 `VerifyRLimit` 使用的是 `local_unix_generic::RlimT`，且没有调用任何平台日志字段构造器。全仓 Rust/Go 引用搜索只在本文件自身发现 Rust `zapRlimT`/`RLimitLogField` 的定义。

## 数据与状态

本文件没有全局或线程局部状态。`RlimT` 是有符号 64 位整数别名；`RLimitLogField` 完全拥有键字符串，并内联保存一个 `i64` 值。

`zapRlimT` 每次调用至多为键名分配一段字符串存储，返回值的释放遵循普通 Rust 所有权规则。函数不会缓存字段、修改输入或保留引用。由于没有对值做约束，负数、零和 `i64` 全范围都会被原样保存；是否为有效 rlimit 必须由上层决定。

## 依赖与调用关系

- 上游装配：`lib.rs` 的 `pub mod local_freebsd;` 使模块成为 crate 公共模块。
- 已验证调用者：RustCodeGraph 能索引本文件及三个符号，但精确 callers/callees 查询没有给出有效调用边；`rg` 也没有找到文件外的 Rust 调用。因此当前没有已验证的生产调用者或测试调用者。
- 下游依赖：仅使用标准库的 `String`、`&str::to_owned` 和派生 trait；`Cargo.toml` 没有为本文件引入专属外部依赖。
- 相邻实现：`local_unix_generic.rs` 提供形状相同但使用 `u64` 的 `RlimT`/`RLimitLogField`/`zapRlimT`；`local_unix.rs` 当前导入该通用版本的 `RlimT`。`local_windows.rs` 则提供 Windows 的 rlimit 占位行为。

这意味着文件注释中“供 local backend 校验时输出诊断信息”描述的是移植意图和 Go 对照用途，而非当前 Rust 调用链已经实现的事实。

## 错误处理与边界

所有公开操作都是不可失败的普通构造：没有 `Result`、错误枚举、系统调用或 panic 分支。唯一隐含失败来源是 `String` 分配遭遇进程级内存耗尽，代码未作专门恢复。

重要边界是平台类型差异：FreeBSD 版本使用 `i64`，通用 Unix 版本使用 `u64`。若未来把本模块接入 `local_unix.rs`，不能在不审查 FFI 布局和数值转换的情况下互换两个别名。还应避免把此结构误认为 `log` 或 Go `zap` 可直接消费的字段；当前 `RLimitLogField` 只是数据容器，尚无日志适配实现。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件描述符或其他外部资源。函数只读取不可变参数并返回新拥有的数据，因此没有共享可变状态，天然可重入。

返回值生命周期与调用者所有权一致：键名在构造时完成复制，输入 `&str` 可在调用后立即失效；克隆字段会再次克隆 `String`。本文件没有显式 `Send`/`Sync` 实现，结构体由 `String` 和 `i64` 组成，可继承标准库类型的自动 trait。

## 与 Go 版本的对应关系

Go 对照文件 `local_freebsd.go` 受 `//go:build freebsd` 限制，定义 `RlimT = int64`，并让 `zapRlimT` 返回 `zap.Int64(key, val)`。Rust 在数值宽度和有符号性上保持一致，也保留了函数名和“键 + 值”的结构化语义。

当前仍有三项显著差异：

1. Rust 模块没有 FreeBSD 条件编译，任何能编译该 crate 的目标都会声明它；Go 文件只在 FreeBSD 构建。
2. Go 返回可直接交给 zap logger 的 `zap.Field`；Rust 返回自定义 `RLimitLogField`，且仓库中没有把它转换或提交给 logger 的已验证调用。
3. Go `local_unix.go::VerifyRLimit` 在成功提升限制后调用 `zapRlimT("old", ...)` 和 `zapRlimT("new", ...)`；Rust `local_unix.rs::VerifyRLimit` 当前不记录这条成功日志，并使用通用 Unix `u64` 类型。

因此本文件只完成了 FreeBSD 类型与字段构造器的局部移植，尚不能视为 Go 平台选择和日志行为的完整对齐。

## 扩展指南

若要让该实现真正参与 FreeBSD 流程，最可能需要：

- 在 `lib.rs` 或专门的平台选择模块中增加互斥、可审查的 `cfg(target_os = "freebsd")` 接线，同时确保非 FreeBSD Unix 继续选择 `local_unix_generic`；不要让两个同名实现被含混导入。
- 审核 `local_unix.rs` 的 `RawRLimit` 字段类型、FFI 布局与 `RlimT` 的有符号性，再决定平台类型如何进入 `GetSystemRLimit`、`verify_rlimit_with` 和 `VerifyRLimit`。直接把 `u64` 改为 `i64` 可能引入转换或 ABI 风险。
- 若恢复 Go 的成功日志，先为 Rust 日志框架设计明确的字段转换接口，再接入 `VerifyRLimit`；不要把 `RLimitLogField` 当成已经可输出的日志字段。
- 测试必须放在独立文件中。可新增或扩展 `local_unix_test.rs` 以验证平台选择和 rlimit 流程，并为纯字段构造另建相邻 `*_test.rs`；应覆盖键所有权、负值/边界值、FreeBSD 与通用 Unix 类型差异以及条件编译。当前没有 `local_freebsd_test.rs`。

兼容风险主要是条件编译导致的公共路径变化和整数符号转换；正确性风险集中在 rlimit FFI 布局；性能影响通常仅为每个字段一次键名分配。任何行为修改都应遵循仓库规则先补失败回归测试，本说明任务本身不修改运行时代码。

## 验证依据

- RustCodeGraph `status`：索引包含本仓库，目标目录中的 `local_freebsd.rs` 已被索引。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/local_freebsd.rs`：核对完整 39 行源码、三个公开符号及其签名。
- RustCodeGraph `query zapRlimT --kind function --json`：确认 Rust FreeBSD、Rust 通用 Unix及对应 Go 文件中的同名实现。
- RustCodeGraph 精确 callers/callees 查询未返回可用调用边；随后用 `rg` 核对全仓直接引用，未发现目标 Rust 符号的文件外调用。此处不将缺失的图边推断为已接线。
- [`local_freebsd.rs`](local_freebsd.rs)：核对 `i64` 别名、字段派生、键复制和无错误分支。
- [`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)：核对 crate 名称、入口、依赖边界和无条件模块声明。
- [`local_freebsd.go`](local_freebsd.go)、[`local_unix.go`](local_unix.go)：核对 FreeBSD build tag、`int64`/`zap.Int64` 语义以及 Go rlimit 成功日志的真实调用点。
- [`local_unix_generic.rs`](local_unix_generic.rs)、[`local_unix.rs`](local_unix.rs)：核对通用 Unix `u64` 版本和当前 Rust rlimit 流程实际选择的类型。
- [`local_unix_test.rs`](local_unix_test.rs)：相关独立 Rust 测试覆盖 rlimit 上限、系统调用失败和复读校验，但不直接覆盖本文件；全仓未发现 `local_freebsd_test.rs` 或其他目标符号测试。
- 本包不存在 `doc.go`，因此没有更近的包级 Go 契约可读取。任务为纯文档分析，按计划未运行 Cargo。
