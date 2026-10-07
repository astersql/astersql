# `pkg/errors/normalize.rs`

## 文件定位

该文件是 `astersql-errors` crate 中“规范化错误”实现，源码入口为 [`normalize.rs`](normalize.rs)。它把可复用的错误原型表示为 `Error`，统一保存 MySQL 数值码、RFC 文本码、消息模板、格式参数、可选 cause 和生成位置，并提供按进程级开关脱敏及 JSON 兼容能力。`pkg/errors/mod.rs` 将这里的公开类型、常量和函数重新导出，因此业务 crate 通常从 `astersql_errors` 直接使用 `Normalize`、`ErrorEqual` 等 API，而不直接引用私有模块 `normalize`。

`pkg/errors/Cargo.toml` 定义 crate 名为 `astersql-errors`、库入口为 `mod.rs`；本文件直接使用 `serde` 做错误 JSON 编解码，并通过同 crate 的 `core`、`wrap`、`stack` 能力完成消息格式化、共享错误链和堆栈处理。工作区根 `Cargo.toml` 以 `facade_errors` 指向该 crate，多个 SQL、执行、存储、BR 等子 crate 也以路径依赖接入，因此这是跨子系统的基础错误协议，不属于某一条具体 SQL 执行链。

## 核心职责

1. `Normalize` 根据消息模板和 `NormalizeOption` 构造尚未绑定运行时参数的 `Error` 原型；同类错误以 `ID()`（文本码优先，否则数值码字符串）作为稳定身份。
2. `Error::generate` 集中实现六个公开生成入口：决定是否替换模板、是否采用 cause 文本、是否脱敏参数、是否捕获调用位置和堆栈，最终返回 `SharedError`。
3. `AtomicRedactLogState`、`RedactLogEnabled` 与 `RedactErrorArg` 提供线程安全的 OFF/ON/MARKER 三态脱敏：ON 用 `?` 替换，MARKER 用 `ErrorArg::Redacted` 延迟保留格式语义并转义标记字符。
4. `Wrap`/`Unwrap`/`Cause`、`StdError::source`、`Equal`/`Is`/`ErrorEqual` 把规范化身份接入 crate 自身 cause 链与 Rust 标准错误链。
5. 自定义 `Display`/`Debug` 和 `Serialize`/`Deserialize` 对齐 Go 错误文本、详细格式与 `class/code/message/rfccode` JSON 协议，并兼容仅含旧 `class` 字段的记录。

## 主要符号

- `RedactLogDisable`、`RedactLogEnable`、`RedactLogMarker`：字符串常量 `OFF`、`ON`、`MARKER`，与 Go 配置值保持一致。
- `AtomicRedactLogState(AtomicU8)`：把三态编码为 `0/1/2`；`Store` 将未知字符串降级为 OFF，`Load` 将未知内部值也解释为 OFF。两者使用 `Ordering::SeqCst`。
- `RedactLogEnabled`：进程级静态脱敏状态。任何修改都会影响同期调用 `GenWithStackByArgs`、`FastGenByArgs` 或直接调用 `RedactErrorArg` 的线程。
- `HackedStr::FreezeStr`：为可能别名可变底层存储的字符串提供稳定快照。具体转换由 `ErrorArg::from_hacked` 承担，本文件在生成时克隆 `ErrorArg`。
- `ErrCode = i32`、`ErrCodeText = String`、`ErrorID = String`、`RFCErrorCode = String`：数值码和文本身份的公开别名。
- `NormalizeOption::{RedactArgs, RFCCodeText, MySQLErrorCode}` 及同名构造函数：配置需脱敏的参数位置、RFC 文本码与 MySQL 数值码。选项按给定顺序应用，同类选项后者覆盖前者。
- `Error`：规范化错误原型/实例。字段均为私有；读取通过 `Code`、`RFCCode`、`ID`、`Location`、`MessageTemplate`、`Args`、`GetMsg` 和 `GetSelfMsg`。
- `Error::{GenWithStack, GenWithStackByArgs, FastGen, FastGenByArgs, FastGenWithCause, GenWithStackByCause}`：六个运行时生成入口。`ByArgs` 路径使用原型模板并脱敏；自定义格式和 cause 消息路径不调用 `RedactErrorArg`，与 Go 当前行为一致。
- `Error::{Wrap, Unwrap, Cause}`：`Wrap` 克隆原型后挂载直接 cause，传入 `None` 返回 `None`；`Unwrap` 返回直接 cause；`Cause` 对直接 cause 仅尝试一次 `NextCause`，不是递归求根因。
- `Error::{Equal, NotEqual, Is}`、`ErrorEqual`、`ErrorNotEqual`：原型比较使用规范化 ID；模块级比较先取双方根因，再按空值、指针、规范化 ID、最终展示文本的顺序判断。
- `error_cause`、`error_message`：crate 内部下钻辅助函数，分别从 `SharedError` 中提取规范化错误的直接 cause 或格式化消息。
- `class_for_component`、`component_for_class`：固定维护 1 至 27 的 TiDB 错误 class 与组件名前缀双向映射，服务于 JSON 兼容。

