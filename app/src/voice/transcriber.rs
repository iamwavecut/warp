use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VoiceTranscriberRouteStatus {
    Ready {
        provider_name: String,
        model: String,
    },
    Disabled {
        reason: String,
    },
}

impl VoiceTranscriberRouteStatus {
    pub(crate) fn ready(provider_name: impl Into<String>, model: impl Into<String>) -> Self {
        Self::Ready {
            provider_name: provider_name.into(),
            model: model.into(),
        }
    }

    pub(crate) fn disabled(reason: impl Into<String>) -> Self {
        Self::Disabled {
            reason: reason.into(),
        }
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Self::Ready {
                provider_name,
                model,
            } => format!("Voice input ready: {provider_name} / {model}."),
            Self::Disabled { reason } => reason.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceTranscriberEvent {
    RouteChanged,
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
        let declared_riff_size =
            u32::from_le_bytes(wav[4..8].try_into().expect("WAV RIFF size is four bytes")) as usize;
        if declared_riff_size != wav.len() - 8 {
            anyhow::bail!("local transcription WAV has an invalid RIFF size");
        }

        let mut offset = 12;
        let mut has_format = false;
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
            let padded_end = data_end
                .checked_add(chunk_size % 2)
                .ok_or_else(|| anyhow::anyhow!("local transcription WAV padding is too large"))?;
            if padded_end > wav.len() {
                anyhow::bail!("local transcription WAV has invalid chunk padding");
            }

            match &wav[offset..offset + 4] {
                b"fmt " => {
                    if chunk_size < 16 {
                        anyhow::bail!("local transcription WAV fmt chunk is truncated");
                    }
                    let fmt = &wav[data_start..data_start + chunk_size];
                    let audio_format = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
                    let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                    let sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                    let byte_rate = u32::from_le_bytes(fmt[8..12].try_into().unwrap());
                    let block_align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
                    let bits_per_sample = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
                    let bytes_per_sample = bits_per_sample / 8;
                    let expected_block_align = channels.checked_mul(bytes_per_sample);
                    let expected_byte_rate = sample_rate
                        .checked_mul(u32::from(expected_block_align.unwrap_or_default()));
                    if audio_format != 1
                        || channels == 0
                        || sample_rate == 0
                        || bits_per_sample == 0
                        || bits_per_sample % 8 != 0
                        || expected_block_align != Some(block_align)
                        || expected_byte_rate != Some(byte_rate)
                    {
                        anyhow::bail!("local transcription WAV fmt fields are invalid");
                    }
                    has_format = true;
                }
                b"data" if chunk_size > 0 => has_audio_data = true,
                _ => {}
            }
            offset = padded_end;
        }
        if !has_format {
            anyhow::bail!("local transcription WAV contains no valid fmt chunk");
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
    route: Option<CustomProviderRoute>,
    route_status: VoiceTranscriberRouteStatus,
}

impl VoiceTranscriber {
    pub fn new(transcriber: Arc<dyn Transcriber>) -> Self {
        Self {
            transcriber: Some(transcriber),
            route: None,
            route_status: VoiceTranscriberRouteStatus::ready(
                "local provider",
                "configured transcription model",
            ),
        }
    }

    pub fn disabled() -> Self {
        Self {
            transcriber: None,
            route: None,
            route_status: VoiceTranscriberRouteStatus::disabled(
                "Voice input is disabled: select a configured custom provider model with transcription enabled.",
            ),
        }
    }

    pub(crate) fn from_route(route: Option<CustomProviderRoute>) -> Self {
        match route {
            Some(route) => Self {
                route_status: VoiceTranscriberRouteStatus::ready(
                    route.provider_name.clone(),
                    route.model.clone(),
                ),
                route: Some(route.clone()),
                transcriber: Some(Arc::new(OpenAICompatibleTranscriber::new(route))),
            },
            None => Self::disabled(),
        }
    }

    pub(crate) fn set_route(&mut self, route: Option<CustomProviderRoute>) {
        *self = Self::from_route(route);
    }

    pub(crate) fn set_route_with_status(
        &mut self,
        route: Option<CustomProviderRoute>,
        route_status: VoiceTranscriberRouteStatus,
        ctx: &mut ModelContext<Self>,
    ) {
        let route_changed = self.route != route || self.route_status != route_status;
        self.transcriber = route
            .clone()
            .map(|route| Arc::new(OpenAICompatibleTranscriber::new(route)) as Arc<dyn Transcriber>);
        self.route = route;
        self.route_status = route_status;
        if route_changed {
            ctx.emit(VoiceTranscriberEvent::RouteChanged);
        }
    }

    /// Returns the transcriber if one is set.
    pub fn transcriber(&self) -> Option<&Arc<dyn Transcriber>> {
        self.transcriber.as_ref()
    }

    pub(crate) fn route_status(&self) -> &VoiceTranscriberRouteStatus {
        &self.route_status
    }

    pub(crate) fn route_status_text(&self) -> String {
        self.route_status.text()
    }

    pub(crate) fn route_status_text_for_app(app: &AppContext) -> String {
        Self::as_ref(app).route_status_text()
    }
}

impl Entity for VoiceTranscriber {
    type Event = VoiceTranscriberEvent;
}

impl SingletonEntity for VoiceTranscriber {}

#[cfg(test)]
#[path = "transcriber_tests.rs"]
mod tests;
