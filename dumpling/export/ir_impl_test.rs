// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `ir_impl_test.go`.
//!
//! 这些测试主要锁住 `ir_impl.rs` 里几类最基础的 IR 运行时语义：
//! `rowIter` 的预取与推进、`Decode` 的字符串解码，
//! 以及 `writerPipe` 在固定行宽下的 statement/file 切换阈值行为。

use crate::*;

// simpleRowReceiver 是测试专用接收器，用最小结构把一行结果保存成字符串数组。
struct simpleRowReceiver {
    data: Vec<String>,
}
impl simpleRowReceiver {
    fn new(length: usize) -> Self {
        Self {
            data: vec![String::new(); length],
        }
    }
}
impl RowReceiver for simpleRowReceiver {
    fn BindAddress(&mut self, args: &mut [RawBytes]) {
        // 这里故意沿用 Go 测试里“BindAddress + Scan”这一路径的外形。
        for (i, a) in args.iter_mut().enumerate() {
            if i < self.data.len() {
                a.0 = Some(self.data[i].as_bytes().to_vec());
            }
        }
        // Go binds pointers into receiver; for decode we copy from scanned bytes after Scan.
    }
}

/// Decode helper matching Go simpleRowReceiver usage via BindAddress + Scan.
/// 把 `SQLRowIter::Decode` 包成一个更接近 Go 测试调用方式的小 helper。
fn decode_into(iter: &mut dyn SQLRowIter, res: &mut simpleRowReceiver) -> Result<()> {
    // 每次 decode 都重新准备一组 RawBytes 槽，便于把结果拷进字符串目标。
    let mut args: Vec<RawBytes> = (0..res.data.len()).map(|_| RawBytes(None)).collect();
    iter.Decode(&mut BindAdapter {
        dest: &mut res.data,
        args: &mut args,
    })?;
    Ok(())
}

// BindAdapter 负责把 Decode 后的原始字节转换成可断言的 UTF-8 字符串。
struct BindAdapter<'a> {
    dest: &'a mut Vec<String>,
    args: &'a mut [RawBytes],
}
impl RowReceiver for BindAdapter<'_> {
    fn BindAddress(&mut self, args: &mut [RawBytes]) {
        // After Scan fills args, copy into dest strings.
        // 这是测试里最接近“最终用户看到的行值”的一步。
        for (i, a) in args.iter().enumerate() {
            if i < self.dest.len() {
                self.dest[i] = String::from_utf8_lossy(a.as_opt().unwrap_or(b"")).to_string();
            }
        }
        // `args` 字段只是为了保持适配器结构完整，当前测试不直接使用。
        let _ = self.args;
    }
}

#[test]
fn test_row_iter() {
    // 第一组用例验证 `rowIter` 的最基本契约：预取首行、Decode 当前行、Next 才推进。
    let rows = Rows::new(
        vec!["id".into()],
        vec![
            vec![Some(b"1".to_vec())],
            vec![Some(b"2".to_vec())],
            vec![Some(b"3".to_vec())],
        ],
    );
    let mut iter = newRowIter(rows, 1);
    // 连续多次 `HasNext` 不应偷偷消费结果集。
    for _ in 0..100 {
        assert!(iter.HasNext());
    }
    let mut res = simpleRowReceiver::new(1);
    // 第一次 decode 命中构造时预取到的第一行。
    decode_into(&mut iter, &mut res).unwrap();
    assert_eq!(res.data, vec!["1".to_string()]);

    // 调一次 Next 后，迭代器才真正前进到第二行。
    iter.Next();
    assert!(iter.HasNext());
    decode_into(&mut iter, &mut res).unwrap();
    assert_eq!(res.data, vec!["2".to_string()]);

    // 第三行读完后再 Next，应把 HasNext 置成 false。
    iter.Next();
    assert!(iter.HasNext());
    decode_into(&mut iter, &mut res).unwrap();
    iter.Next();
    assert_eq!(res.data, vec!["3".to_string()]);
    assert!(!iter.HasNext());
}

#[test]
fn test_chunk_row_iter() {
    // 第二组用例把 rowIter 接到 writerPipe 上，验证按 statement/file 切换时的累计尺寸。
    let twenty = "x".repeat(20);
    let thirty = "x".repeat(30);
    let mut data = Vec::new();
    // 每行固定 20 + 30 字节，便于手工推导累计值。
    for _ in 0..10 {
        data.push(vec![
            Some(twenty.as_bytes().to_vec()),
            Some(thirty.as_bytes().to_vec()),
        ]);
    }
    let rows = Rows::new(vec!["a".into(), "b".into()], data);
    let mut sql_row_iter = newRowIter(rows, 2);
    let mut res = simpleRowReceiver::new(2);
    let metrics = newMetrics(NewDefaultFactory().as_ref(), &Labels::default());
    let mut wp = newWriterPipe(None, 200, 101, Some(&metrics), None);

    // 期望值中的每一项分别表示 `[currentFileSize, currentStatementSize]`。
    // 因为每行 50 字节，所以语句尺寸按 50/100/150 递增，文件尺寸到 200 后触发切换。
    let expected = vec![
        vec![50u64, 50],
        vec![100, 100],
        vec![150, 150],
        vec![200, 50],
    ];
    let mut res_size = Vec::new();
    // 外层 while 模拟文件级切换，内层 while 模拟 statement 级切换。
    while sql_row_iter.HasNext() {
        wp.currentStatementSize = 0;
        while sql_row_iter.HasNext() {
            decode_into(&mut sql_row_iter, &mut res).unwrap();
            // 每行字符串长度之和就是当前写入字节数。
            let sz = (res.data[0].len() + res.data[1].len()) as u64;
            wp.AddFileSize(sz);
            sql_row_iter.Next();
            res_size.push(vec![wp.currentFileSize, wp.currentStatementSize]);
            // 达到 statement 阈值后只跳出内层，让外层决定是否切文件。
            if wp.ShouldSwitchStatement() {
                break;
            }
        }
        // 达到文件阈值后停止本轮，验证迭代器还停在下一条待消费记录。
        if wp.ShouldSwitchFile() {
            break;
        }
    }
    assert_eq!(expected, res_size);
    // 这里仍然有下一条，是因为切文件只中断循环，不会额外消费一行。
    assert!(sql_row_iter.HasNext());
    assert!(wp.ShouldSwitchFile());
    assert!(wp.ShouldSwitchStatement());
    // 与 Go 测试一致：关闭底层 rows 后，Decode 必须返回错误。
    sql_row_iter.Close().unwrap();
    assert!(decode_into(&mut sql_row_iter, &mut res).is_err());
    sql_row_iter.Next();
}
