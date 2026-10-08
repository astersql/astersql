# `pkg/util/plancodec/go_quote_printable.rs`

## 文件定位

本文件是 `astersql-util-plancodec` crate 内部的一份生成数据表。crate 边界由 `pkg/util/plancodec/Cargo.toml` 定义，入口 `pkg/util/plancodec/lib.rs` 将 `codec.rs` 包装为私有 `codec` 模块并再导出其公开 API；`pkg/util/plancodec/codec.rs:633` 又通过 `include!("go_quote_printable.rs")` 把本文件文本纳入同一个模块。因此，它不是可单独调用的模块，也不导出公共 API，而是计划解码错误兼容逻辑的编译期数据依赖。

它位于文本执行计划的解码路径上，但不参与成功计划的树构造。只有数值字段解析失败、需要生成与 Go `strconv.Atoi` 一致的诊断文本时，`codec.rs` 中的 `go_atoi`、`quote_go_bytes` 才会间接读取该表。

## 核心职责

文件唯一职责是固定 Go 1.25 `strconv.IsPrint` 所采用的 Unicode 15.0.0 可打印字符集合，避免 Rust 当前 Unicode 数据版本变化导致协议诊断字符串漂移。源码注释明确规定该集合由所有满足 `strconv.IsPrint` 的 rune 合并成连续区间生成。

这份固定集合尤其用于区分“原样放入双引号”与“写成 `\u`/`\U` 转义”的 Unicode 字符。它保留 Go 的规则：ASCII 可打印区间为 U+0020..U+007E；非 ASCII 空格并不会仅因是空格就自动成为 `IsPrint` 字符，而字母、标记、数字、标点和符号等 Go 认定可打印的码点按表收录。其目标是错误消息字节兼容，不是面向终端显示宽度、SQL 字符串转义或通用 Unicode 分类。

## 主要符号

- `GO_PRINTABLE_RANGES: &[(u32, u32)]`：私有、静态生命周期的有序闭区间切片。每个元组的两个端点都包含在集合内；消费方把 Rust `char` 转成 `u32` 后查询该表。
- `#[rustfmt::skip]`：阻止格式化器重排这份生成数据，便于按生成结果审阅和再生成。它不改变运行时语义。

本文件没有函数、类型、trait、`impl`、条件编译项或可变状态。RustCodeGraph 将它识别为常量 `GO_PRINTABLE_RANGES`（`go_quote_printable.rs:7`）；调用关系存在于消费它的 `codec.rs` 中。

## 执行流程

1. `DecodePlan` 或 `DecodeNormalizedPlan` 进入 `PlanDecoder::buildPlanTree`，后者逐行调用 `decodePlanInfo`（均在 `pkg/util/plancodec/codec.rs`）。
2. `decodePlanInfo` 使用 `go_atoi` 解析 depth 和物理计划 ID；`decodeTaskType` 也使用它解析带后缀的 store type。RustCodeGraph 的 `go_atoi` 调用边确认其调用者为 `decodePlanInfo` 与 `decodeTaskType`。
3. 解析出现非法语法或范围溢出时，`go_atoi` 调用 `quote_go_bytes` 构造 `strconv.Atoi: parsing ...` 错误片段。
4. `quote_go_bytes` 先处理非法 UTF-8 字节、双引号、反斜杠、Go 短转义控制字符和 ASCII 空格。对其余合法 Rust `char`，它在 `GO_PRINTABLE_RANGES` 上执行 `partition_point(|(start, _)| start <= code)`。
5. 若前一个候选区间存在且 `code <= end`，字符原样写入；否则控制字符写成 `\xNN`，BMP 字符写成 `\uNNNN`，补充平面字符写成 `\UNNNNNNNN`。最终错误由 `decodePlanInfo` 包装进包含原计划行和字段标签的 `Error::GoMessage`。

表的有序、互不重叠性质是二分定位正确的前提；若区间乱序，`partition_point` 会产生错误分类而不会主动报错。

## 数据与状态

`GO_PRINTABLE_RANGES` 是只读常量切片，共 711 个闭区间。仓库内的静态检查确认每个区间均满足 `start <= end`，相邻区间严格递增且不重叠；首区间是 U+0020..U+007E，末区间是 U+E0100..U+E01EF。

区间使用 `u32`，与 Unicode 标量值比较时无需分配。消费方输入来自 Rust `char`，因此不会把 surrogate 范围当作合法字符；原始输入中的非法 UTF-8 则在查询该表之前逐字节转成 `\xNN`。本文件不缓存查询结果，也不持有请求级或全局可变状态。

## 依赖与调用关系

上游链路为 `DecodePlan`/`DecodeNormalizedPlan` → `PlanDecoder::buildPlanTree` → `decodePlanInfo` → `go_atoi` → `quote_go_bytes` → `GO_PRINTABLE_RANGES`。另一路为 `decodePlanInfo` → `decodeTaskType` → `go_atoi`，用于 task/store 字段错误。

下游只有 Rust 标准库切片方法 `partition_point` 和整数比较；本文件本身没有 Cargo 外部依赖。`pkg/util/plancodec/Cargo.toml` 中的 `base64`、`snap`、`protobuf`、`texttree-dependency`、`thiserror` 等依赖服务于同 crate 的其他编解码工作，并非生成表所需。

RustCodeGraph 能定位本常量和 `quote_go_bytes`/`go_atoi` 的源码，并确认 `go_atoi` 的上述调用者；当前索引未把 `include!` 展开为 `GO_PRINTABLE_RANGES` 的跨文件 callers/callees 边。因此 `codec.rs:633-665` 的 include 与直接读取关系以源码搜索作为补充证据，不能把图中“无调用者”解释为未接线。

