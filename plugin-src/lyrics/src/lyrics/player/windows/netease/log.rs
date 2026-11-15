use super::events::State;
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::windows::io::AsRawHandle,
    path::PathBuf,
};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
};

const MAX_BACKLOG: u64 = 4 * 1024 * 1024;
const MAX_LINE: usize = 256 * 1024;

pub(super) struct Reader {
    path: PathBuf,
    identity: Option<(u32, u64)>,
    offset: u64,
    pending: Vec<u8>,
    skip_partial: bool,
    pub state: State,
}

impl Reader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            identity: None,
            offset: 0,
            pending: Vec::new(),
            skip_partial: false,
            state: State::default(),
        }
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let mut file = File::open(&self.path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(io::Error::other)?;
        // File IDs detect rotation even when Windows preserves the creation time
        // of a replaced file and the replacement has the same length.
        let identity = (
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        );
        let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
        if self.identity != Some(identity)
            || length < self.offset
            || length - self.offset > MAX_BACKLOG
        {
            self.identity = Some(identity);
            self.offset = length.saturating_sub(MAX_BACKLOG);
            self.skip_partial = self.offset > 0;
            self.pending.clear();
            self.state = State::default();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(MAX_BACKLOG).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        self.pending.extend(bytes.into_iter().map(decode));
        let mut consumed = 0;
        for line in self.pending.split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            if !self.skip_partial
                && line.len() <= MAX_LINE
                && let Ok(line) = std::str::from_utf8(line)
            {
                self.state.push(line);
            }
            self.skip_partial = false;
            consumed += line.len();
        }
        self.pending.drain(..consumed);
        if self.pending.len() > MAX_LINE {
            self.pending.clear();
            self.skip_partial = true;
        }
        Ok(())
    }
}

// Adapted from ElogAnalysis.decode in netease-cloudmusic-detector (MIT).
fn decode(byte: u8) -> u8 {
    ((((byte >> 4) ^ ((byte & 15) + 8)) & 15) << 4) | ((byte >> 6) * 4 + (!(byte >> 4) & 3))
}

#[cfg(test)]
pub(super) fn encode(text: &[u8]) -> Vec<u8> {
    text.iter()
        .map(|&value| (0..=u8::MAX).find(|&byte| decode(byte) == value).unwrap())
        .collect()
}
