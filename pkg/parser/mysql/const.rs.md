# `pkg/parser/mysql/const.rs`

## 文件定位

`const.rs` 位于独立 crate `astersql-parser-mysql`（`pkg/parser/mysql/Cargo.toml`）中，由 `pkg/parser/mysql/lib.rs` 以 `pub mod r#const` 公开。它集中定义 MySQL/TiDB 协议常量、会话 `sql_mode` 位集合、服务端版本串与语句优先级转换，是协议层和 SQL 兼容层共用的“常量与轻量转换”边界，不负责网络收发、SQL 解析、认证执行或系统表访问。

上层 `astersql-parser` 在 `pkg/parser/lib.rs::mysql` 中只选择性再导出 `DefaultSQLMode`、`GetSQLMode`、`SQLMode` 及若干解析器关心的 mode 位；其他消费者可直接使用 `astersql_parser_mysql::r#const`。RustCodeGraph 的文件节点显示该文件被 82 个文件使用，调用面横跨 session、server、expression、dumpling 和测试代码，因此协议数值与字符串字面量属于跨模块兼容契约。

## 核心职责

- 固定 MySQL wire protocol 常量：packet 头字节、server status 位、`COM_*` 命令编号、client capability 位、游标与压缩类型，并用 `Command2Str` 提供诊断展示名。
- 固定 SQL 层共享元数据：标识符/字段长度上限、认证插件名、系统库表名、类型物理长度、时间小数存储长度、默认认证插件顺序和分区上限。
- 用 `SQLMode(i64)` 表示 MySQL `sql_mode` 位集合，提供查询、置位、清位、组合模式展开、字符串规范化与严格解析。
- 生成 classic/next-gen 对外版本字符串，校验 `vYY.MM.PATCH[-pre]` 并转换为 `CLOUD.YYYYMM.PATCH[-pre]`。
- 在配置字符串、内部 `PriorityEnum` 与恢复关键字之间转换语句优先级。

该文件没有 trait、异步函数或条件编译项。主要行为都是纯计算；唯一可变全局是 `TiDBReleaseVersion`。

## 主要符号

- 版本 API：`ServerVersion() -> String` 读取 `TiDBReleaseVersion` 并拼成 `8.0.11-TiDB-<release>`；`NormalizeTiDBReleaseVersionForNextGen` 仅把 classic 默认占位值改为 next-gen 占位值；`BuildTiDBXReleaseVersion` 校验并转换 release；`BuildTiDBXServerVersion` 再加 MySQL 兼容前缀。`VersionSeparator` 是 PD 解析版本所依赖的固定 `"-TiDB-"`。
- 协议常量：`OKHeader`/`ErrHeader`/`EOFHeader`/`LocalInFileHeader`，`ServerStatus*`，连续的 `ComSleep..ComEnd`，`ClientLongPassword..ClientZstdCompressionAlgorithm`，以及 `CursorType*`、`Compression*`。`HasCursorExistsFlag` 是 server status 的位测试。
- 查表数据：`Command2Str`、`DefaultLengthOfMysqlTypes`、`DefaultLengthOfTimeFraction`、`DefaultAuthPlugins` 都是只读切片。它们保留确定顺序，但不像 Go `map` 那样提供键索引；调用者通常迭代并查找。
- `SQLMode(pub i64)`：可复制的位集合；私有 `has` 实现“指定 flag 的全部位均存在”，公开 `HasStrictMode` 等查询方法表达解析和执行所需语义。`SetSQLMode` 使用按位或，`DelSQLMode` 使用按位与非。
- `ModeRealAsFloat..ModeAllowInvalidDates`：按 Go `iota` 顺序固定在 bit 0..32；`Str2SQLMode` 将单项大写名称映射为位，`CombinationSQLMode` 保存 `ANSI`、`TRADITIONAL` 等组合模式的展开序列。
- `FormatSQLModeStr`：大写、只裁掉字符串末尾的 ASCII 空格、忽略空段、按首次出现顺序展开组合模式并去重。`GetSQLMode` 假定输入已经规范化，把逗号分隔项折叠为位集合，遇到未知非空项返回 `SQLError`。
- `PriorityEnum(pub i32)` 与 `NoPriority`、`LowPriority`、`HighPriority`、`DelayedPriority`：`Str2Priority` 不区分大小写，未知值降为 `NoPriority`；`Restore` 将合法值变为 SQL 关键字，非法整数返回错误。

