# `br/pkg/restore/log_client/id_map.rs`

## 文件定位

把 TableMappingManager 映射包装为 BackupMeta 持久化，保存路由指纹，按当前任务或唯一前序任务读回。

## 核心职责

把 TableMappingManager 映射包装为 BackupMeta 持久化，保存路由指纹，按当前任务或唯一前序任务读回。介质选择保持检查点存储优先、系统表其次、备份存储最后；映射落盘成功后才推进检查点。

## 主要符号

`PITRIdMapBlockSize` 为 524288 字节；`PitrIDMapsFilename` 保留 legacy 名称；`PitrIDMapsFilenameForTask` 构造 v2 任务文件名；`FindUniquePitrIDMapFile` 查找唯一归属文件。LogClient 的 saveIDMap/saveIDMap2Storage/saveIDMap2Table 接收 routeFingerprint；loadSchemasMap/loadSchemasMapFromStorage/loadSchemasMapFromTable 接收 loadPreviousTask，返回 `Result<Option<BackupMeta>>`，不再仅返回 DbMaps。

## 执行流程

写路径先 ToProto，选择介质，写入带 PitrIdMapRouteFingerprint 的 BackupMeta。外部文件按目标 cluster、上游 cluster、restored TS 和 restore ID 隔离；表路径按 schema 删除同键旧行，再以 512 KiB 分段 REPLACE，最后 SaveCheckpointProgress。

读路径同样选介质。当前任务使用精确任务文件；前序任务扫描对应 v2 前缀，严格要求唯一合法 owner。表有 restore_id 且读前序时先 SELECT DISTINCT restore_id LIMIT 2，零项返回 None、一项采用、多项拒绝；之后按 segment_id 查询并拼接。旧表无该列时仍按时间戳和上游 ID 查询。

## 数据与状态

v2 路径为 `pitr_id_maps/v2/cluster-{clusterID}/upstream-{upstreamClusterID}/restored-{restoredTS}/restore-{restoreID}.meta`。legacy 路径仍为 `pitr_id_maps/pitr_id_map.cluster_id:{clusterID}.restored_ts:{restoredTS}`。空来源用 None 表达，有元数据但 DbMaps 为空仍是 Some；指纹保留到调用方检查，加载层不自行比较。

## 依赖与调用关系

`client.rs::SaveIdMapWithFailPoints` 转交指纹；`GetBaseIDMapAndMerge` 选择当前/前序任务并验证指纹后合并。Storage trait 提供 FileExists/ReadFile/WriteFile/WalkDir；本地 backuppb stub 提供 BackupMeta 编码；内部 SQL 用 unsafeSession 和 InternalTxnBR 上下文。Cargo 和 lib 证明可达，不能据此声称真实 TiDB/TiKV 集成已验证。

## 错误处理与边界

前序文件扫描拒绝非法命名、不可解析 u64、非规范重建名称或多 owner；legacy 存在时不允许选前序；续跑精确文件缺失且 legacy 存在时拒绝，绝不静默读取无归属旧文件。源不存在返回 None；访问错误上抛。表分段必须从 0 连续且非空。反序列化兼容 BM stub，额外可选长度前缀指纹段须检查溢出/截断，字节用 lossy UTF-8。checkRequirements 控制元数据兼容错误是拒绝还是告警。

## 并发与资源生命周期

同步执行，无线程或显式事务。系统表 DELETE 后多次 REPLACE 中途可能失败，后续读校验不构成原子提交保证。新表和新文件按任务隔离，旧表无 restore_id 仍可能同键覆盖。Arc Storage 仅共享所有权；进度写入失败时映射可能已成功落盘，重试会重写同键。

## 与 Go 版本的对应关系

Rust 增量增加 v2 owner 文件及前序唯一性检查，不能沿用旧说明的“文件不存在返回空 Vec”或“文件只含 cluster/TS”。Go 使用真实 protobuf/MetaWriter，Rust 使用本地 Marshal/Unmarshal 与 BM 手工还原；Go 完整集成测试不能替代 Rust 实测。

## 扩展指南

文件协议、owner 选择和指纹必须一起审查；保持“精确续跑不能改用前序”和“缺失与空映射不同”。新旧系统表键及 DELETE/REPLACE/SELECT 参数须成组维护。修改应扩展独立 id_map_test.rs、client_test.rs、parity_test.rs 和任务名称路由测试，不放源文件内。
