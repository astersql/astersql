# `pkg/util/dbterror/ddl_terror.rs`

## 文件定位

本文件属于 `astersql-util-dbterror` crate，由 [`pkg/util/dbterror/lib.rs`](lib.rs) 以私有模块 `ddl_terror` 装入并将其公开项重新导出。它不是 DDL job 的执行器，而是 DDL 子系统共享的错误目录：把 `astersql-errno` 提供的 MySQL 错误码和消息表包装成 `astersql-parser-terror::Error`，再向 `pkg/ddl/**` 等调用方提供稳定的错误对象；同时保存 DDL reorganization（reorg/backfill）的可重试错误码与消息边界。

crate 边界由 [`pkg/util/dbterror/Cargo.toml`](Cargo.toml) 给出：生产依赖只有 `astersql-errno` 和 `astersql-parser-terror`，测试依赖为 `astersql-testkit-testsetup`。源码顶部明确对应 Go 文件 [`pkg/util/dbterror/ddl_terror.go`](ddl_terror.go)，当前 Rust 与 Go 两侧都定义了 231 个 `Err*` 错误变量。

## 核心职责

1. 以 231 个公开的 `LazyLock<Box<terror::Error>>` 作为 DDL 错误目录，覆盖 worker/job 生命周期、表列与索引校验、分区、序列、临时表、placement、TiFlash、ingest、外键、TTL、check constraint、storage class 等错误族（`ErrInvalidWorker` 至 `ErrForbiddenDDL`）。
2. 对可直接采用标准 MySQL 文案的条目调用 `ClassDDL.NewStd`；对错误码与消息来源不同或需要预填模板片段的条目调用 `ClassDDL.NewStdErr`。例如 `ErrWaitReorgTimeout` 使用 `ErrLockWaitTimeout` 作为对外 code，但使用 `ErrWaitReorgTimeout` 对应的消息；`ErrUnsupportedModifyCollation` 预填成 `Unsupported modifying collation from %s to %s`，留待调用者继续绑定参数。
3. 用 `DDL_ERRORS` 和 `initialize_ddl_errors` 强制完成所有懒初始化，恢复 Go 包级 `var` 在包初始化阶段按顺序构造错误对象的语义，避免错误注册表冻结后第一次访问才注册。
4. 用 `ReorgRetryableErrCodes` 与 `ReorgRetryableErrMsgs` 描述 reorg 遇到瞬时存储、网络、schema/owner 变化时允许重试的判定数据。

## 主要符号

- `format_mysql_message(template: &str, arguments: &[&str]) -> String`：按参数顺序逐次替换第一个 `%s`。它只服务于本文件中 `NewStdErr` 的模板预组装，不是完整的 Go `fmt.Sprintf` 实现；未匹配的 `%s` 会保留，供错误实例生成时绑定业务参数。
- `ErrInvalidWorker`、`ErrNotOwner`、`ErrInvalidDDLJob`、`ErrCancelledDDLJob`、`ErrPausedDDLJob`：DDL worker、owner 与 job 状态的基础错误对象。
- `ErrUnsupported*` 系列：把各种尚不支持或组合非法的 DDL 操作归入标准/自定义模板，常以 `mysql::ErrUnsupportedDDLOperation` 为 code。
- `ErrDDLJobNotFound`、`ErrCancelFinishedDDLJob`、`ErrCannotCancelDDLJob`、`ErrCannotPauseDDLJob`、`ErrCannotResumeDDLJob`：job 管理操作的错误对象。
- `ErrIngestFailed`、`ErrIngestCheckEnvFailed` 以及 TiFlash、TTL、外键、check constraint 错误族：供对应 DDL 分支生成兼容 MySQL/TiDB 的分类错误。
- `DDL_ERRORS: [&LazyLock<Box<terror::Error>>; 231]`：按 Go `var` 块顺序保存全部 231 个错误懒值；它是 crate 内部初始化清单，不对外导出。
- `initialize_ddl_errors()`：遍历 `DDL_ERRORS` 并调用 `LazyLock::force`。直接上游是 `lib.rs` 的 `DBTERROR_PACKAGE_INIT` 启动段钩子。
- `ReorgRetryableErrCodes: LazyLock<HashSet<u16>>`：包含 19 个可重试 code，包括 PD/TiKV/TiFlash timeout 或 busy、锁解析超时、region 不可用、GC 中止、write conflict、schema 过期/变化、事务可重试、owner 变化、split region 范围异常，以及 `CodeResultUndetermined`。
- `ReorgRetryableErrMsgs: &[&str]`：四条无法仅凭 code 分类的瞬时错误消息，顺序与 Go 切片一致。

