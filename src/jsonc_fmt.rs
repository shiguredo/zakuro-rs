//! JSONC 整形 (jcfmt 相当の comment-aware formatter)
//!
//! nojson の `parse_jsonc` が返す comment ranges を辿り、空白だけを整える。
//! インデントは 2 スペース固定。コメント・trailing comma・空行は保持する。

use std::collections::BTreeMap;
use std::ops::Range;

use nojson::{JsonParseError, JsonValueKind, RawJson, RawJsonValue};

/// インデント幅 (スペース数)。常に 2 を使う。
const INDENT_SIZE: usize = 2;

/// JSONC 文字列を整形する
pub(crate) fn format_jsonc(text: &str) -> Result<String, JsonParseError> {
    let (json, comment_ranges) = RawJson::parse_jsonc(text)?;
    let mut output = String::new();
    let mut formatter = Formatter::new(text, comment_ranges, &mut output);
    formatter
        .format(json.value())
        .expect("パース成功後のフォーマット処理は失敗しない");
    Ok(output)
}

/// jcfmt (MIT, https://github.com/sile/jcfmt) と同系統のフォーマッタ
#[derive(Debug)]
struct Formatter<'a, W> {
    text: &'a str,
    comment_ranges: BTreeMap<usize, usize>,
    writer: W,
    level: usize,
    text_position: usize,
    multiline_mode: bool,
}

impl<'a, W: std::fmt::Write> Formatter<'a, W> {
    fn new(text: &'a str, comment_ranges: Vec<Range<usize>>, writer: W) -> Self {
        Self {
            text,
            comment_ranges: comment_ranges
                .into_iter()
                .map(|r| (r.start, r.end))
                .collect(),
            writer,
            level: 0,
            text_position: 0,
            multiline_mode: false,
        }
    }

    fn format(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        self.multiline_mode = self.is_newline_needed(value);
        self.format_value(value)?;
        self.format_comments(self.text.len())?;
        writeln!(self.writer)?;
        Ok(())
    }

    fn format_value(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        if self.multiline_mode {
            self.format_comments(value.position())?;
            self.indent(value.position())?;
        }
        self.format_value_content(value)?;
        Ok(())
    }

    fn format_member_value(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        if self.contains_comment(value.position()) {
            self.format_comments(value.position())?;
            self.indent(value.position())?;
        } else {
            write!(self.writer, " ")?;
        }
        self.format_value_content(value)?;
        Ok(())
    }

    fn format_value_content(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        match value.kind() {
            JsonValueKind::Null
            | JsonValueKind::Boolean
            | JsonValueKind::Integer
            | JsonValueKind::Float
            | JsonValueKind::String => write!(self.writer, "{}", value.as_raw_str())?,
            JsonValueKind::Array => self.format_array(value)?,
            JsonValueKind::Object => self.format_object(value)?,
        }
        self.text_position = value.position() + value.as_raw_str().len();
        Ok(())
    }

    fn has_trailing_comma(&self, close_position: usize) -> bool {
        let Some(mut position) = self.text[self.text_position..close_position].find(',') else {
            return false;
        };
        position += self.text_position;
        while self
            .comment_ranges
            .range(..position)
            .next_back()
            .is_some_and(|(_, &comment_end)| position < comment_end)
        {
            position += 1;
            let Some(offset) = self.text[position..close_position].find(',') else {
                return false;
            };
            position += offset;
        }
        true
    }

    fn format_symbol(&mut self, ch: char) -> std::fmt::Result {
        let mut position =
            self.text_position + self.text[self.text_position..].find(ch).expect("bug") + 1;
        while self
            .comment_ranges
            .range(..position)
            .next_back()
            .is_some_and(|(_, &end)| position < end)
        {
            position += self.text[position..].find(ch).expect("bug") + 1;
        }

        if (self.multiline_mode && matches!(ch, ']' | '}')) || self.contains_comment(position) {
            self.format_comments(position)?;
            if matches!(ch, ']' | '}') {
                self.text_position = position - 1;
            }
            self.indent(position)?;
        }

        write!(self.writer, "{ch}")?;
        self.text_position = position;
        Ok(())
    }

    fn contains_comment(&self, position: usize) -> bool {
        self.comment_ranges.range(..position).next().is_some()
    }

    fn format_comments(&mut self, position: usize) -> std::fmt::Result {
        self.format_trailing_comment(position)?;
        self.format_leading_comment(position)?;
        Ok(())
    }

    fn format_leading_comment(&mut self, position: usize) -> std::fmt::Result {
        loop {
            let Some((comment_start, comment_end)) = self
                .comment_ranges
                .range(..position)
                .next()
                .map(|x| (*x.0, *x.1))
            else {
                return Ok(());
            };

            self.indent(comment_start)?;
            self.text_position = comment_start;
            let comment = &self.text[comment_start..comment_end];
            if comment.starts_with("//") {
                write!(self.writer, "{}", comment.trim_end())?;
            } else {
                let after_indent = self.level * INDENT_SIZE;
                let before_indent = self.text[..comment_start]
                    .lines()
                    .next_back()
                    .expect("bug")
                    .len();
                for (i, mut line) in comment.lines().enumerate() {
                    if i == 0 {
                        write!(self.writer, "{}", line.trim())?;
                    } else if let Some(delta) = after_indent.checked_sub(before_indent) {
                        write!(
                            self.writer,
                            "\n{:width$}{}",
                            "",
                            line.trim_end(),
                            width = delta
                        )?;
                    } else {
                        let delta = before_indent - after_indent;
                        for _ in 0..delta {
                            if let Some(l) = line.strip_prefix(' ') {
                                line = l;
                            } else {
                                break;
                            };
                        }
                        write!(self.writer, "\n{}", line.trim_end())?;
                    }
                }
            }
            self.comment_ranges.remove(&comment_start);
            self.text_position = comment_end;
        }
    }

