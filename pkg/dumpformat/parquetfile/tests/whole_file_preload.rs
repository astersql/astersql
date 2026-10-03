// Copyright 2026 AsterSQL.
use astersql_dumpformat_parquetfile::source_reader::{RangeOpener, SourceReader};
use parquet::file::reader::ChunkReader;
use std::io::Cursor;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn exact_threshold_preloads_once_without_opening_footer_reader() {
    let opens = Arc::new(AtomicUsize::new(0));
    let count = opens.clone();
    let open: RangeOpener = Arc::new(move |start, end| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Cursor::new(
            (start..end).map(|n| n as u8).collect::<Vec<_>>(),
        )))
    });
    let source = SourceReader::prepare_with_thresholds(
        64,
        || panic!("whole-file preload must not open footer reader"),
        open,
        64,
        128,
    )
    .unwrap();
    assert!(source.whole_file_preloaded());
    source
        .set_ranges(vec![(4, 24), (24, 48)], vec![(4, 12), (12, 24), (24, 48)])
        .unwrap();
    assert_eq!(
        source.get_bytes(4, 8).unwrap().as_ref(),
        &[4, 5, 6, 7, 8, 9, 10, 11]
    );
    assert_eq!(
        source.get_bytes(24, 8).unwrap().as_ref(),
        &[24, 25, 26, 27, 28, 29, 30, 31]
    );
    assert_eq!(source.buffer_bytes(), 64);
    assert_eq!(opens.load(Ordering::SeqCst), 1);
}

fn parquet_bytes() -> Vec<u8> {
    parquet_bytes_groups(2)
}
fn parquet_bytes_groups(groups: i64) -> Vec<u8> {
    use parquet::data_type::Int64Type;
    use parquet::file::{properties::WriterProperties, writer::SerializedFileWriter};
    use parquet::schema::parser::parse_message_type;
    let schema = Arc::new(parse_message_type("message schema { OPTIONAL INT64 V; }").unwrap());
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        schema,
        Arc::new(
            WriterProperties::builder()
                .set_dictionary_enabled(false)
                .build(),
        ),
    )
    .unwrap();
    for group_index in 0..groups {
        let mut group = writer.next_row_group().unwrap();
        let mut column = group.next_column().unwrap().unwrap();
        let values: Vec<i64> = (group_index * 32..(group_index + 1) * 32).collect();
        column
            .typed::<Int64Type>()
            .write_batch(&values, Some(&[1; 32]), None)
            .unwrap();
        column.close().unwrap();
        group.close().unwrap();
    }
    writer.into_inner().unwrap()
}

#[test]
fn real_rows_and_eof_match_across_whole_group_and_column_strategies() {
    use astersql_dumpformat_parquetfile::{file_parser::FileParser, type_converter::Datum};
    let bytes = Arc::new(parquet_bytes());
    let size = bytes.len() as u64;
    for (known, whole, group, expected_whole) in [
        (size as i64, size, 128 << 20, true),
        (0, size, 128 << 20, false),
        (size as i64, size - 1, 128 << 20, false),
        (size as i64, size - 1, 1, false),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let data = bytes.clone();
        let open: RangeOpener = Arc::new(move |start, end| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Cursor::new(
                data[start as usize..end as usize].to_vec(),
            )))
        });
        let count = requests.clone();
        let source = SourceReader::prepare_with_thresholds(
            known,
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(size)
            },
            open,
            whole,
            group,
        )
        .unwrap();
        assert_eq!(source.whole_file_preloaded(), expected_whole);
        let mut parser = FileParser::new(source).unwrap();
        assert_eq!(parser.columns(), ["v"]);
        assert_eq!(parser.total_rows(), 64);
        for i in 0..64 {
            assert_eq!(parser.read_row().unwrap(), vec![Datum::Int(i)]);
        }
        assert_eq!(parser.read_row().unwrap_err().0, "EOF");
        if expected_whole {
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            assert_eq!(parser.source().buffer_bytes(), size);
        } else {
            assert!(requests.load(Ordering::SeqCst) > 1);
        }
        parser.close();
        assert!(parser.read_row().unwrap_err().0.contains("closed"));
    }
}

#[test]
fn preload_read_failure_does_not_fall_back_to_opener() {
    let open: RangeOpener = Arc::new(|_, _| Ok(Box::new(Cursor::new(vec![1, 2]))));
    assert!(SourceReader::prepare(16, || panic!("failed preload must propagate"), open).is_err());
}

