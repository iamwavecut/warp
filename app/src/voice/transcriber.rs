use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use warpui::{Entity, SingletonEntity};

use crate::ai::agent::api::direct_openai::CustomProviderRoute;

#[derive(thiserror::Error, Debug)]
pub enum TranscribeError {
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub(crate) const MAX_TRANSCRIPTION_AUDIO_BYTES: usize = 25 * 1024 * 1024;

/// Interface for transcribing voice input.
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait Transcriber: Send + Sync {
    /// Transcribe the given base64 encoded wav file into text.
    /// This is expected to be async and called off the main thread.
    async fn transcribe(&self, wav_base64: String) -> Result<String, TranscribeError>;
}

/// Direct OpenAI-compatible transcription adapter for the currently selected
/// custom provider. The route has already resolved the explicit transcription
/// model and optional key, so this adapter only owns the multipart request.
pub(crate) struct OpenAICompatibleTranscriber {
    route: CustomProviderRoute,
}

impl OpenAICompatibleTranscriber {
    pub(crate) fn new(route: CustomProviderRoute) -> Self {
        Self { route }
    }

    fn decode_wav(wav_base64: &str) -> anyhow::Result<Vec<u8>> {
        let wav = base64::engine::general_purpose::STANDARD
            .decode(wav_base64)
            .context("local transcription audio is not valid base64")?;
        if wav.is_empty() || wav.len() > MAX_TRANSCRIPTION_AUDIO_BYTES {
            anyhow::bail!("local transcription audio is empty or exceeds the size limit");
        }
        if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
            anyhow::bail!("local transcription audio is not a WAV file");
        }

        let mut offset = 12;
        let mut has_audio_data = false;
        while offset < wav.len() {
            if wav.len() - offset < 8 {
                anyhow::bail!("local transcription WAV has a truncated chunk header");
            }
            let chunk_size = u32::from_le_bytes(
                wav[offset + 4..offset + 8]
                    .try_into()
                    .expect("WAV chunk size is four bytes"),
            ) as usize;
            let data_start = offset + 8;
            let data_end = data_start
                .checked_add(chunk_size)
                .ok_or_else(|| anyhow::anyhow!("local transcription WAV chunk is too large"))?;
            if data_end > wav.len() {
                anyhow::bail!("local transcription WAV has a truncated chunk");
            }
            if &wav[offset..offset + 4] == b"data" && chunk_size > 0 {
                has_audio_data = true;
            }
            offset = data_end + (chunk_size % 2);
            if offset > wav.len() {
                anyhow::bail!("local transcription WAV has invalid chunk padding");
            }
        }
        if !has_audio_data {
            anyhow::bail!("local transcription WAV contains no audio data");
        }
        Ok(wav)
    }
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: Option<String>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Transcriber for OpenAICompatibleTranscriber {
    async fn transcribe(&self, wav_base64: String) -> Result<String, TranscribeError> {
        let wav = Self::decode_wav(&wav_base64)?;

        #[cfg(target_family = "wasm")]
        {
            let _ = wav;
            return Err(anyhow::anyhow!("local transcription is unavailable on wasm").into());
        }

        #[cfg(not(target_family = "wasm"))]
        {
            let audio = reqwest::multipart::Part::bytes(wav)
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .context("failed to build local transcription audio part")?;
            let form = reqwest::multipart::Form::new()
                .text("model", self.route.model.clone())
                .part("file", audio);
            let url = format!(
                "{}/audio/transcriptions",
                self.route.base_url.trim_end_matches('/')
            );
            let client = reqwest::Client::new();
            let mut request = client.post(url).multipart(form);
            if let Some(api_key) = self
                .route
                .api_key
                .as_deref()
                .filter(|api_key| !api_key.trim().is_empty())
            {
                request = request.bearer_auth(api_key);
            }
            let response = request
                .send()
                .await
                .context("failed to send local transcription request")?;
            let status = response.status();
            if !status.is_success() {
                return Err(
                    anyhow::anyhow!("local transcription endpoint returned HTTP {status}").into(),
                );
            }
            let payload: TranscriptionResponse = response
                .json()
                .await
                .context("failed to decode local transcription JSON")?;
            let text = payload
                .text
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("local transcription response is missing text"))?;
            Ok(text.trim().to_owned())
        }
    }
}

/// A voice transcriber that is enabled or disabled.
///
/// This is a singleton model that the app can decide to enable or disable.
/// The editor does expect that it will exist as a singleton fetchable from app context
/// either way though, and depending on whether the optional transcriber is set,
/// the editor considers transcriber to be enabled or disabled.
///
/// We set it up this way to avoid the editor having a direct dependency on any server api.
pub struct VoiceTranscriber {
    /// The transcriber to use. If `None`, the transcriber is disabled.
    #[cfg_attr(not(feature = "voice_input"), allow(dead_code))]
    transcriber: Option<Arc<dyn Transcriber>>,
}

impl VoiceTranscriber {
    pub fn new(transcriber: Arc<dyn Transcriber>) -> Self {
        Self {
            transcriber: Some(transcriber),
        }
    }

    pub fn disabled() -> Self {
        Self { transcriber: None }
    }

    pub(crate) fn from_route(route: Option<CustomProviderRoute>) -> Self {
        match route {
            Some(route) => Self::new(Arc::new(OpenAICompatibleTranscriber::new(route))),
            None => Self::disabled(),
        }
    }

    pub(crate) fn set_route(&mut self, route: Option<CustomProviderRoute>) {
        *self = Self::from_route(route);
    }

    /// Returns the transcriber if one is set.
    pub fn transcriber(&self) -> Option<&Arc<dyn Transcriber>> {
        self.transcriber.as_ref()
    }
}

impl Entity for VoiceTranscriber {
    type Event = ();
}

impl SingletonEntity for VoiceTranscriber {}

#[cfg(test)]
#[path = "transcriber_tests.rs"]
mod tests;
