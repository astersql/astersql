// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Chunk/Column 行拷贝工具、临时磁盘读写封装，以及列交换辅助结构。
//
// Join 等算子通过 selection 向量挑选命中行拷贝到目标 Chunk；
// `diskFileReaderWriter` 在 spill 文件上叠加 checksum/可选 AES 加密；
// `ColumnSwapHelper` 处理投影中同源列多次引用时的合并与交换。

use std::collections::HashMap;

// CopySelectedJoinRowsDirect directly copies the selected joined rows from the source Chunk
// to the destination Chunk. Return true if at least one joined row was selected.
/// 把源 Chunk 中 selected 为 true 的 join 行直接拷到目标；至少选中一行则返回 true。
pub fn CopySelectedJoinRowsDirect(
    src: &Chunk,
    selected: &[bool],
    dst: &mut Chunk,
) -> Result<bool, errors::Error> {
    if src.NumRows() == 0 {
        return Ok(false);
    }
    if src.sel.is_some() || dst.sel.is_some() {
        return Err(errors::New(MSG_ERR_SEL_NOT_NIL));
    }
    if src.columns.is_empty() {
        let numSelected = selected.iter().filter(|v| **v).count();
        dst.numVirtualRows += numSelected;
        return Ok(numSelected > 0);
    }

    let oldLen = dst.columns[0].length;
    for (j, srcCol) in src.columns.iter().enumerate() {
        let dstCol = &mut dst.columns[j];
        if srcCol.IsFixed() {
            for (i, is_selected) in selected.iter().enumerate() {
                if !*is_selected {
                    continue;
                }
                dstCol.appendNullBitmap(!srcCol.IsNull(i));
                dstCol.length += 1;
                let elemLen = srcCol.elemBuf.len();
                let offset = i * elemLen;
                dstCol
                    .data
                    .extend_from_slice(&srcCol.data[offset..offset + elemLen]);
            }
        } else {
            for (i, is_selected) in selected.iter().enumerate() {
                if !*is_selected {
                    continue;
                }
                dstCol.appendNullBitmap(!srcCol.IsNull(i));
                dstCol.length += 1;
                let start = srcCol.offsets[i] as usize;
                let end = srcCol.offsets[i + 1] as usize;
                dstCol.data.extend_from_slice(&srcCol.data[start..end]);
                dstCol.offsets.push(dstCol.data.len() as i64);
            }
        }
    }
    let numSelected = dst.columns[0].length - oldLen;
    dst.numVirtualRows += numSelected;
    Ok(numSelected > 0)
}

// CopySelectedJoinRowsWithSameOuterRows copies selected joined rows.
// NOTE: All the outer rows in the source Chunk should be the same.
/// 拷贝选中的 join 行；要求源 Chunk 中所有 outer 行相同以便批量复制。
pub fn CopySelectedJoinRowsWithSameOuterRows(
    src: &Chunk,
    innerColOffset: usize,
    innerColLen: usize,
    outerColOffset: usize,
    outerColLen: usize,
    selected: &[bool],
    dst: &mut Chunk,
) -> Result<bool, errors::Error> {
    if src.NumRows() == 0 {
        return Ok(false);
    }
    if src.sel.is_some() || dst.sel.is_some() {
        return Err(errors::New(MSG_ERR_SEL_NOT_NIL));
    }

    let numSelected = copySelectedInnerRows(innerColOffset, innerColLen, src, selected, dst);
    copySameOuterRows(outerColOffset, outerColLen, src, numSelected, dst);
    dst.numVirtualRows += numSelected;
    Ok(numSelected > 0)
}

// CopySelectedRows copies the selected rows in srcCol to dstCol.
/// 按 bool selection 把源列选中行拷到目标列。
pub fn CopySelectedRows(dstCol: &mut Column, srcCol: &Column, selected: &[bool]) {
    CopySelectedRowsWithRowIDFunc(dstCol, srcCol, selected, 0, selected.len(), |i| i)
}

// CopySelectedRowsWithRowIDFunc copies the selected rows in srcCol to dstCol.
/// 带行号映射函数的选中行拷贝（selected 为 true 时拷贝）。
pub fn CopySelectedRowsWithRowIDFunc<F>(
    dstCol: &mut Column,
    srcCol: &Column,
    selected: &[bool],
    start: usize,
    end: usize,
    rowIDFunc: F,
) where
    F: Fn(usize) -> usize,
{
    CopyExpectedRowsWithRowIDFunc(dstCol, srcCol, selected, true, start, end, rowIDFunc)
}

