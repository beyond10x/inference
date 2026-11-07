//! The `sherpa-onnx` registration: the streaming recogniser of `inference-sherpa-onnx` for the
//! `stt` role (feature `sherpa-onnx`).
//!
//! The key `dir` names the directory of the pinned checkpoint, which is checked and opened when
//! the stack resolves; the binding has a model name only when the key `model` gives one.

use inference_core::model::Locality;
use inference_sherpa_onnx::stt::SherpaOnnxRecogniser;

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, Role};

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new("sherpa-onnx", Locality::SameProcess)
        .feature("sherpa-onnx")
        .role(Role::Stt, |keys| {
            let dir = keys.get("dir").ok_or_else(|| Error::Layer {
                message: "the adapter \"sherpa-onnx\" needs the key dir, the checkpoint \
                          directory: set INFERENCE_STT_DIR, or dir in the [stt] table of the \
                          stack file"
                    .to_owned(),
                source: None,
            })?;
            let recogniser = SherpaOnnxRecogniser::open(dir).map_err(|error| Error::Provider {
                adapter: "sherpa-onnx".to_owned(),
                message: format!("the checkpoint in {dir} did not open: {error}"),
                source: Some(Box::new(error)),
            })?;
            Ok(Binding::stt(recogniser).into())
        })
}
