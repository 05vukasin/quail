//! Synthetic contracts built in code (no real documents) and a runner that puts them
//! through the steps the engine runs before `contract.facts` and `card.facts`.
#![allow(dead_code)]

use quail_core::{
    Analyzer, Artifacts, Blob, Block, BlockKind, Ctx, Document, Emitter, Fact, Format, Limits,
    Page, PageKind, Profile, Span, TextSource, Zone,
};

/// A Serbian (Latin) service contract with every fact the step reads.
pub const SR_LATIN: &str = "UGOVOR O ODRŽAVANJU SOFTVERA
Broj ugovora: 114/2026
Zaključen dana 15.01.2026. godine između:
1. Alfa Soft d.o.o., sa sedištem u Beogradu, ul. Knez Mihailova 10, PIB 100000024, MB 07000012, tekući račun 160-0000000123456-78, koga zastupa direktor Petar Petrović (u daljem tekstu: Izvođač)
i
2. Beta Trgovina d.o.o., sa sedištem u Novom Sadu, Bulevar oslobođenja 5, PIB 100000032, MB 07000020, koga zastupa generalni direktor Marko Marković (u daljem tekstu: Naručilac)
Član 1. Predmet ugovora
Predmet ovog ugovora je održavanje poslovnog softvera Naručioca.
Član 2. Cena i plaćanje
Ukupna vrednost ugovora iznosi 1.200.000,00 RSD bez PDV-a.
Naručilac se obavezuje da plati fakturu u roku od 15 dana od dana prijema fakture, virmanski na tekući račun Izvođača.
Naručilac plaća avans u iznosu od 30% ugovorene cene.
Cena se usklađuje jednom godišnje sa indeksom potrošačkih cena.
U slučaju kašnjenja Naručilac plaća zakonsku zateznu kamatu.
Član 3. Trajanje
Ugovor se zaključuje na period od 12 (dvanaest) meseci i automatski se produžava za isti period.
Svaka strana može otkazati ugovor uz otkazni rok od 30 dana.
Naručilac može raskinuti ugovor u slučaju kršenja ugovornih obaveza Izvođača.
Član 4. Obaveze
Izvođač je dužan da isporuči izveštaj u roku od 10 dana od potpisivanja.
Vreme odziva je 4 sata, a dostupnost sistema najmanje 99,5%.
Ugovorna kazna iznosi 0,5% za svaki dan kašnjenja, a najviše 10% ugovorene vrednosti.
Izvođač dostavlja bankarsku garanciju za dobro izvršenje posla u visini od 10% vrednosti ugovora.
Ukupna odgovornost Izvođača ograničena je na iznos ugovorene cene.
Strane se obavezuju da čuvaju poverljive informacije u periodu od 3 godine.
Garantni rok je 12 meseci.
Član 5. Završne odredbe
Na ovaj ugovor primenjuje se pravo Republike Srbije.
Za sve sporove nadležan je Privredni sud u Beogradu.
Ugovor je sačinjen na srpskom jeziku u 4 istovetna primerka.
Prilog 1: Tehnička specifikacija
Prilog 2: Cenovnik
Za Izvođača
Petar Petrović, direktor
Za Naručioca
Marko Marković
generalni direktor";

/// The same kind of contract in Serbian Cyrillic (a shorter one).
pub const SR_CYRILLIC: &str = "УГОВОР О ЗАКУПУ ПОСЛОВНОГ ПРОСТОРА
Уговор бр. 7/2026
1. Гама Некретнине д.о.о., са седиштем у Нишу, ПИБ 100000024, МБ 07000012, кога заступа директор Јован Јовановић (у даљем тексту: Закуподавац)
и
2. Делта Трговина д.о.о., ПИБ 100000032 (у даљем тексту: Закупац)
Члан 1. Трајање
Уговор се закључује на период од 2 године.
Уговор се аутоматски продужава за исти период.
Свака страна може отказати уговор уз отказни рок од 60 дана.
Члан 2. Плаћање
Закупац плаћа закупнину у року од 8 дана од дана пријема фактуре.
Уговорна казна износи 0,1% за сваки дан кашњења.
Закупац предаје бланко меницу као средство обезбеђења плаћања.
Члан 3. Спорови
На овај уговор примењује се право Републике Србије.
За све спорове надлежан је Привредни суд у Нишу.
Прилог 1: Скица простора";

