// The implementation lives in speech.rs; the dual name is intentional.
#[allow(clippy::module_inception)]
pub mod speech;

pub use speech::{test_audio, SpeechToText, TextToSpeech};