// CopyExpectedRowsWithRowIDFunc copies the expected rows in srcCol to dstCol.
/// 拷贝 `selected[i] == expectedResult` 的行，支持自定义物理行号映射。
pub fn CopyExpectedRowsWithRowIDFunc<F>(
    dstCol: &mut Column,
    srcCol: &Column,
    selected: &[bool],
    expectedResult: bool,
    start: usize,
    end: usize,
    rowIDFunc: F,
) where
    F: Fn(usize) -> usize,
{
    if srcCol.IsFixed() {
        for i in start..end {
            if selected[i] != expectedResult {
                continue;
            }
            let rowID = rowIDFunc(i);
            dstCol.appendNullBitmap(!srcCol.IsNull(rowID));
            dstCol.length += 1;
            let elemLen = srcCol.elemBuf.len();
            let offset = rowID * elemLen;
            dstCol
                .data
                .extend_from_slice(&srcCol.data[offset..offset + elemLen]);
        }
    } else {
        for i in start..end {
            if selected[i] != expectedResult {
                continue;
            }
            let rowID = rowIDFunc(i);
            dstCol.appendNullBitmap(!srcCol.IsNull(rowID));
            dstCol.length += 1;
            let start = srcCol.offsets[rowID] as usize;
            let end = srcCol.offsets[rowID + 1] as usize;
            dstCol.data.extend_from_slice(&srcCol.data[start..end]);
            dstCol.offsets.push(dstCol.data.len() as i64);
        }
    }
}

// CopyRows copies all rows in srcCol to dstCol.
/// 按物理行下标列表拷贝源列到目标列。
pub fn CopyRows(dstCol: &mut Column, srcCol: &Column, selected: &[usize]) {
    if srcCol.IsFixed() {
        for rowID in selected {
            dstCol.appendNullBitmap(!srcCol.IsNull(*rowID));
            dstCol.length += 1;
            let elemLen = srcCol.elemBuf.len();
            let offset = *rowID * elemLen;
            dstCol
                .data
                .extend_from_slice(&srcCol.data[offset..offset + elemLen]);
        }
    } else {
        for rowID in selected {
            dstCol.appendNullBitmap(!srcCol.IsNull(*rowID));
            dstCol.length += 1;
            let start = srcCol.offsets[*rowID] as usize;
            let end = srcCol.offsets[*rowID + 1] as usize;
            dstCol.data.extend_from_slice(&srcCol.data[start..end]);
            dstCol.offsets.push(dstCol.data.len() as i64);
        }
    }
}

// copySelectedInnerRows copies selected inner rows and returns selected count.
/// 拷贝选中的 inner 列区间，返回选中行数。
pub fn copySelectedInnerRows(
    innerColOffset: usize,
    innerColLen: usize,
    src: &Chunk,
    selected: &[bool],
    dst: &mut Chunk,
) -> usize {
    let srcCols = &src.columns[innerColOffset..innerColOffset + innerColLen];
    if srcCols.is_empty() {
        return selected.iter().filter(|v| **v).count();
    }
    let oldLen = dst.columns[innerColOffset].length;
    for (j, srcCol) in srcCols.iter().enumerate() {
        let dstCol = &mut dst.columns[innerColOffset + j];
        CopySelectedRows(dstCol, srcCol, selected);
    }
    dst.columns[innerColOffset].length - oldLen
}

// copySameOuterRows copies the continuous 'numRows' outer rows in the source Chunk.
/// 把源 Chunk 第 0 行的 outer 列连续复制 `numRows` 次到目标。
pub fn copySameOuterRows(
    outerColOffset: usize,
    outerColLen: usize,
    src: &Chunk,
    numRows: usize,
    dst: &mut Chunk,
) {
    if numRows == 0 || outerColLen == 0 {
        return;
    }
    let row = src.GetRow(0);
    let srcCols = &src.columns[outerColOffset..outerColOffset + outerColLen];
    for (i, srcCol) in srcCols.iter().enumerate() {
        let dstCol = &mut dst.columns[outerColOffset + i];
        dstCol.appendMultiSameNullBitmap(!srcCol.IsNull(row.idx), numRows);
        dstCol.length += numRows;
        if srcCol.IsFixed() {
            let elemLen = srcCol.elemBuf.len();
            let start = row.idx * elemLen;
            let end = start + numRows * elemLen;
            dstCol.data.extend_from_slice(&srcCol.data[start..end]);
        } else {
            let start = srcCol.offsets[row.idx] as usize;
            let end = srcCol.offsets[row.idx + numRows] as usize;
            dstCol.data.extend_from_slice(&srcCol.data[start..end]);
            let elemLen = srcCol.offsets[row.idx + 1] - srcCol.offsets[row.idx];
            for _ in 0..numRows {
                let next = dstCol.offsets.last().copied().unwrap_or(0) + elemLen;
                dstCol.offsets.push(next);
            }
        }
    }
}

