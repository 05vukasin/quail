//! File-level properties: `docProps/*`, `word/settings.xml`, `[Content_Types].xml` and the
//! CLM placeholder form in `customXml/`.

use quail_core::{Emitter, FactKey, Value};

use crate::keys;
use crate::package::Package;
use crate::rels::Rels;
use crate::util::{MAX_LIST, clean};
use crate::xml::{Ev, Flow, attr, attr_exact, name, on, val, walk};

/// Facts gathered so far, emitted together at the end.
pub(crate) type Facts = Vec<(FactKey, Value)>;

/// Children of the root element with their text: `(local name, text)`.
fn root_fields(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let Some(reader) = pkg.open_part(part) else {
        return fields;
    };
    let mut depth = 0usize;
    let mut cur: Option<(String, String)> = None;
    let r = walk(reader, out, |ev| {
        match ev {
            Ev::Start(e) => {
                depth += 1;
                if depth == 2 {
                    cur = Some((name(e).to_owned(), String::new()));
                }
            }
            Ev::Text(t) => {
                if depth == 2
                    && let Some((_, s)) = cur.as_mut()
                    && s.len() < 64 << 10
                {
                    s.push_str(t);
                }
            }
            Ev::End(_) => {
                if depth == 2
                    && let Some(f) = cur.take()
                    && fields.len() < 1000
                {
                    fields.push(f);
                }
                depth = depth.saturating_sub(1);
            }
            Ev::Empty(_) => {}
        }
        Flow::Go
    });
    if let Err(stop) = r {
        warn.push(stop.describe(part));
    }
    fields
}

fn text_fact(facts: &mut Facts, key: FactKey, raw: &str) {
    let v = clean(raw);
    if !v.is_empty() {
        facts.push((key, Value::text(v)));
    }
}

fn int_fact(facts: &mut Facts, key: FactKey, raw: &str) {
    if let Ok(v) = raw.trim().parse::<i64>() {
        facts.push((key, Value::Int(v)));
    }
}

/// `docProps/core.xml`.
pub(crate) fn core(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
    facts: &mut Facts,
) {
    for (n, v) in root_fields(pkg, part, out, warn) {
        let key = match n.as_str() {
            "title" => keys::TITLE,
            "subject" => keys::SUBJECT,
            "creator" => keys::CREATOR,
            "lastModifiedBy" => keys::LAST_MODIFIED_BY,
            "created" => keys::CREATED,
            "modified" => keys::MODIFIED,
            "lastPrinted" => keys::LAST_PRINTED,
            "keywords" => keys::KEYWORDS,
            "category" => keys::CATEGORY,
            "description" => keys::DESCRIPTION,
            "revision" => {
                match v.trim().parse::<i64>() {
                    Ok(r) => facts.push((keys::REVISION, Value::Int(r))),
                    Err(_) => text_fact(facts, keys::REVISION, &v),
                }
                continue;
            }
            _ => continue,
        };
        text_fact(facts, key, &v);
    }
}

/// `docProps/app.xml`.
pub(crate) fn app(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
    facts: &mut Facts,
) {
    for (n, v) in root_fields(pkg, part, out, warn) {
        match n.as_str() {
            "Application" => text_fact(facts, keys::APPLICATION, &v),
            "AppVersion" => text_fact(facts, keys::APP_VERSION, &v),
            "Company" => text_fact(facts, keys::COMPANY, &v),
            "Manager" => text_fact(facts, keys::MANAGER, &v),
            "Template" => text_fact(facts, keys::TEMPLATE, &v),
            "TotalTime" => int_fact(facts, keys::TOTAL_TIME, &v),
            "Pages" => int_fact(facts, keys::PAGES, &v),
            "Words" => int_fact(facts, keys::WORDS, &v),
            "Characters" => int_fact(facts, keys::CHARACTERS, &v),
            "Paragraphs" => int_fact(facts, keys::APP_PARAGRAPHS, &v),
            _ => {}
        }
    }
}

