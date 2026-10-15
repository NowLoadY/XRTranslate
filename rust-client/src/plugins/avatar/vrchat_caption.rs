//! VRChat's compact caption bus: held chunks update the Avatar's local glyph cache.
use crate::plugins::osc::transport::{AvatarSnapshot, OscTransportHandle, OscValue, WriteLease};
use std::{collections::VecDeque, time::Duration};
use unicode_normalization::UnicodeNormalization;

const PREFIX: &str = "/avatar/parameters/XRT/Caption/";
const POINTER: &str = "/avatar/parameters/XRT/Caption/Pointer";
const SLOTS: usize = 128;
// Parameter sync is best effort. Keep each complete bus value visible instead
// of relying on OSC arrival rate or replaying missed deadlines in a burst.
const HOLD: Duration = Duration::from_millis(200);
const DISPLAY_TIME: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    columns: usize,
    bytes: usize,
}

pub(super) fn paths() -> Vec<String> {
    ["Version", "Columns", "Bytes", "Pointer"]
        .into_iter()
        .map(|name| format!("{PREFIX}{name}"))
        .chain((0..8).map(|index| format!("{PREFIX}Byte{index}")))
        .collect()
}

impl Layout {
    pub(super) fn discover(snapshot: &AvatarSnapshot) -> Option<Self> {
        let integer = |name: &str, access: u8| {
            let node = snapshot.nodes.get(&format!("{PREFIX}{name}"))?;
            if node.osc_type != "i" || node.access & access != access {
                return None;
            }
            match node.value {
                Some(OscValue::Int(value)) => Some(value),
                _ => None,
            }
        };
        if integer("Version", 1)? != 1 {
            return None;
        }
        let columns = integer("Columns", 1)?;
        let bytes = integer("Bytes", 1)?;
        if !matches!(columns, 8 | 16 | 32) || !matches!(bytes, 2 | 4 | 8) {
            return None;
        }
        if !(0..=255).contains(&integer("Pointer", 3)?) {
            return None;
        }
        for index in 0..bytes {
            if !(0..=255).contains(&integer(&format!("Byte{index}"), 3)?) {
                return None;
            }
        }
        Some(Self {
            columns: columns as usize,
            bytes: bytes as usize,
        })
    }

    fn glyphs(self, text: &str) -> Vec<u16> {
        let normalized: String = text
            .chars()
            .map(|c| {
                if c as u32 > 0xffff {
                    '\u{fffd}'
                } else if c.is_control() && c != '\n' {
                    ' '
                } else {
                    c
                }
            })
            .nfc()
            .collect();
        let mut glyphs = Vec::with_capacity(SLOTS + 1);
        let mut previous_line_empty = false;
        'lines: for (line_index, line) in normalized.trim().split('\n').enumerate() {
            if line_index > 0 {
                let remaining = self.columns - glyphs.len() % self.columns;
                if remaining != self.columns || previous_line_empty {
                    glyphs.resize((glyphs.len() + remaining).min(SLOTS + 1), 0);
                }
            }
            previous_line_empty = line.trim().is_empty();
            for (word_index, word) in line.split_whitespace().enumerate() {
                let width = word.chars().count();
                let column = glyphs.len() % self.columns;
                if word_index > 0 && column != 0 {
                    if width <= self.columns && column + width + 1 > self.columns {
                        glyphs.resize((glyphs.len() + self.columns - column).min(SLOTS + 1), 0);
                    } else {
                        glyphs.push(0x20);
                    }
                }
                for character in word.chars().filter(|c| !c.is_control()) {
                    if glyphs.len() > SLOTS {
                        break 'lines;
                    }
                    glyphs.push(u16::try_from(character as u32).unwrap_or(0xfffd));
                }
            }
            if glyphs.len() > SLOTS {
                break;
            }
        }
        if glyphs.len() > SLOTS {
            glyphs.truncate(SLOTS);
            glyphs[SLOTS - 1] = 0x2026;
        }
        glyphs
    }

    fn frames(self, text: &str) -> VecDeque<Vec<(String, OscValue)>> {
        let glyphs = self.glyphs(text);
        let mut frames = VecDeque::from([vec![(POINTER.into(), OscValue::Int(255))]]);
        for (pointer, chunk) in glyphs.chunks(self.bytes / 2).enumerate() {
            let mut values = Vec::with_capacity(self.bytes + 1);
            for index in 0..self.bytes / 2 {
                let glyph = chunk.get(index).copied().unwrap_or_default();
                for (offset, byte) in glyph.to_be_bytes().into_iter().enumerate() {
                    values.push((
                        format!("{PREFIX}Byte{}", index * 2 + offset),
                        OscValue::Int(i32::from(byte)),
                    ));
                }
            }
            // Bytes precede the pointer within one immediate OSC bundle.
            values.push((POINTER.into(), OscValue::Int(pointer as i32)));
            frames.push_back(values);
        }
        frames
    }
}

