//! The `whispercpp` registration: the batch recogniser of `inference-whispercpp` for the `stt`
//! role (feature `whispercpp`).
//!
//! The key `path` names the pinned checkpoint file. Resolving declares the model and the adapter
//! and reads nothing; the checkpoint is checked and opened when a session starts the recogniser
//! ([`BoundRecogniser::open`]), under the adapter resolving declared. The binding has a model
//! name only when the key `model` gives one.

use std::path::PathBuf;

use inference_core::model::{Adapter, Locality, Model};
use inference_whispercpp::stt::{
    DEFAULT_MAX_CONCURRENCY, WhisperCppRecogniser, declare_adapter, declare_model,
};

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, BoundRecogniser, Role};

const NAME: &str = "whispercpp";

/// The recogniser over the checkpoint file at `path`, declared and not yet opened.
struct Deferred {
    path: PathBuf,
    model: Model,
    adapter: Adapter,
    opened: Option<WhisperCppRecogniser>,
}

impl BoundRecogniser for Deferred {
    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn open(&mut self) -> Result<(), Error> {
        if self.opened.is_some() {
            return Ok(());
        }
        let recogniser = WhisperCppRecogniser::open_declared(
            &self.path,
            self.model.clone(),
            self.adapter.clone(),
        )
        .map_err(|error| Error::Provider {
            adapter: NAME.to_owned(),
            message: format!(
                "the checkpoint {} did not open: {error}",
                self.path.display()
            ),
            source: Some(Box::new(error)),
        })?;
        self.opened = Some(recogniser);
        Ok(())
    }
}

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new(NAME, Locality::SameProcess)
        .feature("whispercpp")
        .role(Role::Stt, |keys| {
            let path = keys.get("path").ok_or_else(|| Error::Layer {
                message: "the adapter \"whispercpp\" needs the key path, the checkpoint file: \
                          set INFERENCE_STT_PATH, or path in the [stt] table of the stack file"
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
                path: PathBuf::from(path),
                model,
                adapter,
                opened: None,
            })
            .into())
        })
}
