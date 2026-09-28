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

// 本文件由 build/linter/bootstrap/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 bootstrap analyzer 如何检查 TiDB bootstrap.go 与 upgrade_def.go 的命名和版本一致性。
// 当前 Rust 草稿不会真正解析 Go AST、不会访问文件系统，也不会执行业务升级动作；ast、token、analysis 等均为占位依赖。
// Go package: bootstrap。
//
// Go imports:
// - go/ast
// - go/token
// - maps
// - strconv
// - strings
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Analyzer is the analyzer struct of unconvert.
// Analyzer 对应 Go 的包级变量，保留 bootstrap 检查的名称、说明和 run 入口。
// 该规则本质上不是检查 SQL 语义，而是检查 bootstrap 元数据有没有随着版本演进保持同步。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "bootstrap",
    doc: "Check developers don't forget something in TiDB bootstrap logic",
    requires: &[],
    run,
};

// bootstrapCodeFile 和 upgradeCodeFile 对应 Go 常量，用于按文件名后缀分派检查逻辑。
// 两个文件分别承载“初始 schema 定义”和“后续版本升级链”，因此需要拆成两套检查。
pub const bootstrapCodeFile: &str = "/bootstrap.go";
pub const upgradeCodeFile: &str = "/upgrade_def.go";

// run 对应 Go analyzer 主流程：遍历当前包文件并按后缀调用具体检查。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    // 同一个 package pass 中同时扫两类文件，能保证 bootstrap 元数据和升级元数据在一次检查中对齐。
    // 这也避免了“只改了一半文件就通过局部检查”的假阳性。
    for file in &pass.Files {
        let name = pass.Fset.File(file.Pos()).Name();
        // bootstrap 规则只关心两类特定源文件，其他文件即使同包也不参与这组一致性检查。
        if strings::HasSuffix(&name, bootstrapCodeFile) {
            checkBootstrapDotGo(pass, file);
        }
        if strings::HasSuffix(&name, upgradeCodeFile) {
            // upgrade_def.go 的检查关注版本推进链条，而不是 bootstrap.go 中的 schema 命名细节。
            checkUpgradeDotGo(pass, file);
        }
    }
    Ok(None)
}

// check whether the spec is a slice variable definition.
// isSliceVarDefNode 对应 Go 的 AST 形状识别：只接受单变量、单值、数组复合字面量定义。
pub fn isSliceVarDefNode(spec: ast::Spec) -> (String, String, Option<ast::CompositeLit>, bool) {
    // 这里不是通用的变量声明解析器，而是专门锁定 bootstrap.go 里约定俗成的 schema 变量写法。
    let valSpec = match spec.as_value_spec() {
        Some(v) => v,
        None => return ("".into(), "".into(), None, false),
    };
    if valSpec.Names.len() != 1 || valSpec.Values.len() != 1 {
        // 一旦同一条声明里混入多个名字或多个值，就无法安全映射到“一个 schema 变量”这一约束。
        return ("".into(), "".into(), None, false);
    }
    let compLit = match valSpec.Values[0].as_composite_lit() {
        Some(v) => v,
        None => return ("".into(), "".into(), None, false),
    };
    let arrTp = match compLit.Type.as_array_type() {
        Some(v) => v,
        None => return ("".into(), "".into(), None, false),
    };
    // 返回元素类型名而不是整个 Type 节点，是因为后续分派只依赖切片里装的是什么业务对象。
    let varTpIdent = match arrTp.Elt.as_ident() {
        Some(v) => v,
        None => return ("".into(), "".into(), None, false),
    };
    (
        valSpec.Names[0].Name.clone(),
        varTpIdent.Name.clone(),
        Some(compLit),
        true,
    )
}

