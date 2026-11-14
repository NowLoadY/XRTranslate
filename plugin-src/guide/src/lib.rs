//! The same embedded lessons and planner serve the bundled plugin and standalone player.
mod audio;
mod planner;
pub use audio::Narrator;
pub use planner::Planner;
pub use xrtranslate_plugin::guide;
