# server_maintainer.rs

## 文件定位

`pkg/mvservice/server_maintainer.rs` 属于新增物化视图服务框架。

## 核心职责

服务成员发现与当前节点归属。

## 主要符号

ServerInfo、ServerDiscovery、ServerConsistentHash、Cancellation。

## 执行流程

init 只读取本机身份，失败按 1/2/4/5 秒上限退避直到成功或取消；refresh 拉取成员、按策略过滤、比较键集合，只在成员变化时重建排序 ID 与哈希环。

## 数据与状态

RwLock 保护身份、成员和哈希环一致快照；ServerInfo 保留 ID/IP/端口/启动时间；同 ID 的元数据变化不会触发重建，与 Go 一致。

## 依赖与调用关系

ServerDiscovery 是外部成员发现边界；hash 使用 consistenthash；退避使用 time_proxy。取消通过丢弃发送端广播给所有接收端。

## 错误处理与边界

保留来源提交的错误分支与空状态语义。独立测试在 `commit_7e84fb645a_test.rs`，包含真实入口调用；外部探针与服务发现可替换，内部队列、哈希和任务执行均为实际实现。

## 并发与资源生命周期

ServerDiscovery 是外部成员发现边界；hash 使用 consistenthash；退避使用 time_proxy。取消通过丢弃发送端广播给所有接收端。

## 与 Go 版本的对应关系

来源 `7e84fb645ad620aed44570985a64efde83a85db9`：server_maintainer.go 全部新增 hunk；i64 使用固定八字节大端编码，字符串和字节按原始字节处理，不支持的动态类型返回 false。

本提交只导出 crate 框架，和 Go 来源一样尚未接入后续后台调度；不宣称 SQL 生产调度调用已经存在。独立测试通过 crate 的实际入口验证行为。PostgreSQL 方言、协议、类型与事务实现未改动。

## 扩展指南

后续提交可通过公开入口接入调度；先保持本提交测试与 Go 契约，避免递归引入其他服务子系统。修改业务源文件时同步本说明和文档清单。