#[test]
fn real_decoder_keeps_null_decimal_and_adjusted_timestamp_semantics() {
    use astersql_dumpformat_parquetfile::{
        file_parser::{FileParser, ImportParser},
        type_converter::Datum,
    };
    use astersql_lightning_mydump::Parser;
    use parquet::{
        data_type::Int64Type,
        file::{properties::WriterProperties, writer::SerializedFileWriter},
        schema::parser::parse_message_type,
    };
    let schema=Arc::new(parse_message_type("message schema { OPTIONAL INT64 amount (DECIMAL(12,2)); OPTIONAL INT64 stamp (TIMESTAMP_MICROS); }").unwrap());
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        schema,
        Arc::new(WriterProperties::builder().build()),
    )
    .unwrap();
    let mut group = writer.next_row_group().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    column
        .typed::<Int64Type>()
        .write_batch(&[1234, -1234], Some(&[1, 0, 1]), None)
        .unwrap();
    column.close().unwrap();
    let mut column = group.next_column().unwrap().unwrap();
    column
        .typed::<Int64Type>()
        .write_batch(&[0, 1_000_000], Some(&[1, 0, 1]), None)
        .unwrap();
    column.close().unwrap();
    group.close().unwrap();
    let bytes = Arc::new(writer.into_inner().unwrap());
    let size = bytes.len();
    let data = bytes.clone();
    let open: RangeOpener = Arc::new(move |start, end| {
        Ok(Box::new(Cursor::new(
            data[start as usize..end as usize].to_vec(),
        )))
    });
    let source = SourceReader::prepare(size as i64, || panic!("whole file"), open).unwrap();
    let mut parser = FileParser::new_with_location(source.clone(), "Asia/Shanghai").unwrap();
    assert_eq!(
        parser.read_row().unwrap(),
        vec![
            Datum::Decimal("12.34".into()),
            Datum::TimeMicros(8 * 3600 * 1_000_000)
        ]
    );
    assert_eq!(parser.read_row().unwrap(), vec![Datum::Null, Datum::Null]);
    let mut adapter =
        ImportParser::new(FileParser::new_with_location(source, "Asia/Shanghai").unwrap());
    adapter.ReadRow().unwrap();
    assert_eq!(adapter.LastRow().length, 16);
    assert_eq!(adapter.ScannedPos().unwrap(), 16);
    adapter.RecycleRow(adapter.LastRow());
    assert_eq!(
        adapter.LastRow().row[1],
        astersql_lightning_mydump::Datum::Bytes(b"1970-01-01 08:00:00.000000".to_vec())
    );
}

#[test]
fn whole_buffer_replaces_row_group_charge_without_changing_decoder_estimate() {
    use astersql_dumpformat_parquetfile::file_parser::FileParser;
    let bytes = Arc::new(parquet_bytes());
    let size = bytes.len() as u64;
    let data = bytes.clone();
    let open: RangeOpener = Arc::new(move |start, end| {
        Ok(Box::new(Cursor::new(
            data[start as usize..end as usize].to_vec(),
        )))
    });
    let source = SourceReader::prepare(size as i64, || panic!("whole file"), open).unwrap();
    let parser = FileParser::new(source).unwrap();
    // The pre-existing decoder estimate is supplied by the runtime. This
    // commit changes its preload component, and must not double charge it.
    use parquet::file::reader::FileReader;
    let reader = astersql_dumpformat_parquetfile::parser::open_file_reader(
        bytes::Bytes::copy_from_slice(&bytes),
    )
    .unwrap();
    let group = reader.metadata().row_group(0);
    let (_, old_size) = group.column(0).byte_range();
    assert_eq!(
        parser
            .adjust_memory_estimate(4096 + old_size as i64)
            .unwrap(),
        4096 + size as i64
    );
}

#[test]
fn http_get_counts_match_whole_unknown_oversized_and_streaming_branches() {
    use astersql_dumpformat_parquetfile::{file_parser::FileParser, type_converter::Datum};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicBool;
    let bytes = Arc::new(parquet_bytes_groups(1));
    let size = bytes.len() as u64;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let stopping = done.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let data = bytes.clone();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let request = String::from_utf8(request).unwrap();
            let range = request
                .lines()
                .find_map(|line| line.strip_prefix("Range: bytes="))
                .unwrap();
            let (start, end) = range.split_once('-').unwrap();
            let start = start.parse::<usize>().unwrap();
            let end = end.parse::<usize>().unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            write!(stream,"HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",end-start+1,data.len()).unwrap();
            stream.write_all(&data[start..=end]).unwrap();
        }
    });
    // Only the external HTTP boundary is replaced. The real decoder consumes
    // these response bodies and all request counts come from the server.
    let fetch = move |start: u64, end: u64| -> std::io::Result<Vec<u8>> {
        let mut socket = TcpStream::connect(address)?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        write!(
            socket,
            "GET /data.parquet HTTP/1.1\r\nHost: localhost\r\nRange: bytes={start}-{}\r\nConnection: close\r\n\r\n",
            end - 1
        )?;
        let mut response = Vec::new();
        socket.read_to_end(&mut response)?;
        let header = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap()
            + 4;
        Ok(response[header..].to_vec())
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for (known, whole, group, expected) in [
            (size as i64, size, 128 << 20, 1),
            (0, size, 128 << 20, 4),
            (size as i64, size - 1, 128 << 20, 4),
            (size as i64, size - 1, 1, 4),
        ] {
            requests.store(0, Ordering::SeqCst);
            let open: RangeOpener =
                Arc::new(move |start, end| Ok(Box::new(Cursor::new(fetch(start, end)?))));
            let source = SourceReader::prepare_with_thresholds(
                known,
                || {
                    fetch(0, 8)?;
                    Ok(size)
                },
                open,
                whole,
                group,
            )
            .unwrap();
            let mut parser = FileParser::new(source).unwrap();
            for value in 0..32 {
                assert_eq!(parser.read_row().unwrap(), vec![Datum::Int(value)]);
            }
            assert_eq!(parser.read_row().unwrap_err().0, "EOF");
            assert_eq!(requests.load(Ordering::SeqCst), expected);
        }
    }));
    done.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(address);
    server.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[test]
fn closing_releases_shared_preload_and_range_errors_remain_errors() {
    let open: RangeOpener =
        Arc::new(|start, end| Ok(Box::new(Cursor::new(vec![0; (end - start) as usize]))));
    let source = SourceReader::prepare(64, || panic!("whole"), open).unwrap();
    assert!(source.get_bytes(63, 2).is_err());
    assert!(source.get_read(65).is_err());
    assert!(source.get_bytes(64, 0).unwrap().is_empty());
    source.close();
    assert_eq!(source.buffer_bytes(), 0);
    assert!(
        source
            .get_bytes(0, 1)
            .unwrap_err()
            .to_string()
            .contains("closed")
    );
}
