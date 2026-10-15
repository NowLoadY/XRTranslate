//! Buffered captions over verified OSC Avatar parameters.
//! Glyph layout and delivery are shared; each profile uses its native parameter width.
use crate::plugins::osc::transport::{
    AvatarSnapshot, OscPeerProfile, OscTransportHandle, OscValue, WriteLease,
};
use std::{collections::VecDeque, time::Duration};
use unicode_normalization::UnicodeNormalization;

const PREFIX: &str = "/avatar/parameters/XRT/Caption/";
const POINTER: &str = "/avatar/parameters/XRT/Caption/Pointer";
const SLOTS: usize = 128;
// Receivers repeatedly sample the held bus. Pacing follows local delivery,
// rather than replaying missed deadlines in a burst.
const HOLD: Duration = Duration::from_millis(200);
const DISPLAY_TIME: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    profile: OscPeerProfile,
    columns: usize,
    bytes: usize,
}

fn capability_prefix(profile: OscPeerProfile) -> &'static str {
    match profile {
        OscPeerProfile::VrChat => PREFIX,
        OscPeerProfile::ChilloutVr => "/avatar/parameters/#XRT/Caption/",
    }
}

pub(super) fn paths(profile: OscPeerProfile) -> Vec<String> {
    let capabilities = capability_prefix(profile);
    let (field, count) = payload_fields(profile);
    ["Version", "Columns", "Bytes"]
        .into_iter()
        .map(|name| format!("{capabilities}{name}"))
        .chain(std::iter::once(POINTER.to_owned()))
        .chain((0..count).map(|index| format!("{PREFIX}{field}{index}")))
        .collect()
}

fn payload_fields(profile: OscPeerProfile) -> (&'static str, usize) {
    match profile {
        OscPeerProfile::VrChat => ("Byte", 8),
        OscPeerProfile::ChilloutVr => ("Word", 2),
    }
}

impl Layout {
    pub(super) fn discover(profile: OscPeerProfile, snapshot: &AvatarSnapshot) -> Option<Self> {
        let integer = |path: String, access: u8| {
            let node = snapshot.nodes.get(&path)?;
            if node.osc_type != "i" || node.access & access != access {
                return None;
            }
            match node.value {
                Some(OscValue::Int(value)) => Some(value),
                _ => None,
            }
        };
        let capabilities = capability_prefix(profile);
        if integer(format!("{capabilities}Version"), 1)? != 1 {
            return None;
        }
        let columns = integer(format!("{capabilities}Columns"), 1)?;
        let bytes = integer(format!("{capabilities}Bytes"), 1)?;
        if !matches!(columns, 8 | 16 | 32) || !matches!(bytes, 2 | 4 | 8) {
            return None;
        }
        if profile == OscPeerProfile::ChilloutVr && bytes != 8 {
            return None;
        }
        if !(0..=255).contains(&integer(POINTER.into(), 3)?) {
            return None;
        }
        let (field, count) = payload_fields(profile);
        let count = if profile == OscPeerProfile::VrChat {
            bytes as usize
        } else {
            count
        };
        for index in 0..count {
            let value = integer(format!("{PREFIX}{field}{index}"), 3)?;
            if profile == OscPeerProfile::VrChat && !(0..=255).contains(&value) {
                return None;
            }
        }
        Some(Self {
            profile,
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
            match self.profile {
                OscPeerProfile::VrChat => {
                    for index in 0..self.bytes / 2 {
                        let glyph = chunk.get(index).copied().unwrap_or_default();
                        for (offset, byte) in glyph.to_be_bytes().into_iter().enumerate() {
                            values.push((
                                format!("{PREFIX}Byte{}", index * 2 + offset),
                                OscValue::Int(i32::from(byte)),
                            ));
                        }
                    }
                }
                OscPeerProfile::ChilloutVr => {
                    for index in 0..2 {
                        let high = u32::from(chunk.get(index * 2).copied().unwrap_or_default());
                        let low = u32::from(chunk.get(index * 2 + 1).copied().unwrap_or_default());
                        values.push((
                            format!("{PREFIX}Word{index}"),
                            OscValue::Int(((high << 16) | low) as i32),
                        ));
                    }
                }
            }
            // Payload precedes the pointer within one immediate OSC bundle.
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
    fn caption_profiles_share_layout_and_preserve_signed_packed_glyphs() {
        use crate::plugins::osc::transport::OscQueryNode;
        use std::time::Instant;

        let snapshot = |profile| AvatarSnapshot {
            epoch: 1,
            query_id: None,
            avatar_id: "caption-avatar".into(),
            queried_at: Instant::now(),
            nodes: paths(profile)
                .into_iter()
                .map(|path| {
                    let value = match path.rsplit('/').next().unwrap() {
                        "Version" => 1,
                        "Columns" => 8,
                        "Bytes" => 8,
                        "Pointer" => 255,
                        "Word0" => i32::MIN,
                        "Word1" => i32::MAX,
                        _ => 0,
                    };
                    (
                        path,
                        OscQueryNode {
                            osc_type: "i".into(),
                            access: 3,
                            value: Some(OscValue::Int(value)),
                        },
                    )
                })
                .collect(),
        };
        let vrchat =
            Layout::discover(OscPeerProfile::VrChat, &snapshot(OscPeerProfile::VrChat)).unwrap();
        let mut cvr_snapshot = snapshot(OscPeerProfile::ChilloutVr);
        let cvr = Layout::discover(OscPeerProfile::ChilloutVr, &cvr_snapshot).unwrap();
        let text = "文한🙂A";
        assert_eq!(cvr.glyphs(text), vrchat.glyphs(text));
        let frames = cvr.frames(text);
        assert_eq!(frames[0], vec![(POINTER.into(), OscValue::Int(255))]);
        assert_eq!(frames[1].len(), 3);
        let decoded: Vec<_> = frames[1][..2]
            .iter()
            .flat_map(|(_, value)| {
                let OscValue::Int(word) = value else {
                    panic!("packed word must be an Int")
                };
                [((*word as u32) >> 16) as u16, *word as u16]
            })
            .collect();
        assert_eq!(decoded, vec![0x6587, 0xd55c, 0xfffd, 65]);
        assert!(matches!(frames[1][1].1, OscValue::Int(word) if word < 0));
        assert_eq!(frames[1][2], (POINTER.into(), OscValue::Int(0)));
        assert!(
            !paths(OscPeerProfile::ChilloutVr)
                .iter()
                .any(|path| path.starts_with(&format!("{PREFIX}Byte")))
        );
        assert!(Layout::discover(OscPeerProfile::VrChat, &cvr_snapshot).is_none());

        // A partial or differently typed bus must not become a display target.
        cvr_snapshot.nodes.get_mut(POINTER).unwrap().osc_type = "f".into();
        assert!(Layout::discover(OscPeerProfile::ChilloutVr, &cvr_snapshot).is_none());
    }

    #[test]
    fn caption_bus_preserves_bmp_layout_and_clears_before_writing() {
        let layout = Layout {
            profile: OscPeerProfile::VrChat,
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