/// `docProps/custom.xml`: `<property name="..."><vt:lpwstr>value</vt:lpwstr></property>`.
pub(crate) fn custom(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
    facts: &mut Facts,
) {
    let Some(reader) = pkg.open_part(part) else {
        return;
    };
    let mut props: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let mut count = 0i64;
    let r = walk(reader, out, |ev| {
        match ev {
            Ev::Start(e) | Ev::Empty(e) if name(e) == "property" => {
                count += 1;
                let n = clean(&attr_exact(e, "name").unwrap_or_default());
                if matches!(ev, Ev::Start(_)) {
                    cur = Some((n, String::new()));
                } else if props.len() < MAX_LIST {
                    props.push((n, String::new()));
                }
            }
            Ev::Text(t) => {
                if let Some((_, v)) = cur.as_mut()
                    && v.len() < 64 << 10
                {
                    v.push_str(t);
                }
            }
            Ev::End("property") => {
                if let Some(p) = cur.take()
                    && props.len() < MAX_LIST
                {
                    props.push(p);
                }
            }
            _ => {}
        }
        Flow::Go
    });
    if let Err(stop) = r {
        warn.push(stop.describe(part));
    }
    facts.push((keys::CUSTOM_PROPERTY_COUNT, Value::Int(count)));
    if !props.is_empty() {
        facts.push((
            keys::CUSTOM_PROPERTY_NAMES,
            Value::List(props.iter().map(|(n, _)| Value::text(n.as_str())).collect()),
        ));
        facts.push((
            keys::CUSTOM_PROPERTIES,
            Value::List(
                props
                    .iter()
                    .map(|(n, v)| Value::text(format!("{n}={}", clean(v))))
                    .collect(),
            ),
        ));
    }
}

/// An rsid as Word writes it: exactly eight hex digits.
pub(crate) fn parse_rsid(v: &str) -> Option<u64> {
    let v = v.trim();
    (v.len() == 8 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(v, 16).ok())
        .flatten()
}

/// `word/settings.xml`: rsids, attached template, revision tracking, protection.
pub(crate) fn settings(
    pkg: &mut Package<'_>,
    part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
    facts: &mut Facts,
) {
    let Some(reader) = pkg.open_part(part) else {
        return;
    };
    let mut rsids: Vec<Value> = Vec::new();
    let mut root: Option<u64> = None;
    let mut bad = 0usize;
    let mut in_rsids = false;
    let mut template_rid: Option<String> = None;
    let mut track = false;
    let mut protection: Option<String> = None;
    let r = walk(reader, out, |ev| {
        match ev {
            Ev::Start(e) | Ev::Empty(e) => match name(e) {
                "rsids" => in_rsids = matches!(ev, Ev::Start(_)),
                "rsidRoot" if in_rsids => match val(e).as_deref().and_then(parse_rsid) {
                    Some(v) => root = Some(v),
                    None => bad += 1,
                },
                "rsid" if in_rsids => match val(e).as_deref().and_then(parse_rsid) {
                    Some(v) => {
                        if rsids.len() < 1_000_000 {
                            rsids.push(Value::U64(v));
                        }
                    }
                    None => bad += 1,
                },
                "attachedTemplate" => {
                    template_rid = attr(e, "id").map(std::borrow::Cow::into_owned)
                }
                "trackRevisions" => track = on(e),
                "documentProtection" => {
                    let enforced = attr(e, "enforcement")
                        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "on"));
                    if enforced {
                        protection = attr(e, "edit").map(|v| clean(&v));
                    }
                }
                _ => {}
            },
            Ev::End("rsids") => in_rsids = false,
            _ => {}
        }
        Flow::Go
    });
    if let Err(stop) = r {
        warn.push(stop.describe(part));
    }
    if bad > 0 {
        warn.push(format!(
            "{part}: {bad} rsid values not 8 hex digits, ignored"
        ));
    }
    if let Some(v) = root {
        facts.push((keys::RSID_ROOT, Value::U64(v)));
    }
    if !rsids.is_empty() {
        facts.push((
            keys::RSID_COUNT,
            Value::Int(i64::try_from(rsids.len()).unwrap_or(i64::MAX)),
        ));
        facts.push((keys::RSIDS, Value::List(rsids)));
    }
    facts.push((keys::TRACK_REVISIONS, Value::Bool(track)));
    if let Some(p) = protection.filter(|p| !p.is_empty() && p != "none") {
        facts.push((keys::PROTECTION, Value::text(p)));
    }
    if let Some(rid) = template_rid {
        let rels = Rels::read(pkg, part, out, warn);
        if let Some(rel) = rels.by_id(&rid) {
            text_fact(facts, keys::ATTACHED_TEMPLATE, &rel.target);
        }
    }
}

