# `build/linter/linter.rs`

## 文件定位

`build/linter/linter.rs` 是 Go 文件 [`build/linter/linter.go`](./linter.go) 的 Rust 侧审计记录，位于开发期静态检查工具目录 `build/linter`，但它本身不是某个 linter 的实现或注册入口。目标文件第 17—24 行只有模块说明和 Go 空白导入的对应结论，没有 `mod`、`use`、常量、类型、trait、函数、`impl` 或条件编译项。因此它当前既不向根 crate 暴露公开 API，也不进入应用运行主链。

根 [`Cargo.toml`](../../Cargo.toml) 定义 `astersql` 根 package，并以 `pkg/lib.rs` 为 `[lib]` 入口；该清单没有 `skywalking-eyes` Rust 依赖。`pkg/lib.rs` 也没有把本文件声明为生产模块。唯一接线位于 `pkg/lib.rs:83-85`：在 `#[cfg(test)]` 下将独立的 `build/linter/linter_test.rs` 纳入根 crate 测试，而测试再用 `include_str!("linter.rs")` 读取本文件文本。

## 核心职责

本文件只有一项职责：保存已经核对过的跨语言迁移结论——Go 侧 `_ "github.com/apache/skywalking-eyes/pkg/config"` 是依赖保留用空白导入，而 Rust 侧不应为此虚构可调用函数、初始化副作用或同名依赖。这个职责由 `build/linter/linter.rs:17-24` 的注释表达，并由 `build/linter/linter_test.rs` 的三个文本契约测试锁定。

它不负责执行 skywalking-eyes、不负责聚合 `build/linter/*` 下的 analyzer，也不参与 TiDB/AsterSQL 的 SQL、存储或服务进程路径。目录归属说明其对应的是构建/代码质量工具面；文件内容则进一步限定它只是 Go 专属依赖语义的记录。

## 主要符号

目标文件没有 Rust 符号。RustCodeGraph 对 `build/linter/linter.rs` 显示 24 行、1 个文件节点、`used by 0 files`，且按文件执行 callers/callees 均返回 `No definition found`；这是没有函数级调用图节点的预期结果，不是遗漏实现。

与本文件直接相关的符号都在独立测试 `build/linter/linter_test.rs` 中：

- `RUST_SOURCE: &str`：通过 `include_str!("linter.rs")` 在编译测试时嵌入目标文件文本。
- `GO_MOD: &str`：通过 `include_str!("../../go.mod")` 嵌入 Go 模块清单文本。
- `go_only_blank_import_has_no_invented_rust_runtime_api()`：确认没有人为增加 `ensure_skywalking_eye_config_dependency` 或任意 `pub fn`，同时确认迁移结论仍在。
- `dependency_retained_by_the_go_blank_import_remains_pinned()`：同时检查 `go.mod` 中的 `v0.4.0` 固定项和目标文件中的原 Go 导入路径。
- `audited_marker_and_original_license_are_preserved()`：检查 AsterSQL 审计标记、PingCAP Apache License 版权声明，并排除过时的“不可编译/占位”措辞。

## 执行流程

生产运行时没有执行流程：编译根库或启动应用时，`build/linter/linter.rs` 不被 `mod` 声明包含，也没有可执行代码。

测试期的实际链路如下：

1. 根 crate 使用 `Cargo.toml` 的 `[lib] path = "pkg/lib.rs"`。
2. 测试配置启用时，`pkg/lib.rs:83-85` 通过 `#[path = "../build/linter/linter_test.rs"]` 声明 `build_linter_linter_test`。
3. `linter_test.rs:3-4` 在编译期分别把目标文件和 `go.mod` 读成静态字符串。
4. 三个 `#[test]` 函数对这些字符串做正向与反向断言，证明 Rust 没有虚构运行时 API、Go 依赖仍固定为 `v0.4.0`、版权与审计状态仍被保留。

这个流程只验证源码文本契约，不加载 Go 包，也不触发 skywalking-eyes 的任何初始化。

## 数据与状态

目标文件不定义运行时数据、全局变量或可变状态。它保存的是静态说明文本，其中关键事实是 Go 导入路径和版本保留语义。

测试中的 `RUST_SOURCE` 与 `GO_MOD` 是编译期生成的 `&'static str`，只读且随测试二进制存在；测试不会写回源文件或模块清单。需要注意，版本真值位于 `go.mod:19`，校验和位于 `go.sum:1437-1438`，本文件只记录原因，不是依赖版本的权威配置源。

## 依赖与调用关系

上游关系只有测试文本引用：`pkg/lib.rs`（测试模块接线）→ `build/linter/linter_test.rs`（`include_str!`）→ `build/linter/linter.rs`。由于 `include_str!` 是编译期文件包含而非函数调用，RustCodeGraph 没有为目标文件产生调用边；其 `used by 0 files` 也说明索引没有把文本包含建模成生产依赖。

