//! The `sherpa-onnx` registration: the streaming recogniser of `inference-sherpa-onnx` for the
//! `stt` role (feature `sherpa-onnx`).
//!
//! The key `dir` names the directory of the pinned checkpoint. Resolving declares the model and
//! the adapter and reads nothing; the checkpoint is checked and opened when a session starts the
//! recogniser ([`BoundRecogniser::open`]), under the adapter resolving declared. The binding has
//! a model name only when the key `model` gives one.

use std::path::PathBuf;

use inference_core::model::{Adapter, Locality, Model};
use inference_sherpa_onnx::stt::{
    DEFAULT_MAX_CONCURRENCY, SherpaOnnxRecogniser, declare_adapter, declare_model,
};

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, BoundRecogniser, Role};

const NAME: &str = "sherpa-onnx";

/// The recogniser over the checkpoint in `dir`, declared and not yet opened.
struct Deferred {
    dir: PathBuf,
    model: Model,
    adapter: Adapter,
    opened: Option<SherpaOnnxRecogniser>,
}

impl BoundRecogniser for Deferred {
    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn open(&mut self) -> Result<(), Error> {
        if self.opened.is_some() {
            return Ok(());
        }
        let recogniser = SherpaOnnxRecogniser::open_declared(
            &self.dir,
            self.model.clone(),
            self.adapter.clone(),
        )
        .map_err(|error| Error::Provider {
            adapter: NAME.to_owned(),
            message: format!(
                "the checkpoint in {} did not open: {error}",
                self.dir.display()
            ),
            source: Some(Box::new(error)),
        })?;
        self.opened = Some(recogniser);
        Ok(())
    }
}

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new(NAME, Locality::SameProcess)
        .feature("sherpa-onnx")
        .role(Role::Stt, |keys| {
            let dir = keys.get("dir").ok_or_else(|| Error::Layer {
                message: "the adapter \"sherpa-onnx\" needs the key dir, the checkpoint \
                          directory: set INFERENCE_STT_DIR, or dir in the [stt] table of the \
                          stack file"
                    .to_owned(),
                source: None,
            })?;
            let (model, _) = declare_model();
            let (adapter, _) =
                declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY).map_err(|error| {
                    Error::Provider {
                        adapter: NAME.to_owned(),
                        message: format!("the adapter was not declared: {error}"),
                        source: Some(Box::new(error)),
                    }
                })?;
            Ok(Binding::deferred_stt(Deferred {
                dir: PathBuf::from(dir),
                model,
                adapter,
                opened: None,
            })
            .into())
        })
}
