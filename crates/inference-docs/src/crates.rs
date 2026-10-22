//! The crate list, from `cargo metadata --no-deps`: every workspace package, its directory, the
//! name Rust code uses for it, its Cargo features and its `description`, grouped by the directory
//! it lives in.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::{Result, TREE, cell, front_matter};

/// Where the crate list lands, relative to the repository root.
pub const PAGE: &str = "website/docs/reference/crates.md";

/// One workspace package as the page lists it.
#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub directory: String,
    /// The library name (`use <lib>`), when the package has a library.
    lib: Option<String>,
    /// The binaries it builds.
    bins: Vec<String>,
    /// Its Cargo features, without `default`.
    pub features: Vec<String>,
    /// Its features on by default.
    defaults: Vec<String>,
    /// Whether the package may be published (`publish = false` marks examples and tooling).
    publishable: bool,
    description: String,
}

/// The `cargo metadata --format-version 1 --no-deps` document of the workspace at `root`.
pub fn metadata(root: &Path) -> Result<Value> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(&cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(|error| format!("running cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parsing cargo metadata: {error}"))
}

/// The version every package of the workspace carries; refused when two differ.
pub fn workspace_version(metadata: &Value) -> Result<String> {
    let mut versions: Vec<&str> = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?
        .iter()
        .filter_map(|package| package["version"].as_str())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    match versions.as_slice() {
        [version] => Ok((*version).to_owned()),
        _ => Err(format!(
            "the workspace packages carry more than one version: {}",
            versions.join(", ")
        )),
    }
}

