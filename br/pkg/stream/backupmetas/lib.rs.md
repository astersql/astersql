# `br/pkg/stream/backupmetas/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-stream-backupmetas` 的 crate 根。`br/pkg/stream/backupmetas/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它指定为库入口，并用 `package.metadata.porting.go-package = "br/pkg/stream/backupmetas"` 记录对应的 Go 包。它不是解析算法的实现文件，而是把 [`parser.rs`](./parser.rs) 装配成该 crate 的公开接口。

生产构建只接入 `parser`：`#[path = "parser.rs"] pub mod parser` 声明公开子模块，`pub use parser::*` 又把其中的公开项重导出到 crate 根。因此调用方既可以写 `astersql_br_pkg_stream_backupmetas::parser::ParseName`，也可以使用仓库现行的扁平路径 `astersql_br_pkg_stream_backupmetas::ParseName`。`#[cfg(test)] #[path = "parity_test.rs"] mod parity_test` 只在测试配置下编译独立测试文件，不把测试逻辑放进生产源文件。

## 核心职责

本文件只有三项职责：

1. 作为独立 backupmeta 文件名解析库的编译入口；
2. 在测试构建中挂接 [`parity_test.rs`](./parity_test.rs)；
3. 将 [`parser.rs`](./parser.rs) 的公开常量、类型和函数提升到 crate 根，保持接近 Go 包级符号的调用方式。

真正的业务契约由 `parser.rs` 提供：识别 legacy 与 tagged backupmeta 文件名，产出 `ParsedName`，计算恢复窗口使用的 shift TS，并解释 flags 中的 empty/DDL 位。门面不读取元文件内容、不访问外部存储，也不自行校验时间戳关系。

crate 根的 `#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_imports, unused_variables)]` 是整个 crate 的 lint 策略。它允许移植代码保留 Go 风格公开命名（例如 `ParseName`、`FlushTS`、`ShiftTSFound`）以及暂未被所有 Rust 路径使用的兼容接口；这不改变运行时行为。

## 主要符号

- `mod parity_test`：私有测试模块，仅在 `cfg(test)` 成立时存在；文件路径显式指向 `parity_test.rs`。其中五个 `#[test]` 覆盖公开契约、shift-TS 分支、DDL flags、tagged 校验/前向兼容和 empty flag。
- `pub mod parser`：公开实现模块，显式映射到 `parser.rs`。它承载本 crate 的全部生产逻辑。
- `pub use parser::*`：glob 重导出 `parser` 的所有公开项。当前包括四个公开标签常量 `NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG`、`NAME_MIN_TS_TAG`、`NAME_MAX_TS_TAG`、`NAME_FLAGS_TAG`，类型 `ParsedName`、`ShiftTSStatus`，函数 `ParseName`、`TryParseTaggedBackupMetaFileName`，以及 `ParsedName` 的 `CalculateShiftTS`、`IsEmpty`、`HasDDLFiles` 方法。

本文件不声明函数、结构体、trait、常量或 `impl`，也没有除 `cfg(test)` 之外的 feature/平台条件编译项。`parser.rs` 内部的正则、flags 位和辅助解析函数保持私有，不会因 glob 重导出而变成公共 API。

## 执行流程

编译与调用链如下：

1. Cargo 以 `lib.rs` 为 crate 根编译库；生产构建跳过 `parity_test`，测试构建额外加载该独立模块。
2. `pub mod parser` 编译 `parser.rs`。其中两个 `LazyLock<regex::Regex>` 在第一次解析时初始化，而不是在 `lib.rs` 加载时主动执行。
3. `pub use parser::*` 在编译期建立扁平公开命名；它不生成一层运行时包装，也不改变返回值或错误。
4. 调用方从 crate 根调用 `ParseName` 或 `TryParseTaggedBackupMetaFileName`。前者在实现模块内先匹配 tagged，再匹配 legacy；后者只接受 tagged 格式。
5. 解析结果随后进入不同业务链：`br/pkg/stream/crr/internal/checkpoint/storage.rs::collect_meta_files` 用 `FlushTS`、`StoreID`、`IsEmpty` 筛选 checkpoint 元文件；`br/pkg/stream/stream_mgr.rs::FilterPathByTs` 用时间范围裁剪路径；`br/pkg/stream/stream_metas.rs::UpdateShiftTS` 与 `br/pkg/restore/log_client/log_file_manager.rs::loadShiftTS` 用 `CalculateShiftTS` 决定 PiTR 恢复的读取下界。

因此，本文件的“执行”本质上是编译期模块接线。文件名解析、错误生成和状态计算均发生在 `parser.rs` 的符号中。