## 执行流程

初始化流程如下：链接 `astersql-util-dbterror` 时，`lib.rs` 把 `DBTERROR_PACKAGE_INIT` 放入 Unix/macOS/Windows 对应的启动段；进程或 libtest 主入口执行前调用其中的 `initialize()`；该函数进入 `initialize_ddl_errors()`，逐项 `force` `DDL_ERRORS`；每项闭包通过 `ClassDDL.NewStd` 或 `ClassDDL.NewStdErr` 创建并注册 `terror::Error`。这一流程在 `terror::RegisterFinish()` 冻结注册表之前完成。

运行期错误流程较短：DDL 业务代码从 crate 根取得某个 `Err*` 静态项，解引用时得到已初始化的 `terror::Error`，再直接读取 code/message，或调用 `GenWithStack`、`GenWithStackByArgs` 绑定上下文并生成具体错误。例如 `pkg/ddl/index.rs` 使用 `ErrUnsupportedAddPartialIndex.GenWithStackByArgs`，`pkg/ddl/backfilling.rs` 使用 `ErrInvalidSplitRegionRanges.GenWithStackByArgs`，`pkg/ddl/ingest/disk_root.rs` 使用 `ErrIngestCheckEnvFailed`。

reorg 重试判定的数据流程是由消费者以错误 code 查询 `ReorgRetryableErrCodes`，或以错误文本匹配 `ReorgRetryableErrMsgs`。当前仓库文本检索只发现这两个符号被 `migration_aster_unit_test.rs` 直接使用，未在已迁移的 Rust 生产代码中找到直接消费者；因此本文只能确认注册表契约和测试覆盖，不能声称 Rust reorg 主链已经接入该判定。

## 数据与状态

错误对象本身是进程级只读全局数据。每个 `LazyLock<Box<terror::Error>>` 在首次强制初始化后保持稳定；`Box` 固定错误对象所有权，`LazyLock` 提供一次初始化，调用方共享读取。`DDL_ERRORS` 只聚合引用，不复制或重新排序错误对象。

`ClassDDL` 来自 [`pkg/util/dbterror/terror.rs`](terror.rs) 的 `ErrClass { inner: terror::ClassDDL }`。`NewStd` 将输入转为完整宽度 `terror::ErrCode`，仅在标准消息表查找时转为 `u16`；`NewStdErr` 把 code 和选定消息模板交给底层 parser terror。迁移测试 `new_std_preserves_wide_error_code` 专门固定了这一宽度语义。

`ReorgRetryableErrCodes` 采用 `HashSet<u16>`，对应 Go 的 `map[uint16]struct{}`，集合只表达成员关系、没有顺序承诺。`ReorgRetryableErrMsgs` 则使用静态切片保留 Go 顺序。文件没有可变业务状态、缓存淘汰、事务状态或持久化数据。

## 依赖与调用关系

上游装配是 `pkg/util/dbterror/lib.rs`：它声明并 re-export `ddl_terror`，并以 `DBTERROR_PACKAGE_INIT` 调用 `initialize_ddl_errors`。RustCodeGraph 对文件给出的直接使用范围为 12 个文件，已确认的生产调用示例包括 `pkg/ddl/ttl.rs`、`pkg/ddl/storage_class.rs`、`pkg/ddl/index.rs`、`pkg/ddl/backfilling.rs`、`pkg/ddl/ingest/disk_root.rs` 和若干 materialized-view 持久化模块；测试调用包括 `pkg/ddl/backfilling_test.rs` 与本 crate 的迁移测试。

