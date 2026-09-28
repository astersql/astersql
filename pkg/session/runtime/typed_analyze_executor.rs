// Copyright 2026 AsterSQL.

use std::rc::Rc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_executor::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, SchemaColumn,
};
use astersql_parser_ast as ast;
use astersql_util_chunk as chunk;

use super::{ConcreteSession, parse_with_sql_mode};

pub(super) struct SessionTypedAnalyzeExecutor {
    session: Rc<ConcreteSession>,
    sql: String,
    opened: bool,
    done: bool,
    schema: Vec<SchemaColumn>,
}

struct RestrictedAnalyzeGuard<'a> {
    session: &'a ConcreteSession,
    build_concurrency: usize,
    scan_concurrency: Option<i32>,
    isolation: String,
}

impl Drop for RestrictedAnalyzeGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.session.state.borrow_mut();
        state.analyze_concurrency = self.build_concurrency;
        state.restricted_analyze_scan_concurrency = self.scan_concurrency;
        state.transaction_isolation = std::mem::take(&mut self.isolation);
    }
}

impl SessionTypedAnalyzeExecutor {
    pub(super) fn new(session: Rc<ConcreteSession>, sql: String) -> Self {
        Self {
            session,
            sql,
            opened: false,
            done: false,
            schema: Vec::new(),
        }
    }

    fn execute(&self) -> AdapterResult {
        let mode =
            astersql_parser_mysql::r#const::GetSQLMode(&self.session.state.borrow().sql_mode)
                .map_err(|error| errors::New(error.to_string()))?;
        let statements =
            parse_with_sql_mode(&self.sql, mode).map_err(|error| errors::New(error.to_string()))?;
        if statements.len() != 1 {
            return Err(errors::New(
                "bound ANALYZE must contain exactly one statement",
            ));
        }
        let statement = statements[0]
            .as_any()
            .downcast_ref::<ast::AnalyzeTableStmt>()
            .ok_or_else(|| errors::New("bound ANALYZE plan/AST kind mismatch"))?;
        let restricted = self.session.state.borrow().in_restricted_sql
            || self.session.WithSessionVars(|vars| vars.InRestrictedSQL);
        let _guard = if restricted {
            let auto_build = self
                .session
                .WithSessionVars(|vars| {
                    vars.GetSessionOrGlobalSystemVar(
                        astersql_sessionctx_variable::Context,
                        astersql_sessionctx_vardef::TiDBAutoBuildStatsConcurrency,
                    )
                })
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| (1..=256).contains(value));
            let sysproc_scan = self
                .session
                .WithSessionVars(|vars| {
                    vars.GetSessionOrGlobalSystemVar(
                        astersql_sessionctx_variable::Context,
                        astersql_sessionctx_vardef::TiDBSysProcScanConcurrency,
                    )
                })
                .ok()
                .and_then(|value| value.parse::<i32>().ok())
                .filter(|value| *value > 0);
            let mut state = self.session.state.borrow_mut();
            let guard = RestrictedAnalyzeGuard {
                session: &self.session,
                build_concurrency: state.analyze_concurrency,
                scan_concurrency: state.restricted_analyze_scan_concurrency,
                isolation: state.transaction_isolation.clone(),
            };
            if let Some(auto_build) = auto_build {
                state.analyze_concurrency = auto_build;
            }
            state.restricted_analyze_scan_concurrency = sysproc_scan;
            state.transaction_isolation = "READ-COMMITTED".into();
            Some(guard)
        } else {
            None
        };
        self.session
            .execute_analyze(statement)
            .map_err(|error| error.into_shared())
    }
}

impl ExecExecutor for SessionTypedAnalyzeExecutor {
    fn Open(&mut self) -> AdapterResult {
        self.opened = true;
        self.done = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.opened = false;
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        if !self.opened {
            return Err(errors::New("ANALYZE executor is not open"));
        }
        output.Reset();
        if !self.done {
            self.done = true;
            self.execute()?;
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig {
            fields: Vec::new(),
            initial_capacity: 1,
            maximum_chunk_size: 1,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        *chunk::New(Vec::<astersql_parser_types::FieldType>::new(), 1, 1)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        true
    }
    fn IsWriteExecutor(&self) -> bool {
        true
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        Vec::new()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
