// Copyright 2026 AsterSQL.

use std::time::Duration;

use astersql_errors as errors;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};
use crate::builder::ExecutorBox;

pub(crate) trait UnionChild {
    fn executor(&self) -> &dyn ExecExecutor;
    fn executor_mut(&mut self) -> &mut dyn ExecExecutor;
}

impl UnionChild for ExecutorBox {
    fn executor(&self) -> &dyn ExecExecutor {
        self.as_ref()
    }

    fn executor_mut(&mut self) -> &mut dyn ExecExecutor {
        self.as_mut()
    }
}

impl UnionChild for Box<dyn ExecExecutor> {
    fn executor(&self) -> &dyn ExecExecutor {
        self.as_ref()
    }

    fn executor_mut(&mut self) -> &mut dyn ExecExecutor {
        self.as_mut()
    }
}

/// Streams every child of a canonical UnionAll or PartitionUnion plan.
pub(crate) struct TypedUnionAll<C = ExecutorBox> {
    children: Vec<C>,
    current: usize,
    page_keys: Vec<Key>,
    opened: bool,
    closed: bool,
}

impl TypedUnionAll<ExecutorBox> {
    pub fn new(children: Vec<ExecutorBox>) -> Result<Self, String> {
        let Some(first) = children.first() else {
            return Err("typed UnionAll requires at least one child".to_owned());
        };
        if children
            .iter()
            .skip(1)
            .any(|child| child.Schema() != first.Schema())
        {
            return Err("typed UnionAll child schemas do not match".to_owned());
        }
        Ok(Self {
            children,
            current: 0,
            page_keys: Vec::new(),
            opened: false,
            closed: false,
        })
    }
}

impl<C: UnionChild> TypedUnionAll<C> {
    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(errors::New("union executor is not open"));
        }
        self.page_keys.clear();
        while let Some(child) = self.children.get_mut(self.current) {
            if let Some(context) = context {
                child.executor_mut().NextWithContext(context, output)?;
            } else {
                child.executor_mut().Next(output)?;
            }
            self.page_keys = child.executor_mut().TakeLockKeys();
            if output.NumRows() != 0 {
                return Ok(());
            }
            self.current += 1;
        }
        output.Reset();
        Ok(())
    }
}

impl<C: UnionChild + 'static> ExecExecutor for TypedUnionAll<C> {
    fn Open(&mut self) -> AdapterResult {
        let mut opened = 0;
        for child in &mut self.children {
            if let Err(error) = child.executor_mut().Open() {
                for opened_child in self.children[..opened].iter_mut().rev() {
                    let _ = opened_child.executor_mut().Close();
                }
                return Err(error);
            }
            opened += 1;
        }
        self.current = 0;
        self.page_keys.clear();
        self.opened = true;
        self.closed = false;
        Ok(())
    }

    fn Close(&mut self) -> AdapterResult {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut first_error = None;
        for child in &mut self.children {
            if let Err(error) = child.executor_mut().Close()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.next_inner(None, output)
    }

    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.next_inner(Some(context), output)
    }

    fn ChunkConfig(&self) -> ChunkConfig {
        self.children[0].executor().ChunkConfig()
    }

    fn NewChunk(&self) -> chunk::Chunk {
        self.children[0].executor().NewChunk()
    }

    fn Schema(&self) -> &[SchemaColumn] {
        self.children[0].executor().Schema()
    }

    fn CalculateNoDelay(&self) -> bool {
        self.children
            .iter()
            .all(|child| child.executor().CalculateNoDelay())
    }

    fn IsWriteExecutor(&self) -> bool {
        self.children
            .iter()
            .any(|child| child.executor().IsWriteExecutor())
    }

    fn CheckForeignKeys(&mut self) -> AdapterResult {
        for child in &mut self.children {
            child.executor_mut().CheckForeignKeys()?;
        }
        Ok(())
    }

    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        self.children
            .iter_mut()
            .flat_map(|child| child.executor_mut().TakeForeignKeyCascades())
            .collect()
    }

    fn HasForeignKeyCascades(&self) -> bool {
        self.children
            .iter()
            .any(|child| child.executor().HasForeignKeyCascades())
    }

    fn PrepareFKCascadeContext(&mut self) {
        for child in &mut self.children {
            child.executor_mut().PrepareFKCascadeContext();
        }
    }

    fn AddFKCheckLockDuration(&mut self, duration: Duration) {
        for child in &mut self.children {
            child.executor_mut().AddFKCheckLockDuration(duration);
        }
    }

    fn TakeLockKeys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.page_keys)
    }

    fn ScannedRows(&self) -> usize {
        self.children
            .iter()
            .map(|child| child.executor().ScannedRows())
            .sum()
    }

    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        let children = self
            .children
            .iter_mut()
            .map(|child| child.executor_mut().Detach())
            .collect::<Option<Vec<_>>>()?;
        Some(Box::new(TypedUnionAll::<Box<dyn ExecExecutor>> {
            children,
            current: self.current,
            page_keys: Vec::new(),
            opened: self.opened,
            closed: self.closed,
        }))
    }
}
