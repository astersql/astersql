// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Anti Semi Join Probe 单元测试与 Go 侧完整测试的迁移草稿。
//
// 活跃测试覆盖：重复 build key 只输出未匹配行、NULL/恢复 Chunk/reset 路径、
// 左侧 build 后扫描未命中 build 行，以及 spill 仅保留未处理 probe 行。
// 块注释内保留 Go 版矩阵测试的中文说明，便于后续完整迁移。

/*

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

// genAntiSemiJoinResult 对应 Go 的 nested loop 期望结果生成器：只输出左侧未匹配任何右侧行的行。
pub fn genAntiSemiJoinResult(
    sessCtx: sessionctx::Context,
    leftChunks: Vec<chunk::Chunk>,
    rightChunks: Vec<chunk::Chunk>,
    leftKeyIndex: Vec<i32>,
    rightKeyIndex: Vec<i32>,
    leftTypes: Vec<types::FieldType>,
    rightTypes: Vec<types::FieldType>,
    leftKeyTypes: Vec<types::FieldType>,
    rightKeyTypes: Vec<types::FieldType>,
    leftUsedColumns: Vec<i32>,
    otherConditions: expression::CNFExprs,
    resultTypes: Vec<types::FieldType>,
) -> Vec<chunk::Chunk> {
    let mut returnChks = Vec::<chunk::Chunk>::with_capacity(1);
    let mut resultChk = chunk::New(resultTypes.clone(), sessCtx.GetSessionVars().MaxChunkSize, sessCtx.GetSessionVars().MaxChunkSize);
    let mut shallowRowTypes = Vec::<types::FieldType>::with_capacity(leftTypes.len() + rightTypes.len());
    shallowRowTypes.extend(leftTypes.clone());
    shallowRowTypes.extend(rightTypes.clone());
    let shallowRow = chunk::MutRowFromTypes(shallowRowTypes);

    for leftChunk in leftChunks {
        let mut isResult = true;
        for leftIndex in 0..leftChunk.NumRows() {
            let leftRow = leftChunk.GetRow(leftIndex);
            for rightChunk in &rightChunks {
                for rightIndex in 0..rightChunk.NumRows() {
                    if resultChk.IsFull() {
                        // 结果 chunk 满时立刻切分，保持 Go 的 MaxChunkSize 边界。
                        returnChks.push(resultChk);
                        resultChk = chunk::New(resultTypes.clone(), sessCtx.GetSessionVars().MaxChunkSize, sessCtx.GetSessionVars().MaxChunkSize);
                    }
                    let rightRow = rightChunk.GetRow(rightIndex);

                    let mut valid = !containsNullKey(leftRow, leftKeyIndex.clone()) && !containsNullKey(rightRow, rightKeyIndex.clone());
                    if !valid {
                        continue;
                    }

                    let (ok, err) = codec::EqualChunkRow(
                        sessCtx.GetSessionVars().StmtCtx.TypeCtx(),
                        leftRow,
                        leftKeyTypes.clone(),
                        leftKeyIndex.clone(),
                        rightRow,
                        rightKeyTypes.clone(),
                        rightKeyIndex.clone(),
                    );
                    require::NoError(err);
                    valid = ok;
                    if valid && otherConditions == nil {
                        isResult = false;
                        break;
                    }

                    if valid && otherConditions != nil {
                        // key is match, check other condition
                        // other condition 需要拼接左右 row 后求值；任何命中都让 anti semi 排除左侧行。
                        let (ok, err) = evalOtherCondition(sessCtx.clone(), leftRow, rightRow, shallowRow, otherConditions.clone());
                        require::NoError(err);
                        valid = ok;
                        if valid {
                            isResult = false;
                            break;
                        }
                    }
                }

                if !isResult {
                    break;
                }
            }

            if isResult {
                appendToResultChk(leftRow, chunk::Row {}, leftUsedColumns.clone(), nil, resultChk);
            }
            isResult = true;
        }
    }

    if resultChk.NumRows() > 0 {
        returnChks.push(resultChk);
    }
    returnChks
}

// constructInput 对应 Go 的小辅助：同时追加两列 datum 和两列 null 标记，避免 NOT IN fixture 构造重复。
pub fn constructInput(
    col0: &mut Vec<any>,
    col1: &mut Vec<any>,
    col0Nulls: &mut Vec<bool>,
    col1Nulls: &mut Vec<bool>,
    col0V: i64,
    col1V: i64,
    col0Null: bool,
    col1Null: bool,
) {
    col0.push(col0V);
    col1.push(col1V);
    col0Nulls.push(col0Null);
    col1Nulls.push(col1Null);
}

// buildNotInAntiSemiDataSourceAndExpectResult 对应 Go 的 NOT IN 数据源构造：混合 NULL、随机重复 key 和期望输出。
pub fn buildNotInAntiSemiDataSourceAndExpectResult(
    ctx: sessionctx::Context,
    leftCols: Vec<expression::Column>,
    rightCols: Vec<expression::Column>,
) -> (testutil::MockDataSource, testutil::MockDataSource, Vec<chunk::Row>) {
    let leftSchema = expression::NewSchema(leftCols);
    let rightSchema = expression::NewSchema(rightCols);

    let rowNum: i64 = 10000;
    let mut leftCol0Datums = Vec::<any>::with_capacity(rowNum as usize);
    let mut leftCol1Datums = Vec::<any>::with_capacity(rowNum as usize);
    let mut rightCol0Datums = Vec::<any>::with_capacity(rowNum as usize);
    let mut rightCol1Datums = Vec::<any>::with_capacity(rowNum as usize);
    let mut leftCol0Nulls = Vec::<bool>::with_capacity(rowNum as usize);
    let mut leftCol1Nulls = Vec::<bool>::with_capacity(rowNum as usize);
    let mut rightCol0Nulls = Vec::<bool>::with_capacity(rowNum as usize);
    let mut rightCol1Nulls = Vec::<bool>::with_capacity(rowNum as usize);

    let intTp = types::NewFieldType(mysql::TypeLonglong);
    let mut expectResultChunk = chunk::NewChunkWithCapacity(vec![intTp.clone(), intTp.clone()], 10000);

    // Fill null keys
    // 左侧 NULL key 在 anti semi 期望中保留；右侧 NULL key 只作为 NOT IN 语义覆盖数据。
    leftCol0Datums.push(0_i64);
    leftCol1Datums.push(0_i64);
    leftCol0Nulls.push(true);
    leftCol1Nulls.push(true);
    expectResultChunk.AppendNull(0);
    expectResultChunk.AppendNull(1);

    rightCol0Datums.push(0_i64);
    rightCol1Datums.push(0_i64);
    rightCol0Nulls.push(true);
    rightCol1Nulls.push(true);

    for i in 1..5 {
        constructInput(&mut leftCol0Datums, &mut leftCol1Datums, &mut leftCol0Nulls, &mut leftCol1Nulls, 0, i as i64, true, false);
        expectResultChunk.AppendNull(0);
        expectResultChunk.AppendInt64(1, i as i64);

        constructInput(&mut rightCol0Datums, &mut rightCol1Datums, &mut rightCol0Nulls, &mut rightCol1Nulls, 0, i as i64, true, false);
    }

    let distinctKeyNum = 100;
    for i in 0..distinctKeyNum {
        let leftSingleKeyNum = rand::Intn(maxChunkSizeInTest) + 1;

        if i % 2 == 0 {
            for _ in 0..leftSingleKeyNum {
                let col1V = rand::Int63n(100);
                let isCol1Null = rand::Intn(10) < 3;
                constructInput(&mut leftCol0Datums, &mut leftCol1Datums, &mut leftCol0Nulls, &mut leftCol1Nulls, i as i64, col1V, false, isCol1Null);
                expectResultChunk.AppendInt64(0, i as i64);
                if isCol1Null {
                    expectResultChunk.AppendNull(1);
                } else {
                    expectResultChunk.AppendInt64(1, col1V);
                }
            }
        } else {
            let mut rightCol1HasNull = false;
            let rightSingleKeyNum = rand::Intn(maxChunkSizeInTest) + 1;

            for j in 0..rightSingleKeyNum {
                let isNull = rand::Intn(100) < 0;
                if isNull {
                    rightCol1HasNull = true;
                }

                constructInput(&mut rightCol0Datums, &mut rightCol1Datums, &mut rightCol0Nulls, &mut rightCol1Nulls, i as i64, j as i64, false, isNull);
            }

            for j in 0..leftSingleKeyNum {
                if rightCol1HasNull {
                    let isCol1Null = rand::Intn(10) < 2;
                    constructInput(&mut leftCol0Datums, &mut leftCol1Datums, &mut leftCol0Nulls, &mut leftCol1Nulls, i as i64, j as i64, false, isCol1Null);
                } else {
                    let isCol1Null = rand::Intn(10) < 0;
                    let col1V = rand::Int63n((rightSingleKeyNum * 2) as i64);
                    constructInput(&mut leftCol0Datums, &mut leftCol1Datums, &mut leftCol0Nulls, &mut leftCol1Nulls, i as i64, col1V, false, isCol1Null);
                    if !isCol1Null && col1V >= rightSingleKeyNum as i64 {
                        expectResultChunk.AppendInt64(0, i as i64);
                        expectResultChunk.AppendInt64(1, col1V);
                    }
                }
            }
        }
    }

    let leftLen = leftCol0Datums.len();
    let rightLen = rightCol0Datums.len();

    // Shuffle
    // Fisher-Yates 形状保持 Go 的随机乱序，确保 hash join 不依赖输入顺序。
    for i in 0..leftLen {
        let j = rand::Int63n((i + 1) as i64) as usize;
        leftCol0Datums.swap(i, j);
        leftCol1Datums.swap(i, j);
        leftCol0Nulls.swap(i, j);
        leftCol1Nulls.swap(i, j);
    }

    for i in 0..rightLen {
        let j = rand::Int63n((i + 1) as i64) as usize;
        rightCol0Datums.swap(i, j);
        rightCol1Datums.swap(i, j);
        rightCol0Nulls.swap(i, j);
        rightCol1Nulls.swap(i, j);
    }

    let expectResult = sortRows(vec![expectResultChunk], semiJoinRetTypes);
    let leftMockSrcParm = testutil::MockDataSourceParameters {
        DataSchema: leftSchema,
        Ctx: ctx.clone(),
        Rows: leftLen,
        Ndvs: vec![-2, -2],
        Datums: vec![leftCol0Datums, leftCol1Datums],
        Nulls: vec![leftCol0Nulls, leftCol1Nulls],
        HasSel: false,
    };
    let rightMockSrcParm = testutil::MockDataSourceParameters {
        DataSchema: rightSchema,
        Ctx: ctx,
        Rows: rightLen,
        Ndvs: vec![-2, -2],
        Datums: vec![rightCol0Datums, rightCol1Datums],
        Nulls: vec![rightCol0Nulls, rightCol1Nulls],
        HasSel: false,
    };
    (
        testutil::BuildMockDataSource(leftMockSrcParm),
        testutil::BuildMockDataSource(rightMockSrcParm),
        expectResult,
    )
}

// testAntiSemiJoin 对应 Go 包装函数：复用 semi/anti semi join 公共测试入口。
pub fn testAntiSemiJoin(rightAsBuildSide: bool, hasOtherCondition: bool, hasDuplicateKey: bool) {
    testSemiOrAntiSemiJoin(rightAsBuildSide, hasOtherCondition, hasDuplicateKey, true);
}

// testNotInAntiSemi 对应 Go 的 NOT IN anti semi 测试主体：构造 otherCondition 并运行 hash join v2。
pub fn testNotInAntiSemi(rightAsBuildSide: bool) {
    let ctx = mock::NewContext();
    ctx.GetSessionVars().InitChunkSize = maxChunkSizeInTest;
    ctx.GetSessionVars().MaxChunkSize = maxChunkSizeInTest;
    let (leftDataSource, rightDataSource, expectedResult) =
        buildNotInAntiSemiDataSourceAndExpectResult(ctx.clone(), semiJoinleftCols, semiJoinrightCols);

    let intTp = types::NewFieldType(mysql::TypeLonglong);

    let leftKeys = vec![expression::Column { Index: 0, RetType: intTp.clone() }];
    let rightKeys = vec![expression::Column { Index: 0, RetType: intTp.clone() }];

    let (buildKeys, probeKeys) = if rightAsBuildSide {
        (rightKeys, leftKeys)
    } else {
        (leftKeys, rightKeys)
    };

    let mut otherCondition = expression::CNFExprs::new();
    let mut lUsedInOtherCondition = vec![];
    let mut rUsedInOtherCondition = vec![];
    lUsedInOtherCondition.push(1);
    rUsedInOtherCondition.push(1);

    let tinyTp = types::NewFieldType(mysql::TypeTiny);
    let a = expression::Column { Index: 1, RetType: intTp.clone(), InOperand: true };
    let b = expression::Column { Index: 3, RetType: intTp.clone(), InOperand: true };
    let (sf, err) = expression::NewFunction(mock::NewContext(), ast::EQ, tinyTp, a, b);
    require::NoError(err, "error when create other condition");
    otherCondition.push(sf);

    let info = hashJoinInfo {
        ctx: ctx.clone(),
        schema: buildSchema(semiJoinRetTypes),
        leftExec: leftDataSource,
        rightExec: rightDataSource,
        joinType: base::AntiSemiJoin,
        rightAsBuildSide,
        buildKeys,
        probeKeys,
        lUsed: vec![0, 1],
        rUsed: vec![],
        otherCondition,
        lUsedInOtherCondition,
        rUsedInOtherCondition,
    };

    leftDataSource.PrepareChunks();
    rightDataSource.PrepareChunks();

    let hashJoinExec = buildHashJoinV2Exec(info);
    let result = getSortedResults(hashJoinExec, semiJoinRetTypes);
    checkResults(semiJoinRetTypes, result, expectedResult);
}

// TestAntiSemiJoinBasic 对应 Go 基础矩阵：左右 build side 与是否存在 other condition 的笛卡尔组合。
#[test]
pub fn TestAntiSemiJoinBasic() {
    testAntiSemiJoin(false, false, false); // Left side build without other condition
    testAntiSemiJoin(false, true, false);  // Left side build with other condition
    testAntiSemiJoin(true, false, false);  // Right side build without other condition
    testAntiSemiJoin(true, true, false);   // Right side build with other condition
}

// TestAntiSemiJoinDuplicateKeys 对应 Go 重复 key 矩阵：复用基础组合但打开 duplicate key 数据。
#[test]
pub fn TestAntiSemiJoinDuplicateKeys() {
    testAntiSemiJoin(false, false, true); // Left side build without other condition
    testAntiSemiJoin(false, true, true);  // Left side build with other condition
    testAntiSemiJoin(true, false, true);  // Right side build without other condition
    testAntiSemiJoin(true, true, true);   // Right side build with other condition
}

// sql: select * from t1 where col1 not in (select col1 from t2 where t1.col0 = t2.col0);
// TestNotInWithAntiSemi 对应 Go 的 NOT IN SQL 语义覆盖：左右侧作为 build side 各跑一次。
#[test]
pub fn TestNotInWithAntiSemi() {
    testNotInAntiSemi(true);
    testNotInAntiSemi(false);
}

// TestAntiSemiJoinProbeBasic 对应 Go 的 probe 基础用例：覆盖 used columns、空输出列、int/uint 和多 key。
#[test]
pub fn TestAntiSemiJoinProbeBasic() {
    // todo test nullable type after builder support nullable type
    let tinyTp = nonNullType(mysql::TypeTiny);
    let intTp = nonNullType(mysql::TypeLonglong);
    let uintTp = unsignedNonNullType(mysql::TypeLonglong);
    let stringTp = nonNullType(mysql::TypeVarString);

    let lTypes = vec![intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone(), tinyTp.clone()];
    let mut rTypes = vec![intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone(), tinyTp.clone()];
    rTypes.extend(retTypes);
    let mut rTypes1 = vec![uintTp.clone(), stringTp.clone(), intTp.clone(), stringTp.clone(), tinyTp.clone()];
    rTypes1.extend(rTypes1.clone());

    let rightAsBuildSide = vec![true, false];
    let partitionNumber = 4;
    let joinType = base::AntiSemiJoin;

    let testCases = vec![
        // normal case
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), nil, vec![], nil, nil, nil),
        // rightUsed is empty
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), vec![0, 1, 2, 3], vec![], nil, nil, nil),
        // leftUsed is empty
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), vec![], vec![], nil, nil, nil),
        // both left/right Used are empty
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), vec![], vec![], nil, nil, nil),
        // both left/right used is part of all columns
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), vec![0, 2], vec![], nil, nil, nil),
        // int join uint
        testCase::new(vec![0], vec![0], vec![intTp.clone()], vec![uintTp.clone()], lTypes.clone(), rTypes1.clone(), vec![0, 1, 2, 3], vec![], nil, nil, nil),
        // multiple join keys
        testCase::new(vec![0, 1], vec![0, 1], vec![intTp.clone(), stringTp.clone()], vec![intTp.clone(), stringTp.clone()], lTypes.clone(), rTypes.clone(), vec![0, 1, 2, 3], vec![], nil, nil, nil),
    ];

    for tc in testCases {
        for value in &rightAsBuildSide {
            testJoinProbe(false, tc.leftKeyIndex, tc.rightKeyIndex, tc.leftKeyTypes, tc.rightKeyTypes, tc.leftTypes, tc.rightTypes, *value, tc.leftUsed,
                tc.rightUsed, tc.leftUsedByOtherCondition, tc.rightUsedByOtherCondition, nil, nil, tc.otherCondition, partitionNumber, joinType, 200);
            // 第二轮把 key/type 全部转成 nullable，保持 Go 对 nullable builder 的覆盖。
            testJoinProbe(false, tc.leftKeyIndex, tc.rightKeyIndex, toNullableTypes(tc.leftKeyTypes), toNullableTypes(tc.rightKeyTypes),
                toNullableTypes(tc.leftTypes), toNullableTypes(tc.rightTypes), *value, tc.leftUsed, tc.rightUsed, tc.leftUsedByOtherCondition,
                tc.rightUsedByOtherCondition, nil, nil, tc.otherCondition, partitionNumber, joinType, 200);
        }
    }
}

// TestAntiSemiJoinProbeAllJoinKeys 对应 Go 的全类型 join key 覆盖：单 key 和多种组合 key 都在左右 build side 下测试。
#[test]
pub fn TestAntiSemiJoinProbeAllJoinKeys() {
    let tinyTp = nonNullType(mysql::TypeTiny);
    let intTp = nonNullType(mysql::TypeLonglong);
    let uintTp = unsignedNonNullType(mysql::TypeLonglong);
    let yearTp = nonNullType(mysql::TypeYear);
    let durationTp = nonNullType(mysql::TypeDuration);
    let enumTp = nonNullType(mysql::TypeEnum);
    let enumWithIntFlag = enumSetAsIntNonNullType(mysql::TypeEnum);
    let setTp = nonNullType(mysql::TypeSet);
    let bitTp = nonNullType(mysql::TypeBit);
    let jsonTp = nonNullType(mysql::TypeJSON);
    let floatTp = nonNullType(mysql::TypeFloat);
    let doubleTp = nonNullType(mysql::TypeDouble);
    let stringTp = nonNullType(mysql::TypeVarString);
    let datetimeTp = nonNullType(mysql::TypeDatetime);
    let decimalTp = nonNullType(mysql::TypeNewDecimal);
    let timestampTp = nonNullType(mysql::TypeTimestamp);
    let dateTp = nonNullType(mysql::TypeDate);
    let binaryStringTp = nonNullType(mysql::TypeBlob);

    let lTypes = vec![
        tinyTp.clone(), intTp.clone(), uintTp.clone(), yearTp.clone(), durationTp.clone(), enumTp.clone(), enumWithIntFlag.clone(),
        setTp.clone(), bitTp.clone(), jsonTp.clone(), floatTp.clone(), doubleTp.clone(), stringTp.clone(), datetimeTp.clone(),
        decimalTp.clone(), timestampTp.clone(), dateTp.clone(), binaryStringTp.clone(),
    ];
    let rTypes = lTypes.clone();
    let lUsed = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
    let rUsed = vec![];
    let joinType = base::AntiSemiJoin;
    let partitionNumber = 4;
    let rightAsBuildSide = vec![true, false];

    // single key
    for i in 0..lTypes.len() {
        let lKeyTypes = vec![lTypes[i].clone()];
        let rKeyTypes = vec![rTypes[i].clone()];
        for rightAsBuild in &rightAsBuildSide {
            testJoinProbe(false, vec![i], vec![i], lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
            testJoinProbe(false, vec![i], vec![i], toNullableTypes(lKeyTypes.clone()), toNullableTypes(rKeyTypes.clone()), toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
        }
    }

    // composed key
    // fixed size, inlined
    for rightAsBuild in &rightAsBuildSide {
        let lKeyTypes = vec![intTp.clone(), uintTp.clone()];
        let rKeyTypes = vec![intTp.clone(), uintTp.clone()];
        testJoinProbe(false, vec![1, 2], vec![1, 2], lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
        testJoinProbe(false, vec![1, 2], vec![1, 2], toNullableTypes(lKeyTypes), toNullableTypes(rKeyTypes), toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
    }
    // variable size, inlined
    for rightAsBuild in &rightAsBuildSide {
        let lKeyTypes = vec![intTp.clone(), binaryStringTp.clone()];
        let rKeyTypes = vec![intTp.clone(), binaryStringTp.clone()];
        testJoinProbe(false, vec![1, 17], vec![1, 17], lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
        testJoinProbe(false, vec![1, 17], vec![1, 17], toNullableTypes(lKeyTypes), toNullableTypes(rKeyTypes), toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
    }
    // fixed size, not inlined
    for rightAsBuild in &rightAsBuildSide {
        let lKeyTypes = vec![intTp.clone(), datetimeTp.clone()];
        let rKeyTypes = vec![intTp.clone(), datetimeTp.clone()];
        testJoinProbe(false, vec![1, 13], vec![1, 13], lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
        testJoinProbe(false, vec![1, 13], vec![1, 13], toNullableTypes(lKeyTypes), toNullableTypes(rKeyTypes), toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
    }
    // variable size, not inlined
    for rightAsBuild in &rightAsBuildSide {
        let lKeyTypes = vec![intTp.clone(), decimalTp.clone()];
        let rKeyTypes = vec![intTp.clone(), decimalTp.clone()];
        testJoinProbe(false, vec![1, 14], vec![1, 14], lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
        testJoinProbe(false, vec![1, 14], vec![1, 14], toNullableTypes(lKeyTypes), toNullableTypes(rKeyTypes), toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), *rightAsBuild, lUsed.clone(), rUsed.clone(), nil, nil, nil, nil, nil, partitionNumber, joinType, 100);
    }
}

// TestAntiSemiJoinJoinProbeWithSel 对应 Go 的带 selection probe 测试：other condition 使用左右侧不同列，并覆盖空 lUsed。
#[test]
pub fn TestAntiSemiJoinJoinProbeWithSel() {
    let intTp = nonNullType(mysql::TypeLonglong);
    let nullableIntTp = types::NewFieldType(mysql::TypeLonglong);
    let uintTp = unsignedNonNullType(mysql::TypeLonglong);
    let nullableUIntTp = unsignedType(mysql::TypeLonglong);
    let stringTp = nonNullType(mysql::TypeVarString);

    let lTypes = vec![intTp.clone(), intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone()];
    let mut rTypes = vec![intTp.clone(), intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone()];
    rTypes.extend(rTypes.clone());

    let tinyTp = types::NewFieldType(mysql::TypeTiny);
    let a = expression::Column { Index: 1, RetType: nullableIntTp.clone() };
    let b = expression::Column { Index: 8, RetType: nullableUIntTp.clone() };
    let (sf, err) = expression::NewFunction(mock::NewContext(), ast::GT, tinyTp, a, b);
    require::NoError(err, "error when create other condition");
    let mut otherCondition = expression::CNFExprs::new();
    otherCondition.push(sf);

    let joinType = base::AntiSemiJoin;
    let rightAsBuildSide = vec![true, false];
    let partitionNumber = 4;
    let rightUsed = vec![];

    for rightBuild in rightAsBuildSide {
        testJoinProbe(true, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightBuild, vec![1, 2, 4], rightUsed.clone(), vec![1], vec![3], nil, nil, otherCondition.clone(), partitionNumber, joinType, 500);
        testJoinProbe(true, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightBuild, vec![], rightUsed.clone(), vec![1], vec![3], nil, nil, otherCondition.clone(), partitionNumber, joinType, 500);
        testJoinProbe(true, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightBuild, vec![1, 2, 4], rightUsed.clone(), vec![1], vec![3], nil, nil, otherCondition.clone(), partitionNumber, joinType, 500);
        testJoinProbe(true, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightBuild, nil, rightUsed.clone(), vec![1], vec![3], nil, nil, otherCondition.clone(), partitionNumber, joinType, 500);
    }
}

// nonNullType 对应测试中反复出现的 FieldType + NotNullFlag 构造，便于 Rust 少重复 Go 样板。
fn nonNullType(tp: u8) -> types::FieldType {
    let fieldType = types::NewFieldType(tp);
    fieldType.AddFlag(mysql::NotNullFlag);
    fieldType
}

// unsignedNonNullType 对应 Go 中 unsigned + not null 的整数类型构造。
fn unsignedNonNullType(tp: u8) -> types::FieldType {
    let fieldType = types::NewFieldType(tp);
    fieldType.AddFlag(mysql::NotNullFlag);
    fieldType.AddFlag(mysql::UnsignedFlag);
    fieldType
}

// unsignedType 对应 Go 中 nullable unsigned 类型构造。
fn unsignedType(tp: u8) -> types::FieldType {
    let fieldType = types::NewFieldType(tp);
    fieldType.AddFlag(mysql::UnsignedFlag);
    fieldType
}

// enumSetAsIntNonNullType 对应 Go 中 EnumSetAsIntFlag + NotNullFlag 的特殊 enum 类型。
fn enumSetAsIntNonNullType(tp: u8) -> types::FieldType {
    let fieldType = types::NewFieldType(tp);
    fieldType.AddFlag(mysql::EnumSetAsIntFlag);
    fieldType.AddFlag(mysql::NotNullFlag);
    fieldType
}
*/

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造单列 Row，便于测试里快速拼装键值。
fn row(value: Value) -> Row {
    vec![value]
}

