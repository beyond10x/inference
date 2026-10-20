//! Hosted adapter of the inference framework for Amazon Bedrock.
//!
//! `inference-aws` is a provider crate: it speaks one AWS service and is enabled explicitly by the
//! caller, never selected by default. Its one module, [`nova_sonic`], is a client for Amazon Nova
//! Sonic, the speech-to-speech model, over Bedrock's `InvokeModelWithBidirectionalStream`.
//! Credentials come from the AWS SDK's default provider chain or from a client the caller builds;
//! the crate never reads a key itself.
//!
//! Behind the `duplex` feature, which `live` enables, [`duplex`] binds the client to the
//! `inference-duplex` contract: Nova Sonic as a native speech model.

#[cfg(feature = "duplex")]
pub mod duplex;
pub mod nova_sonic;