下游依赖有三层：标准库的 `LazyLock`/`HashSet`；`astersql-errno::{errcode, errname::MySQLErrName}` 提供 MySQL/TiDB code 与标准模板；`astersql-parser-terror` 提供 `Error`、`ErrCode`、`CodeResultUndetermined`、消息类型以及底层注册行为。所有错误都经相邻 `terror.rs` 中的 `ClassDDL` 包装创建，因此 RFC 分类保持 `ddl:<code>`。

RustCodeGraph 能解析 `initialize_ddl_errors -> DDL_ERRORS` 的引用边和 `format_mysql_message` 的精确签名，但当前索引未把 Rust `pub static` 暴露为可直接查询的 variable 节点；具体静态项使用点因此由 `rg` 补证。图查询没有提供 reorg 两张表的生产调用边，与文本搜索结果一致。

## 错误处理与边界

本文件定义错误而不捕获错误。标准条目依赖 `MySQLErrName` 必须存在相应 code；使用索引访问意味着缺失映射会在初始化期暴露，而不是静默退化。自定义条目必须谨慎区分“错误 code”与“消息模板来源”，`ErrWaitReorgTimeout` 就是两者不同的明确例子。

`format_mysql_message` 只替换 `%s`，不会解释 `%d`、`%v`、位置参数、转义百分号或类型格式化；新增模板若依赖这些能力，不能假定该帮助函数等价于 `fmt.Sprintf`。参数少于占位符时，剩余占位符有意保留；参数过多时，多余参数没有效果。

reorg 重试表是一条安全边界：误增会让永久性/数据类错误反复执行，误删会把可恢复的瞬时故障升级为 job 失败。`ErrDupEntry` 在迁移测试中被明确断言为不可重试。消息匹配比 code 脆弱，新增或修改文本必须同时检查实际错误来源与 Go 对照，不能只凭相似语义加入。

## 并发与资源生命周期

`LazyLock` 保证每个静态错误在并发访问下只初始化一次；正常请求路径只读共享对象，没有显式锁、channel、异步任务或 per-request 分配生命周期。`HashSet` 同样在首次访问时一次构造，之后只读。

需要注意的是底层 `ErrClass::NewStd` 的 Go 对照语义注明“通常只用于全局变量初始化且非 goroutine-safe”。Rust 通过启动段提前、串行地 force 231 个懒值，避免在并发请求开始后才触发底层注册。`migration_aster_unit_test::ddl_errors_are_registered_before_registry_freeze` 在子进程中先调用 `RegisterFinish`，再读取 `ErrForbiddenDDL`，验证此时不会因迟到注册而 panic。

资源生命周期为进程全程：错误对象、code 集合和消息切片都不会释放或更新。此文件不持有网络连接、磁盘句柄、事务、DDL job 或 backfill worker；重试表也只提供判定数据，不负责调度或 sleep/backoff。

## 与 Go 版本的对应关系

Go 的 `var (...)` 与 Rust 的 231 个 `pub static Err*: LazyLock<Box<terror::Error>>` 数量一致，定义顺序由 `DDL_ERRORS` 显式复制。Go 导入包时立即执行变量初始化；Rust 没有等价 crate 初始化钩子，所以由 `lib.rs::DBTERROR_PACKAGE_INIT` 在平台启动段中调用 `initialize_ddl_errors`。这是语言运行时差异下的必要接线，不改变错误目录的业务含义。

Go 的 `ClassDDL.NewStd`/`NewStdErr` 分别对应 Rust 同名方法；Go `fmt.Sprintf` 在本文件使用场景中由 `format_mysql_message` 的顺序 `%s` 替换模拟；Go `map[uint16]struct{}` 对应 `LazyLock<HashSet<u16>>`；Go `[]string` 对应静态 `&[&str]`。`CodeResultUndetermined` 在两侧都显式转换为 `u16` 后加入重试集合。

