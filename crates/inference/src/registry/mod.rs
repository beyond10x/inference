//! The adapter registry: a constructor per adapter name, registered for each adapter whose feature
//! is on, and open to an application's own adapters (`docs/design/sdk.md` §4.4).

#[cfg(feature = "aws")]
mod aws;
#[cfg(feature = "llama-server")]
mod llama_server;
#[cfg(feature = "testing")]
mod mock;
#[cfg(feature = "openai")]
mod openai;
#[cfg(feature = "sherpa-onnx")]
mod sherpa_onnx;
#[cfg(feature = "whispercpp")]
mod whispercpp;
