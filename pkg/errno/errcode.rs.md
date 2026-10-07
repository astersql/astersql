# `pkg/errno/errcode.rs`

## 文件定位

[`errcode.rs`](./errcode.rs) 是 `astersql-errno` crate 的错误编号目录。crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod errcode` 暴露它；[`Cargo.toml`](./Cargo.toml) 指定该 crate 的库入口为 `lib.rs`。本文件本身不创建错误对象，也不保存错误消息，只把 MySQL、MariaDB、TiDB 以及 TiKV/PD/TiFlash 使用的协议级 errno 声明为 `pub const ...: u16`。

它位于错误处理链的最底层标识层：业务模块选择常量，`pkg/errno/errname.rs` 将编号映射为消息模板，`pkg/util/dbterror` 等模块再把编号、错误类别和消息包装成可传播的数据库错误。例如 `pkg/kv/error.rs` 的 `ErrKeyExists` 用 `ErrDupEntry` 创建 KV 错误，`pkg/store/driver/error/error.rs` 用 `ErrPDServerTimeout` 创建 TiKV 类错误。

## 核心职责

1. 为兼容 MySQL 客户端和 Go 实现提供稳定的 `u16` 错误编号。编号是协议标识，不能因重排、重命名或代码整理而自动重新分配。
2. 集中维护多个编号域：经典 MySQL 范围、MySQL 8 扩展、MariaDB 扩展、TiDB 自定义范围、DDL/资源组范围和分布式存储范围。
3. 保留特殊边界和空洞，包括匿名 `const _: u16 = 8181` 以及禁止本仓库占用的 `[8800, 8900)` 下游预留区间。
4. 给 `errname.rs`、错误类别包装器及业务代码提供共享的编译期常量，避免各子系统各自硬编码同一 errno。

本文件不是错误注册表的完整实现：编号到文案的完整性由 `errname.rs` 和独立测试保证；SQLSTATE、错误严重级别、重试策略以及 Error/Warn/Ignore 决策也分别由其他模块负责。

## 主要符号

- `#![allow(non_upper_case_globals)]`：允许沿用 Go/MySQL 风格的 `ErrXxx` 名称，而不强制改成 Rust 的全大写常量名；这有助于跨语言逐项核对。
- `ErrErrorFirst = 1000`、`ErrHashchk = 1000` 与 `ErrErrorLast = 1863`、`ErrRowInWrongPartition = 1863`：定义经典 MySQL 错误段的两端；边界常量与真实错误常量共享数值，因此“常量名”并不保证“一名一值”。
- `ErrForeignKeyCascadeDepthExceeded = 3008` 起的一组常量：MySQL 8 及后续扩展。该段不是连续表，且声明顺序不总是严格按数值递增，例如 `ErrTableWithoutPrimaryKey = 3750` 位于若干 39xx 常量之后。
- `ErrOnlyOneDefaultPartionAllowed = 4030` 至 `ErrSequenceInvalidTableStructure = 4141`：MariaDB 兼容错误。
- `ErrMemExceedThreshold = 8001` 起的一组常量：TiDB 自定义错误，覆盖事务、类型、优化器、执行器、权限、TTL、导入等子系统。
- 匿名常量 `const _: u16 = 8181`：为 `ErrPDTimestampLagsTooMuch` 保留编号，但不向调用方导出可使用的符号。
- `ErrUnsupportedDDLOperation = 8200` 起的一组常量：DDL 错误；`ErrResourceGroupExists = 8248` 起的一组常量单独按资源组语义排列。两组的数值区间交错，因此新增项必须检查全文件而不能只看相邻行。
- `ErrEngineAttributeInvalidFormat = 8270` 至 `ErrMaxKeysReadExceeded = 8274`：文件注释标为未来用途预留的具名槽位；这些名称已经是公开 API，不能当作匿名空洞复用。
- `ErrPDServerTimeout = 9001` 至 `ErrSharedLockLost = 9015`：TiKV、PD、TiFlash 及事务存储相关错误；`ErrUserPrefixMismatch = 20003` 是当前文件中的高位 keyspace 用户名前缀错误。

源码中没有 struct、enum、trait、函数、`impl` 或条件编译项；公开 API 由 1159 个具名 `pub const u16` 组成，另有一个不导出的匿名保留值。

## 执行流程

本文件没有运行时控制流，实际链路是编译期选择与运行时消费：