目标文件没有下游 Rust 调用。Go 对照文件的下游依赖是 `github.com/apache/skywalking-eyes/pkg/config`，但使用 `_` 空白导入，Go 源码不能引用该包导出的名称。`go.mod:19` 与 `go.sum:1437-1438` 证明依赖由 Go 模块系统管理。根 `Cargo.toml` 不含 skywalking-eyes，也没有为 `build/linter/linter.rs` 设置独立 crate、feature 或 target。

不要把 `pkg/lib.rs:1937` 的 `util::linter::constructor` 门面与本文件混为一谈：该门面再导出的是 `pkg/util/linter/constructor` crate，与 `build/linter/linter.go` 的包级空白导入不是同一个实现位置。

## 错误处理与边界

本文件没有 `Result`、错误类型、panic 或诊断分支，因而不存在运行时错误传播。其边界由测试的反向断言界定：不得添加用于模拟 Go 依赖保留的公开 Rust 函数，不得把已审计文件重新描述为“不可编译”或“文档化占位点”。

现有测试是精确字符串检查，能捕获导入路径、版本、特定公开函数和说明文字的漂移，但不会解析 Rust AST，也不会证明 Go 依赖为何在所有构建场景中必需。若 Go 依赖版本或保留策略改变，需要同步更新 `go.mod`、必要时 `go.sum`、本文件说明以及独立测试断言，不能只改其中一处。

## 并发与资源生命周期

没有线程、异步任务、锁、通道、文件句柄、网络连接或事务。生产阶段没有资源创建与释放。

测试阶段的文件读取发生在 Rust 编译期，由 `include_str!` 将内容嵌入测试二进制；测试执行时仅对不可变字符串做包含关系判断。三个测试之间没有共享可变状态，因此不存在顺序依赖、锁竞争或清理要求。

## 与 Go 版本的对应关系

Go 原文件 `build/linter/linter.go` 定义 `package linter`，唯一代码行为是空白导入 `_ "github.com/apache/skywalking-eyes/pkg/config"`，旁注说明其目的是让 skywalking-eyes 进入 `go.mod`。对应版本确实固定在 `go.mod:19` 的 `github.com/apache/skywalking-eyes v0.4.0`，校验和记录在 `go.sum:1437-1438`。

Rust 文件没有逐句模拟 Go 的空白导入。差异是有意的：Cargo 依赖由 manifest 声明，Rust 源码不存在等价且必要的空白导入机制；仓库的根 `Cargo.toml` 也没有对应依赖。当前迁移策略因此是保留审计说明并禁止虚构 Rust 运行时 API。此结论同时受到 `build/linter/linter_test.rs` 的版本、路径与无 API 断言保护。

## 扩展指南

- 若只是升级 Go 的 skywalking-eyes 版本，应修改 Go 模块配置并同步 `dependency_retained_by_the_go_blank_import_remains_pinned()` 的期望值及本文件文字；确认新版本仍由 `linter.go` 的空白导入承担保留职责。
- 若 Rust 将来确实需要同类能力，应先在适当的独立 crate/Cargo target 中引入真实依赖和行为，再将实现接入明确入口；不要在此文件添加无调用者的占位函数来伪造对等。届时应新增或扩展同目录独立测试文件，验证真实行为，而不是把测试逻辑内嵌进生产源文件。
- 若本文件开始成为生产模块，需要同步增加 `mod`/Cargo target 接线，并重新核对公开 API、错误处理、资源生命周期与调用图。当前根 crate 只有测试文件的 `#[path]` 接线，不能据此声称生产可达。
- 兼容性风险主要是 Go 依赖保留语义漂移和测试文本断言失配；当前文件没有运行时路径，因此没有直接性能风险。新增实际初始化行为则可能改变构建依赖、启动副作用和供应链边界，必须单独评审。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter build/linter` 找到 `linter.rs` 与 `linter_test.rs`。
- RustCodeGraph `node --file build/linter/linter.rs --offset 1 --limit 240`：目标文件共 24 行、仅注释、`used by 0 files`；对该文件执行 callers/callees 无可用定义。
- RustCodeGraph 对三个测试函数的 `query --kind function`：分别定位到 `build/linter/linter_test.rs:7`、`:15`、`:21`；`node --file` 核对了完整断言。
- 源码与配置：`build/linter/linter.rs`、`build/linter/linter.go`、`build/linter/linter_test.rs`、`pkg/lib.rs:83-85`、根 `Cargo.toml` 的 `[package]`/`[lib]`、`go.mod:19`、`go.sum:1437-1438`。
- 人工复核结论：文件存在是为了记录 Go 专属依赖保留约束；生产运行时不执行；安全扩展必须先建立真实 Cargo/模块接线和独立行为测试，不能从注释文件推导已支持的 Rust 功能。
- 本任务是纯文档分析，按计划不运行 Cargo；验收采用任务指定的 11 章节结构命令。
