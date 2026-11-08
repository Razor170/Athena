//! Alert sounds: the voice lines ("Enemy 3 jumps") or a beep.
//!
//! The voice lines are WAV files in a `sounds` folder next to Athena.exe.
//! CI makes them with the Windows built-in voice; the user can replace any
//! of them with a recording of their own under the same name. A missing
//! file plays the beep instead.
//!
//! Windows' `PlaySoundW` has no volume control, so the volume slider is
//! applied by scaling the samples of a copy of the file in memory and
//! playing that copy (`SND_MEMORY`). Only the alert gets quieter, not other
//! sounds on the PC. A loud alert ignores the slider and plays twice.
//!
//! Sounds play on their own background thread, one after the other and
//! synchronously, so the second play of a loud alert isn't cut off and the
//! UI thread never waits. When several alerts arrive while one plays, only
//! the closest of them plays next, so busy intel doesn't build a backlog.
//!
//! The file is read again for every alert (it's small, and this is the
//! background thread), so a replaced file is heard without a restart.
//! `PlaySoundW` trusts the sizes inside the file, so a file whose sizes
//! don't fit its length plays the beep instead.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use serde::{Deserialize, Serialize};

/// The alert range setting goes up to this many jumps, so there is a voice
/// file for each distance from 0 ("in system") to here.
pub const MAX_JUMPS: u32 = 10;

/// Which sound alerts play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundKind {
    Voice,
    Beep,
}

/// The file name of the voice line for a distance.
pub fn file_name(jumps: u32) -> String {
    match jumps {
        0 => "enemy-in-system.wav".to_string(),
        1 => "enemy-1-jump.wav".to_string(),
        n => format!("enemy-{n}-jumps.wav"),
    }
}

/// What the voice line for a distance says, for buttons and tooltips.
pub fn spoken_text(jumps: u32) -> String {
    match jumps {
        0 => "Enemy in system".to_string(),
        1 => "Enemy 1 jump".to_string(),
        n => format!("Enemy {n} jumps"),
    }
}

/// The `sounds` folder next to the exe.
pub fn sounds_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("sounds"))
}

/// Opens the sounds folder in Explorer, creating it first if it's missing.
/// Runs on a click; Explorer starts as its own program, so this returns at
/// once.
pub fn open_sounds_folder() -> Result<(), String> {
    let folder = sounds_folder().ok_or("Can't find the folder Athena runs from.")?;
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("Can't create {}: {e}", folder.display()))?;
    platform::open_folder(&folder)
}

/// One thing to play.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayRequest {
    pub jumps: u32,
    pub kind: SoundKind,
    /// 0.0 to 1.0, from the slider. Ignored when `loud`.
    pub volume: f32,
    /// Full volume, played twice.
    pub loud: bool,
}

/// Handle to the sound thread. Dropping it stops the thread.
pub struct SoundPlayer {
    send: Sender<PlayRequest>,
}

impl SoundPlayer {
    pub fn start() -> SoundPlayer {
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || play_loop(receive, sounds_folder()));
        SoundPlayer { send }
    }

    /// Queues a sound. Returns at once.
    pub fn play(&self, request: PlayRequest) {
        // If the thread is gone there's nothing to play with.
        let _ = self.send.send(request);
    }
}

/// The sound thread: plays requests one after the other until the module
/// drops its handle.
fn play_loop(receive: Receiver<PlayRequest>, folder: Option<PathBuf>) {
    while let Ok(first) = receive.recv() {
        // Everything else waiting: keep only the most urgent request.
        let mut request = first;
        while let Ok(next) = receive.try_recv() {
            request = more_urgent(request, next);
        }
        let times = if request.loud { 2 } else { 1 };
        let volume = if request.loud {
            1.0
        } else {
            request.volume.clamp(0.0, 1.0)
        };
        let wav = match (request.kind, &folder) {
            (SoundKind::Voice, Some(folder)) => read_wav(folder, request.jumps)
                .filter(|bytes| wav_sizes_fit(bytes))
                .map(|bytes| scale_wav(&bytes, volume).unwrap_or(bytes)),
            _ => None,
        };
        for _ in 0..times {
            match &wav {
                Some(bytes) => platform::play_wav(bytes),
                None => platform::beep(),
            }
        }
    }
}

/// Of two queued requests, the one to play: loud before quiet, then the
/// fewest jumps, then the newer one.
pub fn more_urgent(a: PlayRequest, b: PlayRequest) -> PlayRequest {
    if (a.loud, std::cmp::Reverse(a.jumps)) > (b.loud, std::cmp::Reverse(b.jumps)) {
        a
    } else {
        b
    }
}

/// True if the RIFF size and every chunk up to and including "data" fit
/// inside the file, so `PlaySoundW` can't read past the end of it.
pub fn wav_sizes_fit(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return false;
    }
    let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if riff_size.saturating_add(8) > bytes.len() {
        return false;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let end = (at + 8).saturating_add(size);
        if end > bytes.len() {
            return false;
        }
        if id == b"data" {
            return true;
        }
        at = end + (size % 2);
    }
    false
}

fn read_wav(folder: &Path, jumps: u32) -> Option<Vec<u8>> {
    std::fs::read(folder.join(file_name(jumps))).ok()
}