// checkSystemTablesDefinitionNode 对应 Go 对系统表定义数组的逐项命名检查。
pub fn checkSystemTablesDefinitionNode(
    pass: &mut analysis::Pass,
    varName: &str,
    compLit: &ast::CompositeLit,
) {
    for elt in &compLit.Elts {
        // Go 使用连续类型断言下钻固定字段布局；expect 保留断言失败时 panic 的契约。
        let eltLit = elt
            .as_composite_lit()
            .expect("system table entry must be a composite literal");
        let idField = eltLit.Elts[0]
            .as_key_value_expr()
            .expect("system table field must be a key-value expression");
        let idSelector = idField
            .Value
            .as_selector_expr()
            .expect("table ID must be a selector expression");
        let idIdentPkg = idSelector
            .X
            .as_ident()
            .expect("table ID selector base must be an identifier")
            .Name
            .clone();
        if idIdentPkg != "metadef" {
            pass.Reportf(
                elt.Pos(),
                "table ID must be defined in metadef pkg, but got %q",
                idIdentPkg,
            );
        }
        let idIdentName = idSelector
            .Sel
            .as_ref()
            .expect("table ID selector must have a selected identifier")
            .Name
            .clone();
        let nameField = eltLit.Elts[1]
            .as_key_value_expr()
            .expect("system table field must be a key-value expression");
        let quotedName = nameField
            .Value
            .as_basic_lit()
            .expect("table name must be a basic literal")
            .Value
            .clone();
        let tableName = match strconv::Unquote(&quotedName) {
            Ok(v) => v,
            Err(_) => {
                // 表名必须是字面量，才能和常量名做静态比对；动态表达式在这里无法安全审计。
                pass.Reportf(
                    elt.Pos(),
                    "the name of the table in %s must be a string literal, but got %q",
                    varName,
                    quotedName,
                );
                continue;
            }
        };
        let sqlField = eltLit.Elts[2]
            .as_key_value_expr()
            .expect("system table field must be a key-value expression");
        let sqlSelector = sqlField
            .Value
            .as_selector_expr()
            .expect("create table SQL must be a selector expression");
        let sqlPkgName = sqlSelector
            .X
            .as_ident()
            .expect("create table SQL selector base must be an identifier")
            .Name
            .clone();
        if sqlPkgName != "metadef" {
            pass.Reportf(
                elt.Pos(),
                "Create table SQL must be defined in metadef pkg, but got %q",
                sqlPkgName,
            );
        }
        // 表 ID 常量和建表 SQL 常量都要求遵循固定命名约定，后续才能通过名字反推出同一张表。
        // 如果其中任意一项漂移，新增系统表时就容易出现“ID、名称、SQL”三处不同步的问题。
        let sqlIdentName = sqlSelector
            .Sel
            .as_ref()
            .expect("create table SQL selector must have a selected identifier")
            .Name
            .clone();
        if !strings::HasSuffix(&idIdentName, "TableID") {
            pass.Reportf(
                elt.Pos(),
                "the name of the constant of table ID in %s must end with TableID, but got %q",
                varName,
                idIdentName,
            );
        }
        if !strings::HasPrefix(&sqlIdentName, "Create")
            || !strings::HasSuffix(&sqlIdentName, "Table")
        {
            pass.Reportf(
                elt.Pos(),
                "the name of the constant of the create table SQL in %s must start 'CreateXXXTable' style, but got %q",
                varName,
                sqlIdentName,
            );
        }
        if strings::TrimSuffix(&idIdentName, "TableID")
            != strings::TrimSuffix(&strings::TrimPrefix(&sqlIdentName, "Create"), "Table")
        {
            // 这里比较的是“语义名”而不是原始常量名，防止 ID 常量和 SQL 常量描述不同对象。
            pass.Reportf(
                elt.Pos(),
                "the name of the constant of table ID in %s must match the name of the create table SQL, but got %q and %q",
                varName,
                idIdentName,
                sqlIdentName,
            );
        }
        let nameInCamel = strings::ReplaceAll(&tableName, "_", "");
        // 真实表名会先去掉下划线再与常量主干比较，确保 snake_case 表名与 Camel 风格常量仍能对应。
        if strings::ToLower(&strings::TrimSuffix(&idIdentName, "TableID")) != nameInCamel {
            pass.Reportf(
                elt.Pos(),
                "the name of the constant of table ID in %s must match the name of the table, but got %q and %q",
                varName,
                idIdentName,
                tableName,
            );
        }
    }
}

