//! Scoring records against gold: per-task counts, precision/recall/F1, abstentions,
//! calibration, breakdowns, status distribution and latency.
//!
//! Per document and task:
//!
//! - **labelled task**: single-valued tasks take the most confident prediction; it is a TP
//!   when it matches any gold label, else one FP and one FN. Multi-valued tasks match
//!   predictions (most confident first) one to one with gold labels: matched ones are TPs,
//!   unmatched predictions FPs, unmatched labels FNs. No prediction at all is an
//!   *abstention* (and its labels are FNs).
//! - **absent task**: every prediction is an FP; no prediction is a correct abstention.
//! - **unannotated task**: not scored.
//!
//! Calibration pools every scored prediction (correct = TP) into ten equal-width `conf` bins;
//! ECE is the support-weighted mean of |accuracy − mean conf|.

use std::collections::BTreeMap;

use quail_core::{FileRecord, Status};
use serde::{Deserialize, Serialize};

use crate::gold::{GoldDoc, Split};
use crate::normalize::label_matches;
use crate::records::{MatchedBy, RecordIndex, Records, value_strings};
use crate::tasks::{TaskMap, TaskSpec};
use crate::{Error, Result};

/// Version of the scoreboard JSON.
pub const SCOREBOARD_SCHEMA: u32 = 1;

/// Number of calibration bins.
pub const CALIBRATION_BINS: usize = 10;

/// TP/FP/FN and the metrics derived from them. A metric is `null` when undefined (no
/// prediction for precision, no gold for recall, neither for F1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    /// True positives.
    pub tp: u64,
    /// False positives.
    pub fp: u64,
    /// False negatives.
    #[serde(rename = "fn")]
    pub fn_: u64,
    /// tp / (tp + fp).
    pub precision: Option<f64>,
    /// tp / (tp + fn).
    pub recall: Option<f64>,
    /// 2tp / (2tp + fp + fn).
    pub f1: Option<f64>,
}

#[allow(clippy::cast_precision_loss)] // counts far below 2^52
fn ratio(num: u64, den: u64) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

impl Counts {
    fn add(&mut self, tp: u64, fp: u64, fn_: u64) {
        self.tp += tp;
        self.fp += fp;
        self.fn_ += fn_;
        self.precision = ratio(self.tp, self.tp + self.fp);
        self.recall = ratio(self.tp, self.tp + self.fn_);
        self.f1 = ratio(2 * self.tp, 2 * self.tp + self.fp + self.fn_);
    }
}

/// One calibration bin `[lo, hi)` (the last one includes 1.0).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bin {
    /// Lower edge.
    pub lo: f64,
    /// Upper edge.
    pub hi: f64,
    /// Predictions in the bin.
    pub n: u64,
    /// Mean confidence, `null` when empty.
    pub mean_conf: Option<f64>,
    /// Share of correct predictions, `null` when empty.
    pub accuracy: Option<f64>,
}

/// Reliability bins and expected calibration error.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Scored predictions.
    pub n: u64,
    /// Expected calibration error, `null` without predictions.
    pub ece: Option<f64>,
    /// The bins.
    pub bins: Vec<Bin>,
}

#[derive(Default)]
struct CalAcc {
    n: [u64; CALIBRATION_BINS],
    conf: [f64; CALIBRATION_BINS],
    correct: [u64; CALIBRATION_BINS],
}

impl CalAcc {
    #[allow(clippy::cast_precision_loss)]
    fn add(&mut self, conf: f32, correct: bool) {
        let c = f64::from(conf.clamp(0.0, 1.0));
        // An f32 0.9 is 0.89999998: nudge so a confidence lands in the bin it names.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = ((c * CALIBRATION_BINS as f64 + 1e-6) as usize).min(CALIBRATION_BINS - 1);
        self.n[b] += 1;
        self.conf[b] += c;
        self.correct[b] += u64::from(correct);
    }

