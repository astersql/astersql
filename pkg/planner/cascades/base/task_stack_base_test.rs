// Copyright 2026 AsterSQL.

use std::error::Error;

struct NoopTask;

impl Task for NoopTask {
    fn Execute(&mut self) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn Desc(&self, _writer: &mut dyn util::StrBufferWriter) {}
}

#[derive(Default)]
struct VecStack(Vec<Box<dyn Task>>);

impl Stack for VecStack {
    fn Push(&mut self, task: Box<dyn Task>) {
        self.0.push(task);
    }

    fn Pop(&mut self) -> Option<Box<dyn Task>> {
        self.0.pop()
    }

    fn Empty(&self) -> bool {
        self.0.is_empty()
    }

    fn Destroy(&mut self) {
        self.0.clear();
    }
}

#[test]
fn pop_returns_none_for_an_empty_stack() {
    let mut stack = VecStack::default();
    assert!(stack.Pop().is_none());

    stack.Push(Box::new(NoopTask));
    assert!(stack.Pop().is_some());
    assert!(stack.Pop().is_none());
}
