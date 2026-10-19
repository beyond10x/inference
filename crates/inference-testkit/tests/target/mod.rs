//! The in-process [`ConformanceTarget`] of `inference-testkit`.
//!
//! Each [`SemanticCommandRequest`], [`SemanticViewRequest`] and [`EventObservationRequest`] goes to
//! the module of the domain its name starts with, one module per domain of `ess/system.yaml`. A
//! module answers from the stores of `inference-testkit` and never from bookkeeping of its own; a
//! name it does not answer yet is [`TargetError::Unsupported`], never faked. Every scenario gets
//! fresh [`Stores`].
//!
//! The suite writes every number of a request as a float, so an ESS `Integer` arrives as `1.0`.
//! [`integer`] reads it back as an `i64` and refuses a fractional or out-of-range value as an
//! error, never as a pass; no request is deserialized whole into an `inference-core` type.

mod graph;
mod model;
mod policy;
mod session;
mod stream;
mod tools;

use std::cell::RefCell;
use std::collections::BTreeMap;

use ess_conformance::target::{
    ConformanceTarget, EventObservationRequest, ExternalOutcomeControl, ImplementationIdentity,
    ObservedEvent, RedeliveryRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_primitives::facts::Number;
use ess_primitives::node::Node;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The stores of one scenario, one field per domain module.
///
/// A module that answers nothing yet holds a unit state, which nothing reads until its story fills
/// it.
#[allow(dead_code)]
#[derive(Default)]
pub struct Stores {
    pub model: model::State,
    pub policy: policy::State,
    pub graph: graph::State,
    pub stream: stream::State,
    pub tools: tools::State,
    pub session: session::State,
}

/// The target the conformance test runs the suite against.
#[derive(Default)]
pub struct Target {
    stores: RefCell<Stores>,
}

/// The domain a semantic name belongs to: its first two segments, `inference.model` for
/// `inference.model.DeclareModel`.
fn domain(name: &str) -> &str {
    let mut dots = name.match_indices('.').map(|(at, _)| at);
    match (dots.next(), dots.next()) {
        (Some(_), Some(second)) => &name[..second],
        _ => name,
    }
}

impl ConformanceTarget for Target {
    fn identity(&self) -> Result<ImplementationIdentity, TargetError> {
        Ok(ImplementationIdentity::new(
            "inference-testkit",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    fn begin_scenario(&self, _scenario: &ScenarioContext) -> Result<(), TargetError> {
        *self.stores.borrow_mut() = Stores::default();
        Ok(())
    }

    fn execute_command(
        &self,
        request: SemanticCommandRequest,
    ) -> Result<SemanticCommandResult, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        let name = request.command.to_string();
        match domain(&name) {
            model::DOMAIN => model::execute_command(stores, &request),
            policy::DOMAIN => policy::execute_command(stores, &request),
            graph::DOMAIN => graph::execute_command(stores, &request),
            stream::DOMAIN => stream::execute_command(stores, &request),
            tools::DOMAIN => tools::execute_command(stores, &request),
            session::DOMAIN => session::execute_command(stores, &request),
            _ => Err(unsupported(&name)),
        }
    }

    fn query_view(&self, request: SemanticViewRequest) -> Result<SemanticViewResult, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        let name = request.view.to_string();
        match domain(&name) {
            model::DOMAIN => model::query_view(stores, &request),
            policy::DOMAIN => policy::query_view(stores, &request),
            graph::DOMAIN => graph::query_view(stores, &request),
            stream::DOMAIN => stream::query_view(stores, &request),
            tools::DOMAIN => tools::query_view(stores, &request),
            session::DOMAIN => session::query_view(stores, &request),
            _ => Err(unsupported(&name)),
        }
    }

    fn observe_events(
        &self,
        request: EventObservationRequest,
    ) -> Result<Vec<ObservedEvent>, TargetError> {
        let stores = &mut *self.stores.borrow_mut();
        let name = request.event.to_string();
        match domain(&name) {
            model::DOMAIN => model::observe_events(stores, &request),
            policy::DOMAIN => policy::observe_events(stores, &request),
            graph::DOMAIN => graph::observe_events(stores, &request),
            stream::DOMAIN => stream::observe_events(stores, &request),
            tools::DOMAIN => tools::observe_events(stores, &request),
            session::DOMAIN => session::observe_events(stores, &request),
            _ => Err(unsupported(&name)),
        }
    }

    fn configure_external_outcome(
        &self,
        request: ExternalOutcomeControl,
    ) -> Result<(), TargetError> {
        Err(TargetError::unsupported(
            format!("forcing `{}`", request.force),
            "no outcome of the stores is declared external",
        ))
    }

    fn redeliver_event(&self, request: RedeliveryRequest) -> Result<(), TargetError> {
        Err(TargetError::unsupported(
            format!("delivering `{}` again", request.event),
            "the stores run no binding",
        ))
    }

    fn end_scenario(&self, _scenario: &ScenarioContext) -> Result<(), TargetError> {
        *self.stores.borrow_mut() = Stores::default();
        Ok(())
    }
}

/// The answer for a semantic name no module answers yet.
pub fn unsupported(name: &str) -> TargetError {
    TargetError::unsupported(
        format!("`{name}`"),
        "the testkit target does not answer this name yet; ess/SKIPPED.md names the story that will",
    )
}

/// A request the target cannot read, which the runner records as an error.
pub fn unreadable(what: impl Into<String>, detail: impl Into<String>) -> TargetError {
    TargetError::unavailable(what, detail)
}

/// The request's input fields, read one at a time; every field must be taken, and
/// [`Input::finish`] refuses any left over.
pub struct Input<'a> {
    command: String,
    fields: BTreeMap<&'a str, &'a Node>,
}

impl<'a> Input<'a> {
    pub fn of(request: &'a SemanticCommandRequest) -> Self {
        Self {
            command: request.command.to_string(),
            fields: request
                .input
                .iter()
                .map(|(name, value)| (name.as_str(), value))
                .collect(),
        }
    }

    fn take(&mut self, field: &str) -> Result<&'a Node, TargetError> {
        self.fields.remove(field).ok_or_else(|| {
            unreadable(
                format!("reading `{}`", self.command),
                format!("the input carries no `{field}`"),
            )
        })
    }

    fn refuse(&self, field: &str, expected: &str, found: &Node) -> TargetError {
        unreadable(
            format!("reading `{}`", self.command),
            format!("`{field}` is not {expected}: {found:?}"),
        )
    }

    /// An ESS `String`.
    pub fn text(&mut self, field: &str) -> Result<String, TargetError> {
        match self.take(field)? {
            Node::Text(text) => Ok(text.clone()),
            other => Err(self.refuse(field, "a string", other)),
        }
    }

    /// An ESS `Integer`, from an integral number within `i64`.
    pub fn integer(&mut self, field: &str) -> Result<i64, TargetError> {
        let node = self.take(field)?;
        integer(node).ok_or_else(|| self.refuse(field, "an integer within i64", node))
    }

    /// A value whose `inference-core` type reads it from its ESS string: an identifier or an enum
    /// variant.
    pub fn named<T: DeserializeOwned>(&mut self, field: &str) -> Result<T, TargetError> {
        let node = self.take(field)?;
        named(node).ok_or_else(|| self.refuse(field, "a declared value", node))
    }

    /// An ESS `List` of values read as [`Input::named`] reads one.
    pub fn named_list<T: DeserializeOwned>(&mut self, field: &str) -> Result<Vec<T>, TargetError> {
        let node = self.take(field)?;
        let Node::Seq(items) = node else {
            return Err(self.refuse(field, "a list", node));
        };
        items
            .iter()
            .map(|item| {
                named(item).ok_or_else(|| self.refuse(field, "a list of declared values", item))
            })
            .collect()
    }

    /// Refuses a field the command does not declare.
    pub fn finish(self) -> Result<(), TargetError> {
        match self.fields.keys().next() {
            None => Ok(()),
            Some(extra) => Err(unreadable(
                format!("reading `{}`", self.command),
                format!("the input carries the undeclared field `{extra}`"),
            )),
        }
    }
}

