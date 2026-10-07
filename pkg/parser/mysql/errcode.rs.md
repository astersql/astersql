# `pkg/parser/mysql/errcode.rs`

## 文件定位

本文件属于 `astersql-parser-mysql` crate，是 MySQL 协议兼容层的数字错误码目录。crate 入口 `pkg/parser/mysql/lib.rs` 以 `pub mod errcode` 暴露它，`pkg/parser/lib.rs` 又在 `mysql` 模块中执行 `pub use parser_mysql::errcode::*`，因此解析器及其使用方可以通过 `parser::mysql::ErrParse` 一类路径取得这些常量。它只声明协议编号，不负责生成消息、选择 SQLSTATE、格式化错误或发送协议包；这些职责分别位于同目录的 `errname.rs`、`state.rs` 和 `error.rs`。

`pkg/parser/mysql/Cargo.toml` 表明该 crate 的库入口是 `lib.rs`，依赖只有 `astersql-errors`、`semver` 和 `unicode-general-category`。`errcode.rs` 自身不导入这些依赖，也没有条件编译项；其内容可在编译期作为纯 `u16` 常量使用。

## 核心职责

文件的唯一职责是把 MySQL、MariaDB 和 TiDB 兼容错误的符号名固定到线上协议使用的 16 位无符号整数。当前共有 954 个 `pub const ...: u16` 声明，与 `pkg/parser/mysql/errcode.go` 的 954 个常量逐项同名同值。

这些值是兼容性契约而不是可自由整理的内部枚举：经典 MySQL 段从 `ErrErrorFirst = 1000` 延伸到 `ErrErrorLast = 1863`，随后保留 MySQL 5.7/8.0 的非连续编号、MariaDB 的 4xxx 扩展以及 TiDB 的 8061–8066 扩展。文件末尾明确要求新错误码转移到 `pkg/errno`，所以本文件是 parser 兼容层的历史目录，不是新增全局错误码的默认落点。

## 主要符号

- `ErrErrorFirst: u16 = 1000` 与 `ErrHashchk: u16 = 1000`：经典段的起点哨兵和第一项共享数值。共享数值是有意保留的协议事实，不能假设常量名与数值一一对应。
- `ErrErrorLast: u16 = 1863` 与 `ErrRowInWrongPartition: u16 = 1863`：经典段末尾同样是哨兵与实际错误共享数值。
- `ErrParse = 1064`、`ErrDupEntry = 1062`、`ErrNoDB = 1046`、`ErrUnknown = 1105`：分别代表解析失败、唯一性冲突、未选择数据库和通用未知错误；它们在解析器、错误消息表、SQLSTATE 表及 terror 转换中有直接使用证据。
- `ErrInvalidFieldSize = 3013` 至 `ErrFunctionalIndexNotApplicable = 3909`：MySQL 后续版本追加的稀疏编号，源码没有尝试补齐缺口。
- `ErrOnlyOneDefaultPartionAllowed = 4030` 至 `ErrSequenceInvalidTableStructure = 4141`：MariaDB 扩展。历史拼写 `Partion` 也属于公开符号兼容面，不应擅自改名。
- `ErrWarnOptimizerHintUnsupportedHint = 8061` 至 `ErrWarnOptimizerHintWrongPos = 8066`：TiDB 自定义警告/错误；例如 hint 解析路径用 8061 表示不支持的 hint，用 8063 表示无效内存配额。

本文件没有类型、trait、函数、`impl`、宏或可变静态数据，所有符号都是公开的 `u16` 常量。

## 执行流程

本文件没有运行时控制流。它参与错误链路的方式是常量替换和下游查表：

