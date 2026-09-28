// Copyright 2026 AsterSQL.

use std::rc::Rc;
use std::time::Duration;

use astersql_errors as errors;
use astersql_executor::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, SchemaColumn,
};
use astersql_util_chunk as chunk;

use super::{ConcreteSession, RuntimeForeignKeyDeleteCascade};

/// One pending parent-row cascade batch. The canonical DML routine still owns
/// the full recursive DELETE/SET NULL/restrict algorithm and its depth guard.
pub(super) struct SessionForeignKeyDeleteCascadeBatch {
    session: Rc<ConcreteSession>,
    pending: Option<RuntimeForeignKeyDeleteCascade>,
}

impl SessionForeignKeyDeleteCascadeBatch {
    pub(super) fn new(
        session: Rc<ConcreteSession>,
        pending: RuntimeForeignKeyDeleteCascade,
    ) -> Self {
        Self {
            session,
            pending: Some(pending),
        }
    }
}

impl CascadeBatch for SessionForeignKeyDeleteCascadeBatch {
    fn HasPendingRows(&self) -> bool {
        self.pending.is_some()
    }

    fn BuildExecutor(&mut self) -> AdapterResult<Option<Box<dyn ExecExecutor>>> {
        Ok(self.pending.as_ref().map(|pending| {
            Box::new(SessionForeignKeyDeleteCascadeExecutor::new(
                Rc::clone(&self.session),
                pending.clone(),
            )) as Box<dyn ExecExecutor>
        }))
    }

    fn MarkBatchComplete(&mut self) {
        self.pending.take();
    }
}

struct SessionForeignKeyDeleteCascadeExecutor {
    session: Rc<ConcreteSession>,
    pending: RuntimeForeignKeyDeleteCascade,
    opened: bool,
    done: bool,
    schema: Vec<SchemaColumn>,
    config: ChunkConfig,
}

impl SessionForeignKeyDeleteCascadeExecutor {
    fn new(session: Rc<ConcreteSession>, pending: RuntimeForeignKeyDeleteCascade) -> Self {
        Self {
            session,
            pending,
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
}

struct FKLockDeferGuard<'a>(&'a ConcreteSession);

impl Drop for FKLockDeferGuard<'_> {
    fn drop(&mut self) {
        self.0.state.borrow_mut().adapter_dml_defer_fk_locks = false;
    }
}

impl ExecExecutor for SessionForeignKeyDeleteCascadeExecutor {
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
            return Err(errors::New("FK cascade executor is not open"));
        }
        output.Reset();
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.session.state.borrow_mut().adapter_dml_defer_fk_locks = true;
        let _guard = FKLockDeferGuard(&self.session);
        self.session
            .execute_pending_fk_delete_cascade(&self.pending)
            .map_err(|error| error.into_shared())
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
