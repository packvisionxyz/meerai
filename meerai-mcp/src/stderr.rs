use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;

pub const MAX_TAIL_LINES: usize = 200;
pub const MAX_TAIL_CHARS: usize = 8 * 1024;

#[derive(Clone, Debug, Default)]
pub struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl StderrTail {
    pub fn spawn(stream: ChildStderr) -> Self {
        let tail = Self::default();
        let sink = tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                sink.push(line);
            }
        });
        tail
    }

    pub fn push(&self, line: String) {
        let mut lines = self.lock();
        lines.push_back(line);
        while lines.len() > MAX_TAIL_LINES {
            lines.pop_front();
        }
        while lines.len() > 1 && total_chars(&lines) > MAX_TAIL_CHARS {
            lines.pop_front();
        }
    }

    pub fn snapshot(&self) -> Option<String> {
        let lines = self.lock();
        if lines.is_empty() {
            return None;
        }
        Some(lines.iter().cloned().collect::<Vec<_>>().join("\n"))
    }

    pub fn suffix(&self) -> String {
        match self.snapshot() {
            Some(tail) => format!("\n--- stderr (last {MAX_TAIL_LINES} lines) ---\n{tail}"),
            None => String::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<String>> {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn total_chars(lines: &VecDeque<String>) -> usize {
    lines.iter().map(|line| line.chars().count() + 1).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_is_empty_before_any_line() {
        assert!(StderrTail::default().snapshot().is_none());
    }

    #[test]
    fn a_tail_keeps_the_newest_lines_in_order() {
        let tail = StderrTail::default();
        for index in 0..(MAX_TAIL_LINES + 5) {
            tail.push(format!("line {index}"));
        }
        let snapshot = tail.snapshot().expect("snapshot");
        let lines: Vec<&str> = snapshot.lines().collect();
        assert_eq!(lines.len(), MAX_TAIL_LINES);
        assert_eq!(lines[0], "line 5");
        assert_eq!(
            lines[MAX_TAIL_LINES - 1],
            format!("line {}", MAX_TAIL_LINES + 4)
        );
    }

    #[test]
    fn a_tail_drops_the_oldest_lines_over_the_char_cap() {
        let tail = StderrTail::default();
        for _ in 0..40 {
            tail.push("x".repeat(1_024));
        }
        let snapshot = tail.snapshot().expect("snapshot");
        assert!(snapshot.chars().count() <= MAX_TAIL_CHARS + 1_024);
        assert!(snapshot.lines().count() <= MAX_TAIL_LINES);
    }
}
