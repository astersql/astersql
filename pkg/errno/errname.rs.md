# `pkg/errno/errname.rs`

## 文件定位

[`pkg/errno/errname.rs`](./errname.rs) 属于 `astersql-errno` crate。该 crate 的入口是 [`pkg/errno/lib.rs`](./lib.rs)，其中以 `pub mod errname` 暴露本模块，并把 `astersql-parser-mysql` 的 `errname` 模块重导出为 `mysql`。因此，本文件位于“数字错误码定义”与“可供协议错误、日志和诊断使用的标准消息元数据”之间：键来自同 crate 的 [`errcode.rs`](./errcode.rs)，值的类型和构造器来自 `pkg/parser/mysql/errname.rs`。

[`pkg/errno/Cargo.toml`](./Cargo.toml) 表明该 crate 没有 feature 分支，运行时仅直接依赖工作区路径依赖 `astersql-parser-mysql`；本文件也没有条件编译项。它保存 TiDB/AsterSQL 自有 errno 消息表，不应与 `pkg/parser/mysql/errname.rs` 中 parser/mysql 自己的默认消息表混为一张表。

## 核心职责

本文件只承担三件事：把 `errcode.rs` 中的 `u16` 错误码与英文 printf 风格消息模板配对；为可能包含敏感信息的格式参数记录零基脱敏位置；通过线程安全的惰性全局值向其他 crate 提供只读查询。源码中的 1,157 个 `ErrXxx => mysql::Message(...)` 表项覆盖通用 MySQL 错误和 TiDB/AsterSQL 扩展错误。

它不负责选择业务错误码、格式化占位符、应用脱敏策略、写日志或把错误编码为 MySQL 协议包。`mysql::Message` 在 `pkg/parser/mysql/errname.rs` 中仅把模板复制到 `ErrMessage.Raw`、把脱敏下标复制到 `ErrMessage.RedactArgPos`；真正的错误构造和输出发生在调用方。

## 主要符号

- `macro_rules! hashmap`（`errname.rs:30`）是模块私有辅助宏。它接受 `key => value` 列表，展开为数组后交给 `HashMap::from`，目的是让 Rust 表项的排版接近 Go map 字面量。宏不导出，也不包含查找或校验逻辑。
- `pub static MySQLErrName: LazyLock<HashMap<u16, mysql::ErrMessage>>`（`errname.rs:42`）是唯一公开符号。公开 API 是一个可解引用为只读 `HashMap` 的进程级静态值；键为 MySQL/TiDB errno，值包含 `Raw: String` 和 `RedactArgPos: Vec<usize>`。
- 初始化闭包中的每个 `ErrXxx` 来自 `use super::errcode::*`；每个 `mysql::Message(&str, &[usize])` 来自 crate 根对 `astersql_parser_mysql::errname` 的重导出。空切片表示没有需要特别脱敏的参数，例如 `ErrNoDB`；非空例子包括 `ErrDupEntry => &[0]`、`ErrSharedLockLost => &[1]`、`ErrWriteConflict => &[3, 4, 5, 6]`。

本文件没有类型、trait、普通函数或 `impl`，也没有可变公开状态。

## 执行流程

1. 某个调用方第一次解引用或索引 `MySQLErrName`，例如 `dbterror::ErrClass::NewStd` 在 `pkg/util/dbterror/terror.rs` 中按错误码索引消息。
2. `LazyLock` 执行一次初始化闭包。`hashmap!` 把全部键值表达式组成数组，逐项调用 `mysql::Message`，为每项分配拥有所有权的 `String` 和 `Vec<usize>`，最后构造 `HashMap<u16, ErrMessage>`。
3. 初始化完成后，所有线程共享同一个表。典型消费方式是读取 `Raw` 作为标准消息，或把整个 `ErrMessage` 交给 `NewStdErr`；本模块本身不再执行任何工作。
4. 后续访问直接读取已初始化的 map，不会重复构建。若调用方使用 `MySQLErrName[&code]` 而键缺失，Rust 的索引语义会 panic；本文件没有未知错误码的回退分支。

