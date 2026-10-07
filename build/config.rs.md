# [`build/config.rs`](config.rs)

## 文件定位

`build/config.rs` 是根 Rust 包 `astersql` 的构建/静态检查配置模块。根 `Cargo.toml` 的 `[lib]` 指向 `pkg/lib.rs`，后者以 `#[path = "../build/config.rs"] pub mod build;` 将本文件公开为 `astersql::build`。它不是 `pkg/config` 的数据库运行时配置；它负责把同目录的 `nogo_config.json` 提供给 Rust linter。直接生产消费者是 `build/linter/util/exclude.rs`。

## 核心职责

- `configFile` 用 `include_bytes!` 在编译期嵌入 `nogo_config.json`，运行时不读文件。
- `NogoConfig` 用 `LazyLock` 一次解析、全局共享配置；内嵌内容无效时 panic。
- `parse_config` 与私有 `Parser` 实现所需 JSON 解析，将“分析器名 -> 配置”转换为 `HashMap<String, AnalysisConfig>`。
- `AnalysisConfig` 以 `Option<HashMap<...>>` 区分字段缺失/`null` 与显式空对象。
- `skip_value` 验证并忽略分析器对象中的未知字段，支持字符串、对象、数组、布尔、`null` 和数字。

## 主要符号

- `configFile: &[u8]`：私有、进程期有效的内嵌 JSON 字节。
- `NogoConfig: LazyLock<NogoConfigFormat>`：主要公开入口；闭包调用 `parse_config`，失败统一 panic 为 `fail to parse nogo_config.json`。
- `NogoConfigFormat = HashMap<String, AnalysisConfig>`：顶层分析器表。
- `AnalysisConfig`：公开值类型，字段为 `ExcludeFiles` 和 `OnlyFiles`；根库允许非 snake case，以保持 Go 形状。
- `init()`：显式调用 `LazyLock::force`；不调用时首次读取也会初始化。
- `ParseError { offset, message }`：crate 内错误，字段私有，记录失败字节位置和静态类别。
- `parse_config(&[u8])`：crate 内解析入口，并拒绝根值后的非空白尾随数据。
- `Parser { input, offset }`：借用输入的游标；`parse_root`、`parse_analysis_config`、`parse_optional_string_map` 分层构造配置。
- `parse_string`、`parse_unicode_escape`、`parse_hex_u16`：处理 UTF-8、JSON 转义和 UTF-16 surrogate pair。
- `skip_value`、`skip_number`、`consume_digits` 及 `consume_literal`、`expect`、`consume`、`peek`、`skip_whitespace`、`error`：未知值与游标辅助逻辑。

## 执行流程

1. 编译根包时，`pkg/lib.rs` 接入本模块，`include_bytes!` 将 JSON 固化进产物。
2. `build/linter/util/exclude.rs::shouldRun` 首次执行 `build::NogoConfig.get(passName)` 时，或显式调用 `init()` 时，`LazyLock` 运行初始化闭包。
3. `parse_config` 创建 `Parser`，调用 `parse_root`，再跳过末尾空白并要求输入耗尽。
4. `parse_root` 接受 `null`（转为空表）或对象；逐项解析分析器名和值，重复键由后值覆盖。
5. `parse_analysis_config` 接受 `null`（默认配置）或对象；以 ASCII 大小写不敏感方式识别 `exclude_files`/`only_files`，未知字段由 `skip_value` 验证后忽略。
6. `parse_optional_string_map` 将 `null` 变为 `None`，将 `{}` 变为 `Some(empty)`，非空对象必须是字符串到字符串的映射。
7. `shouldRunConfig` 先应用 `OnlyFiles`；其不存在时才应用 `ExcludeFiles`；均不存在则允许运行。

## 数据与状态

永久状态只有内嵌字节和进程级只读 `NogoConfig`。解析中的 `Parser.input` 借用调用者数据，`offset` 指向下一字节或错误位置；结果拥有其字符串和映射。对象使用 `HashMap`，遍历无序，重复键最后一次写入生效。

字段级 `Option<HashMap<...>>` 是关键不变量：缺失和 JSON `null` 都是 `None`，显式空对象是 `Some(empty)`。顶层 `null` 被规范化为空 `HashMap`，不保留 nil/非 nil 区别。未知对象键或字符串值解析时会临时分配，但不会构造通用 JSON 树。

## 依赖与调用关系