1. Rust 编译器通过 `pkg/errno/lib.rs` 编译并导出所有 `errcode` 常量。
2. `pkg/errno/errname.rs` 以 `use super::errcode::*` 引入编号，并在 `MySQLErrName` 中为编号关联 `mysql::Message` 模板及脱敏参数位置。
3. 下游错误层选择编号。例如 `pkg/kv/error.rs` 将 `ErrDupEntry` 交给 `dbterror::ClassKV.NewStd`，`pkg/domain/domain.rs` 使用 `ErrInfoSchemaExpired`/`ErrInfoSchemaChanged`，`pkg/session/runtime/control.rs` 使用资源组和用户相关编号。
4. `dbterror`/`terror` 等错误包装层把 errno 连同错误类别和消息组成错误对象；最终 SQL 协议、日志或测试读取该编号。编号自身不决定是否重试、是否降级为警告，也不执行格式化。

RustCodeGraph 的文件级索引显示 `errcode.rs` 被 47 个 Rust 文件使用。单个常量的 `callers` 查询因索引中同时存在 Go 与 Rust 同名常量而无法消歧，因此这里仅陈述由直接源码引用确认的调用关系，不把文件级“used by”误写为精确调用边。

## 数据与状态

所有值都是编译期 `u16` 常量，不分配堆内存、不初始化全局可变状态，也没有运行时查表成本。`u16` 与 MySQL errno 的线协议宽度相符；最高的当前具名值 `20003` 仍在 `u16` 范围内。

编号集合具有以下不变量：

- 名称和值必须与 `pkg/errno/errcode.go` 保持一致；本次静态抽取核对到两侧均为 1159 个具名常量，名称集合相同且没有数值差异。
- 编号不要求连续或唯一。边界别名、兼容别名可以共享数值，历史空洞必须保留。
- `[8800, 8900)` 专供下游 fork，当前仓库不得新增该范围的错误码。
- 8181 仅占位，不能因没有公开名称就随意复用。
- 声明分组表达来源和维护归属，而不是可据以二分查找的排序结构；消费者应按符号使用，不能依赖源码行序。

## 依赖与调用关系

本文件只使用 Rust 原生整数常量，没有 `use` 项，也不直接依赖第三方 crate。所属 `astersql-errno` crate 在 [`Cargo.toml`](./Cargo.toml) 中仅对 `astersql-parser-mysql` 声明普通依赖，但该依赖是 `lib.rs`/`errname.rs` 构造消息所需，不是 `errcode.rs` 自身所需。

直接下游包括：

- [`errname.rs`](./errname.rs)：把常量作为 `MySQLErrName` 的键，补上消息模板和脱敏元数据。
- `pkg/util/dbterror/lib.rs`：在 `errno` 模块中重新导出全部常量和消息表，供 DDL、KV、执行器等错误类别使用。
- `pkg/param/lib.rs`、`pkg/server/err/lib.rs`、`pkg/util/memory/lib.rs` 等门面：重新导出常量，维持包级兼容路径。
- `pkg/errctx/context.rs`：按 `ErrDupEntry` 等编号把错误归入可返回、警告或忽略的处理组。
- `pkg/kv/error.rs`、`pkg/domain/domain.rs`、`pkg/store/driver/error/error.rs`、`pkg/session/runtime/control.rs`：在具体业务边界选择并包装编号。

因此此文件位于 SQL 执行、DDL、会话和存储错误路径的共享依赖点。修改一个既有数值的影响面远大于普通内部常量修改，可能破坏客户端判断、监控聚合、重试分类及 Go/Rust 一致性。

## 错误处理与边界

由于只有常量声明，本文件不会产生或传播 Rust `Result`/panic。它的“错误处理”职责是让其他层使用正确且稳定的编号，主要风险是静态数据错误：重复占用不应复用的编号、错配消息、侵入预留区间、溢出 `u16`，或与 Go 版本漂移。

边界由独立测试覆盖：[`errcode_1_aster_unit_test.rs`](./errcode_1_aster_unit_test.rs) 抽查经典 MySQL、MySQL 8、MariaDB、TiDB、DDL、资源组及分布式存储锚点；[`errname_test.rs`](./errname_test.rs) 对照嵌入的 Go `errcode.go` 检查每个 Go 错误码均有消息，并禁止 `[8800, 8900)`；[`errname_2_aster_unit_test.rs`](./errname_2_aster_unit_test.rs) 抽查关键文案和脱敏位置。后者说明“编号存在”还不足以完成扩展，消息及敏感参数元数据也必须同步。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或文件/网络句柄。常量在编译期内联或作为静态值引用，因而没有初始化顺序和释放阶段，也不存在此文件内部的数据竞争。