## 数据与状态

状态只有 `MySQLErrName` 内部的惰性初始化状态和初始化后的不可变 `HashMap`。表项的 `Raw` 保留 `%s`、`%d`、`%v`、宽度限制等 Go/MySQL 风格占位符；本模块把它们当普通字符串，不解析其类型或参数个数。`RedactArgPos` 是零基参数下标，例如 `ErrWriteConflict` 的 `[3, 4, 5, 6]` 对应 key 的四段，而不是字符串字节位置。

`ErrMessage` 拥有自己的字符串和向量，因此表初始化后不借用临时输入。公开 API 只暴露共享引用，没有插入、删除或重载入口。源文件和 Go 对照文件各统计到 1,157 个表项；该数量是当前源码事实，不是协议保证，测试只要求 Rust 表不被明显截断并覆盖声明的错误码。

## 依赖与调用关系

下游依赖只有标准库的 `HashMap`/`LazyLock`、`super::errcode::*` 和 crate 根的 `mysql` 重导出。`mysql::Message` 的直接被调用关系落到 `pkg/parser/mysql/errname.rs`，生成 `ErrMessage`；除此之外初始化闭包不访问 I/O、配置、网络、时钟或存储。

主要上游路径包括：

- `pkg/util/dbterror/terror.rs` 的 `ErrClass::NewStd`：将错误码转成 `u16` 后从表中取标准消息，再调用 `NewStdErr`。大量子系统的标准错误静态量经由这条路径间接依赖本表。
- `pkg/domain/domain.rs` 的 `ERR_INFO_SCHEMA_CHANGED`：读取 `ErrInfoSchemaChanged` 的 `Raw` 和 `RedactArgPos`，追加事务可重试标记后构造新消息。
- `pkg/types/errors.rs`：为 `ErrSyntax`、`ErrWrongValue` 等绑定“实际错误码 + 指定消息模板”的组合。
- `pkg/server/err/lib.rs`：把本表再次导出为 `errno::MySQLErrName`，供 server 错误层和迁移测试使用。

RustCodeGraph 能定位 `MySQLErrName`（`pkg/errno/errname.rs:42`），但当前索引对这个大型静态初始化报告 `used by 0 files`，精确 callers 查询也未在限定时间内返回。因此上述调用边以 Rust 源码引用搜索复核，而没有把缺失的图边误写成“无人使用”。

## 错误处理与边界

初始化闭包返回裸 `HashMap`，没有 `Result`，因而没有可传播的业务错误。字符串/向量/map 分配失败只会遵循 Rust 分配失败行为；`LazyLock` 初始化闭包若 panic，访问也会失败。本文件不校验模板占位符与调用参数是否匹配，也不验证脱敏下标是否越界，这些都是维护时必须由对照和测试保证的契约。

边界约束由独立测试覆盖：[`errname_test.rs`](./errname_test.rs) 解析 `errcode.go`，要求每个声明的错误码都存在于本表，并禁止 `[8800, 8900)` 预留区间；[`errname_2_aster_unit_test.rs`](./errname_2_aster_unit_test.rs) 要求表长大于 1,000，并抽查 `ErrNoDB`、`ErrDupEntry`、`ErrWriteConflict`、`ErrUserPrefixMismatch` 和 `ErrSharedLockLost` 的消息/脱敏元数据。缺失键的直接索引 panic、重复键可能在构造 map 时覆盖先前值、错误的模板或脱敏位置，都不会由本文件在运行时主动诊断。

## 并发与资源生命周期

`std::sync::LazyLock` 保证并发首次访问时只完成一次初始化并安全发布结果；其同步细节由标准库承担。本表初始化后只读，没有锁竞争式更新、后台任务、通道、事务或显式清理。作为 `static`，map 及其拥有的字符串/向量存活到进程结束，不执行面向业务的释放流程。

