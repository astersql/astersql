# `pkg/plugin/audit.rs`

## 文件定位

`pkg/plugin/audit.rs` 属于 `astersql-plugin` crate（见 `pkg/plugin/Cargo.toml`），由 crate 根 `pkg/plugin/lib.rs` 以 `pub mod audit` 声明并用 `pub use audit::*` 对外重导出。它是审计插件的 SPI（Service Provider Interface）模型层：定义审计事件、传给回调的最小会话快照、四类回调签名、专用清单，以及审计上下文的类型化键；它本身不负责插件发现、生命周期调度或遍历执行。

通用 `Manifest`、`Context`、`ContextKey` 和 `ExportManifest` 位于 `pkg/plugin/spi.rs`，加载和 `Ready` 状态管理位于 `pkg/plugin/plugin.rs`。当前可确认的 Rust 应用主链接线位于 `pkg/session/runtime/scan_adapter_runtime.rs` 的 `Audit` 实现：该路径遍历 `Kind::Audit` 插件并触发 `on_general_event`。连接、全局变量和解析回调在本文件中有完整类型定义，但本次搜索未发现对应的非测试 Rust 主链调用点；不能据此宣称它们已经像 Go 版本一样全部接线。

## 核心职责

1. 用 `GeneralEvent`、`ConnectionEvent` 和 `ParseEvent` 建模语句、连接与解析阶段事件，并保持 Go `pkg/plugin/audit.go` 的数值和字符串约定。
2. 用 `ConnectionInfo`、`TableEntry` 和 `SessionVars` 提供与 Go 回调所读字段对应、但刻意收窄的 Rust 数据视图，隔离插件与完整 session 内部对象。
3. 用四个 `Arc<dyn Fn ... + Send + Sync>` 回调别名规定跨插件共享、可并发调用的 ABI 内部契约。
4. 通过 `AuditManifest::export_manifest` 把审计专用回调封装进通用 `Manifest.extension`，再由 `audit_callbacks` 和 `pkg/plugin/helper.rs::declare_audit_manifest` 恢复。
5. 通过实现 `ContextKey` 的零大小键，将拒绝原因、执行开始时间、预处理语句 ID 和重试状态以确定的 Rust 类型传入插件回调。

## 主要符号

- `GeneralEvent`：`#[repr(u8)]` 枚举，`Starting = 0`、`Completed = 1`、`Error = 2`；`COUNT = 3` 是合法值数量。`as_str`/`Display` 输出固定大写字符串，`from_u8` 对越界值返回 `None`。
- `general_event_from_string` 与 `FromStr for GeneralEvent`：先做 ASCII 大写归一化，再匹配三个事件；非法输入返回 `PluginErrorKind::Backend`，消息为 `Invalid general event: <原值>`。
- `ConnectionEvent(pub u8)`：保留 Go `type ConnectionEvent byte` 可承载未知数值的性质。五个关联常量对应 0 至 4；`as_str`/`Display` 对未知值返回空串。
- `ParseEvent`：`#[repr(u8)]`，从 1 开始的 `PreParse`、`PostParse`，与 Go 的 `1 + iota` 对齐。
- `ConnectionInfo`：连接回调视图，包含 `user`、`host`、`database`、`connection_type`。
- `TableEntry` 与 `SessionVars`：语句审计视图。`SessionVars` 包含连接 ID、原始 SQL、语句类型、影响行数、涉及表、协议状态和用户名。
- `ConnectionEventCallback`、`GeneralEventCallback`、`GlobalVariableEventCallback`、`ParseEventCallback`：四类可选回调。连接和解析回调返回 `Result<(), PluginError>`；通用事件和全局变量回调无返回错误通道。
- `AuditManifest`：组合基础 `Manifest` 与四个可选回调；`Default` 生成默认基础清单且所有审计回调均为空。
- `AuditCallbacks`：写入 `Manifest.extension` 的类型擦除载荷。`audit_callbacks` 使用 `Any::downcast_ref` 恢复它，类型不匹配或无扩展时返回 `None`。
- `RejectReasonContextKey`、`ExecStartTimeContextKey`、`PrepareStatementIdContextKey`、`IsRetryingContextKey`：分别约束值类型为 `String`、`SystemTime`、`u32`、`bool`。后三者另有大写常量单例；拒绝原因键可直接以零大小值构造。

