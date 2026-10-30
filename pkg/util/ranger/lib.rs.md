# `pkg/util/ranger/lib.rs`

## 文件定位

Ranger crate 入口，导出范围构造与比较 API。

## 核心职责

组织 types、points、checker、detacher、ranger、row_count_bound 模块，提供表达式与类型模块兼容入口。

## 主要符号

Range、Ranges、端点/范围工厂、DetachCondAndBuildRangeForIndex、RowCountUpperBoundByUniqueKey、bytes::Compare。

## 执行流程

调用者经导出 API 构造逻辑 Range，或使用唯一键证明检查其行数上界。bytes::Compare 返回字典序 -1/0/1。

## 数据与状态

无 crate 根全局可变业务状态。具体数据由各模块持有。

## 依赖与调用关系

复用 expression、model、types 和 rangerctx；模块内部依赖经 crate 根统一访问。

## 错误处理与边界

根入口不吞错误；具体范围推导和比较规则参见对应模块说明。

## 并发与资源生命周期

根模块不创建线程、I/O 或锁；资源由实际 API 与调用方管理。

## 与 Go 版本的对应关系

来源 e48ac5fef19ea30a3cae73606ca8ad27c538e7f3 新增 row_count_bound 模块的必要 Rust 装配；其余导出保持既有职责。

## 扩展指南

新增范围 API 时在此导出，并将独立测试挂入已有测试模块；不将测试与生产逻辑混放。
