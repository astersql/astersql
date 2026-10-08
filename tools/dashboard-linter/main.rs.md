# `tools/dashboard-linter/main.rs`

## 文件定位

[`main.rs`](main.rs) 是 `astersql-tools-dashboard-linter` crate 的二进制实现，也是 Grafana dashboard JSON 静态检查器的 Rust 规则主体。`tools/dashboard-linter/Cargo.toml` 用 `[[bin]]` 将它注册为 `astersql-tools-dashboard-linter`，并以 `[package.metadata.porting].go-package = "tools/dashboard-linter"` 标明其 Go 对照目录；根 `Cargo.toml` 又把该 crate 纳入 workspace。

同目录的 [`lib.rs`](lib.rs) 通过 `#[path = "main.rs"] pub mod main` 把这份二进制逻辑同时暴露为库模块，并由 `entry()` 转发到 `main::main()`；`#[cfg(test)]` 下则挂接 [`parity_test.rs`](parity_test.rs)。因此本文件既是 CLI 入口，又是 Go/Rust 对齐测试直接调用的生产实现，而不是仅供命令行使用的薄转发层。

Go 侧真实入口是 [`main.go`](main.go)。仓库 `Makefile` 的 `lint` 目标目前仍以 `go run tools/dashboard-linter/main.go <dashboard.json>` 检查 `pkg/metrics/grafana` 与 `pkg/metrics/nextgengrafana` 下的一组 dashboard；Rust 文件移植并复现该行为，但现有 Makefile 证据没有表明 lint 目标已切换到 Rust 二进制。

## 核心职责

本文件的职责是把 dashboard 文件从命令行输入转成一组按固定优先级执行的检查，并用进程返回码表达结果：

1. `run` 校验参数、读取目标文件，再把原始字节交给 `lint_dashboard`。
2. `lint_dashboard` 通过 `parse_dashboard_json` 解析出 linter 所需的最小模型，只保留顶层 `panels` 及 panel 的 ID、嵌套 panel、类型、标题、折叠状态、数据源和高度。
3. 首先递归检查 panel 字段约束；任一字段错误都会返回 `1`，后续重复 ID 与模式检查不再执行。
4. 统计顶层 panel 及其一层 `panels` 子项的 ID，报告重复 ID 和可用 ID 区间。
5. 在原始 JSON 字节中拒绝 `.*$tidb_cluster` 或 `$tidb_cluster.*` 两种不必要的模式匹配写法。

它不负责通用 Grafana schema 验证，也不遍历任意深度来统计 ID。字段校验由 `check_panel` 对 row 子项递归执行，而 ID 统计在 `lint_dashboard` 中明确只展开一层；这两个遍历深度不能混为一谈。

## 主要符号

- `BasicDashboard { panels: Vec<Panel> }`：最小顶层数据模型；未知 JSON 字段由解析层忽略。
- `Panel`：统一表示普通 panel 与 row panel。`id` 是重复检测键；`panel_type` 决定校验分支；`panels` 承载 row 子项；`title`、`collapsed`、`datasource`、`grid_pos` 分别服务标题、折叠、模板数据源和高度规则。
- `GridPos { h: i64 }`：对应 Go 匿名 `gridPos.h` 结构。
- `ROW_TYPE`：值为 `"row"`，是 row 分支的共享判别常量。
- `main() -> ()`：进程入口；调用 `run`，仅在返回码非零时调用 `std::process::exit`。
- `run(args: &[String]) -> i32`：可测试的 CLI 外壳；缺少路径时打印 usage 并返回 `1`，文件读取失败则 panic。
- `lint_dashboard(file_name: &str, content: &[u8]) -> i32`：规则总入口，依次执行解析、字段检查、ID 检查和 `$tidb_cluster` 检查。
- `parse_dashboard_json` / `try_parse_dashboard_json`：分别提供 panic 型生产解析入口和 `Result` 型测试/细粒度调用入口；实际反序列化位于 `stubs::unmarshal_dashboard`。
- `index_of_any`：按模式切片顺序逐一搜索，返回第一个成功模式的首个字节偏移；并非在所有模式的命中位置中选全局最小值。
- `collect_id_stats`：从 `id -> 使用次数` 映射产生重复项及排序后的可用区间文本。
- `check_dashboard_panel_fields` / `collect_panel_field_errors`：共享 `check_panel`，前者打印错误并返回可选失败码，后者纯收集错误供测试使用。
- `check_panel`：字段规则核心；row 要求折叠并递归子项，普通 panel 要求模板数据源、高度 7、非空标题，且标题中长度大于 1 的空格分词必须以 Go `unicode.IsUpper` 或 `unicode.IsDigit` 接受的字符开头。
- `go_is_upper` / `go_is_digit`：私有 Unicode 兼容辅助函数，用于缩小 Rust 字符分类与 Go Unicode general category 的差异。
- `format_go_int_map` / `format_go_string_slice`：私有输出适配函数，生成接近 Go `%v` 的文本；map 键值会额外排序以稳定 Rust 输出。

