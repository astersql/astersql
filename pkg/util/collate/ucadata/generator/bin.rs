// Copyright 2026 AsterSQL.

// UCA 排序权重表生成器的命令行入口。
//
// 参数解析与文件生成统一委托给库中的 `runGenerator`，这里仅负责将失败信息写入
// 标准错误，并用非零状态码通知调用方生成失败。

fn main() {
    // 保持入口层无业务状态，便于库接口复用相同的参数校验与生成流程。
    if let Err(error) = astersql_util_collate_ucadata_generator::runGenerator(std::env::args_os()) {
        eprintln!("ucadata-generator: {error}");
        std::process::exit(1);
    }
}
