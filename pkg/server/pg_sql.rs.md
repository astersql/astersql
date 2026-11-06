# `pkg/server/pg_sql.rs`

PostgreSQL查询进入共享执行器之前的有界语法适配与源错误检查。使用token位置保留原输入定位，沿现有元数据/绑定接口处理DDL、类型、别名、RETURNING及关系重命名；不在适配层建立对象、授权或另一套执行状态。

## 数据库TEMPORARY权限消费者

CREATE TEMP TABLE规范化为共享执行器接受的TEMPORARY关键字，保留临时表类别；命名空间桥接沿pg_name，数据库TEMPORARY权限由共享临时身份分配器在产生局部对象前校验。语法适配器不授予权限。

聚焦回归：`pg_catalog_test::cat14_17`，涵盖真实非root连接、授权后建临时表、撤权后拒绝新表及已有表仍可读取。
