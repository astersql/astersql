# `build.rs`

## 文件定位

`build.rs` 是根包 `astersql` 的 Cargo 构建脚本；`Cargo.toml:805-812` 通过 `build = "build.rs"` 把它挂到根包，根库入口则是 `pkg/lib.rs`。它在编译根包之前运行，所有派生产物写入 Cargo 提供的 `OUT_DIR`，不参与数据库进程的运行时请求链。

该文件目前兼有三类构建期职责：编译仓库内的 `pkg/extworkload/proto/externalworkload.proto`，把既有独立 Rust 测试/实现改写为根级 harness 可用的临时源码，以及从 Go module 中取得 `kvproto`、`tipb` schema 后生成 protobuf 绑定（`build.rs:137-629`）。这些职责属于根包迁移和代码生成设施，而不是 Go 服务端的对应业务模块。

当前仓库没有根目录 `build.go`。此外，检索 `pkg/lib.rs` 和其他 Rust 源文件，没有发现根脚本生成的测试适配文件、`selector.rs` 或 `kvproto_bindings.rs` 的 `include!` 消费点；因此只能确认这些文件会被生成，不能据此宣称它们当前已经接入根 crate。`pkg/extworkload/client/build.rs` 还会为 canonical `astersql-extworkload-client` crate 独立编译同一个 proto，其 `client.rs:33` 使用 `tonic::include_proto!("externalworkload")`；这条子 crate 链路不依赖根脚本的 `OUT_DIR`。

## 核心职责

- `main`（`build.rs:144`）取得工具链和输出路径，按固定顺序执行本地 gRPC proto 编译、30 份测试/实现源码适配、共享 selector 复制、9 份 kvproto schema 与 3 份 tipb schema 的绑定生成和结果归一化。
- `go_module_dir`（`build.rs:21`）定位 Go module 根目录：优先采用 `go list -m -f {{.Dir}}` 的结果；只有启动 `go` 命令失败时才解析 `go.mod` 并退回 `$HOME/go/pkg/mod/<module>@<version>`。
- `normalized_proto`（`build.rs:64`）复制 schema 到 `OUT_DIR` 下的临时目录，并把三个 `rustproto` 生成选项从 `true` 文本替换为 `false`，不改变 protobuf 字段定义。
- `root_test`（`build.rs:92`）处理只需移除 crate 级属性和改写 crate 路径的 13 份源文件；`adapted_test`（`build.rs:113`）处理还需前导导入或定制替换的 17 份源文件。
- `main` 对 protobuf-codegen 结果做兼容修正：规范 `brpb` oneof 变体名、追加 `Gcs` 类型别名、剥离生成文件的 crate 属性/模块文档，并写出模块清单（`build.rs:505-625`）。

## 主要符号

文件没有自定义常量、类型、trait、`impl` 或条件编译项，五个函数均为私有构建脚本实现细节：

- `fn go_module_dir(module: &str) -> PathBuf`：返回 Go module 的实际目录；命令成功但退出状态失败时立即断言失败，不走缓存回退（`build.rs:24-30`）。
- `fn normalized_proto(source: &Path, destination: &Path)`：读入 UTF-8 schema，执行三项固定字符串替换后写入目标路径（`build.rs:64-85`）。
- `fn root_test(out_dir: &Path, output: &str, source: &str, crate_name: &str)`：过滤所有去除前导空白后以 `#![` 开头的行，再把所有 `crate_name` 文本替换成 `crate::<crate_name>`，最后发出 `cargo:rerun-if-changed`（`build.rs:92-106`）。
- `fn adapted_test(out_dir: &Path, output: &str, source: &str, prelude: &str, replacements: &[(&str, &str)])`：同样过滤 crate 级属性，按给定次序执行全局文本替换，在文件前插入 prelude，并登记源文件变更追踪（`build.rs:113-135`）。
- `fn main()`：Cargo 的唯一入口。RustCodeGraph 的可靠文件内调用边是 `main -> {go_module_dir, normalized_proto, root_test, adapted_test}`；标准库函数在图中出现了跨语言同名误配，不能作为语义证据。

