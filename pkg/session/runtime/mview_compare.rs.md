# `pkg/session/runtime/mview_compare.rs` 逻辑说明

## 文件定位

canonical MySQL 会话的 COMPARE SQL 生产入口，独立于 PostgreSQL 执行分支。

## 核心职责

按当前权限和历史元数据读取 MV、水位、当前行及定义查询，输出摘要或独立差异表。

## 主要符号

execute_compare_sql、target、Output、flush、check_compare_base_select。

## 执行流程

先收集计划权限，再检查 OPERATE VIEW、输出 CREATE/INSERT；计算比较快照并检查当前目标与基表 SELECT；读取历史目标及快照水位；分别编译当前 MV 和重算查询，full outer join 后分类与写入。

## 数据与状态

比较快照决定历史表 ID；LAST_SUCCESS_READ_TSO 决定重算快照；两侧和输出 writer 使用定义 SQL mode、时区和 div precision。

## 依赖与调用关系

复用 definition_with_schema、BuildCompleteDiffSource、compile_mview_source、initialize_session、typed_hash_join 与 executor/mview_compare；实际 KV 数据、DDL、事务来自现有 canonical session/domain。

## 错误处理与边界

非法目标、未 ready、空定义、隐藏列、空/未来水位、schema 和权限错误向上传递。写入或后续读取失败回滚活动事务并删除整张输出表，包括已提交批次。

## 并发与资源生命周期

内部会话是调用栈拥有的独立会话；SessionInitGuard 恢复变量；Output guard 在成功之前拥有输出表；join 的 Open/Next/Close 错误均保留。

## 与 Go 版本的对应关系

对应 Go 59bfe3daabe4af4088631bb5094d0a67f204f613 的 mview_compare.go 与 builder.go；Rust canonical session 负责 Go executor/sessiontxn 的局部接线，不引入后续 touched_rows 扩展。

## 扩展指南

修改快照、时区、输出或权限顺序时，扩展独立 mview_compare_test.rs，并保留真实 cutover、批量事务、失败清理和 PostgreSQL 回归。
