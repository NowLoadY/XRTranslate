//! Cancellable HTTP reads shared by podcast discovery and streaming decoding.
use std::{
    future::Future,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use reqwest::{Response, Url, header};
use symphonia::core::io::MediaSource;

pub fn http_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|_| "Enter a valid HTTP or HTTPS URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Enter a valid HTTP or HTTPS URL".into());
    }
    Ok(url)
}

pub struct HttpClient {
    runtime: tokio::runtime::Runtime,
    client: reqwest::Client,
    stop: Arc<AtomicBool>,
}

impl HttpClient {
    pub fn new(proxy: &str, stop: Arc<AtomicBool>) -> Result<Self, String> {
        let mut builder = xrtranslate_download::client_builder()
            .user_agent("XRTranslate/0.3 podcast reader")
            .connect_timeout(Duration::from_secs(15));
        if !proxy.trim().is_empty() {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
        }
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?,
            client: builder.build().map_err(|e| e.to_string())?,
            stop,
        })
    }

    fn wait<T>(
        &self,
        future: impl Future<Output = Result<T, reqwest::Error>>,
    ) -> Result<T, String> {
        self.runtime.block_on(async {
            let operation = tokio::time::timeout(Duration::from_secs(30), future);
            tokio::pin!(operation);
            let mut poll = tokio::time::interval(Duration::from_millis(50));
            loop {
                if self.stop.load(Ordering::Acquire) {
                    return Err("Audio import stopped".into());
                }
                tokio::select! {
                    result = &mut operation => return result
                        .map_err(|_| "Audio request timed out".to_owned())?
                        .map_err(request_error),
                    _ = poll.tick() => {}
                }
            }
        })
    }

    pub fn get(&self, url: &Url, offset: Option<u64>) -> Result<Response, String> {
        http_url(url.as_str())?;
        let mut request = self
            .client
            .get(url.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(offset) = offset {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let response = self.wait(request.send())?;
        http_url(response.url().as_str())?;
        response
            .error_for_status()
            .map_err(|e| e.without_url().to_string())
    }

    pub fn chunk(&self, response: &mut Response) -> Result<Option<Vec<u8>>, String> {
        self.wait(response.chunk())
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
    }

    pub fn document(&self, mut response: Response) -> Result<String, String> {
        const LIMIT: usize = 8 * 1024 * 1024;
        let mut bytes = Vec::new();
        while let Some(chunk) = self.chunk(&mut response)? {
            if bytes.len() + chunk.len() > LIMIT {
                return Err("Podcast page or feed exceeds 8 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|_| "Podcast page or feed is not UTF-8".into())
    }
}

fn request_error(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(error) = cause {
        message.push_str(": ");
        message.push_str(&error.to_string());
        cause = error.source();
    }
    message
}

pub struct HttpAudioSource {
    client: HttpClient,
    url: Url,
    response: Option<Response>,
    buffer: io::Cursor<Vec<u8>>,
    position: u64,
    length: Option<u64>,
    seekable: bool,
    etag: Option<header::HeaderValue>,
}

impl HttpAudioSource {
    pub fn open(url: &str, proxy: &str, stop: Arc<AtomicBool>) -> Result<Self, String> {
        let client = HttpClient::new(proxy, stop)?;
        let url = http_url(url)?;
        let response = client.get(&url, Some(0))?;
        let length = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            check_range(&response, 0)?
        } else {
            response.content_length()
        };
        let seekable = response.status() == reqwest::StatusCode::PARTIAL_CONTENT
            || response
                .headers()
                .get(header::ACCEPT_RANGES)
                .is_some_and(|v| v == "bytes");
        let url = response.url().clone();
        let etag = response.headers().get(header::ETAG).cloned();
        Ok(Self {
            client,
            url,
            response: Some(response),
            buffer: io::Cursor::new(Vec::new()),
            position: 0,
            length,
            seekable,
            etag,
        })
    }
}

impl Read for HttpAudioSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.buffer.read(output)?;
            if count > 0 {
                self.position += count as u64;
                return Ok(count);
            }
            if self.length.is_some_and(|length| self.position >= length) {
                return Ok(0);
            }
            if self.response.is_none() {
                let response = self
                    .client
                    .get(&self.url, Some(self.position))
                    .map_err(io::Error::other)?;
                let length = check_range(&response, self.position).map_err(io::Error::other)?;
                if length != self.length
                    || self
                        .etag
                        .as_ref()
                        .is_some_and(|etag| response.headers().get(header::ETAG) != Some(etag))
                {
                    return Err(io::Error::other("Online audio changed while reading"));
                }
                self.response = Some(response);
            }
            match self
                .client
                .chunk(self.response.as_mut().unwrap())
                .map_err(io::Error::other)?
            {
                Some(bytes) => self.buffer = io::Cursor::new(bytes),
                None if self.length.is_some_and(|length| self.position < length) => {
                    // Never use UnexpectedEof here: the decoder treats that as a clean end.
                    return Err(io::Error::other(
                        "Audio download ended before the advertised length",
                    ));
                }
                None => return Ok(0),
            }
        }
    }
}

impl Seek for HttpAudioSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self
                .length
                .and_then(|length| length.checked_add_signed(offset)),
        }
        .ok_or_else(|| io::Error::other("Audio source has no known seek position"))?;
        if position == self.position {
            return Ok(position);
        }
        if !self.seekable {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Audio server does not support byte ranges; import the downloaded file instead",
            ));
        }
        self.position = position;
        self.response = None;
        self.buffer = io::Cursor::new(Vec::new());
        Ok(position)
    }
}

impl MediaSource for HttpAudioSource {
    fn is_seekable(&self) -> bool {
        self.seekable
    }
    fn byte_len(&self) -> Option<u64> {
        self.length
    }
}

fn check_range(response: &Response, offset: u64) -> Result<Option<u64>, String> {
    let range = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok());
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
        || !range.is_some_and(|range| range.starts_with(&format!("bytes {offset}-")))
    {
        return Err("Audio server returned an invalid byte range".into());
    }
    let total = range
        .and_then(|range| range.split_once('/'))
        .map(|(_, total)| total)
        .ok_or("Audio server returned an invalid byte range")?;
    if total == "*" {
        Ok(None)
    } else {
        total
            .parse()
            .map(Some)
            .map_err(|_| "Audio server returned an invalid byte range".into())
    }
}
