//! The capability record of an adapter, as its crate commits it: one JSON file per hardware
//! target, checked by a test against the adapter's declaration.
//!
//! A record file holds the fields of `inference.model.DeclareCapabilityRecord` except the two
//! identities, `adapter_id` and `model_id`: those are the adapter's, issued when it is declared,
//! so [`load`] takes them from the [`Adapter`] it is given and refuses a file that names either.
//! A field the specification does not declare is refused, at every level of the file.
//!
//! [`check`] holds the record's evidence to the adapter's claims: every capability the adapter
//! claims has an evidence entry naming at least one conformance test by a name that is not empty
//! or blank, and no evidence entry names a capability the adapter does not claim. [`load`] runs
//! it, so a record that loads is one that passed. Binding a node does not require a record.
//!
//! ```
//! # use inference_testkit::model::capability::{self, RecordRefused};
//! # use inference_core::model::{Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId};
//! let (adapter, _) = Adapter::declare(DeclareAdapter {
//!     model_id: ModelId::generate(),
//!     backend: "backend".to_owned(),
//!     contract_revision: "1".to_owned(),
//!     execution_form: ExecutionForm::InProcess,
//!     locality: Locality::SameProcess,
//!     capabilities: vec![Capability::OutputStreaming],
//!     max_concurrency: 1,
//! })
//! .unwrap();
//! let mut record = capability::parse(
//!     r#"{
//!         "hardware": "reference machine",
//!         "languages": ["en"],
//!         "audio_formats": [],
//!         "evidence": [{"capability": "output_streaming", "tests": ["stream::partials"]}],
//!         "resource_profile": {
//!             "peak_resident_memory_mib": 512,
//!             "load_time_ms": 900,
//!             "cpu_real_time_factor_milli": 250
//!         },
//!         "adapter_revision": "0.1.0"
//!     }"#,
//!     &adapter,
//! )
//! .unwrap();
//! assert_eq!(record.adapter_id, adapter.adapter_id());
//!
//! record.evidence.clear();
//! assert_eq!(
//!     capability::check(&record, &adapter),
//!     Err(RecordRefused::Unevidenced(Capability::OutputStreaming))
//! );
//! ```

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use inference_core::model::{Adapter, Capability, DeclareCapabilityRecord};
use serde_json::Value;

/// Why a capability record was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordRefused {
    /// The file could not be read.
    Unreadable { path: PathBuf, detail: String },
    /// The file is not a record: not JSON, not an object, naming an identity, missing a declared
    /// field or carrying an undeclared one. `path` is empty for a record [`parse`] was given as
    /// text.
    Malformed { path: PathBuf, detail: String },
    /// The adapter claims the capability and no evidence entry names a test that observed it.
    Unevidenced(Capability),
    /// An evidence entry names the capability and the adapter does not claim it.
    Unclaimed(Capability),
}

impl fmt::Display for RecordRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, detail } => {
                write!(f, "{} cannot be read: {detail}", path.display())
            }
            Self::Malformed { path, detail } if path.as_os_str().is_empty() => {
                write!(f, "the text is not a capability record: {detail}")
            }
            Self::Malformed { path, detail } => {
                write!(f, "{} is not a capability record: {detail}", path.display())
            }
            Self::Unevidenced(capability) => write!(
                f,
                "the adapter claims `{}` and the record names no conformance test that observed it",
                name(*capability)
            ),
            Self::Unclaimed(capability) => write!(
                f,
                "the record names evidence for `{}`, which the adapter does not claim",
                name(*capability)
            ),
        }
    }
}

impl Error for RecordRefused {}

/// The two fields of the declaration a record file must not carry: the adapter's identities.
const IDENTITIES: [&str; 2] = ["adapter_id", "model_id"];

/// Reads the record file at `path` as the record of `adapter`, then [`check`]s it.
///
/// # Errors
///
/// [`RecordRefused::Unreadable`] when the file cannot be read; [`RecordRefused::Malformed`] when
/// it is not a record, as [`parse`] decides; otherwise what [`check`] refuses.
pub fn load(
    path: impl AsRef<Path>,
    adapter: &Adapter,
) -> Result<DeclareCapabilityRecord, RecordRefused> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|error| RecordRefused::Unreadable {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    parse(&text, adapter).map_err(|refused| match refused {
        RecordRefused::Malformed { detail, .. } => RecordRefused::Malformed {
            path: path.to_path_buf(),
            detail,
        },
        other => other,
    })
}

/// Reads `text`, the content of a record file, as the record of `adapter`, then [`check`]s it.
///
/// # Errors
///
/// [`RecordRefused::Malformed`] when `text` is not a JSON object, names `adapter_id` or
/// `model_id`, or does not read as the rest of `inference.model.DeclareCapabilityRecord` with no
/// undeclared field; otherwise what [`check`] refuses.
pub fn parse(text: &str, adapter: &Adapter) -> Result<DeclareCapabilityRecord, RecordRefused> {
    let malformed = |detail: String| RecordRefused::Malformed {
        path: PathBuf::new(),
        detail,
    };
    let value: Value = serde_json::from_str(text).map_err(|error| malformed(error.to_string()))?;
    let Value::Object(mut fields) = value else {
        return Err(malformed("it is not a JSON object".to_owned()));
    };
    if let Some(identity) = IDENTITIES
        .into_iter()
        .find(|identity| fields.contains_key(*identity))
    {
        return Err(malformed(format!(
            "it names `{identity}`, which is the adapter's and is not written in the file"
        )));
    }
    fields.insert(
        "adapter_id".to_owned(),
        Value::String(adapter.adapter_id().to_string()),
    );
    fields.insert(
        "model_id".to_owned(),
        Value::String(adapter.model_id().to_string()),
    );
    let record: DeclareCapabilityRecord = serde_json::from_value(Value::Object(fields))
        .map_err(|error| malformed(error.to_string()))?;
    check(&record, adapter)?;
    Ok(record)
}

/// Holds the evidence of `record` to the claims of `adapter`: each claimed capability, in the
/// order the adapter claims them, must have an evidence entry naming at least one test, and then
/// each evidence entry, in the record's order, must name a claimed capability. An entry naming no
/// test is no evidence, and a test name that is empty or only blank names no test: nothing can be
/// run under it.
///
/// # Errors
///
/// [`RecordRefused::Unevidenced`] naming the first claimed capability without evidence;
/// otherwise [`RecordRefused::Unclaimed`] naming the first evidenced capability the adapter does
/// not claim.
pub fn check(record: &DeclareCapabilityRecord, adapter: &Adapter) -> Result<(), RecordRefused> {
    let claims = adapter.capabilities();
    if let Some(unevidenced) = claims.iter().find(|claim| {
        !record.evidence.iter().any(|evidence| {
            evidence.capability == **claim
                && evidence.tests.iter().any(|test| !test.trim().is_empty())
        })
    }) {
        return Err(RecordRefused::Unevidenced(*unevidenced));
    }
    if let Some(unclaimed) = record
        .evidence
        .iter()
        .find(|evidence| !claims.contains(&evidence.capability))
    {
        return Err(RecordRefused::Unclaimed(unclaimed.capability));
    }
    Ok(())
}

/// The ESS name of `capability`, `output_streaming` for [`Capability::OutputStreaming`].
fn name(capability: Capability) -> String {
    match serde_json::to_value(capability) {
        Ok(Value::String(name)) => name,
        _ => format!("{capability:?}"),
    }
}