并发语义只出现在消费者处。例如错误统计由 `infoschema.rs` 管理，错误对象可能由 `LazyLock` 初始化；这些生命周期不属于 `errcode.rs`。新增常量不应在本文件引入可变注册过程，否则会改变当前“纯编译期目录”的简单并发模型。

## 与 Go 版本的对应关系

直接对照文件是 [`errcode.go`](./errcode.go)。Rust 版本保留了 Go 常量名、数值、主要分段和历史拼写（例如 `ErrOnlyOneDefaultPartionAllowed`），把 Go 的无显式类型整数常量写成 `pub const ...: u16`。Go 的 `_ = 8181` 对应 Rust 的 `const _: u16 = 8181`。

本次用正则静态抽取两文件的具名 `Err* = 十进制值` 后比较，结果是：Go 1159 项、Rust 1159 项，双方没有独有名称，也没有数值不一致。这证明当前声明集合逐项对齐，但不替代消息表、SQLSTATE 或实际错误包装行为的测试。

Go 测试 [`errname_test.go`](./errname_test.go) 会嵌入并解析 `errcode.go`，验证消息覆盖和下游预留区间；Rust 的 `errname_test.rs` 有意继续解析同一 Go 文件，以 Go 清单作为兼容基准。Rust 另设 `errcode_1_aster_unit_test.rs` 对 Rust 常量进行锚点断言，符合“源文件与测试逻辑分离”的仓库要求。

## 扩展指南

新增或迁移错误码时应按以下顺序处理：

1. 先确认编号来源与所有权：MySQL/MariaDB 兼容号沿用上游值，TiDB 自定义号选择所属分组；不得使用 8181 或 `[8800, 8900)`，也不能仅根据相邻行判断可用空洞。
2. 在 `errcode.go` 与 `errcode.rs` 同步同名同值声明，保留必要的历史拼写和兼容别名。若编号大于 65535，当前 `u16` API 无法表达，必须先评估协议与全链路类型，不能强制截断。
3. 在 `errname.go` 与 `errname.rs` 同步消息模板和脱敏参数位置。涉及 SQLSTATE、错误类别、重试集合或 Error/Warn/Ignore 分组时，还要修改对应消费者，而不是把策略塞进本常量文件。
4. 在独立 Rust 测试文件中增加有代表性的锚点或行为断言；不要把 `#[cfg(test)]` 测试内嵌进 `errcode.rs`。至少同步 `errcode_1_aster_unit_test.rs`，消息/保留区间影响还应覆盖 `errname_test.rs` 或 `errname_2_aster_unit_test.rs`，具体业务包装则在所属 crate 的测试中验证。
5. 检查兼容风险：既有编号变更会影响客户端、告警、日志解析和持久化诊断数据；错误模板参数变化会影响格式化与脱敏；增加大量运行时映射会改变当前零初始化成本，但单纯增加编译期常量没有并发成本。

## 验证依据

- RustCodeGraph：`status` 显示索引含 `pkg/errno/errcode.rs`；`files --filter pkg/errno` 确认同目录 Go/Rust 源与独立测试；`node --file pkg/errno/errcode.rs` 显示全文件 1227 行并报告 47 个 Rust 文件使用；对 `ErrDupEntry`、`ErrErrorFirst`、`ErrPDServerTimeout` 的 `query` 找到 Go/Rust 同名定义，`callers` 因定义歧义未返回精确边。
- 源与 crate 边界：阅读 `pkg/errno/errcode.rs`、`pkg/errno/lib.rs`、`pkg/errno/Cargo.toml`、`pkg/errno/errname.rs`。
- Go 对照：阅读 `pkg/errno/errcode.go`、`pkg/errno/errname_test.go`；静态抽取比较确认 1159 个具名常量的名称和值全部一致。
- Rust 测试：阅读 `pkg/errno/errcode_1_aster_unit_test.rs`、`pkg/errno/errname_test.rs`、`pkg/errno/errname_2_aster_unit_test.rs`，以及下游行为证据 `pkg/util/dbterror/migration_aster_unit_test.rs`。
- 调用关系抽样：阅读 `pkg/kv/error.rs`、`pkg/util/dbterror/lib.rs`，并用 `rg` 核对 `pkg/domain/domain.rs`、`pkg/store/driver/error/error.rs`、`pkg/session/runtime/control.rs`、`pkg/errctx/context.rs` 的直接引用。
- 本任务是纯文档分析，按计划未运行 Cargo；最终仅执行任务指定的 11 章节结构检查。