## 数据与状态

门面自身不持有数据或可变状态。经其重导出的主要数据模型是 `ParsedName`：包含 `FlushTS`、`StoreID`、`MinBeginTsInDefaultCf`、`MinTS`、`MaxTS`、`Flags` 和用于区分“未提供 flags”与“flags 值为 0”的 `HasFlags`。这种区分保证 legacy/无 `p` 标签时 `HasDDLFiles()` 默认返回 true，并保证仅有 flags 数值但 `HasFlags == false` 时 `IsEmpty()` 不会误判。

`ShiftTSStatus` 用 `#[repr(u8)]` 固定 `ShiftTSFound = 0`、`ShiftTSNotFound = 1`、`ShiftTSInvalidStats = 2`；`log_file_manager.rs::loadShiftTS` 当前确实按 `status as u8` 匹配这些数值，所以调整枚举顺序或判别值属于兼容性变更。

实现模块唯一的进程级状态是两个只读 `LazyLock<regex::Regex>`。它们首次使用时初始化，此后共享；解析结果按值返回，没有全局缓存、数据库事务或持久化副作用。

## 依赖与调用关系

直接下游只有 `parser.rs`，其唯一 Cargo 外部依赖是 `regex = "1"`。`lib.rs` 不直接引用其他 crate。`Cargo.toml` 将本包标记为 `kind = "library"`、porting lane 2；Go 的 Bazel `BUILD.bazel` 则仍把 `parser.go` 构建为公开 `go_library`，说明 Rust 与 Go 实现目前并存。

RustCodeGraph 对 `parser.rs` 给出的直接使用文件包括：

- `br/pkg/stream/backupmetas/parity_test.rs`：从 crate 根导入重导出的公开 API；
- `br/pkg/stream/crr/internal/checkpoint/storage.rs`：导入 `ParseName`，遇到坏文件名时将上下文包装为扫描错误并停止 walk；
- `br/pkg/stream/stream_mgr_fuzz_test.rs`：对 legacy/tagged、flags 和缺失标签做契约验证。

仓库搜索还确认以下 crate 通过 Cargo 路径依赖使用该门面：`br/pkg/stream/Cargo.toml`、`br/pkg/stream/crr/internal/checkpoint/Cargo.toml` 和 `br/pkg/utiltest/crr/Cargo.toml`。生产代码中的直接/间接消费者包括 `stream_mgr.rs`、`stream_metas.rs`、CRR checkpoint `storage.rs`，以及恢复侧 `restore/log_client/log_file_manager.rs`；测试辅助 `utiltest/crr/flush_sim.rs` 使用公开 tag 常量生成文件名，`utiltest/crr/harness.rs` 再用 `ParseName` 回读。

## 错误处理与边界

门面不捕获、包装或转换错误；重导出后仍保留 `parser.rs` 的 `Result<ParsedName, String>` 契约。解析错误会带入原文件名，并区分总体格式错误、tagged/legacy 格式错误、前缀长度、十六进制转换、非法或重复标签、残缺标签段和必需 `d/l/u` 标签缺失。

关键边界来自实现与测试：legacy 必须是四段、每段 16 位十六进制；tagged 必须以 32 位十六进制的 flush/store 前缀开头，后接一个或多个“ASCII 字母数字 tag + 16 位十六进制值”的 17 字节段；`d/l/u` 必须出现且不可重复；未知的字母数字 tag 会被解析并标记为已见，但其值被忽略，以保留前向兼容性。`TryParseTaggedBackupMetaFileName` 明确拒绝 legacy 名称。

错误策略由上游决定，并不统一：CRR checkpoint 将坏名称视为错误并停止扫描；`stream_mgr.rs::FilterPathByTs` 为兼容未来命名而对解析失败的路径原样放行；shift-TS 路径在 tagged 名称无效或统计无效时可回退读取元文件内容。扩展门面时不能假设所有调用者都会以相同方式处理 `Err`。

## 并发与资源生命周期

`lib.rs` 不创建线程、异步任务、通道、锁、文件句柄或网络连接，也没有显式初始化/关闭协议。模块声明与重导出均在编译期完成。

两个正则由 `parser.rs` 的 `std::sync::LazyLock` 管理：首次并发访问只初始化一次，之后进行不可变共享；固定正则若编译失败会在初始化时因 `expect` panic，但模式是源码常量。单次解析只创建局部字符串切片、legacy 分段向量、tag 访问表和 `ParsedName` 值；没有跨调用共享的可变解析状态。资源读取、walk 的停止、缓存关闭等生命周期属于调用者（例如 checkpoint `storage.rs` 或 `stream_mgr.rs`），不由本门面拥有。

