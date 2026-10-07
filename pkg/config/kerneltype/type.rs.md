# [`pkg/config/kerneltype/type.rs`](type.rs)

## 文件定位

`type.rs` 是 `astersql-config-kerneltype` crate 中定义“当前二进制属于哪种内核形态”的名称与匹配规则文件。crate 根 `pkg/config/kerneltype/lib.rs` 以原始标识符 `mod r#type` 声明它，再通过 `pub use r#type::*` 无条件对外重导出其 API。因此外部调用方使用 `astersql_config_kerneltype::Name` 或 `astersql_config_kerneltype::IsMatch`，不需要感知 `type` 是 Rust 关键字，也不需要直接访问子模块。

该文件处在配置与进程启动的交界处：`cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 在创建存储、DDL owner manager 和 Domain 之前，把 PD 上报的 `KernelType` 交给 `IsMatch` 检查；不匹配时中止启动。`pkg/util/printer/printer.rs::{PrintTiDBInfo, GetTiDBInfo}` 则使用 `Name` 展示构建的内核类型。

## 核心职责

- 维护与 Go/PD 协议一致的两个规范字符串：`classicKernelName = "Classic"` 和 `nextgenKernelName = "Next Generation"`。
- 通过 `Name() -> &'static str` 把编译期选定的 Classic/NextGen 形态转换成稳定的对外名称。
- 通过 `IsMatch(pdKernelType: &str) -> bool` 校验 PD 字符串是否与当前二进制匹配，并保留“旧 PD 未携带字段时按 Classic 处理”的兼容规则。

本文件不解析配置、不请求 PD，也不在运行时切换内核。它只把编译期形态映射为名称，并提供纯函数式比较；实际的 Classic/NextGen 布尔判定由相邻的 `classic.rs` 或 `nextgen.rs` 提供。

## 主要符号

- `pub const classicKernelName: &str = "Classic"`：Classic 构建的规范名称，也是旧 PD 空值的兼容归宿。虽然 Rust 常量命名保留了 Go 风格，但因为它是 `pub`，字面值属于可见 API/PD 协议。
- `pub const nextgenKernelName: &str = "Next Generation"`：NextGen 构建的规范名称，包含空格且匹配时区分大小写。
- `pub fn IsMatch(pdKernelType: &str) -> bool`：公开的 PD 兼容检查入口。空字符串单独处理，其他值与 `Name()` 做完全相等比较。
- `pub fn Name() -> &'static str`：公开的当前内核名称查询。返回静态字符串，不分配内存。
- `use super::{classic,nextgen}::IsNextGen`：两条条件导入的符号名相同；`#[cfg(not(feature = "nextgen"))]` 选 `classic.rs::IsNextGen`，`#[cfg(feature = "nextgen")]` 选 `nextgen.rs::IsNextGen`。两个实现分别恒为 `false` 和 `true`。

文件没有 trait、struct、enum、`impl`、宏、可变静态量或文件内部的私有函数。`#![allow(non_snake_case, non_upper_case_globals, dead_code)]` 只用于保留 Go 移植后的 API 命名并容纳当前构建中的未使用符号，不改变行为。

## 执行流程

`Name` 的流程只有一次形态判断：

1. 调用当前 feature 分支导入的 `IsNextGen()`。
2. 若为 `true`，返回 `nextgenKernelName`。
3. 否则返回 `classicKernelName`。

`IsMatch` 在此基础上执行两段判断：

1. 如果 `pdKernelType.is_empty()`，将其视为旧 PD 的“未声明内核类型”，返回 `classicKernelName == Name()`。因此空值只匹配 Classic 构建，不匹配 NextGen 构建。
2. 非空时执行 `pdKernelType == Name()`。这是完全字符串比较，既不 trim 空白，也不忽略大小写，还不接受别名。

