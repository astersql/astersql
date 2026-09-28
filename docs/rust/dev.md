# dev

## 提示词

- 参考go版本，修复，尽量一比一对齐，不要删减，不要简化实现。

## AI 自动测试数据库

数据库连接 jdbc:mysql://localhost:4000/load_test 用户名 root 密码为空

```
SELECT t.*
FROM load_test.orders t
WHERE user_id = 14
LIMIT 4
```

这个会执行4秒，太卡了吧。修复一下呢，参考go，使用 EXPLAIN ANALYZE

## WorkPress 测试

php -S 127.0.0.1:8081 -t /Users/Shared/work/wordpress-astersql

##

在 codex 中，帮我给下面的任务创建会话，当前项目，一个任务一个会话，会话创建完毕之后，立刻停止，不用等待。