/// An ESS `Integer` read from a wire number: `1.0` is `1`; `1.5` and anything beyond `i64` are
/// none.
pub fn integer(node: &Node) -> Option<i64> {
    match node {
        Node::Number(number) => number.as_i64(),
        _ => None,
    }
}

fn named<T: DeserializeOwned>(node: &Node) -> Option<T> {
    match node {
        Node::Text(text) => serde_json::from_value(serde_json::Value::String(text.clone())).ok(),
        _ => None,
    }
}

/// The node of an ESS `String`.
pub fn text_node(text: &str) -> Node {
    Node::Text(text.to_owned())
}

/// The node of an ESS `Integer`, built from the `i64` itself.
pub fn integer_node(value: i64) -> Node {
    Node::Number(Number::from(value))
}

/// The node of a value an `inference-core` type serializes as one string: an identifier or an
/// enum variant.
pub fn named_node<T: Serialize>(value: &T) -> Result<Node, TargetError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(Node::Text(text)),
        other => Err(unreadable(
            "writing a declared value",
            format!("it does not serialize as one string: {other:?}"),
        )),
    }
}

/// The node of an ESS `List` of values [`named_node`] writes.
pub fn named_list_node<T: Serialize>(values: &[T]) -> Result<Node, TargetError> {
    values
        .iter()
        .map(named_node)
        .collect::<Result<Vec<_>, _>>()
        .map(Node::Seq)
}

mod wire_integers {
    use super::integer;
    use ess_primitives::facts::Number;
    use ess_primitives::node::Node;

    fn number(value: f64) -> Node {
        Node::Number(Number::new(value).expect("finite"))
    }

    #[test]
    fn an_integral_float_is_the_integer() {
        assert_eq!(integer(&number(1.0)), Some(1));
        assert_eq!(integer(&number(0.0)), Some(0));
        assert_eq!(integer(&number(-3.0)), Some(-3));
    }

    #[test]
    fn a_fractional_or_out_of_range_number_is_refused() {
        assert_eq!(integer(&number(1.5)), None);
        assert_eq!(integer(&number(9.3e18)), None);
        assert_eq!(integer(&number(-9.3e18)), None);
        assert_eq!(integer(&Node::Text("1".to_owned())), None);
    }
}