    #[allow(clippy::cast_precision_loss)]
    fn finish(&self) -> Calibration {
        let total: u64 = self.n.iter().sum();
        let mut ece = 0.0;
        let bins = (0..CALIBRATION_BINS)
            .map(|b| {
                let n = self.n[b];
                let mean_conf = (n > 0).then(|| self.conf[b] / n as f64);
                let accuracy = ratio(self.correct[b], n);
                if let (Some(m), Some(a)) = (mean_conf, accuracy) {
                    ece += n as f64 / total as f64 * (a - m).abs();
                }
                Bin {
                    lo: b as f64 / CALIBRATION_BINS as f64,
                    hi: (b + 1) as f64 / CALIBRATION_BINS as f64,
                    n,
                    mean_conf,
                    accuracy,
                }
            })
            .collect();
        Calibration {
            n: total,
            ece: (total > 0).then_some(ece),
            bins,
        }
    }
}

/// Scores of one task.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskScore {
    /// Pooled over all scored documents.
    #[serde(flatten)]
    pub counts: Counts,
    /// Gold labels (support).
    pub support: u64,
    /// Documents where the task is labelled.
    pub docs_labelled: u64,
    /// Documents where the task is marked absent.
    pub docs_absent: u64,
    /// Labelled documents without any prediction.
    pub abstained: u64,
    /// `abstained / docs_labelled`.
    pub abstain_rate: Option<f64>,
    /// Absent documents with a prediction (each is at least one FP).
    pub predicted_on_absent: u64,
    /// Predictions dropped for being below `min_conf`.
    pub below_min_conf: u64,
    /// Reliability of `conf`.
    pub calibration: Calibration,
    /// Per dataset.
    pub by_dataset: BTreeMap<String, Counts>,
    /// Per split.
    pub by_split: BTreeMap<String, Counts>,
}

/// Status distribution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCounts {
    /// Records.
    pub total: u64,
    /// `ok`.
    pub ok: u64,
    /// `partial`.
    pub partial: u64,
    /// `failed`.
    pub failed: u64,
    /// Records with at least one warning.
    pub with_warnings: u64,
}

impl StatusCounts {
    fn add(&mut self, r: &FileRecord) {
        self.total += 1;
        match r.status {
            Status::Ok => self.ok += 1,
            Status::Partial { .. } => self.partial += 1,
            Status::Failed { .. } => self.failed += 1,
        }
        self.with_warnings += u64::from(!r.warnings.is_empty());
    }
}

/// Latency of one step, microseconds, nearest-rank percentiles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    /// Samples.
    pub n: u64,
    /// Mean.
    pub mean: f64,
    /// Median.
    pub p50: u64,
    /// 95th percentile.
    pub p95: u64,
    /// 99th percentile.
    pub p99: u64,
    /// Maximum.
    pub max: u64,
}

/// Latency of a sample of microsecond values.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn latency(mut v: Vec<u64>) -> Latency {
    if v.is_empty() {
        return Latency::default();
    }
    v.sort_unstable();
    let n = v.len();
    let pick = |p: f64| v[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    Latency {
        n: n as u64,
        mean: v.iter().sum::<u64>() as f64 / n as f64,
        p50: pick(0.50),
        p95: pick(0.95),
        p99: pick(0.99),
        max: v[n - 1],
    }
}

/// What the gold set and the run looked like.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Corpus {
    /// Gold documents after the split / dataset filter.
    pub gold_docs: u64,
    /// Of those, matched to a record and scored.
    pub scored_docs: u64,
    /// Matched by locator because no record had the hash.
    pub matched_by_locator: u64,
    /// Ids of gold documents without a record (a broken run or data root).
    pub unmatched: Vec<String>,
    /// Records in the run.
    pub records: u64,
    /// Unparsable JSONL lines.
    pub bad_record_lines: u64,
    /// Gold labels whose task is not in the task map, per task.
    pub unmapped_tasks: BTreeMap<String, u64>,
    /// BLAKE3 over the filtered gold set: two scoreboards are comparable only when equal.
    pub gold_digest: String,
}