/// Content type of `main_part` from `[Content_Types].xml`.
pub(crate) fn main_content_type(
    pkg: &mut Package<'_>,
    main_part: &str,
    out: &Emitter,
    warn: &mut Vec<String>,
) -> Option<String> {
    let reader = pkg.open_part("[Content_Types].xml")?;
    let want = format!("/{}", main_part.trim_start_matches('/'));
    let mut found = None;
    let r = walk(reader, out, |ev| {
        if let Ev::Start(e) | Ev::Empty(e) = ev
            && name(e) == "Override"
            && attr_exact(e, "PartName").is_some_and(|p| p.eq_ignore_ascii_case(&want))
        {
            found = attr_exact(e, "ContentType").map(|c| clean(&c));
            return Flow::Stop;
        }
        Flow::Go
    });
    if let Err(stop) = r {
        warn.push(stop.describe("[Content_Types].xml"));
    }
    found
}

const CLM_MARKER: &str = "contractplaceholders";
const ENTITY_NAME: &str = "PlaceholderLegalEntityName";

/// The CLM placeholder form (`SciQuest` / Contract Express `sqph:contractplaceholders`) some
/// contract platforms store in `customXml/item*.xml`: the filled legal-entity names of the
/// First and Second Party. Unfilled `[[ ... ]]` placeholders are ignored.
pub(crate) fn clm(pkg: &mut Package<'_>, out: &Emitter, warn: &mut Vec<String>, facts: &mut Facts) {
    let items: Vec<String> = pkg
        .names()
        .iter()
        .filter(|n| is_custom_item(n))
        .cloned()
        .collect();
    let mut first: Option<String> = None;
    let mut second: Option<String> = None;
    for item in items {
        if first.is_some() && second.is_some() {
            break;
        }
        let Some(reader) = pkg.open_part(&item) else {
            continue;
        };
        let mut marked = false;
        // Party being read (true = first) and the entity name found in it.
        let mut party: Option<(bool, usize)> = None;
        let mut depth = 0usize;
        let mut in_name = false;
        let mut found = String::new();
        let r = walk(reader, out, |ev| {
            match ev {
                Ev::Start(e) => {
                    depth += 1;
                    let full = e.name().0;
                    if !marked
                        && (full.to_ascii_lowercase().contains(CLM_MARKER)
                            || e.attributes_raw().to_ascii_lowercase().contains(CLM_MARKER))
                    {
                        marked = true;
                    }
                    match name(e) {
                        "FirstParty" if party.is_none() => party = Some((true, depth)),
                        "SecondParty" if party.is_none() => party = Some((false, depth)),
                        ENTITY_NAME if party.is_some() => {
                            in_name = true;
                            found.clear();
                        }
                        _ => {}
                    }
                }
                Ev::Text(t) => {
                    if in_name && found.len() < 4096 {
                        found.push_str(t);
                    }
                }
                Ev::End(n) => {
                    if n == ENTITY_NAME && in_name {
                        in_name = false;
                        let v = clean(&found);
                        let placeholder = v.starts_with("[[") && v.ends_with("]]");
                        if let Some((is_first, _)) = party
                            && marked
                            && !v.is_empty()
                            && !placeholder
                        {
                            let slot = if is_first { &mut first } else { &mut second };
                            if slot.is_none() {
                                *slot = Some(v);
                            }
                        }
                    }
                    if party.is_some_and(|(_, d)| d == depth) {
                        party = None;
                    }
                    depth = depth.saturating_sub(1);
                }
                Ev::Empty(_) => {}
            }
            Flow::Go
        });
        if let Err(stop) = r {
            warn.push(stop.describe(&item));
        }
    }
    if let Some(v) = first {
        facts.push((keys::CLM_FIRST_PARTY, Value::text(v)));
    }
    if let Some(v) = second {
        facts.push((keys::CLM_SECOND_PARTY, Value::text(v)));
    }
}

fn is_custom_item(n: &str) -> bool {
    let lower = n.to_ascii_lowercase();
    lower
        .strip_prefix("customxml/item")
        .and_then(|r| r.strip_suffix(".xml"))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsids() {
        assert_eq!(parse_rsid("00A1B2C3"), Some(0x00A1_B2C3));
        assert_eq!(parse_rsid("ffffffff"), Some(0xFFFF_FFFF));
        assert_eq!(parse_rsid("A1B2C3"), None);
        assert_eq!(parse_rsid("00A1B2C3D"), None);
        assert_eq!(parse_rsid("00A1B2CG"), None);
        assert!(is_custom_item("customXml/item12.xml"));
        assert!(!is_custom_item("customXml/itemProps1.xml"));
    }
}
