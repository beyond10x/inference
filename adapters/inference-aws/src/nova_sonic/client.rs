//! The binding of a session to Bedrock's `InvokeModelWithBidirectionalStream`.

use std::future::Future;

use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_bedrockruntime::operation::invoke_model_with_bidirectional_stream::InvokeModelWithBidirectionalStreamError;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::{
    InvokeModelWithBidirectionalStreamInputError, InvokeModelWithBidirectionalStreamOutputError,
};
use aws_sdk_bedrockruntime::types::{
    BidirectionalInputPayloadPart, InvokeModelWithBidirectionalStreamInput,
    InvokeModelWithBidirectionalStreamOutput,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;

use super::config::SessionConfig;
use super::error::{NovaSonicError, ServiceError, ServiceErrorKind, redact};
use super::session::{OutputFrame, Session};

/// Input events queued ahead of the service: enough for the opening events and a second of audio.
const INPUT_CAPACITY: usize = 64;

/// Output frames buffered ahead of the reader.
const OUTPUT_CAPACITY: usize = 256;

/// The kind of a failed stream start the service itself refused, with which its
/// [`NovaSonicError::Connect`] message begins.
pub(crate) const SERVICE_ERROR: &str = "the service returned an error";

/// A Nova Sonic client: a Bedrock runtime client and the model id it invokes.
#[derive(Debug, Clone)]
pub struct NovaSonic {
    client: Client,
    model_id: String,
}

impl NovaSonic {
    /// A client over `client`, whose credentials and region the caller chose.
    pub fn new(client: Client, model_id: impl Into<String>) -> Self {
        NovaSonic {
            client,
            model_id: model_id.into(),
        }
    }

    /// A client from the AWS SDK's default chain: credentials from the environment, profile or
    /// single sign-on, the region from `AWS_REGION` or the profile, and the model id of
    /// [`super::model_id_from_env`].
    pub async fn from_env() -> Self {
        let config = aws_config::load_from_env().await;
        NovaSonic::new(Client::new(&config), super::model_id_from_env())
    }

    /// The model id this client invokes.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Opens a bidirectional stream and a session over it.
    ///
    /// The opening events are queued before the request is sent, so they are the first frames of
    /// the stream. A task of the current Tokio runtime forwards the service's output to the
    /// session's reader until the stream ends or the reader is dropped.
    ///
    /// A failure to open is a [`NovaSonicError::Connect`] whose message names the failure's kind
    /// and, for an error the service returned, its error code: never the SDK's error chain, which
    /// can name the caller's profile, single sign-on session or files.
    pub async fn start(&self, config: &SessionConfig) -> Result<Session, NovaSonicError> {
        let (input, queued) = mpsc::channel::<Vec<u8>>(INPUT_CAPACITY);
        let (frames, output) = mpsc::channel::<OutputFrame>(OUTPUT_CAPACITY);
        let session = Session::open(config, input, output).await?;
        let body = ReceiverStream::new(queued).map(|bytes| {
            Ok::<_, InvokeModelWithBidirectionalStreamInputError>(
                InvokeModelWithBidirectionalStreamInput::Chunk(
                    BidirectionalInputPayloadPart::builder()
                        .bytes(Blob::new(bytes))
                        .build(),
                ),
            )
        });
        let response = self
            .client
            .invoke_model_with_bidirectional_stream()
            .model_id(&self.model_id)
            .body(body.into())
            .send()
            .await
            .map_err(|error| NovaSonicError::Connect(connect_message(&error)))?;
        tokio::spawn(bridge(
            SdkSource {
                receiver: response.body,
                done: false,
            },
            frames,
        ));
        Ok(session)
    }
}

/// Where the bridge reads output frames from: the SDK's event receiver, or a test's channel.
pub(crate) trait FrameSource: Send {
    /// The next frame; `None` once the source ended.
    fn next(&mut self) -> impl Future<Output = Option<OutputFrame>> + Send;
}

struct SdkSource {
    receiver: EventReceiver<
        InvokeModelWithBidirectionalStreamOutput,
        InvokeModelWithBidirectionalStreamOutputError,
    >,
    done: bool,
}

impl FrameSource for SdkSource {
    async fn next(&mut self) -> Option<OutputFrame> {
        if self.done {
            return None;
        }
        loop {
            match self.receiver.recv().await {
                Ok(Some(InvokeModelWithBidirectionalStreamOutput::Chunk(part))) => {
                    if let Some(bytes) = part.bytes {
                        return Some(Ok(bytes.into_inner()));
                    }
                }
                Ok(Some(_)) => {
                    return Some(Err(ServiceError::new(
                        ServiceErrorKind::Other("UnknownOutput".into()),
                        "an output stream member this SDK does not know",
                    )));
                }
                Ok(None) => return None,
                Err(error) => {
                    self.done = true;
                    return Some(Err(service_error(error)));
                }
            }
        }
    }
}

/// Forwards `source` into `frames` until the source ends or the reader is gone. A dropped reader
/// (or session) ends it even while the service is silent, which drops the SDK's receiver and with
/// it the stream.
pub(crate) async fn bridge<S: FrameSource>(mut source: S, frames: mpsc::Sender<OutputFrame>) {
    loop {
        let next = tokio::select! {
            () = frames.closed() => break,
            next = source.next() => next,
        };
        let Some(frame) = next else { break };
        if frames.send(frame).await.is_err() {
            break;
        }
    }
}

/// What kind of failure an SDK error that is not a service error is, without any of its text: the
/// error chain of a credential or connector failure can name the caller's profile, single sign-on
/// session or files.
fn failure_kind<E, R>(error: &SdkError<E, R>) -> &'static str {
    match error {
        SdkError::ConstructionFailure(_) => "the request could not be built",
        SdkError::TimeoutError(_) => "the request timed out",
        SdkError::DispatchFailure(failure) if failure.is_timeout() => {
            "dispatch failure: connection timeout"
        }
        SdkError::DispatchFailure(failure) if failure.is_io() => {
            "dispatch failure: connection error"
        }
        SdkError::DispatchFailure(failure) if failure.is_user() => {
            "dispatch failure: invalid request, credentials or configuration"
        }
        SdkError::DispatchFailure(_) => {
            "dispatch failure: credentials, configuration or connection"
        }
        SdkError::ResponseError(_) => "the response could not be read",
        SdkError::ServiceError(_) => SERVICE_ERROR,
        _ => "an unknown failure",
    }
}

