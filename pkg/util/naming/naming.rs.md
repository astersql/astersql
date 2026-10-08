# [`pkg/util/naming/naming.rs`](./naming.rs)

## 文件定位

本文件是 `astersql-util-naming` crate 的核心实现，提供与 Go 包 `pkg/util/naming` 对齐的名称合法性校验。crate 入口 `pkg/util/naming/lib.rs` 公开 `naming` 模块并通过 `pub use naming::*` 再导出本文件的公开函数；`pkg/util/naming/Cargo.toml` 指定 `lib.rs` 为库入口，运行时依赖只有 `regex = "1"`。

当前已确认的生产接线有两处：`pkg/sessionctx/variable/sysvar_builtins.rs` 在注册实例级系统变量 `tidb_service_scope` 时调用 `naming::Check`；`pkg/server/handler/tikvhandler/dxf.rs` 的 `parseStoredTaskHistoryQuery` 对非空历史任务查询 keyspace 调用 `astersql_util_naming::CheckKeyspaceName`。工作区根 `Cargo.toml` 还以 `facade_util_naming` 名称登记该 crate。`cmd/tidb-server/main.rs` 虽也调用 `naming::Check`，但其导入来自 `cmd/tidb-server/stubs.rs` 的同名桩模块，并非本文件，不能作为本实现已接入启动参数路径的证据。

## 核心职责

- `Check` 实施通用名称规则：长度不超过 64，只允许 ASCII 字母、数字、连字符 `-` 和下划线 `_`。
- `CheckKeyspaceName` 复用相同字符集规则，但把最大长度收紧到 20，以符合 keyspace ID/名称写入 KV 后难以扩展编码长度的约束。
- `CheckWithMaxLen` 是两种策略的共同机制：按调用者给出的上限构造全串匹配正则，成功返回 `Ok(())`，失败返回与 Go 版本一致的用户可见错误文本。

规则允许空串，因为量词下界为 0。它不做规范化、大小写折叠、去空白或 Unicode 等价处理；这些都应由调用方在需要时另行完成。

## 主要符号

- `maxKeyspaceNameLength: isize = 20`：模块私有常量，仅供 `CheckKeyspaceName` 选择 keyspace 上限。
- `pub fn Check(name: &str) -> Result<(), String>`：通用入口，固定调用 `CheckWithMaxLen(name, 64)`。在系统变量注册路径中，错误字符串会被转换为 `VariableErrorKind::InvalidValue`。
- `pub fn CheckKeyspaceName(name: &str) -> Result<(), String>`：keyspace 专用入口，固定调用 `CheckWithMaxLen(name, maxKeyspaceNameLength)`。DXF HTTP 查询解析路径会把错误字符串转换为 `DxfError`。
- `pub fn CheckWithMaxLen(name: &str, maxLen: isize) -> Result<(), String>`：公开的参数化入口。它构造 `^[a-zA-Z0-9_-]{0,N}$`，用 `Regex::new(...).expect(...)` 编译，再以 `is_match` 判断整个输入。

这些符号沿用 Go 风格的大写/驼峰命名；crate 根在 `pkg/util/naming/lib.rs` 通过 lint allow 保留这种跨语言 API 形状。

## 执行流程

1. 调用方根据业务语义选择 `Check`、`CheckKeyspaceName` 或直接选择 `CheckWithMaxLen`。
2. 两个策略入口只负责确定上限，随后委托 `CheckWithMaxLen`。
3. `CheckWithMaxLen` 把 `maxLen` 插入带首尾锚点的 ASCII 字符类正则；`{0,maxLen}` 同时表达“允许空串”和“不得超过上限”。
4. 正则在每次调用时编译。若上限形成非法正则，`expect("invalid naming regular expression")` 触发 panic。
5. 匹配成功返回 `Ok(())`；匹配失败返回包含原始输入、上限和允许字符说明的 `Err(String)`。

在 `tidb_service_scope` 系统变量路径中，校验发生在保存规范化值之前；通过后，调用方再将值转成 ASCII 小写并更新进程状态与配置。名称校验本身不参与该状态更新。DXF 路径则在解析分页参数后，仅当 keyspace 非空时执行校验；空 keyspace 被保留为默认查询条件，非空值校验通过后才随分页条件返回。

## 数据与状态

本文件没有可变全局状态、缓存、锁或持久化数据。唯一模块级数据是编译期常量 `maxKeyspaceNameLength`。输入通过借用的 `&str` 传入；成功结果不携带数据，失败时才分配错误 `String`。

字符合法性与长度由正则共同判断。由于允许集合全部是单字节 ASCII，任何非 ASCII 字符都会先因字符集不匹配而失败；对可接受输入而言，字符数与 UTF-8 字节数相同。空串、仅连字符和仅下划线均符合规则。

## 依赖与调用关系

内部调用边由 RustCodeGraph 确认：

- `Check -> CheckWithMaxLen`
- `CheckKeyspaceName -> CheckWithMaxLen`

`CheckWithMaxLen` 的直接外部依赖是 `regex::Regex`，以及标准库的 `format!`、`Result` 和 `String`。图索引未为其报告可识别的函数级 callee，这是宏调用和外部库调用未展开所致，源码仍明确显示正则构造与匹配。

上游直接证据包括：

