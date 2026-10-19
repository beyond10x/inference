//! The in-process [`ConformanceTarget`] of `inference-testkit`.
//!
//! Each [`SemanticCommandRequest`], [`SemanticViewRequest`] and [`EventObservationRequest`] goes to
//! the module of the domain its name starts with, one module per domain of `ess/system.yaml`. A
//! module answers from the stores of `inference-testkit` and never from bookkeeping of its own; a
//! name it does not answer yet is [`TargetError::Unsupported`], never faked. Every scenario gets
//! fresh [`Stores`].
//!
//! A command's consistency token is the target's, not its module's: [`Stores::token`] counts the
//! events every store has recorded, so a view of one domain read at the token of another domain's
//! command is answered.
//!
//! An observed event's position is the target's too: [`Stores::publish`] places every event a
//! store recorded in the target's one publication order, and [`Stores::model_position`],
//! [`Stores::policy_position`], [`Stores::graph_position`] and [`Stores::stream_position`] read its
//! place there. Each store orders its own events only, as an
//! envelope's `sequence` orders one stream only, so the merge order across domains is the
//! target's, the recorder's, and the one thing a module reads that no store holds.
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
use ess_primitives::consistency::{ConsistencyToken, QueryConsistency};
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
    publication: Publication,
}

/// The target's one publication order: for each domain whose store records events, the position
/// of each of its events, in the order its store recorded them.
#[derive(Default)]
struct Publication {
    model: Vec<u64>,
    policy: Vec<u64>,
    graph: Vec<u64>,
    stream: Vec<u64>,
}

impl Stores {
    /// Places every event a store recorded since the last call after the events placed before,
    /// in the order the store recorded them. Each module calls it before and after it runs a
    /// command and before it reads events, so an event a command emits is placed when the command
    /// returns. Events recorded straight into the stores between two calls, as a test arranges a
    /// generation that no command can start yet, are placed at the next call, in the order
    /// `inference.model`, `inference.policy`, `inference.graph`, `inference.stream`. A story that
    /// fills a module adds its events here.
    pub fn publish(&mut self) {
        let mut placed = self.publication.model.len()
            + self.publication.policy.len()
            + self.publication.graph.len()
            + self.publication.stream.len();
        let model = self.model.events().len();
        let policy = self.policy.events().len();
        let graph = self.graph.events().len();
        let stream = self.stream.events().len();
        for (positions, recorded) in [
            (&mut self.publication.model, model),
            (&mut self.publication.policy, policy),
            (&mut self.publication.graph, graph),
            (&mut self.publication.stream, stream),
        ] {
            while positions.len() < recorded {
                placed += 1;
                positions.push(u64::try_from(placed).unwrap_or(u64::MAX));
            }
        }
    }

    /// The position, from 1, of the `index`th event (from 0) the `inference.model` store recorded,
    /// in the target's one publication order.
    pub fn model_position(&self, index: usize) -> Result<u64, TargetError> {
        Self::placed(&self.publication.model, "inference.model", index)
    }

    /// The position, from 1, of the `index`th event (from 0) the `inference.policy` store
    /// recorded, in the target's one publication order.
    pub fn policy_position(&self, index: usize) -> Result<u64, TargetError> {
        Self::placed(&self.publication.policy, "inference.policy", index)
    }

    /// The position, from 1, of the `index`th event (from 0) the `inference.graph` store recorded,
    /// in the target's one publication order.
    pub fn graph_position(&self, index: usize) -> Result<u64, TargetError> {
        Self::placed(&self.publication.graph, "inference.graph", index)
    }

    /// The position, from 1, of the `index`th event (from 0) the `inference.stream` store
    /// recorded, in the target's one publication order.
    pub fn stream_position(&self, index: usize) -> Result<u64, TargetError> {
        Self::placed(&self.publication.stream, "inference.stream", index)
    }

    /// The position [`Stores::publish`] gave the `index`th event of `domain`; an event it has not
    /// placed is an error, never a guessed position.
    fn placed(positions: &[u64], domain: &str, index: usize) -> Result<u64, TargetError> {
        positions.get(index).copied().ok_or_else(|| {
            unreadable(
                format!("placing event {index} of `{domain}`"),
                "the publication order has not placed it",
            )
        })
    }