/// One prediction of one task on one document, with its outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredOutcome {
    /// The value as predicted.
    pub value: String,
    /// Its confidence.
    pub conf: f32,
    /// The fact key it came from.
    pub key: String,
    /// Index of the fact in the record.
    pub fact: usize,
    /// Matched a gold label.
    pub correct: bool,
}

/// One task on one document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocTask {
    /// Gold values (display form); empty for an absent task.
    pub gold: Vec<String>,
    /// The task is marked absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// Scored predictions.
    pub pred: Vec<PredOutcome>,
    /// Gold labels with no matching prediction (display form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missed: Vec<String>,
    /// True positives.
    pub tp: u64,
    /// False positives.
    pub fp: u64,
    /// False negatives.
    #[serde(rename = "fn")]
    pub fn_: u64,
    /// Predictions dropped below `min_conf`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub below_min_conf: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl DocTask {
    /// Any FP or FN.
    pub fn is_mismatch(&self) -> bool {
        self.fp > 0 || self.fn_ > 0
    }
}

/// One scored document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocRow {
    /// Gold id (BLAKE3 hex).
    pub id: String,
    /// Dataset.
    pub dataset: String,
    /// Split.
    pub split: Split,
    /// Locator of the matched record.
    pub locator: String,
    /// `ok`, `partial` or `failed`.
    pub status: String,
    /// Per task.
    pub tasks: BTreeMap<String, DocTask>,
}

/// Which split to score.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitFilter {
    /// Development documents only (the default).
    #[default]
    Dev,
    /// Holdout only: release time.
    Holdout,
    /// Both.
    All,
}

impl SplitFilter {
    fn keeps(self, s: Split) -> bool {
        match self {
            Self::Dev => s == Split::Dev,
            Self::Holdout => s == Split::Holdout,
            Self::All => true,
        }
    }

    /// `dev`, `holdout` or `all`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Holdout => "holdout",
            Self::All => "all",
        }
    }
}

/// Options of [`score`].
#[derive(Clone, Debug, Default)]
pub struct ScoreOptions {
    /// Name of the run, shown in reports.
    pub label: String,
    /// Split to score.
    pub split: SplitFilter,
    /// Must be true to score holdout documents: the holdout is frozen and scored only for
    /// releases.
    pub allow_holdout: bool,
    /// Only this dataset.
    pub dataset: Option<String>,
    /// Global minimum confidence (a task's own `min_conf` wins when higher).
    pub min_conf: Option<f32>,
    /// Free key/value notes stored in the scoreboard (input paths, git revision, ...).
    pub inputs: BTreeMap<String, String>,
}

/// Everything measured in one run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scoreboard {
    /// [`SCOREBOARD_SCHEMA`].
    pub schema: u32,
    /// Name of the run.
    pub label: String,
    /// Split scored.
    pub split: SplitFilter,
    /// Dataset filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    /// Global minimum confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_conf: Option<f32>,
    /// Notes about the inputs.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Gold set and run sizes.
    pub corpus: Corpus,
    /// Status over every record of the run.
    pub status: StatusCounts,
    /// Status over the scored documents.
    pub status_scored: StatusCounts,
    /// Per task, for every task of the map that has gold in this set.
    pub tasks: BTreeMap<String, TaskScore>,
    /// Per step (`name@version`) and `total` (sum per file), over every record.
    pub latency: BTreeMap<String, Latency>,
    /// One row per scored document (values included: keep scoreboards out of git when the
    /// gold describes private documents).
    pub docs: Vec<DocRow>,
}

/// One candidate value for a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    /// Value as a string.
    pub value: String,
    /// Confidence.
    pub conf: f32,
    /// Fact key.
    pub key: String,
    /// Index of the fact in the record.
    pub fact: usize,
}

