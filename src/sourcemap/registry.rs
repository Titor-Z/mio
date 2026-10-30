//! 源码映射注册表。
//!
//! 按文件路径登记转译产出的源码映射，错误上报时按路径查找并换算行列。
//!
//! 解码器为自研的最小 Source Map v3 实现（base64 VLQ），不依赖 `oxc_*`：
//! 依赖隔离约束下映射层不得引用转译层内部类型；且阶段 B 自研前端直接携带
//! 原始位置时，本文件退化为恒等映射，实现体量应当与其过渡角色相称。

use std::path::{Path, PathBuf};

/// base64 字符表（Source Map v3 的 VLQ 编码字母表）。
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 单条映射：一个生成坐标段对应到原始文件的行列（均为 0-based）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    generated_column: u32,
    source_line: u32,
    source_column: u32,
}

/// 解析 base64 VLQ 的一个字段；游标前进到该字段末尾。
///
/// VLQ：每字符 6 位，低位为延续位（bit5），拼接后 bit0 为符号位。
fn decode_vlq_field(bytes: &[u8], cursor: &mut usize) -> Option<i64> {
    let mut result: i64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(*cursor)?;
        *cursor += 1;
        let value = B64.iter().position(|item| *item == byte)? as i64;
        let continues = value & 32 != 0;
        result += (value & 31) << shift;
        shift += 5;
        if !continues {
            break;
        }
    }
    let negative = result & 1 != 0;
    result >>= 1;
    Some(if negative { -result } else { result })
}

/// 解码 `mappings` 字段：按生成行分组的段列表（累积状态跨行延续，列每行归零）。
fn decode_mappings(mappings: &str) -> Option<Vec<Vec<Segment>>> {
    let mut lines: Vec<Vec<Segment>> = Vec::new();
    let (mut src_idx, mut src_line, mut src_col): (i64, i64, i64) = (0, 0, 0);
    for line_str in mappings.split(';') {
        // 生成列每行归零；其余累积量跨行延续。
        let mut gen_col = 0i64;
        let mut segments = Vec::new();
        for seg_str in line_str.split(',') {
            if seg_str.is_empty() {
                continue;
            }
            let bytes = seg_str.as_bytes();
            let mut cursor = 0;
            let column = decode_vlq_field(bytes, &mut cursor)?;
            gen_col += column;
            // 长度 1 的段没有源信息，仅占位，跳过。
            if cursor < bytes.len() {
                let idx = decode_vlq_field(bytes, &mut cursor)?;
                let s_line = decode_vlq_field(bytes, &mut cursor)?;
                let s_col = decode_vlq_field(bytes, &mut cursor)?;
                // 第 5 字段（names）对本用途无用，读掉以保持游标语义完整。
                if cursor < bytes.len() {
                    decode_vlq_field(bytes, &mut cursor)?;
                }
                src_idx += idx;
                src_line += s_line;
                src_col += s_col;
                if src_idx >= 0 {
                    segments.push(Segment {
                        generated_column: gen_col.max(0) as u32,
                        source_line: src_line.max(0) as u32,
                        source_column: src_col.max(0) as u32,
                    });
                }
            }
        }
        lines.push(segments);
    }
    Some(lines)
}

/// 在指定生成行的段列表中找 `column` 的前驱段（段按生成列升序，规范保证）。
fn lookup_segment(lines: &[Vec<Segment>], gen_line: usize, column: u32) -> Option<Segment> {
    let mut best = None;
    for segment in lines.get(gen_line)? {
        if segment.generated_column <= column {
            best = Some(*segment);
        } else {
            break;
        }
    }
    best
}

/// 一份源码映射：记录 JavaScript 行列到 TypeScript 行列的换算关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMapEntry {
    /// 对应的 TypeScript 源文件路径。
    pub source_path: PathBuf,
    /// 源码映射原文（JSON 文本）。
    pub mapping: String,
}

/// 以文件路径为键的源码映射注册表。
#[derive(Debug, Default)]
pub struct SourceMapRegistry {
    entries: rustc_hash::FxHashMap<PathBuf, SourceMapEntry>,
}

impl SourceMapRegistry {
    /// 登记一个文件的源码映射。
    pub fn register(&mut self, generated_path: &Path, entry: SourceMapEntry) {
        self.entries.insert(generated_path.to_path_buf(), entry);
    }

    /// 查询某个产物文件对应的源码映射。
    pub fn get(&self, generated_path: &Path) -> Option<&SourceMapEntry> {
        self.entries.get(generated_path)
    }

