// Copyright 2026 AsterSQL.

// SchemaLoader 的生产选项注入契约测试。
//
// 验证测试 Mock 在转换为生产环境使用的 trait object 后，仍会被 functional option
// 原样保留，并能通过统一的 `SchemaLoader` 接口接收重载调用。

use std::sync::Arc;

use astersql_ddl_mock::{Controller, new_mock_schema_loader};

use crate::SchemaLoader;
use crate::options::{apply_options, with_schema_loader};

#[test]
fn schema_loader_mock_is_injectable_through_production_options() {
    let controller = Controller::default();
    let loader = new_mock_schema_loader(controller.clone());
    loader.expect().reload(Ok(()));

    // 先擦除 Mock 的具体类型，再走生产选项链，避免测试绕过真实的依赖注入边界。
    let production: Arc<dyn SchemaLoader> = Arc::new(loader);
    let options = apply_options([with_schema_loader(production)]);
    options
        .schema_loader
        .expect("schema loader must be retained")
        .reload()
        .unwrap();
    controller.verify().unwrap();
}
