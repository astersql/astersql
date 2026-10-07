# `pkg/domain/infosync/schedule_manager.rs`

## 文件定位

本文件是 `astersql-domain-infosync` crate 内的 PD 调度配置访问层。模块由 `pkg/domain/infosync/lib.rs` 以 `mod schedule_manager` 声明并完整再导出，因此 `ScheduleManager`、`PDScheduleManager` 和 `mockScheduleManager` 都属于该 crate 的公共接口。crate 的边界和 Go 来源分别由 `pkg/domain/infosync/Cargo.toml` 中的包名与 `[package.metadata.porting].go-package = "pkg/domain/infosync"` 声明。

它位于 `InfoSyncer` 与 PD HTTP 客户端之间：`pkg/domain/infosync/info.rs` 的 `InfoSyncer::scheduleManager` 保存 `Arc<dyn ScheduleManager>`，公开函数 `GetPDScheduleConfig` 和 `SetPDScheduleConfig` 通过这个 trait object 转发调用。文件自身不负责全局单例初始化、HTTP 编解码或具体网络传输。

## 核心职责

1. `ScheduleManager` 统一定义“读取全部 PD schedule 配置”和“合并写入一批配置”两个操作，使调用方不依赖真实 PD 或内存替身。
2. `PDScheduleManager` 把两个操作原样委托给 `PdHttpClient::get_schedule_config` 与 `PdHttpClient::set_schedule_config`，是生产路径的薄适配器。
3. `mockScheduleManager` 用受读写锁保护的内存映射实现同一契约；读取返回快照，写入按键合并，供未注入 PD 客户端的 `InfoSyncer` 使用。

配置值不是字符串专用：键为 `String`，值为 `pkg/domain/infosync/types.rs` 定义的 `ConfigValue`，可表示布尔、数字、字符串、数组、对象和空值，语义上对应 PD 接口的异构 JSON 字段。

## 主要符号

- `pub trait ScheduleManager: Send + Sync`：并发可共享的抽象接口。`GetScheduleConfig(&self) -> Result<HashMap<String, ConfigValue>>` 返回配置所有权；`SetScheduleConfig(&self, &HashMap<...>) -> Result<()>` 借用输入，不消费调用方数据。
- `pub struct PDScheduleManager { pub Client: Arc<dyn PdHttpClient> }`：真实实现。公开字段允许初始化代码注入共享的 PD 客户端；`Arc` 使管理器和其他子管理器可以复用同一客户端。
- `impl ScheduleManager for PDScheduleManager`：读取调用 `Client.get_schedule_config()`，写入调用 `Client.set_schedule_config(config)`，不改写返回值或错误。
- `pub struct mockScheduleManager`：内存实现，唯一状态为私有的 `RwLock<HashMap<String, ConfigValue>>`。`Default` 同时创建空映射和未加锁状态。
- `impl ScheduleManager for mockScheduleManager`：读取在读锁内克隆整个映射；写入在写锁内以 `HashMap::extend` 合并克隆的输入。

文件没有模块级常量、自由函数、条件编译项或异步任务。

## 执行流程

生产对象的选择发生在 `pkg/domain/infosync/info.rs::GlobalInfoSyncerInit`：若参数 `pdHTTPCli` 为 `Some(client)`，构造 `PDScheduleManager { Client: client }`；否则构造 `mockScheduleManager::default()`。选出的对象被擦除为 `Arc<dyn ScheduleManager>` 并存入 `InfoSyncer::scheduleManager`。

读取链路为：调用者进入 `pkg/domain/infosync/info.rs::GetPDScheduleConfig`，该函数先用 `getGlobalInfoSyncer()` 取得全局 `InfoSyncer`，再调用 trait 的 `GetScheduleConfig`。真实实现继续调用 `PdHttpClient::get_schedule_config`；mock 实现取得读锁、克隆当前映射、释放锁并返回快照。

写入链路为：调用者把映射传给 `pkg/domain/infosync/info.rs::SetPDScheduleConfig`，后者转发给 trait 的 `SetScheduleConfig`。真实实现把相同引用交给 PD 客户端；mock 实现取得写锁，把输入中的键值克隆进内部映射，然后释放锁。`extend` 对不存在的键执行插入，对已存在的键执行覆盖，但不会删除输入中未出现的旧键。

## 数据与状态

