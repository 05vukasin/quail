//! Command-line arguments (clap derive).

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use quail_core::{Limits, Profile};

/// quail: rich per-file metadata for business documents, offline.
#[derive(Debug, Parser)]
#[command(name = "quail", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Process files or directory trees into JSONL records (one per file).
    // Boxed: ScanArgs is ~290 bytes on Windows (larger PathBuf), far above the others.
    Scan(Box<ScanArgs>),
    /// Process one file and pretty-print its record (and, with --text, its pages).
    Inspect(InspectArgs),
    /// List every fact key and step this build knows, with one-line docs.
    Keys(KeysArgs),
    /// Versions of every step: the pipeline fingerprint used for re-indexing decisions.
    Version(VersionArgs),
}

/// `--profile` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ProfileArg {
    /// Rules and statistics only.
    Fast,
    /// Plus static embeddings and small linear models (default).
    #[default]
    Standard,
    /// Plus neural models.
    Deep,
}

impl From<ProfileArg> for Profile {
    fn from(p: ProfileArg) -> Self {
        match p {
            ProfileArg::Fast => Profile::Fast,
            ProfileArg::Standard => Profile::Standard,
            ProfileArg::Deep => Profile::Deep,
        }
    }
}

/// Per-file limit overrides shared by `scan` and `inspect`.
#[derive(Clone, Debug, Default, Args)]
pub struct LimitArgs {
    /// Largest file read at all (e.g. `64M`, `1G`; default 512M). Bigger files are not read:
    /// they get a `failed` record naming the limit.
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub max_bytes: Option<u64>,
    /// Bytes a document may inflate to while it is decoded (ZIP entries, PDF streams; e.g.
    /// `512M`; default 256M). Past it the extractor stops with a partial document.
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub max_decompressed: Option<u64>,
    /// Pages extracted per document (default 2000).
    #[arg(long, value_name = "N")]
    pub max_pages: Option<u32>,
    /// Wall time per file (e.g. `30s`, `500ms`, `2m`; a bare number is seconds; default 2m).
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub timeout: Option<Duration>,
}

impl LimitArgs {
    /// Defaults with the overrides applied.
    pub fn limits(&self) -> Limits {
        let mut l = Limits::default();
        if let Some(b) = self.max_bytes {
            l.max_bytes = b;
        }
        if let Some(d) = self.max_decompressed {
            l.max_decompressed = d;
        }
        if let Some(p) = self.max_pages {
            l.max_pages = p;
        }
        if let Some(t) = self.timeout {
            l.timeout = t;
        }
        l
    }
}

/// `quail scan`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent flags
pub struct ScanArgs {
    /// Files and directories to walk.
    #[arg(
        value_name = "PATH",
        required_unless_present = "manifest",
        conflicts_with = "manifest"
    )]
    pub paths: Vec<PathBuf>,
    /// Read the files named by a manifest instead (`path` or `locator<TAB>path` per line;
    /// `-` reads it from stdin).
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,
    /// Write records here instead of stdout (`-` is stdout).
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Also write each extracted document as one JSON line
    /// `{"locator","blake3","document"}` (the quail-eval sidecar format).
    #[arg(long, value_name = "FILE")]
    pub dump_documents: Option<PathBuf>,
    /// zstd-compress the records (needs the `zstd` build feature).
    #[arg(long)]
    pub zstd: bool,
    /// Work profile.
    #[arg(long, value_enum, default_value_t = ProfileArg::Standard)]
    pub profile: ProfileArg,
    /// Worker threads (default: CPUs - 1).
    #[arg(short = 'j', long, value_name = "N")]
    pub threads: Option<usize>,
    /// Files between source and sink (default: 2 x threads).
    #[arg(long, value_name = "N")]
    pub max_in_flight: Option<usize>,
    /// Bytes of files in flight (e.g. `512M`; default 1G).
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub max_in_flight_bytes: Option<u64>,
    /// Write records in source order (a slow file holds back the ones after it).
    #[arg(long)]
    pub ordered: bool,
    /// Only files whose path relative to its root matches one of these globs (`*` crosses `/`).
    #[arg(long, value_name = "GLOB")]
    pub include: Vec<String>,
    /// Skip files and directories matching these globs.
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
    /// Only these extensions, comma-separated, case-insensitive (`pdf,docx`).
    #[arg(long, value_name = "EXT", value_delimiter = ',')]
    pub ext: Vec<String>,
    /// Skip files and directories whose name starts with `.`.
    #[arg(long)]
    pub skip_hidden: bool,
    /// Walk in sorted order (reproducible runs).
    #[arg(long)]
    pub sorted: bool,
    /// Follow symbolic links.
    #[arg(long)]
    pub follow_symlinks: bool,
    /// Limit overrides.
    #[command(flatten)]
    pub limits: LimitArgs,
    /// No progress line and no summary on stderr.
    #[arg(short, long)]
    pub quiet: bool,
    /// Print the final stats as one JSON object on stderr instead of the summary.
    #[arg(long)]
    pub json_stats: bool,
    /// Process every file in a worker child process: an abort, an out-of-memory, a stack
    /// overflow or a hang costs that file a `failed` record ("worker crashed: ..."), not the
    /// run. Workers read the files themselves (`-j` workers, each loads its own models).
    #[arg(long)]
    pub isolate: bool,
    /// Address space per worker (`RLIMIT_AS`, e.g. `2G`; default 1G). Budget about the models
    /// plus 4x the largest file accepted.
    #[arg(long, value_name = "SIZE", value_parser = parse_size, requires = "isolate")]
    pub worker_mem: Option<u64>,
    /// Retry a file whose worker crashed once, in a fresh worker (the default).
    #[arg(long, overrides_with = "no_retry", requires = "isolate")]
    pub retry_crashed: bool,
    /// Record a worker crash as final, without a retry.
    #[arg(long, overrides_with = "retry_crashed", requires = "isolate")]
    pub no_retry: bool,
}

