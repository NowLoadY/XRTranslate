//! Download-source routing shared by all artifact installers.

use std::borrow::Cow;

/// User-selected route for an immutable artifact download.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DownloadSource {
    #[default]
    Official,
    Mirror,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_cover_pinned_model_and_license_sources() {
        assert_eq!(
            DownloadSource::Mirror.resolve("https://huggingface.co/org/model/resolve/rev/file"),
            "https://hf-mirror.com/org/model/resolve/rev/file"
        );
        assert_eq!(
            DownloadSource::Mirror
                .resolve("https://raw.githubusercontent.com/org/repo/rev/assets/model"),
            "https://ghfast.top/https://raw.githubusercontent.com/org/repo/rev/assets/model"
        );
        assert_eq!(
            DownloadSource::Mirror
                .resolve("https://github.com/org/repo/releases/download/tag/file"),
            "https://ghfast.top/https://github.com/org/repo/releases/download/tag/file"
        );
        assert_eq!(
            DownloadSource::Mirror.resolve("https://example.com/file"),
            "https://example.com/file"
        );
    }
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

    pub fn resolve<'a>(self, official_url: &'a str) -> Cow<'a, str> {
        if self == Self::Official {
            return Cow::Borrowed(official_url);
        }
        if let Some(path) = official_url.strip_prefix("https://huggingface.co/") {
            return Cow::Owned(format!("https://hf-mirror.com/{path}"));
        }
        if official_url.starts_with("https://github.com/")
            || official_url.starts_with("https://raw.githubusercontent.com/")
        {
            return Cow::Owned(format!("https://ghfast.top/{official_url}"));
        }
        Cow::Borrowed(official_url)
    }
}
