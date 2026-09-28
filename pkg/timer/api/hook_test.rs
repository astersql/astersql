// Copyright 2026 AsterSQL.

use std::sync::Arc;

struct NoopHook;

impl Hook for NoopHook {
    fn Start(&mut self) {}

    fn Stop(&mut self) {}

    fn OnPreSchedEvent(
        &mut self,
        _ctx: &Context,
        _event: &dyn TimerShedEvent,
    ) -> TimerResult<PreSchedEventResult> {
        Ok(PreSchedEventResult::default())
    }

    fn OnSchedEvent(&mut self, _ctx: &Context, _event: &dyn TimerShedEvent) -> TimerResult<()> {
        Ok(())
    }
}

#[test]
fn hook_factory_accepts_capturing_closures_like_go() {
    let expected_class = String::from("captured-class");
    let factory: HookFactory = Arc::new(move |hook_class, _client| {
        assert_eq!(expected_class, hook_class);
        Box::new(NoopHook)
    });

    drop(factory);
}