除最后四个私有辅助函数外，上述模型和规则辅助入口多数为 `pub`，原因是同一实现需要由 `lib.rs`、独立 parity 测试及潜在库调用方复用。

## 执行流程

正常 CLI 路径为 `main → run → lint_dashboard`。`run` 只读取 `args[1]`，多余参数被忽略；读取使用 `std::fs::read`，因此模式扫描面对的是未经重新序列化的原始字节。

`lint_dashboard` 的顺序具有用户可见意义：

1. `parse_dashboard_json` 调用 `stubs::unmarshal_dashboard`；语法或类型错误触发 panic。
2. `check_dashboard_panel_fields` 以顶层顺序遍历 panel。普通 panel 的错误按数据源、高度、空标题、标题单词顺序追加；row 先检查 `collapsed`，再按子项顺序递归。收集完成后逐行打印并返回 `1`。
3. 字段全部合法时，以容量 1024 创建 `HashMap<i64, i64>`，统计每个顶层 ID 以及每个顶层 panel 的直接子 panel ID。
4. `collect_id_stats` 提取计数大于 1 的 ID，排序所有已用 ID，并从最小已用 ID 的后继开始生成缺口：单个缺号写成数字，连续缺口写成半开区间，最后追加 `[next, ∞)`。它不会报告最小已用 ID 之前的空间。
5. 有重复 ID 时打印重复计数与可用区间并返回 `1`。否则调用 `index_of_any`；命中时截取命中点前最多 150 字节、后最多 50 字节，用 `String::from_utf8_lossy` 打印上下文并返回 `1`。
6. 所有检查通过时返回 `0`；`main` 不显式退出，让进程自然以成功状态结束。

RustCodeGraph 的 `callees lint_dashboard` 证实其直接下游为 `parse_dashboard_json`、`check_dashboard_panel_fields`、`collect_id_stats`、`index_of_any` 以及两个格式化函数；`callees check_panel` 证实字符规则调用 `go_is_upper` 与 `go_is_digit`。索引未返回这两个符号的调用者列表，但文件关系显示 `parity_test.rs` 直接调用公开辅助函数，`lib.rs` 通过路径模块复用整个文件。

## 数据与状态

所有状态都限定在一次同步调用内，没有全局可变状态：JSON 字节由 `run` 所有并借用给 `lint_dashboard`；解析结果 `BasicDashboard` 在调用栈内拥有 `Panel` 树；错误、ID 计数和可用区间均为局部集合。

`Panel::default()` 以及解析层的零值语义很重要：缺失数值字段为 `0`，字符串为空，布尔值为 `false`，列表为空。对普通 panel 来说，这通常会自然触发 datasource、高度或标题规则；空 `BasicDashboard` 则合法通过。未知 JSON 字段不进入模型，但原始字节仍用于 `$tidb_cluster` 子串检查，因此 targets 等未建模内容依然会被该规则覆盖。

ID 使用 `i64`，以对应仓库支持的 64 位 Go `int`。区间推进使用 `wrapping_add` / `wrapping_sub`，避免 debug 构建因整数溢出 panic；极端 `i64` 边界下会出现环绕文本，这是当前实现的明确行为，扩展时不能未经对照就改成饱和或报错语义。

## 依赖与调用关系

该 crate 的 `[dependencies]` 为空。本文件只依赖标准库的 `HashMap`、环境参数、文件读取、进程退出与 UTF-8 有损转换；JSON 兼容逻辑通过私有 `mod stubs` 落在同目录 [`stubs.rs`](stubs.rs)，没有引入 serde 等外部依赖。

