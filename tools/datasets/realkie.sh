#!/usr/bin/env bash
# RealKIE (indico): enterprise key-information extraction, public Wasabi bucket
# s3://project-fruitfly (anonymous). Full set is ~50 GB, so we cap at ~1 GB:
#   fcc_invoices        CSV splits + all PDFs + OCR JSON
#   nda                 CSV splits + all PDFs
#   resource_contracts  CSV splits + PDFs of the test and val splits only
# Page PNG renders (images/) are skipped everywhere.
# Uses `aws s3 sync --no-sign-request` when the aws CLI exists, otherwise a
# stdlib S3 lister plus curl (identical result).
source "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"
dest="$(dataset_dir realkie)"
need_space "$dest" 2
endpoint="https://s3.us-east-2.wasabisys.com"
bucket="project-fruitfly"

# get_prefix PREFIX REGEX -> fetch keys under PREFIX whose key matches REGEX
get_prefix() {
    local prefix="$1" re="$2" size key
    if command -v aws >/dev/null 2>&1 && [[ "$re" == "." ]]; then
        aws s3 sync --no-sign-request --endpoint-url="$endpoint" \
            "s3://$bucket/$prefix" "$dest/$prefix"
        return
    fi
    while IFS=$'\t' read -r size key; do
        [[ "$key" =~ $re ]] || continue
        if [[ -f "$dest/$key" && "$(stat -c %s "$dest/$key")" == "$size" ]]; then continue; fi
        rm -f "$dest/$key"
        fetch "$endpoint/$bucket/$key" "$dest/$key"
    done < <(python3 -I "$DATASETS_DIR/lib/s3_list.py" "$endpoint" "$bucket" "$prefix")
}

for sub in fcc_invoices nda resource_contracts; do
    get_prefix "$sub/" "^$sub/[^/]+\.(csv|json)$"
done
get_prefix "fcc_invoices/files/" "."
get_prefix "fcc_invoices/ocr/" "."
get_prefix "nda/files/" "."

# resource_contracts: only PDFs referenced by test.csv and val.csv
while read -r key; do
    fetch "$endpoint/$bucket/$key" "$dest/$key"
done < <(python3 -I -c '
import csv, sys
csv.field_size_limit(1 << 30)
for path in sys.argv[1:]:
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            print(row["document_path"])
' "$dest/resource_contracts/test.csv" "$dest/resource_contracts/val.csv" | sort -u)
done_marker "$dest"