1. 上游代码在识别错误条件时选取常量。例如 `pkg/parser/yy_parser.rs` 用 `mysql::ErrParse` 创建 parser terror 错误，`pkg/parser/hintparserimpl.rs` 用 optimizer-hint 常量构造警告。
2. `pkg/parser/mysql/errname.rs::MySQLErrName` 以这些 `u16` 值为键，惰性创建“错误码 → 默认消息模板”映射。
3. `pkg/parser/mysql/state.rs::MySQLState` 以相同值为键创建“错误码 → SQLSTATE”映射；没有专用映射时由 `error.rs` 使用 `DefaultMySQLState`（`HY000`）。
4. `pkg/parser/mysql/error.rs::NewErr` 或 `NewErrf` 把编号、消息和 SQLSTATE 组合成 `SQLError`，最终编号保存在 `SQLError.Code` 中。
5. parser 顶层的再导出使 terror、类型检查及外部 crate 能共享同一协议编号，而无需依赖本文件的实现细节。

常量本身不检查错误是否发生，也不保证每个编号都有消息或 SQLSTATE 条目；映射完整性和回退行为由相邻模块承担。

## 数据与状态

全部数据都在编译期确定，类型统一为 `u16`，不存在堆分配、初始化顺序或运行时状态。数值布局包含三个必须保留的性质：经典区间内大体连续但有首尾别名；后续 MySQL 编号显式跳号；MariaDB/TiDB 使用独立高位区段。调用方可以复制这些 `u16` 值作为 HashMap 键、terror 的 `ErrCode` 输入或 `SQLError.Code`。

常量名不是规范化数据模型：源码保留 `ErrUnsuportedLogEngine`、`ErrOnlyOneDefaultPartionAllowed` 等历史拼写，也同时存在 `ErrJSON...` 与 `ErrJson...` 的大小写差异。安全维护应以 Go 对照的符号和值为准，不能通过统一命名、排序或自动递增来“修复”它们。

## 依赖与调用关系

直接装配关系为 `pkg/parser/mysql/lib.rs -> errcode.rs`，再由 `pkg/parser/lib.rs::mysql` 和 `pkg/parser/charset/lib.rs` 重导出。目标文件本身没有下游函数调用，也不依赖外部 crate API。

直接消费者包括：

- `pkg/parser/mysql/errname.rs`：通配导入全部错误码，构建默认消息表。
- `pkg/parser/mysql/state.rs`：通配导入错误码，构建 SQLSTATE 表。
- `pkg/parser/mysql/const.rs`：使用同目录错误码参与 MySQL 常量兼容逻辑。
- `pkg/parser/yy_parser.rs`：以 `ErrParse` 标识 SQL 语法分析错误。
- `pkg/parser/hintparser.rs` 和 `pkg/parser/hintparserimpl.rs`：使用 TiDB hint 警告码。
- `pkg/parser/terror/terror.rs`：在无法获得更具体的 MySQL 映射时使用 `ErrUnknown`。
- `pkg/parser/types/etc_test.rs` 及 terror 测试：通过完整模块路径验证类型错误码和 terror 转换。

RustCodeGraph 对目标文件显示 986 行、单一文件节点且 `used by 0 files`；按 `ErrErrorFirst`、`ErrDupEntry` 查询只识别到 `pkg/errno` 的同名后继常量，未为本文件的 954 个声明建立独立常量节点。因此上述消费者关系使用 `rg` 对真实源码引用补证，不能把图上的零使用误解为本模块未接线。

## 错误处理与边界

文件自身不返回 `Result`、不 panic，也不执行输入校验。其主要风险是协议兼容性错误：改值会让客户端收到错误编号、让消息/SQLSTATE 查表命中错误条目，或破坏 terror 与 MySQL 错误之间的映射。重复值也意味着反向从数字恢复唯一常量名在边界哨兵处不可行。

编号范围由 `u16` 保证可容纳当前最大值 8066，但类型只提供存储边界，不提供“已注册”语义。未知编号在 `NewErr` 中会使用默认 SQLSTATE，并在消息表缺失时直接拼接参数；这属于 `error.rs` 的回退逻辑，不是本文件的行为。经典区间以外的显式缺口同样不是错误，不能用连续性断言拒绝它们。

## 并发与资源生命周期

