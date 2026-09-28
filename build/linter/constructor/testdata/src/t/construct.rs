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

// 本文件由 build/linter/constructor/testdata/src/t/construct.go 机械迁移而来，保留 Go 测试数据的构造场景。
// 这是 constructor analyzer 的 fixture 草稿，只表达哪些构造应被允许或报错；不会连接数据库、不会执行业务动作，也不会真正运行 Go analyzer。
// Go package: t。
//
// Go imports:
// - github.com/pingcap/tidb/pkg/util/linter/constructor

// StructWithSpecificConstructor is a struct with `Constructor`
// StructWithSpecificConstructor 对应 Go 中带 constructor.Constructor 标记字段的结构体。
// Go 的空白字段携带 `ctor:"NewStructWithSpecificConstructor,AnotherConstructor"` tag；
// Rust 草稿用具名字段保留类型关系，并在注释中保留 tag 语义。
pub struct StructWithSpecificConstructor {
    // Go field: _ constructor.Constructor `ctor:"NewStructWithSpecificConstructor,AnotherConstructor"`
    // 该字段只供 linter 读取允许的构造函数名，不承载测试运行时数据。
    pub constructor_marker: constructor::Constructor,
    pub otherField: String,
}

// NewStructWithSpecificConstructor creates a new `StructWithSpecificConstructor`
// this function should work fine
// NewStructWithSpecificConstructor 是允许手工构造 StructWithSpecificConstructor 的 Go 构造函数。
pub fn NewStructWithSpecificConstructor() -> Box<StructWithSpecificConstructor> {
    // Go 这里同时覆盖 new(T)、var T 和 &T{} 三种允许场景，Rust 草稿只保留这些检查点。
    let _ = Box::new(StructWithSpecificConstructor::zero_value_fixture());
    let _var_fixture: StructWithSpecificConstructor =
        StructWithSpecificConstructor::zero_value_fixture();
    Box::new(StructWithSpecificConstructor::zero_value_fixture())
}

// AnotherConstructor gives another constructor of StructWithSpecificConstructor
// AnotherConstructor 是 ctor tag 中列出的第二个允许构造函数。
pub fn AnotherConstructor() -> Box<StructWithSpecificConstructor> {
    Box::new(StructWithSpecificConstructor::zero_value_fixture())
}

impl StructWithSpecificConstructor {
    // zero_value_fixture 只是为了让 Rust 草稿能重复表达 Go 的 `StructWithSpecificConstructor{}` 字面量形状。
    // constructor::Constructor 是迁移占位依赖，不在本 fixture 中提供真实实现。
    pub fn zero_value_fixture() -> Self {
        Self {
            constructor_marker: constructor::Constructor,
            otherField: String::new(),
        }
    }

    // with_other_field_fixture 对应 Go 测试数据中显式写 otherField 的结构体字面量。
    pub fn with_other_field_fixture(otherField: &str) -> Self {
        Self {
            constructor_marker: constructor::Constructor,
            otherField: otherField.to_string(),
        }
    }
}

// otherFunction 汇总 Go fixture 中所有“构造器外部”的正反例。
// 行尾 want 注释保留 analyzer_test 读取的预期诊断文本。
fn otherFunction() {
    let _ = NewStructWithSpecificConstructor();
    let _ = AnotherConstructor();

    let _ = StructWithSpecificConstructor::zero_value_fixture(); // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    let _ = Box::new(StructWithSpecificConstructor::zero_value_fixture()); // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    let _ = vec![StructWithSpecificConstructor::zero_value_fixture()]; // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    let _ = vec![
        Box::new(StructWithSpecificConstructor::with_other_field_fixture("a")), // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
        Box::new(StructWithSpecificConstructor::zero_value_fixture()), // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    ];

    // anonymous struct
    // Go 在匿名结构体中声明 constructor.Constructor 标记字段；
    // Rust 不能直接写同形匿名 struct item，因此用局部结构体保留“未命名 fixture 类型携带 ctor tag”的语义。
    struct AnonymousStructFixture {
        // Go field: _ constructor.Constructor `ctor:"NewStructWithSpecificConstructor"`
        constructor_marker: constructor::Constructor,
    }
    let _ = AnonymousStructFixture {
        constructor_marker: constructor::Constructor,
    }; // want `struct can only be constructed in constructors NewStructWithSpecificConstructor`

    // new
    let _ = Box::new(StructWithSpecificConstructor::zero_value_fixture()); // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`

    // var
    let _var_fixture: StructWithSpecificConstructor =
        StructWithSpecificConstructor::zero_value_fixture(); // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    let _ptr_fixture: Option<Box<StructWithSpecificConstructor>> = None;

    // compositeImplicitInitiate1 对应 Go 的匿名嵌入结构体；默认构造会隐式构造内嵌字段，应触发诊断。
    struct compositeImplicitInitiate1 {
        StructWithSpecificConstructor: StructWithSpecificConstructor,
    }
    let _ = compositeImplicitInitiate1 {
        StructWithSpecificConstructor: StructWithSpecificConstructor::zero_value_fixture(),
    }; // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    let _ = Box::new(compositeImplicitInitiate1 {
        StructWithSpecificConstructor: StructWithSpecificConstructor::zero_value_fixture(),
    }); // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`

    // specified field is also allowed
    // Go 显式填写内嵌字段且值来自允许构造函数，constructor analyzer 应放行。
    // 这个对照用例说明 analyzer 关注的是“值从哪里构造出来”，而不只是有没有写字段名。
    let _ = compositeImplicitInitiate1 {
        StructWithSpecificConstructor: *NewStructWithSpecificConstructor(),
    };

    // specified field with manually constructed struct is not allowed
    let _ = compositeImplicitInitiate1 {
        StructWithSpecificConstructor: StructWithSpecificConstructor::zero_value_fixture(), // want `struct can only be constructed in constructors NewStructWithSpecificConstructor, AnotherConstructor`
    };

    // pointer field is allowed
    // 指针字段的零值不是实际构造 StructWithSpecificConstructor，Go analyzer 这里不报错。
    struct compositeImplicitInitiate2 {
        StructWithSpecificConstructor: Option<Box<StructWithSpecificConstructor>>,
    }
    let _ = compositeImplicitInitiate2 {
        StructWithSpecificConstructor: None,
    };
}
