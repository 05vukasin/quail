//! `quail keys` and `quail version`.

use std::fmt::Write as _;

use quail_core::{AnalyzerId, Format, Limits, Profile, SCHEMA_VERSION};
use quail_engine::{Engine, SNIFF_ID};

use crate::cli::{KeysArgs, VersionArgs};
use crate::registry::{self, STEPS};

/// Formats an extractor can be registered for.
const FORMATS: [Format; 6] = [
    Format::Pdf,
    Format::Docx,
    Format::Zip,
    Format::Image,
    Format::Text,
    Format::Unknown,
];

/// `quail keys`: every key and step with its doc.
pub fn keys(args: &KeysArgs) -> String {
    let prefix = args.prefix.as_deref().unwrap_or("");
    let tables = registry::key_tables();
    let steps: Vec<_> = STEPS
        .iter()
        .filter(|s| s.id.name.starts_with(prefix))
        .collect();
    if args.json {
        let keys: Vec<_> = tables
            .iter()
            .flat_map(|(krate, t)| t.iter().map(move |k| (krate, k)))
            .filter(|(_, k)| k.key.as_str().starts_with(prefix))
            .map(|(krate, k)| serde_json::json!({"key": k.key, "crate": krate, "doc": k.doc}))
            .collect();
        let steps: Vec<_> = steps
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id.to_string(),
                    "kind": s.kind.as_str(),
                    "formats": s.formats.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                    "doc": s.doc,
                })
            })
            .collect();
        return format!("{}\n", serde_json::json!({"steps": steps, "keys": keys}));
    }
    let mut s = String::new();
    let _ = writeln!(s, "steps");
    let w = steps
        .iter()
        .map(|s| s.id.to_string().len())
        .max()
        .unwrap_or(0);
    for st in &steps {
        let fmts = if st.formats.is_empty() {
            "any".to_owned()
        } else {
            st.formats
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        let _ = writeln!(
            s,
            "  {:w$}  {:9}  {:5}  {}",
            st.id.to_string(),
            st.kind.as_str(),
            fmts,
            st.doc
        );
    }
    for (krate, table) in tables {
        let keys: Vec<_> = table
            .iter()
            .filter(|k| k.key.as_str().starts_with(prefix))
            .collect();
        if keys.is_empty() {
            continue;
        }
        let _ = writeln!(s, "\n{krate} ({} keys)", keys.len());
        let w = keys.iter().map(|k| k.key.as_str().len()).max().unwrap_or(0);
        for k in keys {
            let _ = writeln!(s, "  {:w$}  {}", k.key.as_str(), k.doc);
        }
    }
    s
}

/// The ids that shape a record at `profile`, in run order: sniffing, the extractor of each
/// format, the analyzers of the plan.
pub fn pipeline(engine: &Engine) -> Vec<(String, AnalyzerId)> {
    let mut v = vec![("engine".to_owned(), SNIFF_ID)];
    for f in FORMATS {
        if let Some(id) = engine.extractor_for(f) {
            v.push((format!("extractor {}", f.as_str()), id));
        }
    }
    v.extend(engine.plan().map(|id| ("analyzer".to_owned(), id)));
    v
}

/// Short fingerprint of the pipeline: BLAKE3 over the schema version, the profile and every
/// step id. It changes exactly when some file's record could change; limits are not in it.
pub fn fingerprint(profile: Profile, steps: &[(String, AnalyzerId)]) -> String {
    let mut text = format!(
        "schema={SCHEMA_VERSION}\nprofile={}\n",
        profile_name(profile)
    );
    for (role, id) in steps {
        let _ = writeln!(text, "{role} {id}");
    }
    let d = quail_hash::blake3(text.as_bytes());
    crate::dump::hex(&d[..8])
}

/// `quail version`.
pub fn version(args: &VersionArgs) -> Result<String, String> {
    let profile: Profile = args.profile.into();
    let engine = registry::builder(profile, Limits::default())
        .build()
        .map_err(|e| e.to_string())?;
    let steps = pipeline(&engine);
    let fp = fingerprint(profile, &steps);
    let inactive: Vec<String> = engine.inactive().iter().map(ToString::to_string).collect();
    let features = features();
    if args.json {
        let v = serde_json::json!({
            "quail": env!("CARGO_PKG_VERSION"),
            "schema": SCHEMA_VERSION,
            "profile": profile,
            "fingerprint": fp,
            "steps": steps.iter().map(|(role, id)| serde_json::json!({
                "role": role, "id": id.to_string(), "name": id.name, "version": id.version,
            })).collect::<Vec<_>>(),
            "inactive": inactive,
            "features": features,
        });
        return Ok(format!("{v}\n"));
    }
    let mut s = String::new();
    let _ = writeln!(s, "quail {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(s, "schema       {SCHEMA_VERSION}");
    let _ = writeln!(s, "profile      {}", profile_name(profile));
    let _ = writeln!(s, "fingerprint  {fp}");
    let _ = writeln!(s, "features     {}", features.join(", "));
    let _ = writeln!(s, "steps");
    for (role, id) in &steps {
        let _ = writeln!(s, "  {role:<17} {id}");
    }
    if !inactive.is_empty() {
        let _ = writeln!(s, "inactive at this profile: {}", inactive.join(", "));
    }
    Ok(s)
}

/// Lowercase profile name, as on the command line.
pub fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::Fast => "fast",
        Profile::Standard => "standard",
        Profile::Deep => "deep",
    }
}

fn features() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "docx") {
        v.push("docx");
    }
    if cfg!(feature = "text") {
        v.push("text");
    }
    if cfg!(feature = "signals") {
        v.push("signals");
    }
    if cfg!(feature = "refs") {
        v.push("refs");
    }
    if cfg!(feature = "parties") {
        v.push("parties");
    }
    if cfg!(feature = "sketch") {
        v.push("sketch");
    }
    if cfg!(feature = "zstd") {
        v.push("zstd");
    }
    v
}