## 执行流程

SQL mode 的标准使用链是：调用方先将外部字符串交给 `FormatSQLModeStr`；函数将尾部空格裁掉并整体大写，逐段查 `CombinationSQLMode`，先追加组合成员、再追加组合名本身，并用 `HashSet` 保证首次出现顺序下的去重；随后 `GetSQLMode` 在 `Str2SQLMode` 中逐项查值，以 `SetSQLMode` 累加。未知非空名称立即终止并构造 `ErrWrongValueForVar`。例如 `pkg/session/runtime/control.rs` 的 `sql_mode` 设置分支先规范化、再校验，成功后才把规范字符串写回 session state；`dumpling/export/schema_projection.rs` 和 `pkg/expression/exprstatic/evalctx.rs` 也采用同一双阶段链路。

版本链分两类。classic 的 `ServerVersion` 直接读取发布版本并拼接兼容版本与固定分隔符；next-gen 启动路径 `cmd/tidb-server/main.rs::deriveRuntimeVersionsFromBuildInfo` 先调用 `NormalizeTiDBReleaseVersionForNextGen`，再调用 `BuildTiDBXServerVersion`。后者委托 `BuildTiDBXReleaseVersion`：要求小写 `v` 前缀，使用 `semver::Version` 解析余串，把两位 major 加 2000 得到年份，约束年份为 2025..=2099、月份为 1..=12，保留 patch 和可选 prerelease，最后拼出 CLOUD 格式。

优先级链由配置进入：`cmd/tidb-server/main.rs` 用 `Str2Priority` 解析 `ForcePriority`，把整数写入原子配置，并用 `Priority2Str` 的调用侧适配函数镜像到 SQL 系统变量。AST/SQL 恢复需要关键字时，`PriorityEnum::Restore` 返回空串或相应优先级关键字。

## 数据与状态

绝大多数数据是编译期常量或不可变 `'static` 切片，可以由所有线程共享。协议位和编号是外部兼容数据而非可自由重排的内部枚举：`Com*`、`Client*`、`Mode*` 的数值位置，`VersionSeparator` 的字面量，以及认证插件和系统表名称都必须保持与消费者及 Go 行为一致。`DefaultAuthPlugins` 与组合模式成员的顺序也有可观察的展示/规范化效果。

`SQLMode` 是一个可复制的 `i64` newtype，没有堆资源；组合模式既保留组合位（如 `ModeANSI`），也展开其子位，因为 `FormatSQLModeStr` 会把组合成员和组合名都放入结果。`FormatSQLModeStr` 每次调用分配结果 `Vec`、去重 `HashSet` 和输出 `String`；其静态映射以线性 `iter().find` 查找，数据规模固定且较小。

`TiDBReleaseVersion` 是 `pub static mut &str`，默认指向 classic 占位字符串。`ServerVersion` 为函数而非缓存值，所以每次读取当前全局值；该读取位于 `unsafe` 块。文件本身不提供同步写入 API、原子性或初始化协议，调用方必须保证对该全局的写入与并发读取不会发生数据竞争。

## 依赖与调用关系

