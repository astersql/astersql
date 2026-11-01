# executor.rs

## 文件定位

`pkg/mvservice/executor.rs` 属于新增物化视图服务框架。

## 核心职责

有界并发的物化视图任务执行队列。

## 主要符号

TaskExecutor、Task、TaskQueue、Metrics。

## 执行流程

Submit 在互斥锁内登记等待任务与计数；Run 启动工作线程。工作线程在锁外检查背压，取得任务后按当前超时执行；超时立即释放工作槽位但任务后台继续。Close 丢弃排队请求、唤醒所有线程并等待后台任务与工作线程退出。

## 数据与状态

Init/Running/Closed 生命周期；动态并发上限与超时；submitted/finished/failed/timeout/rejected/backpressure 计数，以及 running/waiting/timed_out_running/backpressure_blocked 瞬时指标。

## 依赖与调用关系

直接使用 backpressure 控制器、time_proxy 时钟与 server_maintainer::Cancellation；VecDeque 提供真实环形 FIFO；crossbeam select 同时等待完成、定时与关闭。

## 错误处理与边界

保留来源提交的错误分支与空状态语义。独立测试在 `commit_7e84fb645a_test.rs`，包含真实入口调用；外部探针与服务发现可替换，内部队列、哈希和任务执行均为实际实现。

## 并发与资源生命周期

执行器显式 close 等待全部工作线程与未结束任务；Arc 保持执行期间对象存活；控制器调用在队列锁和控制器锁之外，Close 通过广播取消打断背压等待。

## 与 Go 版本的对应关系

来源 `7e84fb645ad620aed44570985a64efde83a85db9`：executor.go 全部新增 hunk；Go goroutine 映射为拥有 Arc 的线程，panic 转错误并记录 Rust backtrace；超时不是取消任务。

本提交只导出 crate 框架，和 Go 来源一样尚未接入后续后台调度；不宣称 SQL 生产调度调用已经存在。独立测试通过 crate 的实际入口验证行为。PostgreSQL 方言、协议、类型与事务实现未改动。

## 扩展指南

后续提交可通过公开入口接入调度；先保持本提交测试与 Go 契约，避免递归引入其他服务子系统。修改业务源文件时同步本说明和文档清单。