- `pkg/sessionctx/variable/sysvar_builtins.rs:1645-1650`：`tidb_service_scope` 的 Validation 钩子调用 `naming::Check`。
- `pkg/server/handler/tikvhandler/dxf.rs:437-445`：持久化任务历史查询调用 `CheckKeyspaceName`。
- `pkg/lib_test.rs:45-62`：验证 `crate::util::naming::Check` 能经聚合模块接线调用；这是接线测试，不是生产调用。

依赖声明分别见 `pkg/sessionctx/variable/Cargo.toml` 的别名 `naming` 与 `pkg/server/handler/tikvhandler/Cargo.toml` 的 `astersql-util-naming` 路径依赖。

## 错误处理与边界

- 合法边界：`Check` 接受长度 0 到 64 的允许字符；`CheckKeyspaceName` 接受长度 0 到 20 的允许字符。
- 非法边界：超长、空格、换行、标点 `)`/`!`、中文及其他非 ASCII 字符返回 `Err(String)`。
- 匹配错误会回显未经转义的原始名称。调用方若把错误写入日志或协议响应，需要自行考虑敏感数据与控制字符的呈现方式。
- `CheckWithMaxLen` 接受有符号上限，但没有先行参数校验。负数会形成非法重复量词，并因 `expect` panic；`migration_negative_max_len_panics_like_go_must_compile` 将此锁定为对齐 Go `regexp.MustCompile` 的行为。其他超出正则引擎可接受范围的上限也可能走同一 panic 边界，因此不应把未经约束的用户整数直接传给该函数。
- 每次调用都重新编译正则，不会返回正则编译错误。固定入口的模式由代码内常量保证有效。

## 并发与资源生命周期

三个函数都是无状态同步函数，不创建线程、任务、通道、事务或 I/O 资源，也不持有跨调用借用。每次调用独立创建模式字符串与 `Regex`，函数返回时释放；因此不存在共享状态竞态，可由多个线程并发调用。

代价是每次校验都有正则构造与编译开销。当前逻辑优先保持 Go 实现形状；若未来在高频路径缓存正则，需要分别缓存固定上限模式，并为任意 `maxLen` 设计有界缓存或改用直接字符扫描，避免引入无界全局状态。

## 与 Go 版本的对应关系

`pkg/util/naming/naming.go` 是逐符号对照来源：Go 的 `maxKeyspaceNameLength`、`Check`、`CheckKeyspaceName`、`CheckWithMaxLen` 与 Rust 版本一一对应，上限 20/64、正则文本、全串匹配和错误文案保持一致。Go 使用 `fmt.Sprintf` 与 `regexp.MustCompile`，Rust 使用 `format!` 与 `Regex::new(...).expect(...)`；两边对非法生成模式都采用 panic，而不是普通错误返回。

返回类型是语言层面的差异：Go 返回 `error`/`nil`，Rust 返回 `Result<(), String>`。Rust 没有定义专用错误类型，因此调用方按所在子系统映射为 `VariableError` 或 `DxfError`。`pkg/util/naming/naming_test.go::TestScope` 与 `pkg/util/naming/naming_test.rs::test_scope` 按相同顺序覆盖基本成功/失败用例；Rust 额外的 `migration_aster_unit_test.rs` 补充了 64/20 精确边界、Unicode、错误文本和负上限 panic。

## 扩展指南

- 修改允许字符、空串策略或通用上限时，集中修改 `CheckWithMaxLen`/`Check`，并同步核对系统变量与 Go API 的兼容性；字符集变化可能影响配置文件、SQL 系统变量和外部 HTTP 参数。
- 修改 keyspace 上限时，修改 `maxKeyspaceNameLength`，同时评估 KV 编码与集群兼容性，不能只放宽正则。
- 新增另一类固定策略时，优先增加薄入口并委托 `CheckWithMaxLen`，避免复制错误文案；若需要不同字符集，则应明确拆分机制而非让现有 API 出现隐式分支。
- 性能优化应先证明正则编译是热点；缓存或直接扫描必须保持锚定、空串、ASCII 集合、错误文本和 panic 契约。
- 测试必须继续放在独立文件。基础 Go 对照用例同步更新 `pkg/util/naming/naming_test.rs`，迁移边界和错误契约更新 `pkg/util/naming/migration_aster_unit_test.rs`，并按需要同步 `pkg/util/naming/naming_test.go`。生产调用变化还应在各调用方自己的独立测试中覆盖。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录中的 `naming.rs`、`lib.rs`、两个 Rust 测试及 Go 对照均已索引。
- RustCodeGraph `node --file`：读取 `pkg/util/naming/naming.rs`、`lib.rs`、`naming_test.rs`、`migration_aster_unit_test.rs`、`naming.go`、`naming_test.go`，以及两个生产调用点和聚合接线测试。
- RustCodeGraph `query/node/callers/callees`：确认三个 Rust/Go 同名符号；确认 Rust 内部的两条委托调用边，且 `CheckWithMaxLen` 没有图中展开的 callee。重名符号的 `callers` 命令未输出上游明细，因此另以精确 `rg` 搜索核对生产引用。
- 配置证据：`pkg/util/naming/Cargo.toml`、`pkg/sessionctx/variable/Cargo.toml`、`pkg/server/handler/tikvhandler/Cargo.toml`、根 `Cargo.toml`。
- 行为测试证据：`pkg/util/naming/naming_test.rs`、`pkg/util/naming/migration_aster_unit_test.rs` 和 `pkg/util/naming/naming_test.go`。本任务是纯文档分析，按计划未运行 Cargo 或代码测试。