/// The predictions of `spec` in `rec`, most confident first (stable), and how many were
/// dropped below the confidence threshold. Single-valued tasks keep only the best one;
/// multi-valued tasks drop values that normalize to one already kept.
pub fn predictions(
    rec: &FileRecord,
    spec: &TaskSpec,
    min_conf: Option<f32>,
) -> (Vec<Prediction>, u64) {
    let threshold = match (spec.min_conf, min_conf) {
        (Some(a), Some(b)) => a.max(b),
        (a, b) => a.or(b).unwrap_or(0.0),
    };
    let mut out = Vec::new();
    let mut dropped = 0;
    for (i, f) in rec.facts.iter().enumerate() {
        if !spec.keys.iter().any(|k| k == f.key.as_str()) {
            continue;
        }
        for v in value_strings(&f.value) {
            if f.conf < threshold {
                dropped += 1;
                continue;
            }
            out.push(Prediction {
                value: v,
                conf: f.conf,
                key: f.key.to_string(),
                fact: i,
            });
        }
    }
    out.sort_by(|a, b| b.conf.total_cmp(&a.conf));
    if spec.multi {
        let mut seen = std::collections::HashSet::new();
        out.retain(|p| seen.insert(spec.normalize.apply(&p.value)));
    } else {
        out.truncate(1);
    }
    (out, dropped)
}

/// Scores one task of one gold document, or `None` when the task is not annotated in it.
pub fn score_doc_task(
    doc: &GoldDoc,
    task: &str,
    spec: &TaskSpec,
    preds: Vec<Prediction>,
) -> Option<DocTask> {
    let labels: Vec<_> = doc.labels_for(task).collect();
    let absent = doc.is_absent(task);
    if labels.is_empty() && !absent {
        return None;
    }
    let mut dt = DocTask {
        gold: labels.iter().map(|l| l.display()).collect(),
        absent,
        ..DocTask::default()
    };
    if absent {
        dt.fp = preds.len() as u64;
        dt.pred = preds.into_iter().map(|p| outcome(p, false)).collect();
        return Some(dt);
    }
    if spec.multi {
        let mut used = vec![false; labels.len()];
        for p in preds {
            let hit = labels
                .iter()
                .enumerate()
                .find(|(j, l)| !used[*j] && label_matches(l, spec.normalize, &p.value))
                .map(|(j, _)| j);
            if let Some(j) = hit {
                used[j] = true;
                dt.tp += 1;
            } else {
                dt.fp += 1;
            }
            dt.pred.push(outcome(p, hit.is_some()));
        }
        for (j, l) in labels.iter().enumerate() {
            if !used[j] {
                dt.fn_ += 1;
                dt.missed.push(l.display());
            }
        }
    } else if let Some(p) = preds.into_iter().next() {
        let ok = labels
            .iter()
            .any(|l| label_matches(l, spec.normalize, &p.value));
        if ok {
            dt.tp = 1;
        } else {
            dt.fp = 1;
            dt.fn_ = 1;
            dt.missed = dt.gold.clone();
        }
        dt.pred.push(outcome(p, ok));
    } else {
        dt.fn_ = 1;
        dt.missed = dt.gold.clone();
    }
    Some(dt)
}

fn outcome(p: Prediction, correct: bool) -> PredOutcome {
    PredOutcome {
        value: p.value,
        conf: p.conf,
        key: p.key,
        fact: p.fact,
        correct,
    }
}

/// Short status name of a record.
pub fn status_name(s: &Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Partial { .. } => "partial",
        Status::Failed { .. } => "failed",
    }
}

/// Gold documents kept by the split and dataset filters, sorted by id.
pub fn filter_gold<'a>(gold: &'a [GoldDoc], opts: &ScoreOptions) -> Result<Vec<&'a GoldDoc>> {
    if opts.split != SplitFilter::Dev && !opts.allow_holdout {
        return Err(Error::Invalid(
            "the holdout is frozen: scoring it needs allow_holdout (--allow-holdout), at release time only"
                .into(),
        ));
    }
    let mut kept: Vec<&GoldDoc> = gold
        .iter()
        .filter(|d| opts.split.keeps(d.split))
        .filter(|d| opts.dataset.as_ref().is_none_or(|ds| &d.dataset == ds))
        .collect();
    kept.sort_by_key(|d| d.id());
    Ok(kept)
}

