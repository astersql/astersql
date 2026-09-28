// Copyright 2026 AsterSQL.

use std::rc::Rc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_executor::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, SchemaColumn,
};
use astersql_parser_ast as ast;
use astersql_util_chunk as chunk;

use super::typed_fk_cascade_executor::SessionForeignKeyDeleteCascadeBatch;
use super::{ConcreteSession, parse_with_sql_mode};

/// Executes the bound canonical DML AST in Next, after adapter Open and write guards.
/// The transaction and mutation collector remain owned by ConcreteSession.
pub(super) struct SessionTypedDMLExecutor {
    session: Rc<ConcreteSession>,
    sql: String,
    kind: astersql_executor::adapter::PlanKind,
    opened: bool,
    done: bool,
    schema: Vec<SchemaColumn>,
    config: ChunkConfig,
}

struct FKLockDeferGuard<'a>(&'a ConcreteSession);

impl Drop for FKLockDeferGuard<'_> {
    fn drop(&mut self) {
        self.0.state.borrow_mut().adapter_dml_defer_fk_locks = false;
    }
}

impl SessionTypedDMLExecutor {
    pub(super) fn new(
        session: Rc<ConcreteSession>,
        sql: String,
        kind: astersql_executor::adapter::PlanKind,
    ) -> Self {
        Self {
            session,
            sql,
            kind,
            opened: false,
            done: false,
            schema: Vec::new(),
            config: ChunkConfig {
                fields: Vec::new(),
                initial_capacity: 1,
                maximum_chunk_size: 1,
            },
        }
    }

    fn execute(&self) -> AdapterResult {
        let defer_fk_locks = self.session.TransactionIsPessimistic()
            && self.session.state.borrow().adapter_dml_statement_staged;
        self.session.state.borrow_mut().adapter_dml_defer_fk_locks = defer_fk_locks;
        let _defer_guard = FKLockDeferGuard(&self.session);
        let mode =
            astersql_parser_mysql::r#const::GetSQLMode(&self.session.state.borrow().sql_mode)
                .map_err(|error| errors::New(error.to_string()))?;
        let statements =
            parse_with_sql_mode(&self.sql, mode).map_err(|error| errors::New(error.to_string()))?;
        if statements.len() != 1 {
            return Err(errors::New("bound DML must contain exactly one statement"));
        }
        let node = statements[0].as_ref();
        match self.kind {
            astersql_executor::adapter::PlanKind::Insert => {
                let insert = node
                    .as_any()
                    .downcast_ref::<ast::InsertStmt>()
                    .ok_or_else(|| errors::New("bound DML plan/AST kind mismatch"))?;
                self.session
                    .execute_insert(insert)
                    .map_err(|error| error.into_shared())
            }
            astersql_executor::adapter::PlanKind::Update => {
                let update = node
                    .as_any()
                    .downcast_ref::<ast::UpdateStmt>()
                    .ok_or_else(|| errors::New("bound DML plan/AST kind mismatch"))?;
                self.session
                    .execute_update(update)
                    .map_err(|error| error.into_shared())
            }
            astersql_executor::adapter::PlanKind::Delete => {
                let delete = node
                    .as_any()
                    .downcast_ref::<ast::DeleteStmt>()
                    .ok_or_else(|| errors::New("bound DML plan/AST kind mismatch"))?;
                self.session
                    .execute_delete(delete)
                    .map_err(|error| error.into_shared())
            }
            _ => Err(errors::New("typed DML executor requires a DML plan")),
        }
    }
}

impl ExecExecutor for SessionTypedDMLExecutor {
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
            return Err(errors::New("typed DML executor is not open"));
        }
        output.Reset();
        if !self.done {
            self.done = true;
            self.execute()?;
        }
        Ok(())
    }
    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if let Some(killer) = &context.sql_killer {
            killer.HandleSignal()?;
        }
        self.Next(output)
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        self.config.clone()
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
        let pending =
            std::mem::take(&mut self.session.state.borrow_mut().pending_fk_delete_cascades);
        pending
            .into_iter()
            .map(|batch| {
                Box::new(SessionForeignKeyDeleteCascadeBatch::new(
                    Rc::clone(&self.session),
                    batch,
                )) as Box<dyn CascadeBatch>
            })
            .collect()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        self.kind == astersql_executor::adapter::PlanKind::Delete
            && self.session.state.borrow().foreign_key_checks
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
