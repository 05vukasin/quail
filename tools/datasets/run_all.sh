#!/usr/bin/env bash
# Fetch every dataset sequentially, smallest first. A failing dataset is logged
# and skipped; the run continues. Re-running resumes / skips completed ones.
# Afterwards writes $QUAIL_TESTDATA/MANIFEST.md (see manifest.py).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
set +e
order=(
    setimes_sr acord ledgar doc_split kleister_nda postocr_sr cuad maud
    sec_material_contracts fintabnet_otsl gleif_lei rvl_cdip_n realkie
    omnidocbench tabmepp rvl_cdip_n_mp kleister_charity doclaynet
)
failed=()
for name in "${order[@]}"; do
    log "=== $name"
    if ! bash "$DATASETS_DIR/$name.sh"; then
        log "=== $name FAILED"
        failed+=("$name")
    fi
done
log "free disk: $(free_gb "$QUAIL_TESTDATA") GiB"
if command -v uv >/dev/null 2>&1; then
    uv run --quiet --no-project --with pyarrow python -I "$DATASETS_DIR/manifest.py" "$QUAIL_TESTDATA"
else
    python3 -I "$DATASETS_DIR/manifest.py" "$QUAIL_TESTDATA"
fi
if (( ${#failed[@]} )); then log "failed: ${failed[*]}"; exit 1; fi
