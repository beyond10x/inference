//! The `openai` registration: OpenAI Realtime for the `speech` role (feature `openai`) and, with the
//! feature `openai-live`, GPT-Live as `openai-live`.
//!
//! The model is the role key `model`, else, for `openai`, `INFERENCE_OPENAI_REALTIME_MODEL` of the
//! stack's environment, else the provider's default. The key is read from `OPENAI_API_KEY` when
//! a session opens, never when the stack resolves.

use inference_core::model::Locality;
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    DEFAULT_MODEL, Endpoint, EnvironmentSource, MODEL_VARIABLE, OPENAI_KEY_NAME,
};

use super::AdapterDescriptor;
use crate::stack::{Binding, Role};

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new("openai", Locality::ExternalProvider)
        .feature("openai")
        .model_default(Some(MODEL_VARIABLE), DEFAULT_MODEL)
        .session_config(inference_openai::duplex::default_session_config())
        .role(Role::Speech, |keys| {
            let model = keys
                .get("model")
                .cloned()
                .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
            let speech = OpenAiRealtime::new(
                Endpoint::new(model.clone()),
                Box::new(EnvironmentSource::openai()),
                OPENAI_KEY_NAME,
            );
            Ok(Binding::speech(speech).model(model).into())
        })
}

#[cfg(feature = "openai-live")]
pub(super) fn live_descriptor() -> AdapterDescriptor {
    use inference_openai::live::LIVE_MODEL;
    use inference_openai::live_duplex::OpenAiLive;

    AdapterDescriptor::new("openai-live", Locality::ExternalProvider)
        .feature("openai-live")
        .model_default(None, LIVE_MODEL)
        .session_config(inference_openai::live_duplex::default_session_config())
        .role(Role::Speech, |keys| {
            let model = keys
                .get("model")
                .cloned()
                .unwrap_or_else(|| LIVE_MODEL.to_owned());
            let speech = OpenAiLive::new(
                Endpoint::live(model.clone()),
                Box::new(EnvironmentSource::openai()),
                OPENAI_KEY_NAME,
            );
            Ok(Binding::speech(speech).model(model).into())
        })
}
