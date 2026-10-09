//! Download-source routing shared by all artifact installers.

use std::borrow::Cow;

/// User-selected route for an immutable artifact download.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DownloadSource {
    #[default]
    Official,
    Mirror,
}

impl DownloadSource {
    #[must_use]
    pub const fn from_mirror_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Mirror
        } else {
            Self::Official
        }
    }

    pub(crate) fn resolve<'a>(self, official_url: &'a str) -> Cow<'a, str> {
        if self == Self::Official {
            return Cow::Borrowed(official_url);
        }
        if let Some(path) = official_url.strip_prefix("https://huggingface.co/") {
            return Cow::Owned(format!("https://hf-mirror.com/{path}"));
        }
        if official_url.starts_with("https://github.com/") {
            return Cow::Owned(format!("https://ghfast.top/{official_url}"));
        }
        Cow::Borrowed(official_url)
    }
}