现有 Rust 测试并非只检查数量：`ddl_errors_preserve_go_custom_templates_and_codes` 抽检 code/message 分离，`go_merge_11_new_ddl_errors_keep_codes_and_templates` 固定较新的 TiFlash/TTL 模板，`reorg_retryable_lists_match_go_exactly` 完整比较 19 个 code 和四条消息，初始化测试固定注册时序。当前证据支持这些已覆盖契约；未对 231 个错误逐项做运行时全量 message/code 对比，因此不能据此断言每一个自定义模板都已被测试穷举。

## 扩展指南

新增 DDL 错误时，应先在 Go 对照和 errno 消息表中确认 code、模板与兼容要求，再在相同语义位置增加独立 `pub static`。标准消息用 `ClassDDL.NewStd`；自定义或预填模板用 `NewStdErr`，并确认 `format_mysql_message` 的有限 `%s` 语义足够。随后必须把新静态项按 Go 顺序加入 `DDL_ERRORS`，同步数组长度，否则启动期不会提前注册或编译期长度会不匹配。

修改 reorg 重试边界时，应同步 `ReorgRetryableErrCodes`/`ReorgRetryableErrMsgs`、Go 文件和 `migration_aster_unit_test.rs::reorg_retryable_lists_match_go_exactly`。新增错误的回归测试应继续放在独立测试文件 `pkg/util/dbterror/migration_aster_unit_test.rs`（或同目录新的 `*_test.rs`），不要嵌入生产源文件；若错误由某个 DDL 功能分支生成，还应在该功能已有的独立测试中覆盖参数绑定及错误分类。

兼容风险主要是 SQL error code、RFC class 和用户可见文案变化；正确性风险主要是遗漏 `DDL_ERRORS`、错误选择消息来源、错误扩张 reorg 可重试集合；性能风险很低，但 231 个对象在启动阶段一次性构造是有意的注册时序成本，不应随意改回请求期初始化。若未来移除启动段方案，必须先提供跨目标平台且发生在 `RegisterFinish` 之前的等价初始化入口。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/dbterror` 找到目标 Rust/Go/测试文件。
- RustCodeGraph `node --file pkg/util/dbterror/ddl_terror.rs`：读取完整 1,499 行源码，确认 231 个错误项、`DDL_ERRORS`、初始化函数及两张 reorg 表；文件节点报告被 12 个文件使用。
- RustCodeGraph `callees initialize_ddl_errors`：确认其引用 `DDL_ERRORS`；`query format_mysql_message --kind function` 确认签名与位置。Rust 静态变量节点缺失的部分以 `rg` 补查。
- 已读生产与装配文件：`pkg/util/dbterror/ddl_terror.rs`、`pkg/util/dbterror/terror.rs`、`pkg/util/dbterror/lib.rs`、`pkg/util/dbterror/Cargo.toml`；目标目录没有 `doc.go`。
- 已读 Go 对照：`pkg/util/dbterror/ddl_terror.go`；文本计数确认 Rust `^pub static Err` 与 Go `Err*=...` 均为 231，Go 尾部的 19 个重试 code 和四条消息与 Rust 一致。
- 已读独立 Rust 测试：`pkg/util/dbterror/migration_aster_unit_test.rs`、`pkg/util/dbterror/main_test.rs`；相关 Go 测试目录文件为 `pkg/util/dbterror/main_test.go` 与 `pkg/util/dbterror/terror_test.go`，目标文件没有同名独立测试。
- 已读 DDL 入口说明 `docs/agents/ddl/README.md`，其中 job/owner/reorg 架构陈述仅用于定位，并由本次源码和调用点核验后写入本文。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求本文恰好包含十一个固定二级标题；验证命令及退出码在任务交付时记录。