## 执行流程

1. `main` 通过 `protoc_bin_vendored::protoc_bin_path` 选择内置 `protoc`，把路径放进本构建脚本进程的 `PROTOC` 环境变量，然后用 `tonic_build` 为 `externalworkload.proto` 生成客户端/服务端代码（`build.rs:147-153`）。
2. 它用 `go env GOMODCACHE` 获取模块缓存根目录，并分别调用 `go_module_dir` 定位 `github.com/pingcap/kvproto` 和 `github.com/pingcap/tipb`；gogo/protobuf 固定取缓存中的 `v1.3.2`（`build.rs:155-169`）。对应版本在 `go.mod:60,101,106` 中锁定。
3. 13 个简单适配项进入 `root_test` 循环（`build.rs:198-266`）；另外 17 个特殊项逐个进入 `adapted_test`（`build.rs:307-441`）。特殊规则包括补导入、删除重复 `include!`/`mod`、调整模块层级路径以及提升 JSON 辅助函数可见性。所有 30 个输入路径均已在仓库中核实存在。
4. `pkg/util/table-rule-selector/trie_selector.rs` 被去掉 crate 级属性后复制为 `OUT_DIR/selector.rs`，供计划中的根级适配模块共享（`build.rs:443-453`）。
5. 对 9 个 kvproto schema，先在 `OUT_DIR/kvproto` 写规范化副本，再以临时目录、原始 proto 目录、kvproto include 和 gogo module 为 include 根调用 `protobuf_codegen_pure`（`build.rs:455-504`）。
6. `brpb.rs` 的三个 oneof 变体被改为 Rust 风格名称并追加 `pub type Gcs = GCS;`；全部 9 份生成文件去掉 `#!...` 与 `//!...` 行，随后写出平铺的 `kvproto_bindings.rs` 和嵌套的 `kvproto/mod.rs`（`build.rs:505-577`）。
7. 对 `schema.proto`、`analyze.proto`、`resourcetag.proto` 重复规范化与 codegen；生成文件同样去掉 crate 属性和模块文档，`tipb/mod.rs` 只声明并再导出 `resourcetag`（`build.rs:579-625`）。最后仅对仓库内的 `externalworkload.proto` 发出额外重跑指令（`build.rs:627-628`）。

## 数据与状态

脚本没有进程内全局可变状态。主要状态是构建进程环境和文件系统：`PROTOC` 在专用构建脚本进程内设置，`HOME`、`OUT_DIR` 由环境读取，Go module 路径由 `go` 命令或 `go.mod`/默认缓存布局决定。`PathBuf` 保存路径，schema、测试和生成代码在内存中以 `String`/`Vec` 做一次性转换。

持久化边界仅是 `OUT_DIR`：测试适配文件、`selector.rs`、临时 proto、生成绑定和模块清单都由当前构建覆盖。源文件本身不被改写。`kv_names` 与 `tipb_names` 是函数局部数组，限定实际生成的 schema 子集；新增 proto 不会被自动发现。

需要注意两个文本不变量。第一，测试适配只识别按行出现的 `#![`，且使用无语法感知的全局 `replace`；字符串、注释或相似标识也可能被替换。第二，proto 规范化依赖三段选项文本精确匹配；上游仅改变空白或拼写时，替换会静默不发生。

## 依赖与调用关系

上游入口只有 Cargo：根 `Cargo.toml` 声明构建脚本，并在 `[build-dependencies]` 中提供 `protobuf-codegen-pure = 2.8.0`、`protoc-bin-vendored = 3`、`tonic-build = 0.12`（`Cargo.toml:1482-1485`）。构建时还要求可读取 `go.mod`、仓库内源文件以及 Go module cache。