真实实现只持有 `Arc<dyn PdHttpClient>`，本文件不缓存服务端配置；每次读写都依赖客户端当前行为。`PdHttpClient` 的默认 schedule 方法位于 `pkg/domain/infosync/types.rs`，若具体客户端没有覆写，会返回 `Error::External("GetScheduleConfig is unsupported")` 或相应的 set 错误，因此“存在客户端对象”不等于“schedule API 一定可用”。

mock 的持久状态是进程内 `HashMap<String, ConfigValue>`。读取结果是深度克隆后的独立快照：调用方随后修改结果不会影响管理器内部状态。写入同样克隆配置值，调用方在返回后修改原映射也不会改变已保存内容。状态生命周期与持有该 mock 的 `Arc<dyn ScheduleManager>` 一致，不落盘、不过期，也不会自动与 PD 同步。

配置合并的不变量是：一次成功写入后，输入中的每个键都映射到本次输入值；其他既有键保持不变。空映射写入成功但不改变状态。

## 依赖与调用关系

- 上游装配：`pkg/domain/infosync/info.rs::GlobalInfoSyncerInit` 构造真实或 mock 实现，并存入 `InfoSyncer::scheduleManager`。
- 上游公共入口：`pkg/domain/infosync/info.rs::GetPDScheduleConfig` 与 `SetPDScheduleConfig` 是目前 Rust 仓库中对这两个方法的直接转发点。源代码搜索未发现其他 Rust 生产调用点。
- 下游真实边：`PDScheduleManager::GetScheduleConfig -> PdHttpClient::get_schedule_config`，`PDScheduleManager::SetScheduleConfig -> PdHttpClient::set_schedule_config`。
- 下游 mock 边：标准库 `RwLock::{read,write}`、`HashMap::clone` 与 `HashMap::extend`。
- 类型与错误：`ConfigValue`、`PdHttpClient`、`Result` 和 `Error` 经 crate 根重新导出后由 `use crate::{...}` 引入。

`Cargo.toml` 没有为本文件设置 feature 开关。直接使用的集合、引用计数与锁均来自标准库；PD 客户端的具体实现能力属于 crate 的 `PdHttpClient` 边界，而不在本文件内。

## 错误处理与边界

真实实现不捕获、不包装也不重试客户端错误，`PdHttpClient` 返回的 `infosync::Error` 原样向上传播。全局同步器未初始化的错误发生在 `info.rs` 的公共包装函数中，尚未进入本文件。

mock 的正常数据操作总是返回 `Ok`，但锁中毒时使用 `unwrap()` 会 panic，而不是转换为 `infosync::Error`。这意味着持锁线程若 panic，后续读写可能终止当前线程。代码也不验证键名、数值范围或 PD 业务约束；真实路径把这些校验留给客户端/PD，mock 路径则接受任意 `String` 与 `ConfigValue`。

`SetScheduleConfig` 是局部更新而非全量替换：缺席键不会被清除。这一点是扩展或调用时的重要边界。文件也没有上下文或取消参数；与 Go 接口相比，Rust 路径不能通过该 trait 直接传递请求截止时间或取消信号。

## 并发与资源生命周期

`ScheduleManager: Send + Sync`、trait object 外层的 `Arc` 以及 mock 内部的 `RwLock` 共同允许跨线程共享。mock 可并发执行多个读取；写入与其他读写互斥。读锁覆盖整个映射克隆过程，保证快照来自一个一致的锁内状态；写锁覆盖完整的批量 `extend`，因此其他线程不会观察到同一次批量写入的中间状态。

