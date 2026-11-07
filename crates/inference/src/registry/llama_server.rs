//! The `llama-server` registration: the text generator of `inference-llama-server` for the `llm`
//! role (feature `llama-server`).
//!
//! The keys are `url`, the server's OpenAI-compatible base URL on this machine, and `model`, the
//! name it serves (its `--alias`), which is also the binding's model name. Nothing is sent before
//! the first turn.

use std::collections::BTreeMap;

use inference_core::model::{Adapter, Locality, Model};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, Role};

fn key<'a>(keys: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, Error> {
    keys.get(key).map(String::as_str).ok_or_else(|| Error::Layer {
        message: format!(
            "the adapter \"llama-server\" needs the key {key}: set INFERENCE_LLM_{}, or {key} in \
             the [llm] table of the stack file",
            key.to_uppercase()
        ),
        source: None,
    })
}

fn provider(message: String, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Provider {
        adapter: "llama-server".to_owned(),
        message,
        source: Some(Box::new(source)),
    }
}

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new("llama-server", Locality::LocalWorker)
        .feature("llama-server")
        .role(Role::Llm, |keys| {
            let config = Config::new(key(keys, "url")?, key(keys, "model")?);
            let server = LlamaServer::new(&config)
                .map_err(|error| provider(format!("the client was refused: {error}"), error))?;
            let (model, _) = Model::declare(declare_model());
            let (adapter, _) = Adapter::declare(declare_adapter(model.model_id(), &config))
                .map_err(|error| provider(format!("the adapter was refused: {error}"), error))?;
            let generator = server
                .generator(adapter)
                .map_err(|error| provider(format!("the adapter was refused: {error}"), error))?;
            Ok(Binding::llm(generator).model(config.served_model).into())
        })
}