// diskFileReaderWriter represents a Reader and a Writer for the temporary disk file.
/// 临时 spill 文件的读写封装，可叠加加密与 checksum。
pub struct diskFileReaderWriter {
    pub file: Option<os::File>,
    pub writer: Option<Box<dyn io::WriteCloser>>,
    // offWrite is the current offset for writing.
    pub offWrite: i64,
    pub checksumWriter: Option<checksum::Writer>,
    // cipherWriter is only enabled when SpilledFileEncryptionMethod is "aes128-ctr".
    pub cipherWriter: Option<encrypt::Writer>,
    // ctrCipher stores the key and nonce used by aes encrypt io layer.
    pub ctrCipher: Option<encrypt::CtrCipher>,
}

impl Default for diskFileReaderWriter {
    fn default() -> Self {
        diskFileReaderWriter {
            file: None,
            writer: None,
            offWrite: 0,
            checksumWriter: None,
            cipherWriter: None,
            ctrCipher: None,
        }
    }
}

impl diskFileReaderWriter {
    /// 创建临时文件并按配置包装加密/校验写路径。
    pub fn initWithFileName(&mut self, fileName: &str) -> Result<(), errors::Error> {
        // `os.CreateTemp` will insert random string so that a random file name will be generated.
        let file = os::CreateTemp(config::GetGlobalConfig().TempStoragePath, fileName)
            .map_err(errors::Trace)?;
        self.file = Some(file);

        let mut underlying: Box<dyn io::WriteCloser> =
            Box::new(self.file.as_ref().unwrap().clone());
        if config::GetGlobalConfig()
            .Security
            .SpilledFileEncryptionMethod
            != config::SpilledFileEncryptionMethodPlaintext
        {
            // Go 允许 plaintext/aes128-ctr；启用加密时先包 encrypt.Writer，再包 checksum.Writer。
            let ctr = encrypt::NewCtrCipher()?;
            self.cipherWriter = Some(encrypt::NewWriter(self.file.as_ref().unwrap(), &ctr));
            underlying = Box::new(self.cipherWriter.as_ref().unwrap().clone());
            self.ctrCipher = Some(ctr);
        }
        self.checksumWriter = Some(checksum::NewWriter(underlying));
        self.writer = Some(Box::new(self.checksumWriter.as_ref().unwrap().clone()));
        self.offWrite = 0;
        Ok(())
    }

    /// 构造带缓存的 ReaderAt，读路径与写路径包装层顺序一致。
    pub fn getReader(&self) -> Box<dyn io::ReaderAt> {
        let mut underlying: Box<dyn io::ReaderAt> = Box::new(self.file.as_ref().unwrap().clone());
        if let Some(ctr) = &self.ctrCipher {
            // 加密文件读取时复用 cipherWriter 的缓存和偏移，保持 Go 的读写包装层顺序。
            let cw = self.cipherWriter.as_ref().unwrap();
            underlying = Box::new(NewReaderWithCache(
                encrypt::NewReader(self.file.as_ref().unwrap(), ctr),
                cw.GetCache(),
                cw.GetCacheDataOffset(),
            ));
        }
        if let Some(checksum_writer) = &self.checksumWriter {
            underlying = Box::new(NewReaderWithCache(
                checksum::NewReader(underlying),
                checksum_writer.GetCache(),
                checksum_writer.GetCacheDataOffset(),
            ));
        }
        underlying
    }

    /// 返回从 `off` 到当前写偏移的区段读取器。
    pub fn getSectionReader(&self, off: i64) -> io::SectionReader {
        let checksumReader = self.getReader();
        io::NewSectionReader(checksumReader, off, self.offWrite - off)
    }

    /// 返回当前写入器。
    pub fn getWriter(&mut self) -> &mut dyn io::Writer {
        self.writer.as_mut().unwrap().as_mut()
    }

    /// 写入数据并推进 `offWrite`。
    pub fn write(&mut self, writeData: &[u8]) -> Result<usize, errors::Error> {
        let writeNum = self.writer.as_mut().unwrap().Write(writeData)?;
        self.offWrite += writeNum as i64;
        Ok(writeNum)
    }
}

