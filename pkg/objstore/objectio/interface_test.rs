// Copyright 2026 AsterSQL.
use astersql_objstore_objectio::{Context, NewIOWriter, Writer};
use std::io::{self, Write};
struct Sink {
    data: Vec<u8>,
}
impl Writer for Sink {
    fn write(&mut self, ctx: &Context, data: &[u8]) -> io::Result<usize> {
        ctx.check()?;
        self.data.extend_from_slice(data);
        Ok(data.len())
    }
    fn close(&mut self, _: &Context) -> io::Result<()> {
        panic!("adapter must not close the object")
    }
}
#[test]
fn io_adapter_binds_context_and_propagates_cancellation() {
    let context = Context::default();
    let mut sink = Sink { data: vec![] };
    let mut adapter = NewIOWriter(context.clone(), &mut sink);
    assert_eq!(adapter.write(b"row").unwrap(), 3);
    context.cancel();
    assert_eq!(
        adapter.write(b"next").unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    adapter.flush().unwrap();
    drop(adapter);
    assert_eq!(sink.data, b"row");
}
#[test]
fn io_adapter_preserves_short_writes_and_errors() {
    struct Short;
    impl Writer for Short {
        fn write(&mut self, _: &Context, data: &[u8]) -> io::Result<usize> {
            if data.is_empty() {
                Err(io::Error::other("original"))
            } else {
                Ok(1)
            }
        }
        fn close(&mut self, _: &Context) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Short;
    let mut adapter = NewIOWriter(Context::default(), &mut sink);
    assert_eq!(adapter.write(b"abc").unwrap(), 1);
    assert_eq!(adapter.write(b"").unwrap_err().to_string(), "original");
}