/// The packages of a `cargo metadata --format-version 1` document, sorted by name, with their
/// directories relative to `workspace_root`. A package without a description is refused: the page
/// would have nothing to say about it.
pub fn packages(metadata: &Value) -> Result<Vec<Package>> {
    let root = metadata["workspace_root"]
        .as_str()
        .ok_or("cargo metadata names no workspace_root")?;
    let mut found = Vec::new();
    for package in metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?
    {
        let name = package["name"]
            .as_str()
            .ok_or("a package has no name")?
            .to_owned();
        let manifest = package["manifest_path"]
            .as_str()
            .ok_or_else(|| format!("{name} has no manifest_path"))?;
        let directory = Path::new(manifest)
            .parent()
            .and_then(|dir| dir.strip_prefix(root).ok())
            .ok_or_else(|| format!("{name} lies outside the workspace"))?
            .to_string_lossy()
            .replace('\\', "/");
        let description = package["description"]
            .as_str()
            .ok_or_else(|| format!("{name} has no description in its Cargo.toml"))?;
        let mut lib = None;
        let mut bins = Vec::new();
        for target in package["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = target["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let target_name = target["name"].as_str().unwrap_or_default().to_owned();
            if kinds.contains(&"lib") {
                lib = Some(target_name);
            } else if kinds.contains(&"bin") {
                bins.push(target_name);
            }
        }
        bins.sort();
        let mut features: Vec<String> = package["features"]
            .as_object()
            .into_iter()
            .flat_map(|features| features.keys().cloned())
            .filter(|feature| feature != "default")
            .collect();
        features.sort();
        let mut defaults: Vec<String> = package["features"]["default"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        defaults.sort();
        // `publish = false` is an empty registry list; an absent key is `null`.
        let publishable = !package["publish"]
            .as_array()
            .is_some_and(|registries| registries.is_empty());
        found.push(Package {
            name,
            directory,
            lib,
            bins,
            features,
            defaults,
            publishable,
            description: description.trim().to_owned(),
        });
    }
    if found.is_empty() {
        return Err("cargo metadata lists no packages".to_owned());
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

fn table(packages: &[&Package]) -> String {
    let mut out = String::from("| Package | Features | What it is |\n|---|---|---|\n");
    for package in packages {
        let mut uses = Vec::new();
        if let Some(lib) = &package.lib
            && *lib != package.name.replace('-', "_")
        {
            uses.push(format!("Used in Rust as `{lib}`."));
        }
        for bin in &package.bins {
            uses.push(format!("Binary `{bin}`."));
        }
        if package.lib.is_none() && package.bins.is_empty() {
            uses.push("Examples only.".to_owned());
        }
        uses.push(cell(&package.description));
        let features = if package.features.is_empty() {
            "none".to_owned()
        } else {
            package
                .features
                .iter()
                .map(|feature| {
                    if package.defaults.contains(feature) {
                        format!("`{feature}` (default)")
                    } else {
                        format!("`{feature}`")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!(
            "| [`{}`]({TREE}/{}) | {features} | {} |\n",
            package.name,
            package.directory,
            uses.join(" "),
        ));
    }
    out
}

/// The crate list page for the packages of `metadata`.
pub fn page_for(metadata: &Value) -> Result<String> {
    let all = packages(metadata)?;
    let version = workspace_version(metadata)?;
    let in_dir = |dir: &str, publishable: bool| -> Vec<&Package> {
        all.iter()
            .filter(|package| {
                package.directory.starts_with(dir) && package.publishable == publishable
            })
            .collect()
    };
    let libraries = in_dir("crates/", true);
    let tooling = in_dir("crates/", false);
    let adapters: Vec<&Package> = all
        .iter()
        .filter(|package| package.directory.starts_with("adapters/"))
        .collect();
    let examples: Vec<&Package> = all
        .iter()
        .filter(|package| package.directory.starts_with("examples/"))
        .collect();
    let listed = libraries.len() + tooling.len() + adapters.len() + examples.len();
    if listed != all.len() {
        return Err(format!(
            "{} packages lie outside crates/, adapters/ and examples/",
            all.len() - listed
        ));
    }
    let mut out = front_matter(
        "Crates",
        "Crates",
        "Every package of the inference workspace, generated from cargo metadata.",
        1,
    );
    out.push_str(&format!(
        "# Crates\n\nGenerated by `inference-docs` from `cargo metadata --no-deps`: every package \
         of the workspace, with the `description` its `Cargo.toml` gives. There are {} packages, \
         all at version `{version}`. None is published to a registry; depend on them by Git tag, \
         as [Getting started](../getting-started.md#use-it-in-your-project) shows. A hosted or \
         native adapter does nothing until you turn on its feature. A library is used in Rust \
         under its name with underscores, `inference-stt` as `inference_stt`, unless its row says \
         otherwise.\n\n\
         ## Libraries\n\nThe contracts, the graph and its runtime, and the optional session API. \
         Each works on its own; a library that needs no adapter links none.\n\n{}\n\
         ## Adapters\n\nOne package per provider or backend. Which capabilities each claims, and \
         what it measured, is in [Adapters](./adapters.md).\n\n{}\n\
         ## Examples\n\nRunnable programs (`publish = false`); [Examples](../examples/index.md) \
         says what each needs.\n\n{}\n\
         ## Test kit and tooling\n\nThe in-memory stores and conformance target the tests use, and \
         this site's generator (`publish = false`). Nothing outside the repository depends on \
         them.\n\n{}",
        all.len(),
        table(&libraries),
        table(&adapters),
        table(&examples),
        table(&tooling),
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(description: Option<&str>) -> Value {
        serde_json::json!({
            "workspace_root": "/w",
            "packages": [
                {
                    "name": "inference-docs",
                    "version": "1.0.0",
                    "manifest_path": "/w/crates/inference-docs/Cargo.toml",
                    "description": "Generates pages.",
                    "publish": [],
                    "features": {},
                    "targets": [{"kind": ["bin"], "name": "inference-docs"}]
                },
                {
                    "name": "inference-openai",
                    "version": "1.0.0",
                    "manifest_path": "/w/adapters/inference-openai/Cargo.toml",
                    "description": description,
                    "publish": null,
                    "features": {"default": ["duplex"], "duplex": [], "gpt-live": []},
                    "targets": [
                        {"kind": ["lib"], "name": "inference_openai"},
                        {"kind": ["test"], "name": "duplex"}
                    ]
                },
                {
                    "name": "stt-only",
                    "version": "1.0.0",
                    "manifest_path": "/w/examples/stt-only/Cargo.toml",
                    "description": "Example.",
                    "publish": [],
                    "features": {},
                    "targets": [{"kind": ["lib"], "name": "stt_only"}]
                }
            ]
        })
    }

    #[test]
    fn packages_are_sorted_grouped_by_directory_and_linked() {
        let page = page_for(&metadata(Some("Hosted | speech."))).unwrap();
        assert!(page.contains("# generated by inference-docs, do not edit"));
        assert!(page.contains("There are 3 packages, all at version `1.0.0`"));
        let adapters = page.find("## Adapters").unwrap();
        let openai = page.find("| [`inference-openai`]").unwrap();
        let examples = page.find("## Examples").unwrap();
        let stt = page.find("| [`stt-only`]").unwrap();
        let tooling = page.find("## Test kit and tooling").unwrap();
        let docs = page.find("| [`inference-docs`]").unwrap();
        assert!(adapters < openai && openai < examples && examples < stt);
        assert!(stt < tooling && tooling < docs);
        assert!(page.contains(
            "| [`inference-openai`](https://github.com/beyond10x/inference/tree/main/adapters/inference-openai) | `duplex` (default), `gpt-live` | Hosted \\| speech. |"
        ));
        assert!(page.contains("| none | Binary `inference-docs`. Generates pages. |"));
        assert!(page.contains("| Package | Features | What it is |\n|---|---|---|\n"));
        assert!(page.contains("used in Rust under its name with underscores"));
    }

    #[test]
    fn a_package_without_a_description_or_with_another_version_is_refused() {
        let error = page_for(&metadata(None)).unwrap_err();
        assert!(
            error.contains("inference-openai has no description"),
            "{error}"
        );
        let mut split = metadata(Some("x"));
        split["packages"][0]["version"] = "0.9.0".into();
        let error = page_for(&split).unwrap_err();
        assert!(error.contains("more than one version"), "{error}");
    }
}
