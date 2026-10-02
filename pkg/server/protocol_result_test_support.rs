// Copyright 2026 AsterSQL.

use super::*;

// Test-only time/failure injection at the actual result-set boundaries; rows,
// metadata, cursor iteration and ownership still come from the canonical session.
#[cfg(test)]
#[derive(Default)]
pub(super) struct BoundaryProbe {
    fault: Option<(&'static str, usize, std::time::Duration, bool)>,
    count: usize,
    events: Vec<String>,
}
#[cfg(test)]
impl BoundaryProbe {
    pub(super) fn before(&mut self, operation: &'static str) -> Result<(), sqlexec::GoError> {
        self.events.push(operation.into());
        if let Some((expected, at, delay, fail)) = self.fault {
            if expected == operation {
                self.count += 1;
                if at == self.count {
                    std::thread::sleep(delay);
                    if fail {
                        return Err(Box::new(ConnError::Session(format!(
                            "injected {operation} failure"
                        ))));
                    }
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
impl WorkerResults {
    pub(crate) fn set_fault(
        &mut self,
        operation: &'static str,
        at: usize,
        delay: std::time::Duration,
        fail: bool,
    ) {
        let mut probe = self.probe.lock().unwrap();
        probe.fault = Some((operation, at, delay, fail));
        probe.count = 0;
        probe.events.clear();
    }
    pub(crate) fn events(&self) -> Vec<String> {
        self.probe.lock().unwrap().events.clone()
    }
}