## 执行流程

定义错误时，调用方先用 `RFCCodeText`、`MySQLErrorCode`、`RedactArgs` 生成选项，再把消息模板与选项切片传给 `Normalize`。`Normalize` 先创建零码、空参数、无 cause、无位置的原型，然后依次应用选项；例如重复传入两个 `RFCCodeText` 时，以后出现的值为准。

抛出具体错误时，六个公开入口汇入 `Error::generate`：

1. 克隆原型，避免修改可复用的静态/共享定义。
2. 若传入自定义 `format`，以它覆盖模板；否则在 `use_cause_message` 为真且原型已有 cause 时，将 cause 的展示文本作为模板。
3. 克隆调用参数；仅 `redact` 为真时，按 `redact_args_pos` 与当前 `RedactLogEnabled` 改写指定位置。越界位置通过 `get_mut` 安全忽略。
4. 带栈路径使用 `#[track_caller]` 的 `Location::caller()` 写入真实外部调用文件与行号，再调用 `AddStack`；快速路径调用 `SuspendStack`，不填写位置也不捕获栈。
5. 两条路径均从确定存在的 `Some(SharedError)` 中取值；源码中的 `expect` 依赖 `AddStack`/`SuspendStack` 对非空输入不返回空这一不变量。

展示时，`GetMsg` 在无参数时原样返回模板，有参数时委托 `core::format_message`。`Display` 输出 `[RFCCode]消息`，有 cause 时追加 `: cause`；交替 `Debug`（`{:#?}`）先展开 cause 的详细格式，再换行附加本层规范化消息。序列化根据文本码冒号前的组件名计算 `class`，写出四个 Go 兼容字段；反序列化若 `rfccode` 为空且 `class > 0`，尝试重建 `组件名:数值码`，随后清空参数、cause 与位置。

## 数据与状态

`Error` 同时承担“原型”和“已生成实例”角色。原型通常只有 `code`、`code_text`、`message`、`redact_args_pos`；生成实例通过克隆继承这些定义，再补充 `args`、位置及可能已有的 cause。`Clone` 保证生成操作不回写原型，但 `SharedError` 中的 cause 是共享克隆，因此保留底层错误身份。

身份不等同于完整内容：`PartialEq`/`Eq`、`Is` 以及两个规范化错误之间的 `ErrorEqual` 都只比较 `ID()`；消息、数值码（文本码存在时）、参数、cause 和位置均不参与。没有文本码时，数值码的十进制字符串成为 ID，所以两个默认数值码为 0 的原型会被视为同类，调用方应为需要区分的公开错误设置稳定码。

唯一可变全局状态是 `RedactLogEnabled`。错误实例自身没有内部可变性；参数与 cause 在生成时克隆。MARKER 状态不立即把任意参数强制转成字符串，而是包装为 `ErrorArg::Redacted`，由格式化层保留 `%d` 等类型化格式，并对已有 `‹`、`›` 做转义；相关行为由 `pkg/errors/tests/normalize_generation_test.rs` 验证。

## 依赖与调用关系

向下依赖如下：

- `super::ErrorArg` 与 `super::core::format_message`：保存并执行 Go 风格格式参数。
- `super::SharedError`：提供可克隆、可 downcast、可做 `ptr_eq` 的共享错误值。
- `super::{AddStack, SuspendStack}`：分别附加真实堆栈或无栈标记。
- `super::{Cause as RootCause, Unwrap as NextCause}`：前者用于相等比较时取根因，后者用于 `Error::Cause` 的单层下钻。
- `std::{error, fmt, panic::Location, sync::atomic}`：标准错误链、文本格式、调用点和全局并发状态。
- `serde`：Go 兼容 JSON schema；`serde_json` 仅为 dev-dependency，由独立测试使用。

