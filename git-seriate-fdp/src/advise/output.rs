// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

use serde_json::{json, Value};

use super::actions::actions;
use super::decide::analyze;
use super::problems::{problems, Problem};
use super::{fixups, status, Input, Source};

/// Everything, with `report` (a protoback build report) read back into the
/// problems when given. Refused if the report isn't from the current
/// fixups.
/// `command` is how the generated files name the command (S7's wording).
pub fn advise(input: &Input, report: Option<&Value>, command: &str) -> Result<Value, String> {
    let a = analyze(input);
    let mut ps = problems(&a, input);
    if let Some(r) = report {
        super::report::apply(r, &a, &input.sources, &mut ps)?;
    }
    let (candidates, no_source) = actions(&ps, &input.sources);
    Ok(json!({
        "fixups": fixups::render(&a, &input.sources, command),
        "status": status::render(&a, input, &ps, command),
        "files": a.files.len(),
        "sources": input.sources.len(),
        "decisions": a.decisions.len(),
        "unreadable": a.unreadable,
        "problems": ps.iter().map(problem).collect::<Vec<_>>(),
        "candidates": candidates.iter().map(|c| json!({
            "group": c.group,
            "sources": c.sources.iter().map(|s| source(s)).collect::<Vec<_>>(),
            "problems": c.problems.iter().map(|p| problem(p)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "no_source": no_source.iter().map(|p| problem(p)).collect::<Vec<_>>(),
    }))
}

/// For each generated file given as `(name, text)`: `null` when current,
/// `"no stamp"`, or `{"new": [...], "gone": [...]}` (spec 0454 S8).
pub fn check(sources: &[Source], files: &[(String, String)]) -> Value {
    use super::check::{staleness, Staleness};
    let mut out = serde_json::Map::new();
    for (name, text) in files {
        let v = match staleness(text, sources) {
            None => Value::Null,
            Some(Staleness::NoStamp) => json!("no stamp"),
            Some(Staleness::Differs { new, gone }) => json!({ "new": new, "gone": gone }),
        };
        out.insert(name.clone(), v);
    }
    Value::Object(out)
}

fn problem(p: &Problem) -> Value {
    let mut v =
        json!({ "kind": p.kind.name(), "files": p.files, "detail": p.detail, "build": p.build });
    if let Some(subject) = &p.subject {
        v["subject"] = json!(subject);
    }
    v
}

fn source(s: &Source) -> Value {
    json!({
        "identity": s.identity,
        "build_time": s.build_time,
        "order_hint": s.order_hint,
        "group": s.group,
        "metadata": s.metadata.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect::<serde_json::Map<String, Value>>(),
    })
}
