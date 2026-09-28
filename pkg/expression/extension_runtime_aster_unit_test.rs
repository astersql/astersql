// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 扩展函数运行时注册与工厂安装的并发契约单元测试。
//
// 对照 Go：校验 nil/缺回调/与内建冲突/重复注册、钩子幂等安装，
// 以及 `BuildSimpleExpr` 工厂在并发下同工厂幂等、异工厂拒绝落败者。

use std::sync::{Arc, Barrier, OnceLock};

use extension_dependency::{ExtensionError, FunctionDef};

use crate::{extensionFuncs, types};

/// 并发安装用的占位工厂：永不真正求值，仅用于比较函数指针。
fn concurrent_build_simple_expr<'a>(
    _context: &dyn crate::BuildContext,
    _expression: &crate::ast::ExprNode,
    _options: Vec<crate::BuildOption<'a>>,
) -> Result<crate::ExprBox, crate::errors::Error> {
    Err(crate::errors::New(
        "concurrency regression factory is not evaluated",
    ))
}

/// 与上一工厂不同的竞争安装方：返回 NULL 常量表达式。
fn competing_build_simple_expr<'a>(
    _context: &dyn crate::BuildContext,
    _expression: &crate::ast::ExprNode,
    _options: Vec<crate::BuildOption<'a>>,
) -> Result<crate::ExprBox, crate::errors::Error> {
    Ok(Box::new(crate::NewNull()))
}

/// 构造仅实现 `EvalString` 的最小扩展函数定义。
fn string_definition(name: &str) -> Arc<FunctionDef> {
    Arc::new(FunctionDef {
        Name: name.to_owned(),
        EvalTp: types::ETString,
        ArgTps: vec![types::ETString],
        OptionalArgsLen: 0,
        EvalStringFunc: Some(Arc::new(|_context, _row| Ok(("ok".to_owned(), false)))),
        EvalIntFunc: None,
        RequireDynamicPrivileges: None,
    })
}

#[test]
/// 注册路径与 Go 一致：nil、缺回调、内建冲突、成功注册、重复名、按精确键删除。
fn extension_registration_matches_go_validation_and_atomicity() {
    assert_eq!(
        crate::extension_kernel::registerExtensionFunc(None).unwrap_err(),
        ExtensionError::new("extension function def is nil")
    );

    let missing_callback = Arc::new(FunctionDef {
        Name: "aster_extension_missing_callback".to_owned(),
        EvalTp: types::ETString,
        ArgTps: Vec::new(),
        OptionalArgsLen: 0,
        EvalStringFunc: None,
        EvalIntFunc: None,
        RequireDynamicPrivileges: None,
    });
    assert_eq!(
        crate::extension_kernel::registerExtensionFunc(Some(&missing_callback)).unwrap_err(),
        ExtensionError::new("eval function is nil")
    );

    let builtin_conflict = string_definition("ABS");
    assert!(
        crate::extension_kernel::registerExtensionFunc(Some(&builtin_conflict))
            .unwrap_err()
            .to_string()
            .contains("conflict with builtin")
    );

    let name = "aster_extension_runtime_registration";
    crate::extension_kernel::removeExtensionFunc(name);
    let definition = string_definition(name);
    crate::extension_kernel::registerExtensionFunc(Some(&definition)).unwrap();
    assert!(extensionFuncs.Load(name).is_some());
    assert!(
        crate::extension_kernel::registerExtensionFunc(Some(&definition))
            .unwrap_err()
            .to_string()
            .contains("duplicated")
    );
    // Go 的 removeExtensionFunc 直接把传入名称交给 sync.Map.Delete；注册表中保存的是
    // 小写键，因此不同大小写的名称不能误删已经注册的函数。
    crate::extension_kernel::removeExtensionFunc(&name.to_uppercase());
    assert!(extensionFuncs.Load(name).is_some());
    crate::extension_kernel::removeExtensionFunc(name);
    assert!(extensionFuncs.Load(name).is_none());
}

#[test]
/// `init` 多次调用应幂等，不重复安装钩子导致 panic。
fn extension_hooks_can_be_installed_idempotently() {
    crate::extension_kernel::init();
    crate::extension_kernel::init();
}

#[test]
/// 两线程同时安装同一工厂：均成功，OnceLock 中留下该工厂。
fn build_simple_expr_factory_concurrent_first_install_is_idempotent() {
    let storage = OnceLock::new();
    let barrier = Arc::new(Barrier::new(2));
    let factory: crate::BuildSimpleExprFn = concurrent_build_simple_expr;

    std::thread::scope(|scope| {
        let handles = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let storage = &storage;
                scope.spawn(move || {
                    // 屏障对齐后同时争用 OnceLock::set。
                    crate::expression_core::installBuildSimpleExprWith(storage, factory, || {
                        barrier.wait();
                    })
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle
                .join()
                .expect("factory installer thread must not panic")
                .expect("the same factory must be accepted by both installers");
        }
    });

    assert!(std::ptr::fn_addr_eq(
        *storage.get().expect("one installer must win OnceLock::set"),
        factory,
    ));
}

#[test]
/// 两线程安装不同工厂：恰一成功，落败者报“已有不同工厂”。
fn build_simple_expr_factory_concurrent_different_install_rejects_loser() {
    let storage = OnceLock::new();
    let barrier = Arc::new(Barrier::new(2));
    let factories: [crate::BuildSimpleExprFn; 2] =
        [concurrent_build_simple_expr, competing_build_simple_expr];

    let results = std::thread::scope(|scope| {
        factories
            .into_iter()
            .map(|factory| {
                let barrier = Arc::clone(&barrier);
                let storage = &storage;
                scope.spawn(move || {
                    crate::expression_core::installBuildSimpleExprWith(storage, factory, || {
                        barrier.wait();
                    })
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("factory installer thread must not panic")
            })
            .collect::<Vec<_>>()
    });

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let errors = results
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        errors,
        vec!["a different BuildSimpleExpr factory is already installed"]
    );
}