## 错误处理与边界

本文件不返回 `Result`、不 panic，也不直接构造错误；风险来自数据不变量被破坏后造成静默的字符分类变化。重要边界如下：

- U+0020 是 ASCII 空格且原样输出；U+007F 不在表内，由消费方写成 `\x7f`。
- 非法 UTF-8 不会查询本表，而是每个无效起始字节写成 `\xNN`。
- 表外的有效 BMP 字符使用四位 `\u`，表外补充平面字符使用八位 `\U`；表内字符保留原 UTF-8。
- Unicode 新版本中新分配的字符不能自动变为可打印，否则会偏离 Go 1.25。`pkg/util/plancodec/codec_test.rs::error_quotes_use_go_unicode_version` 以 U+31E4 验证这一点：它必须被写成 `"\\u31e4"`。
- 区间端点采用闭区间；消费方先按起点二分，再检查前一区间终点，修改时必须同时保持排序、合并连续区间和无重叠。

## 并发与资源生命周期

该表位于程序只读静态数据中，没有初始化锁、堆分配、句柄或析构过程，可被所有线程并发读取。每次查找只借用静态切片；二分查询复杂度为 `O(log 711)`，不改变 `PlanDecoder` 池或其锁的生命周期。

错误路径中的临时 `String` 和转义片段由 `quote_go_bytes` 局部创建并在返回后按 Rust 所有权释放。虽然上层 `DecodePlan` 使用全局 decoder 池，池化与互斥行为定义在 `codec.rs`，本表既不参与同步也不延长 decoder 的借用周期。

## 与 Go 版本的对应关系

Go 同路径 `pkg/util/plancodec/codec.go` 没有对应数据文件。Go 的 `decodePlanInfo` 和 `decodeTaskType` 直接调用标准库 `strconv.Atoi`，并通过 `%v` 把标准库错误写入 plancodec 错误；字符引用形式由 Go 的 `strconv` 实现及其 `IsPrint` Unicode 表决定。

Rust 不能依赖自身 Unicode 版本来复现这一诊断，因此把 Go 1.25/Unicode 15.0.0 的 `strconv.IsPrint` 快照移植为本文件，再由 `go_atoi`/`quote_go_bytes` 模拟 Go 错误。两端成功解析逻辑不依赖本表，差异仅在失败诊断的引用表示。相关 Rust 回归位于独立文件 `pkg/util/plancodec/codec_test.rs`，符合测试与生产源文件分离要求；Go 的一般解码测试位于 `pkg/util/plancodec/codec_test.go`，当前未包含这份 Unicode 版本锁定用例。

## 扩展指南

若升级所对齐的 Go 工具链，应从目标 Go 版本重新枚举 `strconv.IsPrint` 为真的 rune，合并连续码点后整体替换 `GO_PRINTABLE_RANGES`，同时更新文件头的 Go/Unicode 版本说明。不要手工零散追加新区间，也不要改用 Rust `char::is_*` 系列判断；后者会随 Rust Unicode 数据变化，并且分类语义未必与 Go 相同。

修改后至少应在独立的 `pkg/util/plancodec/codec_test.rs` 中同步：旧版本与新版本分界码点、区间首尾内外各一点、ASCII 空格与 DEL、BMP/补充平面转义、非法 UTF-8。若改变 `quote_go_bytes` 的输出，还要核对 `decodePlanInfo` 的完整 `Error::GoMessage` 字节，避免只验证经 `Display` 有损转换后的文本。

兼容性风险高于实现复杂度：诊断文本可能被测试、日志比较或跨语言一致性检查依赖。性能风险主要是区间数量和排序；保持合并区间可维持较小只读体积与对数查找。该文件不是通用引用 API，新增其他引用场景应优先复用或抽取消费函数，并为新调用路径建立独立测试，而不是公开常量。

## 验证依据

- 源码与装配：`pkg/util/plancodec/go_quote_printable.rs`；`pkg/util/plancodec/codec.rs` 中的 `go_atoi`、`include!`、`quote_go_bytes`、`decodePlanInfo`、`decodeTaskType`；`pkg/util/plancodec/lib.rs` 的 codec 模块装配与再导出。
- crate 边界：`pkg/util/plancodec/Cargo.toml`，package 名为 `astersql-util-plancodec`，`[lib] path = "lib.rs"`；其 porting metadata 指向 Go package `pkg/util/plancodec`。
- Go 对照：`pkg/util/plancodec/codec.go` 的 `decodePlanInfo` 与 `decodeTaskType` 均调用 `strconv.Atoi`；错误由 `errors.Errorf(... %v ...)` 传播。
- 独立测试：`pkg/util/plancodec/codec_test.rs::error_quotes_use_go_unicode_version` 锁定 U+31E4 的 Go Unicode 版本行为；`signed_depth_matches_go_atoi_and_panic_boundary` 覆盖相邻的 Atoi/解码边界；`pkg/util/plancodec/codec_test.go` 提供 Go 解码基线。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；查询定位 `GO_PRINTABLE_RANGES` 于本文件第 7 行、`quote_go_bytes` 于 `codec.rs:635`、`go_atoi` 于 `codec.rs:600`，并给出 `decodePlanInfo`、`decodeTaskType` → `go_atoi` 调用边。图未解析本次 `include!` 常量使用，已用 `rg` 对 `codec.rs:633-665` 补证。
- 数据不变量检查：脚本解析出 711 个区间，验证所有端点合法、区间严格递增且不重叠，并确认首尾为 U+0020..U+007E 与 U+E0100..U+E01EF。
- 本包没有 `doc.go`；未运行 Cargo，符合本纯文档任务约束。交付前另执行任务指定的 11 章节结构验证与文档 diff 检查。