/// `quail inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// The file.
    pub file: PathBuf,
    /// Also print every page's text, block by block, with its zone.
    #[arg(long)]
    pub text: bool,
    /// Print JSON instead (`{"record": .., "document": ..}` with --text).
    #[arg(long)]
    pub json: bool,
    /// Work profile.
    #[arg(long, value_enum, default_value_t = ProfileArg::Standard)]
    pub profile: ProfileArg,
    /// Limit overrides.
    #[command(flatten)]
    pub limits: LimitArgs,
}

/// `quail keys`.
#[derive(Debug, Args)]
pub struct KeysArgs {
    /// Only keys and steps starting with this prefix (`docx.`, `hash`).
    pub prefix: Option<String>,
    /// JSON output.
    #[arg(long)]
    pub json: bool,
}

/// `quail version`.
#[derive(Debug, Args)]
pub struct VersionArgs {
    /// Profile whose pipeline is described.
    #[arg(long, value_enum, default_value_t = ProfileArg::Standard)]
    pub profile: ProfileArg,
    /// JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Parses `123`, `64k`, `64K`, `64KiB`, `10M`, `1G`, `2T` (binary multiples).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("invalid size {s:?}"))?;
    let shift = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => return Err(format!("invalid size unit in {s:?} (use K, M, G or T)")),
    };
    n.checked_mul(1u64 << shift)
        .ok_or_else(|| format!("size {s:?} is too large"))
}

/// Parses `90` (seconds), `30s`, `500ms`, `2m`, `1h`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let t = s.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: u64 = num.parse().map_err(|_| format!("invalid duration {s:?}"))?;
    Ok(match unit.trim() {
        "" | "s" => Duration::from_secs(n),
        "ms" => Duration::from_millis(n),
        "m" | "min" => Duration::from_secs(n.saturating_mul(60)),
        "h" => Duration::from_secs(n.saturating_mul(3600)),
        _ => {
            return Err(format!(
                "invalid duration unit in {s:?} (use ms, s, m or h)"
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations() {
        assert_eq!(parse_size("123"), Ok(123));
        assert_eq!(parse_size("64k"), Ok(64 << 10));
        assert_eq!(parse_size("10MiB"), Ok(10 << 20));
        assert_eq!(parse_size("1G"), Ok(1 << 30));
        assert!(parse_size("1X").is_err());
        assert!(parse_size("M").is_err());
        assert!(parse_size("99999999999T").is_err());
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_mins(2)));
        assert!(parse_duration("2d").is_err());
    }

    #[test]
    fn limit_overrides() {
        let a = LimitArgs {
            max_decompressed: Some(1 << 30),
            max_pages: Some(5),
            ..LimitArgs::default()
        };
        let l = a.limits();
        assert_eq!(l.max_decompressed, 1 << 30);
        assert_eq!(l.max_pages, 5);
        assert_eq!(l.max_bytes, Limits::default().max_bytes);
        assert_eq!(
            LimitArgs::default().limits().max_decompressed,
            Limits::default().max_decompressed
        );
    }

    #[test]
    fn clap_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