文件内调用关系为 `main` 调用四个辅助函数；辅助函数向下依赖 `std::fs`、`std::env`、`std::process::Command` 与 `std::path`。protobuf 路径进一步依赖 vendored protoc、tonic/prost 生成链和 rust-protobuf 2.8 codegen。`Cargo.toml` 的 `[patch.crates-io]` 将 `protobuf`/`protobuf-codegen` 指向 PingCAP fork，但本文件自身只直接调用 `protobuf_codegen_pure`。

Go 侧输入来自 `github.com/pingcap/kvproto`、`github.com/pingcap/tipb` 与 `github.com/gogo/protobuf`。`go list` 会尊重当前 Go module 的代理与 replace 解析；回退分支则只支持默认 `$HOME/go/pkg/mod` 布局，不执行 Go module 路径的大小写转义。

输出消费关系必须分开描述：源码证明脚本会写出这些产物，但当前全仓检索没有找到根包对测试适配文件、`selector.rs` 或 `kvproto_bindings.rs` 的引用。其他子 crate 的 `OUT_DIR/kvproto/mod.rs`、`OUT_DIR/tipb/mod.rs` 或 `externalworkload` include 由各自构建脚本生成，Cargo 的 `OUT_DIR` 隔离意味着它们不是根脚本产物的消费者。

## 错误处理与边界

这是“失败即终止构建”的脚本：文件读写、UTF-8 解码、环境变量、路径转换和 codegen 主要使用 `expect`、`unwrap` 或 `assert!`。错误消息通常指出阶段或源路径，如 `read {source}`、`generate kvproto bindings`，但没有恢复、重试或部分成功协议。

`go_module_dir` 的回退边界尤其严格：只有 `Command::output()` 返回启动错误才进入 `go.mod` 回退；若 `go` 存在但 `go list` 返回非零，脚本直接 panic。读取 `go.mod` 时只接受模块名和版本位于同一行、以空白分隔的形式；未找到版本、缺少 `HOME` 或缓存目录不存在都会终止。

`go env GOMODCACHE` 采用类似策略：命令启动成功但状态非零会失败，命令无法启动才使用默认缓存。`Path::to_str().unwrap()` 还要求 proto 临时路径是有效 UTF-8。生成后处理假定 `brpb.rs` 等文件存在；若上游生成器改名或停止输出，会在读取阶段暴露错误。

Cargo 重跑追踪并不完整：30 个适配源、selector 和本地 external workload proto 都有 `rerun-if-changed`，但 `go.mod`、外部 module schema、构建脚本读取到的 Go cache 路径没有显式登记。Cargo 通常仍会在构建脚本自身变化时重跑，但外部 schema/cache 单独变化未必触发重新生成。

## 并发与资源生命周期

脚本自身单线程、顺序执行，没有锁、异步任务、通道或事务。两个 codegen 阶段共享一个 `OUT_DIR`，文件名空间也是共享的；顺序保证 kvproto 的 include 路径已解析后才生成 tipb。Cargo 为不同 package/build 实例分配隔离的 `OUT_DIR`，所以不能跨 crate 假定产物可见。

外部子进程只有短生命周期的 `go list` 与 `go env`，均通过 `output()` 等待完成并一次性收集 stdout/stderr。vendored protoc 由 tonic 构建链启动；失败通过 `expect` 回传为构建失败。脚本不创建长期后台进程，也没有显式临时目录清理；`OUT_DIR` 的保存与回收由 Cargo 管理。

文件写入不是原子替换，构建中途失败可能在 `OUT_DIR` 留下部分产物，但失败构建不会被视为成功。下一次执行会覆盖固定名称的输出；数组删项不会主动清理旧文件，因此消费端必须只引用当前模块清单，不能扫描目录推断有效模块。

## 与 Go 版本的对应关系

