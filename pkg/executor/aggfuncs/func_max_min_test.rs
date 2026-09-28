// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// MAX/MIN 聚合与滑动窗口 deque 测试。
//
// 块注释内保留 Go `TestMergePartialResult4MaxMin`、`TestMaxMin`、`TestMemMaxMin`、
// `TestMaxSlidingWindow`、`TestDequeReset`/`TestDequePushPop` 的用例语义；
// 可执行部分验证整组 MAX/MIN 排序，以及单调队列在入队/过期剔除后的队头极值。

/*
// MAX/MIN 聚合、内存增量、窗口 max SQL 用例，以及 deque 辅助结构测试的 Go 语义。
// maxMinUpdateMemDeltaGens 对应 Go 的同名测试辅助：按输入 chunk 推算 MAX/MIN 替换可变长值时的内存增量。
pub fn maxMinUpdateMemDeltaGens(
    srcChk: &chunk::Chunk,
    dataType: &types::FieldType,
    isMax: bool,
) -> Result<Vec<i64>, Error> {
    let mut memDeltas = vec![0_i64; srcChk.NumRows()];
    let mut preStringVal = String::new();
    let mut preJSONVal = String::new();
    let mut preEnumVal = types::Enum::default();
    let mut preSetVal = types::Set::default();

    for i in 0..srcChk.NumRows() {
        let row = srcChk.GetRow(i);
        if row.IsNull(0) {
            // Go 测试中 NULL 行不参与 MAX/MIN 值替换，也不会产生内存增量。
            continue;
        }
        match dataType.GetType() {
            mysql::TypeString => {
                let curVal = row.GetString(0);
                if i == 0 {
                    memDeltas[i] = curVal.len() as i64;
                    preStringVal = curVal;
                } else if (isMax && curVal > preStringVal) || (!isMax && curVal < preStringVal) {
                    // 只有新值成为当前 MAX/MIN 时，增量才等于新旧字符串长度差。
                    memDeltas[i] = curVal.len() as i64 - preStringVal.len() as i64;
                    preStringVal = curVal;
                }
            }
            mysql::TypeJSON => {
                let curVal = row.GetJSON(0);
                let curStringVal = String::from_utf8_lossy(&curVal.Value).to_string();
                if i == 0 {
                    memDeltas[i] = curStringVal.len() as i64;
                    preJSONVal = curStringVal;
                } else if (isMax && curStringVal > preJSONVal) || (!isMax && curStringVal < preJSONVal) {
                    memDeltas[i] = curStringVal.len() as i64 - preJSONVal.len() as i64;
                    preJSONVal = curStringVal;
                }
            }
            mysql::TypeEnum => {
                let curVal = row.GetEnum(0);
                if i == 0 {
                    memDeltas[i] = curVal.Name.len() as i64;
                    preEnumVal = curVal;
                } else if (isMax && curVal.Name > preEnumVal.Name) || (!isMax && curVal.Name < preEnumVal.Name) {
                    memDeltas[i] = curVal.Name.len() as i64 - preEnumVal.Name.len() as i64;
                    preEnumVal = curVal;
                }
            }
            mysql::TypeSet => {
                let curVal = row.GetSet(0);
                if i == 0 {
                    memDeltas[i] = curVal.Name.len() as i64;
                    preSetVal = curVal;
                } else if (isMax && curVal.Name > preSetVal.Name) || (!isMax && curVal.Name < preSetVal.Name) {
                    memDeltas[i] = curVal.Name.len() as i64 - preSetVal.Name.len() as i64;
                    preSetVal = curVal;
                }
            }
            _ => {}
        }
    }
    Ok(memDeltas)
}

// maxUpdateMemDeltaGens 对应 Go 包装函数：固定以 MAX 比较规则计算内存增量。
pub fn maxUpdateMemDeltaGens(param: updateMemDeltaGensParams) -> Result<Vec<i64>, Error> {
    maxMinUpdateMemDeltaGens(param.srcChk, param.keyType, true)
}

// minUpdateMemDeltaGens 对应 Go 包装函数：固定以 MIN 比较规则计算内存增量。
pub fn minUpdateMemDeltaGens(param: updateMemDeltaGensParams) -> Result<Vec<i64>, Error> {
    maxMinUpdateMemDeltaGens(param.srcChk, param.keyType, false)
}

// Max/min merge 测试用例；args 只保存参数形状，真实 builder 仍是 Go 侧测试辅助。
struct AggCaseDraft {
    builder: &'static str,
    func_name: &'static str,
    mysql_type: &'static str,
    args: Vec<&'static str>,
}

// test_merge_partial_result_4_max_min 对应 Go TestMergePartialResult4MaxMin。
#[test]
pub fn test_merge_partial_result_4_max_min() {
    // Go 先构造 enum/set 值；这里保留名字和值语义，供人工核对预期结果。
    let elems = ["e", "d", "c", "b", "a"];
    let enum_a = "types.ParseEnum(elems, \"a\", mysql.DefaultCollationName)";
    let enum_c = "types.ParseEnum(elems, \"c\", mysql.DefaultCollationName)";
    let enum_e = "types.ParseEnum(elems, \"e\", mysql.DefaultCollationName)";
    let set_c = "types.ParseSet(elems, \"c\", mysql.DefaultCollationName)"; // setC.Value == 4
    let set_ed = "types.ParseSet(elems, \"e,d\", mysql.DefaultCollationName)"; // setED.Value == 3
    let unsigned_type = "types.NewFieldType(mysql.TypeLonglong) + mysql.UnsignedFlag";

    let tests = vec![
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeLonglong", args: vec!["0", "5", "4", "4", "4"] },
        AggCaseDraft { builder: "buildAggTesterWithFieldType", func_name: "ast.AggFuncMax", mysql_type: unsigned_type, args: vec!["nil", "5", "4", "4", "4"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeFloat", args: vec!["0", "5", "4.0", "4.0", "4.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDouble", args: vec!["0", "5", "4.0", "4.0", "4.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeNewDecimal", args: vec!["0", "5", "types.NewDecFromInt(4)", "types.NewDecFromInt(4)", "types.NewDecFromInt(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeString", args: vec!["0", "5", "\"4\"", "\"4\"", "\"4\""] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDate", args: vec!["0", "5", "types.TimeFromDays(369)", "types.TimeFromDays(369)", "types.TimeFromDays(369)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDuration", args: vec!["0", "5", "time.Duration(4)", "time.Duration(4)", "time.Duration(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeJSON", args: vec!["0", "5", "JSON(4)", "JSON(4)", "JSON(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeEnum", args: vec!["0", "5", enum_e, enum_c, enum_e] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeSet", args: vec!["0", "5", set_ed, set_ed, set_ed] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeLonglong", args: vec!["0", "5", "0", "2", "0"] },
        AggCaseDraft { builder: "buildAggTesterWithFieldType", func_name: "ast.AggFuncMin", mysql_type: unsigned_type, args: vec!["nil", "5", "0", "2", "0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeFloat", args: vec!["0", "5", "0.0", "2.0", "0.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDouble", args: vec!["0", "5", "0.0", "2.0", "0.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeNewDecimal", args: vec!["0", "5", "types.NewDecFromInt(0)", "types.NewDecFromInt(2)", "types.NewDecFromInt(0)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeString", args: vec!["0", "5", "\"0\"", "\"2\"", "\"0\""] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDate", args: vec!["0", "5", "types.TimeFromDays(365)", "types.TimeFromDays(367)", "types.TimeFromDays(365)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDuration", args: vec!["0", "5", "time.Duration(0)", "time.Duration(2)", "time.Duration(0)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeJSON", args: vec!["0", "5", "JSON(0)", "JSON(2)", "JSON(0)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeEnum", args: vec!["0", "5", enum_a, enum_a, enum_a] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeSet", args: vec!["0", "5", set_c, set_c, set_c] },
    ];

    for test in tests {
        // Go 这里执行 t.Run(test.funcName, testMergePartialResult)；仅保留子测试入口和参数对应关系。
        testMergePartialResult(test);
    }
}

// test_max_min 对应 Go TestMaxMin，覆盖 MAX/MIN 原始聚合结果。
#[test]
pub fn test_max_min() {
    let unsigned_type = "types.NewFieldType(mysql.TypeLonglong) + mysql.UnsignedFlag";
    let tests = vec![
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeLonglong", args: vec!["0", "5", "nil", "4"] },
        AggCaseDraft { builder: "buildAggTesterWithFieldType", func_name: "ast.AggFuncMax", mysql_type: unsigned_type, args: vec!["nil", "5", "nil", "4"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeFloat", args: vec!["0", "5", "nil", "4.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDouble", args: vec!["0", "5", "nil", "4.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeNewDecimal", args: vec!["0", "5", "nil", "types.NewDecFromInt(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeString", args: vec!["0", "5", "nil", "\"4\"", "\"4\""] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDate", args: vec!["0", "5", "nil", "types.TimeFromDays(369)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeDuration", args: vec!["0", "5", "nil", "time.Duration(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMax", mysql_type: "mysql.TypeJSON", args: vec!["0", "5", "nil", "JSON(4)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeLonglong", args: vec!["0", "5", "nil", "0"] },
        AggCaseDraft { builder: "buildAggTesterWithFieldType", func_name: "ast.AggFuncMin", mysql_type: unsigned_type, args: vec!["nil", "5", "nil", "0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeFloat", args: vec!["0", "5", "nil", "0.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDouble", args: vec!["0", "5", "nil", "0.0"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeNewDecimal", args: vec!["0", "5", "nil", "types.NewDecFromInt(0)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeString", args: vec!["0", "5", "nil", "\"0\""] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDate", args: vec!["0", "5", "nil", "types.TimeFromDays(365)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeDuration", args: vec!["0", "5", "nil", "time.Duration(0)"] },
        AggCaseDraft { builder: "buildAggTester", func_name: "ast.AggFuncMin", mysql_type: "mysql.TypeJSON", args: vec!["0", "5", "nil", "JSON(0)"] },
    ];
    for test in tests {
        // Go 中每个用例都委托 testAggFunc；这里保留委托点，未执行真实聚合。
        testAggFunc(test);
    }
}

// test_mem_max_min 对应 Go TestMemMaxMin，重点是固定大小结果与字符串/JSON/Enum/Set 增量函数的配对。
#[test]
pub fn test_mem_max_min() {
    let tests = vec![
        ("ast.AggFuncMax", "mysql.TypeLonglong", "DefPartialResult4MaxMinIntSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeLonglong unsigned", "DefPartialResult4MaxMinUintSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeNewDecimal", "DefPartialResult4MaxMinDecimalSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeFloat", "DefPartialResult4MaxMinFloat32Size", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeDouble", "DefPartialResult4MaxMinFloat64Size", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeDate", "DefPartialResult4MaxMinTimeSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeDuration", "DefPartialResult4MaxMinDurationSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeString", "DefPartialResult4MaxMinStringSize", "maxUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeJSON", "DefPartialResult4MaxMinJSONSize", "maxUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeEnum", "DefPartialResult4MaxMinEnumSize", "maxUpdateMemDeltaGens"),
        ("ast.AggFuncMax", "mysql.TypeSet", "DefPartialResult4MaxMinSetSize", "maxUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeLonglong", "DefPartialResult4MaxMinIntSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeLonglong unsigned", "DefPartialResult4MaxMinUintSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeNewDecimal", "DefPartialResult4MaxMinDecimalSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeFloat", "DefPartialResult4MaxMinFloat32Size", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeDouble", "DefPartialResult4MaxMinFloat64Size", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeDate", "DefPartialResult4MaxMinTimeSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeDuration", "DefPartialResult4MaxMinDurationSize", "defaultUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeString", "DefPartialResult4MaxMinStringSize", "minUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeJSON", "DefPartialResult4MaxMinJSONSize", "minUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeEnum", "DefPartialResult4MaxMinEnumSize", "minUpdateMemDeltaGens"),
        ("ast.AggFuncMin", "mysql.TypeSet", "DefPartialResult4MaxMinSetSize", "minUpdateMemDeltaGens"),
    ];
    for test in tests {
        // Go 逐项调用 buildAggMemTester 后进入 testAggMemFunc；这里保留常量名和增量函数绑定。
        testAggMemFunc(test);
    }
}

// maxSlidingWindowTestCase 对应 Go 的测试表结构，保存 SQL 类型、插入值和窗口结果。
pub struct maxSlidingWindowTestCase {
    pub rowType: &'static str,
    pub insertValue: &'static str,
    pub expect: Vec<&'static str>,
    pub orderByExpect: Vec<&'static str>,
    pub orderBy: bool,
    pub frameType: &'static str,
}

// testMaxSlidingWindow 对应 Go 辅助函数：建表、写入数据，并按 frame 类型选择不同窗口 SQL。
pub fn testMaxSlidingWindow(tk: &mut testkit::TestKit, tc: maxSlidingWindowTestCase) {
    tk.MustExec(format!("CREATE TABLE t (a {});", tc.rowType));
    tk.MustExec(format!("insert into t values {};", tc.insertValue));
    let orderBy = if tc.orderBy { "ORDER BY a" } else { "" };

    let result = match tc.frameType {
        "ast.Rows" => tk.MustQuery(format!(
            "SELECT max(a) OVER ({} ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t;",
            orderBy
        )),
        "ast.Ranges" => tk.MustQuery(format!(
            "SELECT max(a) OVER ({} RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t;",
            orderBy
        )),
        _ => {
            let result = tk.MustQuery(format!("SELECT max(a) OVER ({}) FROM t;", orderBy));
            if tc.orderBy {
                // Go 默认 frame 且带 ORDER BY 时，期望值来自 orderByExpect。
                result.Check(testkit::Rows(tc.orderByExpect));
                return;
            }
            result
        }
    };
    result.Check(testkit::Rows(tc.expect));
}

// test_max_sliding_window 对应 Go TestMaxSlidingWindow，保留所有 SQL 类型和 frame/orderBy 组合。
#[test]
pub fn test_max_sliding_window() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    let testCases = vec![
        maxSlidingWindowTestCase { rowType: "bigint", insertValue: "(1), (3), (2)", expect: vec!["3", "3", "3"], orderByExpect: vec!["1", "2", "3"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "int unsigned", insertValue: "(1), (3), (2)", expect: vec!["3", "3", "3"], orderByExpect: vec!["1", "2", "3"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "float", insertValue: "(1.1), (3.3), (2.2)", expect: vec!["3.3", "3.3", "3.3"], orderByExpect: vec!["1.1", "2.2", "3.3"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "double", insertValue: "(1.1), (3.3), (2.2)", expect: vec!["3.3", "3.3", "3.3"], orderByExpect: vec!["1.1", "2.2", "3.3"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "decimal(5, 2)", insertValue: "(1.1), (3.3), (2.2)", expect: vec!["3.30", "3.30", "3.30"], orderByExpect: vec!["1.10", "2.20", "3.30"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "text", insertValue: "('1.1'), ('3.3'), ('2.2')", expect: vec!["3.3", "3.3", "3.3"], orderByExpect: vec!["1.1", "2.2", "3.3"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "time", insertValue: "('00:00:00'), ('03:00:00'), ('02:00:00')", expect: vec!["03:00:00", "03:00:00", "03:00:00"], orderByExpect: vec!["00:00:00", "02:00:00", "03:00:00"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "date", insertValue: "('2020-09-08'), ('2022-09-10'), ('2020-09-10')", expect: vec!["2022-09-10", "2022-09-10", "2022-09-10"], orderByExpect: vec!["2020-09-08", "2020-09-10", "2022-09-10"], orderBy: false, frameType: "" },
        maxSlidingWindowTestCase { rowType: "datetime", insertValue: "('2020-09-08 02:00:00'), ('2022-09-10 00:00:00'), ('2020-09-10 00:00:00')", expect: vec!["2022-09-10 00:00:00", "2022-09-10 00:00:00", "2022-09-10 00:00:00"], orderByExpect: vec!["2020-09-08 02:00:00", "2020-09-10 00:00:00", "2022-09-10 00:00:00"], orderBy: false, frameType: "" },
    ];

    let orderBy = [false, true];
    let frameType = ["ast.Rows", "ast.Ranges", "-1"];
    for o in orderBy {
        for f in frameType {
            for mut tc in testCases.clone() {
                // Go 子测试名为 rowType_orderBy_frame；每次执行前 drop table，避免上一轮数据污染。
                tc.frameType = f;
                tc.orderBy = o;
                tk.MustExec("drop table if exists t;");
                testMaxSlidingWindow(&mut tk, tc);
            }
        }
    }
}

// test_deque_reset 对应 Go TestDequeReset，验证 Reset 后清空 Items 且保留 IsMax。
#[test]
pub fn test_deque_reset() {
    let mut deque = aggfuncs::NewDeque(true, |i: i64, j: i64| i.cmp(&j));
    deque.PushBack(0, 12);
    deque.Reset();
    require::Len(&deque.Items, 0);
    require::True(deque.IsMax);
}

// test_deque_push_pop 对应 Go TestDequePushPop，先连续 PushBack 再从 Back 反向弹出。
#[test]
pub fn test_deque_push_pop() {
    let mut deque = aggfuncs::NewDeque(true, |i: i64, j: i64| i.cmp(&j));
    let times = 15;
    for i in 0..times {
        if i != 0 {
            let (front, isEnd) = deque.Front();
            require::False(isEnd);
            require::Zero(front.Item);
            require::Zero(front.Idx);
        }
        deque.PushBack(i as u64, i);
        let (back, isEnd) = deque.Back();
        require::False(isEnd);
        require::Equal(back.Item, i);
        require::Equal(back.Idx, i as u64);
    }

    for i in 0..times {
        let (pair, isEnd) = deque.Back();
        require::False(isEnd);
        require::Equal(pair.Item, times - i - 1);
        require::Equal(pair.Idx, (times - i - 1) as u64);
        let (front, isEnd) = deque.Front();
        require::False(isEnd);
        require::Zero(front.Item);
        require::Zero(front.Idx);
        // Go require.NoError 会检查 PopBack 的错误；保留错误检查点。
        require::NoError(deque.PopBack());
    }
}
*/

