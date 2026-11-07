//! The `whispercpp` registration: the batch recogniser of `inference-whispercpp` for the `stt`
//! role (feature `whispercpp`).
//!
//! The key `path` names the pinned checkpoint file, which is checked and opened when the stack
//! resolves; the binding has a model name only when the key `model` gives one.

use inference_core::model::Locality;
use inference_whispercpp::stt::WhisperCppRecogniser;

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, Role};

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new("whispercpp", Locality::SameProcess)
        .feature("whispercpp")
        .role(Role::Stt, |keys| {
            let path = keys.get("path").ok_or_else(|| Error::Layer {
                message: "the adapter \"whispercpp\" needs the key path, the checkpoint file: \
                          set INFERENCE_STT_PATH, or path in the [stt] table of the stack file"
                    .to_owned(),
                source: None,
            })?;
            let recogniser = WhisperCppRecogniser::open(path).map_err(|error| Error::Provider {
                adapter: "whispercpp".to_owned(),
                message: format!("the checkpoint {path} did not open: {error}"),
                source: Some(Box::new(error)),
            })?;
            Ok(Binding::stt(recogniser).into())
        })
}
