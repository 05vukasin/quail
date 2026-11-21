#!/usr/bin/env python3
"""Builds quail-items' compact CPV 2008 index (code, English label, Serbian Latin label).

Sources (fetch with `fetch.sh`, both public):
- EU CPV 2008, Regulation (EC) No 213/2008, XML from TED: the code list and the English
  labels. Reuse is allowed under Commission Decision 2011/833/EU (source acknowledged).
- The Serbian translation published by the Serbian Public Procurement Office (UJN,
  "Opšti rečnik nabavke", revised 2018), a PDF printed from a spreadsheet with the columns
  Code / Text_EN / Text_SRB (Cyrillic) / Text_SRB_Latin. Official public-procurement
  material (not protected by copyright, Art. 6 of the Serbian copyright law).

From the PDF only the Cyrillic column is taken: it is the only column that can be told
apart by script on every wrapped line. It is transliterated to Latin here (the mapping is
exact for Serbian).

Usage: build_index.py <cpv_2008.xml> <cpv_sr.txt from `pdftotext -layout`> <out.tsv>
Output: one line per code, `code<TAB>en<TAB>sr`, sorted by code, UTF-8.
"""
import re
import sys
import xml.etree.ElementTree as ET

_CYR = "абвгдђежзијклљмнњопрстћуфхцчџшАБВГДЂЕЖЗИЈКЛЉМНЊОПРСТЋУФХЦЧЏШ"
_LAT = [
    "a", "b", "v", "g", "d", "đ", "e", "ž", "z", "i", "j", "k", "l", "lj", "m", "n", "nj",
    "o", "p", "r", "s", "t", "ć", "u", "f", "h", "c", "č", "dž", "š",
    "A", "B", "V", "G", "D", "Đ", "E", "Ž", "Z", "I", "J", "K", "L", "Lj", "M", "N", "Nj",
    "O", "P", "R", "S", "T", "Ć", "U", "F", "H", "C", "Č", "Dž", "Š",
]
CYR = dict(zip(_CYR, _LAT))


def translit(s):
    return "".join(CYR.get(c, c) for c in s)


def is_cyr(c):
    return "Ѐ" <= c <= "ӿ"


def cyr_part(line):
    """The Cyrillic column of one layout line: first to last Cyrillic letter, plus the
    brackets and punctuation that hug it."""
    idx = [i for i, c in enumerate(line) if is_cyr(c)]
    if not idx:
        return ""
    a, b = idx[0], idx[-1] + 1
    while a > 0 and line[a - 1] in "(\"„«":
        a -= 1
    while b < len(line) and line[b] in ")\"“»,.;:-–/":
        b += 1
    return line[a:b].strip()


def main():
    xml_path, txt_path, out_path = sys.argv[1:4]
    en = {}
    for cpv in ET.parse(xml_path).getroot().iter("CPV"):
        code = cpv.get("CODE")
        for t in cpv.iter("TEXT"):
            if t.get("LANG") == "EN":
                en[code] = " ".join((t.text or "").split())
    sr = {}
    cur = None
    code_re = re.compile(r"^\f?(\d{8}-\d)\s")
    with open(txt_path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.lstrip("\f").startswith("Code "):
                cur = None
                continue
            m = code_re.match(line)
            if m:
                cur = m.group(1)
                sr[cur] = [cyr_part(line)]
            elif cur and line.strip():
                part = cyr_part(line)
                if part:
                    sr[cur].append(part)
    missing = 0
    with open(out_path, "w", encoding="utf-8") as out:
        for code in sorted(en):
            parts = sr.get(code) or []
            label = " ".join(" ".join(p for p in parts if p).split())
            if not label:
                missing += 1
            out.write(f"{code}\t{en[code]}\t{translit(label)}\n")
    print(
        f"codes {len(en)}, serbian {len(en) - missing}, missing {missing}, "
        f"not in EU list {len(set(sr) - set(en))}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
