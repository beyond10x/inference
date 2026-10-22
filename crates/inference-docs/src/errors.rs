//! The error reference, `website/docs/reference/errors.md`: every error kind of the compiled ESS
//! specification (`ess specify compile --path ess --format json`, its `errors`), grouped by domain,
//! with its fields and the first source file of a library or adapter that names it. A kind no
//! source file names is refused: the page would document an error nothing can raise.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::{Result, SOURCE, cell, front_matter};

/// Where the error reference lands, relative to the repository root.
pub const PAGE: &str = "website/docs/reference/errors.md";

#[derive(Deserialize)]
struct Compiled {
    errors: BTreeMap<String, ErrorKind>,
}

#[derive(Deserialize)]
struct ErrorKind {
    name: String,
    domain: String,
    summary: String,
    #[serde(default)]
    fields: Vec<Field>,
}

#[derive(Deserialize)]
struct Field {
    name: String,
    type_ref: TypeRef,
}

#[derive(Deserialize)]
struct TypeRef {
    #[serde(default)]
    name: Option<String>,
    kind: String,
    #[serde(default)]
    of: Option<Box<TypeRef>>,
}

impl TypeRef {
    /// The type as the page shows it: a declared name without its domain (`Run.State`), a
    /// primitive by name, `List<…>` and `Optional<…>` around their element.
    fn shown(&self) -> String {
        match (&self.of, &self.name) {
            (Some(of), _) => {
                let mut wrapper = self.kind.clone();
                if let Some(first) = wrapper.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                format!("{wrapper}<{}>", of.shown())
            }
            (None, Some(name)) => {
                let declared: Vec<&str> = name
                    .split('.')
                    .skip_while(|part| part.starts_with(|c: char| c.is_ascii_lowercase()))
                    .collect();
                if declared.is_empty() {
                    name.clone()
                } else {
                    declared.join(".")
                }
            }
            (None, None) => self.kind.clone(),
        }
    }
}

/// The compiled specification of the repository at `root`.
fn compile(root: &Path) -> Result<Compiled> {
    let output = Command::new("ess")
        .args(["specify", "compile", "--path", "ess", "--format", "json"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("running ess specify compile: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "ess specify compile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing the compiled specification: {error}"))
}

/// Every `.rs` file under `src/` of each package in `crates/` and `adapters/`, sorted, with its
/// text; this generator is left out.
fn sources(root: &Path) -> Result<Vec<(String, String)>> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) -> Result<()> {
        let entries =
            fs::read_dir(dir).map_err(|error| format!("reading {}: {error}", dir.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("reading {}: {error}", dir.display()))?
                .path();
            if path.is_dir() {
                walk(&path, root, out)?;
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = fs::read_to_string(&path)
                    .map_err(|error| format!("reading {}: {error}", path.display()))?;
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| format!("{} lies outside the repository", path.display()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((relative, text));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for group in ["crates", "adapters"] {
        let dir = root.join(group);
        let entries =
            fs::read_dir(&dir).map_err(|error| format!("reading {}: {error}", dir.display()))?;
        for entry in entries {
            let package = entry
                .map_err(|error| format!("reading {}: {error}", dir.display()))?
                .path();
            if package.ends_with("inference-docs") {
                continue;
            }
            let src = package.join("src");
            if src.is_dir() {
                walk(&src, root, &mut out)?;
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Whether `text` holds `word` as a whole identifier.
fn names(text: &str, word: &str) -> bool {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(identifier) && !after.is_some_and(identifier)
    })
}

fn render(compiled: &Compiled, sources: &[(String, String)]) -> Result<String> {
    let mut by_domain: BTreeMap<&str, Vec<(&str, &ErrorKind, &str)>> = BTreeMap::new();
    let mut unnamed = Vec::new();
    for kind in compiled.errors.values() {
        let short = kind.name.rsplit('.').next().unwrap_or(&kind.name);
        match sources.iter().find(|(_, text)| names(text, short)) {
            Some((file, _)) => by_domain
                .entry(&kind.domain)
                .or_default()
                .push((short, kind, file)),
            None => unnamed.push(kind.name.as_str()),
        }
    }
    if !unnamed.is_empty() {
        return Err(format!(
            "no library or adapter source names these error kinds of the specification: {}",
            unnamed.join(", ")
        ));
    }
    let mut out = front_matter(
        "Errors",
        "Errors",
        "Every error kind the inference specification declares, by domain, with its fields and where the code names it.",
        3,
    );
    out.push_str(&format!(
        "# Errors\n\nGenerated by `inference-docs` from the compiled ESS specification \
         (`ess specify compile --path ess --format json`). There are {} error kinds in {} domains. \
         Each is a refusal: the command it answers changed nothing. The code names each one, as an \
         enum variant or a type, in the file the last column links; the generator refuses a kind \
         no source file names.\n",
        compiled.errors.len(),
        by_domain.len(),
    ));
    for (domain, kinds) in &by_domain {
        out.push_str(&format!(
            "\n## `{domain}`\n\n| Error | Meaning | Fields | Named in |\n|---|---|---|---|\n"
        ));
        for (short, kind, file) in kinds {
            let fields = if kind.fields.is_empty() {
                "none".to_owned()
            } else {
                kind.fields
                    .iter()
                    .map(|field| format!("`{}`: `{}`", field.name, field.type_ref.shown()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str(&format!(
                "| `{short}` | {} | {fields} | [{}]({SOURCE}/{file}) |\n",
                cell(&kind.summary),
                file.rsplit('/').next().unwrap_or(file),
            ));
        }
    }
    Ok(out)
}

/// The error reference for the repository at `root`.
pub fn page(root: &Path) -> Result<String> {
    render(&compile(root)?, &sources(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled() -> Compiled {
        serde_json::from_value(serde_json::json!({
            "errors": {
                "a.x.RunNotFound": {
                    "name": "a.x.RunNotFound",
                    "domain": "a.x",
                    "summary": "No run.",
                    "fields": [
                        {"name": "run_id", "type_ref": {"kind": "declared", "name": "a.x.RunId"}},
                        {"name": "state", "type_ref": {"kind": "declared", "name": "a.x.Run.State"}},
                        {"name": "at", "type_ref": {"kind": "optional", "of": {"kind": "primitive", "name": "integer"}}}
                    ]
                },
                "a.y.Gone": {"name": "a.y.Gone", "domain": "a.y", "summary": "Gone.", "fields": []}
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_kind_is_found_by_its_whole_name_and_one_nothing_names_is_refused() {
        let sources = vec![
            (
                "crates/a/src/lib.rs".to_owned(),
                "RunNotFoundish GoneAway".to_owned(),
            ),
            (
                "crates/b/src/run.rs".to_owned(),
                "Error::RunNotFound { run_id }".to_owned(),
            ),
        ];
        let error = render(&compiled(), &sources).err().unwrap();
        assert!(error.ends_with(": a.y.Gone"), "{error}");
        let mut sources = sources;
        sources.push(("crates/c/src/lib.rs".to_owned(), "struct Gone;".to_owned()));
        let page = render(&compiled(), &sources).unwrap();
        assert!(page.contains("There are 2 error kinds in 2 domains."));
        assert!(page.contains(
            "| `RunNotFound` | No run. | `run_id`: `RunId`, `state`: `Run.State`, `at`: `Optional<integer>` | [run.rs](https://github.com/beyond10x/inference/blob/main/crates/b/src/run.rs) |"
        ));
        assert!(page.find("## `a.x`").unwrap() < page.find("## `a.y`").unwrap());
    }
}