上游关系包括：二进制 target 直接调用本文件 `main`；`lib.rs::entry` 调用 `main::main`；`parity_test.rs` 直接调用 `run`、`lint_dashboard`、`try_parse_dashboard_json`、`collect_panel_field_errors`、`check_panel`、`collect_id_stats` 与 `index_of_any`。仓库实际 Go lint 链的上游是 Makefile `lint` 目标，但当前命令调用的是 `main.go` 而非 Rust target。

下游关系包括：`run` 依赖 `std::fs::read`；`lint_dashboard` 依赖解析兼容层与本文件各规则函数；解析层依赖本文件定义的 `BasicDashboard`、`Panel` 和 `GridPos`。这种 `main.rs ↔ stubs.rs` 的源码级关系由 Rust 模块树解析，不是运行时循环依赖。

## 错误处理与边界

错误分为三类。用户未给路径时，`run` 打印 usage 并返回 `1`；字段、重复 ID 或模式规则失败时，lint 入口打印诊断并返回 `1`；文件读取与生产解析错误使用 panic，以保留 Go `panic(err)` 的失败面。只有 `try_parse_dashboard_json` 把解析错误作为 `Result<_, String>` 返回。

字段规则的关键边界如下：row panel 不检查自身 datasource、高度或标题，只要求折叠并递归校验其子 panel；非 row panel 即使标题为空，也会先累积数据源与高度错误。标题用单个 ASCII 空格分词，长度不超过 1 字节的词跳过；其余词仅看第一个 Unicode 字符。`go_is_upper` 排除 Rust `is_uppercase` 接受但不属于 Go `Lu` 类别的已列范围，`go_is_digit` 显式枚举 `Nd` 范围。

模式搜索是原始字节的字面匹配，不解析正则含义。上下文切片按字节边界计算，随后才有损转 UTF-8；若边界落在多字节字符内部，诊断可能出现替代字符但不会 panic。`index_of_any` 对空模式返回 `Some(0)`，与 Go `bytes.Index` 一致。

当前 ID 统计只检查顶层和一层子项，而字段规则可继续递归更深层 row。新增深层 dashboard 支持时必须明确这是要保持 Go 当前限制，还是同时修改 Go、Rust 与 parity 测试；不能只改变其中一个遍历。

## 并发与资源生命周期

实现是单线程、同步、一次性读取整份文件的。没有线程、异步任务、锁、通道或事务；`HashMap`、解析树和诊断字符串在函数返回时释放。内存复杂度主要由输入字节、解析后的 panel 树、字段错误数量和不同 ID 数量决定；ID 排序使统计阶段为 `O(n log n)`，字面模式逐个做窗口扫描。

`run` 读取完成后不保留文件句柄。`parity_test.rs::contract_resource_cleanup` 在调用 `run` 后能删除临时文件，直接验证成功路径不会持有阻碍清理的资源。临时目录的创建与删除属于测试夹具，不是 linter 的生产职责。

`main` 在失败码上调用 `process::exit`，因此不会执行调用栈中局部值的正常析构；当前资源仅为进程私有内存和已完成读取的文件，不存在必须显式提交或释放的外部资源。若未来加入缓冲写入、锁或遥测，应避免把需要清理的资源放在返回 `main` 之前仍未完成的生命周期中，或改用由外层统一退出的结构。

## 与 Go 版本的对应关系

结构与主规则逐段对应 [`main.go`](main.go)：`basicDashboard`、`panel`、`rowType` 分别映射为 `BasicDashboard`、`Panel`、`ROW_TYPE`；Go `main` 的参数、读取、解析、字段检查、ID 统计和模式扫描被拆为可测试的 `run` 与 `lint_dashboard`；`indexOfAny` 和 `checkPanel` 分别对应 `index_of_any` 与 `check_panel`。

为允许测试而产生的 Rust 结构性差异包括：Go 的 `os.Exit(1)` 被内部返回码替代，只有 Rust `main` 真正退出；Go 匿名 `GridPos` 被提取为具名类型；错误使用 `String` 而非 `error`；解析实现移到 `stubs.rs`；统计和无副作用字段收集被提取成辅助函数。`format_go_int_map` 会排序输出，而 Go map 输出顺序未规定，因此诊断更稳定但键顺序不承诺逐字一致。

