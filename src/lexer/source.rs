//! 源码文本与位置索引。
//!
//! 词法分析阶段所有 token 的文本都以切片形式指向这里，避免逐 token 拷贝。
//! 行列号在构造时一次性建成索引，之后每次查询 O(log n)，
//! 保证错误报告与源码映射都能廉价地拿到人类可读位置。

/// token 或子串在源码中的字节区间，左闭右开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    /// 起始字节偏移。
    pub start: u32,
    /// 结束字节偏移。
    pub end: u32,
}

impl Span {
    /// 构造一个区间。
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// 区间字节长度。
    pub const fn len(&self) -> u32 {
        self.end - self.start
    }

    /// 空区间判定，与 `len` 成对出现。
    pub const fn is_empty(&self) -> bool {
        self.end == self.start
    }

    /// 合并两个区间为覆盖两者的最小区间，供表达式节点构造整体跨度。
    pub fn merge(&self, other: &Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// 一份待分析的源码文本及其行索引。
#[derive(Debug, Clone)]
pub struct Source {
    /// 完整源码文本。
    text: String,
    /// 每一行起始处的字节偏移，首元素恒为 0。
    line_starts: Vec<u32>,
}

impl Source {
    /// 建索引并持有源码。
    pub fn new(text: String) -> Self {
        let mut line_starts = vec![0u32];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push((index + 1) as u32);
            }
        }
        Self { text, line_starts }
    }

    /// 源码全文。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 源码字节数。
    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    /// 空源码判定，与 `len` 成对。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 按字节区间取出原始文本。
    ///
    /// 区间边界由词法器在字符边界上产生，因此不会切断多字节字符。
    pub fn slice(&self, span: Span) -> &str {
        &self.text[span.start as usize..span.end as usize]
    }

    /// 把字节偏移换算为 1 基的行号与列号，列号按字符计而非按字节计。
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let index = self
            .line_starts
            .binary_search(&offset)
            .unwrap_or_else(|found| found.saturating_sub(1));
        let line_start = self.line_starts[index];
        let line_number = (index + 1) as u32;
        let column = self.text[line_start as usize..offset as usize]
            .chars()
            .count() as u32;
        (line_number, column + 1)
    }

    /// 取出某一行的完整文本，不含行尾换行符。
    pub fn line_text(&self, line_number: u32) -> &str {
        let index = (line_number as usize).saturating_sub(1);
        let start = self.line_starts.get(index).copied().unwrap_or(0) as usize;
        let end = self
            .line_starts
            .get(index + 1)
            .map(|next| *next as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\r', '\n'])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_returns_span_text() {
        let source = Source::new(String::from("const a = 1;"));
        assert_eq!(source.slice(Span::new(6, 7)), "a");
    }

    #[test]
    fn line_col_counts_one_based() {
        let source = Source::new(String::from("ab\ncde\nf"));
        assert_eq!(source.line_col(0), (1, 1));
        assert_eq!(source.line_col(3), (2, 1));
        assert_eq!(source.line_col(5), (2, 3));
        assert_eq!(source.line_col(7), (3, 1));
    }

    #[test]
    fn line_col_counts_characters_not_bytes() {
        // 非 ASCII 字符占多字节，列号必须按字符计
        let source = Source::new(String::from("let \u{03A9} = 1;"));
        // 等号在第 7 字节，但 Ω 占两字节，它是第 7 列（按字节计会得第 8 列）
        assert_eq!(source.line_col(7), (1, 7));
    }

    #[test]
    fn line_text_strips_newline() {
        let source = Source::new(String::from("first\r\nsecond"));
        assert_eq!(source.line_text(1), "first");
        assert_eq!(source.line_text(2), "second");
    }

    #[test]
    fn span_merge_covers_both() {
        let merged = Span::new(3, 6).merge(&Span::new(9, 12));
        assert_eq!(merged, Span::new(3, 12));
    }
}
