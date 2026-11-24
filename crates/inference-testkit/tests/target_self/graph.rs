//! The tests of `tests/target/graph.rs`.

mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, OutcomeRef, ViewRef};
    use ess_conformance::target::{
        ConformanceTarget, Deadline, EventObservationRequest, ExternalOutcomeControl,
        ObservedEvent, SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest,
        TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;

    use crate::subject::graph::{execute_command, observe_events, query_view};
    use crate::subject::{Stores, Target, model, policy, stream};

    const UNKNOWN: &str = "00000000-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("graph").expect("a correlation id")
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn number(value: f64) -> Node {
        Node::Number(Number::new(value).expect("finite"))
    }

    fn list(values: &[&str]) -> Node {
        Node::Seq(values.iter().map(|value| text(value)).collect())
    }

    fn command(name: &str, input: &[(&str, Node)]) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect(),
            correlation: correlation(),
        }
    }

    /// Runs `name` through the module of its domain.
    fn run(stores: &mut Stores, name: &str, input: &[(&str, Node)]) -> SemanticCommandResult {
        let request = command(name, input);
        let result = if name.starts_with("inference.policy.") {
            policy::execute_command(stores, &request)
        } else if name.starts_with("inference.model.") {
            model::execute_command(stores, &request)
        } else {
            execute_command(stores, &request)
        };
        result.unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
    }

    fn branch(result: &SemanticCommandResult) -> String {
        result
            .outcome
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// The field `field` of the one event `result` emitted.
    fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
        assert_eq!(result.direct_events.len(), 1, "{result:?}");
        result.direct_events[0].payload[field].clone()
    }

    /// The rows of `name`, read through the module of its domain.
    fn rows(stores: &mut Stores, name: &str) -> Vec<BTreeMap<String, Node>> {
        let request = SemanticViewRequest {
            view: name.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::Current,
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        };
        let answer = if name.starts_with("inference.stream.") {
            stream::query_view(stores, &request)
        } else {
            query_view(stores, &request)
        };
        answer
            .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
            .rows
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ObservedEvent> {
        observe_events(
            stores,
            &EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    }

    fn profile(stores: &mut Stores) -> Node {
        let result = run(
            stores,
            "inference.policy.DeclareProfile",
            &[
                ("kind", text("offline_strict")),
                ("egress", text("denied")),
                ("admitted", list(&["same_process"])),
            ],
        );
        emitted(&result, "profile_id")
    }

    fn pipeline(stores: &mut Stores, profile_id: Node) -> Node {
        let result = run(
            stores,
            "inference.graph.DeclarePipeline",
            &[
                ("name", text("voice")),
                ("schema_revision", text("1")),
                ("mode", text("turn_based")),
                ("profile_id", profile_id),
                ("required_capabilities", list(&["output_streaming"])),
            ],
        );
        emitted(&result, "pipeline_id")
    }

    fn node(stores: &mut Stores, pipeline_id: Node) -> Node {
        let result = run(
            stores,
            "inference.graph.AddNode",
            &[
                ("pipeline_id", pipeline_id),
                ("name", text("llm")),
                ("role", text("model")),
                ("requires", list(&[])),
            ],
        );
        emitted(&result, "node_id")
    }

    fn port_input(
        pipeline_id: Node,
        node_id: Node,
        direction: &str,
        incremental: Node,
    ) -> Vec<(&'static str, Node)> {
        vec![
            ("pipeline_id", pipeline_id),
            ("node_id", node_id),
            ("name", text(direction)),
            ("direction", text(direction)),
            ("modality", text("Text")),
            ("essentiality", text("essential")),
            ("delivery", text("reliable")),
            ("incremental", incremental),
            ("end_marker_required", Node::Bool(false)),
            ("provisional_allowed", Node::Bool(true)),
        ]
    }

    fn port(stores: &mut Stores, pipeline_id: Node, node_id: Node, direction: &str) -> Node {
        let result = run(
            stores,
            "inference.graph.AddPort",
            &port_input(pipeline_id, node_id, direction, Node::Bool(true)),
        );
        emitted(&result, "port_id")
    }

    fn adapter(stores: &mut Stores) -> Node {
        let model_id = emitted(
            &run(
                stores,
                "inference.model.DeclareModel",
                &[
                    ("family", text("f")),
                    ("checkpoint", text("c")),
                    ("code_license", text("l")),
                    ("weight_license", text("w")),
                    ("checksum", text("s")),
                    ("input_modalities", list(&["Text"])),
                    ("output_modalities", list(&["Text"])),
                    ("provenance", text("p")),
                    ("source_revision", text("r")),
                ],
            ),
            "model_id",
        );
        emitted(
            &run(
                stores,
                "inference.model.DeclareAdapter",
                &[
                    ("model_id", model_id),
                    ("backend", text("b")),
                    ("contract_revision", text("c")),
                    ("execution_form", text("in_process")),
                    ("locality", text("same_process")),
                    ("capabilities", list(&["output_streaming"])),
                    ("max_concurrency", number(1.0)),
                    ("descriptor", empty_descriptor()),
                ],
            ),
            "adapter_id",
        )
    }

    #[test]
    fn a_declaration_through_the_target_is_in_every_view_and_observed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id.clone());
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");
        let input = port(&mut stores, pipeline_id.clone(), node_id.clone(), "input");
        let connected = run(
            &mut stores,
            "inference.graph.ConnectPorts",
            &[
                ("pipeline_id", pipeline_id.clone()),
                ("from_port", output.clone()),
                ("to_port", input.clone()),
                ("capacity", number(4.0)),
                ("overload", text("drop_stale_epochs")),
            ],
        );
        assert_eq!(branch(&connected), "inference.graph.ConnectPorts/connected");
        let adapter_id = adapter(&mut stores);
        let bound = run(
            &mut stores,
            "inference.graph.BindNode",
            &[
                ("pipeline_id", pipeline_id.clone()),
                ("node_id", node_id.clone()),
                ("adapter_id", adapter_id.clone()),
            ],
        );
        assert_eq!(branch(&bound), "inference.graph.BindNode/bound");
        assert_eq!(
            bound.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:9"),
            "every store's events count"
        );

        let pipelines = rows(&mut stores, "inference.graph.Pipelines");
        assert_eq!(pipelines.len(), 1);
        assert_eq!(pipelines[0]["pipeline_id"], pipeline_id);
        assert_eq!(pipelines[0]["profile_id"], profile_id);
        assert_eq!(pipelines[0]["mode"], text("turn_based"));
        assert_eq!(pipelines[0]["effective_capabilities"], Node::Null);
        assert_eq!(pipelines[0]["downgrades"], Node::Null);
        assert_eq!(pipelines[0]["state"], text("Declared"));
        assert_eq!(pipelines[0].len(), 9);

        let ports = rows(&mut stores, "inference.graph.Ports");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0]["port_id"], output);
        assert_eq!(ports[0]["incremental"], Node::Bool(true));
        assert_eq!(ports[0]["end_marker_required"], Node::Bool(false));
        assert_eq!(ports[0]["provisional_allowed"], Node::Bool(true));
        assert_eq!(ports[0]["pipeline_id"], pipeline_id);
        assert_eq!(ports[0].len(), 11);

        let edges = rows(&mut stores, "inference.graph.Edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["capacity"], Node::Number(Number::from(4_i64)));
        assert_eq!(edges[0]["overload"], text("drop_stale_epochs"));
        assert_eq!(edges[0]["to_port"], input);

        let bindings = rows(&mut stores, "inference.graph.Bindings");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["node_id"], node_id);
        assert_eq!(bindings[0]["adapter_id"], adapter_id);
        assert_eq!(rows(&mut stores, "inference.graph.Nodes").len(), 1);

        let at = |stores: &mut Stores, name: &str| -> Vec<Option<u64>> {
            observed(stores, name)
                .iter()
                .map(|event| event.sequence)
                .collect()
        };
        assert_eq!(
            at(&mut stores, "inference.graph.PipelineDeclared"),
            [Some(2)]
        );
        assert_eq!(at(&mut stores, "inference.graph.NodeAdded"), [Some(3)]);
        assert_eq!(
            at(&mut stores, "inference.graph.PortAdded"),
            [Some(4), Some(5)]
        );
        assert_eq!(at(&mut stores, "inference.graph.PortsConnected"), [Some(6)]);
        assert_eq!(at(&mut stores, "inference.graph.NodeBound"), [Some(9)]);
        let profiles = policy::observe_events(
            &mut stores,
            &EventObservationRequest {
                event: "inference.policy.ProfileDeclared"
                    .parse::<EventRef>()
                    .expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .expect("the policy module observes its event");
        assert_eq!(
            profiles
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [Some(1)]
        );
        assert_eq!(profiles[0].payload["profile_id"], profile_id);
        assert_eq!(profiles[0].payload["admitted"], list(&["same_process"]));
    }

    #[test]
    fn a_refused_command_emits_nothing_and_advances_no_token() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let input = port(&mut stores, pipeline_id.clone(), node_id, "input");

        let refused = run(
            &mut stores,
            "inference.graph.ConnectPorts",
            &[
                ("pipeline_id", pipeline_id),
                ("from_port", input.clone()),
                ("to_port", text(UNKNOWN)),
                ("capacity", number(1.0)),
                ("overload", text("pause")),
            ],
        );
        assert_eq!(
            branch(&refused),
            "inference.graph.ConnectPorts/source-not-output"
        );
        let error = refused.error.as_ref().expect("an error");
        assert_eq!(error.fields["from_port"], input);
        assert_eq!(error.fields["to_port"], text(UNKNOWN));
        assert_eq!(error.fields["defect"], text("source_not_output"));
        assert!(refused.direct_events.is_empty());
        assert_eq!(
            refused.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:4")
        );
        assert!(rows(&mut stores, "inference.graph.Edges").is_empty());
    }

    #[test]
    fn an_exposed_port_is_in_the_exposures_view_and_observed_and_a_refusal_emits_nothing() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let input = port(&mut stores, pipeline_id.clone(), node_id, "input");
        let expose = |port_id: Node, capacity: f64| {
            vec![
                ("pipeline_id", pipeline_id.clone()),
                ("port_id", port_id),
                ("name", text("user text")),
                ("capacity", number(capacity)),
                ("overload", text("pause")),
            ]
        };

        let exposed = run(
            &mut stores,
            "inference.graph.ExposePort",
            &expose(input.clone(), 4.0),
        );
        assert_eq!(branch(&exposed), "inference.graph.ExposePort/exposed");
        assert_eq!(emitted(&exposed, "pipeline_id"), pipeline_id);
        assert_eq!(emitted(&exposed, "port_id"), input);
        assert_eq!(emitted(&exposed, "name"), text("user text"));
        let exposure_id = emitted(&exposed, "exposure_id");

        let unbounded = run(
            &mut stores,
            "inference.graph.ExposePort",
            &expose(input.clone(), 0.0),
        );
        assert_eq!(branch(&unbounded), "inference.graph.ExposePort/unbounded");
        let error = unbounded.error.as_ref().expect("an error");
        assert_eq!(error.fields["submitted"], Node::Number(Number::from(0_i64)));
        assert!(unbounded.direct_events.is_empty());
        let unknown = run(
            &mut stores,
            "inference.graph.ExposePort",
            &expose(text(UNKNOWN), 4.0),
        );
        assert_eq!(branch(&unknown), "inference.graph.ExposePort/unknown-port");
        let error = unknown.error.as_ref().expect("an error");
        assert_eq!(error.fields["port_id"], text(UNKNOWN));
        assert_eq!(
            unknown.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:5"),
            "a refusal advances no token"
        );

        let exposures = rows(&mut stores, "inference.graph.Exposures");
        assert_eq!(exposures.len(), 1);
        assert_eq!(exposures[0]["exposure_id"], exposure_id);
        assert_eq!(exposures[0]["pipeline_id"], pipeline_id);
        assert_eq!(exposures[0]["port_id"], input);
        assert_eq!(exposures[0]["name"], text("user text"));
        assert_eq!(exposures[0]["capacity"], Node::Number(Number::from(4_i64)));
        assert_eq!(exposures[0]["overload"], text("pause"));
        assert_eq!(exposures[0].len(), 6);
        let observed = observed(&mut stores, "inference.graph.PortExposed");
        assert_eq!(
            observed
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [Some(5)]
        );
        assert_eq!(observed[0].payload["exposure_id"], exposure_id);
        assert_eq!(observed[0].payload.len(), 4);
    }

    /// A descriptor that declares nothing: its three lists, each empty, as the wire requires.
    fn empty_descriptor() -> Node {
        Node::Map(
            ["voices", "codecs", "rate_limits"]
                .into_iter()
                .map(|list| (list.to_owned(), Node::Seq(Vec::new())))
                .collect(),
        )
    }

    fn port_descriptor(name: &str) -> Node {
        Node::Map(BTreeMap::from([
            ("name".to_owned(), text(name)),
            ("direction".to_owned(), text("input")),
            ("modality".to_owned(), text("Audio")),
            ("essentiality".to_owned(), text("essential")),
            ("delivery".to_owned(), text("reliable")),
            ("incremental".to_owned(), Node::Bool(true)),
            ("end_marker_required".to_owned(), Node::Bool(true)),
            ("provisional_allowed".to_owned(), Node::Bool(false)),
        ]))
    }

    #[test]
    fn a_node_descriptor_is_declared_once_per_kind_and_read_back_whole() {
        let mut stores = Stores::default();
        let ports = Node::Seq(vec![port_descriptor("audio")]);
        let input = [
            ("kind", text("vad")),
            ("role", text("policy")),
            ("requires", list(&["output_streaming"])),
            ("ports", ports.clone()),
        ];
        let declared = run(&mut stores, "inference.graph.DeclareNodeDescriptor", &input);
        assert_eq!(
            branch(&declared),
            "inference.graph.DeclareNodeDescriptor/declared"
        );
        assert_eq!(emitted(&declared, "kind"), text("vad"));
        assert_eq!(emitted(&declared, "role"), text("policy"));

        let again = run(&mut stores, "inference.graph.DeclareNodeDescriptor", &input);
        assert_eq!(
            branch(&again),
            "inference.graph.DeclareNodeDescriptor/already-declared"
        );
        assert_eq!(
            again.error.as_ref().expect("an error").fields["kind"],
            text("vad")
        );
        assert!(again.direct_events.is_empty());
        assert_eq!(
            again.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:1"),
            "a refusal advances no token"
        );

        let published = rows(&mut stores, "inference.graph.NodeDescriptors");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0]["kind"], text("vad"));
        assert_eq!(published[0]["role"], text("policy"));
        assert_eq!(published[0]["requires"], list(&["output_streaming"]));
        assert_eq!(
            published[0]["ports"], ports,
            "every port descriptor field is read back"
        );
        assert_eq!(published[0].len(), 4);
        assert_eq!(
            observed(&mut stores, "inference.graph.NodeDescriptorDeclared")
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [Some(1)]
        );

        let Node::Map(mut extra) = port_descriptor("other") else {
            unreachable!("a record")
        };
        extra.insert("encoding".to_owned(), text("pcm"));
        let undeclared = execute_command(
            &mut stores,
            &command(
                "inference.graph.DeclareNodeDescriptor",
                &[
                    ("kind", text("other")),
                    ("role", text("policy")),
                    ("requires", list(&[])),
                    ("ports", Node::Seq(vec![Node::Map(extra)])),
                ],
            ),
        );
        assert!(
            undeclared.is_err(),
            "an undeclared port descriptor field is an error"
        );
        assert_eq!(
            rows(&mut stores, "inference.graph.NodeDescriptors").len(),
            1
        );
    }

    #[test]
    fn an_undeclared_field_or_a_flag_that_is_not_a_boolean_is_an_error_and_adds_nothing() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());

        let textual = execute_command(
            &mut stores,
            &command(
                "inference.graph.AddPort",
                &port_input(pipeline_id.clone(), node_id.clone(), "input", text("true")),
            ),
        );
        assert!(
            matches!(&textual, Err(TargetError::Unavailable { .. })),
            "{textual:?}"
        );
        let mut extra = port_input(pipeline_id.clone(), node_id, "input", Node::Bool(true));
        extra.push(("profile_id", pipeline_id.clone()));
        let extra = execute_command(&mut stores, &command("inference.graph.AddPort", &extra));
        assert!(
            matches!(&extra, Err(TargetError::Unavailable { .. })),
            "{extra:?}"
        );
        let fractional = execute_command(
            &mut stores,
            &command(
                "inference.graph.ConnectPorts",
                &[
                    ("pipeline_id", pipeline_id),
                    ("from_port", text(UNKNOWN)),
                    ("to_port", text(UNKNOWN)),
                    ("capacity", number(1.5)),
                    ("overload", text("pause")),
                ],
            ),
        );
        assert!(
            matches!(&fractional, Err(TargetError::Unavailable { .. })),
            "{fractional:?}"
        );
        assert!(rows(&mut stores, "inference.graph.Ports").is_empty());
        assert_eq!(stores.graph.events().len(), 2);
    }

    fn validate(stores: &mut Stores, pipeline_id: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.ValidatePipeline",
            &[("pipeline_id", pipeline_id)],
        )
    }

    #[test]
    fn a_valid_pipeline_is_validated_once_and_its_event_is_placed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);

        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        assert_eq!(emitted(&validated, "pipeline_id"), pipeline_id);
        assert_eq!(validated.direct_events[0].payload.len(), 1);
        assert_eq!(
            validated.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:3")
        );
        assert_eq!(
            rows(&mut stores, "inference.graph.Pipelines")[0]["state"],
            text("Validated")
        );
        let placed: Vec<_> = observed(&mut stores, "inference.graph.PipelineValidated")
            .iter()
            .map(|event| (event.sequence, event.payload["pipeline_id"].clone()))
            .collect();
        assert_eq!(placed, [(Some(3), pipeline_id.clone())]);

        let again = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&again),
            "inference.graph.ValidatePipeline/wrong-state"
        );
        let error = again.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert_eq!(
            error.fields,
            BTreeMap::from([("current".to_owned(), text("Validated"))])
        );
        assert!(again.direct_events.is_empty());
        assert_eq!(
            again.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:3")
        );
    }

    #[test]
    fn an_unknown_pipeline_is_wrong_state_with_no_current() {
        let mut stores = Stores::default();
        let refused = validate(&mut stores, text(UNKNOWN));
        assert_eq!(
            branch(&refused),
            "inference.graph.ValidatePipeline/wrong-state"
        );
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert!(error.fields.is_empty(), "{error:?}");
        assert!(refused.direct_events.is_empty());
    }

    #[test]
    fn an_invalid_pipeline_is_rejected_with_its_findings_and_stays_declared() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");

        let rejected = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&rejected),
            "inference.graph.ValidatePipeline/rejected"
        );
        let error = rejected.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.IncompatibleGraph");
        assert_eq!(
            error.fields,
            BTreeMap::from([(
                "findings".to_owned(),
                Node::Seq(vec![Node::Map(BTreeMap::from([
                    ("defect".to_owned(), text("unreachable_mandatory_output")),
                    ("node_id".to_owned(), node_id),
                    ("port_id".to_owned(), output),
                ]))])
            )])
        );
        assert!(rejected.direct_events.is_empty());
        assert_eq!(
            rows(&mut stores, "inference.graph.Pipelines")[0]["state"],
            text("Declared")
        );
        assert!(observed(&mut stores, "inference.graph.PipelineValidated").is_empty());
    }

    #[test]
    fn a_port_is_added_only_under_a_known_pipeline_that_is_still_declared() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());

        let unknown = run(
            &mut stores,
            "inference.graph.AddPort",
            &port_input(text(UNKNOWN), node_id.clone(), "input", Node::Bool(true)),
        );
        assert_eq!(branch(&unknown), "inference.graph.AddPort/unknown-pipeline");
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotFound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("pipeline_id".to_owned(), text(UNKNOWN))])
        );

        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        let events = stores.graph.events().len();
        let refused = run(
            &mut stores,
            "inference.graph.AddPort",
            &port_input(pipeline_id.clone(), node_id, "input", Node::Bool(true)),
        );
        assert_eq!(branch(&refused), "inference.graph.AddPort/not-declared");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotReady");
        assert_eq!(
            error.fields,
            BTreeMap::from([
                ("pipeline_id".to_owned(), pipeline_id),
                ("required".to_owned(), text("Declared")),
            ])
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(stores.graph.events().len(), events);
        assert!(rows(&mut stores, "inference.graph.Ports").is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_rejection_of_a_declared_pipeline_carries_the_stores_findings() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");

        force(&mut stores, "inference.graph.ValidatePipeline/rejected")
            .expect("rejected is external");
        let forced = validate(&mut stores, pipeline_id);
        assert_eq!(branch(&forced), "inference.graph.ValidatePipeline/rejected");
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields["findings"],
            Node::Seq(vec![Node::Map(BTreeMap::from([
                ("defect".to_owned(), text("unreachable_mandatory_output")),
                ("node_id".to_owned(), node_id),
                ("port_id".to_owned(), output),
            ]))])
        );
        assert_eq!(state_of(&mut stores), text("Declared"));
    }

    #[test]
    fn a_forced_rejection_answers_in_every_state_as_the_specification_orders_it() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );

        force(&mut stores, "inference.graph.ValidatePipeline/rejected")
            .expect("rejected is external");
        let forced = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&forced),
            "inference.graph.ValidatePipeline/rejected",
            "`rejected` moves nothing, so `wrong_state` never answers before it (ESS precedence, \
             step 4 against step 6), and the interpreter answers it on a Validated pipeline"
        );
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields["findings"],
            Node::Seq(Vec::new())
        );
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_rejection_answers_the_next_validation_only() {
        let target = Target::default();
        let pipeline_id = {
            let stores = &mut *target.stores.borrow_mut();
            let profile_id = profile(stores);
            pipeline(stores, profile_id)
        };
        let force = |outcome: &str| {
            target.configure_external_outcome(ExternalOutcomeControl {
                force: OutcomeRef::new(
                    outcome
                        .split_once('/')
                        .and_then(|(command, _)| command.parse::<CommandRef>().ok())
                        .expect("a command name"),
                    outcome
                        .split_once('/')
                        .and_then(|(_, branch)| branch.parse().ok())
                        .expect("an outcome name"),
                ),
                correlation: correlation(),
            })
        };
        let validate = || {
            target
                .execute_command(command(
                    "inference.graph.ValidatePipeline",
                    &[("pipeline_id", pipeline_id.clone())],
                ))
                .expect("ValidatePipeline answers")
        };

        force("inference.graph.ValidatePipeline/rejected").expect("rejected is external");
        let rejected = validate();
        assert_eq!(
            branch(&rejected),
            "inference.graph.ValidatePipeline/rejected"
        );
        let error = rejected.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.IncompatibleGraph");
        assert_eq!(error.fields["findings"], Node::Seq(Vec::new()));
        assert!(rejected.direct_events.is_empty());

        let validated = validate();
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated",
            "the force lapses after one invocation"
        );

        let declared = force("inference.graph.ValidatePipeline/validated");
        assert!(
            matches!(&declared, Err(TargetError::Unavailable { .. })),
            "{declared:?}"
        );
        let planned = force("inference.graph.PlanPipeline/planned");
        assert!(
            matches!(&planned, Err(TargetError::Unavailable { .. })),
            "{planned:?}"
        );
        let run = force("inference.graph.CreateRun/created");
        assert!(
            matches!(&run, Err(TargetError::Unsupported { .. })),
            "{run:?}"
        );
    }

    /// A Planned pipeline with one node, as the run scenarios arrange it.
    fn planned(stores: &mut Stores) -> (Node, Node) {
        let (pipeline_id, node_id) = one_model(stores, &[], false);
        let planned = plan(stores, pipeline_id.clone());
        assert_eq!(branch(&planned), "inference.graph.PlanPipeline/planned");
        (pipeline_id, node_id)
    }

    fn create(stores: &mut Stores, pipeline_id: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.CreateRun",
            &[("pipeline_id", pipeline_id)],
        )
    }

    fn on_run(stores: &mut Stores, command: &str, run_id: &Node) -> SemanticCommandResult {
        run(
            stores,
            &format!("inference.graph.{command}"),
            &[("run_id", run_id.clone())],
        )
    }

    fn run_row(stores: &mut Stores) -> BTreeMap<String, Node> {
        let runs = rows(stores, "inference.graph.Runs");
        assert_eq!(runs.len(), 1, "{runs:?}");
        runs[0].clone()
    }

    /// A Planned pipeline with no node, as a subgraph's child, and a Declared parent.
    fn child_and_parent(stores: &mut Stores) -> (Node, Node) {
        let profile_id = profile(stores);
        let child = emitted(
            &run(
                stores,
                "inference.graph.DeclarePipeline",
                &[
                    ("name", text("child")),
                    ("schema_revision", text("1")),
                    ("mode", text("turn_based")),
                    ("profile_id", profile_id.clone()),
                    ("required_capabilities", list(&[])),
                ],
            ),
            "pipeline_id",
        );
        assert_eq!(
            branch(&validate(stores, child.clone())),
            "inference.graph.ValidatePipeline/validated"
        );
        assert_eq!(
            branch(&plan(stores, child.clone())),
            "inference.graph.PlanPipeline/planned"
        );
        let parent = pipeline(stores, profile_id);
        (child, parent)
    }

    fn add_subgraph(stores: &mut Stores, parent: Node, child: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.AddSubgraphNode",
            &[
                ("pipeline_id", parent),
                ("name", text("frontend")),
                ("role", text("transform")),
                ("subgraph_id", child),
            ],
        )
    }

    #[test]
    fn a_subgraph_node_is_added_over_a_planned_child_and_read_back_with_its_child() {
        let mut stores = Stores::default();
        let (child, parent) = child_and_parent(&mut stores);
        let refused = add_subgraph(&mut stores, parent.clone(), parent.clone());
        assert_eq!(
            branch(&refused),
            "inference.graph.AddSubgraphNode/subgraph-not-planned"
        );
        let error = refused.error.as_ref().expect("an error");
        assert_eq!(error.fields["pipeline_id"], parent);
        assert_eq!(error.fields["required"], text("Planned"));
        let added = add_subgraph(&mut stores, parent.clone(), child.clone());
        assert_eq!(branch(&added), "inference.graph.AddSubgraphNode/added");
        assert_eq!(emitted(&added, "subgraph_id"), child);
        assert_eq!(emitted(&added, "pipeline_id"), parent);
        let node_id = emitted(&added, "node_id");
        let nodes = rows(&mut stores, "inference.graph.Nodes");
        let row = nodes
            .iter()
            .find(|row| row["node_id"] == node_id)
            .expect("the node is in the view");
        assert_eq!(row["requires"], list(&[]));
        assert_eq!(row["role"], text("transform"));
        assert_eq!(
            row.len(),
            5,
            "the Nodes view does not publish the subgraph reference"
        );
        assert_eq!(
            observed(&mut stores, "inference.graph.SubgraphNodeAdded").len(),
            1
        );
    }

    #[test]
    fn planning_refuses_a_subgraph_node_whose_port_no_exposure_matches() {
        let mut stores = Stores::default();
        let (child, parent) = child_and_parent(&mut stores);
        let node_id = emitted(&add_subgraph(&mut stores, parent.clone(), child), "node_id");
        let port_id = port(&mut stores, parent.clone(), node_id.clone(), "input");
        assert_eq!(
            branch(&validate(&mut stores, parent.clone())),
            "inference.graph.ValidatePipeline/validated"
        );
        let refused = plan(&mut stores, parent);
        assert_eq!(
            branch(&refused),
            "inference.graph.PlanPipeline/interface-mismatch"
        );
        let error = refused.error.as_ref().expect("an error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.SubgraphInterfaceMismatch"
        );
        assert_eq!(error.fields["node_id"], node_id);
        assert_eq!(error.fields["port_id"], port_id);
        assert_eq!(error.fields["exposure_id"], Node::Null);
        assert!(refused.direct_events.is_empty());
    }

    #[test]
    fn a_nested_run_records_its_parent_instance_and_a_run_never_nested_none() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id.clone()), "run_id");
        on_run(&mut stores, "ValidateRun", &run_id);
        on_run(&mut stores, "PrepareRun", &run_id);
        let instance_id = emitted(
            &run(
                &mut stores,
                "inference.graph.DescribeNodeInstance",
                &[("run_id", run_id.clone()), ("node_id", node_id)],
            ),
            "instance_id",
        );
        let child = emitted(&create(&mut stores, pipeline_id), "run_id");
        let nest = |stores: &mut Stores, parent: Node| {
            run(
                stores,
                "inference.graph.NestRun",
                &[("run_id", child.clone()), ("parent", parent)],
            )
        };
        let unknown = nest(&mut stores, text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.graph.NestRun/unknown-parent");
        assert_eq!(
            unknown.error.as_ref().expect("an error").fields["instance_id"],
            text(UNKNOWN)
        );
        assert!(unknown.direct_events.is_empty());
        let nested = nest(&mut stores, instance_id.clone());
        assert_eq!(branch(&nested), "inference.graph.NestRun/nested");
        assert_eq!(emitted(&nested, "parent"), instance_id);
        assert_eq!(emitted(&nested, "run_id"), child);
        let runs = rows(&mut stores, "inference.graph.Runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["run_id"], run_id);
        assert_eq!(
            runs[0]["parent"],
            Node::Null,
            "a run never nested has no parent"
        );
        assert_eq!(runs[1]["run_id"], child);
        assert_eq!(runs[1]["parent"], instance_id);
        assert_eq!(runs[1]["state"], text("Created"));
        assert_eq!(observed(&mut stores, "inference.graph.RunNested").len(), 1);
        let unknown_run = execute_command(
            &mut stores,
            &command(
                "inference.graph.NestRun",
                &[("run_id", text(UNKNOWN)), ("parent", instance_id)],
            ),
        );
        assert!(
            unknown_run.is_err(),
            "an unknown run has no declared outcome"
        );
    }

    #[test]
    fn a_run_of_a_planned_pipeline_moves_to_completed_and_each_event_is_placed() {
        let mut stores = Stores::default();
        let (pipeline_id, _) = planned(&mut stores);
        let created = create(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&created), "inference.graph.CreateRun/created");
        let run_id = emitted(&created, "run_id");
        assert_eq!(
            run_id,
            text("00000003-0001-4000-8000-000000000001"),
            "a run counts in the graph space, apart from the declared entities"
        );
        assert_eq!(emitted(&created, "pipeline_id"), pipeline_id);
        assert_eq!(created.direct_events[0].payload.len(), 2);
        let row = run_row(&mut stores);
        assert_eq!(row["run_id"], run_id);
        assert_eq!(row["pipeline_id"], pipeline_id);
        assert_eq!(row["failure"], Node::Null);
        assert_eq!(row["state"], text("Created"));
        assert_eq!(
            row["parent"],
            Node::Null,
            "a run created on its own has no parent"
        );
        assert_eq!(row.len(), 5);

        for (command, outcome, state, event) in [
            ("ValidateRun", "validating", "Validating", "RunValidating"),
            ("PrepareRun", "preparing", "Preparing", "RunPreparing"),
            ("ActivateRun", "active", "Active", "RunActivated"),
            ("DrainRun", "draining", "Draining", "RunDraining"),
            ("CompleteRun", "completed", "Completed", "RunCompleted"),
        ] {
            let moved = on_run(&mut stores, command, &run_id);
            assert_eq!(
                branch(&moved),
                format!("inference.graph.{command}/{outcome}")
            );
            assert_eq!(emitted(&moved, "run_id"), run_id);
            assert_eq!(moved.direct_events[0].payload.len(), 1);
            assert_eq!(run_row(&mut stores)["state"], text(state));
            let token = moved
                .consistency
                .as_ref()
                .map(|token| token.as_str().to_owned());
            let placed = observed(&mut stores, &format!("inference.graph.{event}"));
            assert_eq!(placed.len(), 1);
            assert_eq!(
                placed[0].sequence.map(|at| format!("seq:{at}")),
                token,
                "the event is the last one the command placed"
            );
        }
    }

    #[test]
    fn create_run_refuses_an_unknown_pipeline_and_one_that_is_not_planned() {
        let mut stores = Stores::default();
        let unknown = create(&mut stores, text(UNKNOWN));
        assert_eq!(
            branch(&unknown),
            "inference.graph.CreateRun/unknown-pipeline"
        );
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotFound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("pipeline_id".to_owned(), text(UNKNOWN))])
        );

        let (pipeline_id, _) = one_model(&mut stores, &[], false);
        let events = stores.graph.events().len();
        let validated = create(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&validated), "inference.graph.CreateRun/not-planned");
        let error = validated.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotReady");
        assert_eq!(
            error.fields,
            BTreeMap::from([
                ("pipeline_id".to_owned(), pipeline_id),
                ("required".to_owned(), text("Planned")),
            ])
        );
        assert!(validated.direct_events.is_empty());
        assert_eq!(stores.graph.events().len(), events);
        assert!(rows(&mut stores, "inference.graph.Runs").is_empty());
    }

    #[test]
    fn a_refused_run_command_moves_nothing_and_an_unknown_run_reports_no_state() {
        let mut stores = Stores::default();
        let (pipeline_id, _) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        let events = stores.graph.events().len();
        let before = run_row(&mut stores);

        for command in ["PrepareRun", "ActivateRun", "DrainRun", "CompleteRun"] {
            let refused = on_run(&mut stores, command, &run_id);
            assert_eq!(
                branch(&refused),
                format!("inference.graph.{command}/wrong-state")
            );
            let error = refused.error.as_ref().expect("a declared error");
            assert_eq!(error.error.to_string(), "inference.graph.RunStateConflict");
            assert_eq!(
                error.fields,
                BTreeMap::from([("current".to_owned(), text("Created"))])
            );
            assert!(refused.direct_events.is_empty());
        }
        let failed = run(
            &mut stores,
            "inference.graph.FailRun",
            &[
                ("run_id", run_id.clone()),
                (
                    "failure",
                    Node::Map(BTreeMap::from([("detail".to_owned(), text("d"))])),
                ),
            ],
        );
        assert_eq!(branch(&failed), "inference.graph.FailRun/wrong-state");
        assert_eq!(stores.graph.events().len(), events);
        assert_eq!(run_row(&mut stores), before);

        let unknown = on_run(&mut stores, "ValidateRun", &text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.graph.ValidateRun/wrong-state");
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.RunStateConflict");
        assert!(error.fields.is_empty(), "{error:?}");
    }

    #[test]
    fn fail_run_records_the_failure_read_field_by_field() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        on_run(&mut stores, "ValidateRun", &run_id);
        let fail = |failure: Node| {
            command(
                "inference.graph.FailRun",
                &[("run_id", run_id.clone()), ("failure", failure)],
            )
        };

        for broken in [
            Node::Map(BTreeMap::from([("node_id".to_owned(), node_id.clone())])),
            Node::Map(BTreeMap::from([
                ("detail".to_owned(), text("d")),
                ("code".to_owned(), text("c")),
            ])),
            Node::Map(BTreeMap::from([
                ("detail".to_owned(), text("d")),
                ("node_id".to_owned(), text("not-a-uuid")),
            ])),
            text("d"),
        ] {
            let answer = execute_command(&mut stores, &fail(broken));
            assert!(
                matches!(&answer, Err(TargetError::Unavailable { .. })),
                "{answer:?}"
            );
        }
        assert_eq!(run_row(&mut stores)["state"], text("Validating"));

        let failure = Node::Map(BTreeMap::from([
            ("detail".to_owned(), text("worker lost")),
            ("node_id".to_owned(), node_id.clone()),
        ]));
        let failed = execute_command(&mut stores, &fail(failure.clone())).expect("FailRun answers");
        assert_eq!(branch(&failed), "inference.graph.FailRun/failed");
        assert_eq!(emitted(&failed, "failure"), failure);
        let row = run_row(&mut stores);
        assert_eq!(row["state"], text("Failed"));
        assert_eq!(row["failure"], failure);

        let mut other = Stores::default();
        let (pipeline_id, _) = planned(&mut other);
        let run_id = emitted(&create(&mut other, pipeline_id), "run_id");
        on_run(&mut other, "ValidateRun", &run_id);
        let failed = run(
            &mut other,
            "inference.graph.FailRun",
            &[
                ("run_id", run_id),
                (
                    "failure",
                    Node::Map(BTreeMap::from([("detail".to_owned(), text("lost"))])),
                ),
            ],
        );
        assert_eq!(branch(&failed), "inference.graph.FailRun/failed");
        assert_eq!(
            run_row(&mut other)["failure"],
            Node::Map(BTreeMap::from([
                ("detail".to_owned(), text("lost")),
                ("node_id".to_owned(), Node::Null),
            ])),
            "a failure naming no node carries a null node_id"
        );
    }

    #[test]
    fn a_forced_admission_refusal_answers_the_next_prepare_only_and_moves_nothing() {
        let mut stores = Stores::default();
        let (pipeline_id, _) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        on_run(&mut stores, "ValidateRun", &run_id);
        let events = stores.graph.events().len();

        force(&mut stores, "inference.graph.PrepareRun/refused").expect("refused is external");
        let refused = on_run(&mut stores, "PrepareRun", &run_id);
        assert_eq!(branch(&refused), "inference.graph.PrepareRun/refused");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.AdmissionDenied");
        assert_eq!(
            error.fields,
            BTreeMap::from([("reason".to_owned(), text("not_enough_vram"))])
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(stores.graph.events().len(), events);
        assert_eq!(run_row(&mut stores)["state"], text("Validating"));

        force(&mut stores, "inference.graph.PrepareRun/refused").expect("refused is external");
        let unknown = on_run(&mut stores, "PrepareRun", &text(UNKNOWN));
        assert_eq!(
            branch(&unknown),
            "inference.graph.PrepareRun/refused",
            "a forced refusal answers an unknown run, as the specification orders it"
        );

        let prepared = on_run(&mut stores, "PrepareRun", &run_id);
        assert_eq!(
            branch(&prepared),
            "inference.graph.PrepareRun/preparing",
            "the force lapses after one invocation"
        );
        assert!(matches!(
            force(&mut stores, "inference.graph.PrepareRun/preparing"),
            Err(TargetError::Unavailable { .. })
        ));
    }

    #[test]
    fn describe_node_instance_refuses_an_unknown_run_then_node_then_a_run_not_running() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        let describe = |stores: &mut Stores, run_id: Node, node_id: Node| {
            run(
                stores,
                "inference.graph.DescribeNodeInstance",
                &[("run_id", run_id), ("node_id", node_id)],
            )
        };

        let unknown_run = describe(&mut stores, text(UNKNOWN), text(UNKNOWN));
        assert_eq!(
            branch(&unknown_run),
            "inference.graph.DescribeNodeInstance/unknown-run"
        );
        let error = unknown_run.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.RunNotFound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("run_id".to_owned(), text(UNKNOWN))])
        );

        let unknown_node = describe(&mut stores, run_id.clone(), text(UNKNOWN));
        assert_eq!(
            branch(&unknown_node),
            "inference.graph.DescribeNodeInstance/unknown-node",
            "an unknown node is refused before the Created run's state"
        );
        let error = unknown_node.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.NodeNotFound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("node_id".to_owned(), text(UNKNOWN))])
        );

        let not_running = describe(&mut stores, run_id.clone(), node_id.clone());
        assert_eq!(
            branch(&not_running),
            "inference.graph.DescribeNodeInstance/run-not-running"
        );
        let error = not_running.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.RunNotRunning");
        assert_eq!(
            error.fields,
            BTreeMap::from([("run_id".to_owned(), run_id.clone())])
        );
        assert!(rows(&mut stores, "inference.graph.NodeInstances").is_empty());

        on_run(&mut stores, "ValidateRun", &run_id);
        on_run(&mut stores, "PrepareRun", &run_id);
        let described = describe(&mut stores, run_id.clone(), node_id.clone());
        assert_eq!(
            branch(&described),
            "inference.graph.DescribeNodeInstance/described"
        );
        let instance_id = emitted(&described, "instance_id");
        assert_eq!(instance_id, text("00000003-0001-4000-8000-000000000002"));
        assert_eq!(emitted(&described, "run_id"), run_id);
        assert_eq!(emitted(&described, "node_id"), node_id);
        let instances = rows(&mut stores, "inference.graph.NodeInstances");
        assert_eq!(
            instances,
            [BTreeMap::from([
                ("instance_id".to_owned(), instance_id),
                ("run_id".to_owned(), run_id),
                ("node_id".to_owned(), node_id),
                ("state".to_owned(), text("Described")),
            ])]
        );
    }

    #[test]
    fn a_node_instance_moves_by_its_own_commands_and_a_refused_one_reports_its_state() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        on_run(&mut stores, "ValidateRun", &run_id);
        on_run(&mut stores, "PrepareRun", &run_id);
        let described = run(
            &mut stores,
            "inference.graph.DescribeNodeInstance",
            &[("run_id", run_id.clone()), ("node_id", node_id)],
        );
        let instance_id = emitted(&described, "instance_id");
        let on_instance = |stores: &mut Stores, command: &str, instance_id: &Node| {
            run(
                stores,
                &format!("inference.graph.{command}"),
                &[("instance_id", instance_id.clone())],
            )
        };
        let state = |stores: &mut Stores| {
            let instances = rows(stores, "inference.graph.NodeInstances");
            assert_eq!(instances.len(), 1, "{instances:?}");
            instances[0]["state"].clone()
        };

        let early = on_instance(&mut stores, "StartNodeInstance", &instance_id);
        assert_eq!(
            branch(&early),
            "inference.graph.StartNodeInstance/wrong-state"
        );
        let error = early.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.NodeInstanceStateConflict"
        );
        assert_eq!(
            error.fields,
            BTreeMap::from([("current".to_owned(), text("Described"))])
        );
        assert!(early.direct_events.is_empty());
        assert_eq!(state(&mut stores), text("Described"));

        for (command, outcome, now) in [
            ("PrepareNodeInstance", "prepared", "Prepared"),
            ("StartNodeInstance", "started", "Started"),
            ("DrainNodeInstance", "draining", "Draining"),
            ("CancelNodeInstance", "cancelling", "Cancelling"),
            ("StopNodeInstance", "stopped", "Stopped"),
        ] {
            let moved = on_instance(&mut stores, command, &instance_id);
            assert_eq!(
                branch(&moved),
                format!("inference.graph.{command}/{outcome}")
            );
            assert_eq!(emitted(&moved, "instance_id"), instance_id);
            assert_eq!(state(&mut stores), text(now));
        }
        assert_eq!(
            observed(&mut stores, "inference.graph.NodeInstanceStopped").len(),
            1
        );
        assert_eq!(
            rows(&mut stores, "inference.graph.Runs")[0]["state"],
            text("Preparing"),
            "an instance command moves no run"
        );

        let unknown = run(
            &mut stores,
            "inference.graph.FailNodeInstance",
            &[("instance_id", text(UNKNOWN)), ("reason", text("gone"))],
        );
        assert_eq!(
            branch(&unknown),
            "inference.graph.FailNodeInstance/wrong-state"
        );
        let error = unknown.error.as_ref().expect("a declared error");
        assert!(error.fields.is_empty(), "no record holds a state to report");
    }

    #[test]
    fn a_failed_node_instance_records_its_reason_in_its_event() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id), "run_id");
        on_run(&mut stores, "ValidateRun", &run_id);
        on_run(&mut stores, "PrepareRun", &run_id);
        let described = run(
            &mut stores,
            "inference.graph.DescribeNodeInstance",
            &[("run_id", run_id), ("node_id", node_id)],
        );
        let instance_id = emitted(&described, "instance_id");

        let failed = run(
            &mut stores,
            "inference.graph.FailNodeInstance",
            &[
                ("instance_id", instance_id.clone()),
                ("reason", text("the device is gone")),
            ],
        );
        assert_eq!(branch(&failed), "inference.graph.FailNodeInstance/failed");
        assert_eq!(emitted(&failed, "reason"), text("the device is gone"));
        let observed = observed(&mut stores, "inference.graph.NodeInstanceFailed");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].payload["instance_id"], instance_id);
        assert_eq!(observed[0].payload["reason"], text("the device is gone"));

        let again = run(
            &mut stores,
            "inference.graph.FailNodeInstance",
            &[("instance_id", instance_id), ("reason", text("again"))],
        );
        assert_eq!(
            again.error.as_ref().expect("a declared error").fields,
            BTreeMap::from([("current".to_owned(), text("Failed"))])
        );
    }

    #[test]
    fn cancel_run_cancels_its_active_generations_and_discards_its_unplayed_segments() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = planned(&mut stores);
        let run_id = emitted(&create(&mut stores, pipeline_id.clone()), "run_id");
        let other = emitted(&create(&mut stores, pipeline_id), "run_id");
        for run_id in [&run_id, &other] {
            for command in ["ValidateRun", "PrepareRun", "ActivateRun"] {
                on_run(&mut stores, command, run_id);
            }
        }
        let start = |stores: &mut Stores, run_id: &Node| {
            let started = stream::execute_command(
                stores,
                &command(
                    "inference.stream.StartGeneration",
                    &[
                        ("run_id", run_id.clone()),
                        ("node_id", node_id.clone()),
                        ("epoch", number(1.0)),
                    ],
                ),
            )
            .expect("StartGeneration answers");
            assert_eq!(branch(&started), "inference.stream.StartGeneration/started");
            emitted(&started, "generation_id")
        };
        let delta = |stores: &mut Stores, generation_id: &Node, sequence: f64| {
            let emitted_delta = stream::execute_command(
                stores,
                &command(
                    "inference.stream.EmitTextDelta",
                    &[
                        ("generation_id", generation_id.clone()),
                        ("sequence", number(sequence)),
                        ("text", text("t")),
                    ],
                ),
            )
            .expect("EmitTextDelta answers");
            emitted(&emitted_delta, "segment_id")
        };
        let generation = start(&mut stores, &run_id);
        let kept = start(&mut stores, &other);
        let tentative = delta(&mut stores, &generation, 1.0);
        let elsewhere = delta(&mut stores, &kept, 1.0);
        let stream_events = stores.stream.events().len();

        let cancelled = on_run(&mut stores, "CancelRun", &run_id);
        assert_eq!(branch(&cancelled), "inference.graph.CancelRun/cancelled");
        assert_eq!(cancelled.direct_events.len(), 1, "RunCancelled only");
        assert_eq!(
            stores.stream.events().len(),
            stream_events,
            "the cascade records no stream event"
        );
        let state_of = |stores: &mut Stores, view: &str, field: &str, id: &Node| {
            rows(stores, view)
                .into_iter()
                .find(|row| &row[field] == id)
                .map(|row| row["state"].clone())
        };
        assert_eq!(
            state_of(
                &mut stores,
                "inference.stream.Generations",
                "generation_id",
                &generation
            ),
            Some(text("Cancelled"))
        );
        assert_eq!(
            state_of(
                &mut stores,
                "inference.stream.Segments",
                "segment_id",
                &tentative
            ),
            Some(text("Discarded"))
        );
        assert_eq!(
            state_of(
                &mut stores,
                "inference.stream.Generations",
                "generation_id",
                &kept
            ),
            Some(text("Active")),
            "another run's generation is not cancelled"
        );
        assert_eq!(
            state_of(
                &mut stores,
                "inference.stream.Segments",
                "segment_id",
                &elsewhere
            ),
            Some(text("Tentative"))
        );

        let again = on_run(&mut stores, "CancelRun", &run_id);
        assert_eq!(branch(&again), "inference.graph.CancelRun/wrong-state");
        assert_eq!(
            again.error.as_ref().expect("a declared error").fields,
            BTreeMap::from([("current".to_owned(), text("Cancelled"))])
        );
    }

    fn plan(stores: &mut Stores, pipeline_id: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.PlanPipeline",
            &[("pipeline_id", pipeline_id)],
        )
    }

    /// Forces `outcome`, written `<command>/<outcome>`, through the module.
    fn force(stores: &mut Stores, outcome: &str) -> Result<(), TargetError> {
        let (command, branch) = outcome.split_once('/').expect("a command and an outcome");
        crate::subject::graph::configure_external_outcome(
            stores,
            &ExternalOutcomeControl {
                force: OutcomeRef::new(
                    command.parse::<CommandRef>().expect("a command name"),
                    branch.parse().expect("an outcome name"),
                ),
                correlation: correlation(),
            },
        )
    }

    fn state_of(stores: &mut Stores) -> Node {
        rows(stores, "inference.graph.Pipelines")[0]["state"].clone()
    }

    /// A Validated pipeline holding one model node that requires `requires`, bound to the
    /// adapter of [`adapter`] (which implements `output_streaming` only) when `bound`.
    fn one_model(stores: &mut Stores, requires: &[&str], bound: bool) -> (Node, Node) {
        let profile_id = profile(stores);
        let pipeline_id = pipeline(stores, profile_id);
        let node_id = emitted(
            &run(
                stores,
                "inference.graph.AddNode",
                &[
                    ("pipeline_id", pipeline_id.clone()),
                    ("name", text("llm")),
                    ("role", text("model")),
                    ("requires", list(requires)),
                ],
            ),
            "node_id",
        );
        if bound {
            let adapter_id = adapter(stores);
            let bound = run(
                stores,
                "inference.graph.BindNode",
                &[
                    ("pipeline_id", pipeline_id.clone()),
                    ("node_id", node_id.clone()),
                    ("adapter_id", adapter_id),
                ],
            );
            assert_eq!(branch(&bound), "inference.graph.BindNode/bound");
        }
        let validated = validate(stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        (pipeline_id, node_id)
    }

    #[test]
    fn a_validated_pipeline_is_planned_once_and_its_event_is_placed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        validate(&mut stores, pipeline_id.clone());

        let planned = plan(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&planned), "inference.graph.PlanPipeline/planned");
        assert_eq!(emitted(&planned, "pipeline_id"), pipeline_id);
        assert_eq!(planned.direct_events[0].payload.len(), 1);
        assert_eq!(
            planned.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:4")
        );
        let row = &rows(&mut stores, "inference.graph.Pipelines")[0];
        assert_eq!(row["state"], text("Planned"));
        assert_eq!(
            row["effective_capabilities"],
            list(&[]),
            "a pipeline with no node has no effective capability"
        );
        assert_eq!(row["downgrades"], list(&[]));
        let placed: Vec<_> = observed(&mut stores, "inference.graph.PipelinePlanned")
            .iter()
            .map(|event| (event.sequence, event.payload["pipeline_id"].clone()))
            .collect();
        assert_eq!(placed, [(Some(4), pipeline_id.clone())]);

        for refused in [
            plan(&mut stores, pipeline_id.clone()),
            validate(&mut stores, pipeline_id),
        ] {
            assert!(
                branch(&refused).ends_with("/wrong-state"),
                "{}",
                branch(&refused)
            );
            let error = refused.error.as_ref().expect("a declared error");
            assert_eq!(
                error.error.to_string(),
                "inference.graph.PipelineStateConflict"
            );
            assert_eq!(
                error.fields,
                BTreeMap::from([("current".to_owned(), text("Planned"))])
            );
            assert!(refused.direct_events.is_empty());
        }
    }

    #[test]
    fn planning_a_declared_or_unknown_pipeline_is_wrong_state() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);

        let declared = plan(&mut stores, pipeline_id);
        assert_eq!(
            branch(&declared),
            "inference.graph.PlanPipeline/wrong-state"
        );
        assert_eq!(
            declared.error.as_ref().expect("a declared error").fields,
            BTreeMap::from([("current".to_owned(), text("Declared"))])
        );
        assert_eq!(state_of(&mut stores), text("Declared"));

        let unknown = plan(&mut stores, text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.graph.PlanPipeline/wrong-state");
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert!(error.fields.is_empty(), "{error:?}");
    }

    #[test]
    fn a_node_short_of_tool_calling_is_refused_naming_the_node_and_the_capability() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &["tool_calling"], true);

        let refused = plan(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&refused), "inference.graph.PlanPipeline/mismatch");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.CapabilityMismatch"
        );
        let fields = BTreeMap::from([
            ("node_id".to_owned(), node_id),
            ("capability".to_owned(), text("tool_calling")),
        ]);
        assert_eq!(error.fields, fields);
        assert!(refused.direct_events.is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));

        force(&mut stores, "inference.graph.PlanPipeline/mismatch").expect("mismatch is external");
        let forced = plan(&mut stores, pipeline_id);
        assert_eq!(branch(&forced), "inference.graph.PlanPipeline/mismatch");
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields,
            fields,
            "a forced mismatch names what the planner finds"
        );
    }

    #[test]
    fn a_node_requiring_tool_calling_with_no_binding_is_refused_as_unbound() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &["tool_calling"], false);

        let refused = plan(&mut stores, pipeline_id);
        assert_eq!(branch(&refused), "inference.graph.PlanPipeline/unbound");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.NodeUnbound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("node_id".to_owned(), node_id)])
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_planning_refusal_answers_the_next_plan_only_and_moves_nothing() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &[], true);
        let events = stores.graph.events().len();
        let adapter_id = rows(&mut stores, "inference.graph.Bindings")[0]["adapter_id"].clone();
        let profile_id = rows(&mut stores, "inference.graph.Pipelines")[0]["profile_id"].clone();
        let fields = |pairs: &[(&str, Node)]| -> BTreeMap<String, Node> {
            pairs
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect()
        };

        for (outcome, error, carried) in [
            (
                "unbound",
                "inference.graph.NodeUnbound",
                fields(&[("node_id", node_id.clone())]),
            ),
            (
                "mismatch",
                "inference.graph.CapabilityMismatch",
                fields(&[
                    ("node_id", Node::Null),
                    ("capability", text("output_streaming")),
                ]),
            ),
            (
                "locality-denied",
                "inference.graph.LocalityDenied",
                fields(&[
                    ("node_id", node_id.clone()),
                    ("adapter_id", adapter_id.clone()),
                    ("locality", text("same_process")),
                    ("profile_id", profile_id.clone()),
                ]),
            ),
        ] {
            let name = format!("inference.graph.PlanPipeline/{outcome}");
            force(&mut stores, &name).expect("the outcome is external");
            let forced = plan(&mut stores, pipeline_id.clone());
            assert_eq!(branch(&forced), name);
            let answered = forced.error.as_ref().expect("a declared error");
            assert_eq!(answered.error.to_string(), error);
            assert_eq!(
                answered.fields, carried,
                "the planner finds no such refusal, so the first declared values are named"
            );
            assert!(forced.direct_events.is_empty());
            assert_eq!(state_of(&mut stores), text("Validated"));
            assert_eq!(stores.graph.events().len(), events);
        }

        let nil = text("00000000-0000-0000-0000-000000000000");
        for (outcome, carried) in [
            ("unbound", fields(&[("node_id", nil.clone())])),
            (
                "mismatch",
                fields(&[
                    ("node_id", Node::Null),
                    ("capability", text("output_streaming")),
                ]),
            ),
            (
                "locality-denied",
                fields(&[
                    ("node_id", nil.clone()),
                    ("adapter_id", nil.clone()),
                    ("locality", text("same_process")),
                    ("profile_id", nil.clone()),
                ]),
            ),
        ] {
            let name = format!("inference.graph.PlanPipeline/{outcome}");
            force(&mut stores, &name).expect("the outcome is external");
            let forced = plan(&mut stores, text(UNKNOWN));
            assert_eq!(branch(&forced), name);
            assert_eq!(
                forced.error.as_ref().expect("a declared error").fields,
                carried,
                "no record names a node, an adapter or a profile, so the nil identity stands in"
            );
        }

        let planned = plan(&mut stores, pipeline_id);
        assert_eq!(
            branch(&planned),
            "inference.graph.PlanPipeline/planned",
            "the force lapses after one invocation"
        );
        assert!(matches!(
            force(&mut stores, "inference.graph.PlanPipeline/wrong-state"),
            Err(TargetError::Unavailable { .. })
        ));
    }
}
