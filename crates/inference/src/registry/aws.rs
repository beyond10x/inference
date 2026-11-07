//! The `aws` registration: Nova Sonic for the `speech` role, bound without I/O until a session
//! opens (feature `aws`).
//!
//! The model is the role key `model`, else `INFERENCE_NOVA_SONIC_MODEL` of the stack's
//! environment, else Nova 2 Sonic. The binding is `NovaSonicSpeech::deferred`, whose connector
//! loads the AWS configuration when a session opens, so a stack naming `aws` resolves with no
//! credentials and no network.

use inference_aws::duplex::{NovaSonicSpeech, default_session_config};
use inference_aws::nova_sonic::{DEFAULT_MODEL_ID, MODEL_ID_ENV};
use inference_core::model::Locality;

use super::AdapterDescriptor;
use crate::error::Error;
use crate::stack::{Binding, Role};

pub(super) fn descriptor() -> AdapterDescriptor {
    AdapterDescriptor::new("aws", Locality::ExternalProvider)
        .feature("aws")
        .model_default(Some(MODEL_ID_ENV), DEFAULT_MODEL_ID)
        .session_config(default_session_config())
        .role(Role::Speech, |keys| {
            let model = keys
                .get("model")
                .cloned()
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_owned());
            let speech =
                NovaSonicSpeech::deferred(model.clone()).map_err(|error| Error::Provider {
                    adapter: "aws".to_owned(),
                    message: format!("the binding's runtime did not start: {error}"),
                    source: Some(Box::new(error)),
                })?;
            Ok(Binding::speech(speech).model(model).into())
        })
}
