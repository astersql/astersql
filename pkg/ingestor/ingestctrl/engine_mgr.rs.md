# `pkg/ingestor/ingestctrl/engine_mgr.rs`

## 文件定位

本地排序引擎的生命周期管理，供 local.rs::Backend 使用。外部引擎接口仍放在此模块供 native adapter 复用，但管理器不再持有外部引擎。

## 核心职责

准备排序目录，维护本地 Engine 注册表，分配 TSO，提供写入、锁、统计、重置和清理。外部引擎的创建与关闭归属于调用者。

## 主要符号

`EngineManager` 保存 BackendConfig、StoreHelper、本地 engines、共享重复 KV 和 KeyAdapter。`ExternalEngine` 是数据加载、资源调节、统计和范围接口；不提供注册表。`newEngineManager`、openEngine、closeEngine、resetEngine、cleanupEngine、close 是主要生命周期入口。

## 执行流程

构造时准备目录并选择重复检测 KeyAdapter；openEngine 幂等创建本地 Engine 并写入 TSO。closeEngine 对本地 Engine 加锁、结束写入并关闭；resetEngine 对未知 ID 成功 no-op，对已知 ID 关闭、移除后重建。cleanupEngine 移除本地项并清目录，close 清理全部本地项及排序根目录。

## 数据与状态

唯一注册表为 Mutex<HashMap<EngineId, Arc<Engine>>>，返回句柄时克隆 Arc。TS 由物理时间左移 18 位并加逻辑时间合成；重复 KV 缓冲与 KeyAdapter 由本地引擎共享。closed 原子位使管理器关闭幂等。不存在 external_engines 或外部统计/查找状态。

## 依赖与调用关系

local.rs 的 OpenEngine、CloseEngine、CleanupEngine、LocalWriter 和 reset/close 入口调用本管理器；ImportEngine 仅通过 lockEngine 获取本地引擎。ExternalEngineSource 在 import_pipeline.rs 消费调用者传入的 ExternalEngine，绕过管理器。

## 错误处理与边界

未知 close 返回 NotFound，未知 reset/cleanup 保留 no-op 契约。TSO 错误传播；清理先摘除注册项，关闭/目录错误不会恢复旧项。close 尽力清理本地资源并忽略单项错误。既有手工锁的提前错误路径属于独立历史风险，本次不扩展修复。

## 并发与资源生命周期

注册表锁只保护表操作，单个引擎的 import/close 状态由 Engine 锁控制。close 通过 swap 保证幂等；并发取得的 Arc 仍依赖引擎自身状态。关闭后端不会关闭调用者持有的外部引擎，也不会在表锁内执行外部 Close 回调。

## 与 Go 版本的对应关系

对齐 373679dd4df08d656e3a95ff0cc0cd245152a9ff 的 engine_mgr.go 10 个 hunk：删除外部表、构造初始化、注册/查找/统计、cleanup 和 close 接管。旧 external close/reset 分支不再存在。Go buffer pool、checkpoint、磁盘 DB 的既有差异未在本增量扩建。

## 扩展指南

本地生命周期继续复用独立 engine_mgr_test.rs；外部引擎测试应直接调用 Backend::IngestExternalEngine，验证 backend close 不接管、调用者重复 close 安全。不得为便利重新引入按 ID 的外部引擎表。
