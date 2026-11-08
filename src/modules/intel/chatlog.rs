//! Reading EVE chat logs (`Documents\EVE\logs\Chatlogs\*.txt`).
//!
//! Chat logs are UTF-16 LE text with a byte order mark, one file per
//! channel per character per session. Each starts with a header:
//!
//! ```text
//!         ---------------------------------------------------------------
//!
//!           Channel ID:      -12345678
//!           Channel Name:    Branch Intel
//!           Listener:        Some Pilot
//!           Session started: 2026.11.08 14:00:00
//!         ---------------------------------------------------------------
//!
//! [ 2026.11.08 14:01:02 ] Other Pilot > X7R-JW Loki nv
//! [ 2026.11.08 14:03:10 ] EVE System > Channel changed to Local : X7R-JW
//! ```
//!
//! [`ChatLogReader`] takes raw bytes as they get appended to the file and
//! gives back whole lines. Like the game log reader it only ever sees
//! appended bytes, but UTF-16 needs extra care: every character is two
//! bytes (or four), so a read can stop in the middle of one. An odd byte at
//! the end is kept for the next read.

use crate::modules::mining::gamelog::LogTime;

/// The header of a chat log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLogHeader {
    /// "Local", "Branch Intel", "Corp", ... The channel is matched by this,
    /// not by the file name.
    pub channel: String,
    /// The character whose client wrote the file.
    pub listener: String,
    pub session_started: Option<LogTime>,
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub time: LogTime,
    pub sender: String,
    pub text: String,
}

/// The sender EVE uses for its own messages, like the system change line.
pub const EVE_SYSTEM: &str = "EVE System";

/// Turns appended bytes of one chat log into lines.
#[derive(Debug, Default)]
pub struct ChatLogReader {
    /// A trailing odd byte from the last push (half a UTF-16 unit).
    odd_byte: Option<u8>,
    /// Decoded text after the last complete line.
    partial: String,
    /// A high surrogate whose low half hasn't arrived yet (characters
    /// outside the Basic Multilingual Plane, like some emoji).
    high_surrogate: Option<u16>,
    header: Option<ChatLogHeader>,
    /// Header lines seen so far, until the header is complete.
    header_lines: Vec<String>,
}

impl ChatLogReader {
    pub fn new() -> ChatLogReader {
        ChatLogReader::default()
    }

    /// A reader for a file whose header was already read, continuing in
    /// the middle (only appended lines will be pushed).
    pub fn resume(header: ChatLogHeader) -> ChatLogReader {
        ChatLogReader {
            header: Some(header),
            ..ChatLogReader::default()
        }
    }

    pub fn header(&self) -> Option<&ChatLogHeader> {
        self.header.as_ref()
    }

    /// Feeds the next bytes of the file and returns the complete chat
    /// lines in them. Header lines are taken in until the header is known.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<ChatLine> {
        self.decode(bytes);

        let mut lines = Vec::new();
        // Take every complete line out of `partial`, keep the rest.
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if self.header.is_none() {
                self.take_header_line(line);
            } else if let Some(chat) = parse_line(line) {
                lines.push(chat);
            }
        }
        lines
    }

    /// Decodes UTF-16 LE into `partial`, keeping half characters for later.
    fn decode(&mut self, bytes: &[u8]) {
        let mut bytes = bytes.iter().copied();
        loop {
            let low = match self.odd_byte.take() {
                Some(byte) => byte,
                None => match bytes.next() {
                    Some(byte) => byte,
                    None => return,
                },
            };
            let Some(high) = bytes.next() else {
                self.odd_byte = Some(low);
                return;
            };
            let unit = u16::from_le_bytes([low, high]);
            if let Some(first) = self.high_surrogate.take() {
                // `decode_utf16` joins a surrogate pair into one character,
                // or gives an error for a broken pair.
                for c in char::decode_utf16([first, unit]) {
                    self.partial.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
                }
            } else if (0xD800..0xDC00).contains(&unit) {
                self.high_surrogate = Some(unit);
            } else {
                match char::from_u32(unit as u32) {
                    // The byte order mark at the start of the file.
                    Some('\u{feff}') => {}
                    Some(c) => self.partial.push(c),
                    None => self.partial.push(char::REPLACEMENT_CHARACTER),
                }
            }
        }
    }

    /// Collects header lines until the closing dashes after `Listener:`.
    fn take_header_line(&mut self, line: &str) {
        let trimmed = line.trim();
        // Lines that already look like chat mean there is no proper header.
        if trimmed.starts_with('[') {
            self.header_lines.clear();
            return;
        }
        self.header_lines.push(trimmed.to_string());
        if trimmed.starts_with("---") && self.header_lines.len() > 1 {
            self.header = parse_header(&self.header_lines);
            self.header_lines.clear();
        }
    }
}

