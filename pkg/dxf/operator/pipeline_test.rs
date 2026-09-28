// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 管道集成测试：多算子异步流水线处理词频，并可注入 Context 错误。
//
// 拓扑：Source → lower → trimmer → counter → collector，对应 Go pipeline 测例。

use crate::compose::Compose;
use crate::operator::Operator;
use crate::pipeline::NewAsyncPipeline;
use crate::workerpool::{self, Error, TaskMayPanic};
use crate::wrapper::{
    NewSimpleDataSource, SimpleOperator, SimpleSink, newSimpleOperator, newSimpleSink,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 构建五段算子管道：小写化、去非字母数字、计数、收集最高频词。
///
/// `mock_error` 为真时 counter 注入错误，Close 应失败；否则断言最常见词为 "hit"。
#[test]
fn test_pipeline_async_multi_operators_without_error() {
    let words = "Bob hiT a ball, the hIt BALL flew far after it was hit.";
    let splitted: Vec<&str> = words.split(' ').collect();

    let tasks: Vec<StringTask> = splitted
        .iter()
        .map(|word| StringTask::from(*word))
        .collect();

    for mock_error in [false, true] {
        let wctx = workerpool::NewContext(workerpool::Context::background());

        let most_common_word = Arc::new(Mutex::new(StringTask::default()));
        let mut source = NewSimpleDataSource(wctx.clone(), tasks.clone());
        let mut lower = make_lower(&wctx);
        let mut trimmer = make_trimmer(&wctx);
        let mut counter = make_counter(&wctx, mock_error);
        let mut collector = make_collector(&wctx, Arc::clone(&most_common_word));

        Compose(&mut source, &mut lower);
        Compose(&mut lower, &mut trimmer);
        Compose(&mut trimmer, &mut counter);
        Compose(&mut counter, &mut collector);

        let string_task = std::any::type_name::<StringTask>();
        let str_cnt = std::any::type_name::<StrCnt>();
        let mut pipeline = NewAsyncPipeline(vec![
            Box::new(source) as Box<dyn Operator>,
            Box::new(lower),
            Box::new(trimmer),
            Box::new(counter),
            Box::new(collector),
        ]);
        // 校验 String() 拓扑描述与算子类型名一致。
        assert_eq!(
            pipeline.String(),
            format!(
                "AsyncPipeline[SimpleDataSource[{string_task}] -> simpleOperator(AsyncOp[{string_task}, {string_task}]) -> simpleOperator(AsyncOp[{string_task}, {string_task}]) -> simpleOperator(AsyncOp[{string_task}, {str_cnt}]) -> simpleSink]"
            )
        );

        pipeline.Execute().expect("pipeline Execute should succeed");
        let close_result = pipeline.Close();
        if mock_error {
            assert!(close_result.is_err());
        } else {
            close_result.expect("Close should succeed without injected error");
            assert_eq!(
                *most_common_word.lock().expect("collector result mutex"),
                StringTask::from("hit")
            );
        }
    }
}

/// 词与当前出现次数，供 counter → collector 传递。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StrCnt {
    str_: StringTask,
    cnt: i32,
}

/// 可在 worker 池传递的字符串任务。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct StringTask(String);

impl From<&str> for StringTask {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl TaskMayPanic for StringTask {
    fn RecoverArgs(&self) -> (String, String, Option<Error>) {
        (String::new(), String::new(), None)
    }
}

/// 小写化变换算子（并发度 3）。
fn make_lower(ctx: &workerpool::Context) -> SimpleOperator<StringTask, StringTask> {
    newSimpleOperator(
        ctx.clone(),
        |task: StringTask| StringTask(task.0.to_lowercase()),
        3,
    )
}

/// 去掉非字母数字字符的修剪算子。
fn make_trimmer(ctx: &workerpool::Context) -> SimpleOperator<StringTask, StringTask> {
    let non_alpha_regex = regex::Regex::new(r"[^a-zA-Z0-9]+").expect("compile trim regex");
    newSimpleOperator(
        ctx.clone(),
        move |s: StringTask| StringTask(non_alpha_regex.replace_all(&s.0, "").to_string()),
        3,
    )
}

/// 词频计数算子；`mock_error` 时通过 Context.OnError 注入测试错误。
fn make_counter(ctx: &workerpool::Context, mock_error: bool) -> SimpleOperator<StringTask, StrCnt> {
    let str_cnt_map = Arc::new(Mutex::new(HashMap::<StringTask, i32>::new()));
    let ctx_for_closure = ctx.clone();
    newSimpleOperator(
        ctx.clone(),
        move |s: StringTask| {
            let mut guard = str_cnt_map.lock().expect("counter mutex");
            let old = *guard.get(&s).unwrap_or(&0);
            guard.insert(s.clone(), old + 1);
            drop(guard);

            if mock_error {
                ctx_for_closure.OnError(Error::new("mock error for testing"));
            }

            StrCnt {
                str_: s,
                cnt: old + 1,
            }
        },
        3,
    )
}

/// 收集当前最高频词到共享 Mutex。
fn make_collector(ctx: &workerpool::Context, value: Arc<Mutex<StringTask>>) -> SimpleSink<StrCnt> {
    let max_cnt = Arc::new(Mutex::new(0));
    newSimpleSink(ctx.clone(), move |sc: StrCnt| {
        let mut max_guard = max_cnt.lock().expect("collector max mutex");
        if sc.cnt > *max_guard {
            *max_guard = sc.cnt;
            *value.lock().expect("collector result mutex") = sc.str_;
        }
    })
}
