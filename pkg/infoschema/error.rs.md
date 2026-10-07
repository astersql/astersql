# `pkg/infoschema/error.rs`

## 文件定位

该文件是 `astersql-infoschema` crate 的错误元数据目录，由 crate 根模块
[`pkg/infoschema/lib.rs`](./lib.rs) 通过 `pub mod error` 公开。crate 边界由
[`pkg/infoschema/Cargo.toml`](./Cargo.toml) 定义；虽然该 manifest 已声明
`astersql-util-dbterror`，本文件的当前可执行实现只依赖标准库 `std::fmt`，并没有构造
`dbterror::Error`。文件顶部保留的 `LazyLock<dbterror::Error>` 方案位于块注释中，仅用于说明迁移历史，不参与编译或运行。

它位于 InfoSchema 元数据查询与上层 SQL/DDL 错误协议之间：为库、表、列、索引、外键、
Placement Policy、Resource Group 等对象定义稳定的错误分类和 MySQL 错误符号名。当前 Rust
接线范围很窄，生产代码 [`pkg/infoschema/infoschema.rs`](./infoschema.rs) 只直接使用
`ErrTableNotExists`；不能把文件中的全部常量理解为全部业务路径已经完成迁移。

## 核心职责

1. `ErrorClass` 区分 `Schema` 与 `Executor` 两类来源，保留 Go 版
   `dbterror.ClassSchema` / `dbterror.ClassExecutor` 的分类意图。
2. `SchemaError` 将分类和静态 MySQL 错误名组合成可复制的只读描述符。
3. `schema_errors!` 批量生成 45 个 `SchemaError::schema(...)` 常量；另有
   `ErrResourceGroupInvalidBackgroundTaskName` 这一项显式使用 `Executor` 类。因此当前生效表
   共 46 项。
4. `SchemaError::message` 和 `Display` 提供最低限度的字符串展示；前者输出
   `"{mysql_name}: {detail}"`，后者只输出 `mysql_name`。

这些职责是“错误元数据和字符串标签”，不是 Go `terror.Error` 的完整替代：当前类型不保存
数值 errno、标准消息模板或堆栈，也没有 `Equal`、`Code`、`GenWithStackByArgs` 等能力。

## 主要符号

- `pub enum ErrorClass { Schema, Executor }`：可复制、可比较的错误类别。46 个生效常量中，
  只有 `ErrResourceGroupInvalidBackgroundTaskName` 是 `Executor`，其余都是 `Schema`。
- `pub struct SchemaError`：包含两个公开字段：`class: ErrorClass` 和
  `mysql_name: &'static str`。静态字符串使该类型无需分配且可作为 `const`。
- `SchemaError::schema(mysql_name)` / `SchemaError::executor(mysql_name)`：`const fn`
  构造器，分别固定错误类别；没有校验名称是否真实存在于 errno 表。
- `SchemaError::message(detail)`：接收任意 `fmt::Display`，分配一个新 `String` 并拼接
  符号名和细节。仓库搜索未发现该方法的目标文件调用者。
- `impl fmt::Display for SchemaError`：只把 `mysql_name` 写入 formatter，不附加类别和细节。
- `schema_errors!`：私有声明宏，输入 `(Rust 常量名, 字符串字面量)` 对并生成公开常量。
  RustCodeGraph 将本文件识别为 5 个显式符号，但不会把宏生成的常量逐项建图，因此常量清单
  需要以该宏调用的源码为准。
- `ErrTableNotExists`：当前唯一被本 crate 生产实现直接引用的错误常量，映射字符串
  `ErrNoSuchTable`。
- `ErrDatabaseNotExists` 与 `ErrEmptyDatabase`：两个不同语义名称都映射 `ErrBadDB`，与 Go
  声明一致，调用方不能只靠 `mysql_name` 反推出 Rust 常量身份。

## 执行流程

本文件没有注册器、初始化函数或运行期状态机。常量在编译期构造；典型流程如下：

1. 调用方选取一个公开常量，例如 `ErrTableNotExists`。
2. 调用方读取 `mysql_name` 作为错误代码标签，或使用 `Display` 取得同一标签。
3. 如需附加上下文，可调用 `message(detail)`，得到新分配的字符串。
4. InfoSchema 当前实际路径中，`infoSchema::TableByName` 在查找失败时构造
   `InfoSchemaError { code: ErrTableNotExists.mysql_name, message: "schema.table" }`；脱敏策略
   加载逻辑又通过比较该 `code`（兼容额外的 `"ErrNoSuchTable"` 字符串）判断系统表尚未就绪，
   将加载状态置为完成，避免永久重试。相关位置是 `infoschema.rs` 的 `TableByName`、
   `LoadMaskingPolicies` 错误分支及 `isMaskingPolicyTableNotReady`。