use crate::func_max_min::{
    BinaryJson, DurationValue, MaxMin, MinMaxDeque, NamedValue, TimeValue, update_float32,
    update_float64,
};

/// 验证 MAX/MIN 忽略 NULL 取极值，以及 deque 入队支配与 dequeue 过期后的队头。
#[test]
fn max_min_and_sliding_deque_keep_production_ordering() {
    // MAX：跳过 None，结果为 5。
    let mut max = MaxMin::new(true);
    max.update([Some(2), None, Some(5), Some(1)]);
    assert_eq!(max.value(), Some(&5));
    // MIN：结果为 1。
    let mut min = MaxMin::new(false);
    min.update([Some(2), Some(5), Some(1)]);
    assert_eq!(min.value(), Some(&1));
    // 单调 MAX 队列：5 支配 2；再入 3 后队为 [5,3]；dequeue(2) 去掉下标 <2 的 5，队头剩 3。
    let mut deque = MinMaxDeque::new(true);
    deque.enqueue(0, 2, Ord::cmp);
    deque.enqueue(1, 5, Ord::cmp);
    deque.enqueue(2, 3, Ord::cmp);
    assert_eq!(deque.front().map(|pair| pair.item), Some(5));
    deque.dequeue(2);
    assert_eq!(deque.front().map(|pair| pair.item), Some(3));
}