/// Reads the `Channel Name:`, `Listener:` and `Session started:` lines.
fn parse_header(lines: &[String]) -> Option<ChatLogHeader> {
    let mut channel = None;
    let mut listener = None;
    let mut session_started = None;
    for line in lines {
        if let Some(value) = line.strip_prefix("Channel Name:") {
            channel = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Listener:") {
            listener = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Session started:") {
            session_started = LogTime::parse(value);
        }
    }
    Some(ChatLogHeader {
        channel: channel.filter(|c| !c.is_empty())?,
        listener: listener.filter(|l| !l.is_empty())?,
        session_started,
    })
}

/// Parses `[ 2026.11.08 14:01:02 ] Sender Name > text`.
pub fn parse_line(line: &str) -> Option<ChatLine> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (time, rest) = rest.split_once(']')?;
    let time = LogTime::parse(time)?;
    // The sender can contain spaces but not " > "; the text can.
    let (sender, text) = rest.split_once(" > ")?;
    let sender = sender.trim();
    if sender.is_empty() {
        return None;
    }
    Some(ChatLine {
        time,
        sender: sender.to_string(),
        text: text.trim().to_string(),
    })
}

/// If this is the line EVE writes into Local on every jump, the system the
/// character is now in.
pub fn local_system_change(line: &ChatLine) -> Option<&str> {
    if line.sender != EVE_SYSTEM {
        return None;
    }
    let system = line.text.strip_prefix("Channel changed to Local :")?.trim();
    // Some clients mark the system with a trailing "*"; it isn't part of the name.
    let system = system.trim_end_matches('*').trim();
    (!system.is_empty()).then_some(system)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A made-up chat log in EVE's layout. Names are invented.
    pub fn sample_text(channel: &str, listener: &str, body: &str) -> String {
        format!(
            "\u{feff}\r\n\r\n        ---------------------------------------------------------------\r\n\r\n          Channel ID:      -1234567\r\n          Channel Name:    {channel}\r\n          Listener:        {listener}\r\n          Session started: 2026.11.08 14:00:00\r\n        ---------------------------------------------------------------\r\n\r\n{body}"
        )
    }

    /// Encodes text as UTF-16 LE, the way EVE writes chat logs.
    pub fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    const BODY: &str = "[ 2026.11.08 14:01:02 ] Scout Pilot > X7R-JW Loki nv\r\n[ 2026.11.08 14:01:30 ] EVE System > Channel changed to Local : KV-8SN\r\n";

    #[test]
    fn reads_header_and_lines() {
        let mut reader = ChatLogReader::new();
        let lines = reader.push(&utf16(&sample_text("Branch Intel", "Pilot Alpha", BODY)));
        let header = reader.header().unwrap();
        assert_eq!(header.channel, "Branch Intel");
        assert_eq!(header.listener, "Pilot Alpha");
        assert!(header.session_started.is_some());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].sender, "Scout Pilot");
        assert_eq!(lines[0].text, "X7R-JW Loki nv");
        assert_eq!(local_system_change(&lines[1]), Some("KV-8SN"));
        assert_eq!(local_system_change(&lines[0]), None);
    }

    #[test]
    fn handles_reads_that_stop_mid_character() {
        let bytes = utf16(&sample_text("Branch Intel", "Pilot Alpha", BODY));
        let mut reader = ChatLogReader::new();
        let mut lines = Vec::new();
        // Feed it in odd-sized pieces, so many reads end on half a character.
        for chunk in bytes.chunks(7) {
            lines.extend(reader.push(chunk));
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "Channel changed to Local : KV-8SN");
    }

    #[test]
    fn keeps_an_unfinished_line_for_later() {
        let mut reader = ChatLogReader::new();
        let head = sample_text("Corp", "Pilot Alpha", "");
        reader.push(&utf16(&head));
        assert!(
            reader
                .push(&utf16("[ 2026.11.08 14:01:02 ] A > hel"))
                .is_empty()
        );
        let lines = reader.push(&utf16("lo\r\n"));
        assert_eq!(lines[0].text, "hello");
    }

    #[test]
    fn decodes_characters_outside_the_basic_plane() {
        let mut reader = ChatLogReader::resume(ChatLogHeader {
            channel: "Corp".into(),
            listener: "Pilot Alpha".into(),
            session_started: None,
        });
        let bytes = utf16("[ 2026.11.08 14:01:02 ] A > o7 \u{1F680}\r\n");
        let mut lines = Vec::new();
        for chunk in bytes.chunks(1) {
            lines.extend(reader.push(chunk));
        }
        assert_eq!(lines[0].text, "o7 \u{1F680}");
    }

    #[test]
    fn parses_senders_with_spaces_and_text_with_arrows() {
        let line = parse_line("[ 2026.11.08 14:01:02 ] Some Long Name > a > b").unwrap();
        assert_eq!(line.sender, "Some Long Name");
        assert_eq!(line.text, "a > b");
        assert!(parse_line("not a chat line").is_none());
        assert!(parse_line("[ 2026.11.08 14:01:02 ] no separator").is_none());
    }

    #[test]
    fn system_change_ignores_a_trailing_star() {
        let line = parse_line(
            "[ 2026.11.08 14:01:02 ] EVE System > Channel changed to Local : New Caldari*",
        )
        .unwrap();
        assert_eq!(local_system_change(&line), Some("New Caldari"));
    }
}