其余常量目前没有从该模块到生产调用方的直接 Rust 引用证据；它们主要保存 Go 对照表和后续
接线所需的稳定名字。

## 数据与状态

`SchemaError` 的全部状态只有两个不可变值：错误类别和 `&'static str`。类型派生
`Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`，所以复制只是复制枚举判别值与静态字符串引用，
相等比较同时比较 `class` 与 `mysql_name`。所有公开错误都是 `pub const`，没有堆分配、惰性
初始化或全局可变状态。

错误名保存的是 Rust errno 表中的符号名（例如 `ErrNoSuchTable`），不是数值 MySQL errno
（该项在 [`pkg/errno/errcode.rs`](../errno/errcode.rs) 中为 1146），也不是
[`pkg/errno/errname.rs`](../errno/errname.rs) 中的标准消息模板。因此修改字符串会影响当前基于
字符串的分支判断，但不会自动更新数值码或模板。

## 依赖与调用关系

- 上游模块入口：`lib.rs::error` 公开模块本身，但没有把 `SchemaError` 或各常量再导出到
  crate 根。
- 已确认的生产调用者：`infoschema.rs` 导入 `crate::error::ErrTableNotExists`，用于
  `TableByName` 失败结果、脱敏策略加载错误分类和 `isMaskingPolicyTableNotReady`。
- 已确认的 Rust 测试调用者：
  [`pkg/infoschema/infoschema_nokit_test.rs`](./infoschema_nokit_test.rs) 用
  `ErrTableNotExists.mysql_name` 构造“脱敏系统表未就绪”错误，并验证只加载一次、不持续重试。
- Go 上游证据：[`pkg/infoschema/infoschema.go`](./infoschema.go) 的 `TableByName` 使用
  `ErrTableNotExists.FastGenByArgs`，`isMaskingPolicyTableNotReady` 使用 `Equal`；这对应 Rust
  当前的构造与字符串比较路径。
- 下游直接依赖仅为 `std::fmt`。`Cargo.toml` 中存在 `astersql-util-dbterror` 依赖，但当前
  生效代码未使用；注释掉的历史实现才引用 `dbterror` 与 `errno`。

RustCodeGraph 对 `SchemaError::message` 的通用名称查询混入大量其他模块的同名方法，不能作为
本文件调用边证据；精确 `rg` 搜索确认目标模块只有上述 `ErrTableNotExists` 引用。

## 错误处理与边界

构造器和格式化方法都不返回 `Result`，也不验证 `mysql_name`。错误拼写、错误分类或 errno
映射漂移都会静默成为运行时字符串协议的一部分。`message` 只做展示拼接，不会应用 MySQL
模板占位规则，也不会转义或结构化 `detail`。

`ErrTableNotExists.mysql_name` 是 `ErrNoSuchTable`，因此 Rust 调用方应比较该字段或共享描述符，
不要按 Rust 常量名 `ErrTableNotExists` 猜测字符串值。脱敏策略判定同时接受
`ErrTableNotExists.mysql_name` 与字面量 `ErrNoSuchTable`，两者当前实际相同；这是迁移期兼容
写法，不代表两套独立错误码。

文件没有实现 `std::error::Error`，也未与 crate 根的
`type Error = astersql_util_dbterror::errors::SharedError` 建立转换。因此它不能直接通过通用错误链
传播；调用方必须像 `InfoSchemaError` 那样显式复制标签和细节。

## 并发与资源生命周期

生效实现没有锁、原子量、线程、异步任务、通道、文件句柄或事务。`pub const` 和静态字符串在
程序整个生命周期内有效，`SchemaError` 为 `Copy`，跨线程读取本身不需要同步。唯一可能的运行期
资源动作是 `SchemaError::message` 为返回值分配 `String`，其生命周期由调用方拥有。

顶部块注释中的 `LazyLock` 不是当前行为；不能据此声称存在一次性初始化或 `dbterror::Error`
共享实例。与并发相关的脱敏策略缓存锁属于 `infoschema.rs`，不是本文件管理的资源。