/// An error code the service sent, passed through [`redact`].
fn code(error: &impl ProvideErrorMetadata) -> String {
    redact(error.code().unwrap_or("Unhandled"))
}

/// The message the service itself sent with an error, passed through [`redact`] and prefixed with
/// `: `, or nothing. Text the service sends names no local profile or file (those come only from
/// failures raised on this side, which never carry their text); ARNs and account ids are redacted.
fn said_by_service(error: &(impl ProvideErrorMetadata + std::fmt::Display)) -> String {
    let message = error
        .message()
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string());
    if message.is_empty() {
        String::new()
    } else {
        format!(": {}", redact(&message))
    }
}

/// The message of a failed stream start: the failure's kind and, for an error the service
/// returned, its error code and the service's own message.
fn connect_message<R>(error: &SdkError<InvokeModelWithBidirectionalStreamError, R>) -> String {
    use InvokeModelWithBidirectionalStreamError as E;
    let SdkError::ServiceError(context) = error else {
        return failure_kind(error).to_owned();
    };
    let code = match context.err() {
        E::AccessDeniedException(_) => "AccessDeniedException".to_owned(),
        E::InternalServerException(_) => "InternalServerException".to_owned(),
        E::ModelErrorException(_) => "ModelErrorException".to_owned(),
        E::ModelNotReadyException(_) => "ModelNotReadyException".to_owned(),
        E::ModelStreamErrorException(_) => "ModelStreamErrorException".to_owned(),
        E::ModelTimeoutException(_) => "ModelTimeoutException".to_owned(),
        E::ResourceNotFoundException(_) => "ResourceNotFoundException".to_owned(),
        E::ServiceQuotaExceededException(_) => "ServiceQuotaExceededException".to_owned(),
        E::ServiceUnavailableException(_) => "ServiceUnavailableException".to_owned(),
        E::ThrottlingException(_) => "ThrottlingException".to_owned(),
        E::ValidationException(_) => "ValidationException".to_owned(),
        other => code(other),
    };
    format!(
        "{}: {code}{}",
        failure_kind(error),
        said_by_service(context.err())
    )
}