/// A copy of a 16-bit PCM WAV file with every sample multiplied by
/// `volume`. `None` if the file isn't a WAV we understand (it then plays
/// unchanged).
pub fn scale_wav(bytes: &[u8], volume: f32) -> Option<Vec<u8>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut out = bytes.to_vec();
    let mut pcm16 = false;
    let mut at = 12;
    // A WAV file is a list of chunks: a 4-letter id, a 4-byte length, the
    // data, and a padding byte if the length is odd.
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let start = at + 8;
        let end = start.checked_add(size)?.min(bytes.len());
        if id == b"fmt " && end - start >= 16 {
            let format = u16::from_le_bytes([bytes[start], bytes[start + 1]]);
            let bits = u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]);
            // 1 = plain PCM.
            pcm16 = format == 1 && bits == 16;
        } else if id == b"data" {
            if !pcm16 {
                return None;
            }
            for sample in out[start..end].chunks_exact_mut(2) {
                let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 * volume;
                let scaled = value.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                sample.copy_from_slice(&scaled.to_le_bytes());
            }
            return Some(out);
        }
        at = start + size + (size % 2);
    }
    None
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
    use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONEXCLAMATION;

    /// Plays a WAV file held in memory and returns when it has finished.
    pub fn play_wav(bytes: &[u8]) {
        // SAFETY: with SND_MEMORY, PlaySoundW reads a complete WAV image
        // from the pointer. `bytes` stays alive until the call returns,
        // and SND_SYNC makes it return only after playing.
        unsafe {
            PlaySoundW(
                bytes.as_ptr() as *const u16,
                std::ptr::null_mut(),
                SND_MEMORY | SND_SYNC | SND_NODEFAULT,
            );
        }
    }

    pub fn open_folder(folder: &std::path::Path) -> Result<(), String> {
        std::process::Command::new("explorer")
            .arg(folder)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Can't open {}: {e}", folder.display()))
    }

    /// The Windows "exclamation" sound. MessageBeep returns at once, so
    /// wait a moment to let a second beep be heard as a second one.
    pub fn beep() {
        // SAFETY: MessageBeep takes a plain number and has no other effects.
        unsafe {
            MessageBeep(MB_ICONEXCLAMATION);
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
}

#[cfg(not(windows))]
mod platform {
    // Sounds are Windows-only; elsewhere (the Linux sandbox) they are silent.
    pub fn play_wav(_bytes: &[u8]) {}
    pub fn beep() {}
    pub fn open_folder(_folder: &std::path::Path) -> Result<(), String> {
        Err("Opening folders works on Windows only.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny 16-bit mono WAV with the given samples.
    fn wav(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&22050u32.to_le_bytes());
        out.extend_from_slice(&44100u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn samples(bytes: &[u8]) -> Vec<i16> {
        bytes[44..]
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect()
    }

    #[test]
    fn scales_samples_and_keeps_the_header() {
        let original = wav(&[1000, -1000, 32767, -32768, 0]);
        let half = scale_wav(&original, 0.5).unwrap();
        assert_eq!(&half[..44], &original[..44]);
        assert_eq!(samples(&half), [500, -500, 16384, -16384, 0]);
        assert_eq!(
            samples(&scale_wav(&original, 1.0).unwrap()),
            samples(&original)
        );
    }

    #[test]
    fn skips_other_chunks_before_the_data() {
        let mut bytes = wav(&[2000]);
        // Put a "LIST" chunk with an odd length (plus padding) before "data".
        let list = [b"LIST".as_slice(), &3u32.to_le_bytes(), b"abc\0"].concat();
        bytes.splice(36..36, list);
        let scaled = scale_wav(&bytes, 0.25).unwrap();
        let tail = &scaled[scaled.len() - 2..];
        assert_eq!(i16::from_le_bytes([tail[0], tail[1]]), 500);
    }

    #[test]
    fn other_files_are_left_alone() {
        assert!(scale_wav(b"not a wav file at all", 0.5).is_none());
        let mut eight_bit = wav(&[1, 2]);
        eight_bit[34] = 8; // bits per sample
        assert!(scale_wav(&eight_bit, 0.5).is_none());
    }

    #[test]
    fn sizes_must_fit_the_file() {
        let good = wav(&[1, 2, 3]);
        assert!(wav_sizes_fit(&good));
        // Cut short: the data chunk claims more than is there.
        assert!(!wav_sizes_fit(&good[..good.len() - 2]));
        // RIFF size too big.
        let mut big = good.clone();
        big[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert!(!wav_sizes_fit(&big));
        assert!(!wav_sizes_fit(b"RIFF"));
    }

    #[test]
    fn the_closest_queued_alert_plays() {
        let request = |jumps, loud| PlayRequest {
            jumps,
            kind: SoundKind::Voice,
            volume: 0.5,
            loud,
        };
        assert_eq!(more_urgent(request(4, false), request(2, false)).jumps, 2);
        assert_eq!(more_urgent(request(1, true), request(3, false)).jumps, 1);
        assert_eq!(more_urgent(request(3, false), request(1, true)).jumps, 1);
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name(0), "enemy-in-system.wav");
        assert_eq!(file_name(1), "enemy-1-jump.wav");
        assert_eq!(file_name(10), "enemy-10-jumps.wav");
        assert_eq!(spoken_text(3), "Enemy 3 jumps");
    }
}