## 执行流程

清单导出流程如下：插件构造 `AuditManifest`，框架经 `ExportManifest::export_manifest` 克隆基础 `Manifest`，再克隆四个 `Option<Arc<...>>` 到 `AuditCallbacks`，最后将其作为 `Arc<dyn Any + Send + Sync>` 写入 `manifest.extension`。加载器仍只处理统一的 `Manifest`，不会丢失审计专用回调。

恢复流程有两种直接形态。调用方可对已加载清单调用 `audit_callbacks(&manifest)`，直接借用 `AuditCallbacks`；也可调用 `pkg/plugin/helper.rs::declare_audit_manifest`，该函数克隆恢复出的各回调并重新组成 `AuditManifest`。若扩展缺失或实际类型不是 `AuditCallbacks`，恢复出的四个回调都为 `None`，基础清单仍被保留。

当前 Rust 语句完成路径在 `pkg/session/runtime/scan_adapter_runtime.rs:1300-1356`：运行时从 session 与 statement context 组装 `TableEntry`/`SessionVars`，构造包含 `EXEC_START_TIME_CONTEXT_KEY` 和 `IS_RETRYING_CONTEXT_KEY` 的 `Context`，把命令码映射为名称，然后用 `foreach_plugin(Kind::Audit, ...)` 遍历已加载审计插件。若扩展中存在 `on_general_event`，则以 `GeneralEvent::Completed` 调用；遍历错误被记录为 `audit_error:<error>`，最后记录被审计 SQL。

事件文本解析是另一条独立流程：`general_event_from_string` 委托 `str::parse`，`FromStr` 做大小写不敏感匹配并返回枚举或插件错误。它不参与回调分发。

## 数据与状态

本文件的事件和值对象均为拥有型数据。`ConnectionInfo` 和 `SessionVars` 使用 `String`/`Vec<TableEntry>`，因此运行时需要从完整 session 状态复制一份稳定快照；回调只借用该快照，不能反向修改 session。`SessionVars` 的 `Default` 是全零/空集合，调用方必须区分“真实空值”和“尚未填充”；回调参数本身又是 `Option<&SessionVars>`，允许根本没有会话上下文的事件。

回调以 `Arc` 持有，`AuditManifest::clone`、导出和恢复只增加共享引用计数，不复制闭包捕获状态。`AuditCallbacks` 作为 `Manifest.extension` 的唯一审计载荷，依赖运行时类型标识完成 downcast；同一 `extension` 槽不能同时直接保存另一种扩展类型。

类型化上下文值实际由 `pkg/plugin/spi.rs::Context` 按键类型的 `TypeId` 存入 `Arc<HashMap<...>>`。`with_value` 写时复制值表，派生上下文与父上下文共享取消状态；同一种键类型的新值遮蔽旧值。注意 `Context` 另有内部原子 `is_retrying` 状态，而本文件的 `IsRetryingContextKey` 是值表中的 `bool` 键；审计调用方应按既有接口使用后者，不能把两条状态通道视为自动同步。

## 依赖与调用关系

本文件只依赖标准库的 `FromStr`、`Arc`、`SystemTime`，以及同 crate 的 `Context`、`ContextKey`、`ExportManifest`、`Manifest`、`PluginError`、`PluginErrorKind`。`pkg/plugin/Cargo.toml` 没有运行时第三方依赖，只有测试用 `serial_test`。

已验证的上游包括：

- `pkg/plugin/helper.rs::declare_audit_manifest` 调用 `audit_callbacks`，完成通用清单到审计清单的恢复；`load_plugin_for_test` 构造并导出测试审计清单。
- `pkg/session/runtime/scan_adapter_runtime.rs::Audit` 直接调用 `audit_callbacks` 并触发 `on_general_event(Completed, ...)`。
- `pkg/plugin/conn_ip_example/conn_ip_example.rs::plugin_manifest` 构造 `AuditManifest`，其示例回调消费 `GeneralEvent`、`ConnectionEvent`、连接/会话视图与拒绝原因键。

