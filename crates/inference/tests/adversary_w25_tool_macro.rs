//! Adversarial cases for `#[inference::tool]`: the schema a macro tool carries must be one the
//! `ToolAuthority` that governs its calls prepares with, and the tool's name must be the name the
//! function is written with.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference::core::envelope::ControlKind;
use inference::core::id::RunId;
use inference::pipeline::NodeImplementation;
use inference::pipeline::nodes::{ToolAuthority, ToolCallUpdate, ToolPayload};
use inference::tools::{Tool, ToolExecutor};

/// The weather in a city.
#[inference::tool]
async fn weather(city: String) -> String {
    format!("sunny in {city}")
}

/// Adds two whole numbers.
#[inference::tool]
async fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Sets the volume, from 0 to 100.
#[inference::tool(effect = "idempotent_write")]
async fn set_volume(level: u8) -> u8 {
    level
}

/// Scales a number.
#[inference::tool]
async fn scale(factor: f64) -> f64 {
    factor * 2.0
}

/// A place on the map.
#[derive(
    inference::__private::serde::Deserialize, inference::__private::schemars::JsonSchema,
)]
#[serde(crate = "inference::__private::serde")]
#[schemars(crate = "inference::__private::schemars")]
struct Place {
    city: String,
    country: String,
}

/// The weather at a place.
#[inference::tool]
async fn forecast(place: Place) -> String {
    format!("sunny in {}, {}", place.city, place.country)
}

/// Moves the robot forward.
#[inference::tool(effect = "non_idempotent_write")]
async fn r#move(steps: String) -> String {
    steps
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other @ Self::Control(_) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

fn pair(tool: impl Into<(Tool, Box<dyn ToolExecutor>)>) -> (Tool, Box<dyn ToolExecutor>) {
    tool.into()
}

/// What `ToolAuthority::prepare` answers for an authority over the one macro tool `tool`.
fn prepared(tool: Tool) -> Result<(), String> {
    let mut authority = ToolAuthority::new(RunId::generate(), [tool]);
    block_on(NodeImplementation::<Payload>::prepare(&mut authority)).map_err(|e| e.to_string())
}

#[test]
fn control_a_macro_tool_with_a_string_argument_prepares_under_the_authority() {
    assert_eq!(prepared(pair(weather).0), Ok(()));
}

#[test]
fn a_macro_tool_with_integer_arguments_prepares_under_the_authority() {
    assert_eq!(prepared(pair(add).0), Ok(()), "{}", pair(add).0.schema());
}

#[test]
fn a_macro_tool_with_an_unsigned_argument_prepares_under_the_authority() {
    assert_eq!(
        prepared(pair(set_volume).0),
        Ok(()),
        "{}",
        pair(set_volume).0.schema()
    );
}

#[test]
fn a_macro_tool_with_a_float_argument_prepares_under_the_authority() {
    assert_eq!(prepared(pair(scale).0), Ok(()), "{}", pair(scale).0.schema());
}

#[test]
fn a_macro_tool_with_a_struct_argument_prepares_under_the_authority() {
    assert_eq!(
        prepared(pair(forecast).0),
        Ok(()),
        "{}",
        pair(forecast).0.schema()
    );
}

#[test]
fn a_macro_tool_written_with_a_raw_identifier_is_named_without_the_prefix() {
    assert_eq!(pair(r#move).0.name(), "move");
}
