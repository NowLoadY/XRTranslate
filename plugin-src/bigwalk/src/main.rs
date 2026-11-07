mod runtime;
mod text;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use xrtranslate_plugin::{
    Context, Environment, Plugin, Result, Text, Widget,
    translation::{AudioSourceFilter, MessageFormat},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    audio_sources: AudioSourceFilter,
    format: MessageFormat,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            audio_sources: AudioSourceFilter {
                microphone: true,
                system_audio: false,
            },
            format: MessageFormat::TargetOnly,
        }
    }
}
#[derive(Default)]
struct BigWalk {
    settings: Settings,
    runtime: Option<runtime::Runtime>,
    view: Vec<Widget>,
    error: Option<String>,
}
fn label(en: &str, zh: &str) -> Text {
    Text([("en".into(), en.into()), ("zh".into(), zh.into())].into())
}
impl Plugin for BigWalk {
    fn initialize(
        &mut self,
        environment: Environment,
        context: &mut Context,
    ) -> std::result::Result<(), String> {
        let config = if environment.config.is_null() {
            environment
                .data_directory
                .ancestors()
                .nth(3)
                .and_then(|root| std::fs::read(root.join("rust-client-settings.json")).ok())
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|settings| settings.get("bigwalk_settings").cloned())
                .unwrap_or(Value::Null)
        } else {
            environment.config
        };
        self.settings = serde_json::from_value(config).unwrap_or_default();
        self.runtime = Some(runtime::Runtime::new(&self.settings, true));
        context.event("config", json!(self.settings));
        Ok(())
    }
    fn call(&mut self, name: &str, _: Value, _: &mut Context) -> Result {
        if name != "bigwalk.status" {
            return Err("Unknown operation".into());
        }
        let status = self.runtime.as_ref().unwrap().snapshot();
        Ok(
            json!({"status":format!("{:?}",status.status),"queued":status.queued,"skipped":status.skipped,"last_sent":status.last_sent,"error":status.error}),
        )
    }
    fn event(&mut self, name: &str, value: Value, context: &mut Context) {
        let runtime = self.runtime.as_ref().unwrap();
        if name != "ui" {
            if let Err(error) = runtime.publisher().event(name, value) {
                log::warn!("Translation event: {error}");
            }
            return;
        }
        match value["id"].as_str().unwrap_or("") {
            "microphone" => self.settings.audio_sources.microphone = value["value"] == true,
            "system_audio" => self.settings.audio_sources.system_audio = value["value"] == true,
            "format" => {
                if let Ok(format) = serde_json::from_value(value["value"].clone()) {
                    self.settings.format = format;
                }
            }
            "text.compose" => {
                self.error = (!runtime.send_manual(value["value"].as_str().unwrap_or("")))
                    .then(|| "Message not sent: connect to a game first.".into());
                return;
            }
            _ => return,
        }
        runtime.configure(&self.settings, true);
        context.event("config", serde_json::to_value(&self.settings).unwrap());
    }
    fn tick(&mut self, _: Duration, _: &mut Context) -> Option<Vec<Widget>> {
        let snapshot = self.runtime.as_ref().unwrap().snapshot();
        let status = match snapshot.status {
            runtime::ConnectionStatus::Disabled => label("Sending is off", "已停止发送"),
            runtime::ConnectionStatus::Connecting => label(
                "Open Big Walk with the XRT bridge.",
                "请打开已安装 XRT 桥接的 Big Walk。",
            ),
            runtime::ConnectionStatus::WaitingForPlayer => {
                label("Join a game to send messages.", "进入游戏后即可发送消息。")
            }
            runtime::ConnectionStatus::Ready => label("Connected", "已连接"),
        };
        let mut view=vec![Widget::Card{children:vec![Widget::Label{text:status},
            Widget::Link{text:label("Setup guide","配置指南"),url:"https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/bigwalk/installation.md".into()},
            Widget::Row{children:vec![
                Widget::Toggle{id:"microphone".into(),label:label("Microphone","麦克风"),value:self.settings.audio_sources.microphone,enabled:true},
                Widget::Toggle{id:"system_audio".into(),label:label("System audio","系统声音"),value:self.settings.audio_sources.system_audio,enabled:true},
            ]},
            Widget::Select{id:"format".into(),label:label("Format","格式"),value:serde_json::to_value(self.settings.format).unwrap().as_str().unwrap().into(),choices:[
                (MessageFormat::TargetOnly,"仅译文"),(MessageFormat::BilingualSourceFirst,"双语：原文在前"),(MessageFormat::BilingualTargetFirst,"双语：译文在前"),(MessageFormat::Inline,"双语：同行")
            ].into_iter().map(|(format,zh)|(serde_json::to_value(format).unwrap().as_str().unwrap().into(),label(format.label_key(),zh))).collect()},
        ]},Widget::Card{children:vec![Widget::Label{text:label("Last sent","最近发送")},Widget::Label{text:snapshot.last_sent.as_str().into()}]},Widget::Host{capability:"text.compose".into(),enabled:snapshot.status==runtime::ConnectionStatus::Ready}];
        if let Some(error) = self.error.as_ref().or(snapshot.error.as_ref()) {
            view.push(Widget::Error {
                message: error.clone(),
            });
        }
        if self.view == view {
            None
        } else {
            self.view = view.clone();
            Some(view)
        }
    }
}
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    if let Err(error) = xrtranslate_plugin::serve(BigWalk::default()) {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