主要下游关系是：`AuditManifest::export_manifest` 写入 `Manifest.extension`；事件解析构造 `PluginError`；上下文键通过 `ContextKey::Value` 约束 `Context::with_value/value` 的编译期类型。插件加载、状态筛选与遍历不由本文件实现，而由 `pkg/plugin/plugin.rs` 提供。

## 错误处理与边界

`GeneralEvent` 的数值恢复是显式有界的：只有 `0..COUNT` 有效。字符串解析仅接受三个事件名（ASCII 大小写不敏感），不修剪空白；空串、未知名称和带额外空白的输入都会返回 `PluginErrorKind::Backend`。错误消息保留原始输入，便于定位配置值。

`ConnectionEvent` 为透明的 `u8` 包装，因此允许未知值存在；未知值格式化为空串而不是报错，这与 Go 对任意 `byte` 强制转换后的 `String()` 行为一致。`ParseEvent` 没有公开的数值解析函数，调用方应使用枚举成员而不是自行解释任意整数。

`audit_callbacks` 不把 extension 类型不匹配当错误，而是返回 `None`；`declare_audit_manifest` 因而会静默得到空回调。新增扩展或修改载荷类型时，必须同步导出和恢复两端，否则插件会表现为“已加载但没有审计回调”。

回调错误语义由签名限定：连接/解析回调能返回 `PluginError` 以拒绝或中止操作；通用事件/全局变量回调不能直接传播错误。当前 Rust 语句完成路径只会捕获 `foreach_plugin` 闭包外围的错误，而 `GeneralEventCallback` 本身没有错误返回值。

## 并发与资源生命周期

所有回调 trait object 都要求 `Send + Sync` 并由 `Arc` 管理，因此可以被清单、加载器和调用方安全共享。该约束只保证闭包对象可跨线程共享，不保证插件内部业务状态天然无竞态；闭包若捕获可变状态，仍需自行使用 `Mutex`、原子量或其他同步原语。

清单导出、`audit_callbacks` downcast 和事件格式化不创建线程、不持锁、不执行 I/O。回调捕获资源的生命周期随最后一个 `Arc` 释放而结束；`AuditManifest`、`AuditCallbacks` 和通用 `Manifest.extension` 的克隆都会延长生命周期。基础插件的 load/init/shutdown 状态与全局集合由 `pkg/plugin/plugin.rs` 管理，本文件不负责取消或清理回调资源。

`Context` 由 `Arc` 支撑，派生上下文共享取消标记；类型化值是不可变快照。`SystemTime` 记录执行起点，`u32` 保存 prepare ID，`bool` 保存重试标记，均无需额外资源回收。运行时传给回调的 `&SessionVars` 和 `&ConnectionInfo` 只在调用期间有效，插件若需异步保留必须显式克隆拥有型值。

## 与 Go 版本的对应关系

`pkg/plugin/audit.go` 是直接语义基准。Rust 的 `GeneralEvent` 数值、`GeneralEventCount`（对应 `COUNT`）、字符串输出和大小写不敏感解析与 Go 一致；Rust 通过封闭枚举加 `from_u8` 拒绝未知数值，而 Go 的 byte 别名本身可承载任意值。`ConnectionEvent` 刻意使用 `u8` 新类型，保留 Go 的未知值边界和空字符串格式化结果。`ParseEvent` 从 1 开始，对齐 Go 的 `PreParse ParseEvent = 1 + iota`。

Go `AuditManifest` 直接嵌入 `Manifest` 并保存函数字段；Rust 使用 `manifest: Manifest` 字段和 `Arc` 回调，并因通用加载接口需要，把专用回调装入 `Manifest.extension`。这是表达方式差异，`pkg/plugin/spi_test.rs::test_export_manifest` 与 `pkg/plugin/helper_test.rs::test_plugin_declare` 验证了导出/恢复行为。

Rust `ConnectionInfo`、`SessionVars` 是 Go `variable.ConnectionInfo` / `variable.SessionVars` 的最小审计视图，不是完整结构体复刻。Go 主链在 `pkg/server/server.go` 触发连接事件，在 `pkg/executor/set.go` 触发全局变量事件，并在 `pkg/server/conn.go`、`pkg/executor/adapter.go` 触发通用事件；当前 Rust 非测试搜索只确认 session adapter 的 `Completed` 通用事件接线。因此四类回调的类型兼容已建立，不代表所有 Go 触发点均已迁移。