根目录没有同路径 Go 构建脚本可逐函数映射。对应关系体现在“复用 Go 依赖输入”：`go.mod` 锁定 gogo/protobuf、kvproto、tipb 版本，`go list -m` 提供 Go 工具链认定的 module 目录，Rust codegen 使用其中的 `.proto`。这保证 schema 来源与 Go 依赖版本一致，但生成器、运行时类型和命名规则仍是 Rust 专属。

`normalized_proto` 主动关闭 lite runtime 与 carllerche bytes/string 选项，`brpb` oneof 命名和 `Gcs` 别名也是 Rust 输出兼容修正；它们没有 Go 代码中的等价执行步骤。测试适配同样是 Rust 迁移期机制：它复用独立 Rust 测试主体，不是 Go 测试的直接转换，也不会改变原测试断言。

`externalworkload.proto` 的 canonical Rust 客户端位于 `pkg/extworkload/client`，该 crate 自己的 `build.rs` 与根脚本都启用服务端生成。Go/Rust 行为一致性应以共享 proto schema 和对应客户端测试为依据，而不能因两处都调用 tonic codegen 就推断根脚本产物已接线。

## 扩展指南

- 新增根级测试适配时，简单路径替换加入 `root_test` 表；需要 prelude 或局部重写时加入显式 `adapted_test` 调用。优先修正 canonical crate 的独立测试与实现，并确认根 `pkg/lib.rs` 或专用测试 target 确实 include 新产物；不要只生成一个无人消费的文件。
- 改写规则必须避免短字符串或通用标识的全局替换。若规则开始依赖语法结构，应改用可验证的生成模板/解析方法，并在独立测试文件中覆盖注释、字符串、crate 属性和模块路径边界；不要把测试嵌入 `build.rs`。
- 增加 kvproto/tipb schema 时，同步更新名称数组、include 目录、模块清单输出和实际消费者；若新增 schema 有跨 proto import，还要验证 include 顺序。`brpb` 后处理依赖生成文本，升级 protobuf codegen 时应增加独立构建脚本测试或产物快照来锁定替换前提。
- 改动 Go module 定位时，应覆盖 `go list` 成功、命令缺失、命令非零、`go.mod` 缺项、非默认 `GOMODCACHE` 和 Go 转义模块路径。当前没有根 `build.rs` 的同名独立测试，这是最直接的测试缺口。
- 增加文件输入时必须同步 `cargo:rerun-if-changed`；外部 module 内容无法可靠追踪时，可令生成输入由 lock/version 明确决定，或增加可审计的版本戳。兼容风险主要是生成 API/命名变化，性能风险主要是扩大 schema 和测试适配集合造成每次根包构建的 I/O/codegen 成本。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter build.rs` 找到根文件；`node --file build.rs --offset ...` 阅读全部 629 行；`query` 定位 `go_module_dir`、`normalized_proto`、`root_test`、`adapted_test`；`callees main` 确认四条文件内调用边。标准库调用的跨文件同名匹配被判定为图噪声，未用于结论。
- 源码与配置：完整阅读 `build.rs`；核对 `Cargo.toml:805-812,1482-1500` 的根包、构建依赖和库入口；核对 `go.mod:60,101,106` 的三个 Go module 版本。
- 入口与消费端：检索并阅读 `pkg/lib.rs`；核对 `pkg/extworkload/client/Cargo.toml`、`pkg/extworkload/client/build.rs` 和 `pkg/extworkload/client/client.rs:33` 的独立 proto 生成/消费链；检索其他 `OUT_DIR` include，避免把子 crate 产物误归给根脚本。
- 测试证据：从 `build.rs:198-441` 提取并逐一检查 13 个 `root_test` 与 17 个 `adapted_test` 输入，所有源路径均存在；仓库中未发现根 `build.rs` 的同名独立 Rust/Go 测试，也未发现根目录 `build.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求 `build.rs.md` 存在并恰含 11 个固定二级标题；人工复核重点是生成与实际消费的边界、失败条件、生命周期和安全扩展位置。