锁是阻塞式 `std::sync::RwLock`，没有公平性、超时或取消保证。大映射读取需要在持锁期间深度克隆全部值，大批量写入也会在独占锁内克隆输入，可能延长等待时间。真实实现的并发安全由 `PdHttpClient: Send + Sync` 契约保证，本文件不另行串行化网络调用。`Arc` 释放最后一个引用时管理器与客户端引用自然销毁；没有显式 `close`、后台线程或清理协议。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/infosync/schedule_manager.go`。两边都包含 `ScheduleManager`、`PDScheduleManager`、`mockScheduleManager`，真实实现都依赖 PD 客户端提供接口方法，mock 都以锁保护映射、读取复制、写入合并且覆盖同名键。

主要差异如下：

- Go 的方法接收 `context.Context`，Rust trait 没有上下文参数，因此取消与超时语义没有逐项移植。
- Go 用 `map[string]any`，Rust 用可序列化的封闭枚举 `ConfigValue`，可表达常见 JSON 值但不能承载任意运行时 Go 值。
- Go 的 `PDScheduleManager` 嵌入 `pd.Client`，接口方法由嵌入类型提升；Rust 显式实现 trait 并委托给 `PdHttpClient`。
- Go mock 在首次写入时延迟初始化 nil map；Rust 的 `Default` 立即创建空 `HashMap`。
- Go mock 的读取使用独占 `Lock`，Rust 使用共享 `read`；两者都在复制期间持锁，但 Rust 允许并行读取。
- Go 的 `InfoSyncer::initScheduleManager` 完成实现选择；Rust 当前在 `GlobalInfoSyncerInit` 内直接选择实现，而同名 `initScheduleManager` 是空占位，不能把该占位描述为实际装配入口。

Go 回归测试 `pkg/ddl/cluster_test.go::TestFlashbackCloseAndResetPDSchedule` 通过公共包装函数先保存 `merge-schedule-limit = 1`，在 flashback 期间观察其变为 `0`，取消后再确认恢复为 `1`，证明这组接口承担 DDL 暂时修改并恢复 PD 调度参数的用途。当前未找到对应的 Rust 独立回归测试。

## 扩展指南

新增一种 schedule 存储后端时，应实现 `ScheduleManager`，并在 `pkg/domain/infosync/info.rs::GlobalInfoSyncerInit` 的装配分支中选择它；若是通用 PD 能力，通常还需扩展或实现 `pkg/domain/infosync/types.rs::PdHttpClient`。不要把后端细节放进 `GetPDScheduleConfig` / `SetPDScheduleConfig` 包装函数，否则会绕过现有替换边界。

改变写入语义前必须确认 Go 兼容性：将“合并”改为“全量替换”、增加删除语义或校验都会影响 flashback 等先保存、临时覆盖、再恢复配置的流程。若要增加取消/超时，应同时设计 trait、`PdHttpClient` 和公共包装函数的参数传播，而不应只改真实实现。

测试应放在独立文件，符合本仓库 Rust 源码与测试分离规则。最接近的新增位置是新建同目录 `schedule_manager_test.rs` 并由 `lib.rs` 的 `#[cfg(test)]` 模块声明接入；至少覆盖空初态、插入、同键覆盖、保留未提交键、读取快照隔离、空写入，以及模拟 `PdHttpClient` 的成功委托和错误原样传播。涉及公共 DDL 行为时，还应与 `pkg/ddl/cluster_test.go::TestFlashbackCloseAndResetPDSchedule` 的保存/关闭/恢复意图保持一致。

兼容风险集中在 Go/Rust 上下文差异与 `ConfigValue::Number(f64)` 的数值表示；正确性风险集中在误把合并当替换、锁中毒 panic 和未实现 schedule 方法的客户端；性能风险主要是锁内全量克隆。扩展时应分别评估这些风险。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标文件被索引且数据库时间晚于本次读取的目标及直接依赖源码。
- RustCodeGraph `files --filter pkg/domain/infosync/schedule_manager.rs`：确认目标文件含 10 个索引符号。
- RustCodeGraph `node --file pkg/domain/infosync/schedule_manager.rs`：核对 trait、两个结构体、两个实现及委托/锁操作；索引报告该文件被 `pkg/domain/infosync/info.rs` 使用。
- RustCodeGraph `node --file pkg/domain/infosync/info.rs`：核对 `InfoSyncer::scheduleManager`、`GlobalInfoSyncerInit` 的实现选择以及两个公共包装函数的调用边。
- RustCodeGraph `node --file pkg/domain/infosync/types.rs`：核对 `ConfigValue` 的 JSON 值集合和 `PdHttpClient` 默认 unsupported 错误。
- 读取 `pkg/domain/infosync/Cargo.toml` 与 `lib.rs`：核对 crate 名称、Go 包映射、无 feature 门控、模块声明和再导出。
- 读取 `pkg/domain/infosync/schedule_manager.go`、`info.go`：核对 Go 接口、mock 锁/复制/合并语义、真实实现装配和上下文参数差异。
- 搜索 Rust/Go 生产及测试引用：确认 Rust 直接接线位于 `info.rs`；确认 Go 行为测试位于 `pkg/ddl/cluster_test.go::TestFlashbackCloseAndResetPDSchedule`；未发现针对该管理器或公共包装函数的 Rust 测试。
- 本任务仅产出文档，未运行 Cargo。交付前使用任务指定的命令校验恰有十一个固定二级章节，并人工复核源码链接、事实范围和 diff。