在应用主链中，`cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 先初始化 storage，只有 storage 支持 PD 且能取得 PD HTTP client 时才获取状态并调用 `IsMatch`。返回 `false` 后记录 `kernel type mismatch` 并返回错误，不继续启动后台组件、DDL owner manager 和 session Domain。

## 数据与状态

本文件的数据只有两个 `&'static str` 常量，不拥有可变状态。当前内核形态由 Cargo feature `nextgen` 在编译期决定；`pkg/config/kerneltype/Cargo.toml` 声明 `default = []` 和空 feature `nextgen = []`，所以默认构建为 Classic，显式启用 feature 时为 NextGen。

可观测不变量为：同一个二进制的 `Name()` 始终返回同一个静态引用；结果只能是两个规范名称之一；`IsMatch(Name())` 始终为真；空 PD 名称的结果等价于“当前构建是 Classic”。这些不变量由 `type_test.rs` 和 `migration_aster_unit_test.rs` 分层验证。

## 依赖与调用关系

下游依赖极小：`Name -> IsNextGen`，`IsMatch -> Name -> IsNextGen`。`IsNextGen` 是同 crate 父模块中的条件实现，不是外部 crate 依赖；`Cargo.toml` 没有 `[dependencies]` 项。

主要上游调用证据如下：

- `cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain` 调用 `kerneltype::IsMatch(&pdStatus.KernelType)`，将本文件接入带 PD 存储的服务器启动校验。
- `cmd/tidb-server/stubs.rs::Storage::{new, from_registered}` 用 `kerneltype::Name()` 初始化 `pd_kernel`，使本地 storage 默认状态与当前构建一致。
- `pkg/util/printer/printer.rs::PrintTiDBInfo` 把 `Name()` 的返回值写入启动日志；`GetTiDBInfo` 把它放入诊断/版本信息的 `Kernel Type` 字段。
- `pkg/config/kerneltype/lib.rs` 是模块装配边界：它条件编译并重导出 `classic.rs`/`nextgen.rs`，同时无条件重导出本文件的符号。

RustCodeGraph 的精确 `node` 证据确认 `IsMatch` 调用 `Name`，且 `Name` 的调用方包含 `IsMatch`。由于代码图的通用 `callees` 对同名符号会返回跨文件模糊匹配，上述应用级调用点另用限定 `*.rs` 路径的文本搜索并通过 RustCodeGraph 文件节点复核，未采用模糊结果作为调用边。

## 错误处理与边界

`Name` 和 `IsMatch` 都是总定义纯函数，不返回 `Result`/`Option`，不 panic，不记录日志。它们只返回内核名称或布尔结果；“不匹配是否构成错误”由上层决定。服务器启动链在 `IsMatch == false` 时构造 `kernel type mismatch` 错误，这不是本文件内部的错误类型。

需要保留的输入边界有：

- `""`：仅在 Classic 构建下返回真，表示兼容没有 kernel type 字段的旧 PD。
- `"Classic"` 与 `"Next Generation"`：只有与当前构建规范名称完全一致时返回真。
- `"Unknown"`、`"classic"` 及其他大小写/空白/别名变体：返回假。当前实现没有正规化步骤，不应在文档或调用方声称支持这些输入。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、网络连接或堆资源。两个常量具有整个程序生命周期，`Name` 只返回对它们之一的 `&'static str`，所以无借用逃逸、释放顺序或争用问题。

`IsNextGen` 的实现在编译期二选一，运行期不存在可被并发更改的内核类型开关。因此 `Name` 和 `IsMatch` 可以在多线程中并发调用，不需要额外同步；调用方自身持有的 PD 客户端、storage 或 Domain 生命周期不由本文件管理。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/config/kerneltype/type.go`：两个规范名称、`IsMatch`的空值分支、非空值与 `Name` 直接比较，以及 `Name` 根据 `IsNextGen` 二选一的控制流程均保持一致。Go 返回 `string`，Rust 借助常量返回 `&'static str`，这是所有权/分配模型上的语言差异，不改变字符串内容或比较语义。