/// Go `MinMaxDeque.Enqueue` removes equal tail values as well as dominated
/// values, retaining the newest row index for a peer value.
#[test]
fn sliding_deque_replaces_equal_tail_with_newest_index() {
    let mut max = MinMaxDeque::new(true);
    max.enqueue(4, 7, Ord::cmp);
    max.enqueue(9, 7, Ord::cmp);
    assert_eq!(
        max.front().map(|pair| (pair.index, pair.item)),
        Some((9, 7))
    );
    assert_eq!(max.back().map(|pair| (pair.index, pair.item)), Some((9, 7)));

    let mut min = MinMaxDeque::new(false);
    min.enqueue(4, 7, Ord::cmp);
    min.enqueue(9, 7, Ord::cmp);
    assert_eq!(
        min.front().map(|pair| (pair.index, pair.item)),
        Some((9, 7))
    );
}

/// Go compares TIME by its packed instant and DURATION by nanoseconds; display
/// metadata must not change MAX/MIN selection.
#[test]
fn temporal_max_min_ignores_type_and_fsp_metadata() {
    let earlier_with_larger_metadata = TimeValue {
        packed: 10,
        kind: u8::MAX,
        fsp: 6,
    };
    let later_with_smaller_metadata = TimeValue {
        packed: 11,
        kind: 0,
        fsp: 0,
    };
    let mut time_max = MaxMin::new(true);
    time_max.update([
        Some(earlier_with_larger_metadata),
        Some(later_with_smaller_metadata.clone()),
    ]);
    assert_eq!(time_max.value(), Some(&later_with_smaller_metadata));

    let first_time = TimeValue {
        packed: 20,
        kind: 1,
        fsp: 6,
    };
    let same_instant = TimeValue {
        packed: 20,
        kind: 2,
        fsp: 0,
    };
    let mut time_min = MaxMin::new(false);
    time_min.update([Some(first_time.clone()), Some(same_instant)]);
    assert_eq!(
        time_min.value().map(|value| (value.kind, value.fsp)),
        Some((1, 6))
    );

    let same_duration_different_fsp = [
        DurationValue { nanos: 42, fsp: 6 },
        DurationValue { nanos: 42, fsp: 0 },
    ];
    let mut duration_min = MaxMin::new(false);
    duration_min.update(same_duration_different_fsp.clone().map(Some));
    assert_eq!(duration_min.value().map(|value| value.fsp), Some(6));
}