    fn format_trailing_comment(&mut self, next_position: usize) -> std::fmt::Result {
        if self.text_position == 0 {
            return Ok(());
        };
        loop {
            let Some((comment_start, comment_end)) = self
                .comment_ranges
                .range(self.text_position..next_position)
                .next()
                .map(|x| (*x.0, *x.1))
            else {
                return Ok(());
            };
            if self.text[self.text_position..comment_end].contains('\n') {
                return Ok(());
            }

            let comment = self.text[comment_start..comment_end].trim_end();
            write!(self.writer, " {comment}")?;
            self.comment_ranges.remove(&comment_start);
            self.text_position = comment_end;
        }
    }

    fn format_array(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        self.format_symbol('[')?;
        self.level += 1;

        let old_multiline_mode = self.multiline_mode;
        self.multiline_mode = self.is_newline_needed(value);
        for (i, element) in value.to_array().expect("bug").enumerate() {
            if i > 0 {
                self.format_symbol(',')?;
                if !self.multiline_mode {
                    write!(self.writer, " ")?;
                }
            }
            self.format_value(element)?;
        }
        let close_position = value.position() + value.as_raw_str().len();
        if self.has_trailing_comma(close_position) {
            self.format_symbol(',')?;
        }
        self.format_comments(close_position)?;

        self.level -= 1;
        self.format_symbol(']')?;
        self.multiline_mode = old_multiline_mode;
        Ok(())
    }

    fn format_object(&mut self, value: RawJsonValue<'_, '_>) -> std::fmt::Result {
        self.format_symbol('{')?;
        self.level += 1;

        let old_multiline_mode = self.multiline_mode;
        self.multiline_mode = self.is_newline_needed(value);
        for (i, (key, member_value)) in value.to_object().expect("bug").enumerate() {
            if i > 0 {
                self.format_symbol(',')?;
                if !self.multiline_mode {
                    write!(self.writer, " ")?;
                }
            }

            self.format_value(key)?;
            self.format_symbol(':')?;
            self.format_member_value(member_value)?;
        }
        let close_position = value.position() + value.as_raw_str().len();
        if self.has_trailing_comma(close_position) {
            self.format_symbol(',')?;
        }
        self.format_comments(close_position)?;

        self.level -= 1;
        self.format_symbol('}')?;
        self.multiline_mode = old_multiline_mode;
        Ok(())
    }

    fn is_newline_needed(&self, value: RawJsonValue<'_, '_>) -> bool {
        self.is_comment_included(value) || self.is_newline_included(value)
    }

    fn is_comment_included(&self, value: RawJsonValue<'_, '_>) -> bool {
        let start = value.position();
        let end = start + value.as_raw_str().len();
        self.comment_ranges.range(start..end).next().is_some()
    }

    fn is_newline_included(&self, value: RawJsonValue<'_, '_>) -> bool {
        let start = value.position();
        let end = start + value.as_raw_str().len();
        self.text[start..end].contains('\n')
    }

    fn blank_line(&mut self, position: usize) -> std::fmt::Result {
        let Some(offset) = self.text[self.text_position..position].find('\n') else {
            return Ok(());
        };
        self.text_position += offset + 1;

        let Some(offset) = self.text[self.text_position..position].find('\n') else {
            return Ok(());
        };
        self.text_position += offset + 1;

        writeln!(self.writer)?;

        Ok(())
    }

    fn indent(&mut self, position: usize) -> std::fmt::Result {
        if self.text_position == 0 {
            return Ok(());
        }
        self.blank_line(position)?;
        write!(
            self.writer,
            "\n{:width$}",
            "",
            width = self.level * INDENT_SIZE
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 入力 JSONC と整形結果をスナップショット比較用に連結する
    fn render(input: &str) -> String {
        let output = format_jsonc(input).expect("有効な JSONC は整形できること");
        format!("--- input ---\n{input}--- output ---\n{output}")
    }

    /// 多段ネストは 2 スペースインデントになること
    #[test]
    fn indentation_is_two_spaces() {
        let input = r#"{
"level1": {
    "level2": {
      "level3": "value"
    }
  }
}"#;
        insta::assert_snapshot!(render(input));
    }

    /// trailing comma が保持されること
    #[test]
    fn trailing_commas() {
        let input = r#"{
  "key1": "value1", // Comment after value
  "key2": "value2", // Another comment
  // Final comment before trailing comma
}"#;
        insta::assert_snapshot!(render(input));
    }

    /// コメント (行・ブロック・trailing) が保持されること
    #[test]
    fn comments_mixed() {
        let input = r#"{
  // Comment before key
  "key1": "value1", // Trailing comment
  /* Block comment */
  "key2": "value2"
}"#;
        insta::assert_snapshot!(render(input));
    }

    /// 1 行 JSON はコンパクトに整形されること
    #[test]
    fn compact_arrays_and_objects() {
        let input = r#"{"a": 1, "b": 2}"#;
        insta::assert_snapshot!(render(input));
    }

    /// 連続空行は 1 行の空行に正規化されること
    #[test]
    fn whitespace_normalization() {
        let input = r#"{

  "key"   :    "value"   ,


  "another"  :   42


}"#;
        insta::assert_snapshot!(render(input));
    }
}