pub(super) struct Board {
    transport: OscTransportHandle,
    pub epoch: u64,
    pub layout: Layout,
    lease: WriteLease,
    pending: VecDeque<Vec<(String, OscValue)>>,
    last_text: String,
}

impl Board {
    pub fn new(transport: OscTransportHandle, epoch: u64, layout: Layout) -> Self {
        Self {
            transport,
            epoch,
            layout,
            lease: WriteLease::new(HOLD),
            pending: VecDeque::new(),
            last_text: String::new(),
        }
    }

    pub fn replace(&mut self, text: &str) {
        if self.last_text == text {
            return;
        }
        self.lease.revoke();
        self.lease = WriteLease::new(HOLD);
        self.pending = self.layout.frames(text);
        self.last_text = text.to_owned();
    }

    pub fn clear(&mut self) {
        self.lease.revoke();
        self.pending.clear();
        self.last_text.clear();
        let _ = self
            .transport
            .write_parameter(self.epoch, POINTER, OscValue::Int(255));
    }

    pub fn poll(&mut self) -> Result<(), String> {
        if let Some(values) = self.pending.front() {
            if self
                .transport
                .write_batch(self.epoch, values.clone(), &self.lease)?
            {
                self.pending.pop_front();
            }
        } else if !self.last_text.is_empty()
            && self
                .lease
                .last_sent()
                .is_some_and(|sent| sent.elapsed() >= DISPLAY_TIME)
        {
            self.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_bus_preserves_bmp_layout_and_clears_before_writing() {
        let layout = Layout {
            columns: 16,
            bytes: 4,
        };
        let glyphs = layout.glyphs("A文\n🙂B");
        assert_eq!(&glyphs[..2], &[65, 0x6587]);
        assert!(glyphs[2..16].iter().all(|glyph| *glyph == 0));
        assert_eq!(&glyphs[16..], &[0xfffd, 66]);
        let frames = layout.frames("A文B");
        assert_eq!(frames[0], vec![(POINTER.into(), OscValue::Int(255))]);
        assert_eq!(
            frames[1].iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
            vec![
                OscValue::Int(0),
                OscValue::Int(65),
                OscValue::Int(0x65),
                OscValue::Int(0x87),
                OscValue::Int(0)
            ]
        );
        assert_eq!(
            frames[2].iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(),
            vec![
                OscValue::Int(0),
                OscValue::Int(66),
                OscValue::Int(0),
                OscValue::Int(0),
                OscValue::Int(1)
            ]
        );
        assert_eq!(
            layout.glyphs("Cafe\u{301}\t한"),
            vec![67, 97, 102, 0xe9, 32, 0xd55c]
        );
        let wrapped = layout.glyphs("Meet us near the entrance");
        assert_eq!(
            &wrapped[16..24],
            &"entrance".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(layout.glyphs("1234567890123456\nB")[16], 66);
        let truncated = layout.glyphs(&"文".repeat(130));
        assert_eq!(truncated.len(), SLOTS);
        assert_eq!(truncated[SLOTS - 1], 0x2026);
    }
}