接线链为 `Cargo.toml [lib] -> pkg/lib.rs -> build/config.rs`。RustCodeGraph 显示 `parse_config` 的调用者为 `NogoConfig` 初始化闭包及 `build/config_test.rs`；内部主边是 `parse_config -> parse_root`、`parse_root -> parse_analysis_config`、`parse_analysis_config -> parse_optional_string_map/skip_value`。

生产读取链为 `build/linter/util/exclude.rs::shouldRun -> build::NogoConfig.get -> shouldRunConfig -> regexMatch`。本文件只依赖标准库 `HashMap`、`LazyLock`，资源依赖为 `build/nogo_config.json`。JSON 变更需重新编译才进入产物。

## 错误处理与边界

`parse_config` 返回 `ParseError`，类别覆盖尾随数据、意外 token、非法 literal/数字/转义/UTF-8、未终止字符串及 surrogate 错误。全局初始化有意丢弃细节并 panic，以对齐 Go 启动失败契约。

解析器接受 JSON 四种空白，拒绝未转义控制字符；`\uXXXX` 支持合法代理对并拒绝孤立代理。数字支持负号、小数和指数，且小数点/指数后必须有数字。根值只允许对象或 `null`；已知映射只允许对象/`null` 且键值必须为字符串，因此 `{"exclude_files":[]}` 会失败。未知值仍须是合法 JSON。递归对象/数组没有显式深度限制，但输入是仓库内编译期配置，不是外部请求。

## 并发与资源生命周期

`LazyLock` 提供线程安全的一次初始化；成功后并发读者共享不可变配置，无每次读取锁。`init()` 重复调用不会重复解析。若初始化闭包 panic，当前访问失败，模块没有恢复、热重载或替换路径。解析临时状态随调用结束释放，最终配置存活至进程退出；不存在文件句柄、任务、通道、事务或显式锁。

## 与 Go 版本的对应关系

`build/config.go` 以 `//go:embed`、包级 `NogoConfig` 和 `init + json.Unmarshal` 完成相同主链；Rust 对应使用 `include_bytes!`、`LazyLock`、专用 `parse_config` 和可选 `init()`。两端都将分析器名映射到 `exclude_files`/`only_files`，无效内嵌配置都以固定消息 panic。

Go map 的字段缺失/`null` 为 nil、`{}` 为非 nil 空 map；Rust `Option<HashMap>` 保留此字段级区别，`build/config_test.rs` 明确覆盖三态。两端并非通用 JSON 实现的逐字等价：Go 使用 `encoding/json`，Rust 使用专用解析器；Rust 还把顶层 `null` 规范化为空表。扩展格式时必须同时审查两端。

## 扩展指南

- 新增配置字段时，扩展 `AnalysisConfig` 和 `parse_analysis_config`，同步 `build/config.go`、消费者与独立 `build/config_test.rs`；若改变过滤语义，还应更新 `build/linter/util/exclude_test.rs`。
- 新值类型若仅用于未知字段，应扩展 `skip_value` 并覆盖非法/嵌套边界，不必把通用 JSON 树引入已知字符串映射路径。
- 改善诊断可为 `ParseError` 增加格式化，但若改变固定 panic 文本需先核对兼容要求。
- 热更新需要重新设计所有权、同步和失败回滚，不能直接改写 `LazyLock`。
- Rust 测试继续放在独立 `build/config_test.rs`，由 `pkg/lib.rs` 接线，不能内嵌到本源文件；真实 JSON 变更还需验证分析器条目及过滤优先级。

## 验证依据

- RustCodeGraph：`status` 确认索引可用；`files --filter build/config.rs` 报告本文件 21 个符号；`node --file build/config.rs --offset 1 --limit 500` 读取完整源码；`explore "build/config.rs configuration symbols callers callees"`、`callers/callees parse_config`、`node shouldRunConfig` 核对解析与消费边。
- 源码/接线：`build/config.rs`、`pkg/lib.rs`、根 `Cargo.toml`、`build/linter/util/exclude.rs`、`build/nogo_config.json`。
- Go 对照：`build/config.go`，用于核对嵌入、数据形状、初始化与 panic 契约。
- 独立测试：`build/config_test.rs` 覆盖真实配置、非法字段类型及三态；`build/linter/util/exclude_test.rs` 覆盖 `OnlyFiles` 优先级、正则错误 panic 和真实规则可执行。
- 本任务按计划不运行 Cargo；交付使用固定 11 节结构检查及人工事实、链接与范围复核。