// checkVersionedBootstrapSchema 对应 Go 的 schema 变量使用完整性检查。
pub fn checkVersionedBootstrapSchema(
    pass: &mut analysis::Pass,
    compLit: &ast::CompositeLit,
    schemaDefVarNames: HashMap<String, ()>,
) {
    let mut nameUsedInVersionedBootstrapSchema: HashMap<String, ()> = HashMap::new();
    // 这里把 map 当作 set 使用，只追踪“是否出现过该 schema 变量名”。
    for elt in &compLit.Elts {
        // must be {ver: xxx, databases: xxx}
        let versionedSchemaEntry = elt
            .as_composite_lit()
            .expect("versioned schema entry must be a composite literal");
        let databasesKVNode = versionedSchemaEntry.Elts[1]
            .as_key_value_expr()
            .expect("databases field must be a key-value expression");
        // versionedBootstrapSchema 的第二个字段必须是 databases，后续检查才能稳定抽取表集合。
        let databasesKey = databasesKVNode
            .Key
            .as_ident()
            .expect("databases field key must be an identifier");
        if databasesKey.Name != "databases" {
            pass.Reportf(
                databasesKVNode.Pos(),
                "the 2nd field of versionedBootstrapSchema must be 'databases'",
            );
            continue;
        }
        let databaseDefinitions = databasesKVNode
            .Value
            .as_composite_lit()
            .expect("databases field value must be a composite literal");
        for dbDefNode in &databaseDefinitions.Elts {
            let dbDefNodeLit = dbDefNode
                .as_composite_lit()
                .expect("database definition must be a composite literal");
            // {ID:xx, Name: xx, Tables: xx...}
            if dbDefNodeLit.Elts.len() >= 3 {
                // 这里只关心 Tables 字段引用了哪个 schema 变量，不关心数据库 ID 和名称细节。
                // 因为 bootstrap 版本演进真正容易漏改的，就是某组表定义有没有被挂进版本列表。
                let tablesField = dbDefNodeLit.Elts[2]
                    .as_key_value_expr()
                    .expect("database field must be a key-value expression");
                let tablesVarNode = match tablesField.Value.as_ident() {
                    Some(v) => v,
                    None => {
                        pass.Reportf(
                            dbDefNodeLit.Elts[2].Pos(),
                            "the Tables field of database definition must be a variable",
                        );
                        continue;
                    }
                };
                nameUsedInVersionedBootstrapSchema.insert(tablesVarNode.Name.clone(), ());
            }
        }
    }
    // Go 使用 maps.Equal 比较两个 set-like map，确保定义过的 schema 变量全部被版本化 schema 使用。
    // 这一步同时防止“定义了但忘记挂进版本列表”和“版本列表引用了不存在变量”两类错误。
    // 两边比较的是集合而不是顺序，因此这里容忍声明顺序变化，但不容忍成员缺失或新增。
    if !maps::Equal(&schemaDefVarNames, &nameUsedInVersionedBootstrapSchema) {
        pass.Reportf(
            compLit.Pos(),
            "the variables used in versionedBootstrapSchema do not match the defined schema variables, %v vs %v",
            schemaDefVarNames,
            nameUsedInVersionedBootstrapSchema,
        );
    }
}

