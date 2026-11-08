//! One report, many files: EVE writes a separate chat log for each of your
//! characters in a channel, so with four pilots in "Branch Intel" every
//! message arrives four times. Someone pasting the same report into two
//! watched channels gives the same message twice too.
//!
//! [`Deduper`] lets the first copy through and drops the others: same
//! sender and same text within [`WINDOW_SECONDS`] of each other, whatever
//! the channel.

use std::collections::VecDeque;

use crate::modules::mining::gamelog::LogTime;

/// Copies further apart than this are separate reports. The logs of
/// different clients can differ by a second or two.
pub const WINDOW_SECONDS: i64 = 5;
/// How long messages are remembered. At startup each file is read in one
/// go, so a copy in the second pilot's file can come minutes "after" the
/// newest line of the first file; reports older than 10 minutes are
/// skipped at startup, so 15 minutes covers that.
const REMEMBER_SECONDS: i64 = 15 * 60;
/// And at most this many, in case a channel is very busy.
const REMEMBER_MAX: usize = 2000;

/// Remembers the recent messages.
#[derive(Debug, Default)]
pub struct Deduper {
    /// (time, sender, text) of recent messages that were let through.
    recent: VecDeque<(LogTime, String, String)>,
    /// The latest time seen, to forget old messages.
    newest: Option<LogTime>,
}

impl Deduper {
    pub fn new() -> Deduper {
        Deduper::default()
    }

    /// True if this message is new, false if it's a copy of one seen
    /// within a few seconds.
    pub fn is_new(&mut self, time: LogTime, sender: &str, text: &str) -> bool {
        let newest = self.newest.map_or(time, |n| n.max(time));
        self.newest = Some(newest);
        // Forget what can't match anymore. The list is in arrival order,
        // which is nearly time order, so the oldest are at the front.
        while let Some((oldest, _, _)) = self.recent.front() {
            if newest.seconds_since(*oldest) > REMEMBER_SECONDS || self.recent.len() > REMEMBER_MAX
            {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        let copy = self.recent.iter().any(|(seen, seen_sender, seen_text)| {
            seen_sender == sender
                && seen_text == text
                && time.seconds_since(*seen).abs() <= WINDOW_SECONDS
        });
        if !copy {
            self.recent
                .push_back((time, sender.to_string(), text.to_string()));
        }
        !copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> LogTime {
        LogTime {
            unix_seconds: 1_800_000_000 + seconds,
        }
    }

    #[test]
    fn copies_from_other_files_are_dropped() {
        let mut d = Deduper::new();
        assert!(d.is_new(at(0), "Scout", "X7R-JW loki"));
        assert!(!d.is_new(at(0), "Scout", "X7R-JW loki"));
        // Another client's log a second later, or a little earlier.
        assert!(!d.is_new(at(1), "Scout", "X7R-JW loki"));
        assert!(!d.is_new(at(-1), "Scout", "X7R-JW loki"));
    }

    #[test]
    fn different_sender_text_or_time_is_new() {
        let mut d = Deduper::new();
        assert!(d.is_new(at(0), "Scout", "X7R-JW loki"));
        assert!(d.is_new(at(0), "Other", "X7R-JW loki"));
        assert!(d.is_new(at(0), "Scout", "X7R-JW loki +1"));
        // The same report again a minute later is a new report.
        assert!(d.is_new(at(60), "Scout", "X7R-JW loki"));
    }

    #[test]
    fn whole_files_read_one_after_the_other() {
        // At startup the first pilot's file is read to its end before the
        // second pilot's file starts at the top.
        let mut d = Deduper::new();
        for t in [-400, -200, -30] {
            assert!(d.is_new(at(t), "Scout", &format!("report {t}")));
        }
        for t in [-400, -200, -30] {
            assert!(!d.is_new(at(t), "Scout", &format!("report {t}")));
        }
    }

    #[test]
    fn old_messages_are_forgotten() {
        let mut d = Deduper::new();
        for i in 0..1000 {
            d.is_new(at(i * 10), "Scout", &format!("message {i}"));
        }
        assert!(d.recent.len() <= 91);
    }
}