    /// The target's one sequence: the number of events every store has recorded, whatever its
    /// domain. Each accepted command that emits an event advances it, and a refusal does not.
    ///
    /// It is one number for the whole target because the runner reads a view at the token of the
    /// scenario's last command, whatever that command's domain. It is read from the stores rather
    /// than kept beside them, so nothing can advance it that no store recorded. A story that fills
    /// a module adds that module's events here.
    pub fn sequence(&self) -> u64 {
        let events = self.model.events().len()
            + self.policy.events().len()
            + self.graph.events().len()
            + self.stream.events().len();
        u64::try_from(events).unwrap_or(u64::MAX)
    }

    /// The token of everything the stores have recorded: `seq:` and [`Stores::sequence`].
    pub fn token(&self) -> Result<ConsistencyToken, TargetError> {
        ConsistencyToken::new(format!("seq:{}", self.sequence()))
            .map_err(|error| unreadable("writing a consistency token", format!("{error:?}")))
    }

    /// Refuses a read of `view` demanding a token the stores have not reached, as an error: the
    /// stores are synchronous, so a read never has anything to wait for.
    pub fn require(&self, view: &str, consistency: &QueryConsistency) -> Result<(), TargetError> {
        let Some(demanded) = consistency.token() else {
            return Ok(());
        };
        if sequence_of(demanded) > self.sequence() {
            return Err(unreadable(
                format!("reading `{view}`"),
                format!(
                    "the read demands {demanded:?}, and the stores have reached seq:{}",
                    self.sequence()
                ),
            ));
        }
        Ok(())
    }
}

/// The sequence a token of [`Stores::token`] carries; a token this target did not write is beyond
/// any.
fn sequence_of(token: &ConsistencyToken) -> u64 {
    token
        .as_str()
        .strip_prefix("seq:")
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(u64::MAX)
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

    /// An ESS `Boolean`.
    pub fn boolean(&mut self, field: &str) -> Result<bool, TargetError> {
        match self.take(field)? {
            Node::Bool(value) => Ok(*value),
            other => Err(self.refuse(field, "a boolean", other)),
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

/// The node of an ESS `Boolean`.
pub fn boolean_node(value: bool) -> Node {
    Node::Bool(value)
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

mod one_sequence {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, ViewRef};
    use ess_conformance::target::{
        ConformanceTarget, Deadline, SemanticCommandRequest, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::{ConsistencyToken, QueryConsistency};
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::generation::StartGeneration;
    use inference_core::id::{NodeId, RunId};
    use uuid::Uuid;

    use super::Target;

    fn correlation() -> CorrelationId {
        CorrelationId::new("one-sequence").expect("a correlation id")
    }

    fn at(view: &str, token: ConsistencyToken) -> SemanticViewRequest {
        SemanticViewRequest {
            view: view.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::at_least(token),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        }
    }

    #[test]
    fn a_model_view_read_at_a_stream_commands_token_is_answered() {
        let target = Target::default();
        let started = target
            .stores
            .borrow_mut()
            .stream
            .start_generation(StartGeneration {
                run_id: RunId::from(Uuid::from_u128(1)),
                node_id: NodeId::from(Uuid::from_u128(2)),
                epoch: 1,
            });
        let delta = target
            .execute_command(SemanticCommandRequest {
                command: "inference.stream.EmitTextDelta"
                    .parse::<CommandRef>()
                    .expect("a command name"),
                actor: None,
                caller: None,
                input: [
                    (
                        "generation_id".to_owned(),
                        Node::Text(started.generation_id.to_string()),
                    ),
                    ("sequence".to_owned(), Node::Number(Number::from(1_i64))),
                    ("text".to_owned(), Node::Text("hello".to_owned())),
                ]
                .into_iter()
                .collect(),
                correlation: correlation(),
            })
            .expect("EmitTextDelta answers");
        let token = delta.consistency.expect("a command answers a token");
        assert_eq!(
            token.as_str(),
            "seq:2",
            "every event any store recorded counts"
        );

        let models = target.query_view(at("inference.model.Models", token));
        assert!(
            models.is_ok_and(|answer| answer.rows.is_empty()),
            "a model view at a stream token"
        );

        let ahead = ConsistencyToken::new("seq:3").expect("a token");
        let unreached = target.query_view(at("inference.stream.Segments", ahead));
        assert!(
            matches!(unreached, Err(TargetError::Unavailable { .. })),
            "{unreached:?}"
        );
    }
}