fn gold_digest(gold: &[&GoldDoc]) -> String {
    let mut buf = Vec::new();
    for d in gold {
        // Serialization of a valid gold doc cannot fail; an empty entry would still change
        // the digest consistently.
        buf.extend(serde_json::to_vec(d).unwrap_or_default());
        buf.push(b'\n');
    }
    crate::blake3_hex(&buf)
}

/// Scores `records` against `gold` with the task map `tasks`.
pub fn score(
    records: &Records,
    gold: &[GoldDoc],
    tasks: &TaskMap,
    opts: &ScoreOptions,
) -> Result<Scoreboard> {
    let kept = filter_gold(gold, opts)?;
    let recs = &records.records;
    let index = RecordIndex::new(recs);

    let mut sb = Scoreboard {
        schema: SCOREBOARD_SCHEMA,
        label: opts.label.clone(),
        split: opts.split,
        dataset: opts.dataset.clone(),
        min_conf: opts.min_conf,
        inputs: opts.inputs.clone(),
        ..Scoreboard::default()
    };
    sb.corpus.gold_docs = kept.len() as u64;
    sb.corpus.records = recs.len() as u64;
    sb.corpus.bad_record_lines = records.bad_lines;
    sb.corpus.gold_digest = gold_digest(&kept);

    // Run-wide status and latency.
    let mut steps: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for r in recs {
        sb.status.add(r);
        let mut total = 0u64;
        for t in &r.timings {
            steps.entry(t.src.to_string()).or_default().push(t.micros);
            total = total.saturating_add(t.micros);
        }
        steps.entry("total".into()).or_default().push(total);
    }
    sb.latency = steps.into_iter().map(|(k, v)| (k, latency(v))).collect();

    let mut cal: BTreeMap<String, CalAcc> = BTreeMap::new();
    for doc in kept {
        for l in &doc.labels {
            if tasks.get(&l.task).is_none() {
                *sb.corpus.unmapped_tasks.entry(l.task.clone()).or_default() += 1;
            }
        }
        for t in &doc.absent {
            if tasks.get(t).is_none() {
                *sb.corpus.unmapped_tasks.entry(t.clone()).or_default() += 1;
            }
        }
        let Some((ri, by)) = index.find(recs, &doc.file) else {
            sb.corpus.unmatched.push(doc.id());
            continue;
        };
        let rec = &recs[ri];
        sb.corpus.scored_docs += 1;
        sb.corpus.matched_by_locator += u64::from(by == MatchedBy::Locator);
        sb.status_scored.add(rec);

        let mut row = DocRow {
            id: doc.id(),
            dataset: doc.dataset.clone(),
            split: doc.split,
            locator: rec.locator.clone(),
            status: status_name(&rec.status).into(),
            tasks: BTreeMap::new(),
        };
        for (name, spec) in &tasks.tasks {
            let (preds, dropped) = predictions(rec, spec, opts.min_conf);
            let Some(mut dt) = score_doc_task(doc, name, spec, preds) else {
                continue;
            };
            dt.below_min_conf = dropped;
            let ts = sb.tasks.entry(name.clone()).or_default();
            ts.counts.add(dt.tp, dt.fp, dt.fn_);
            ts.by_dataset
                .entry(doc.dataset.clone())
                .or_default()
                .add(dt.tp, dt.fp, dt.fn_);
            ts.by_split
                .entry(doc.split.as_str().into())
                .or_default()
                .add(dt.tp, dt.fp, dt.fn_);
            ts.below_min_conf += dropped;
            if dt.absent {
                ts.docs_absent += 1;
                ts.predicted_on_absent += u64::from(!dt.pred.is_empty());
            } else {
                ts.docs_labelled += 1;
                ts.support += dt.gold.len() as u64;
                ts.abstained += u64::from(dt.pred.is_empty());
            }
            let acc = cal.entry(name.clone()).or_default();
            for p in &dt.pred {
                acc.add(p.conf, p.correct);
            }
            row.tasks.insert(name.clone(), dt);
        }
        sb.docs.push(row);
    }
    for (name, ts) in &mut sb.tasks {
        ts.abstain_rate = ratio(ts.abstained, ts.docs_labelled);
        ts.calibration = cal.get(name).map(CalAcc::finish).unwrap_or_default();
    }
    Ok(sb)
}

