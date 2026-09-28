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

// 索引推荐（Recommend Index）执行器。
//
// 对应 Go 的 `RECOMMEND INDEX`：通过 `IndexAdvisor` 抽象顾问实现，
// `RecommendIndexExec` 按 action 分发 set/show/run，将推荐结果写成结果集行
// （含库表、索引列、规模、原因、受影响查询 JSON，以及可执行的 CREATE INDEX）。

#![allow(non_snake_case)]

use std::collections::BTreeMap;

use astersql_util_chunk::Chunk;

/// 单条索引推荐结果（写入结果集的一行语义）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecommendIndexResult {
    pub database: String,
    pub table: String,
    pub index_name: String,
    pub index_columns: Vec<String>,
    pub index_size: String,
    pub reason: String,
    pub top_impacted_queries_json: String,
}

/// 索引顾问能力边界：选项读写、基于 SQL 列表给出推荐、错误工厂。
pub trait IndexAdvisor {
    type Context;
    type Option: Clone;
    type Error;

    fn set_options(
        &mut self,
        context: &mut Self::Context,
        options: &[Self::Option],
    ) -> Result<(), Self::Error>;
    fn all_options(&self) -> &[String];
    fn get_options(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>), Self::Error>;
    fn advise_indexes<C>(
        &mut self,
        request_context: C,
        context: &mut Self::Context,
        sqls: Vec<String>,
        options: &[Self::Option],
    ) -> (Vec<RecommendIndexResult>, Result<(), Self::Error>);
    fn unsupported_action(&self, action: &str) -> Self::Error;
    fn empty_sqls(&self) -> Self::Error;
}

/// Recommend Index 执行器状态：顾问、会话上下文、动作与 SQL、是否已产出。
pub struct RecommendIndexExec<A: IndexAdvisor> {
    pub advisor: A,
    pub context: A::Context,
    pub action: String,
    pub sql: String,
    pub advise_id: i64,
    pub options: Vec<A::Option>,
    pub done: bool,
}

impl<A: IndexAdvisor> RecommendIndexExec<A> {
    /// 按 action 执行一次：set 写选项、show 列出选项、run 产出推荐行。
    pub fn Next<C>(&mut self, ctx: C, req: &mut Chunk) -> Result<(), A::Error> {
        req.Reset();
        if self.done {
            return Ok(());
        }
        self.done = true;

        // 分发动作；仅 "run" 落入下方推荐主路径。
        match self.action.as_str() {
            "set" => {
                return self.advisor.set_options(&mut self.context, &self.options);
            }
            "show" => return self.showOptions(req),
            "run" => {}
            action => return Err(self.advisor.unsupported_action(action)),
        }

        // 按分号拆分多条 SQL；全空则报 empty_sqls。
        let mut sqls = Vec::new();
        if !self.sql.is_empty() {
            sqls.extend(
                self.sql
                    .split(';')
                    .map(str::trim)
                    .filter(|sql| !sql.is_empty())
                    .map(str::to_owned),
            );
            if sqls.is_empty() {
                return Err(self.advisor.empty_sqls());
            }
        }

        // Go deliberately writes all returned recommendations before returning
        // the advisor error, so partial useful results remain visible.
        // 先写出全部推荐行，再返回顾问错误，保证部分结果仍可见。
        let (results, result) =
            self.advisor
                .advise_indexes(ctx, &mut self.context, sqls, &self.options);
        for recommendation in results {
            let columns = recommendation.index_columns.join(",");
            req.AppendString(0, &recommendation.database);
            req.AppendString(1, &recommendation.table);
            req.AppendString(2, &recommendation.index_name);
            req.AppendString(3, &columns);
            req.AppendString(4, &recommendation.index_size);
            req.AppendString(5, &recommendation.reason);
            req.AppendString(6, &recommendation.top_impacted_queries_json);
            // 第 8 列给出可直接执行的 CREATE INDEX 语句。
            req.AppendString(
                7,
                &format!(
                    "CREATE INDEX {} ON {}({});",
                    recommendation.index_name, recommendation.table, columns
                ),
            );
        }
        result
    }

    /// 将顾问全部选项的名称、当前值与说明写入结果 Chunk。
    pub fn showOptions(&mut self, req: &mut Chunk) -> Result<(), A::Error> {
        let options = self.advisor.all_options().to_vec();
        let (values, descriptions) = self.advisor.get_options(&mut self.context)?;
        for option in options {
            if let Some(value) = values.get(&option) {
                req.AppendString(0, &option);
                req.AppendString(1, value);
                req.AppendString(2, descriptions.get(&option).map_or("", String::as_str));
            }
        }
        Ok(())
    }
}