// checkBootstrapDotGo 对应 Go 对 bootstrap.go 的检查：统计 schema 定义和 versionedBootstrapSchema。
pub fn checkBootstrapDotGo(pass: &mut analysis::Pass, file: &ast::File) {
    let mut foundVarNames: HashMap<String, ()> = HashMap::new();
    let mut schemaDefNodeCount = 0;
    let mut versionedBootstrapSchemaDefCount = 0;
    // 整个 bootstrap.go 只关心两种切片变量：系统表定义列表和 versionedBootstrapSchema 列表。
    // 其它常量、函数或辅助结构即使同文件出现，也不会影响 bootstrap 元数据一致性。
    for decl in &file.Decls {
        match decl {
            ast::Decl::GenDecl(v) => {
                for spec in &v.Specs {
                    let (varName, eleTpName, compLit, ok) = isSliceVarDefNode(spec.clone());
                    if !ok {
                        continue;
                    }
                    if eleTpName == "TableBasicInfo" {
                        // 每个 TableBasicInfo 切片都代表一组 schema 定义，既要做内容校验，也要登记变量名供后续引用检查。
                        schemaDefNodeCount += 1;
                        checkSystemTablesDefinitionNode(pass, &varName, compLit.as_ref().unwrap());
                        foundVarNames.insert(varName, ());
                    } else if eleTpName == "versionedBootstrapSchema" {
                        // versionedBootstrapSchema 不是定义新表，而是声明“某个版本应该启用哪些已定义 schema”。
                        versionedBootstrapSchemaDefCount += 1;
                        checkVersionedBootstrapSchema(
                            pass,
                            compLit.as_ref().unwrap(),
                            foundVarNames.clone(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    // 没有任何 schema 定义通常意味着开发者新增表后忘了把元数据接进 bootstrap。
    if schemaDefNodeCount < 1 {
        pass.Reportf(
            file.Pos(),
            "there must be at least one schema definition variable defined",
        );
    }
    // 这里只允许一个总的 versionedBootstrapSchema，避免不同列表彼此覆盖导致升级路径失真。
    if versionedBootstrapSchemaDefCount != 1 {
        pass.Reportf(
            file.Pos(),
            "there must be exactly one versionedBootstrapSchema variable defined",
        );
    }
}

// checkUpgradeDotGo 对应 Go 对 upgrade_def.go 的版本一致性检查。
pub fn checkUpgradeDotGo(pass: &mut analysis::Pass, file: &ast::File) {
    let mut maxVerVariable = 0;
    let mut maxVerVariablePos: token::Pos = token::NoPos;
    let mut curVerVariable = 0;
    let mut curVerVariablePos: token::Pos = token::NoPos;
    let mut maxVerFunc = 0;
    let mut maxVerFuncPos: token::Pos = token::NoPos;
    let mut maxVerFuncUsed = 0;
    let mut maxVerFuncUsedPos: token::Pos = token::NoPos;
    // 这里并行维护四个“最新版本”来源，最后统一比较是否收敛到同一数字。
    // 任一来源落后，都意味着升级入口、版本常量或当前版本声明存在漏改。

    for decl in &file.Decls {
        match decl {
            ast::Decl::GenDecl(v) => {
                if v.Specs.len() == 1 {
                    // 单规格声明对应 upgradeToVerFunctions 和 currentBootstrapVersion 这两类“总入口”变量。
                    let spec = &v.Specs[0];
                    let v2 = match spec.as_value_spec() {
                        Some(v) => v,
                        None => continue,
                    };
                    if v2.Names.len() != 1 {
                        continue;
                    }
                    match v2.Names[0].Name.as_str() {
                        "upgradeToVerFunctions" => {
                            let composeLit = v2.Values[0]
                                .as_composite_lit()
                                .expect("upgrade function list must be a composite literal");
                            let lastElm = composeLit
                                .Elts
                                .last()
                                .expect("upgrade function list must not be empty");
                            let lastEntry = lastElm
                                .as_composite_lit()
                                .expect("upgrade function entry must be a composite literal");
                            let functionField = lastEntry.Elts[1]
                                .as_key_value_expr()
                                .expect("upgrade function field must be a key-value expression");
                            let ident = functionField
                                .Value
                                .as_ident()
                                .expect("upgrade function field must reference an identifier");
                            // 函数表最后一项代表当前升级链能走到的最高版本，因此直接从尾元素抽取版本号。
                            // 这里默认 upgradeToVerFunctions 已按版本递增排列，这和 Go 源码维护约定保持一致。
                            maxVerFuncUsed = strconv::Atoi(&ident.Name["upgradeToVer".len()..])
                                .unwrap_or_else(|_| {
                                    panic!(
                                        "unexpected value of upgradeToVerFunctions: {}",
                                        ident.Name
                                    )
                                });
                            maxVerFuncUsedPos = lastElm.Pos();
                        }
                        "currentBootstrapVersion" => {
                            let valueIdent = v2.Values[0]
                                .as_ident()
                                .expect("currentBootstrapVersion must reference an identifier");
                            curVerVariablePos = valueIdent.Pos();
                            let value = valueIdent.Name.clone();
                            // currentBootstrapVersion 不是数字字面量，而是指向某个 versionN 常量的别名。
                            // 这样定义可以复用已有常量名，但也要求别名和常量序列始终同步推进。
                            curVerVariable = strconv::Atoi(&value["version".len()..])
                                .unwrap_or_else(|_| {
                                    panic!("unexpected value of currentBootstrapVersion: {}", value)
                                });
                        }
                        _ => continue,
                    }
                } else if v.Tok == token::CONST && v.Specs.len() > 1 {
                    // Go 要求 versionN 常量按递增顺序显式赋值，且变量名中的数字等于字面量值。
                    // 这让代码审查时只看常量名和字面量，就能快速确认升级链有没有断档。
                    for spec in &v.Specs {
                        let v2 = match spec.as_value_spec() {
                            Some(v) => v,
                            None => continue,
                        };
                        if v2.Names.len() != 1 {
                            continue;
                        }
                        let name = v2.Names[0].Name.clone();
                        if !strings::HasPrefix(&name, "version") {
                            continue;
                        }

                        // 常量名里的数字承担“排序”和“版本值”双重职责，所以必须能稳定解析成整数。
                        let valInName = match strconv::Atoi(&name["version".len()..]) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        if valInName < maxVerVariable {
                            // 版本常量必须单调递增，避免历史版本插队导致“最大版本”推导失真。
                            pass.Reportf(
                                spec.Pos(),
                                "version variable %q is not valid, we should have a increment list of version variables",
                                name,
                            );
                            continue;
                        }

                        maxVerVariable = valInName;
                        maxVerVariablePos = v2.Names[0].Pos();

                        if v2.Values.len() != 1 {
                            pass.Reportf(
                                spec.Pos(),
                                "the value of version variable %q must be specified explicitly",
                                name,
                            );
                            continue;
                        }

                        // 显式字面量值必须等于名字里的版本号，避免通过间接表达式制造难以审计的升级链。
                        let valStr = v2.Values[0]
                            .as_basic_lit()
                            .expect("version value must be a basic literal")
                            .Value
                            .clone();
                        let val = match strconv::Atoi(&valStr) {
                            Ok(v) => v,
                            Err(_) => {
                                pass.Reportf(
                                    spec.Pos(),
                                    "unexpected value of version variable %q: %q",
                                    name,
                                    valStr,
                                );
                                continue;
                            }
                        };

                        if val != valInName {
                            pass.Reportf(
                                spec.Pos(),
                                "the value of version variable %q must be '%d', but now is '%d'",
                                name,
                                valInName,
                                val,
                            );
                            continue;
                        }
                    }
                }
            }
            ast::Decl::FuncDecl(v) => {
                let name = v.Name.Name.clone();
                if !strings::HasPrefix(&name, "upgradeToVer") {
                    continue;
                }
                // 升级函数名本身就是版本元数据的一部分，因此直接从函数名后缀提取版本号。
                let t = match strconv::Atoi(&name["upgradeToVer".len()..]) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if t > maxVerFunc {
                    // 这里只记录最大的升级函数版本，最终再和其它三个版本来源做交叉校验。
                    maxVerFunc = t;
                    maxVerFuncPos = v.Pos();
                }
            }
            _ => {}
        }
    }

    // 四个来源的版本号必须完全一致：最新常量、最新函数、函数表最后一项、currentBootstrapVersion。
    let minv = [maxVerVariable, maxVerFunc, maxVerFuncUsed, curVerVariable]
        .into_iter()
        .min()
        .expect("version source list is non-empty");
    let maxv = [maxVerVariable, maxVerFunc, maxVerFuncUsed, curVerVariable]
        .into_iter()
        .max()
        .expect("version source list is non-empty");
    if minv == maxv && minv != 0 {
        // 只有四个来源都收敛到同一非零版本，才能说明升级链、常量和当前版本声明全部同步。
        return;
    }
    // 一旦不一致，就分别在各自来源位置报出数值，方便开发者快速定位漏改的是哪一处。
    pass.Reportf(maxVerFuncUsedPos, "found inconsistent bootstrap versions:");
    pass.Reportf(
        maxVerFuncUsedPos,
        "max version function used: %d",
        maxVerFuncUsed,
    );
    pass.Reportf(maxVerFuncPos, "max version function: %d", maxVerFunc);
    pass.Reportf(
        maxVerVariablePos,
        "max version variable: %d",
        maxVerVariable,
    );
    pass.Reportf(
        curVerVariablePos,
        "current version variable: %d",
        curVerVariable,
    );
}

// init 对应 Go 的 init：根据配置跳过该 analyzer。
pub fn init() {
    // bootstrap 校验通常只在修改引导逻辑时才会触发，保留配置化开关便于逐步接入。
    // 这样在大规模迁移期间可以先收集问题，再逐步把规则从提示提升为门禁。
    util::SkipAnalyzerByConfig(&Analyzer);
}
