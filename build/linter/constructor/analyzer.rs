// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 build/linter/constructor/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 constructor analyzer 如何根据 struct tag 限制手工构造结构体的位置。
// 当前 Rust 草稿不会真正执行 go/types 或 AST inspector；ast、types、structtag、analysis 等均为占位依赖。
// Go package: constructor。
//
// Go imports:
// - go/ast
// - go/types
// - slices
// - strings
// - github.com/fatih/structtag
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/ast/inspector

// ConstructorUtilPath defines the path of the constructor utility package.
// ConstructorUtilPath 对应 Go 常量，用于识别 pkg/util/linter/constructor.Constructor 字段。
// 单独提成常量可以让 analyzer 与测试共享同一包路径，避免路径字符串在多处漂移。
pub const ConstructorUtilPath: &str = "github.com/pingcap/tidb/pkg/util/linter/constructor";

// Analyzer is the analyzer struct of constructor.
// constructor only allows constructing a struct manually in some specific functions, which is specified with tags for
// `constructor.Constructor` field.
//
// It can detect the following pattern and give error (if not in constructor functions):
//
// 1. Create struct directly, like `SomeStruct{}` or `&SomeStruct{}`.
// 2. Create struct with `new`: `new(SomeStruct)`
// 3. Struct literal in slice or other struct: `[]SomeStruct{{}}`
// 4. Define variables through `var`: `var a SomeStruct`.
// 5. Implicit zero value in other struct literal: `type OtherStruct struct{SomeStruct}; other := OtherStruct{}`
//
// TODO: verify whether this linter can work well with generics
// Analyzer 对应 Go 的包级变量，Run 绑定到 run。
// 它不试图识别“推荐构造方式”，只负责阻止明确被 ctor tag 禁止的手工构造入口。
// 因而规则语义更接近“白名单构造器约束”，而不是通用的对象初始化最佳实践检查。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "constructor",
    doc: "Check developers don't create structs manually without using constructors",
    requires: &[],
    run,
};

// getConstructorList 对应 Go 的递归查找：从结构体字段的 ctor tag 中提取允许的构造函数名。
pub fn getConstructorList(
    t: types::Type,
    ignoreFields: Option<HashMap<String, ()>>,
) -> Vec<String> {
    let mut structTyp = match t.as_struct() {
        Some(v) => v,
        None => {
            // It's also possible to construct a pointer directly, e.g. []*Struct{{}}
            // Go 允许传入指针类型，这里取 Elem().Underlying() 后继续按结构体处理。
            // 这样 new(T)、切片里的 *T 字面量和其它间接构造路径都能复用同一套 ctor 提取逻辑。
            let ptr = match t.as_pointer() {
                Some(v) => v,
                None => return vec![],
            };
            match ptr.Elem().Underlying().as_struct() {
                Some(v) => v,
                None => return vec![],
            }
        }
    };

    let mut ctors: Vec<String> = Vec::new();
    for i in 0..structTyp.NumFields() {
        let field = structTyp.Field(i);
        let named = match field.Type().as_named() {
            Some(v) => v,
            None => continue,
        };
        if let Some(ignoreFields) = &ignoreFields {
            if ignoreFields.contains_key(field.Name()) {
                // 已显式给值的字段会在别的节点递归检查，这里只关注“因省略字段而触发的隐式构造”。
                continue;
            }
        }
        let obj = named.Obj().expect("named type must have an object");
        let pkg = obj
            .Pkg()
            .expect("constructor marker type must have a package");
        if obj.Name() == "Constructor" && pkg.Path() == ConstructorUtilPath {
            // 字段名和包路径必须同时命中，才能确认它是约定俗成的 constructor 标记字段。
            let tags = match structtag::Parse(structTyp.Tag(i)) {
                Ok(v) => v,
                Err(_) => continue, // skip invalid tags
            };
            // 非法 tag 在 Go 中也不会阻断整个 analyzer；这里只把它视为“没有声明 ctor 白名单”。
            let ctorTag = match tags.Get("ctor") {
                Ok(v) => v,
                Err(_) => continue,
            };
            // ctor tag 里列出的多个函数名直接构成白名单，顺序保持 Go 原字符串顺序。
            ctors = strings::Split(ctorTag.Value(), ",");
            continue;
        }

        // 嵌套结构体中也可能携带 Constructor 标记，Go 递归展开 named.Underlying()。
        if let Some(fieldStruct) = named.Underlying().as_struct() {
            ctors.extend(getConstructorList(fieldStruct, None));
        }
    }
    ctors
}

// assertInConstructor 对应 Go 的调用栈检查：向外找最近的函数声明并校验函数名。
pub fn assertInConstructor(
    pass: &mut analysis::Pass,
    n: ast::Node,
    stack: &[ast::Node],
    ctors: &[String],
) -> bool {
    // check whether this call is in `ctor`
    for i in (0..stack.len()).rev() {
        let funcDecl = match stack[i].as_func_decl() {
            Some(v) => v,
            None => continue,
        };

        // 只检查最近外层函数声明是否命中白名单；一旦离开当前函数，继续向外追溯已没有意义。
        if !slices::Contains(ctors, funcDecl.Name.Name) {
            pass.Reportf(
                n.Pos(),
                "struct can only be constructed in constructors %s",
                strings::Join(ctors, ", "),
            );
            return false;
        }

        return true;
    }
    true
}