- crate 边界：`pkg/parser/mysql/Cargo.toml` 声明库入口 `lib.rs`；本文件直接使用标准库 `HashSet`、外部 `semver::Version`、同 crate 的 `errcode`、`error::{NewErr, NewErrf, SQLError}` 和 `type` 中的 MySQL 类型编号。`astersql-errors` 由错误模块间接承接共享错误基础设施。
- 上游会话链：`pkg/session/runtime/control.rs` 对 `sql_mode` 做格式化、合法性检查和状态写回；多个 session 执行分支再用 `GetSQLMode` 读取保存的 mode，决定严格模式、ANSI_QUOTES、DDL 校验等行为。
- 上游解析/表达式链：`pkg/parser/yy_parser.rs` 用默认 mode 初始化解析器；`pkg/expression/exprstatic/evalctx.rs` 把系统变量转换为求值上下文中的 `SQLMode`；`pkg/parser/lib.rs` 再导出解析器需要的 mode API。
- 上游版本链：`cmd/tidb-server/main.rs::deriveRuntimeVersionsFromBuildInfo` 和 `pkg/util/printer/printer.rs` 消费 next-gen 版本构造函数。
- 上游诊断与元数据链：`pkg/server/server.rs`、`pkg/server/runtime.rs` 和 `pkg/session/sessmgr/processinfo.rs` 从 `Command2Str` 取命令显示名；`pkg/session/runtime/ddl_index_validation.rs`、`pkg/testkit/testutil/handle.rs` 查询类型默认长度。

RustCodeGraph 的 `status` 与分段 `node` 确认源码和文件级消费者，但精确 `callers/callees` 对这些同名函数未返回边；以上调用边由限定 `.rs` 文件的直接引用搜索和对应源码片段补证，不把模糊的同名图结果当作精确调用关系。

## 错误处理与边界

`GetSQLMode` 对空字符串和空段容忍，但不主动大写或清理每个逗号段；文档契约和 Go 实现都要求先调用 `FormatSQLModeStr`。该格式化函数只删除整个输入末尾的空格，不删除逗号后或段首空格，因此 `"NO_ZERO_DATE, NO_ZERO_IN_DATE"` 会保留第二段前导空格并在解析时失败。未知项通过 `newInvalidModeErr` 产生 `ErrWrongValueForVar(sql_mode, value)`。

`BuildTiDBXReleaseVersion` 将三类失败统一为 `SQLError`：缺少小写 `v`、semver 解析失败、年份或月份越界；`BuildTiDBXServerVersion` 用 `?` 原样传播。它接受 semver prerelease 并保留前导连字符；当前输出不带 build metadata。年份来自 `2000 + major`，因此调用者不能把普通 TiDB 大版本直接当作任意四位年份。

`Str2Priority` 对包括 `NO_PRIORITY` 在内的非三种显式关键字都返回 `NoPriority`，属于宽松配置解析；相反，`PriorityEnum::Restore` 会拒绝通过公开字段构造出的未知整数。`HasCursorExistsFlag` 只检查目标位，其他 server status 位不影响结果。

## 并发与资源生命周期

常量、字符串切片和映射切片都是进程级只读数据，无锁、无任务、无通道、无 I/O、无事务资源，也没有显式清理阶段。`SQLMode`、`PriorityEnum` 和状态位按值传递；格式化和版本转换产生的 `String`/`Vec`/`HashSet` 由单次调用拥有，返回或离开作用域后按 Rust 所有权规则释放。

并发上的唯一例外是 `TiDBReleaseVersion`：`static mut` 的读写同步责任不在本文件中。安全扩展不应新增无同步的可变全局；若要支持运行时变更，应先设计一次性初始化或同步容器，并审计所有 `unsafe` 读取。当前其他函数均不保留跨调用状态，可并发调用。

## 与 Go 版本的对应关系

直接对照是 `pkg/parser/mysql/const.go`，Rust 保留了协议数值、SQL mode 位序、组合展开表、版本校验和优先级语义。`pkg/parser/mysql/const_test.rs` 对照 `const_test.go`，逐项锁定 bit 0..31、`VersionSeparator`、CLOUD 版本成功/失败样例和占位版本改写；`ModeAllowInvalidDates` 位于新增的 bit 32，源码与 Go 定义保持同序。

两边存在实现形状差异：Go 的 `ServerVersion` 是包初始化时计算的变量，Rust 是每次读取 `TiDBReleaseVersion` 的函数；Go 的 maps 在 Rust 中多为有序切片并通过线性查找；Go `PriorityEnum.Restore` 写入 `format.RestoreCtx`，Rust 为避免把完整 format crate 引入该轻量 crate，直接返回 `&'static str`；Go 用 `errors.Errorf`，Rust 统一构造 `SQLError`。这些差异不改变已覆盖的外部字符串与位值语义，但 `ServerVersion` 的动态读取和 `static mut` 安全责任是 Rust 特有边界。