## 与 Go 版本的对应关系

[`pkg/infoschema/error.go`](./error.go) 是直接对照文件。Rust 生效表保留了 Go 变量名、错误类别
和 errno 符号映射：45 项来自 `dbterror.ClassSchema.NewStd(...)`，资源组无效后台任务名这一项
来自 `dbterror.ClassExecutor.NewStd(...)`。`ErrDatabaseNotExists` / `ErrEmptyDatabase` 共享
`ErrBadDB` 等别名关系也被保留。

关键差异是表示能力：Go 变量是带错误类、数值码和标准消息模板的 `terror.Error`，可生成带参数
错误、比较错误和读取错误码；Rust `SchemaError` 目前只是 `{ class, mysql_name }` 描述符。
例如 Go `TableByName` 通过 `FastGenByArgs(schema, table)` 生成标准错误，而 Rust
`TableByName` 自行建立 `InfoSchemaError`，只写入字符串代码与 `schema.table` 细节。因此当前 Rust
实现保持了分类和名称协议，但尚未完整复刻 Go 的错误对象语义。

Go 测试 `pkg/infoschema/infoschema_test.go` 覆盖 `ErrTableNotExists.Equal(err)`，
`pkg/resourcegroup/tests/resource_group_test.go` 覆盖后台任务名的数值错误码；Rust 侧只有
`infoschema_nokit_test.rs` 间接覆盖 `ErrTableNotExists.mysql_name` 驱动的“不重试”分支，没有同名
独立 `error_test.rs`，也没有对 46 项映射和格式化行为的完整表驱动测试。

## 扩展指南

- 新增或调整错误时，先核对 `error.go`、`pkg/errno/errcode.rs` 和
  `pkg/errno/errname.rs`，再在 `schema_errors!` 表中保持 Rust 名、类别和 errno 符号一致；若 Go
  使用非 `ClassSchema` 分类，应像 `ErrResourceGroupInvalidBackgroundTaskName` 一样显式声明。
- 若只是接线现有错误，优先复用常量的 `mysql_name`，不要复制字符串；同时检查所有字符串比较
  分支，避免别名映射变化破坏行为。
- 若要实现 Go 等价能力，应在清晰的迁移任务中决定是否改用
  `astersql-util-dbterror` 的真实错误对象，并同步调用方；不能仅扩充 `message` 就宣称支持 errno、
  标准模板或错误等价比较。
- 测试应放在独立 Rust 测试文件中，建议新增 `pkg/infoschema/error_test.rs` 并在 `lib.rs` 以
  `#[cfg(test)]` 接入，覆盖完整映射表、唯一 `Executor` 项、共享 errno 名别名、`Display` 和
  `message`；涉及业务接线时同步扩展 `infoschema_nokit_test.rs`。不要把测试嵌入 `error.rs`。
- 兼容风险主要是错误名/类别被外部逻辑识别；性能风险主要来自高频路径调用 `message` 的字符串
  分配。当前常量读取本身无分配。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter
  pkg/infoschema/error.rs` 找到目标文件，报告 5 个显式符号；`query SchemaError --kind struct
  --json` 将目标结构定位到 `error.rs:122`。对 `SchemaError::message` 的 callers/callees 查询因
  同名符号产生跨模块噪声，未将其当作目标调用边。
- 已读 Rust/Cargo 路径：`pkg/infoschema/error.rs`、`pkg/infoschema/lib.rs`、
  `pkg/infoschema/Cargo.toml`、`pkg/infoschema/infoschema.rs`、
  `pkg/infoschema/infoschema_nokit_test.rs`、`pkg/util/dbterror/terror.rs`、
  `pkg/errno/errcode.rs`、`pkg/errno/errname.rs`。
- 已读 Go/测试路径：`pkg/infoschema/error.go`、`pkg/infoschema/infoschema.go`、
  `pkg/infoschema/infoschema_test.go`、`pkg/resourcegroup/tests/resource_group_test.go`；并以 `rg`
  查找目标类型、模块路径和全部错误名的 Rust/Go 测试引用。
- 人工复核结论：当前文件存在是为了集中保存 InfoSchema 错误分类与 MySQL 名称映射；实际运行
  只确认 `ErrTableNotExists` 接入 InfoSchema 查找和脱敏缓存分支；安全扩展必须同步 Go/errno
  映射和独立测试，不能把迁移期描述符误当成完整 `dbterror::Error`。