常量是不可变的编译期值，可被任意线程无锁读取；本文件不创建锁、原子变量、线程、任务、通道、文件句柄、网络连接或事务，也没有清理阶段。相邻的 `errname.rs::MySQLErrName` 使用 `LazyLock` 管理共享消息表，但该惰性初始化生命周期不属于 `errcode.rs`。因此扩展本文件不会直接引入并发或资源风险，风险集中在跨模块映射一致性和协议兼容性。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/mysql/errcode.go`。机械抽取 Rust 的 `pub const 名称: u16 = 数值;` 与 Go 的 `名称 = 数值` 后，两侧均得到 954 项，逐项 `diff` 无输出，说明当前符号顺序、名称和值完全一致。Rust 将 Go `const (...)` 块展开为独立的 `pub const u16`；这是类型表达差异，不改变协议数值。

两端都保留 `ErrErrorFirst/ErrHashchk = 1000`、`ErrErrorLast/ErrRowInWrongPartition = 1863`、后续稀疏 MySQL 编号、MariaDB 扩展和 TiDB 8061–8066，并都以注释要求新错误码转到 `pkg/errno`。Rust 额外在文件顶部和分段处写了迁移/兼容说明，但没有新增计算逻辑或改变 Go 语义。

## 扩展指南

维护现有 parser 兼容码时，应先在 `pkg/parser/mysql/errcode.go` 或上游协议来源确认精确名称和值，再同步本文件；不要自动重编号、去重、改历史拼写或填补数值空洞。随后检查 `errname.rs` 是否需要默认消息、`state.rs` 是否需要专用 SQLSTATE，以及 `error.rs`/terror 调用方是否能正确构造和传播该错误。

新增 TiDB 全局错误码时应遵循文件末尾指示，优先修改 `pkg/errno/errcode.rs` 及其 Go 对照，而不是继续扩展这里。只有维持 parser 旧 API/协议兼容所必需的变更才应进入本文件，并应同步独立测试 `pkg/parser/mysql/errcode_2_aster_unit_test.rs`；若涉及消息或状态，还应同步 `error_3_aster_unit_test.rs`、`error_test.rs` 或 `unit_test.rs`。测试逻辑必须继续放在独立测试文件中，不能内嵌到 `errcode.rs`。

建议至少验证：Go/Rust 名称和值全量相等；首尾共享值保持不变；代表性的 MySQL 稀疏码、MariaDB 码和 TiDB 码保持原值；新增码在消息表与 SQLSTATE 表中的有意映射或回退行为有测试。兼容性风险高于性能风险；常量访问没有可测量的运行时开销。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含目标文件；`files --filter pkg/parser/mysql` 确认源文件、Go 对照和独立测试；`node --file pkg/parser/mysql/errcode.rs --offset ...` 读取完整 986 行；`query/callers` 暴露了目标常量未被单独索引的限制。
- 源码：`pkg/parser/mysql/errcode.rs`（954 个公开 `u16` 常量、分段与停止新增说明）、`pkg/parser/mysql/lib.rs` 和 `pkg/parser/lib.rs`（模块声明与再导出）。
- crate 边界：`pkg/parser/mysql/Cargo.toml`（`astersql-parser-mysql`、`lib.rs` 入口及直接依赖）。
- 运行链证据：`pkg/parser/mysql/errname.rs`、`state.rs`、`error.rs`、`pkg/parser/yy_parser.rs`、`pkg/parser/hintparserimpl.rs`、`pkg/parser/terror/terror.rs`。
- Go 对照：`pkg/parser/mysql/errcode.go`；抽取双方常量后计数均为 954，名称和值的统一 diff 为空。
- 独立测试：`pkg/parser/mysql/errcode_2_aster_unit_test.rs` 验证首尾哨兵、MariaDB/TiDB 代表值及消息表；`error_3_aster_unit_test.rs`、`error_test.rs`、`unit_test.rs` 验证编号参与消息和 SQLSTATE 构造；`pkg/parser/terror/terror_test.rs` 验证 terror 映射与 `ErrUnknown` 回退。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令确认本文件存在且恰有 11 个固定二级章节，并人工复核没有把 RustCodeGraph 的缺失调用边写成未接线事实。
