//! Native Chatbox contracts; content composition stays shared.
use rosc::{OscMessage, OscPacket, OscType};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OscGame {
    #[default]
    VRChat,
    ChilloutVR,
}

impl OscGame {
    pub fn studio_title(self) -> &'static str {
        match self {
            Self::VRChat => "VRChat OSC Studio",
            Self::ChilloutVR => "ChilloutVR OSC Studio",
        }
    }

    pub fn setup_url(self) -> &'static str {
        match self {
            Self::VRChat => "https://docs.vrchat.com/docs/osc-as-input-controller#chatbox",
            Self::ChilloutVR => "https://docs.chilloutvr.net/chilloutvr/game/osc/#chatbox",
        }
    }

    pub fn max_text_length(self) -> usize {
        match self {
            Self::VRChat => 144,
            // Application budget ceiling; CVR does not publish a ChatBox limit.
            Self::ChilloutVR => 10_000,
        }
    }

    pub(super) fn packets(self, text: &str, typing: bool, notify: bool) -> [OscPacket; 2] {
        let mut typing_args = vec![OscType::Bool(typing)];
        let mut input_args = vec![
            OscType::String(text.to_owned()),
            OscType::Bool(true),
            OscType::Bool(notify),
        ];
        if self == Self::ChilloutVR {
            typing_args.push(OscType::Bool(false));
            // A changing caption belongs in the bubble, without repeatedly
            // adding lyrics, clocks and partial sentences to chat history.
            input_args[2] = OscType::Bool(false);
            input_args.extend([OscType::Bool(true), OscType::Bool(false)]);
        }
        [
            OscPacket::Message(OscMessage {
                addr: "/chatbox/typing".into(),
                args: typing_args,
            }),
            OscPacket::Message(OscMessage {
                addr: "/chatbox/input".into(),
                args: input_args,
            }),
        ]
    }
}