## 与 Go 版本的对应关系

Rust `parser.rs` 对应同目录 `parser.go`，而 `lib.rs` 用“公开子模块 + crate 根重导出”模拟 Go 包天然的包级符号。公开数据、标签、解析入口和方法保持一一对应；Rust 只将 Go 的 `error` 换成字符串错误，并将接收者方法实现为 `impl ParsedName`。

当前可验证的语义对齐包括：两种正则格式及 tagged 优先级；`ParsedName` 字段集合；`ShiftTSStatus` 的三个数值；窗口无交集、统计为 0 或 `MinBeginTsInDefaultCf > MinTS` 的分支；缺少 flags 时默认含 DDL；empty 与 no-DDL 分别使用 flags 的第 1、0 位；未知 tag 忽略、重复 tag 拒绝、`d/l/u` 必填。`parity_test.rs` 和 `stream_mgr_fuzz_test.rs` 对这些行为提供 Rust 独立测试证据，Go 侧 `stream_mgr_fuzz_test.go` 提供对应输入与期望。

差异主要在工程装配：Go 由 `BUILD.bazel` 直接把 `parser.go` 作为 `backupmetas` 包；Rust 由本文件成为 Cargo crate 根。Rust 门面还设置了全 crate lint allow，以容纳 Go 风格命名。当前没有证据表明 Rust 门面取代了 Go 生产包；两套依赖图并存，应把它视为已接线的 Rust 移植库，而非 Go 包的透明运行时替换。

## 扩展指南

- 新增解析行为应修改 `parser.rs`，而不是在 `lib.rs` 再包一层；若符号应成为包级 API，保持 `pub` 即会被现有 `pub use parser::*` 导出。新增私有辅助项应保持私有，避免无意扩大根级 API。
- 新增模块时需在本文件显式声明，并判断是否真的需要根级重导出；glob 导出多个模块可能造成同名冲突或难以追踪的 API 漂移。
- 更改公开字段、tag 常量、flags 位或 `ShiftTSStatus` 判别值前，应同步核查 Go `parser.go`、`stream_metas.rs`、`stream_mgr.rs`、CRR checkpoint `storage.rs`、恢复侧 `log_file_manager.rs` 与 utiltest 文件名生成器。尤其不能破坏 `status as u8` 的现有依赖。
- 测试逻辑继续放在独立的 `parity_test.rs`，并通过本文件的 `cfg(test)` 模块声明接入；跨 crate 行为可在 `stream_mgr_fuzz_test.rs`、`crr/internal/checkpoint/storage_internal_test.rs` 或对应调用模块的独立测试文件中扩展，不应把单元测试内嵌到 `lib.rs`/`parser.rs`。
- 新增文件或改 Cargo/Bazel 接线时，应同步检查 workspace/Bazel 元数据要求；本次仅新增说明文档，没有改变这些构建输入。

## 验证依据

- RustCodeGraph `status`：索引覆盖 7,032 个 Rust 文件；`files --filter br/pkg/stream/backupmetas` 定位到 `lib.rs`、`parser.rs`、`parity_test.rs` 与 Go 对照文件。
- RustCodeGraph `node --file br/pkg/stream/backupmetas/lib.rs`：确认 20 行 crate 根、`cfg(test)` 测试接线、公开 `parser` 和 glob 重导出；图中该文件仅有一个模块级符号。
- RustCodeGraph `node --file br/pkg/stream/backupmetas/parser.rs`：核对全部公开 API、正则惰性初始化、解析分支、错误路径和 flags/shift-TS 语义。
- RustCodeGraph 对 `parity_test.rs`、CRR checkpoint `storage.rs`、`stream_metas.rs`、`stream_mgr.rs`、恢复侧 `log_file_manager.rs` 的文件节点：核对测试边界及真实生产消费方式。
- `br/pkg/stream/backupmetas/Cargo.toml` 与 `BUILD.bazel`：核对 Rust crate 根、`regex` 依赖、Go 包映射和 Go 构建边界。
- `br/pkg/stream/backupmetas/parser.go`、`parity_test.rs`、`br/pkg/stream/stream_mgr_fuzz_test.go`、`br/pkg/stream/stream_mgr_fuzz_test.rs`：核对 Go/Rust API 与边界条件。仓库 `rg` 搜索补充确认 Cargo 路径依赖和未由调用图完整列出的消费者。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核唯一新增生产物为本文档。