上下文键按 Go 值类型映射：`RejectReasonCtxValue{}` → `RejectReasonContextKey/String`，`ExecStartTimeCtxKey` → `SystemTime`，`PrepareStmtIDCtxKey` → `u32`，`IsRetryingCtxKey` → `bool`。Rust 用 `ContextKey` 在编译期绑定键和值类型，比 Go `context.Value` 的运行时断言更严格。

## 扩展指南

新增通用语句事件时，应在 `GeneralEvent` 的既有成员之后、计数边界之前加入成员，同步 `COUNT`、`as_str`、`from_u8` 和 `FromStr`，并更新 `pkg/plugin/const_test.rs` 的字符串、遍历与非法输入测试；同时核对 Go `GeneralEvent` 的数值顺序，避免持久化配置或插件判断发生兼容破坏。

新增回调类别或修改回调签名时，必须成对修改 `AuditManifest`、`AuditCallbacks`、`ExportManifest::export_manifest` 和 `pkg/plugin/helper.rs::declare_audit_manifest`，再扩展 `pkg/plugin/spi_test.rs` 的往返与实际调用断言。还要定位真实生产触发点，不能仅新增类型便宣称功能可用。新回调继续需要 `Send + Sync`；若进入高频 SQL 路径，应评估快照复制、锁竞争和插件耗时。

扩展 `SessionVars`/`ConnectionInfo` 字段时，应在构造它们的运行时适配器同步赋值，并更新 `pkg/session/runtime_test/typed_adapter_bridge.rs` 等独立测试验证真实值，而不是只依赖 `Default`。字段类型或事件数值属于插件兼容面，修改前应核对 Go 字段语义、空值含义和已有插件消费方式。

新增审计上下文值时，应定义独立零大小键并实现 `ContextKey`，明确唯一的 `Value` 类型；常用键可提供大写常量单例。测试应放在独立的 `pkg/plugin/audit_test.rs` 或 `pkg/plugin/spi_test.rs`，验证父子上下文、遮蔽和类型正确性，不应把测试嵌入生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/plugin` 确认 `audit.rs` 及其 Go/测试邻接文件；`node --file pkg/plugin/audit.rs` 核对全部 271 行、事件、回调、导出实现和上下文键；`query AuditManifest`、`query GeneralEvent`、`query audit_callbacks` 定位 Rust/Go 定义及辅助入口。
- RustCodeGraph 源码节点：`pkg/plugin/helper.rs:31-46` 验证回调恢复；`pkg/session/runtime/scan_adapter_runtime.rs:1300-1356` 验证 `Completed` 事件、session 快照、上下文值和错误记录；`pkg/plugin/spi.rs:33-81,98-125` 验证类型化 `Context` 与 `Manifest.extension`；`pkg/plugin/plugin.rs` 验证加载/生命周期职责位于其他模块。
- crate 与模块边界：`pkg/plugin/Cargo.toml`、`pkg/plugin/lib.rs`。
- Go 对照与调用面：`pkg/plugin/audit.go`，以及搜索到的 `pkg/server/server.go`、`pkg/server/conn.go`、`pkg/executor/adapter.go`、`pkg/executor/set.go`。
- Rust 独立测试：`pkg/plugin/audit_test.rs` 验证三个常量键的值类型；`pkg/plugin/const_test.rs` 验证事件字符串、数值往返和非法解析；`pkg/plugin/spi_test.rs` 验证上下文派生/遮蔽/取消传播及清单往返；`pkg/plugin/helper_test.rs` 验证 declare/export；`pkg/plugin/conn_ip_example/conn_ip_example_test.rs` 验证加载后回调；`pkg/session/runtime_test/typed_adapter_bridge.rs` 验证生产 adapter 传入规范 session 视图。
- 本任务是纯文档分析，按计划未运行 Cargo。最终结构检查要求本文恰好包含规定的十一个二级标题。
