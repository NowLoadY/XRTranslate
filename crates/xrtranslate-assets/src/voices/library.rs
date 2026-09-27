//! One atomic directory per user card; listing never loads reference audio.
use super::{BUILTIN_VOICES, SAMPLE_RATE, builtin};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use xrtranslate_engine::audio::pcm16_mono_16khz_to_wav;

pub const MAX_REFERENCE_SECONDS: usize = 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoiceCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub duration_ms: u64,
    pub created_at_ms: u64,
}

impl VoiceCard {
    pub fn is_builtin(&self) -> bool {
        builtin(&self.id).is_some()
    }
}

#[derive(Clone)]
pub struct VoiceCatalog {
    root: PathBuf,
}

impl VoiceCatalog {
    pub fn new(voice_clones_directory: impl AsRef<Path>) -> Self {
        Self {
            root: voice_clones_directory.as_ref().join("cards"),
        }
    }

    pub fn builtin_cards() -> Vec<VoiceCard> {
        BUILTIN_VOICES
            .iter()
            .map(|voice| VoiceCard {
                id: voice.id.into(),
                name: voice.name.into(),
                description: voice.description.into(),
                duration_ms: (voice.pcm16().len() as u64 * 1000) / (u64::from(SAMPLE_RATE) * 2),
                created_at_ms: 0,
            })
            .collect()
    }

    pub fn list(&self) -> Result<Vec<VoiceCard>, String> {
        let mut cards = Self::builtin_cards();
        if !self.root.exists() {
            return Ok(cards);
        }
        let mut user_cards = Vec::new();
        for shard in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let shard = shard.map_err(|e| e.to_string())?;
            if !shard.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            for entry in fs::read_dir(shard.path())
                .map_err(|e| e.to_string())?
                .flatten()
            {
                let id = entry.file_name().to_string_lossy().into_owned();
                if self
                    .directory(&id)
                    .as_ref()
                    .is_ok_and(|path| *path == entry.path())
                    && let Ok(bytes) = fs::read(entry.path().join("card.json"))
                    && let Ok(card) = serde_json::from_slice::<VoiceCard>(&bytes)
                    && card.id == id
                {
                    user_cards.push(card);
                }
            }
        }
        user_cards.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms).then(a.id.cmp(&b.id)));
        cards.extend(user_cards);
        Ok(cards)
    }

    pub fn add(
        &self,
        name: &str,
        description: &str,
        transcript: &str,
        pcm: &[u8],
    ) -> Result<VoiceCard, String> {
        let name = name.trim();
        let description = description.trim();
        let transcript = transcript.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err("Voice name must contain 1–100 characters.".into());
        }
        if description.chars().count() > 500 {
            return Err("Voice description is too long (500 characters maximum).".into());
        }
        if transcript.is_empty() || transcript.chars().count() > 20_000 {
            return Err(
                "Enter the words spoken in the reference audio (20,000 characters maximum).".into(),
            );
        }
        if pcm.len() < SAMPLE_RATE as usize
            || pcm.len() > MAX_REFERENCE_SECONDS * SAMPLE_RATE as usize * 2
        {
            return Err("Reference audio must be between 0.5 and 60 seconds.".into());
        }
        let wav = pcm16_mono_16khz_to_wav(pcm)?;
        let card = VoiceCard {
            id: uuid::Uuid::new_v4().simple().to_string(),
            name: name.into(),
            description: description.into(),
            duration_ms: pcm.len() as u64 * 1000 / (u64::from(SAMPLE_RATE) * 2),
            created_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };
        let directory = self.directory(&card.id)?;
        let staging = directory.with_file_name(format!(".import-{}", card.id));
        fs::create_dir_all(directory.parent().expect("card shard")).map_err(|e| e.to_string())?;
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        let result = (|| {
            fs::write(staging.join("reference.wav"), wav)?;
            fs::write(staging.join("reference.txt"), transcript)?;
            fs::write(staging.join("card.json"), serde_json::to_vec_pretty(&card)?)?;
            fs::rename(&staging, &directory)
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error.to_string());
        }
        Ok(card)
    }

    pub fn reference(&self, id: &str) -> Result<(Cow<'static, [u8]>, Cow<'static, str>), String> {
        if let Some(voice) = builtin(id) {
            return Ok((Cow::Borrowed(voice.wav), Cow::Borrowed(voice.transcript)));
        }
        let directory = self.directory(id)?;
        // Metadata is the commit marker; never expose incomplete imports.
        let card: VoiceCard = serde_json::from_slice(
            &fs::read(directory.join("card.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if card.id != id {
            return Err("Voice card identity does not match its directory.".into());
        }
        let audio_path = directory.join("reference.wav");
        if fs::metadata(&audio_path).map_err(|e| e.to_string())?.len()
            > (MAX_REFERENCE_SECONDS * SAMPLE_RATE as usize * 2 + 44) as u64
        {
            return Err("Reference audio must be between 0.5 and 60 seconds.".into());
        }
        let wav = fs::read(audio_path).map_err(|e| e.to_string())?;
        let transcript =
            fs::read_to_string(directory.join("reference.txt")).map_err(|e| e.to_string())?;
        let pcm = wav.get(44..).ok_or("Invalid reference WAV.")?;
        if pcm.len() > MAX_REFERENCE_SECONDS * SAMPLE_RATE as usize * 2
            || pcm16_mono_16khz_to_wav(pcm)? != wav
        {
            return Err("Reference WAV must be canonical mono PCM16 at 16 kHz.".into());
        }
        Ok((Cow::Owned(wav), Cow::Owned(transcript)))
    }

    fn directory(&self, id: &str) -> Result<PathBuf, String> {
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Unknown voice card.".into());
        }
        Ok(self.root.join(&id[..2]).join(id))
    }
}
