// Copyright 2026 AsterSQL.

// BR 集成测试的二进制入口。
//
// 此处仅将进程控制转交给 `astersql_br_tests`，使测试启动逻辑集中在可复用的库入口中。

fn main() {
    astersql_br_tests::main();
}