相关 Rust 边界测试不只在同目录：`pkg/types/const_test.rs` 覆盖合法/非法 SQL mode 字符串、mode 查询方法和游标状态位；`pkg/parser/mysql/charset_1_aster_unit_test.rs` 与 `unit_test.rs` 还覆盖组合展开、未知 mode、prerelease 和版本错误。它们提供迁移语义证据，但本任务未运行 Cargo，未把测试结果误报为本轮执行证据。

## 扩展指南

修改协议常量时，应先确认 MySQL 协议编号和 Go `pkg/parser/mysql/const.go`，再同步所有以数组/位位置为契约的消费者；不得重排 `Com*`、`Client*` 或 `Mode*`。增加 SQL mode 时至少同步 mode 位、`Str2SQLMode`、必要的 `CombinationSQLMode` 成员和查询方法；若 mode 影响词法/语法或执行，还要审计 parser、expression、session 的读取点。

调整格式化逻辑要明确是否仍保留 Go 的“只裁尾空格、不裁每段空格”和“组合成员在组合名之前”的行为；这会影响系统变量拒绝边界和规范字符串。版本规则变化必须同步年份/月范围、占位值、server 拼接及 printer/启动路径，并在独立的 `pkg/parser/mysql/const_test.rs` 中补成功、边界和错误样例。协议/SQL mode 边界可同步扩展 `pkg/types/const_test.rs`，测试不要内嵌到生产文件。

`PriorityEnum` 若增加成员，要同步常量、`Priority2Str`、`Str2Priority`、`Restore` 及配置镜像测试。性能风险主要来自热路径新增线性表扫描或额外分配；兼容风险则集中在公开数值、字符串大小写/空白规则、错误码和展示顺序。任何涉及 `TiDBReleaseVersion` 的扩展还必须处理 `static mut` 的并发安全，而不能仅增加另一个 `unsafe` 访问点。

## 验证依据

- 目标源码：RustCodeGraph `node --file pkg/parser/mysql/const.rs` 分段读取全部 712 行，核对了 imports、版本函数、协议常量、SQL mode 位/表/方法、优先级转换及末尾压缩常量；文件节点报告 82 个消费者。
- crate 与模块：`pkg/parser/mysql/Cargo.toml`（`astersql-parser-mysql`、`semver = "1"`、库入口 `lib.rs`），`pkg/parser/mysql/lib.rs`（公开 `r#const`），`pkg/parser/lib.rs::mysql`（选择性再导出）。该目录没有 `doc.go`，因此包契约以 Rust manifest 和库入口为准。
- 直接调用证据：`pkg/session/runtime/control.rs` 的 `sql_mode` 写入链；`pkg/expression/exprstatic/evalctx.rs`、`dumpling/export/schema_projection.rs` 的格式化/解析链；`cmd/tidb-server/main.rs::deriveRuntimeVersionsFromBuildInfo` 的版本链；`pkg/server/server.rs`、`pkg/server/runtime.rs`、`pkg/session/sessmgr/processinfo.rs` 的命令展示引用。
- Go 对照：`pkg/parser/mysql/const.go` 与 `pkg/parser/mysql/const_test.go`，核对协议数值、SQL mode、组合模式、版本转换和优先级恢复语义。
- 独立 Rust 测试：`pkg/parser/mysql/const_test.rs`、`pkg/types/const_test.rs`、`pkg/parser/mysql/charset_1_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`。本任务按计划是纯文档分析，未运行 Cargo。
- RustCodeGraph：运行 `status`、目标 `explore`、目标文件分段 `node` 以及 `query GetSQLMode`、`query FormatSQLModeStr`、`query BuildTiDBXReleaseVersion`、`query SQLMode`；索引有效。精确 callers/callees 未产出边，故按技能允许的回退使用 `rg` 限定 Rust 调用点补证。
- 交付验证：使用任务指定命令确认目标文件存在且恰有 11 个固定二级标题，并人工复核只有该说明文档与任务文件删除属于本会话变更。
