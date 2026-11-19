use super::KeyResponses;
use crate::InferenceError;
use serde_json::{Value, json};
use std::time::Duration;
use xrtranslate_assets::remote::AgentApi;
use xrtranslate_protocol::image::{AgentImageRequest, MAX_ENCODED_BYTES};

impl KeyResponses {
    pub(crate) async fn image(
        &self,
        model: &str,
        request: &AgentImageRequest,
    ) -> Result<String, InferenceError> {
        if self.api != AgentApi::OpenaiResponses {
            return Err(InferenceError::InvalidConfiguration {
                field: "agent.model",
                message:
                    "Image generation requires an OpenAI API-key Agent with Responses image tools"
                        .into(),
            });
        }
        request
            .validate()
            .map_err(|message| InferenceError::InvalidConfiguration {
                field: "image",
                message: message.into(),
            })?;
        let mut content = vec![json!({"type":"input_text","text":request.prompt})];
        content.extend(
            request
                .images
                .iter()
                .map(|url| json!({"type":"input_image","image_url":url})),
        );
        let mut tool =
            json!({"type":"image_generation","action":request.action,"output_format":"png"});
        if let Some(transparent) = request.transparent_background {
            tool["background"] = json!(if transparent { "transparent" } else { "opaque" });
        }
        if let Some(size) = &request.size {
            tool["size"] = json!(size);
        }
        if let Some(quality) = request.quality {
            tool["quality"] = json!(quality);
        }
        let payload = json!({"model":model, "store":false, "tools":[tool],
            "tool_choice":{"type":"image_generation"}, "input":[{"role":"user","content":content}]});
        // An interrupted paid image request may already have run. Send it once.
        let mut response = self
            .http
            .client
            .post(&self.endpoint)
            .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
            .timeout(Duration::from_secs(300))
            .json(&payload)
            .send()
            .await
            .map_err(|err| self.transport("image_request", err.without_url()))?;
        let status = response.status();
        let mut bytes = Vec::new();
        let limit = if status.is_success() {
            MAX_ENCODED_BYTES + 1024 * 1024
        } else {
            64 * 1024
        };
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| self.transport("image_response", err.without_url()))?
        {
            if bytes.len() + chunk.len() > limit {
                return Err(self.transport(
                    "image_response",
                    "Image response exceeded the application size limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() || body["status"] != "completed" {
            let error = &body["error"];
            let message = error["message"]
                .as_str()
                .or(error["code"].as_str())
                .unwrap_or("The provider did not complete the image");
            let key = self
                .authorization
                .to_str()
                .expect("validated bearer")
                .trim_start_matches("Bearer ");
            return Err(InferenceError::HttpStatus {
                endpoint: self.endpoint.clone(),
                status: status.as_u16(),
                body_preview: image_error(message, key),
                retry_after: None,
            });
        }
        let png = body["output"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["type"] == "image_generation_call" && item["status"] == "completed")
            .and_then(|item| item["result"].as_str())
            .filter(|data| !data.is_empty() && data.len() <= MAX_ENCODED_BYTES)
            .ok_or_else(|| {
                self.transport("image_response", "OpenAI returned no completed PNG image")
            })?;
        Ok(png.to_owned())
    }
}

fn image_error(message: &str, key: &str) -> String {
    let redacted = message.replace(key, "[redacted]");
    // Authentication errors can echo only the beginning and end of a secret.
    let words: Vec<_> = redacted
        .split_whitespace()
        .map(|word| {
            if word.contains("sk-") || word.contains("data:image/") || word.len() > 128 {
                "[redacted]"
            } else {
                word
            }
        })
        .collect();
    crate::error::preview(&words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_errors_do_not_echo_credentials_or_pixels() {
        let error = image_error(
            "Incorrect API key: sk-proj-abc***34. token=other-test-credential. data:image/png;base64,iVBOR",
            "other-test-credential",
        );
        assert!(
            !error.contains("abc") && !error.contains("credential") && !error.contains("iVBOR")
        );
    }
}