向上调用来自 `pkg/errors/mod.rs` 的公开再导出以及大量工作区 crate。RustCodeGraph 的文件查询显示 `normalize.rs` 被 85 个文件使用；直接、可复核的代表包括 `pkg/parser/terror/terror.rs` 用 `errors::Normalize` 定义 terror，`lightning/pkg/importer/dup_detect.rs` 构造带 RFC 码的 Lightning 错误，`pkg/types/internal/core_time/lib.rs` 构造类型错误，`pkg/types/string.rs` 为具体字符串类型实现 `astersql_errors::HackedStr`。根 `Cargo.toml` 与各子 crate 的路径依赖说明该模块作为通用错误门面进入 SQL、存储、导入恢复等链路。

RustCodeGraph 对全局同名 `Normalize` 的宽查询会同时命中 SQL 规范化等无关符号；本次以目标文件节点、文件级使用关系及上述直接源码引用交叉消歧。精确 callers/callees 命令在本次查询时限内未返回结果，因此没有把不完整的图边当作穷尽调用清单。

## 错误处理与边界

- `RedactErrorArg` 对越界下标静默忽略，对重复下标会重复包装；ON 下重复替换仍是 `?`，MARKER 下重复位置可能形成嵌套标记，因此选项提供者应保证位置唯一。
- `AtomicRedactLogState::Store` 对任何非 ON/MARKER 值使用 OFF，配置拼写错误不会报错，而会关闭脱敏。安全敏感的配置入口应在调用前自行校验。
- `Error::Wrap(None)` 返回 `None`，不会生成没有 cause 的包装实例；`Cause` 只剥直接 cause 的一层包装，这一语义由 `pkg/errors/normalize_test.rs::cause_unwraps_only_one_attached_layer` 固定。
- `ErrorEqual` 对非规范化根因退化为展示文本相等，两个类型和语义不同但文本相同的错误可能被判等；规范化错误则以 ID 判等。
- JSON 不保存 `redact_args_pos`、args、cause、file、line；反序列化得到的是可展示/识别的扁平错误，不是原错误链或可完整复用的原型。未知 class 无法反推组件时保持空文本码，ID 随后退化为数值码字符串。
- `component_for_class` 是封闭映射。新增组件若未同步此表，序列化的 `class` 为 0，旧 class-only 数据也无法恢复该组件文本码。
- `generate` 中的两个 `expect` 不是面向用户的错误分支；其安全性建立在向堆栈函数传入 `Some` 后必得 `Some` 的 crate 内契约上。若未来修改 `AddStack`/`SuspendStack`，必须同步审查这里。

## 并发与资源生命周期

`RedactLogEnabled` 使用 `AtomicU8` 和最强的 `SeqCst` 顺序，读写没有锁、借用或任务生命周期问题；一次 `RedactErrorArg` 先读取一次模式，因此单次调用不会在参数之间混用两个模式。不过全局状态会跨线程、跨测试共享：独立测试用静态 `Mutex` 串行化脱敏用例，并以 RAII guard 在退出时恢复 OFF。生产侧临时切换也应采用等价的恢复策略，避免污染其他请求。

错误生成不启动线程、异步任务或通道，也不持有文件、网络、事务资源。带栈实例的资源成本来自调用点记录和 `AddStack` 捕获；`FastGen*` 通过 `SuspendStack` 明确选择较低成本的无栈路径。参数和 `Error` 被拥有式克隆，cause 通过 `SharedError` 共享；因此生成实例可离开调用栈继续存在。对别名可变缓冲的字符串，应先经 `HackedStr`/`ErrorArg::from_hacked` 冻结，以免延迟格式化观察到后来写入。

## 与 Go 版本的对应关系

仓库 `go.mod` 锁定 `github.com/pingcap/errors v0.11.5-0.20260508054701-306e305bcf41`；其模块缓存中的 `normalize.go` 和 `normalize_test.go` 是本次直接对照。Rust 保留了 Go 的公开命名和总体语义：三态脱敏、错误原型、六条生成路径、ID 判等、单层 `Cause`、标准 unwrap、文本格式、JSON 字段及 class 映射。

主要语言适配和差异如下：