这与 `pkg/errno/infoschema.rs` 中可变统计数据的锁和刷新生命周期无关。`errname_2_aster_unit_test.rs` 中的并发增量测试验证的是 infoschema 统计，不应被当成本消息表的并发行为证据；本表相关的并发保证来自 `LazyLock` 和只读 API。

## 与 Go 版本的对应关系

Go 对照是 [`pkg/errno/errname.go`](./errname.go) 的包级 `var MySQLErrName = map[uint16]*mysql.ErrMessage{...}`。Rust 保留相同的“错误码 → 模板与脱敏位置”模型和当前相同的 1,157 个表项，但做了语言层适配：Go 的包初始化改为首次访问的 `LazyLock`；Go 的 `*mysql.ErrMessage` 改为 map 内拥有的 `ErrMessage` 值；Go 的 `nil`/`[]int{...}` 改为 `&[]`/`&[...]`，随后由 `Message` 复制成 `Vec<usize>`。

[`errname_test.go`](./errname_test.go) 与 Rust 的 `errname_test.rs` 都从 `errcode.go` 提取 `ErrXxx = N`，验证消息覆盖与 `[8800, 8900)` 预留区间。Rust 另有 `errname_2_aster_unit_test.rs` 提供表规模和代表性内容回归。Go 和 Rust 的 map/`HashMap` 迭代顺序都不应成为契约；调用方应按 errno 查找，而不是依赖声明或遍历顺序。

## 扩展指南

新增或调整 errno 时，最可能同时修改 [`errcode.rs`](./errcode.rs) 中的数值常量与本文件 `MySQLErrName` 初始化项，并同步同路径 Go 文件以保持移植语义。若消息含用户输入、键、SQL 或其他敏感字段，必须逐个核对 printf 参数顺序并为 `mysql::Message` 提供正确的零基 `RedactArgPos`；不能因为当前日志路径未使用某参数就省略脱敏元数据。

测试逻辑应继续放在独立文件，而不是嵌入 `errname.rs`：覆盖性/预留区间更新放在 `errname_test.rs`，具体消息和脱敏回归放在 `errname_2_aster_unit_test.rs`，并同步 `errname_test.go` 或相应 Go 测试。安全扩展至少检查：错误码不冲突且不进入预留区、表中存在键、模板文本/占位符兼容、敏感参数位置准确、现有直接索引调用不会遇到缺键。表规模增长会增加首次访问时的分配和构建成本，但稳态读取不引入重复初始化。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引可用（11,467 files）；`files --filter pkg/errno` 确认 Rust/Go 源与测试均在索引中；`query MySQLErrName --json` 和 `node pkg/errno/errname.rs::MySQLErrName` 定位公开静态量；`node --file pkg/errno/errname.rs --offset 1 --limit 260` 核对声明与初始化开头。图索引未产出可靠 callers 边，已明确记录限制并用源码搜索补证。
- 源与 crate 边界：`pkg/errno/errname.rs`、`pkg/errno/errcode.rs`、`pkg/errno/lib.rs`、`pkg/errno/Cargo.toml`、`pkg/parser/mysql/errname.rs`。
- Rust 调用证据：`pkg/util/dbterror/terror.rs`、`pkg/domain/domain.rs`、`pkg/types/errors.rs`、`pkg/server/err/lib.rs`，并以 `rg -n "MySQLErrName" --glob '*.rs' --glob '*.go' pkg` 复核引用。
- Go 与测试证据：`pkg/errno/errname.go`、`pkg/errno/errname_test.go`、`pkg/errno/errname_test.rs`、`pkg/errno/errname_2_aster_unit_test.rs`。按表项声明模式统计，Rust 与 Go 当前均为 1,157 项。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证恰有十一个固定二级章节，并人工复核本文回答了文件为何存在、首次访问如何运行以及新增错误时应同步哪些源码与独立测试。
