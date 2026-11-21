# cpv2008.tsv

The EU Common Procurement Vocabulary (CPV 2008), one line per code:
`code<TAB>English label<TAB>Serbian label (Latin script)`, 9 454 codes, about 620 kB.
Rebuild with `tools/cpv/fetch.sh <work-dir>`.

Sources:

- Codes and English labels: Commission Regulation (EC) No 213/2008 (CPV 2008), XML
  distribution from TED (`https://ted.europa.eu/documents/d/ted/cpv_2008_xml`).
  © European Union; reuse is authorised under Commission Decision 2011/833/EU provided the
  source is acknowledged.
- Serbian labels: "Opšti rečnik nabavke" (revised CPV translation, 2018) published by the
  Serbian Public Procurement Office (Kancelarija za javne nabavke) at
  `https://eupodrska.ujn.gov.rs/`, Cyrillic column transliterated to Latin. Official
  public-procurement material, not subject to copyright under Article 6 of the Serbian Law
  on Copyright and Related Rights. One code (30237380-6, CD-ROM) has no Serbian label.
