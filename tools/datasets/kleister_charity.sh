#!/usr/bin/env bash
# Kleister-Charity (applicaai/kleister-charity): UK charity annual reports (PDF).
# The PDFs are git-annex objects on a public S3 bucket. This mirrors upstream
# annex-get-test-documents-from-s3.sh (dev-0 + test-A only, ~2.2 GB) without
# needing git-annex: each documents/<name> symlink target names the annex key
# (SHA256E-s<size>--<sha256>.<ext>), which is fetched and sha256-verified.
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir kleister-charity)"
need_space "$dest" 3
annex_url="https://applica-public.s3-eu-west-1.amazonaws.com/kleister-charity-git-annex"
fetch "https://codeload.github.com/applicaai/kleister-charity/tar.gz/refs/heads/master" "$dest/kleister-charity.tar.gz"
if [[ ! -d "$dest/documents" ]]; then
    tar -xzf "$dest/kleister-charity.tar.gz" -C "$dest" --strip-components=1
fi
cd "$dest" || exit 1
n=0; fail=0
while read -r name; do
    f="documents/$name"
    if [[ -f "$f" && ! -L "$f" ]]; then continue; fi   # already materialised
    [[ -L "$f" ]] || { log "no annex link for $name"; fail=$((fail+1)); continue; }
    key="$(basename "$(readlink "$f")")"
    sha="${key##*--}"; sha="${sha%%.*}"
    if ! curl -fsSL --retry 5 --retry-delay 5 -C - -A "$UA" -o "$f.part" "$annex_url/$key"; then
        log "download failed: $name"; fail=$((fail+1)); continue
    fi
    if sha256_ok "$f.part" "$sha"; then
        rm -f "$f"; mv "$f.part" "$f"; n=$((n+1))
    else
        log "sha256 mismatch: $name"; rm -f "$f.part"; fail=$((fail+1))
    fi
    if (( n > 0 && n % 100 == 0 )); then log "fetched $n documents"; fi
done < <(xzcat dev-0/in.tsv.xz test-A/in.tsv.xz | cut -f1 | sort -u)
log "fetched $n new documents, $fail failures"
# Remaining dangling symlinks are train documents, skipped on purpose.
if (( fail == 0 )); then done_marker "$dest"; else exit 1; fi
