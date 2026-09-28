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

// Hash Join V2 Inner Join 探测路径的单元测试。
//
// 覆盖：重复 key 匹配与 hash miss、other condition 过滤与 chunk 容量切分、
// restore/spill 后剩余探测行保留，以及探测 key 列越界拒绝。

/*
// HashJoin V2 probe 阶段的共享测试辅助和 inner join 场景。

#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]

// toNullableTypes 对应 Go helper：复制 FieldType 并去掉 NotNullFlag。
// 调用方用它复用同一批 join case，覆盖 nullable key 和 nullable row 类型。
pub fn toNullableTypes(tps: Vec<types::FieldType>) -> Vec<types::FieldType> {
    let mut ret = Vec::with_capacity(tps.len());
    for tp in tps {
        let mut nullableTp = tp.Clone();
        nullableTp.DelFlag(mysql::NotNullFlag);
        ret.push(nullableTp);
    }
    ret
}

// evalOtherCondition 对应 Go 的 other condition 求值：把左右行浅拷贝到同一 MutRow 后 EvalBool。
pub fn evalOtherCondition(
    sessCtx: sessionctx::Context,
    leftRow: chunk::Row,
    rightRow: chunk::Row,
    mut shallowRow: chunk::MutRow,
    otherCondition: expression::CNFExprs,
) -> Result<bool, Error> {
    shallowRow.ShallowCopyPartialRow(0, leftRow.clone());
    shallowRow.ShallowCopyPartialRow(leftRow.Len(), rightRow);
    let (valid, _, err) = expression::EvalBool(sessCtx.GetExprCtx().GetEvalCtx(), otherCondition, shallowRow.ToRow());
    err?;
    Ok(valid)
}

// appendToResultChk 对应 Go 的结果 chunk 拼接逻辑。
// 空行表示 outer/semi 场景的补 NULL；当前 inner join 测试主要复用该共享 helper。
pub fn appendToResultChk(
    leftRow: chunk::Row,
    rightRow: chunk::Row,
    leftUsedColumns: Vec<i32>,
    rightUsedColumns: Vec<i32>,
    resultChunk: &mut chunk::Chunk,
) {
    let mut lWide = 0;
    if leftRow.IsEmpty() {
        for index in 0..leftUsedColumns.len() {
            resultChunk.Column(index).AppendNull();
        }
        resultChunk.SetNumVirtualRows(resultChunk.NumRows() + 1);
        lWide = leftUsedColumns.len();
    } else {
        lWide = resultChunk.AppendRowByColIdxs(leftRow, leftUsedColumns);
    }

    if rightRow.IsEmpty() {
        for index in 0..rightUsedColumns.len() {
            resultChunk.Column(index + lWide).AppendNull();
        }
    } else {
        resultChunk.AppendPartialRowByColIdxs(lWide, rightRow, rightUsedColumns);
    }
}

// containsNullKey 对应 Go 的 slices.ContainsFunc(row.IsNull)，用于 NULL join key 过滤。
pub fn containsNullKey(row: chunk::Row, keyIndex: Vec<i32>) -> bool {
    keyIndex.iter().any(|idx| row.IsNull(*idx))
}

// genInnerJoinResult 对应 Go 的 nested-loop 期望结果生成器。
// 它不走 hash table，而是逐行比较 key 和 other condition，用来校验 probe 输出。
pub fn genInnerJoinResult(
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
    rightUsedColumns: Vec<i32>,
    otherConditions: expression::CNFExprs,
    resultTypes: Vec<types::FieldType>,
) -> Vec<chunk::Chunk> {
    let mut returnChks = Vec::with_capacity(1);
    let mut resultChk = chunk::New(resultTypes.clone(), sessCtx.GetSessionVars().MaxChunkSize, sessCtx.GetSessionVars().MaxChunkSize);
    let mut shallowRowTypes = Vec::with_capacity(leftTypes.len() + rightTypes.len());
    shallowRowTypes.extend(leftTypes.clone());
    shallowRowTypes.extend(rightTypes.clone());
    let shallowRow = chunk::MutRowFromTypes(shallowRowTypes);

    // Go 注释说明：right outer join 用 left 作 build，其它 join 默认 right 作 build；这里按输入左右侧做穷举。
    for leftChunk in leftChunks {
        for leftIndex in 0..leftChunk.NumRows() {
            let leftRow = leftChunk.GetRow(leftIndex);
            for rightChunk in &rightChunks {
                for rightIndex in 0..rightChunk.NumRows() {
                    if resultChk.IsFull() {
                        returnChks.push(resultChk);
                        resultChk = chunk::New(resultTypes.clone(), sessCtx.GetSessionVars().MaxChunkSize, sessCtx.GetSessionVars().MaxChunkSize);
                    }

                    let rightRow = rightChunk.GetRow(rightIndex);
                    let mut valid = !containsNullKey(leftRow.clone(), leftKeyIndex.clone())
                        && !containsNullKey(rightRow.clone(), rightKeyIndex.clone());
                    if valid {
                        let ok = codec::EqualChunkRow(
                            sessCtx.GetSessionVars().StmtCtx.TypeCtx(),
                            leftRow.clone(),
                            leftKeyTypes.clone(),
                            leftKeyIndex.clone(),
                            rightRow.clone(),
                            rightKeyTypes.clone(),
                            rightKeyIndex.clone(),
                        ).unwrap();
                        valid = ok;
                    }
                    if valid && !otherConditions.is_empty() {
                        // key 命中后才计算 other condition，和 Go 的短路顺序一致。
                        let ok = evalOtherCondition(sessCtx.clone(), leftRow.clone(), rightRow.clone(), shallowRow.clone(), otherConditions.clone()).unwrap();
                        valid = ok;
                    }
                    if valid {
                        appendToResultChk(leftRow.clone(), rightRow, leftUsedColumns.clone(), rightUsedColumns.clone(), &mut resultChk);
                    }
                }
            }
        }
    }
    if resultChk.NumRows() > 0 {
        returnChks.push(resultChk);
    }
    returnChks
}

// checkVirtualRows 对应 Go helper：确认每个结果 chunk 都不是 incomplete chunk，且列行数等于 virtual rows。
pub fn checkVirtualRows(resultChunks: Vec<chunk::Chunk>) {
    for chk in resultChunks {
        require::Equal(false, chk.IsInCompleteChunk());
        let numRows = chk.GetNumVirtualRows();
        for i in 0..chk.NumCols() {
            require::Equal(numRows, chk.Column(i).Rows());
        }
    }
}

// checkChunksEqual 对应 Go helper：按 schema compare func 排序后逐行比较 expected/result。
// 该测试允许输出顺序不同，但要求多重集合内容一致。
pub fn checkChunksEqual(expectedChunks: Vec<chunk::Chunk>, resultChunks: Vec<chunk::Chunk>, schema: Vec<types::FieldType>) {
    let expectedNum = expectedChunks.iter().map(|chk| chk.NumRows()).sum::<i32>();
    let resultNum = resultChunks.iter().map(|chk| chk.NumRows()).sum::<i32>();
    require::Equal(expectedNum, resultNum);
    if expectedNum == 0 || schema.is_empty() {
        return;
    }

    let cmpFuncs = schema.iter().map(|colType| chunk::GetCompareFunc(colType.clone())).collect::<Vec<_>>();
    let mut expectedRows = Vec::with_capacity(expectedNum as usize);
    let mut resultRows = Vec::with_capacity(expectedNum as usize);

    for chk in expectedChunks {
        let mut iter = chunk::NewIterator4Chunk(chk);
        let mut row = iter.Begin();
        while row != iter.End() {
            expectedRows.push(row.clone());
            row = iter.Next();
        }
    }
    for chk in resultChunks {
        let mut iter = chunk::NewIterator4Chunk(chk);
        let mut row = iter.Begin();
        while row != iter.End() {
            resultRows.push(row.clone());
            row = iter.Next();
        }
    }

    let cmp = |rowI: &chunk::Row, rowJ: &chunk::Row| -> i32 {
        for (i, cmpFunc) in cmpFuncs.iter().enumerate() {
            let ret = cmpFunc(rowI.clone(), i, rowJ.clone(), i);
            if ret != 0 {
                return ret;
            }
        }
        0
    };
    expectedRows.sort_by(|i, j| cmp(i, j).cmp(&0));
    resultRows.sort_by(|i, j| cmp(i, j).cmp(&0));

    for i in 0..expectedRows.len() {
        let x = cmp(&expectedRows[i], &resultRows[i]);
        require::Equal(0, x, format!("result index = {}", i));
    }
}

// copySelectedRows 对应 Go helper：按 selected bitmap 从 src 拷贝到 dst。
// src/dst 有 selection vector 时 Go 直接返回错误；零列 chunk 只更新 virtual rows。
pub fn copySelectedRows(src: &chunk::Chunk, dst: &mut chunk::Chunk, selected: Vec<bool>) -> Result<bool, Error> {
    if src.NumRows() == 0 {
        return Ok(false);
    }
    if src.Sel().is_some() || dst.Sel().is_some() {
        return Err(errors::New("copy with sel"));
    }
    if src.NumCols() == 0 {
        let numSelected = selected.iter().filter(|s| **s).count() as i32;
        dst.SetNumVirtualRows(dst.GetNumVirtualRows() + numSelected);
        return Ok(numSelected > 0);
    }

    let oldLen = dst.NumRows();
    for j in 0..src.NumCols() {
        if j >= dst.NumCols() {
            break;
        }
        let srcCol = src.Column(j);
        let dstCol = dst.Column(j);
        chunk::CopySelectedRows(dstCol, srcCol, selected.clone());
    }
    let numSelected = dst.NumRows() - oldLen;
    dst.SetNumVirtualRows(dst.GetNumVirtualRows() + numSelected);
    Ok(numSelected > 0)
}

// testJoinProbe 对应 Go 主测试驱动：准备 build/probe chunks，构建 row table/hash table，再 probe 并比对期望结果。
// 这个函数也服务其它 join 类型测试；本文件只迁移其声明和 inner join 调用。
#[allow(clippy::too_many_arguments)]
pub fn testJoinProbe(
    withSel: bool,
    leftKeyIndex: Vec<i32>,
    rightKeyIndex: Vec<i32>,
    leftKeyTypes: Vec<types::FieldType>,
    rightKeyTypes: Vec<types::FieldType>,
    leftTypes: Vec<types::FieldType>,
    rightTypes: Vec<types::FieldType>,
    rightAsBuildSide: bool,
    mut leftUsed: Option<Vec<i32>>,
    mut rightUsed: Option<Vec<i32>>,
    leftUsedByOtherCondition: Vec<i32>,
    rightUsedByOtherCondition: Vec<i32>,
    leftFilter: expression::CNFExprs,
    rightFilter: expression::CNFExprs,
    otherCondition: expression::CNFExprs,
    mut partitionNumber: i32,
    joinType: base::JoinType,
    inputRowNumber: i32,
) {
    // leftUsed/rightUsed 为 nil 时表示选择所有列。
    if leftUsed.is_none() {
        leftUsed = Some((0..leftTypes.len() as i32).collect());
    }
    if rightUsed.is_none() {
        rightUsed = Some((0..rightTypes.len() as i32).collect());
    }
    let leftUsed = leftUsed.unwrap();
    let rightUsed = rightUsed.unwrap();

    let (mut buildKeyIndex, mut probeKeyIndex) = (leftKeyIndex.clone(), rightKeyIndex.clone());
    let (mut buildKeyTypes, mut probeKeyTypes) = (leftKeyTypes.clone(), rightKeyTypes.clone());
    let (mut buildTypes, mut probeTypes) = (leftTypes.clone(), rightTypes.clone());
    let mut buildUsed = leftUsed.clone();
    let mut buildUsedByOtherCondition = leftUsedByOtherCondition.clone();
    let (mut buildFilter, mut probeFilter) = (leftFilter.clone(), rightFilter.clone());
    let mut needUsedFlag = false;

    if rightAsBuildSide {
        probeKeyIndex = leftKeyIndex.clone();
        buildKeyIndex = rightKeyIndex.clone();
        probeKeyTypes = leftKeyTypes.clone();
        buildKeyTypes = rightKeyTypes.clone();
        probeTypes = leftTypes.clone();
        buildTypes = rightTypes.clone();
        buildUsed = rightUsed.clone();
        buildUsedByOtherCondition = rightUsedByOtherCondition.clone();
        buildFilter = rightFilter.clone();
        probeFilter = leftFilter.clone();
        if joinType == base::RightOuterJoin {
            needUsedFlag = true;
        }
    } else {
        match joinType {
            base::LeftOuterJoin | base::SemiJoin | base::AntiSemiJoin => needUsedFlag = true,
            base::LeftOuterSemiJoin | base::AntiLeftOuterSemiJoin => {
                require::NoError(errors::New("left semi/anti join does not support use left as build side"));
            }
            _ => {}
        }
    }

    match joinType {
        base::InnerJoin => {
            require::Equal(0, leftFilter.len(), "inner join does not support left filter");
            require::Equal(0, rightFilter.len(), "inner join does not support right filter");
        }
        base::LeftOuterJoin => require::Equal(0, rightFilter.len(), "left outer join does not support right filter"),
        base::RightOuterJoin => require::Equal(0, leftFilter.len(), "right outer join does not support left filter"),
        base::SemiJoin | base::AntiSemiJoin => {
            require::Equal(0, leftFilter.len(), "semi/anti join does not support left filter");
            require::Equal(0, rightFilter.len(), "semi/anti join does not support right filter");
        }
        base::LeftOuterSemiJoin | base::AntiLeftOuterSemiJoin => require::Equal(0, rightFilter.len(), "left outer semi/anti join does not support right filter"),
        _ => {}
    }

    let mut joinedTypes = Vec::with_capacity(leftTypes.len() + rightTypes.len());
    joinedTypes.extend(leftTypes.clone());
    joinedTypes.extend(rightTypes.clone());
    let mut resultTypes = Vec::with_capacity(leftUsed.len() + rightUsed.len());
    for colIndex in &leftUsed {
        let mut tp = leftTypes[*colIndex as usize].Clone();
        if joinType == base::RightOuterJoin {
            tp.DelFlag(mysql::NotNullFlag);
        }
        resultTypes.push(tp);
    }
    for colIndex in &rightUsed {
        let mut tp = rightTypes[*colIndex as usize].Clone();
        if joinType == base::LeftOuterJoin {
            tp.DelFlag(mysql::NotNullFlag);
        }
        resultTypes.push(tp);
    }
    if joinType == base::LeftOuterSemiJoin || joinType == base::AntiLeftOuterSemiJoin {
        resultTypes.push(types::NewFieldType(mysql::TypeTiny));
    }

    let meta = newTableMeta(buildKeyIndex.clone(), buildTypes.clone(), buildKeyTypes.clone(), probeKeyTypes.clone(), buildUsedByOtherCondition.clone(), buildUsed.clone(), needUsedFlag);
    let mut hashJoinCtx = HashJoinCtxV2 {
        hashTableMeta: meta.clone(),
        BuildFilter: buildFilter,
        ProbeFilter: probeFilter,
        OtherCondition: otherCondition.clone(),
        BuildKeyTypes: buildKeyTypes.clone(),
        ProbeKeyTypes: probeKeyTypes.clone(),
        RightAsBuildSide: rightAsBuildSide,
        LUsed: leftUsed.clone(),
        RUsed: rightUsed.clone(),
        LUsedInOtherCondition: leftUsedByOtherCondition.clone(),
        RUsedInOtherCondition: rightUsedByOtherCondition.clone(),
        ..Default::default()
    };
    hashJoinCtx.SessCtx = mock::NewContext();
    hashJoinCtx.JoinType = joinType;
    hashJoinCtx.Concurrency = partitionNumber as u32;
    hashJoinCtx.SetupPartitionInfo();
    partitionNumber = hashJoinCtx.partitionNumber as i32;
    hashJoinCtx.spillHelper = newHashJoinSpillHelper(None, partitionNumber, None, "");
    hashJoinCtx.initHashTableContext();

    let mut joinProbe = NewJoinProbe(hashJoinCtx.clone(), 0, joinType, probeKeyIndex.clone(), joinedTypes.clone(), probeKeyTypes.clone(), rightAsBuildSide);
    let mut hasNullableKey = false;
    for buildKeyType in &buildKeyTypes {
        if !mysql::HasNotNullFlag(buildKeyType.GetFlag()) {
            hasNullableKey = true;
            break;
        }
    }
    let mut builder = createRowTableBuilder(buildKeyIndex, buildKeyTypes, hashJoinCtx.partitionNumber, hasNullableKey, !hashJoinCtx.BuildFilter.is_empty(), joinProbe.NeedScanRowTable(), meta.nullMapLength);

    let chunkNumber = 3;
    let mut buildChunks = Vec::with_capacity(chunkNumber);
    let mut probeChunks = Vec::with_capacity(chunkNumber);
    let selected = (0..inputRowNumber).map(|i| i % 3 == 0).collect::<Vec<_>>();

    // Go 在生成数据前检查 build/probe 对应列的固定长度兼容，避免随机数据无法直接拷贝。
    for i in 0..buildTypes.len().min(probeTypes.len()) {
        let buildLength = chunk::GetFixedLen(buildTypes[i].clone());
        let probeLength = chunk::GetFixedLen(probeTypes[i].clone());
        require::Equal(buildLength, probeLength, "build type and probe type is not compatible");
    }
    for _ in 0..chunkNumber {
        if buildTypes.len() >= probeTypes.len() {
            let buildChunk = testutil::GenRandomChunks(buildTypes.clone(), inputRowNumber);
            let mut probeChunk = testutil::GenRandomChunks(probeTypes.clone(), inputRowNumber * 2 / 3);
            copySelectedRows(&buildChunk, &mut probeChunk, selected.clone()).unwrap();
            buildChunks.push(buildChunk);
            probeChunks.push(probeChunk);
        } else {
            let probeChunk = testutil::GenRandomChunks(probeTypes.clone(), inputRowNumber);
            let mut buildChunk = testutil::GenRandomChunks(buildTypes.clone(), inputRowNumber * 2 / 3);
            copySelectedRows(&probeChunk, &mut buildChunk, selected.clone()).unwrap();
            probeChunks.push(probeChunk);
            buildChunks.push(buildChunk);
        }
    }

    if withSel {
        let sel = (0..inputRowNumber).filter(|i| i % 9 != 0).collect::<Vec<_>>();
        for chk in &mut buildChunks {
            chk.SetSel(sel.clone());
        }
        for chk in &mut probeChunks {
            chk.SetSel(sel.clone());
        }
    }

    let (leftChunks, rightChunks) = if !rightAsBuildSide {
        (buildChunks.clone(), probeChunks.clone())
    } else {
        (probeChunks.clone(), buildChunks.clone())
    };

    for chk in &buildChunks {
        builder.processOneChunk(chk.clone(), hashJoinCtx.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), &mut hashJoinCtx, 0).unwrap();
    }
    checkRowLocationAlignment(hashJoinCtx.hashTableContext.rowTables[0].clone());
    hashJoinCtx.hashTableContext.mergeRowTablesToHashTable(hashJoinCtx.partitionNumber, None);
    for i in 0..partitionNumber {
        hashJoinCtx.hashTableContext.build(Box::new(buildTask {
            partitionIdx: i,
            segStartIdx: 0,
            segEndIdx: hashJoinCtx.hashTableContext.hashTable.tables[i as usize].rowData.segments.len(),
        }));
    }

    // probe 阶段按 chunk 驱动 JoinProbe，满 chunk 立即收集，保留 sqlkiller 参数形状。
    let mut resultChunks = Vec::new();
    let mut joinResult = hashjoinWorkerResult {
        chk: chunk::New(resultTypes.clone(), hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize, hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize),
        ..Default::default()
    };
    for probeChunk in probeChunks {
        joinProbe.SetChunkForProbe(probeChunk).unwrap();
        while !joinProbe.IsCurrentChunkProbeDone() {
            let (_, ret) = joinProbe.Probe(joinResult, &sqlkiller::SQLKiller::default());
            joinResult = ret;
            require::NoError(joinResult.err.clone(), "unexpected error during join probe");
            if joinResult.chk.IsFull() {
                resultChunks.push(joinResult.chk);
                joinResult.chk = chunk::New(resultTypes.clone(), hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize, hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize);
            }
        }
    }

    if joinProbe.NeedScanRowTable() {
        let mut joinProbes = Vec::with_capacity(hashJoinCtx.Concurrency as usize);
        for i in 0..hashJoinCtx.Concurrency {
            joinProbes.push(NewJoinProbe(hashJoinCtx.clone(), i, joinType, probeKeyIndex.clone(), joinedTypes.clone(), probeKeyTypes.clone(), rightAsBuildSide));
        }
        for prober in &mut joinProbes {
            prober.InitForScanRowTable();
            while !prober.IsScanRowTableDone() {
                joinResult = prober.ScanRowTable(joinResult, &sqlkiller::SQLKiller::default());
                require::NoError(joinResult.err.clone(), "unexpected error during scan row table");
                if joinResult.chk.IsFull() {
                    resultChunks.push(joinResult.chk);
                    joinResult.chk = chunk::New(resultTypes.clone(), hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize, hashJoinCtx.SessCtx.GetSessionVars().MaxChunkSize);
                }
            }
        }
    }
    if joinResult.chk.NumRows() > 0 {
        resultChunks.push(joinResult.chk);
    }
    checkVirtualRows(resultChunks.clone());

    match joinType {
        base::InnerJoin => {
            let expectedChunks = genInnerJoinResult(hashJoinCtx.SessCtx, leftChunks, rightChunks, leftKeyIndex, rightKeyIndex, leftTypes, rightTypes, leftKeyTypes, rightKeyTypes, leftUsed, rightUsed, otherCondition, resultTypes.clone());
            checkChunksEqual(expectedChunks, resultChunks, resultTypes);
        }
        // 其它 join 类型的期望函数由相邻 Go/Rust 测试文件提供；这里保留调度分支，不在本任务扩展迁移范围。
        base::LeftOuterJoin => checkChunksEqual(genLeftOuterJoinResult(), resultChunks, resultTypes),
        base::RightOuterJoin => checkChunksEqual(genRightOuterJoinResult(), resultChunks, resultTypes),
        base::LeftOuterSemiJoin => checkChunksEqual(genLeftOuterSemiJoinResult(), resultChunks, resultTypes),
        base::SemiJoin => checkChunksEqual(genSemiJoinResult(), resultChunks, resultTypes),
        base::AntiSemiJoin => checkChunksEqual(genAntiSemiJoinResult(), resultChunks, resultTypes),
        base::AntiLeftOuterSemiJoin => checkChunksEqual(genLeftOuterAntiSemiJoinResult(), resultChunks, resultTypes),
        _ => require::NoError(errors::New("not supported join type")),
    }
}

// testCase 对应 Go 的同名 struct，描述一组 inner join probe 输入列、key、投影和 other condition。
pub struct testCase {
    pub leftKeyIndex: Vec<i32>,
    pub rightKeyIndex: Vec<i32>,
    pub leftKeyTypes: Vec<types::FieldType>,
    pub rightKeyTypes: Vec<types::FieldType>,
    pub leftTypes: Vec<types::FieldType>,
    pub rightTypes: Vec<types::FieldType>,
    pub leftUsed: Option<Vec<i32>>,
    pub rightUsed: Option<Vec<i32>>,
    pub otherCondition: expression::CNFExprs,
    pub leftUsedByOtherCondition: Vec<i32>,
    pub rightUsedByOtherCondition: Vec<i32>,
}

// TestInnerJoinProbeBasic 对应 Go 的基础 inner join probe case 集合。
#[test]
pub fn TestInnerJoinProbeBasic() {
    // todo test nullable type after builder support nullable type
    let mut tinyTp = types::NewFieldType(mysql::TypeTiny);
    tinyTp.AddFlag(mysql::NotNullFlag);
    let mut intTp = types::NewFieldType(mysql::TypeLonglong);
    intTp.AddFlag(mysql::NotNullFlag);
    let mut uintTp = types::NewFieldType(mysql::TypeLonglong);
    uintTp.AddFlag(mysql::NotNullFlag);
    uintTp.AddFlag(mysql::UnsignedFlag);
    let mut stringTp = types::NewFieldType(mysql::TypeVarString);
    stringTp.AddFlag(mysql::NotNullFlag);

    let lTypes = vec![intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone(), tinyTp.clone()];
    let mut rTypes = vec![intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone(), tinyTp.clone()];
    rTypes.extend(retTypes.clone());
    let mut rTypes1 = vec![uintTp.clone(), stringTp.clone(), intTp.clone(), stringTp.clone(), tinyTp.clone()];
    rTypes1.extend(rTypes1.clone());
    let rightAsBuildSide = vec![true, false];
    let partitionNumber = 4;

    let testCases = vec![
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![intTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: None, rightUsed: None, otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![intTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: Some(vec![0, 1, 2, 3]), rightUsed: Some(vec![]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![intTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: Some(vec![]), rightUsed: Some(vec![0, 1, 2, 3]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![intTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: Some(vec![]), rightUsed: Some(vec![]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![intTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: Some(vec![0, 2]), rightUsed: Some(vec![1, 3]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0], rightKeyIndex: vec![0], leftKeyTypes: vec![intTp.clone()], rightKeyTypes: vec![uintTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes1.clone(), leftUsed: Some(vec![0, 1, 2, 3]), rightUsed: Some(vec![0, 1, 2, 3]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
        testCase { leftKeyIndex: vec![0, 1], rightKeyIndex: vec![0, 1], leftKeyTypes: vec![intTp.clone(), stringTp.clone()], rightKeyTypes: vec![intTp.clone(), stringTp.clone()], leftTypes: lTypes.clone(), rightTypes: rTypes.clone(), leftUsed: Some(vec![0, 1, 2, 3]), rightUsed: Some(vec![0, 1, 2, 3]), otherCondition: expression::CNFExprs::default(), leftUsedByOtherCondition: vec![], rightUsedByOtherCondition: vec![] },
    ];

    for tc in testCases {
        // inner join 不支持左右 filter；Go 对每个 case 同时覆盖 right/left build side 和 nullable 类型。
        for rightAsBuild in &rightAsBuildSide {
            testJoinProbe(false, tc.leftKeyIndex.clone(), tc.rightKeyIndex.clone(), tc.leftKeyTypes.clone(), tc.rightKeyTypes.clone(), tc.leftTypes.clone(), tc.rightTypes.clone(), *rightAsBuild, tc.leftUsed.clone(), tc.rightUsed.clone(), tc.leftUsedByOtherCondition.clone(), tc.rightUsedByOtherCondition.clone(), expression::CNFExprs::default(), expression::CNFExprs::default(), tc.otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
            testJoinProbe(false, tc.leftKeyIndex.clone(), tc.rightKeyIndex.clone(), toNullableTypes(tc.leftKeyTypes.clone()), toNullableTypes(tc.rightKeyTypes.clone()), toNullableTypes(tc.leftTypes.clone()), toNullableTypes(tc.rightTypes.clone()), *rightAsBuild, tc.leftUsed.clone(), tc.rightUsed.clone(), tc.leftUsedByOtherCondition.clone(), tc.rightUsedByOtherCondition.clone(), expression::CNFExprs::default(), expression::CNFExprs::default(), tc.otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
        }
    }
}

// TestInnerJoinProbeAllJoinKeys 对应 Go 的全类型 join key 覆盖，包括单 key 和多种组合 key。
#[test]
pub fn TestInnerJoinProbeAllJoinKeys() {
    let lTypes = build_all_join_key_field_types_for_test();
    let rTypes = lTypes.clone();
    let nullableLTypes = toNullableTypes(lTypes.clone());
    let nullableRTypes = toNullableTypes(rTypes.clone());
    let lUsed = (0..18).collect::<Vec<_>>();
    let rUsed = lUsed.clone();
    let rightAsBuildSide = vec![true, false];
    let partitionNumber = 4;

    for i in 0..lTypes.len() {
        for rightAsBuild in &rightAsBuildSide {
            testJoinProbe(false, vec![i as i32], vec![i as i32], vec![lTypes[i].clone()], vec![rTypes[i].clone()], lTypes.clone(), rTypes.clone(), *rightAsBuild, Some(lUsed.clone()), Some(rUsed.clone()), vec![], vec![], expression::CNFExprs::default(), expression::CNFExprs::default(), expression::CNFExprs::default(), partitionNumber, base::InnerJoin, 100);
            testJoinProbe(false, vec![i as i32], vec![i as i32], toNullableTypes(vec![lTypes[i].clone()]), toNullableTypes(vec![rTypes[i].clone()]), nullableLTypes.clone(), nullableRTypes.clone(), *rightAsBuild, Some(lUsed.clone()), Some(rUsed.clone()), vec![], vec![], expression::CNFExprs::default(), expression::CNFExprs::default(), expression::CNFExprs::default(), partitionNumber, base::InnerJoin, 100);
        }
    }

    // Go 源依次覆盖 composed key：fixed/variable、inlined/not inlined。
    let composedCases = vec![(vec![1, 2], "fixed size, inlined"), (vec![1, 17], "variable size, inlined"), (vec![1, 13], "fixed size, not inlined"), (vec![1, 14], "variable size, not inlined")];
    for (keys, _name) in composedCases {
        for rightAsBuild in &rightAsBuildSide {
            let lKeyTypes = keys.iter().map(|i| lTypes[*i as usize].clone()).collect::<Vec<_>>();
            let rKeyTypes = keys.iter().map(|i| rTypes[*i as usize].clone()).collect::<Vec<_>>();
            testJoinProbe(false, keys.clone(), keys.clone(), lKeyTypes.clone(), rKeyTypes.clone(), lTypes.clone(), rTypes.clone(), *rightAsBuild, Some(lUsed.clone()), Some(rUsed.clone()), vec![], vec![], expression::CNFExprs::default(), expression::CNFExprs::default(), expression::CNFExprs::default(), partitionNumber, base::InnerJoin, 100);
            testJoinProbe(false, keys.clone(), keys.clone(), toNullableTypes(lKeyTypes), toNullableTypes(rKeyTypes), nullableLTypes.clone(), nullableRTypes.clone(), *rightAsBuild, Some(lUsed.clone()), Some(rUsed.clone()), vec![], vec![], expression::CNFExprs::default(), expression::CNFExprs::default(), expression::CNFExprs::default(), partitionNumber, base::InnerJoin, 100);
        }
    }
}

// build_all_join_key_field_types_for_test 折叠 Go 测试中连续的 FieldType 构造，顺序与原 lTypes 完全一致。
pub fn build_all_join_key_field_types_for_test() -> Vec<types::FieldType> {
    let mut tps = vec![
        types::NewFieldType(mysql::TypeTiny),
        types::NewFieldType(mysql::TypeLonglong),
        types::NewFieldType(mysql::TypeLonglong),
        types::NewFieldType(mysql::TypeYear),
        types::NewFieldType(mysql::TypeDuration),
        types::NewFieldType(mysql::TypeEnum),
        types::NewFieldType(mysql::TypeEnum),
        types::NewFieldType(mysql::TypeSet),
        types::NewFieldType(mysql::TypeBit),
        types::NewFieldType(mysql::TypeJSON),
        types::NewFieldType(mysql::TypeFloat),
        types::NewFieldType(mysql::TypeDouble),
        types::NewFieldType(mysql::TypeVarString),
        types::NewFieldType(mysql::TypeDatetime),
        types::NewFieldType(mysql::TypeNewDecimal),
        types::NewFieldType(mysql::TypeTimestamp),
        types::NewFieldType(mysql::TypeDate),
        types::NewFieldType(mysql::TypeBlob),
    ];
    for tp in &mut tps {
        tp.AddFlag(mysql::NotNullFlag);
    }
    tps[6].AddFlag(mysql::EnumSetAsIntFlag);
    tps
}

// TestInnerJoinProbeOtherCondition 对应 Go 的 other condition 场景，构造 a > b 表达式后验证投影和条件列。
#[test]
pub fn TestInnerJoinProbeOtherCondition() {
    let (intTp, nullableIntTp, uintTp, stringTp, lTypes, mut rTypes) = build_basic_probe_types();
    rTypes.extend(rTypes.clone());
    let tinyTp = types::NewFieldType(mysql::TypeTiny);
    let a = expression::Column { Index: 1, RetType: nullableIntTp.clone(), ..Default::default() };
    let b = expression::Column { Index: 8, RetType: nullableIntTp.clone(), ..Default::default() };
    let sf = expression::NewFunction(mock::NewContext(), ast::GT, tinyTp, a, b).unwrap();
    let otherCondition = vec![sf];
    let rightAsBuildSide = vec![true, false];
    let partitionNumber = 4;

    for rightAsBuild in rightAsBuildSide {
        testJoinProbe(false, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightAsBuild, Some(vec![1, 2, 4]), Some(vec![0]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
        testJoinProbe(false, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightAsBuild, Some(vec![]), Some(vec![]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
        testJoinProbe(false, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightAsBuild, Some(vec![1, 2, 4]), Some(vec![0]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
        testJoinProbe(false, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightAsBuild, None, None, vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), otherCondition.clone(), partitionNumber, base::InnerJoin, 200);
    }
}

// TestInnerJoinProbeWithSel 对应 Go 的 selection vector 场景，同时覆盖有/无 other condition。
#[test]
pub fn TestInnerJoinProbeWithSel() {
    let (intTp, nullableIntTp, uintTp, stringTp, lTypes, mut rTypes) = build_basic_probe_types();
    rTypes.extend(rTypes.clone());
    let mut nullableUIntTp = types::NewFieldType(mysql::TypeLonglong);
    nullableUIntTp.AddFlag(mysql::UnsignedFlag);

    let tinyTp = types::NewFieldType(mysql::TypeTiny);
    let a = expression::Column { Index: 1, RetType: nullableIntTp.clone(), ..Default::default() };
    let b = expression::Column { Index: 8, RetType: nullableUIntTp, ..Default::default() };
    let sf = expression::NewFunction(mock::NewContext(), ast::GT, tinyTp, a, b).unwrap();
    let otherConditions = vec![vec![sf], expression::CNFExprs::default()];
    let partitionNumber = 4;
    let rightAsBuildSide = vec![true, false];

    for rightAsBuild in rightAsBuildSide {
        for oc in &otherConditions {
            testJoinProbe(true, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightAsBuild, Some(vec![1, 2, 4]), Some(vec![0]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), oc.clone(), partitionNumber, base::InnerJoin, 500);
            testJoinProbe(true, vec![0], vec![0], vec![intTp.clone()], vec![intTp.clone()], lTypes.clone(), rTypes.clone(), rightAsBuild, Some(vec![]), Some(vec![]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), oc.clone(), partitionNumber, base::InnerJoin, 500);
            testJoinProbe(true, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightAsBuild, Some(vec![1, 2, 4]), Some(vec![0]), vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), oc.clone(), partitionNumber, base::InnerJoin, 500);
            testJoinProbe(true, vec![0], vec![0], vec![nullableIntTp.clone()], vec![nullableIntTp.clone()], toNullableTypes(lTypes.clone()), toNullableTypes(rTypes.clone()), rightAsBuild, None, None, vec![1], vec![3], expression::CNFExprs::default(), expression::CNFExprs::default(), oc.clone(), partitionNumber, base::InnerJoin, 500);
        }
    }
}

// build_basic_probe_types 提取 Go 中两处重复的 int/uint/string 类型构造；字段顺序仍按原测试保留。
pub fn build_basic_probe_types() -> (
    types::FieldType,
    types::FieldType,
    types::FieldType,
    types::FieldType,
    Vec<types::FieldType>,
    Vec<types::FieldType>,
) {
    let mut intTp = types::NewFieldType(mysql::TypeLonglong);
    intTp.AddFlag(mysql::NotNullFlag);
    let nullableIntTp = types::NewFieldType(mysql::TypeLonglong);
    let mut uintTp = types::NewFieldType(mysql::TypeLonglong);
    uintTp.AddFlag(mysql::NotNullFlag);
    uintTp.AddFlag(mysql::UnsignedFlag);
    let mut stringTp = types::NewFieldType(mysql::TypeVarString);
    stringTp.AddFlag(mysql::NotNullFlag);

    let lTypes = vec![intTp.clone(), intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone()];
    let rTypes = vec![intTp.clone(), intTp.clone(), stringTp.clone(), uintTp.clone(), stringTp.clone()];
    (intTp, nullableIntTp, uintTp, stringTp, lTypes, rTypes)
}
*/

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 将整型切片转为测试用探测/构建行。
fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// 构造以列 0 为 join key 的 Inner Join Probe，可注入 other condition 与 chunk 容量。
fn inner_probe(build: Vec<Row>, condition: Vec<Predicate>, max_chunk: usize) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        condition,
        None,
        false,
        max_chunk,
    )
    .unwrap();
    let context = HashJoinContext::new(build, vec![0], vec![0], joiner, true, true, max_chunk);
    new_join_probe(context, 0, JoinType::Inner, true, false).unwrap()
}