Go 通过 `classic.go`/`nextgen.go` 的 build tag 选择 `IsNextGen`；Rust 通过 Cargo `nextgen` feature 和 `#[cfg]` 条件模块/导入达到同样的“不同形态产生不同二进制”效果。`pkg/config/kerneltype/doc.go` 明确说明不支持在同一部署中混用不同 kernel type 的组件，这是启动链使用 `IsMatch` 阻止混连的背景。

Go 测试 `type_test.go::{TestKernelType, TestIsMatch}` 的核心断言已在独立 Rust 测试 `type_test.rs::{test_kernel_type, test_is_match}` 中保留。Rust 还有 `migration_aster_unit_test.rs`，补充验证 feature 与名称的对应、`IsMatch(Name())`、大小写敏感和两种内核对空值/对方名称的全部矩阵。

## 扩展指南

- 若变更 PD 与 TiDB/AsterSQL 共享的名称字面值，修改点是 `classicKernelName`/`nextgenKernelName`；必须同步核对 PD 协议、`type.go` 对照实现、启动信息展示以及两个 Rust 独立测试文件。任意单边改名都有导致启动期错误拒绝或混部署判定失效的兼容风险。
- 若调整旧 PD 兼容策略，修改 `IsMatch` 的空字符串分支，并在 `type_test.rs` 与 `migration_aster_unit_test.rs` 分别覆盖 Classic 和 NextGen。删除该特例会影响旧 PD 连接，必须经过明确的协议兼容性决策。
- 若新增第三种内核形态，仅在本文件增常量不够；还需扩展 `Name` 的选择模型、`lib.rs` 的条件模块与重导出、对应形态实现、Cargo features，并重新定义“形态互斥”测试。当前 `IsClassic == !IsNextGen` 的二值不变量无法直接表示三态。
- 若希望接受大小写不同、首尾空白或别名，接入点是 `IsMatch`，但这是 PD 协议语义扩张，不应只为让某个调用方通过而宽松比较。需增加独立回归用例并评估错误接受不同内核类型的安全风险。

本文件不存在性能热点式复杂路径；扩展的首要风险是协议和部署兼容性，其次才是字符串比较本身。Rust 测试逻辑应继续保持在同目录的独立 `*_test.rs` 文件中，不应内嵌到 `type.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/config/kerneltype` 列出本 crate 的 Rust/Go 对照文件。
- RustCodeGraph 源码/符号证据：`node --file pkg/config/kerneltype/type.rs`；`node pkg/config/kerneltype/type.rs::IsMatch`；`node pkg/config/kerneltype/type.rs::Name`。精确节点显示 `IsMatch -> Name`，`Name` 节点显示来自 `IsMatch` 的调用边。
- crate 与条件编译证据：`pkg/config/kerneltype/Cargo.toml`、`pkg/config/kerneltype/lib.rs`、`pkg/config/kerneltype/classic.rs`、`pkg/config/kerneltype/nextgen.rs`。
- 包语义与 Go 对照证据：`pkg/config/kerneltype/doc.go`、`pkg/config/kerneltype/type.go`、`pkg/config/kerneltype/type_test.go`。
- Rust 测试证据：`pkg/config/kerneltype/type_test.rs`、`pkg/config/kerneltype/migration_aster_unit_test.rs`。前者保留 Go 测试的主要断言，后者增加完整匹配矩阵和 feature/名称一致性检查。
- 应用调用点证据：`cmd/tidb-server/main.rs::createStoreDDLOwnerMgrAndDomain`、`cmd/tidb-server/stubs.rs::Storage::{new, from_registered}`、`pkg/util/printer/printer.rs::{PrintTiDBInfo, GetTiDBInfo}`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付时使用任务指定的结构命令，验证目标文件存在且恰好包含 11 个固定二级章节。
