#!/usr/bin/env sh
# Fetches the CPV 2008 sources and rebuilds crates/quail-items/data/cpv2008.tsv.
# Usage: tools/cpv/fetch.sh <work-dir>   (needs curl, unzip, pdftotext from poppler, python3)
set -eu
work=${1:?work dir}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$work/xml"
# EU CPV 2008 (Regulation (EC) No 213/2008), all EU languages, from TED.
curl -sSfL -o "$work/cpv_2008_xml.zip" "https://ted.europa.eu/documents/d/ted/cpv_2008_xml"
unzip -o -q "$work/cpv_2008_xml.zip" -d "$work/xml"
# Serbian translation by the Public Procurement Office (UJN), revised 2018.
curl -sSfL -o "$work/cpv_sr.pdf" \
  "https://eupodrska.ujn.gov.rs/wp-content/uploads/2018/11/CPV_Opsti-recnik-javne-nabavke_revidirani.pdf"
pdftotext -layout "$work/cpv_sr.pdf" "$work/cpv_sr.txt"
python3 -I "$here/build_index.py" "$work/xml/cpv_2008.xml" "$work/cpv_sr.txt" \
  "$here/../../crates/quail-items/data/cpv2008.tsv"