// ColumnSwapHelper is used to help swap columns in a chunk.
/// 帮助在 Chunk 间按输入/输出列映射交换并修复列引用。
pub struct ColumnSwapHelper {
    // InputIdxToOutputIdxes maps the input column index to the output column indexes.
    pub InputIdxToOutputIdxes: HashMap<usize, Vec<usize>>,
    // Go 用 atomic.Pointer 延迟发布合并结果；保留“只初始化一次”的并发语义。
    pub mergedInputIdxToOutputIdxes: atomic::Pointer<HashMap<usize, Vec<usize>>>,
}

impl ColumnSwapHelper {
    /// 用输入列到输出列下标映射构造 helper。
    pub fn New(input_idx_to_output_idxes: HashMap<usize, Vec<usize>>) -> Self {
        Self {
            InputIdxToOutputIdxes: input_idx_to_output_idxes,
            mergedInputIdxToOutputIdxes: atomic::Pointer::new(std::ptr::null_mut()),
        }
    }

    // SwapColumns evaluates "Column" expressions and changes the content of the input Chunk.
    /// 按合并后的映射交换列，并让同组输出列互为引用。
    pub fn SwapColumns(&self, input: &mut Chunk, output: &mut Chunk) -> Result<(), errors::Error> {
        if self.mergedInputIdxToOutputIdxes.Load().is_null() {
            self.mergeInputIdxToOutputIdxes(input, &self.InputIdxToOutputIdxes);
        }
        let merged = unsafe { &*self.mergedInputIdxToOutputIdxes.Load() };
        for (inputIdx, outputIdxes) in merged {
            output.swapColumn(outputIdxes[0], input, *inputIdx)?;
            for i in 1..outputIdxes.len() {
                output.MakeRef(outputIdxes[0], outputIdxes[i]);
            }
        }
        Ok(())
    }

    // mergeInputIdxToOutputIdxes merges entries when column references are detected in input.
    // 这段逻辑对应 Go 中长注释的投影链场景：同源列多次引用时必须合并成同一个输入根。
    /// 检测 input 内同源列引用，合并映射后 CAS 发布（只初始化一次）。
    pub fn mergeInputIdxToOutputIdxes(
        &self,
        input: &Chunk,
        inputIdxToOutputIdxes: &HashMap<usize, Vec<usize>>,
    ) {
        let mut originalDJSet = disjointset::NewSet::<usize>(4);
        let mut flag = vec![false; input.NumCols()];

        // Detect self column-references inside the input chunk by comparing column addresses.
        for i in 0..input.NumCols() {
            if flag[i] {
                continue;
            }
            for j in i + 1..input.NumCols() {
                if input.Column(i).same_ref(input.Column(j)) {
                    flag[j] = true;
                    originalDJSet.Union(i, j);
                }
            }
        }

        let mut newInputIdxToOutputIdxes: HashMap<usize, Vec<usize>> =
            HashMap::with_capacity(inputIdxToOutputIdxes.len());
        for inputIdx in inputIdxToOutputIdxes.keys() {
            // Root idx is internal offset, not the right column index.
            let originalRootIdx = originalDJSet.FindRoot(*inputIdx);
            let (originalVal, ok) = originalDJSet.FindVal(originalRootIdx);
            intest::Assert(ok, &[]);
            let entry = newInputIdxToOutputIdxes.entry(originalVal).or_default();
            entry.extend_from_slice(&inputIdxToOutputIdxes[inputIdx]);
        }

        // CompareAndSwap 失败表示其他 worker 已经发布了合并结果，和 Go 行为一致无需覆盖。
        self.mergedInputIdxToOutputIdxes.CompareAndSwap(
            std::ptr::null_mut(),
            Box::into_raw(Box::new(newInputIdxToOutputIdxes)),
        );
    }
}

// NewColumnSwapHelper creates a new ColumnSwapHelper.
/// 由“输出下标 -> 输入下标”列表构建 `ColumnSwapHelper`。
pub fn NewColumnSwapHelper(usedColumnIndex: &[usize]) -> Box<ColumnSwapHelper> {
    let mut helper = Box::new(ColumnSwapHelper::New(HashMap::new()));
    for (outputIndex, inputIndex) in usedColumnIndex.iter().enumerate() {
        helper
            .InputIdxToOutputIdxes
            .entry(*inputIndex)
            .or_default()
            .push(outputIndex);
    }
    helper
}