    /// 当前登记的映射数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 把产物坐标（1-based 行列）换算回 TypeScript 坐标；查不到映射返回 `None`。
    ///
    /// 行列采用前驱匹配：即使类型擦除导致列漂移，落在同一映射段内的坐标
    /// 都会回到该段携带的原始位置。
    #[must_use]
    pub fn remap(&self, generated_path: &Path, line: u32, column: u32) -> Option<(u32, u32)> {
        let entry = self.entries.get(generated_path)?;
        let value: serde_json::Value = serde_json::from_str(&entry.mapping).ok()?;
        let mappings = value.get("mappings")?.as_str()?;
        let lines = decode_mappings(mappings)?;
        // 源码映射的行列为 0-based，栈帧为 1-based，在此换算。
        let segment = lookup_segment(
            &lines,
            usize::try_from(line.checked_sub(1)?).ok()?,
            column.saturating_sub(1),
        )?;
        Some((segment.source_line + 1, segment.source_column + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_get_returns_entry() {
        let mut registry = SourceMapRegistry::default();
        let generated = Path::new("/tmp/a.js");
        registry.register(
            generated,
            SourceMapEntry {
                source_path: PathBuf::from("/tmp/a.ts"),
                mapping: String::from("{}"),
            },
        );
        assert_eq!(
            registry
                .get(generated)
                .map(|entry| entry.source_path.clone()),
            Some(PathBuf::from("/tmp/a.ts"))
        );
    }

    /// 手工构造映射：第一行无段；第二行在生成列 8 处回到源 0:0。
    #[test]
    fn remap_handwritten_mappings() {
        let mut registry = SourceMapRegistry::default();
        registry.register(
            Path::new("/tmp/m.js"),
            SourceMapEntry {
                source_path: PathBuf::from("/tmp/m.ts"),
                mapping: String::from(r#"{"mappings":";QAAA"}"#),
            },
        );
        // 行 2 列 9（1-based）落在段 [8, ∞) 内 → 源 1:1（1-based）。
        assert_eq!(registry.remap(Path::new("/tmp/m.js"), 2, 9), Some((1, 1)));
        // 段起点之前（生成列 0-based 7 < 8）没有前驱段。
        assert_eq!(registry.remap(Path::new("/tmp/m.js"), 2, 8), None);
        // 未登记的路径与空行。
        assert_eq!(registry.remap(Path::new("/tmp/other.js"), 1, 1), None);
        assert_eq!(registry.remap(Path::new("/tmp/m.js"), 1, 1), None);
    }

    #[test]
    fn remap_multiple_segments_with_accumulators() {
        let mut registry = SourceMapRegistry::default();
        // 行 1（AAAA）：生成列 0 → 源 0:0。累积状态跨行延续。
        // 行 2（ACUA）：生成列 0 → srcIdx+1、srcLine+10、srcCol+0 → 源行 10。
        // 行 3（ECCG）：生成列 2 → srcLine+1、srcCol+3 → 源行 11、列 3。
        registry.register(
            Path::new("/tmp/acc.js"),
            SourceMapEntry {
                source_path: PathBuf::from("/tmp/acc.ts"),
                mapping: String::from(r#"{"mappings":"AAAA;ACUA;ECCG"}"#),
            },
        );
        // 行 2 任意列 → (11, 1)；段内前驱匹配：列 50 也回到同一源点。
        assert_eq!(
            registry.remap(Path::new("/tmp/acc.js"), 2, 1),
            Some((11, 1))
        );
        assert_eq!(
            registry.remap(Path::new("/tmp/acc.js"), 2, 50),
            Some((11, 1))
        );
        // 行 3：生成列 1（0-based）在段起点 2 之前 → 无前驱；列 3（0-based 2）命中。
        assert_eq!(registry.remap(Path::new("/tmp/acc.js"), 3, 2), None);
        assert_eq!(
            registry.remap(Path::new("/tmp/acc.js"), 3, 3),
            Some((12, 4))
        );
    }

    #[test]
    fn remap_invalid_json_is_none_not_panic() {
        let mut registry = SourceMapRegistry::default();
        registry.register(
            Path::new("/tmp/bad.js"),
            SourceMapEntry {
                source_path: PathBuf::from("/tmp/bad.ts"),
                mapping: String::from("not json"),
            },
        );
        assert_eq!(registry.remap(Path::new("/tmp/bad.js"), 1, 1), None);
    }

    /// golden：转译器真实产出的映射必须能把已知漂移坐标送回 TS 行列。
    ///
    /// 选取 interface/type 擦除导致行漂移的源码（全 ASCII，避免列单位歧义），
    /// 在产物文本里定位 `undefined_var` 的 JS 行列，断言 remap 回到其 TS 行列。
    #[test]
    fn remap_roundtrip_with_transpiler_output() {
        use crate::transpiler::{TranspileConfig, Transpiler};

        let source = "interface Box {\n  w: number;\n}\n\ntype Pad = number;\nconst pad: Pad = 1;\nundefined_var;\n";
        let path = Path::new("roundtrip.ts");
        let output = Transpiler::transpile(
            source,
            path,
            &TranspileConfig {
                sourcemap: true,
                ..TranspileConfig::default()
            },
        )
        .expect("转译应成功");
        let mapping = output.sourcemap.expect("应开启源码映射");

        // 在 JS 产物中定位目标 token 的 1-based 行列（ASCII 下字节偏移即列）。
        let byte_offset = output
            .code
            .find("undefined_var")
            .expect("产物应包含语句本体");
        let before = &output.code[..byte_offset];
        let js_line = before.matches('\n').count() as u32 + 1;
        let js_column = byte_offset as u32 - before.rfind('\n').map(|p| p + 1).unwrap_or(0) as u32;
        // 前置 7 行类型声明全部擦除，坐标必然漂移（测试有效性前提）。
        assert_ne!(js_line, 7, "构造应有行漂移");

        let mut registry = SourceMapRegistry::default();
        registry.register(
            path,
            SourceMapEntry {
                source_path: path.to_path_buf(),
                mapping,
            },
        );
        assert_eq!(
            registry.remap(path, js_line, js_column + 1),
            Some((7, 1)),
            "应映射回 TS 第 7 行列 1"
        );
    }
}