/// Go ENUM/SET MAX/MIN compares the collated name only; the numeric payload is
/// retained from the first peer value when names compare equal.
#[test]
fn named_value_order_ignores_numeric_payload() {
    let first = NamedValue {
        name: "same".to_owned(),
        value: 9,
    };
    let peer = NamedValue {
        name: "same".to_owned(),
        value: 1,
    };
    let mut min = MaxMin::new(false);
    min.update([Some(first.clone()), Some(peer)]);
    assert_eq!(min.value().map(|value| value.value), Some(first.value));
}

/// Go compares signed and unsigned JSON numbers by numeric value, not their
/// distinct binary type codes.
#[test]
fn binary_json_order_uses_json_semantics() {
    let signed = BinaryJson {
        type_code: 0x09,
        value: 7_i64.to_le_bytes().to_vec(),
    };
    let unsigned = BinaryJson {
        type_code: 0x0a,
        value: 7_u64.to_le_bytes().to_vec(),
    };
    let mut max = MaxMin::new(true);
    max.update([Some(signed.clone()), Some(unsigned)]);
    assert_eq!(max.value().map(|value| value.type_code), Some(0x09));
}

/// Go's generic `cmp.Compare` orders NaN before ordinary floating-point
/// values, rather than treating incomparable values as equal.
#[test]
fn float_max_min_matches_go_nan_ordering() {
    let mut max32 = MaxMin::new(true);
    update_float32(&mut max32, [Some(f32::NAN), Some(1.0)]);
    assert_eq!(max32.value(), Some(&1.0));

    let mut min64 = MaxMin::new(false);
    update_float64(&mut min64, [Some(1.0), Some(f64::NAN)]);
    assert!(min64.value().is_some_and(|value| value.is_nan()));
}
