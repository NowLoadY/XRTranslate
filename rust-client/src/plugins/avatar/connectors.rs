//! Platform capabilities and OSC address mapping; session policy stays shared.
use crate::plugins::osc::transport::OscPeerProfile;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connector {
    #[default]
    Vrchat,
    Chilloutvr,
    Resonite,
    Overte,
    Secondlife,
}

impl Connector {
    pub const ALL: [Self; 5] = [
        Self::Vrchat,
        Self::Chilloutvr,
        Self::Resonite,
        Self::Overte,
        Self::Secondlife,
    ];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Vrchat => "vrchat",
            Self::Chilloutvr => "chilloutvr",
            Self::Resonite => "resonite",
            Self::Overte => "overte",
            Self::Secondlife => "secondlife",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vrchat => "VRChat",
            Self::Chilloutvr => "ChilloutVR",
            Self::Resonite => "Resonite",
            Self::Overte => "Overte",
            Self::Secondlife => "Second Life",
        }
    }
    pub fn osc(self) -> Option<OscContract> {
        match self {
            Self::Vrchat => Some(OscContract {
                profile: OscPeerProfile::VrChat,
                enabled: xrt_avatar_control::protocol::ENABLED_ADDRESS,
                status: xrt_avatar_control::protocol::STATUS_ADDRESS,
                version: xrt_avatar_control::protocol::VERSION_ADDRESS,
            }),
            Self::Chilloutvr => Some(OscContract {
                profile: OscPeerProfile::ChilloutVr,
                enabled: xrt_chilloutvr_control::protocol::ENABLED_ADDRESS,
                status: xrt_chilloutvr_control::protocol::STATUS_ADDRESS,
                version: xrt_chilloutvr_control::protocol::VERSION_ADDRESS,
            }),
            _ => None,
        }
    }
}

pub struct OscContract {
    pub profile: OscPeerProfile,
    pub enabled: &'static str,
    pub status: &'static str,
    pub version: &'static str,
}
impl OscContract {
    pub fn paths(&self) -> Vec<String> {
        let mut paths = [self.enabled, self.status, self.version]
            .map(str::to_owned)
            .to_vec();
        paths.extend(super::osc_caption::paths(self.profile));
        paths
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connector: Connector,
    pub captions: bool,
}