/// Reads a scoreboard JSON file.
pub fn load_scoreboard(path: &std::path::Path) -> Result<Scoreboard> {
    let text = crate::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| Error::parse(path, e))
}

/// Writes a scoreboard as pretty JSON.
pub fn write_scoreboard(path: &std::path::Path, sb: &Scoreboard) -> Result<()> {
    let mut json = serde_json::to_string_pretty(sb)
        .map_err(|e| Error::Invalid(format!("serializing scoreboard: {e}")))?;
    json.push('\n');
    crate::write_file(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gold::{GoldFile, Label, MatchMode};
    use crate::normalize::Normalizer;
    use quail_core::{AnalyzerId, Fact, FactKey, Format, Timing, Value};

    fn spec(key: &str, multi: bool, normalize: Normalizer) -> TaskSpec {
        TaskSpec {
            keys: vec![key.into()],
            normalize,
            multi,
            min_conf: None,
            description: None,
        }
    }

    fn fact(key: &'static str, v: &str, conf: f32) -> Fact {
        Fact {
            key: FactKey::new(key),
            value: Value::text(v),
            conf,
            src: AnalyzerId::new("t.x", 1),
            evidence: None,
        }
    }

    fn record(hash: u8, facts: Vec<Fact>, status: Status, micros: u64) -> FileRecord {
        let mut all = vec![Fact {
            key: FactKey::new("hash.blake3"),
            value: Value::Bytes(vec![hash; 32].into()),
            conf: 1.0,
            src: AnalyzerId::new("hash.identity", 1),
            evidence: None,
        }];
        all.extend(facts);
        FileRecord {
            schema: 1,
            locator: format!("/x/{hash}.txt"),
            size: 1,
            format: Format::Text,
            pages: None,
            facts: all,
            steps: vec![],
            timings: vec![Timing {
                src: AnalyzerId::new("t.x", 1),
                micros,
            }],
            status,
            warnings: vec![],
        }
    }

    fn gold(hash: u8, labels: Vec<Label>, absent: &[&str]) -> GoldDoc {
        GoldDoc {
            file: GoldFile {
                blake3: Some(crate::hex(&[hash; 32])),
                locator_hint: None,
            },
            dataset: "syn".into(),
            split: Split::Dev,
            source: None,
            labels,
            absent: absent.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn multi_valued_set_semantics() {
        let s = spec("p.party", true, Normalizer::Company);
        let doc = gold(
            1,
            vec![
                Label::text("party", "Alpha Inc.", MatchMode::Normalized),
                Label::text("party", "Beta LLC", MatchMode::Normalized),
            ],
            &[],
        );
        let rec = record(
            1,
            vec![
                fact("p.party", "ALPHA", 0.9),
                fact("p.party", "alpha inc", 0.5), // duplicate after normalization
                fact("p.party", "Gamma", 0.4),
            ],
            Status::Ok,
            1,
        );
        let (preds, _) = predictions(&rec, &s, None);
        assert_eq!(preds.len(), 2);
        let dt = score_doc_task(&doc, "party", &s, preds).unwrap();
        assert_eq!((dt.tp, dt.fp, dt.fn_), (1, 1, 1));
        assert_eq!(dt.missed, vec!["Beta LLC"]);
    }

    #[test]
    fn single_valued_takes_most_confident() {
        let s = spec("p.date", false, Normalizer::Date);
        let doc = gold(
            1,
            vec![Label::text("d", "2001-02-03", MatchMode::Normalized)],
            &[],
        );
        let rec = record(
            1,
            vec![
                fact("p.date", "1999-01-01", 0.2),
                fact("p.date", "February 3, 2001", 0.8),
            ],
            Status::Ok,
            1,
        );
        let (preds, _) = predictions(&rec, &s, None);
        let dt = score_doc_task(&doc, "d", &s, preds).unwrap();
        assert_eq!((dt.tp, dt.fp, dt.fn_), (1, 0, 0));
        // With a threshold above the best, it abstains.
        let (preds, dropped) = predictions(&rec, &s, Some(0.9));
        assert_eq!(dropped, 2);
        let dt = score_doc_task(&doc, "d", &s, preds).unwrap();
        assert_eq!((dt.tp, dt.fp, dt.fn_), (0, 0, 1));
    }

    #[test]
    fn scoreboard_counts_calibration_and_latency() {
        let mut tasks = TaskMap::default();
        tasks
            .tasks
            .insert("iban".into(), spec("s.iban", true, Normalizer::Identifier));
        let gold = vec![
            gold(
                1,
                vec![Label::text(
                    "iban",
                    "RS35 2600 0560 1001 6113 79",
                    MatchMode::Normalized,
                )],
                &[],
            ),
            gold(2, vec![], &["iban"]),
            gold(
                3,
                vec![Label::text("iban", "X", MatchMode::Normalized)],
                &[],
            ),
            gold(
                9,
                vec![Label::text("other", "y", MatchMode::Normalized)],
                &[],
            ),
        ];
        let records = Records {
            records: vec![
                record(
                    1,
                    vec![fact("s.iban", "RS35260005601001611379", 0.95)],
                    Status::Ok,
                    10,
                ),
                record(
                    2,
                    vec![fact("s.iban", "DE00", 0.15)],
                    Status::Failed { error: "x".into() },
                    20,
                ),
                record(3, vec![], Status::Partial { problems: vec![] }, 30),
            ],
            bad_lines: 0,
        };
        let sb = score(&records, &gold, &tasks, &ScoreOptions::default()).unwrap();
        let t = &sb.tasks["iban"];
        assert_eq!((t.counts.tp, t.counts.fp, t.counts.fn_), (1, 1, 1));
        assert_eq!(t.counts.f1, Some(0.5));
        assert_eq!(t.abstained, 1);
        assert_eq!(t.abstain_rate, Some(0.5));
        assert_eq!(t.predicted_on_absent, 1);
        assert_eq!(t.calibration.n, 2);
        // bin 9: conf .95 correct; bin 1: conf .15 wrong -> ECE = (0.05 + 0.15) / 2
        assert!((t.calibration.ece.unwrap() - 0.10).abs() < 1e-6);
        assert_eq!(sb.corpus.unmatched, vec![crate::hex(&[9; 32])]);
        assert_eq!(sb.corpus.unmapped_tasks["other"], 1);
        assert_eq!(
            (sb.status.ok, sb.status.partial, sb.status.failed),
            (1, 1, 1)
        );
        assert_eq!(sb.latency["total"].p50, 20);
        assert_eq!(sb.latency["t.x@1"].p99, 30);
    }

    #[test]
    fn holdout_needs_permission() {
        let opts = ScoreOptions {
            split: SplitFilter::Holdout,
            ..ScoreOptions::default()
        };
        assert!(score(&Records::default(), &[], &TaskMap::default(), &opts).is_err());
    }

    #[test]
    fn percentiles_nearest_rank() {
        let l = latency((1..=100).collect());
        assert_eq!((l.p50, l.p95, l.p99, l.max), (50, 95, 99, 100));
        assert_eq!(latency(vec![]).n, 0);
    }
}
