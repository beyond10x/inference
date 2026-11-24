//! What a synthesis asks of the backend: the audio of one text, handed over part by part.

use sherpa_onnx::{GenerationConfig, OfflineTts};

/// Takes each part of the audio of one text as it is generated, mono f32 samples at the engine's
/// rate, and answers whether to go on: `false` stops the generation.
pub type Parts = Box<dyn FnMut(&[f32]) -> bool + Send>;

/// The backend of a synthesiser: sherpa-onnx's offline text-to-speech, or a double in this
/// crate's tests.
pub trait Engine: Send + Sync {
    /// Generates the audio of `text` in the voice `speaker` and hands each part to `parts` as it
    /// is made, until `parts` answers `false`. Returns whether the backend generated: `false` is
    /// its refusal.
    fn generate(&self, text: &str, speaker: i32, parts: Parts) -> bool;
}

/// sherpa-onnx's offline text-to-speech.
pub struct Native(pub OfflineTts);

impl Engine for Native {
    fn generate(&self, text: &str, speaker: i32, mut parts: Parts) -> bool {
        if text.contains('\0') {
            return false;
        }
        let config = GenerationConfig {
            sid: speaker,
            ..GenerationConfig::default()
        };
        self.0
            .generate_with_config(
                text,
                &config,
                Some(move |samples: &[f32], _progress: f32| parts(samples)),
            )
            .is_some()
    }
}
