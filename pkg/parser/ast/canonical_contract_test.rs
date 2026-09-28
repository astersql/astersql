// Copyright 2026 AsterSQL.

// 验证 AST 各兼容导出路径始终指向 crate 根模块定义的规范类型。
//
// `integration`、`model` 与 `ast` 只应重导出根类型，不能形成彼此独立、随演进漂移的
// 类型副本；这些编译期契约为历史调用路径保留源码兼容性。

use super::*;
use std::any::TypeId;

/// 利用同一个泛型参数要求两个实参具有完全相同的具体类型。
fn assert_same_type<T>(_: &T, _: &T) {}

#[test]
fn compatibility_paths_share_the_canonical_cistr_type() {
    let canonical = NewCIStr("MiXeD");
    let integration = integration::NewCIStr("MiXeD");
    let model = model::NewCIStr("MiXeD");

    assert_same_type(&canonical, &integration);
    assert_same_type(&canonical, &model);
}

#[test]
fn compatibility_paths_share_the_canonical_node_protocol() {
    // trait 对象的 TypeId 相等可防止兼容模块意外声明同名但不同身份的 Node 协议。
    assert_eq!(
        TypeId::of::<dyn Node>(),
        TypeId::of::<dyn integration::Node>(),
    );
    assert_eq!(TypeId::of::<dyn Node>(), TypeId::of::<dyn ast::Node>());
}