/// 重复 key 应产出多行匹配；hash miss（key=2）不产生结果。
#[test]
fn inner_join_probe_matches_duplicates_and_rejects_hash_misses() {
    let mut probe = inner_probe(
        vec![row(&[1, 10]), row(&[1, 11]), row(&[3, 30])],
        Vec::new(),
        32,
    );
    probe
        .set_chunk_for_probe(vec![row(&[1, 100]), row(&[2, 200])])
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        [row(&[1, 100, 1, 10]), row(&[1, 100, 1, 11]),]
    );
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
fn inner_join_probe_respects_chunk_capacity_with_duplicate_matches() {
    let mut probe = inner_probe(vec![row(&[1, 10]), row(&[1, 11])], Vec::new(), 1);
    probe.set_chunk_for_probe(vec![row(&[1, 100])]).unwrap();

    assert_eq!(probe.probe().rows, [row(&[1, 100, 1, 10])]);
    assert!(!probe.is_current_chunk_probe_done());
    assert_eq!(probe.probe().rows, [row(&[1, 100, 1, 11])]);
    assert!(probe.is_current_chunk_probe_done());
}

/// other condition 过滤后仅保留满足谓词的行；max_chunk=1 时分多次 probe 输出。
#[test]
fn inner_join_probe_applies_other_condition_and_chunk_capacity() {
    let condition: Predicate = Arc::new(|joined| match (&joined[3], &joined[1]) {
        (Value::Int(build), Value::Int(probe)) => Ok(Some(build < probe)),
        _ => Ok(None),
    });
    let mut probe = inner_probe(
        vec![row(&[1, 10]), row(&[1, 20]), row(&[2, 5])],
        vec![condition],
        1,
    );
    probe
        .set_chunk_for_probe(vec![row(&[1, 15]), row(&[2, 9])])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(&[1, 15, 1, 10])]);
    assert_eq!(probe.probe().rows, [row(&[2, 9, 2, 5])]);
}

/// restore 路径探测一行后，spill 应保留剩余探测行。
#[test]
fn inner_join_restored_and_spill_paths_preserve_remaining_rows() {
    let mut probe = inner_probe(vec![row(&[1])], Vec::new(), 1);
    probe
        .set_restored_chunk_for_probe(vec![row(&[1]), row(&[2]), row(&[3])])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(&[1, 1])]);
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        [vec![row(&[2]), row(&[3])]]
    );
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_scan_row_table_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_init_for_scan_row_table_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).init_for_scan_row_table();
}

#[test]
#[should_panic(expected = "should not reach here")]
fn inner_join_is_scan_row_table_done_panics_like_go() {
    inner_probe(Vec::new(), Vec::new(), 1).is_scan_row_table_done();
}

/// 探测侧 key 列下标越界时应拒绝 set_chunk_for_probe。
#[test]
fn inner_join_probe_rejects_out_of_range_probe_key() {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(vec![row(&[1])], vec![0], vec![1], joiner, true, true, 8);
    let mut probe = new_join_probe(context, 0, JoinType::Inner, true, false).unwrap();
    assert!(probe.set_chunk_for_probe(vec![row(&[1])]).is_err());
}