/// 创建 AntiSemi Join Probe：build 键与 probe 键均为第 0 列。
fn anti_probe(build: Vec<Row>, right_build: bool, max_chunk: usize) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        max_chunk,
    )
    .unwrap();
    let context = HashJoinContext::new(
        build,
        vec![0],
        vec![0],
        joiner,
        right_build,
        true,
        max_chunk,
    );
    new_join_probe(context, 0, JoinType::AntiSemi, right_build, false).unwrap()
}

#[test]
/// 右侧 build 且存在重复 key 时，仅输出在 build 侧找不到匹配的 probe 行。
fn anti_semi_probe_returns_only_unmatched_rows_with_duplicate_build_keys() {
    let mut probe = anti_probe(
        vec![row(Value::Int(1)), row(Value::Int(1)), row(Value::Int(3))],
        true,
        32,
    );
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// 覆盖 restored Chunk、NULL probe 行输出，以及 reset 后状态清空。
fn anti_semi_probe_preserves_null_restored_and_reset_paths() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_restored_chunk_for_probe(vec![row(Value::Null), row(Value::Int(2))])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Null)]);
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    probe.reset_probe();
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// 左侧 build：probe 阶段不输出，随后扫描 build 表得到未命中行。
fn anti_semi_left_build_scans_unmatched_build_rows_after_probe() {
    let mut probe = anti_probe(
        vec![row(Value::Int(1)), row(Value::Int(2)), row(Value::Int(3))],
        false,
        8,
    );
    probe
        .set_chunk_for_probe(vec![row(Value::Int(1)), row(Value::Int(3))])
        .unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, [row(Value::Int(2))]);
    assert!(probe.is_scan_row_table_done());
}

