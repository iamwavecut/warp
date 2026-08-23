use base64::Engine as _;
use mockito::Matcher;

use super::{
    MAX_TRANSCRIPTION_AUDIO_BYTES, OpenAICompatibleTranscriber, Transcriber, VoiceTranscriber,
};
use crate::ai::agent::api::direct_openai::CustomProviderRoute;
use crate::settings::CustomProviderCapabilities;

fn route(base_url: String, api_key: Option<String>) -> CustomProviderRoute {
    CustomProviderRoute {
        provider_name: "local".to_string(),
        base_url,
        model: "local-whisper".to_string(),
        api_key,
        capabilities: CustomProviderCapabilities {
            transcription: true,
            transcription_model: Some("local-whisper".to_string()),
            ..Default::default()
        },
    }
}

fn wav_bytes() -> Vec<u8> {
    // RIFF/WAVE with a mono 16 kHz PCM format chunk and two samples.
    vec![
        b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 16, 0,
        0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a', b't', b'a', 4,
        0, 0, 0, 0, 0, 0, 0,
    ]
}

fn wav_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(wav_bytes())
}

fn multipart_body_matcher() -> Matcher {
    Matcher::Regex(
        r#"(?s).*name="model".*\r\n\r\nlocal-whisper.*name="file"; filename="audio\.wav".*RIFF.*WAVE.*"#
            .to_string(),
    )
}

#[tokio::test]
async fn keyless_transcription_sends_one_multipart_wav_request() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .match_header("authorization", Matcher::Missing)
        .match_header(
            "content-type",
            Matcher::Regex("multipart/form-data; boundary=.*".into()),
        )
        .match_body(multipart_body_matcher())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"text":"hello local"}"#)
        .expect(1)
        .create_async()
        .await;

    let transcriber = OpenAICompatibleTranscriber::new(route(format!("{}/v1", server.url()), None));
    assert_eq!(
        transcriber.transcribe(wav_base64()).await.unwrap(),
        "hello local"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn keyed_transcription_sends_bearer_authorization_without_persisting_it() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .match_header("authorization", Matcher::Regex("Bearer .+".into()))
        .match_body(multipart_body_matcher())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"text":"keyed local"}"#)
        .expect(1)
        .create_async()
        .await;

    let transcriber = OpenAICompatibleTranscriber::new(route(
        format!("{}/v1", server.url()),
        Some("a-test-only-key-value".to_string()),
    ));
    assert_eq!(
        transcriber.transcribe(wav_base64()).await.unwrap(),
        "keyed local"
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn invalid_audio_is_rejected_before_http_without_retry() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .expect(0)
        .create_async()
        .await;
    let transcriber = OpenAICompatibleTranscriber::new(route(format!("{}/v1", server.url()), None));

    for invalid in [
        "not-base64".to_string(),
        base64::engine::general_purpose::STANDARD.encode(b"not-a-wav"),
        base64::engine::general_purpose::STANDARD
            .encode(vec![0; MAX_TRANSCRIPTION_AUDIO_BYTES + 1]),
    ] {
        let error = transcriber
            .transcribe(invalid)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("audio") || error.contains("WAV"));
    }
    mock.assert_async().await;
}

#[tokio::test]
async fn http_and_payload_errors_are_local_and_not_retried() {
    let mut server = mockito::Server::new_async().await;
    let status_mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .match_body(multipart_body_matcher())
        .with_status(502)
        .expect(1)
        .create_async()
        .await;
    let transcriber = OpenAICompatibleTranscriber::new(route(format!("{}/v1", server.url()), None));
    let error = transcriber
        .transcribe(wav_base64())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("502"));
    status_mock.assert_async().await;

    let malformed_mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .match_body(multipart_body_matcher())
        .with_status(200)
        .with_body("not json")
        .expect(1)
        .create_async()
        .await;
    let error = transcriber
        .transcribe(wav_base64())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("JSON") || error.contains("json"));
    malformed_mock.assert_async().await;

    let missing_text_mock = server
        .mock("POST", "/v1/audio/transcriptions")
        .match_body(multipart_body_matcher())
        .with_status(200)
        .with_body(r#"{}"#)
        .expect(1)
        .create_async()
        .await;
    let error = transcriber
        .transcribe(wav_base64())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing text"));
    missing_text_mock.assert_async().await;
}

#[test]
fn live_transcriber_route_can_be_replaced_and_cleared() {
    let mut transcriber =
        VoiceTranscriber::from_route(Some(route("http://localhost:1234/v1".to_string(), None)));
    assert!(transcriber.transcriber().is_some());

    transcriber.set_route(None);
    assert!(transcriber.transcriber().is_none());

    transcriber.set_route(Some(route("http://localhost:5678/v1".to_string(), None)));
    assert!(transcriber.transcriber().is_some());
}
