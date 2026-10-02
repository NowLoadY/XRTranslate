use std::{
    future::Future,
    net::{Ipv4Addr, TcpListener},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use base64::Engine;
use image::{ImageEncoder, RgbImage};
use xrtranslate_supervisor::{
    LlamaServerEndpoint, LlamaServerLauncher, LlamaServerProcess, LlamaServerProcessHandle,
    LlamaServerSpec, StdLlamaServerLauncher,
};

pub(super) struct VisionOcr {
    process: LlamaServerProcess,
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    api_key: String,
    #[cfg(windows)]
    _job: crate::child_process::KillOnCloseJob,
}

impl VisionOcr {
    pub(super) fn load(mut spec: LlamaServerSpec, cancelled: &AtomicBool) -> Result<Self, String> {
        let api_key = uuid::Uuid::new_v4().to_string();
        spec.extra_args
            .extend(["--api-key".into(), api_key.clone().into()]);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("Cannot reserve an OCR service port: {error}"))?;
        spec.endpoint = LlamaServerEndpoint::new(
            Ipv4Addr::LOCALHOST.into(),
            listener
                .local_addr()
                .map_err(|error| error.to_string())?
                .port(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| error.to_string())?;
        drop(listener);
        #[cfg(windows)]
        let job = crate::child_process::KillOnCloseJob::new()?;
        let process = StdLlamaServerLauncher
            .launch(&spec)
            .map_err(|error| error.to_string())?;
        #[cfg(windows)]
        job.assign_pid(
            process
                .id()
                .ok_or("OCR model process has already stopped.")?,
        )?;
        let mut model = Self {
            process,
            runtime,
            client,
            api_key,
            #[cfg(windows)]
            _job: job,
        };
        let deadline = Instant::now() + spec.startup_timeout;
        loop {
            super::check_cancelled(cancelled)?;
            if let Some(status) = model
                .process
                .try_wait()
                .map_err(|error| error.to_string())?
            {
                return Err(format!("OCR model could not start ({status})."));
            }
            let health = model
                .client
                .get(model.process.endpoint().url("/health"))
                .bearer_auth(&model.api_key)
                .timeout(Duration::from_secs(1))
                .send();
            if model
                .runtime
                .block_on(cancellable(health, cancelled))?
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(model);
            }
            if Instant::now() >= deadline {
                return Err("OCR model startup timed out.".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(super) fn recognize(
        &mut self,
        image: RgbImage,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        if let Some(status) = self.process.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("OCR model stopped ({status})."));
        }
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|error| error.to_string())?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let request = self.client.post(self.process.endpoint().url("/v1/chat/completions"))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.process.model_alias(),
                "messages": [{"role":"user", "content":[
                    {"type":"image_url", "image_url":{"url":format!("data:image/png;base64,{encoded}")}},
                    {"type":"text", "text":"OCR:"}
                ]}],
                "temperature": 0,
                "max_tokens": 2048,
                "stream": false
            }));
        let output: serde_json::Value = self
            .runtime
            .block_on(cancellable(
                async { request.send().await?.error_for_status()?.json().await },
                cancelled,
            ))?
            .map_err(|error: reqwest::Error| format!("OCR recognition failed: {error}"))?;
        output
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .map(|text| text.trim().to_owned())
            .ok_or_else(|| "OCR model returned no text result.".into())
    }
}

async fn cancellable<T>(
    work: impl Future<Output = T>,
    cancelled: &AtomicBool,
) -> Result<T, String> {
    tokio::pin!(work);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("OCR stopped.".into());
        }
        tokio::select! {
            result = &mut work => return Ok(result),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}