/// The service error a failed read of the output stream reports: its kind, and as its message the
/// error code with the service's own message, or the kind of a failure raised on this side, never
/// that failure's text.
fn service_error<R>(
    error: SdkError<InvokeModelWithBidirectionalStreamOutputError, R>,
) -> ServiceError {
    use InvokeModelWithBidirectionalStreamOutputError as E;
    let SdkError::ServiceError(context) = &error else {
        return ServiceError::new(
            ServiceErrorKind::Other("Transport".into()),
            failure_kind(&error),
        );
    };
    let (kind, code) = match context.err() {
        E::ValidationException(_) => (
            ServiceErrorKind::Validation,
            "ValidationException".to_owned(),
        ),
        E::ThrottlingException(_) => (
            ServiceErrorKind::Throttling,
            "ThrottlingException".to_owned(),
        ),
        E::ModelTimeoutException(_) => (
            ServiceErrorKind::ModelTimeout,
            "ModelTimeoutException".to_owned(),
        ),
        E::ModelStreamErrorException(_) => (
            ServiceErrorKind::ModelStreamError,
            "ModelStreamErrorException".to_owned(),
        ),
        E::InternalServerException(_) => (
            ServiceErrorKind::InternalServer,
            "InternalServerException".to_owned(),
        ),
        E::ServiceUnavailableException(_) => (
            ServiceErrorKind::ServiceUnavailable,
            "ServiceUnavailableException".to_owned(),
        ),
        other => {
            let code = code(other);
            (ServiceErrorKind::Other(code.clone()), code)
        }
    };
    ServiceError::new(kind, format!("{code}{}", said_by_service(context.err())))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aws_sdk_bedrockruntime::error::ConnectorError;
    use aws_sdk_bedrockruntime::types::error::{AccessDeniedException, ValidationException};

    use super::*;

    const SECRET: &str = "operator-private-profile";

    fn failures<E>(service: E) -> Vec<(&'static str, SdkError<E, ()>)> {
        let said = || format!("profile `{SECRET}` in /somewhere/{SECRET}/cache");
        vec![
            ("construction", SdkError::construction_failure(said())),
            ("timeout", SdkError::timeout_error(said())),
            (
                "dispatch other",
                SdkError::dispatch_failure(ConnectorError::other(said().into(), None)),
            ),
            (
                "dispatch io",
                SdkError::dispatch_failure(ConnectorError::io(said().into())),
            ),
            ("response", SdkError::response_error(said(), ())),
            ("service", SdkError::service_error(service, ())),
        ]
    }

    #[test]
    fn connect_messages_carry_no_text_of_the_error_chain() {
        let service = InvokeModelWithBidirectionalStreamError::AccessDeniedException(
            AccessDeniedException::builder()
                .message("arn:aws:sts::123456789012:assumed-role/r/s is not authorized")
                .build(),
        );
        for (case, error) in failures(service) {
            let message = connect_message(&error);
            assert!(!message.contains(SECRET), "{case}: {message}");
            assert!(!message.contains("somewhere"), "{case}: {message}");
            assert!(!message.contains("123456789012"), "{case}: {message}");
            assert!(!message.is_empty(), "{case}");
            if case == "service" {
                assert!(message.contains("AccessDeniedException"), "{message}");
                assert!(message.contains("is not authorized"), "{message}");
            }
        }
        let service = InvokeModelWithBidirectionalStreamError::AccessDeniedException(
            AccessDeniedException::builder().build(),
        );
        let message = connect_message(&SdkError::<_, ()>::service_error(service, ()));
        assert!(message.contains("AccessDeniedException"), "{message}");
    }

    #[test]
    fn stream_errors_carry_no_text_of_the_error_chain() {
        let service = InvokeModelWithBidirectionalStreamOutputError::ValidationException(
            ValidationException::builder()
                .message("model arn:aws:bedrock:us-east-1:123456789012:x is not valid")
                .build(),
        );
        for (case, error) in failures(service) {
            let error = service_error(error);
            assert!(!error.message.contains(SECRET), "{case}: {error}");
            assert!(!error.to_string().contains(SECRET), "{case}: {error}");
            assert!(!error.message.contains("somewhere"), "{case}: {error}");
            assert!(!error.message.contains("123456789012"), "{case}: {error}");
            if case == "service" {
                assert_eq!(error.kind, ServiceErrorKind::Validation);
                assert!(error.message.contains("ValidationException"), "{error}");
                assert!(error.message.contains("is not valid"), "{error}");
            }
        }
    }

    struct Silent(mpsc::Receiver<OutputFrame>);

    impl FrameSource for Silent {
        async fn next(&mut self) -> Option<OutputFrame> {
            self.0.recv().await
        }
    }

    #[tokio::test]
    async fn the_bridge_ends_when_the_reader_is_dropped() {
        let (_service, silent) = mpsc::channel(1);
        let (frames, output) = mpsc::channel(4);
        let task = tokio::spawn(bridge(Silent(silent), frames));
        drop(output);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the bridge ends once the reader is gone, though the service is silent")
            .unwrap();
    }

    #[tokio::test]
    async fn the_bridge_ends_when_the_session_is_dropped() {
        let (_service, silent) = mpsc::channel(1);
        let (frames, output) = mpsc::channel(4);
        let (input, _sent) = mpsc::channel(64);
        let session = Session::open(&SessionConfig::new("x"), input, output)
            .await
            .unwrap();
        let task = tokio::spawn(bridge(Silent(silent), frames));
        drop(session);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the bridge ends once the session is gone, though the service is silent")
            .unwrap();
    }
}