Go `encoding/json` 的相关语义由 `stubs.rs` 与 `parity_test.rs::go_json_unmarshal_contract_matches` 钉住，包括已知字段类型错误、大小写/Unicode simple-fold 字段匹配、重复键后值覆盖、代理对、非 JSON 空白与控制字符。字符分类由 `go_unicode_title_categories_match` 覆盖。主契约测试还覆盖成功路径、空 dashboard、ID 区间、row 递归、字段错误、重复 ID、模式拒绝、缺失文件 panic 和资源清理。

值得注意的是，Go `checkDashboardPanelFields` 直接 `os.Exit(1)`，Rust 通过返回 `Some(1)` 实现同一 CLI 短路效果；库调用时 Rust 因而可以观察返回码而不终止测试进程。这是为了可测试性做的控制流适配，不是规则放宽。

## 扩展指南

新增 panel 字段规则应优先修改 `check_panel`，并在独立 [`parity_test.rs`](parity_test.rs) 中增加正常、边界和失败断言；不要把测试内嵌到 `main.rs`。若规则需要新 JSON 字段，还必须同步 `Panel`/`BasicDashboard` 与 `stubs.rs` 的字段匹配和类型转换，并以 `main.go` 的对应增量作为语义依据。

新增整份文档级规则应接入 `lint_dashboard`，同时明确它在字段检查、重复 ID 和模式检查之间的优先级，因为返回码短路会改变用户首先看到的诊断。若改变 CLI 参数，应修改 `run` 并同步 usage、panic/返回码契约及真实文件入口测试。

修改 ID 行为时应同时审查两处遍历：`lint_dashboard` 的收集深度与 `check_panel` 的递归深度。扩展到任意深度可能增加栈深、时间与内存成本，也会改变 Go 版当前“ID 只嵌套一次”的假设；需要同步 Go 实现或明确迁移状态，并补深层、重复和整数边界测试。

修改 Unicode 或 JSON 兼容性时不要只依据 Rust 标准库的近似 API。应先核对 Go `unicode.IsUpper`、`unicode.IsDigit` 或 `encoding/json` 的真实行为，再更新 `go_is_upper`、`go_is_digit` 或 `stubs.rs`，并扩充 parity 测试。修改错误文案、打印格式或规则顺序同样具有兼容风险，因为 Makefile lint 与人工修复流程依赖可读诊断。

性能方面，当前工具面向有限大小的 dashboard 并整文件加载。若输入规模增长，优化应保留原始字节模式扫描、确定性错误顺序和 Go 兼容解析；引入流式解析不能悄悄遗漏未知字段中的 `$tidb_cluster` 模式。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter tools/dashboard-linter` 列出 `main.rs`、`lib.rs`、`stubs.rs`、`parity_test.rs` 与 `main.go`。
- RustCodeGraph `node --file tools/dashboard-linter/main.rs`：核对了本文件 1–353 行的全部类型、常量、入口、规则与私有辅助函数，并显示该文件与 `parity_test.rs`、`stubs.rs` 的文件关系。
- RustCodeGraph `callees lint_dashboard`：得到 `parse_dashboard_json`、`check_dashboard_panel_fields`、`collect_id_stats`、`index_of_any`、`format_go_int_map`、`format_go_string_slice` 六条直接调用边。
- RustCodeGraph `callees check_panel`：得到 `go_is_upper` 与 `go_is_digit` 两条直接调用边；`callers` 查询未返回边，因此上游引用另由 `lib.rs` 和 `parity_test.rs` 的源码核实，没有将索引空结果推断为“无调用者”。
- [`Cargo.toml`](Cargo.toml)：核对 crate 名、workspace 属性、lib/bin target、Go 包映射、binary 移植类别以及无外部依赖事实。
- [`lib.rs`](lib.rs)、[`stubs.rs`](stubs.rs)：核对库入口、测试模块挂接、最小 JSON 解析边界及与本文件模型的直接依赖。
- [`main.go`](main.go)：逐段核对数据形状、规则顺序、诊断、panic/退出语义、一层 ID 统计与递归字段检查。
- [`parity_test.rs`](parity_test.rs)：核对独立测试位置及正常、边界、错误、Unicode/JSON 兼容与资源清理覆盖；本任务按计划不运行 Cargo。
- 仓库 `Makefile` 与 `tools/dashboard-linter/BUILD.bazel`：核对 Go 工具在 lint 目标中的实际调用，以及 Go binary/library 的构建边界。

本说明只分析 `tools/dashboard-linter/main.rs` 及上述直接证据；未修改或执行 Rust、Go、Cargo/Bazel 构建与测试。
