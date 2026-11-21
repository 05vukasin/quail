# shellcheck shell=bash
# Shared helpers for tools/datasets/*.sh. Source it, do not run it.
#
# Environment:
#   QUAIL_TESTDATA   target root (default: <repo>/../testdata)
#   QUAIL_MIN_FREE_GB  abort a download if free space would drop below this (default 30)

set -euo pipefail

DATASETS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$DATASETS_DIR/../.." && pwd)"
QUAIL_TESTDATA="${QUAIL_TESTDATA:-$REPO_ROOT/../testdata}"
QUAIL_MIN_FREE_GB="${QUAIL_MIN_FREE_GB:-30}"
# Generic client identification only; never put personal data here.
UA="dataset-fetch/1.0"
export HF_HUB_DISABLE_TELEMETRY=1

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

# dataset_dir NAME -> creates and prints $QUAIL_TESTDATA/NAME
dataset_dir() {
    local d="$QUAIL_TESTDATA/$1"
    mkdir -p "$d"
    (cd "$d" && pwd)
}

# free_gb DIR -> free space (GiB, integer) on the filesystem holding DIR
free_gb() {
    df -P -BG "$1" | awk 'NR==2 { sub(/G$/, "", $4); print $4 }'
}

# need_space DIR EST_GB -> abort unless free - EST_GB >= QUAIL_MIN_FREE_GB
need_space() {
    local dir="$1" est="$2" free
    mkdir -p "$dir"
    free="$(free_gb "$dir")"
    if (( free - est < QUAIL_MIN_FREE_GB )); then
        die "not enough disk: ${free} GiB free, need ~${est} GiB, floor ${QUAIL_MIN_FREE_GB} GiB"
    fi
    log "disk ok: ${free} GiB free, this dataset needs ~${est} GiB"
}

# hf_bin -> path of the huggingface CLI (hf or huggingface-cli)
hf_bin() {
    if command -v hf >/dev/null 2>&1; then echo hf
    elif command -v huggingface-cli >/dev/null 2>&1; then echo huggingface-cli
    else die "huggingface CLI missing; install with: uv tool install huggingface_hub"
    fi
}

# hf_get REPO DEST [--include PATTERN ...] -> resumable, idempotent dataset download
hf_get() {
    local repo="$1" dest="$2"; shift 2
    log "hf download $repo -> $dest $*"
    "$(hf_bin)" download "$repo" --repo-type dataset --local-dir "$dest" "$@" >/dev/null
}

# fetch URL DEST -> resumable download; skips if DEST exists
fetch() {
    local url="$1" dest="$2"
    if [[ -s "$dest" ]]; then log "have $(basename "$dest")"; return 0; fi
    mkdir -p "$(dirname "$dest")"
    log "curl $url"
    curl -fsSL --retry 5 --retry-delay 5 -C - -A "$UA" -o "$dest.part" "$url"
    mv "$dest.part" "$dest"
}

# sha256_ok FILE HEX -> true if FILE hashes to HEX
sha256_ok() {
    [[ "$(sha256sum "$1" | cut -d' ' -f1)" == "$2" ]]
}

# done_marker DIR -> marks a completed fetch (used by run_all/manifest)
done_marker() { date -u +%FT%TZ > "$1/.fetch-complete"; }
