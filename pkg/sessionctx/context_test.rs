// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `BasicCtxType` 展示名单测。
//
// 校验会话上下文键到字符串的映射与未知值回落为 `unknown`。

use astersql_sessionctx::{
    BasicCtxType, Initing, LastExecuteDDL, PlanContextCommon, QueryString, ValueStoreContext,
};

#[test]
/// 验证 QueryString/Initing/LastExecuteDDL 与未知键的 String() 结果。
fn test_basic_ctx_type_to_string() {
    let tests = [
        (QueryString, "query_string"),
        (Initing, "initing"),
        (LastExecuteDDL, "last_execute_ddl"),
        (BasicCtxType::new(9), "unknown"),
    ];

    for (key, expected) in tests {
        assert_eq!(key.String(), expected);
    }
}

#[test]
fn test_plan_context_common_embeds_value_store_context() {
    fn assert_value_store_supertrait<C: PlanContextCommon>() {
        fn assert_value_store<C: ValueStoreContext>() {}
        assert_value_store::<C>();
    }
}
