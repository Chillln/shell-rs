//! 根据提示符边界和已经回显的输入匹配历史，不记录按键或密码。

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::{TermMode, cell::Flags};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher as _;
use unicode_width::UnicodeWidthChar as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PromptMark {
    Ready,
    Inactive,
}

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

/// 只保留短协议头，长日志和未结束的 OSC 不会积累到内存中。
#[derive(Default)]
pub(super) struct PromptScanner {
    state: State,
    body: Vec<u8>,
    oversized: bool,
}

impl PromptScanner {
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<(usize, PromptMark)> {
        let mut marks = Vec::new();
        for (index, &byte) in bytes.iter().enumerate() {
            match self.state {
                State::Ground if byte == 0x1b => self.state = State::Escape,
                State::Escape => match byte {
                    b']' => {
                        self.body.clear();
                        self.oversized = false;
                        self.state = State::Osc;
                    }
                    b'P' | b'_' | b'^' | b'X' => self.state = State::String,
                    0x1b => {}
                    _ => self.state = State::Ground,
                },
                State::Osc => match byte {
                    7 => self.finish(index + 1, &mut marks),
                    0x1b => self.state = State::OscEscape,
                    0x18 | 0x1a => self.state = State::Ground,
                    _ if self.body.len() < 64 => self.body.push(byte),
                    _ => self.oversized = true,
                },
                State::OscEscape => {
                    if byte == b'\\' {
                        self.finish(index + 1, &mut marks);
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::String if byte == 0x1b => self.state = State::StringEscape,
                State::String if matches!(byte, 0x18 | 0x1a) => self.state = State::Ground,
                State::StringEscape => {
                    self.state = if byte == b'\\' {
                        State::Ground
                    } else {
                        State::String
                    }
                }
                _ => {}
            }
        }
        marks
    }

    fn finish(&mut self, end: usize, marks: &mut Vec<(usize, PromptMark)>) {
        self.state = State::Ground;
        let mut params = self.body.split(|b| *b == b';');
        if params.next() != Some(&b"133"[..]) {
            return;
        }
        let mark = match params.next() {
            Some(b"B") if !self.oversized => PromptMark::Ready,
            Some(b"B") => PromptMark::Inactive,
            Some(b"A" | b"C" | b"D") => PromptMark::Inactive,
            _ => return,
        };
        marks.push((end, mark));
    }
}

#[derive(Default)]
pub(super) struct PromptState {
    anchor: Option<Anchor>,
}

struct Anchor {
    point: Point,
    text: String,
    expected_column: usize,
    expected: Vec<(usize, u64)>,
    hashes: RandomState,
}

impl PromptState {
    pub fn clear(&mut self) {
        self.anchor = None;
    }

    pub fn mark<T: EventListener>(&mut self, mark: PromptMark, term: &Term<T>) {
        self.anchor = match mark {
            PromptMark::Ready if !term.mode().contains(TermMode::ALT_SCREEN) => {
                let point = term.grid().cursor.point;
                Some(Anchor {
                    point,
                    text: row_text(term, point, 0, point.column.0),
                    expected_column: point.column.0,
                    expected: Vec::new(),
                    hashes: RandomState::new(),
                })
            }
            _ => None,
        };
    }

    /// 只保留列宽和临时摘要；全部回显且文字匹配后才允许采纳建议。
    pub fn input(&mut self, bytes: &[u8]) {
        let Some(anchor) = self.anchor.as_mut() else {
            return;
        };
        if bytes == [8] || bytes == [127] {
            if let Some((width, _)) = anchor.expected.pop() {
                anchor.expected_column = anchor.expected_column.saturating_sub(width);
            } else {
                self.clear();
            }
        } else if bytes == [21] {
            anchor.expected.clear();
            anchor.expected_column = anchor.point.column.0;
        } else if let Ok(text) = std::str::from_utf8(bytes) {
            if text.chars().any(char::is_control) {
                self.clear();
                return;
            }
            for character in text.chars() {
                let width = character.width().unwrap_or(0);
                anchor
                    .expected
                    .push((width, anchor.hashes.hash_one(character)));
                anchor.expected_column = anchor.expected_column.saturating_add(width);
            }
            if anchor.expected.len() > 4096 {
                self.clear();
            }
        } else {
            self.clear();
        }
    }

    pub fn prefix<T: EventListener>(&self, term: &Term<T>) -> Option<String> {
        let anchor = self.anchor.as_ref()?;
        let point = anchor.point;
        let cursor = term.grid().cursor.point;
        if term.mode().intersects(TermMode::ALT_SCREEN | TermMode::VI)
            || term.grid().display_offset() != 0
            || cursor.line != point.line
            || cursor.column <= point.column
            || cursor.column.0 != anchor.expected_column
            || row_text(term, point, 0, point.column.0) != anchor.text
        {
            return None;
        }
        // 光标后有文字或 shell 自带的灰字补全时不接管右方向键。
        let row = &term.grid()[cursor.line];
        if (cursor.column.0..row.len()).any(|column| row[Column(column)].c != ' ') {
            return None;
        }
        let prefix = row_text(term, cursor, point.column.0, cursor.column.0);
        if !prefix
            .chars()
            .map(|ch| anchor.hashes.hash_one(ch))
            .eq(anchor.expected.iter().map(|(_, hash)| *hash))
        {
            return None;
        }
        (!prefix.trim().is_empty() && !prefix.chars().any(char::is_control)).then_some(prefix)
    }
}

fn row_text<T: EventListener>(term: &Term<T>, point: Point, from: usize, to: usize) -> String {
    let mut text = String::new();
    for column in from..to {
        let cell = &term.grid()[Point::new(point.line, Column(column))];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        text.push(cell.c);
        if let Some(marks) = cell.zerowidth() {
            text.extend(marks);
        }
    }
    text
}

pub(super) fn suffix<'a>(prefix: &str, history: &'a [String]) -> Option<&'a str> {
    if prefix.trim().is_empty() {
        return None;
    }
    history
        .iter()
        .filter(|command| command.len() <= 4096 && !command.chars().any(char::is_control))
        .find_map(|command| command.strip_prefix(prefix).filter(|rest| !rest.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::Event;
    use alacritty_terminal::vte::ansi::Processor;
    #[derive(Clone)]
    struct Sink;
    impl EventListener for Sink {
        fn send_event(&self, _: Event) {}
    }

    #[test]
    fn waits_for_matching_echo_and_stops_after_submit_or_fullscreen() {
        let mut term = Term::new(
            Default::default(),
            &super::super::TerminalSize::DEFAULT,
            Sink,
        );
        let mut processor: Processor = Processor::new();
        let mut state = PromptState::default();
        processor.advance(&mut term, b"$ ");
        state.mark(PromptMark::Ready, &term);
        state.input(b"ls");
        processor.advance(&mut term, b"l");
        assert_eq!(state.prefix(&term), None);
        processor.advance(&mut term, b"s");
        assert_eq!(state.prefix(&term), Some("ls".into()));
        state.input(&[127]);
        state.input(b"t");
        assert_eq!(state.prefix(&term), None);
        processor.advance(&mut term, b"\x08 \x08t");
        assert_eq!(state.prefix(&term), Some("lt".into()));
        processor.advance(&mut term, b"\x1b[?1049h");
        assert_eq!(state.prefix(&term), None);
        processor.advance(&mut term, b"\x1b[?1049l");
        state.input(b"\r");
        assert_eq!(state.prefix(&term), None);
        processor.advance(&mut term, b"\r\nPassword: ");
        state.input(b"secret");
        assert_eq!(state.prefix(&term), None);
    }

    #[test]
    fn unicode_echo_and_native_suggestions_do_not_take_over_cursor_keys() {
        let mut term = Term::new(
            Default::default(),
            &super::super::TerminalSize::DEFAULT,
            Sink,
        );
        let mut processor: Processor = Processor::new();
        let mut state = PromptState::default();
        processor.advance(&mut term, b"$ ");
        state.mark(PromptMark::Ready, &term);
        state.input("echo 中文".as_bytes());
        processor.advance(&mut term, "echo 中文".as_bytes());
        assert_eq!(state.prefix(&term), Some("echo 中文".into()));
        processor.advance(&mut term, b" file\x1b[5D");
        assert_eq!(state.prefix(&term), None);
    }
    #[test]
    fn markers_survive_chunks_and_ignore_other_protocol_strings() {
        let mut scanner = PromptScanner::default();
        assert!(scanner.scan(b"\x1b]133;").is_empty());
        assert_eq!(
            scanner.scan(b"B\x1b\\ls\x1b]133;C\x07"),
            vec![(3, PromptMark::Ready), (13, PromptMark::Inactive)]
        );
        assert!(scanner.scan(b"\x1bP\x1b]133;B\x07\x1b\\").is_empty());
        let oversized = format!("\x1b]133;B;{}\x07", "x".repeat(10000));
        assert_eq!(
            scanner.scan(oversized.as_bytes()),
            vec![(oversized.len(), PromptMark::Inactive)]
        );
        assert_eq!(
            scanner.scan(b"\x1b]133;D;0\x07"),
            vec![(10, PromptMark::Inactive)]
        );
    }
    #[test]
    fn history_matches_exact_prefix_and_skips_multiline_or_control_characters() {
        let history = vec![
            "ls\nrm -rf /tmp/test".into(),
            "ls -a\x1b".into(),
            "ls -lh 中文".into(),
            "ls -l".into(),
        ];
        assert_eq!(suffix("ls -l", &history), Some("h 中文"));
        assert_eq!(suffix("LS", &history), None);
        assert_eq!(suffix("ls -lh 中文", &history), None);
    }
}