#[test]
/// Go parity: left-build anti semi treats a NULL other-condition result as used.
fn anti_semi_left_build_null_condition_suppresses_build_row() {
    let condition: Predicate = Arc::new(|_| Ok(None));
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(Value::Int(1))],
        vec![0],
        vec![0],
        joiner,
        false,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::AntiSemi, false, false).unwrap();
    probe.set_chunk_for_probe(vec![row(Value::Int(1))]).unwrap();

    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert!(probe.scan_row_table().rows.is_empty());
}

#[test]
/// spill 只带走尚未处理的 probe 行，已输出的匹配结果不重复刷盘。
fn anti_semi_spills_only_unprocessed_probe_rows() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        [vec![row(Value::Int(3))]]
    );
}

#[test]
/// NULL build keys do not match either NULL or non-NULL probe keys in ordinary anti join.
fn anti_semi_null_build_key_is_not_equal() {
    let mut probe = anti_probe(vec![row(Value::Null), row(Value::Int(1))], true, 8);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Null),
            row(Value::Int(1)),
            row(Value::Int(2)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Null), row(Value::Int(2))]);
}

#[test]
/// A capacity-limited anti probe resumes at the next probe row without duplicating output.
fn anti_semi_probe_resumes_after_required_rows() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert_eq!(probe.probe().rows, [row(Value::Int(3))]);
    assert!(probe.is_current_chunk_probe_done());
}