- Go 的 `NormalizeOption` 是修改 `*Error` 的闭包，Rust 用可枚举、可克隆的 `NormalizeOption`；两者都顺序应用，Rust API 接收切片。
- Go 的 `atomic.String` 对应 Rust 的 `AtomicU8` 编码；Rust 未知输入明确归一为 OFF。
- Go 用 `runtime.Caller`，Rust 用 `#[track_caller]`/`Location::caller`；快速路径都不记录位置。Go 无法取位置时写 `<unknown>/-1`，Rust 当前没有该失败分支。
- Go 的 `error`/`fmt.Formatter` 对应 Rust 的 `StdError`、`Display` 和 `Debug`；Rust 的交替 Debug 承担 Go `%+v` 的 cause 展开语义。
- Go 的参数是 `[]interface{}`，Rust 使用受控的 `ErrorArg`；MARKER 的 `redactFormatter` 对应 `ErrorArg::Redacted`。Rust 测试确认数字格式与标记字符转义语义得以保留。
- Go 指针/`nil` 语义由 Rust 的 `Error` 值、`SharedError` 和 `Option` 表达。Rust `Error::Equal` 没有 Go 中“根因就是原型同一指针”的单独分支，但规范化错误仍按 ID 判等；模块级 `ErrorEqual` 保留 `ptr_eq` 快路径。
- Go 自定义 JSON 方法与 Rust serde 实现都只编码 `class/code/message/rfccode`。Rust 独立测试额外明确反序列化后没有 cause。

## 扩展指南

新增一种规范化选项时，应同时修改 `NormalizeOption`、新增公开构造函数、补充 `Normalize` 的应用分支，并在 `pkg/errors/mod.rs` 再导出；测试应放在独立的 `pkg/errors/tests/normalize_identity_test.rs` 或新建同目录独立测试文件，不要把测试逻辑继续加入生产文件。若该选项对应 Go API，还需逐项核对锁定版本的 `normalize.go`，避免仅为 Rust 增加不兼容行为。

新增错误 class/组件时，必须成对更新 `component_for_class` 与由其反查的 `class_for_component` 语义，并扩展 `normalize_identity_test.rs` 的当前 schema 和旧 class-only schema 用例。需要评估已有持久 JSON、日志消费端及 RFC 码兼容性，不能重用或重编号既有 class。

调整生成行为时，优先修改唯一汇合点 `Error::generate`，并在 `pkg/errors/tests/normalize_generation_test.rs` 同步覆盖六个公开入口、带栈/无栈、位置、三种脱敏状态、cause 模板和 `HackedStr` 快照。改变相等性或错误链时，同步扩展 `normalize_identity_test.rs`、`std_interop_test.rs` 与 `normalize_test.rs` 的单层 Cause 回归。性能敏感路径应保留 `FastGen*` 不捕获堆栈的性质；安全敏感改动要特别检查全局模式竞态、越界/重复脱敏位置和 MARKER 格式转义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录中 `normalize.rs` 有 67 个符号；`node --file pkg/errors/normalize.rs` 完整读取 1–489 行；`explore` 识别 `Normalize` 的目标文件调用面及测试覆盖，并显示该文件被 85 个文件使用。精确 callers/callees 因同名符号消歧查询在时限内未返回，已用局部源码引用补证。
- 生产源码：`pkg/errors/normalize.rs`；模块边界与公开再导出：`pkg/errors/mod.rs`；crate 声明与依赖：`pkg/errors/Cargo.toml`；工作区接线：根 `Cargo.toml` 及各消费 crate 的路径依赖。
- Rust 测试：`pkg/errors/normalize_test.rs`（Cause 只下钻一层）、`pkg/errors/tests/normalize_generation_test.rs`（六条生成路径、堆栈、三态脱敏、cause、冻结字符串）、`pkg/errors/tests/normalize_identity_test.rs`（码、ID、相等、包装、JSON）、`pkg/errors/tests/std_interop_test.rs`（标准 source/downcast 与栈标记）、`pkg/errors/tests/api_parity_test.rs`（公开 API 面）。
- Go 对照：`go.mod` 锁定版本在本机模块缓存中的 `github.com/pingcap/errors@v0.11.5-0.20260508054701-306e305bcf41/normalize.go` 与 `normalize_test.go`。仓库 `pkg/errors` 下没有同路径 Go 源文件或 `doc.go`。
- 调用样本：`pkg/parser/terror/terror.rs`、`lightning/pkg/importer/dup_detect.rs`、`pkg/types/internal/core_time/lib.rs`、`pkg/types/string.rs`。这些样本分别验证原型定义、跨工具复用和 `HackedStr` 实现入口，不代表穷尽列表。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工检查仅新增本说明、链接指向真实文件、重要结论均可由上述符号或测试复核。