/// An English agreement with every fact.
pub const EN: &str = "MASTER SERVICES AGREEMENT
Agreement No. MSA-2026-07
This Agreement is made on March 1, 2026 between Alpha Widgets LLC, with its registered office at 1 Main Street, Dover, Delaware (the \"Supplier\"), represented by John Smith, Chief Executive Officer, and Beta Components GmbH, Registration No. HRB 12345, VAT No. DE136695976 (the \"Customer\").
1. Term
This Agreement shall remain in force for a period of two (2) years from the Effective Date and shall automatically renew for successive one-year periods.
Either party may terminate this Agreement for convenience upon ninety (90) days' prior written notice.
Either party may terminate this Agreement if the other party commits a material breach or becomes insolvent.
2. Payment
The Customer shall pay each invoice within thirty (30) days of receipt of the invoice by bank transfer to IBAN DE89 3704 0044 0532 0130 00.
An advance payment of 20% of the Contract Price is due on signing.
Prices shall remain fixed for the Term.
3. Liability
The Supplier's total liability under this Agreement shall not exceed the fees paid in the twelve (12) months preceding the claim.
Liquidated damages of 1% per week of delay apply, up to a maximum of 10% of the Contract Price.
The Supplier shall provide a performance bond of 5% of the Contract Price.
4. Service Levels
The Supplier shall respond within 2 hours and resolve critical incidents within 8 hours. Availability shall be at least 99.9%.
5. Confidentiality
Each party shall keep the Confidential Information confidential for five (5) years after termination.
6. Governing Law
This Agreement is governed by the laws of England and Wales. The courts of London shall have exclusive jurisdiction over any dispute.
Any dispute shall be finally settled by arbitration under the ICC Rules, and the seat of arbitration shall be London.
This Agreement is executed in the English language, and the English version shall prevail.
Annex A - Pricing
Schedule 1: Service Description
For the Supplier
Name: John Smith
Title: Chief Executive Officer
For the Customer
Anna Weber, Managing Director";

/// A Serbian amendment (aneks).
pub const SR_AMENDMENT: &str = "ANEKS BR. 1 UZ UGOVOR BR. 114/2026
Ovim aneksom menja se član 2 Ugovora i glasi: cena iznosi 1.500.000,00 RSD bez PDV-a.
Briše se član 7 stav 2 Ugovora.
Ostale odredbe Ugovora ostaju nepromenjene.";

/// Text that carries the words of the cues but none of the facts.
pub const NEGATIVE: &str = "IZVEŠTAJ O RADU
Ovaj izveštaj opisuje sastanak o mogućoj saradnji i ne predstavlja ugovor.
Na sastanku se razgovaralo o tome da li će se cene menjati u budućnosti.
Rok za odgovor na anketu je bio dugačak, a učesnici su govorili o višoj sili u prirodi.
The meeting discussed liability in general terms and how courts work in other countries.
Prilogu ovog izveštaja nema.
The weather was fine and the agenda was short.";

/// A document of one page: one paragraph block per line.
pub fn doc(text: &str, ocr: bool) -> Document {
    let mut page = Page::new(0, PageKind::Logical);
    text.clone_into(&mut page.text);
    let mut off = 0u32;
    for line in text.split('\n') {
        let len = u32::try_from(line.len()).unwrap();
        if !line.trim().is_empty() {
            page.blocks.push(Block::new(
                Span {
                    start: off,
                    end: off + len,
                },
                BlockKind::Paragraph,
                Zone::Body,
                None,
            ));
        }
        off += len + 1;
    }
    page.text_source = if ocr {
        TextSource::Ocr
    } else {
        TextSource::Native
    };
    Document { pages: vec![page] }
}

/// Runs `steps` in order on `doc`, each seeing the facts of the ones before.
pub fn run(doc: Document, steps: &[&dyn Analyzer]) -> Vec<Fact> {
    let blob = Blob::from(Vec::new());
    let mut arts = Artifacts::new();
    arts.insert(doc);
    let limits = Limits::default();
    let mut facts: Vec<Fact> = Vec::new();
    for s in steps {
        let ctx = Ctx {
            blob: &blob,
            format: Format::Text,
            locator: "test",
            artifacts: &arts,
            facts: &facts,
            limits: &limits,
            profile: Profile::Fast,
        };
        let mut out = Emitter::new(s.id());
        s.run(&ctx, &mut out).unwrap();
        let new = out.facts().to_vec();
        facts.extend(new);
    }
    facts
}
