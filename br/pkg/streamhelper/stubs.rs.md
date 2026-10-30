# `br/pkg/streamhelper/stubs.rs`

## 文件定位

streamhelper 的本地外部系统边界与元数据模型；包含 MemEtcd、PD/TiKV 接口替身及可持久化任务配置。

## 核心职责

为既有任务元数据读写、watch、region/checkpoint 边界提供类型与内存 KV；不实现生产 etcd/gRPC 连接。

## 主要符号

StorageBackend、StreamBackupTaskInfo、StreamBackupError、EtcdKV、MemEtcd、WatchContext。

## 执行流程

任务经 MetaDataClient::PutTask 的既有 serde JSON 编码写 KV；新客户端 GetTask 解码，暂停/恢复沿用任务状态键。

## 数据与状态

StorageBackend 保留 Uri，新增 serde 默认的可选 S3 完整配置；省略 None 以兼容既有 JSON。S3.context 保存一次编码结果。

## 依赖与调用关系

可选 S3 类型复用 objstore::parse::S3，避免重建配置子系统；client.rs/models.rs 负责实际元数据生命周期。

## 错误处理与边界

既有 KV 错误与 JSON 错误继续向上传递。老记录缺失 S3 时默认为 None，不能把缺字段误当非法记录。

## 并发与资源生命周期

MemEtcd 共享状态由既有锁与 Arc 管理，watch/context 沿用原取消机制；本增量不增加线程或连接。

## 与 Go 版本的对应关系

cb395494e6 streamhelper/integration_test.go 的最小持久化接线：保存完整 S3 配置并在重读及暂停/恢复后保留 context；独立测试只替换外部 etcd 边界，真实 serde 编解码不作桩。

## 扩展指南

本文件部分能力仍是本地边界。不得将此模型描述为完整 kvproto/生产网络实现；新增持久化字段须兼容旧记录并验证往返。
