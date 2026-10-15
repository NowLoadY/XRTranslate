//! Persisted host preferences; connector contracts are owned by the plugin SDK.
use serde::{Deserialize, Serialize};
pub use xrt_connector_core::{Connector, osc::Contract as OscContract};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connector: Connector,
    pub captions: bool,
}