// handleCompositeLit 对应 Go 对复合字面量的处理：识别隐式构造并排除显式填写字段。
pub fn handleCompositeLit(
    pass: &mut analysis::Pass,
    n: &ast::CompositeLit,
    push: bool,
    stack: &[ast::Node],
) -> bool {
    if !push {
        // 仅在进入节点时判定，退出阶段不重复处理，和 Go inspector.WithStack 的使用方式保持一致。
        return false;
    }

    // Go immediately calls Underlying on TypeOf's result. Missing type information is therefore
    // an invariant violation rather than a branch that the analyzer silently ignores.
    let t = pass
        .TypesInfo
        .TypeOf(n)
        .expect("composite literal type information is required")
        .Underlying();

    // Just ignore the specified fields. They'll be checked recursively later. In this round, we only need to avoid
    // the case that the struct is implicitly initiated.
    // Go 通过 ignoreFields 区分“已显式初始化字段”和“隐式零值构造字段”。
    let mut ignoreFields: HashMap<String, ()> = HashMap::new();
    for (i, elt) in n.Elts.iter().enumerate() {
        match elt {
            ast::Expr::KeyValueExpr(elt) => {
                if let Some(ident) = elt.Key.as_ident() {
                    // 具名字段字面量已经明确指定初始化来源，后续由对应值表达式继续递归检查。
                    ignoreFields.insert(ident.Name.clone(), ());
                }
            }
            _ => {
                let strctTyp = match t.as_struct() {
                    Some(v) => v,
                    None => continue,
                };
                // 非 KeyValue 的位置初始化同样算“显式给值”，因此要按字段顺序映射回字段名。
                ignoreFields.insert(strctTyp.Field(i).Name().to_string(), ());
            }
        }
    }

    let ctors = getConstructorList(t, Some(ignoreFields));
    if ctors.is_empty() {
        return true;
    }

    // 走到这里说明剩余未显式赋值字段里存在受 ctor 约束的结构体，需要校验当前位置是否在白名单构造器中。
    assertInConstructor(pass, n.as_node(), stack, &ctors)
}

// handleCallExpr 对应 Go 对 new(T) 的处理：只检查内建 new 且参数存在的调用。
pub fn handleCallExpr(
    pass: &mut analysis::Pass,
    n: &ast::CallExpr,
    push: bool,
    stack: &[ast::Node],
) -> bool {
    let fun = match n.Fun.as_ident() {
        Some(v) => v,
        None => return true,
    };
    // 只拦截内建 new；普通函数调用即使返回同名类型，也不属于“手工构造”范畴。
    if fun.Name != "new" || n.Args.is_empty() {
        return true;
    }

    let t = pass
        .TypesInfo
        .TypeOf(n)
        .expect("call expression type information is required")
        .Underlying();
    // 这里取调用结果类型，而不是参数语法节点类型，是为了统一覆盖 `new(alias)` 等经类型推断后的结果。
    let ctors = getConstructorList(t, None);
    if ctors.is_empty() {
        return true;
    }

    assertInConstructor(pass, n.as_node(), stack, &ctors)
}

// handleValueSpec 对应 Go 对 var 声明的处理：声明指针不视为构造，其它类型检查 Constructor 标记。
pub fn handleValueSpec(
    pass: &mut analysis::Pass,
    n: &ast::ValueSpec,
    _push: bool,
    stack: &[ast::Node],
) -> bool {
    let Some(t) = pass.TypesInfo.TypeOf(n.Type) else {
        return true;
    };

    // allow declaring a pointer, as it's actually not constructed.
    // `var p *T` 只得到零值指针，不会立即制造一个 T；而 `var v T` 会得到完整零值对象。
    if t.as_pointer().is_some() {
        return true;
    }

    let ctors = getConstructorList(t.Underlying(), None);
    if ctors.is_empty() {
        // 没有 Constructor 标记就说明该类型不受本规则约束，保留普通零值声明能力。
        return true;
    }

    assertInConstructor(pass, n.as_node(), stack, &ctors)
}

// run 对应 Go analyzer 主流程：用 inspector.WithStack 同时监听三类构造形态。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    for file in &pass.Files {
        let i = inspector::New(vec![file]);

        // 三类节点共用同一条 ctor 白名单判定链，从而覆盖显式字面量、new 和 var 零值构造三种入口。
        i.WithStack(
            vec![ast::CompositeLit {}, ast::CallExpr {}, ast::ValueSpec {}],
            |n: ast::Node, push: bool, stack: Vec<ast::Node>| -> bool {
                match n {
                    ast::Node::CompositeLit(n) => handleCompositeLit(pass, &n, push, &stack),
                    ast::Node::CallExpr(n) => handleCallExpr(pass, &n, push, &stack),
                    ast::Node::ValueSpec(n) => handleValueSpec(pass, &n, push, &stack),
                    _ => true,
                }
            },
        );
    }
    Ok(None)
}

// init 对应 Go 的 init：支持按配置跳过 constructor analyzer。
pub fn init() {
    // constructor 规则常用于逐步清理遗留调用点，因此保留可配置关闭能力，便于分阶段迁移。
    util::SkipAnalyzerByConfig(&Analyzer);
}
