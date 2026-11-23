//! Finding company mentions, their roles, defined terms, name statements and signature
//! titles on one page. Every rule looks a bounded number of tokens around an anchor, so a
//! page is scanned in linear time.

use std::collections::HashSet;
use std::sync::LazyLock;

use quail_core::Zone;

use crate::forms::{self, Form, Position};
use crate::quality::{self, Quality};
use crate::roles::{self, LABELS, Role, role_of_word};
use crate::vocab::{self, has};
use crate::words::PageWords;

/// Most words walked over from a legal form into the name it qualifies.
const MAX_WALK_WORDS: usize = 8;
/// Most words of a name taken after a label or a name-statement marker.
const MAX_RUN_WORDS: usize = 8;
/// How far (tokens) after a name its defined term may follow.
const TERM_WINDOW: usize = 80;
/// Words before a name searched for a parties-clause opener.
const CLAUSE_BEFORE_WORDS: usize = 25;
/// Words after a name searched for registration details.
const CLAUSE_AFTER_WORDS: usize = 15;
/// First lines of the first page read as a letterhead when the extractor gives no zones.
const LETTERHEAD_LINES: u32 = 6;
/// Last lines of a page read as a running footer when the extractor gives no zones.
const FOOTER_LINES: u32 = 3;
/// Mentions kept per page.
pub const MAX_MENTIONS_PER_PAGE: usize = 200;

/// Where a mention was found, as far as confidence is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent observations
pub struct Context {
    /// In a parties clause (opener before, registration details after).
    pub clause: bool,
    /// Letterhead or running header.
    pub header: bool,
    /// Running footer.
    pub footer: bool,
    /// Signature block.
    pub signature: bool,
    /// After a role label ("Dobavljač:", "Bill to:").
    pub label: bool,
    /// In a title or heading.
    pub title: bool,
    /// A bank named in payment details ("Bank: X", "X Bank plc").
    pub bank: bool,
    /// A formless name that an e-mail or web domain of the document spells.
    pub domain: bool,
    /// The EDGAR filer named on a `Source: X, 10-K, ...` line (a page header or footer).
    pub filer: bool,
    /// After `with a copy to` in a notices clause (usually counsel).
    pub copy_to: bool,
    /// Named as an adviser: under an `Auditors` / `Bankers` / `Solicitors` label, or followed
    /// by `Chartered Accountants`, `Statutory Auditor`.
    pub adviser: bool,
}

/// One company mention on a page.
#[derive(Clone, Debug)]
pub struct Mention {
    /// First token.
    pub a: usize,
    /// One past the last token.
    pub b: usize,
    /// Its legal form, when it has one.
    pub form: Option<&'static Form>,
    /// Country of the seat city written after the form.
    pub seat: Option<&'static str>,
    /// One past the last token of the name proper (before the seat city).
    pub name_end: usize,
    /// Roles stated for it, in order, without repeats.
    pub roles: Vec<Role>,
    /// Defined term and the role it states.
    pub term: Option<(String, Option<Role>)>,
    /// Where it was found.
    pub ctx: Context,
    /// Taken from the other side of a name statement.
    pub from_statement: bool,
    /// Zone for the evidence.
    pub zone: Zone,
}

impl Mention {
    fn add_role(&mut self, r: Role) {
        if !self.roles.contains(&r) {
            self.roles.push(r);
        }
    }
}

/// A name statement: `X (formerly Y)`, `X d/b/a Y`, `X trading as Y`, `X (prethodno: Y)`.
#[derive(Clone, Debug)]
pub struct Statement {
    /// Mention index of the current or legal name.
    pub current: usize,
    /// Mention index of the former or trading name.
    pub other: usize,
    /// `former_name`, `trading_name` or `dba`.
    pub kind: &'static str,
    /// Tokens covering both names.
    pub a: usize,
    /// One past the last token.
    pub b: usize,
}

/// A signature title ("Direktor", "Managing Director") without the person.
#[derive(Clone, Debug)]
pub struct Title {
    /// Title as written.
    pub text: String,
    /// Tokens.
    pub a: usize,
    /// One past the last token.
    pub b: usize,
}

/// Everything found on one page.
#[derive(Default)]
pub struct PageScan {
    /// Mentions in text order.
    pub mentions: Vec<Mention>,
    /// Name statements.
    pub statements: Vec<Statement>,
    /// Signature titles.
    pub titles: Vec<Title>,
    /// Lines of the signature block holding a title or a name/by label.
    pub signatory_lines: usize,
    /// Mentions dropped at [`MAX_MENTIONS_PER_PAGE`].
    pub dropped: usize,
}

/// Where the page sits in the document.
#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent observations
pub struct PagePos {
    /// First page.
    pub first: bool,
    /// One of the last two pages (signature blocks live there).
    pub near_end: bool,
    /// The extractor gave zones.
    pub has_zones: bool,
    /// The document defines `"Company"` as a term, so `X and Company` names two parties.
    pub company_term: bool,
    /// The page opens the agreement (the first one, or the first full page after a short cover
    /// or exhibit page): a parties clause may stand here.
    pub opening: bool,
}

/// Scans one page. `domains` are the second-level labels of the document's e-mail and web
/// domains (see [`domain_labels`]): a formless name spelling one is a brand candidate.
pub fn scan(w: &PageWords, pos: PagePos, domains: &[String]) -> PageScan {
    let s = Scanner::new(w, pos, domains);
    s.run()
}

struct Scanner<'a> {
    w: &'a PageWords<'a>,
    pos: PagePos,
    /// Token index of every word, in order.
    words: Vec<usize>,
    /// Word position of every token (the next word's position for punctuation).
    wpos: Vec<usize>,
    /// Bare keys of all tokens, concatenated.
    bare_buf: String,
    /// Range of each token's bare key in `bare_buf`.
    bare_rng: Vec<(usize, usize)>,
    /// Domain labels of the document.
    domains: &'a [String],
}

const BREAKING: &[char] = &[
    ',', ';', ':', '(', ')', '[', ']', '|', '/', '!', '?', '*', '•', '<', '>', '=',
];

impl<'a> Scanner<'a> {
    fn new(w: &'a PageWords<'a>, pos: PagePos, domains: &'a [String]) -> Self {
        let mut words = Vec::new();
        let mut wpos = Vec::with_capacity(w.len());
        let mut bare_buf = String::new();
        let mut bare_rng = Vec::with_capacity(w.len());
        for i in 0..w.len() {
            wpos.push(words.len());
            if w.is_word(i) {
                words.push(i);
            }
            let s = bare_buf.len();
            bare_buf.extend(w.key(i).chars().filter(|c| *c != '.' && *c != '/'));
            bare_rng.push((s, bare_buf.len()));
        }
        Self {
            w,
            pos,
            words,
            wpos,
            bare_buf,
            bare_rng,
            domains,
        }
    }

    fn run(self) -> PageScan {
        let mut out = PageScan::default();
        self.find_formed(&mut out);
        self.find_labels(&mut out);
        out.mentions.sort_by_key(|m| m.a);
        self.find_brands(&mut out);
        out.mentions.sort_by_key(|m| m.a);
        // Sorts again and remaps its statement indices when it adds mentions.
        self.find_statements(&mut out);
        let sig_line = self.signature_start();
        for i in 0..out.mentions.len() {
            let next = out.mentions.get(i + 1).map_or(self.w.len(), |m| m.a);
            self.contextualize(&mut out.mentions[i], next, sig_line);
        }
        self.find_titles(&mut out, sig_line);
        out
    }

    // ----- token predicates -------------------------------------------------------------

    fn key(&self, i: usize) -> &str {
        self.w.key(i)
    }

    /// Bare key: folded, dots and slashes removed.
    fn bare(&self, i: usize) -> &str {
        self.bare_rng
            .get(i)
            .map_or("", |&(s, e)| self.bare_buf.get(s..e).unwrap_or(""))
    }

    fn is_stop(&self, i: usize) -> bool {
        let k = self.key(i);
        has(vocab::WALK_STOP_WORDS, k)
            || has(vocab::CALENDAR_WORDS, k)
            || has(EXTRA_STOP, k)
            || role_of_word(k).is_some()
            || has(vocab::ROLE_TERM_WORDS, k) && !has(NAME_OK_ROLE_WORDS, k)
    }

    /// Could be a word of a company name: capitalised, a digit-led code, camel case.
    fn namey(&self, i: usize) -> bool {
        if !self.w.is_word(i) {
            return false;
        }
        let t = self.w.text(i);
        let mut chars = t.chars();
        let Some(c0) = chars.next() else {
            return false;
        };
        // Codes and registration numbers are not names ("D1532C00057", "100200300").
        if t.chars().filter(char::is_ascii_digit).count() >= 4 {
            return false;
        }
        if c0.is_uppercase() || c0.is_ascii_digit() {
            return true;
        }
        // "eResearch", "iGo".
        c0.is_lowercase() && t.chars().skip(1).any(char::is_uppercase)
    }

    /// Does the line ending at token `k` run on into the next one: a line of running text
    /// (eight tokens or more) that ends in a name word, not in punctuation?
    fn wraps_into(&self, k: usize) -> bool {
        if !self.w.is_word(k) || !self.namey(k) {
            return false;
        }
        let line = self.w.toks[k].line;
        let mut n = 0;
        let mut a = k;
        while a > 0 && self.w.toks[a - 1].line == line && n < 8 {
            a -= 1;
            n += 1;
        }
        n >= 7
    }

    /// A word of four letters or more, all of them capitals ("NIKE", "COMMUNICATIONS").
    fn all_caps(&self, i: usize) -> bool {
        let t = self.w.text(i);
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        letters >= 4
            && t.chars()
                .filter(|c| c.is_alphabetic())
                .all(char::is_uppercase)
    }

    fn connector(&self, i: usize) -> bool {
        self.w.is_word(i) && has(NAME_CONNECTORS, self.key(i))
    }

    fn same_line(&self, a: usize, b: usize) -> bool {
        a < self.w.len() && b < self.w.len() && self.w.toks[a].line == self.w.toks[b].line
    }

    /// Words `seq` start at word position `p`.
    fn seq_at(&self, p: usize, seq: &[&str]) -> bool {
        seq.iter()
            .enumerate()
            .all(|(k, s)| self.words.get(p + k).is_some_and(|&t| self.key(t) == *s))
    }

    fn attached(&self, i: usize) -> bool {
        self.w.toks.get(i).is_some_and(|t| !t.space_before)
    }

    // ----- legal forms ------------------------------------------------------------------

    /// A legal form starting at token `i`: the form and one past its last token.
    fn form_at(&self, i: usize) -> Option<(&'static Form, usize)> {
        if !self.w.is_word(i) {
            return None;
        }
        if let Some(hit) = self.spelled_prefix_form(i) {
            return Some(hit);
        }
        // A form stands as a word of its own, never glued to a code ("AV-W-AD").
        if i > 0 && self.attached(i) && !self.w.is_word(i - 1) && !self.w.is_punct(i - 1, '"') {
            let p = self.w.text(i - 1);
            if p.starts_with(['-', '_', '/', '.']) {
                return None;
            }
        }
        let first = self.bare(i);
        let short = first.len() <= 3;
        let mut best: Option<(&'static Form, usize, bool)> =
            forms::by_bare(first).map(|f| (f, i + 1, false));
        if best.is_none() && !short {
            return None;
        }
        let mut combined = String::from(first);
        let mut dotted = self.key(i).contains('.') || self.key(i).contains('/');
        let mut slashed = false;
        // Pieces of a dotted or slashed form: "d. o. o.", "A/S", "Sp. z o.o.".
        let mut j = i;
        let mut pieces = 1;
        while pieces < 4 && combined.len() <= 6 && self.bare(j).len() <= 3 {
            let mut k = j + 1;
            if self.w.is_punct(k, '.') {
                k += 1;
            } else if self.w.is_punct(k, '/') {
                slashed = true;
                k += 1;
            } else if !dotted {
                break;
            }
            if !self.w.is_word(k) || !self.same_line(i, k) || self.bare(k).len() > 3 {
                break;
            }
            dotted = true;
            combined.push_str(self.bare(k));
            pieces += 1;
            j = k;
            if let Some(f) = forms::by_bare(&combined) {
                best = Some((f, k + 1, slashed));
            }
        }
        let (form, end, slashed) = best?;
        // Only the Nordic A/S is written with a slash ("A/B" is a column heading).
        if slashed && form.bare != "as" {
            return None;
        }
        let surface = self.w.slice(i, end);
        let dotted = dotted || self.w.is_punct(end, '.') && self.attached(end);
        if !forms::gate_ok(form.gate, surface, dotted) {
            return None;
        }
        // A form written in Cyrillic is one of a Cyrillic-writing country: "СА" is the
        // Serbian preposition "sa", never a société anonyme.
        if surface
            .chars()
            .any(|c| ('\u{0400}'..='\u{04ff}').contains(&c))
            && !forms::cyrillic_ok(form)
        {
            return None;
        }
        if form.bare == "co" && !self.co_closes(end) {
            return None;
        }
        Some((form, end))
    }

    /// "Co" is a form only as "Co." / "Co," / "Co Ltd" / at a line end.
    fn co_closes(&self, end: usize) -> bool {
        self.w.is_punct(end, '.')
            || self.w.is_punct(end, ',')
            || self.w.breaks_before(end)
            || self.form_word(end)
    }

    fn form_word(&self, i: usize) -> bool {
        self.w.is_word(i) && forms::by_bare(self.bare(i)).is_some_and(|f| !f.weak)
    }

    /// "Javno preduzeće X", "Javno komunalno preduzeće X", "Akcionarsko društvo X".
    fn spelled_prefix_form(&self, i: usize) -> Option<(&'static Form, usize)> {
        let (k0, k1, k2) = (self.key(i), self.key(i + 1), self.key(i + 2));
        let hit = match (k0, k1, k2) {
            ("javno", "preduzece", _) => Some(("jp", 2)),
            ("javno", "komunalno", "preduzece") => Some(("jkp", 3)),
            ("akcionarsko", "drustvo", _) => Some(("ad", 2)),
            _ => None,
        }?;
        if !self.w.text(i).starts_with(char::is_uppercase) {
            return None;
        }
        let base = forms::by_bare(hit.0)?;
        let form = match hit.0 {
            "ad" => &AD_PREFIX,
            _ => base,
        };
        Some((form, i + hit.1))
    }

    // ----- formed mentions --------------------------------------------------------------

    fn find_formed(&self, out: &mut PageScan) {
        let mut i = 0;
        let mut floor = 0;
        while i < self.w.len() {
            let Some((form, end)) = self.form_at(i) else {
                i += 1;
                continue;
            };
            let m = match form.position {
                Position::Suffix => self.suffix_mention(form, i, end, floor),
                Position::Prefix => self.prefix_mention(form, i, end),
                Position::Infix => self.infix_mention(form, i, end, floor),
            };
            match m {
                Some(m) => {
                    i = m.b.max(i + 1);
                    floor = m.b;
                    push_mention(out, m);
                }
                None => i = end,
            }
        }
    }

    fn suffix_mention(
        &self,
        form: &'static Form,
        i: usize,
        end: usize,
        floor: usize,
    ) -> Option<Mention> {
        // "Company" right after a comma is the defined term ("Article 20, Company").
        if form.bare == "company" && i > 0 && self.w.is_punct(i - 1, ',') {
            return None;
        }
        let a = self.walk_left(i, floor, form)?;
        // ... and it needs a real word before it ("C. Company" is an initial; "3M Company"
        // has one).
        if form.bare == "company"
            && !(a..i).any(|k| {
                self.w.is_word(k) && {
                    let t = self.w.text(k);
                    let letters = t.chars().filter(|c| c.is_alphabetic()).count();
                    letters >= 3
                        || letters >= 1
                            && t.chars().count() >= 2
                            && t.chars().any(|c| c.is_ascii_digit())
                }
            })
        {
            return None;
        }
        // A weak form after a number is a clause reference, not a name ("Article 20 AS").
        if form.weak && (a..i).any(|k| self.number(k)) {
            return None;
        }
        // "Verizon and Company" where the document defines "Company": two parties.
        if form.bare == "company" && self.pos.company_term && i > 0 && self.key(i - 1) == "and" {
            return None;
        }
        // "Licensee's Company" is a possessive, "Brian J. Cirbo Company" a signatory over the
        // defined term.
        let possessive = i >= 2 && self.w.is_punct(i - 2, '\'') && self.key(i - 1) == "s"
            || i >= 1 && {
                let t = self.w.text(i - 1);
                t.ends_with("'s") || t.ends_with("\u{2019}s")
            };
        if form.weak && (possessive || self.initialled_person(a, i)) {
            return None;
        }
        // Column or list letters: "V W X Y Z AA AB", "AC AD", "AE AF AG".
        if form.weak {
            let words: Vec<usize> = (a..i).filter(|&k| self.w.is_word(k)).collect();
            let letters = |k: &usize| {
                let t = self.w.text(*k);
                t.chars().count() <= 2 && t.chars().all(char::is_uppercase)
            };
            if words.len() >= 2 && words.iter().all(letters) {
                return None;
            }
        }
        // A US state code after a city, not a form: "Springfield, NV 89000".
        if self.state_code(i, end) {
            return None;
        }
        // Column headings: "Len Group", "Qty Co.".
        if (a..i)
            .filter(|&k| self.w.is_word(k))
            .all(|k| has(HEADER_ABBREVIATIONS, self.key(k)))
        {
            return None;
        }
        // A weak form in a heading or a phrase is a common noun: "Return of Company",
        // "HOLDING CO.", "Parent Company".
        if form.weak {
            let words: Vec<usize> = (a..i).filter(|&k| self.w.is_word(k)).collect();
            let of_before = words.last().is_some_and(|&k| self.key(k) == "of");
            let generic = words.iter().all(|&k| has(WEAK_FORM_NOUNS, self.key(k)));
            if of_before || generic {
                return None;
            }
        }
        let (b, seat) = self.close_form(end);
        self.accept(a, b, Some(form), seat)
    }

    /// Tokens `i..end` are a two-letter state code in capitals after a comma, followed on its
    /// line by a ZIP code: `Springfield, NV 89000`, `Denver, CO 80202`.
    fn state_code(&self, i: usize, end: usize) -> bool {
        let t = self.w.text(i);
        let zip = |k: usize| {
            let z = self.w.text(k);
            z.len() == 5 && z.bytes().all(|b| b.is_ascii_digit())
        };
        end == i + 1
            && t.len() == 2
            && t.bytes().all(|b| b.is_ascii_uppercase())
            && has(US_STATES, self.key(i))
            && i > 0
            && self.w.is_punct(i - 1, ',')
            && self.same_line(i, end)
            && zip(end)
    }

    fn prefix_mention(&self, form: &'static Form, i: usize, end: usize) -> Option<Mention> {
        let b = self.walk_right(end)?;
        let (b, seat) = self.close_form(b);
        self.accept(i, b, Some(form), seat)
    }

    fn infix_mention(
        &self,
        form: &'static Form,
        i: usize,
        end: usize,
        floor: usize,
    ) -> Option<Mention> {
        let a = self.walk_left(i, floor, form)?;
        // A sole trader is a person: two or three alphabetic words before the form.
        let person: Vec<usize> = (a..i).filter(|&k| self.w.is_word(k)).collect();
        let alphabetic = |k: &usize| self.w.text(*k).chars().all(char::is_alphabetic);
        if !(2..=3).contains(&person.len()) || !person.iter().all(alphabetic) {
            return None;
        }
        // The shop's name usually follows; it may also stop here.
        let b = self.walk_right(end).unwrap_or(end);
        if (end..b).any(|k| self.w.text(k).chars().any(|c| c.is_ascii_digit())) {
            return None;
        }
        let (b, seat) = self.close_form(b);
        self.accept(a, b, Some(form), seat)
    }

    /// Absorbs what closes a form: its dot, further forms (`Co., Ltd.`, `GmbH & Co. KG`) and a
    /// seat city ("d.o.o. Beograd").
    fn close_form(&self, mut b: usize) -> (usize, Option<(&'static str, usize)>) {
        for _ in 0..3 {
            if self.w.is_punct(b, '.') && self.attached(b) {
                b += 1;
            }
            let mut k = b;
            if (self.w.is_punct(k, ',') || self.w.is_punct(k, '&'))
                && self.same_line(b.saturating_sub(1), k)
            {
                k += 1;
            }
            if !self.same_line(b.saturating_sub(1), k) {
                break;
            }
            match self.form_at(k) {
                Some((f, e)) if f.position == Position::Suffix => b = e,
                _ => break,
            }
        }
        let mut seat = None;
        let mut k = b;
        if self.w.is_punct(k, ',') {
            k += 1;
        }
        let city = (self.same_line(b.saturating_sub(1), k) && self.namey(k))
            .then(|| vocab::lookup(vocab::SEAT_CITIES, self.key(k)))
            .flatten();
        if let Some(c) = city {
            let two = matches!(
                (self.key(k), self.key(k + 1)),
                ("novi", "sad" | "beograd") | ("banja", "luka")
            );
            if two || self.key(k) != "novi" {
                seat = Some((c, b));
                b = if two { k + 2 } else { k + 1 };
            }
        }
        (b, seat)
    }

    /// First token of the name ending right before `form` at `i`, or `None`.
    fn walk_left(&self, i: usize, floor: usize, form: &Form) -> Option<usize> {
        let mut j = i;
        // The form takes a registration number as the name's first word.
        let numbered = NUMBERED_FORMS.contains(&form.bare);
        // A strong form that starts its line takes the name at the end of the line above
        // ("Sibannac," / "Inc.", "Purinix Pharmaceuticals" / "LLC").
        let wrapped_form = !form.weak
            && i > floor + 1
            && self.w.breaks_before(i)
            && (self.namey(i - 1) || self.w.is_punct(i - 1, ',') && self.namey(i - 2));
        // "Acme, Inc.": one comma between the name and its form.
        if j > floor + 1
            && self.w.is_punct(j - 1, ',')
            && (self.same_line(j - 2, j) || wrapped_form)
        {
            j -= 1;
        }
        let mut words = 0;
        let mut comma = false;
        // The leftmost name word so far is set in capitals ("NIKE"): a Titlecase word before
        // it is another name, usually the signatory's ("Mark G. Parker NIKE, Inc.").
        let mut caps_run = false;
        // The line the name may still take words from: the form's, or once the line above
        // when the name starts its line inside a wrapped paragraph ("between Vertex" /
        // "Pharmaceuticals Incorporated").
        let mut anchor = i;
        while j > floor {
            let k = j - 1;
            if !self.same_line(k, anchor) {
                let joins = if words == 0 {
                    wrapped_form
                } else {
                    self.wraps_into(k) || self.joins_lines(k)
                };
                if anchor != i || !joins {
                    break;
                }
                anchor = k;
            } else if self.w.toks.get(k + 1).is_some_and(|t| t.column_gap) {
                // Two columns of a signature block: "Snap Technologies      United Airlines, Inc."
                break;
            }
            if self.w.is_word(k) {
                if self.is_stop(k) && !self.hyphened(k) && !self.connector(k)
                    || has(EXTRA_STOP, self.bare(k))
                    || has(&["auditor", "auditors"], self.key(k))
                {
                    // "At Home Corporation", "ON Semiconductor": a capitalised preposition
                    // mid-sentence opens the name.
                    if words > 0 && self.opens_name(k) {
                        j = k;
                    }
                    break;
                }
                if self.form_word(k) {
                    break;
                }
                if !(self.namey(k) || self.connector(k)) {
                    // "2205925 Ontario Limited": a registration number opens the name.
                    if words > 0 && numbered && self.number_opens_name(k) {
                        j = k;
                    }
                    break;
                }
                let lower = self.w.text(k).chars().any(char::is_lowercase);
                // "ChipMOS", "LiquidTOOL": a brand mixing cases reads as capitals.
                let mixed = lower && self.mixed_caps(k);
                if caps_run && lower && !mixed && !self.connector(k) {
                    break;
                }
                // A connector says nothing about the case style ("Yu Zhou and GENEXOSOME").
                if !self.connector(k) {
                    if lower && !mixed {
                        caps_run = false;
                    } else if self.all_caps(k) || mixed {
                        caps_run = true;
                    }
                }
                words += 1;
                if words > MAX_WALK_WORDS {
                    break;
                }
                // "Autoliv ASP and Autoliv AB": the same first word on both sides of `and`
                // starts two names.
                if self.connector(k) && self.key(k) == "and" && self.repeats_across_and(k, j) {
                    break;
                }
            } else if self.w.is_punct(k, ')') {
                // "GALEN (CHEMICALS) LIMITED", "Dialog Semiconductor (UK) Ltd.".
                match self.paren_in_name(k, floor) {
                    Some(open) => {
                        words += 1;
                        j = open;
                        continue;
                    }
                    None => break,
                }
            } else if self.w.is_punct(k, ',') {
                // One comma inside a name: "Brenwick, Dalton and Company".
                let name_before = k > floor
                    && self.namey(k - 1)
                    && !self.is_stop(k - 1)
                    && self.same_line(k - 1, i);
                if comma || !name_before || j >= i || !self.namey(j) {
                    break;
                }
                comma = true;
            } else if !self.inner_punct(k) || self.closing_quote(k) {
                break;
            }
            j = k;
        }
        self.trim(j, i).map(|(a, _)| a)
    }

    /// Word `k` is inside a hyphenated compound, where a stop word is part of the name
    /// ("Build-A-Bear", "AT&T", "Zeta-Mart": `Mart` is March in Serbian).
    fn hyphened(&self, k: usize) -> bool {
        let joint = |t: usize| self.w.is_punct(t, '-') || self.w.is_punct(t, '&');
        joint(k + 1) && self.attached(k + 1) && self.attached(k + 2)
            || k >= 2
                && joint(k - 1)
                && self.attached(k)
                && self.attached(k - 1)
                && self.w.is_word(k - 2)
    }

    /// An apostrophe at `k` closing a quote, not an elision or a plural possessive:
    /// `POLICY' Acme, Inc.` (`O'Neil` is one word, `Florists' Exchange` one name).
    fn closing_quote(&self, k: usize) -> bool {
        self.w.is_punct(k, '\'')
            && !self.attached(k + 1)
            && !self.w.text(k.saturating_sub(1)).ends_with(['s', 'S'])
    }

    /// Crossing from the line of token `k + 1` up to `k` keeps one name: `Leggett &` /
    /// `Platt, Incorporated`, `NORTH AMERICAN GALVANIZING` / `& COATING, INC.`.
    fn joins_lines(&self, k: usize) -> bool {
        let amp = |t: usize| self.w.is_punct(t, '&') || self.key(t) == "and";
        if amp(k) {
            return k > 0 && self.same_line(k - 1, k) && self.namey(k - 1);
        }
        if !self.w.is_word(k) || !self.namey(k) || self.is_stop(k) {
            return false;
        }
        if amp(k + 1) {
            return true;
        }
        // "between Aspect Medical" / "Systems, Inc.": the name's start follows an opener.
        let mut a = k;
        while a > 0
            && self.same_line(a - 1, k)
            && self.w.is_word(a - 1)
            && self.namey(a - 1)
            && !self.is_stop(a - 1)
            && k - a < 4
        {
            a -= 1;
        }
        a > 0
            && self.same_line(a - 1, k)
            && has(
                &["between", "and", "among", "by", "with"],
                self.w.text(a - 1),
            )
    }

    /// A capitalised preposition that opens a name mid-sentence: `by At Home Corporation`,
    /// `and ON Semiconductor Corporation`.
    fn opens_name(&self, k: usize) -> bool {
        self.w.is_word(k)
            && has(NAME_OPENING_WORDS, self.key(k))
            && self.w.text(k).starts_with(char::is_uppercase)
            && k > 0
            && self.w.is_word(k - 1)
            && self.w.text(k - 1).starts_with(char::is_lowercase)
    }

    /// A registration number that opens a name: six to ten digits before a capitalised word
    /// on the same line (`2205925 Ontario Limited`).
    fn number_opens_name(&self, k: usize) -> bool {
        let t = self.w.text(k);
        self.w.is_word(k)
            && (6..=10).contains(&t.len())
            && t.bytes().all(|b| b.is_ascii_digit())
            && self.w.is_word(k + 1)
            && self.same_line(k, k + 1)
            && self.w.text(k + 1).starts_with(char::is_uppercase)
            && self.w.text(k + 1).chars().all(char::is_alphabetic)
    }

    /// Mixed case with a run of capitals: `ChipMOS`, `LiquidTOOL` (not `McDonald`).
    fn mixed_caps(&self, k: usize) -> bool {
        let mut run = 0;
        let mut best = 0;
        for c in self.w.text(k).chars() {
            if c.is_uppercase() {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        best >= 2
    }

    /// The `(` of a parenthesised name part closing at `k`: one to three name words inside,
    /// a name word right before it, all on one line (`GALEN (CHEMICALS) LIMITED`,
    /// `Bunker One (USA) Inc.`, `SHBV (HONG KONG) LTD`).
    fn paren_in_name(&self, k: usize, floor: usize) -> Option<usize> {
        let mut open = k;
        while open > floor && k - open <= 4 {
            open -= 1;
            if self.w.is_punct(open, '(') {
                break;
            }
            if !self.w.is_word(open) || !self.namey(open) || self.is_stop(open) {
                return None;
            }
        }
        if !self.w.is_punct(open, '(') || open + 1 >= k || open <= floor {
            return None;
        }
        let before = open - 1;
        (self.same_line(before, k)
            && self.w.is_word(before)
            && self.namey(before)
            && !self.is_stop(before))
        .then_some(open)
    }

    /// `k` is `and`, `j` the first word after it: the name's first word before `and` is that
    /// word again (`Autoliv ASP and Autoliv AB`, `Adaptimmune and Adaptimmune Limited`).
    fn repeats_across_and(&self, k: usize, j: usize) -> bool {
        let mut a = k;
        while a > 0
            && self.same_line(a - 1, k)
            && self.w.is_word(a - 1)
            && self.namey(a - 1)
            && !self.is_stop(a - 1)
        {
            a -= 1;
        }
        a < k && self.key(a) == self.key(j)
    }

    /// One past the last token of the name starting at `start`, or `None`.
    fn walk_right(&self, start: usize) -> Option<usize> {
        let mut j = start;
        let mut words = 0;
        while j < self.w.len() {
            if j > start && self.w.breaks_before(j) || !self.same_line(start, j) {
                break;
            }
            if self.w.is_word(j) {
                if self.is_stop(j) {
                    break;
                }
                if let Some((f, e)) = self.form_at(j)
                    && f.position == Position::Suffix
                {
                    j = e;
                    break;
                }
                if !(self.namey(j) || self.connector(j)) {
                    break;
                }
                words += 1;
                if words > MAX_WALK_WORDS {
                    break;
                }
            } else if !self.inner_punct(j) {
                break;
            }
            j += 1;
        }
        self.trim(start, j).map(|(_, b)| b)
    }

    /// Punctuation a name may hold: `&`, `-`, `'`, `"`, `+`, `.` after an initial.
    fn inner_punct(&self, k: usize) -> bool {
        let t = self.w.text(k);
        match t.chars().next() {
            Some('&' | '-' | '\'' | '"' | '+') => true,
            // After an initial or a dotted abbreviation: "J. Smith", "L.E.K. Consulting".
            Some('.') => {
                k > 0 && self.w.is_word(k - 1) && {
                    let prev = self.w.text(k - 1);
                    prev.chars().count() <= 2 || prev.contains('.')
                }
            }
            _ => false,
        }
    }

    /// Trims punctuation, connectors and leading numbers off `a..b`; `None` when no word of a
    /// name is left.
    fn trim(&self, mut a: usize, mut b: usize) -> Option<(usize, usize)> {
        // A leading connector goes only when written in lowercase: "I" opens "I-ESCROW".
        // ... or as a word of its own in capitals ("JOINDER OF X, INC.").
        let lower_connector = |k: usize| {
            self.connector(k)
                && (self.w.text(k).starts_with(char::is_lowercase)
                    || self.w.text(k).chars().count() > 1 && !self.attached(k + 1))
        };
        while a < b
            && (!self.w.is_word(a) && !self.w.is_punct(a, '"')
                || lower_connector(a)
                || self.number(a) && !self.number_opens_name(a))
        {
            a += 1;
        }
        while b > a
            && (!self.w.is_word(b - 1)
                && !self.w.is_punct(b - 1, '"')
                && !self.w.is_punct(b - 1, '.')
                || self.connector(b - 1))
        {
            b -= 1;
        }
        (a..b)
            .any(|k| self.w.is_word(k) && !self.number(k))
            .then_some((a, b))
    }

    fn number(&self, i: usize) -> bool {
        self.w.is_word(i)
            && self
                .w
                .text(i)
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    }

    /// Builds the mention `a..b` when its text passes the name gates.
    fn accept(
        &self,
        a: usize,
        b: usize,
        form: Option<&'static Form>,
        seat: Option<(&'static str, usize)>,
    ) -> Option<Mention> {
        let joined = self.w.slice(a, b).replace('\n', " ");
        let surface = joined.as_str();
        let q = quality::verdict(surface).ok()?;
        // A formless name must not have a person's shape in any letter case.
        if form.is_none() && (q != Quality::Ok || quality::is_person_shaped(surface)) {
            return None;
        }
        Some(Mention {
            a,
            b,
            form,
            seat: seat.map(|s| s.0),
            name_end: seat.map_or(b, |s| s.1),
            roles: Vec::new(),
            term: None,
            ctx: Context::default(),
            from_statement: false,
            zone: self.w.toks[a].zone,
        })
    }

    // ----- labels -----------------------------------------------------------------------

    /// A role label starting at token `i`: its role and one past its last token.
    fn label_at(&self, i: usize) -> Option<(Role, usize)> {
        if !self.w.is_word(i) || !self.w.text(i).starts_with(char::is_uppercase) {
            return None;
        }
        let p = self.wpos[i];
        for (seq, role) in LABELS {
            if self.seq_at(p, seq) {
                return Some((*role, self.words[p + seq.len() - 1] + 1));
            }
        }
        let r = role_of_word(self.key(i))?;
        Some((r, i + 1))
    }

    fn find_labels(&self, out: &mut PageScan) {
        let mut i = 0;
        while i < self.w.len() {
            let Some((role, end)) = self.label_at(i) else {
                i += 1;
                continue;
            };
            let colon =
                self.w.is_punct(end, ':') || self.w.is_punct(end, '-') && self.same_line(i, end);
            let line_start = self.w.breaks_before(i);
            let ends_line = self.w.breaks_before(end);
            if !(colon || line_start && ends_line)
                || !self.w.text(i).starts_with(char::is_uppercase)
            {
                i = end;
                continue;
            }
            let v = if colon { end + 1 } else { end };
            // "Odgovorno lice naručioca:", "Contact person of the Supplier:": the value is a
            // person or a title, not the company.
            if self.person_before(i) {
                i = end;
                continue;
            }
            // Side-by-side columns ("Remit To: Ship To:" over two names) say nothing.
            if self.w.breaks_before(v) && self.labels_on_line(i) > 1 {
                i = end;
                continue;
            }
            let serbian = roles::is_serbian_role_word(self.key(i));
            self.label_value(out, role, v, serbian);
            i = v.max(i + 1);
        }
    }

    /// A person noun right before the label at `i`, on its line, `of` / `the` between:
    /// `odgovorno lice naručioca`, `contact person of the Supplier`.
    fn person_before(&self, i: usize) -> bool {
        let mut p = self.wpos[i];
        while p > 0 {
            p -= 1;
            let t = self.words[p];
            if !self.same_line(t, i) {
                return false;
            }
            let k = self.key(t);
            if has(&["of", "the"], k) {
                continue;
            }
            return has(PERSON_NOUNS, k);
        }
        false
    }

    fn labels_on_line(&self, i: usize) -> usize {
        let line = self.w.toks[i].line;
        let mut n = 0;
        let mut k = i;
        while k > 0 && self.w.toks[k - 1].line == line {
            k -= 1;
        }
        while k < self.w.len() && self.w.toks[k].line == line {
            match self.label_at(k) {
                Some((_, e)) => {
                    n += 1;
                    k = e;
                }
                None => k += 1,
            }
        }
        n
    }

    /// The name after a label: an existing formed mention, or a formless name on that line.
    fn label_value(&self, out: &mut PageScan, role: Role, v: usize, serbian: bool) {
        let mut v = v;
        while self.w.is_punct(v, '"') || self.w.is_punct(v, '-') {
            v += 1;
        }
        if let Some(m) = out.mentions.iter_mut().find(|m| m.a >= v && m.a <= v + 2) {
            m.add_role(role);
            m.ctx.label = true;
            return;
        }
        // "НАРУЧИЛАЦ:" over "Републички фонд за здравствено осигурање, ул. …": a Serbian
        // institution's name is written in sentence case.
        if serbian
            && let Some((a, b)) = self.sentence_case_value(v)
            && let Some(mut m) = self.accept(a, b, None, None)
        {
            m.add_role(role);
            m.ctx.label = true;
            push_mention(out, m);
            return;
        }
        let mut j = v;
        let mut words = 0;
        while j < self.w.len() && (j == v || !self.w.breaks_before(j)) {
            if self.w.is_word(j) {
                // A field label is no name ("Client" over "Submit EDI Invoice"): it ends the
                // run, and the whole-line test below drops what is left.
                if self.is_stop(j) && j == v
                    || !(self.namey(j) || self.connector(j))
                    || has(FIELD_WORDS, self.key(j))
                {
                    break;
                }
                words += 1;
                if words > MAX_RUN_WORDS {
                    return;
                }
            } else if !self.inner_punct(j) || self.w.text(j).starts_with(BREAKING) {
                break;
            }
            j += 1;
        }
        let Some((a, b)) = self.trim(v, j) else {
            return;
        };
        // A formless name must be a whole line's value, not a sentence's start.
        if b < self.w.len()
            && !self.w.breaks_before(b)
            && !self.w.text(b).starts_with([',', ';', '('])
        {
            return;
        }
        let words: Vec<usize> = (a..b).filter(|&k| self.w.is_word(k)).collect();
        // One formless word after a label is a field value ("Data", "Part"), not a company.
        if words.len() < 2 || words.iter().any(|&k| self.number(k)) {
            return;
        }
        if self
            .w
            .slice(a, b)
            .chars()
            .filter(|c| c.is_alphabetic())
            .count()
            < 3
        {
            return;
        }
        if let Some(mut m) = self.accept(a, b, None, None) {
            m.add_role(role);
            m.ctx.label = true;
            push_mention(out, m);
        }
    }

    /// A sentence-case name at `v` after a Serbian role label: a capitalised word, then words
    /// of letters (lowercase ones included) up to a comma, a parenthesis or the end of the
    /// line, two to [`MAX_RUN_WORDS`] words, at least one of them lowercase (the Title Case
    /// and capital names are the plain label path's).
    fn sentence_case_value(&self, v: usize) -> Option<(usize, usize)> {
        if !self.w.is_word(v) || !self.w.text(v).starts_with(char::is_uppercase) || self.is_stop(v)
        {
            return None;
        }
        let mut j = v;
        let mut words = 0;
        let mut lower = false;
        while j < self.w.len() && (j == v || !self.w.breaks_before(j)) {
            if self.w.is_word(j) {
                let t = self.w.text(j);
                // Prepositions join an institution's name ("fond za zdravstveno"); document
                // words and labels end it.
                let stop = has(vocab::WALK_STOP_WORDS, self.key(j)) && t.chars().count() > 3;
                if !t.chars().all(char::is_alphabetic) || j > v && stop {
                    return None;
                }
                lower |= t.starts_with(char::is_lowercase);
                words += 1;
                if words > MAX_RUN_WORDS {
                    return None;
                }
            } else if self.w.text(j).starts_with([',', '(', ';']) {
                break;
            } else if !self.inner_punct(j) {
                return None;
            }
            j += 1;
        }
        // A name cut by a line break ends in its preposition ("Републички фонд за"): the
        // rest is on the next line, in a column of a signature table more often than not.
        let last = (v..j).rev().find(|&k| self.w.is_word(k))?;
        let t = self.w.text(last);
        if t.starts_with(char::is_lowercase) && t.chars().count() <= 3 {
            return None;
        }
        (words >= 2 && lower).then_some((v, j))
    }

    // ----- name statements --------------------------------------------------------------

    fn marker_at(&self, p: usize) -> Option<(&'static str, bool, usize)> {
        if !MARKER_FIRST.contains(self.key(self.words[p])) {
            return None;
        }
        for (seq, kind, swapped, needs_sep) in MARKERS {
            if self.seq_at(p, seq) {
                let first = self.words[p];
                let last = self.words[p + seq.len() - 1];
                if *needs_sep && !self.w.slice(first, last + 1).contains(['/', '.']) {
                    continue;
                }
                return Some((kind, *swapped, last + 1));
            }
        }
        None
    }

    fn find_statements(&self, out: &mut PageScan) {
        let mut found: Vec<(usize, usize, &'static str, bool)> = Vec::new();
        for p in 0..self.words.len() {
            let Some((kind, swapped, after)) = self.marker_at(p) else {
                continue;
            };
            let marker = self.words[p];
            // X: a mention ending right before the marker, "(" or "," between.
            let Some(x) = out
                .mentions
                .iter()
                .rposition(|m| m.b <= marker && marker - m.b <= 3)
            else {
                continue;
            };
            let gap_ok = (out.mentions[x].b..marker).all(|k| {
                self.w.is_punct(k, '(') || self.w.is_punct(k, ',') || self.w.is_punct(k, '.')
            });
            if !gap_ok {
                continue;
            }
            let Some(y) = self.statement_other(out, after) else {
                continue;
            };
            found.push((x, y, kind, swapped));
        }
        for (x, y, kind, swapped) in found {
            let (current, other) = if swapped { (y, x) } else { (x, y) };
            let a = out.mentions[x].a.min(out.mentions[y].a);
            let b = out.mentions[x].b.max(out.mentions[y].b);
            out.statements.push(Statement {
                current,
                other,
                kind,
                a,
                b,
            });
        }
        // Mentions added for the other sides go to the end; sort and remap.
        let mut order: Vec<usize> = (0..out.mentions.len()).collect();
        order.sort_by_key(|&k| out.mentions[k].a);
        let mut rank = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            rank[old] = new;
        }
        let mut sorted: Vec<Mention> = order.iter().map(|&k| out.mentions[k].clone()).collect();
        std::mem::swap(&mut out.mentions, &mut sorted);
        for s in &mut out.statements {
            s.current = rank[s.current];
            s.other = rank[s.other];
        }
    }

    /// The other side of a statement starting at token `v`: an existing mention or a new one.
    #[allow(clippy::many_single_char_names)] // token indices
    fn statement_other(&self, out: &mut PageScan, start: usize) -> Option<usize> {
        let mut v = start;
        while self.w.is_punct(v, ':')
            || self.w.is_punct(v, '"')
            || self.w.is_punct(v, ',')
            || self.w.is_punct(v, '-')
        {
            v += 1;
        }
        if let Some(k) = out.mentions.iter().position(|m| m.a >= v && m.a <= v + 1) {
            return Some(k);
        }
        let mut j = v;
        let mut words = 0;
        while j < self.w.len() && (j == v || !self.w.breaks_before(j)) {
            if self.w.is_word(j) {
                if self.is_stop(j) || !(self.namey(j) || self.connector(j)) {
                    break;
                }
                words += 1;
                if words > MAX_RUN_WORDS {
                    return None;
                }
            } else if !self.inner_punct(j) || self.w.is_punct(j, '"') && j > v {
                break;
            }
            j += 1;
        }
        let (a, b) = self.trim(v, j)?;
        let surface = self.w.slice(a, b);
        quality::verdict(surface).ok()?;
        if surface.chars().filter(|c| c.is_alphabetic()).count() < 3 {
            return None;
        }
        let mut m = Mention {
            a,
            b,
            form: None,
            seat: None,
            name_end: b,
            roles: Vec::new(),
            term: None,
            ctx: Context::default(),
            from_statement: true,
            zone: self.w.toks[a].zone,
        };
        m.form = (a..b).find_map(|k| self.form_at(k).map(|(f, _)| f));
        if out.mentions.len() >= MAX_MENTIONS_PER_PAGE {
            return None;
        }
        out.mentions.push(m);
        Some(out.mentions.len() - 1)
    }

    // ----- context ----------------------------------------------------------------------

    fn any_word_in(&self, from: usize, to: usize, set: &[&str]) -> bool {
        (from..to.min(self.words.len())).any(|p| has(set, self.key(self.words[p])))
    }

    fn contextualize(&self, m: &mut Mention, next: usize, sig_line: Option<u32>) {
        let (pa, pb) = (self.wpos[m.a], self.wpos[m.b.min(self.w.len() - 1)]);
        let pb = if m.b >= self.w.len() {
            self.words.len()
        } else {
            pb
        };
        let opener = self.pos.opening
            && self.any_word_in(pa.saturating_sub(CLAUSE_BEFORE_WORDS), pa, CLAUSE_OPENERS);
        let details = self.any_word_in(pb, pb + CLAUSE_AFTER_WORDS, CLAUSE_DETAILS)
            || (pb..(pb + CLAUSE_AFTER_WORDS).min(self.words.len()))
                .any(|p| self.seq_at(p, &["registered", "office"]));
        m.ctx.clause = opener || details;
        m.term = self.defined_term(m.b, next);
        if let Some((_, Some(r))) = &m.term {
            m.add_role(*r);
        }
        let line = self.w.toks[m.a].line;
        let zone = self.w.toks[m.a].zone;
        m.ctx.title = zone == Zone::Title;
        m.ctx.signature = zone == Zone::Signature
            || self.signs_for(m.a)
            || sig_line.is_some_and(|s| line >= s)
            || self.over_by_line(m.b);
        m.ctx.copy_to = (pa.saturating_sub(6)..pa)
            .any(|p| self.seq_at(p, &["copy", "to"]) || self.seq_at(p, &["copies", "to"]));
        let inferable = !self.pos.has_zones || zone == Zone::Unknown;
        m.ctx.header =
            zone == Zone::Header || inferable && self.pos.first && line < LETTERHEAD_LINES;
        m.ctx.footer = zone == Zone::Footer
            || inferable
                && !m.ctx.signature
                && line + FOOTER_LINES >= self.w.lines
                && self.w.lines > 2 * FOOTER_LINES;
        m.ctx.bank = self.is_bank(m, pa);
        m.ctx.filer = self.is_filer_line(m.a);
        m.ctx.adviser = !m.ctx.label && self.is_adviser(m);
        let customer = m.roles.iter().any(|r| r.is_customer_side());
        if m.ctx.bank {
            m.ctx.header = false;
            m.ctx.footer = false;
            m.ctx.signature = false;
        }
        // The filer line repeats on every page, below the signatures too: it signs nothing.
        if m.ctx.filer {
            m.ctx.signature = false;
        }
        // An auditor signs its own report, and a firm listed under "Solicitors" sits next to
        // the registered office: neither is a party.
        if m.ctx.adviser {
            m.ctx.signature = false;
            m.ctx.clause = false;
            m.roles
                .retain(|r| !matches!(r, Role::Party | Role::Signatory));
            m.add_role(Role::Adviser);
        }
        if m.ctx.signature {
            m.add_role(Role::Signatory);
        }
        if (m.ctx.header || m.ctx.footer)
            && !customer
            && !m.ctx.label
            && !m.ctx.adviser
            && !m.ctx.filer
        {
            m.add_role(Role::Issuer);
        }
        if m.ctx.clause && m.roles.is_empty() {
            m.add_role(Role::Party);
        }
        if m.roles.is_empty() {
            m.add_role(Role::Mentioned);
        }
        if zone == Zone::Unknown || zone == Zone::Body || zone == Zone::Table {
            if m.ctx.signature {
                m.zone = Zone::Signature;
            } else if zone == Zone::Unknown && m.ctx.header {
                m.zone = Zone::Header;
            } else if zone == Zone::Unknown && m.ctx.footer {
                m.zone = Zone::Footer;
            }
        }
    }

    /// Tokens `a..b` are a person's name with a middle initial: `Brian J. Cirbo`.
    fn initialled_person(&self, a: usize, b: usize) -> bool {
        let words: Vec<usize> = (a..b).filter(|&k| self.w.is_word(k)).collect();
        let titled = |k: usize| {
            let t = self.w.text(k);
            let mut c = t.chars();
            c.next().is_some_and(char::is_uppercase)
                && c.clone().count() >= 1
                && c.all(char::is_lowercase)
        };
        words.len() == 3
            && titled(words[0])
            && titled(words[2])
            && self.w.text(words[1]).chars().count() == 1
            && self.w.is_punct(words[1] + 1, '.')
    }

    /// The mention ending at `b` ends its line and the next line opens with `By:` (a
    /// signature block: `ACME CORP.` / `By: /s/ ...`).
    fn over_by_line(&self, b: usize) -> bool {
        if b == 0 || b > self.w.len() {
            return false;
        }
        let line = self.w.toks[b - 1].line;
        let mut t = b;
        while t < self.w.len() && self.w.toks[t].line == line {
            if self.w.is_word(t) {
                return false;
            }
            t += 1;
        }
        t < self.w.len()
            && self.w.toks[t].line == line + 1
            && self.key(t) == "by"
            && (self.w.is_punct(t + 1, ':') || self.w.is_punct(t + 1, '/'))
    }

    /// `Source: X, 10-K, ...`: the mention sits on a line that opens with `Source:`, or on
    /// the line after a bare `Source:`.
    fn is_filer_line(&self, a: usize) -> bool {
        let Some(line) = self.w.toks.get(a).map(|t| t.line) else {
            return false;
        };
        let mut s = a;
        while s > 0 && self.w.toks[s - 1].line == line {
            s -= 1;
        }
        let opens = |t: usize| self.key(t) == "source" && self.w.is_punct(t + 1, ':');
        if opens(s) {
            return true;
        }
        // "Source:" alone on the line above.
        s >= 2 && self.w.breaks_before(s - 2) && opens(s - 2) && self.w.toks[s - 2].line + 1 == line
    }

    /// An adviser named as such: the mention opens its line under a short label line ending
    /// in an adviser word (`Auditors`, `Principal solicitors`, `Bankers`), or after such a
    /// label on its own line (`Auditors: X LLP`); or the words after it, on its line or the
    /// next, call it one (`X LLP, Chartered Accountants`, `X LLP, statutory auditor`).
    fn is_adviser(&self, m: &Mention) -> bool {
        let line = self.w.toks[m.a].line;
        // Label before it on its line, `Auditors: X LLP`.
        let mut s = m.a;
        while s > 0 && self.w.toks[s - 1].line == line {
            s -= 1;
        }
        let label_words = |from: usize, to: usize| -> Option<usize> {
            let words: Vec<usize> = (from..to).filter(|&k| self.w.is_word(k)).collect();
            (!words.is_empty() && words.len() <= 3).then(|| words[words.len() - 1])
        };
        if s < m.a
            && let Some(last) = label_words(s, m.a)
            && has(ADVISER_WORDS, self.key(last))
            && (last + 1..m.a).any(|k| self.w.is_punct(k, ':'))
        {
            return true;
        }
        // A label line right above a mention that opens its line.
        if s == m.a && m.a > 0 && self.w.toks[m.a - 1].line + 1 == line {
            let mut ls = m.a - 1;
            while ls > 0 && self.w.toks[ls - 1].line == line - 1 {
                ls -= 1;
            }
            if let Some(last) = label_words(ls, m.a)
                && has(ADVISER_WORDS, self.key(last))
                && (last + 1..m.a).all(|k| !self.w.is_word(k))
            {
                return true;
            }
        }
        // `X LLP, Chartered Accountants` / `X LLP` + `Statutory Auditor` on the next line.
        let mut k = m.b;
        let mut words = 0;
        while k < self.w.len() && words < 6 && self.w.toks[k].line <= line + 1 {
            if self.w.is_word(k) {
                if has(ADVISER_WORDS, self.key(k)) {
                    // On the next line only after a qualifier: a bare `Auditors` there is
                    // the next section's label. And it ends the phrase: `Independent
                    // Auditors' Report` under a name is a heading.
                    let ends = self.w.breaks_before(k + 1)
                        || [',', ';', '.', ')']
                            .iter()
                            .any(|&c| self.w.is_punct(k + 1, c));
                    if ends {
                        return words > 0 || self.w.toks[k].line == line;
                    }
                    // `Chartered Accountants and Statutory Auditor`: read on.
                } else if !has(ADVISER_QUALIFIERS, self.key(k)) {
                    return false;
                }
                words += 1;
            } else if !(self.w.is_punct(k, ',') || self.w.is_punct(k, '-')) {
                return false;
            }
            k += 1;
        }
        false
    }

    /// A bank: a bank word in the name, or a bank label before it on its line.
    fn is_bank(&self, m: &Mention, pa: usize) -> bool {
        let in_name = (m.a..m.b).any(|k| self.w.is_word(k) && has(BANK_WORDS, self.key(k)));
        let label = (pa.saturating_sub(4)..pa).any(|p| {
            let t = self.words[p];
            self.same_line(t, m.a) && has(BANK_WORDS, self.key(t))
        });
        in_name || label
    }

    // ----- brands spelled by a domain ---------------------------------------------------

    /// Formless names of one to three capitalised words that spell a domain label of the
    /// document ("KESTRELON" next to `@kestrelon.com`).
    fn find_brands(&self, out: &mut PageScan) {
        if self.domains.is_empty() {
            return;
        }
        let taken =
            |out: &PageScan, a: usize, b: usize| out.mentions.iter().any(|m| m.a < b && a < m.b);
        let mut per_label = vec![0usize; self.domains.len()];
        let mut p = 0;
        while p < self.words.len() {
            let t = self.words[p];
            if !self.namey(t) || self.is_stop(t) {
                p += 1;
                continue;
            }
            let mut hit = None;
            let mut squashed = String::new();
            for n in 0..3 {
                let Some(&k) = self.words.get(p + n) else {
                    break;
                };
                let adjacent = n == 0 || k == self.words[p + n - 1] + 1 && self.same_line(t, k);
                if !adjacent || !self.namey(k) {
                    break;
                }
                squashed.extend(self.key(k).chars().filter(|c| c.is_alphanumeric()));
                if let Some(d) = self.domains.iter().position(|d| *d == squashed) {
                    hit = Some((d, k + 1));
                }
            }
            let Some((d, b)) = hit else {
                p += 1;
                continue;
            };
            if per_label[d] < 3
                && !taken(out, t, b)
                && out.mentions.len() < MAX_MENTIONS_PER_PAGE
                && let Some(mut m) = self.accept_brand(t, b)
            {
                m.ctx.domain = true;
                per_label[d] += 1;
                out.mentions.push(m);
            }
            p = self.wpos[b.min(self.w.len() - 1)].max(p + 1);
        }
    }

    fn accept_brand(&self, a: usize, b: usize) -> Option<Mention> {
        let surface = self.w.slice(a, b);
        if quality::verdict(surface).ok()? != Quality::Ok || quality::is_person_shaped(surface) {
            return None;
        }
        Some(Mention {
            a,
            b,
            form: None,
            seat: None,
            name_end: b,
            roles: Vec::new(),
            term: None,
            ctx: Context::default(),
            from_statement: false,
            zone: self.w.toks[a].zone,
        })
    }

    /// "For and on behalf of X", "On behalf of X", "Za X" at a line start, "Za i u ime X".
    fn signs_for(&self, a: usize) -> bool {
        let p = self.wpos[a];
        let before = |n: usize, seq: &[&str]| {
            p >= n && self.seq_at(p - n, seq) && self.same_line(self.words[p - n], a)
        };
        // "For and on behalf of" / "Belgacom S.A." on the next line.
        let line_above = |n: usize, seq: &[&str]| {
            p >= n
                && self.seq_at(p - n, seq)
                && self.w.breaks_before(a)
                && self.w.toks[self.words[p - 1]].line + 1 == self.w.toks[a].line
        };
        before(2, &["behalf", "of"])
            || line_above(2, &["behalf", "of"])
            || before(4, &["za", "i", "u", "ime"])
            || before(5, &["u", "ime", "i", "za", "racun"])
            || (before(1, &["za"]) || before(1, &["for"]))
                && self.w.breaks_before(self.words[p - 1])
    }

    /// A defined term after a name: `(u daljem tekstu: Naručilac)`, `(hereinafter the
    /// "Supplier")`, `("Company")`, `hereinafter referred to as "Customer"`.
    fn defined_term(&self, b: usize, next: usize) -> Option<(String, Option<Role>)> {
        let stop = next.min(b + TERM_WINDOW).min(self.w.len());
        let mut k = b;
        while k < stop {
            let opens = self.w.is_punct(k, '(')
                || self.key(k) == "hereinafter"
                || self.key(k) == "daljem" && self.key(k + 1) == "tekstu"
                || self.key(k) == "dalje" && self.w.is_punct(k + 1, ':');
            if opens && let Some(t) = self.parse_term(k + 1, stop) {
                return Some(t);
            }
            k += 1;
        }
        None
    }

    fn parse_term(&self, mut j: usize, stop: usize) -> Option<(String, Option<Role>)> {
        let mut quoted = false;
        while j < stop {
            if self.w.is_punct(j, '"') || self.w.is_punct(j, '\'') {
                quoted = true;
                j += 1;
            } else if self.w.is_punct(j, ':')
                || self.w.is_punct(j, ',')
                || self.w.is_word(j) && has(TERM_FILLERS, self.key(j))
            {
                j += 1;
            } else {
                break;
            }
        }
        let a = j;
        let mut words = 0;
        while j < stop && self.w.is_word(j) && words < 4 {
            if !self.w.text(j).starts_with(char::is_uppercase) && words == 0 {
                return None;
            }
            words += 1;
            j += 1;
        }
        if words == 0 {
            return None;
        }
        let closes = self.w.is_punct(j, ')')
            || self.w.is_punct(j, '"')
            || self.w.is_punct(j, '\'')
            || quoted && (self.w.is_punct(j, ',') || self.w.is_punct(j, '.'));
        if !closes {
            return None;
        }
        let role = (a..j).find_map(|k| role_of_word(self.key(k)));
        Some((self.w.slice(a, j).to_owned(), role))
    }

    // ----- signatures -------------------------------------------------------------------

    /// First line of the signature block on this page, when it is one of the last two.
    fn signature_start(&self) -> Option<u32> {
        if !self.pos.near_end {
            return None;
        }
        for p in 0..self.words.len() {
            let t = self.words[p];
            let colon = self.w.is_punct(t + 1, ':');
            let cue = SIGN_FIRST.contains(self.key(t))
                && SIGN_CUES.iter().any(|s| self.seq_at(p, s))
                || colon
                    && has(
                        &["by", "name", "title", "potpisao", "funkcija"],
                        self.key(t),
                    );
            if cue {
                return Some(self.w.toks[t].line);
            }
        }
        None
    }

    fn find_titles(&self, out: &mut PageScan, sig_line: Option<u32>) {
        let Some(sig) = sig_line else {
            return;
        };
        let mut lines: Vec<u32> = Vec::new();
        let mut p = 0;
        while p < self.words.len() {
            let t = self.words[p];
            if self.w.toks[t].line < sig {
                p += 1;
                continue;
            }
            let line = self.w.toks[t].line;
            if let Some(seq) = TITLE_FIRST
                .contains(self.key(t))
                .then(|| TITLES.iter().find(|s| self.seq_at(p, s)))
                .flatten()
            {
                let b = self.words[p + seq.len() - 1] + 1;
                if out.titles.len() < 8 {
                    out.titles.push(Title {
                        text: self.w.slice(t, b).to_owned(),
                        a: t,
                        b,
                    });
                }
                if !lines.contains(&line) {
                    lines.push(line);
                }
                p += seq.len();
                continue;
            }
            let label = self.w.is_punct(t + 1, ':')
                && (has(&["by", "name", "potpisao"], self.key(t))
                    || self.seq_at(p, &["ime", "i", "prezime"]));
            if label && !lines.contains(&line) {
                lines.push(line);
            }
            p += 1;
        }
        out.signatory_lines = lines.len();
    }
}

fn push_mention(out: &mut PageScan, m: Mention) {
    if out.mentions.len() < MAX_MENTIONS_PER_PAGE {
        out.mentions.push(m);
    } else {
        out.dropped += 1;
    }
}

/// First words of [`MARKERS`], [`SIGN_CUES`] and [`TITLES`]: a cheap test before the
/// sequences are tried (built once, read-only).
static MARKER_FIRST: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| MARKERS.iter().map(|m| m.0[0]).collect());
static SIGN_FIRST: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| SIGN_CUES.iter().map(|s| s[0]).collect());
static TITLE_FIRST: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| TITLES.iter().map(|s| s[0]).collect());

/// Words naming a bank.
const BANK_WORDS: &[&str] = &[
    "bank",
    "banka",
    "banke",
    "banca",
    "banque",
    "bancaria",
    "beneficiary",
    "korespondent",
    "intermediary",
    "swift",
    "iban",
];

/// Second-level labels of the e-mail and web domains in `text` (lowercase, at least four
/// characters, free-mail and platform hosts left out), at most `cap`, in order of appearance.
///
/// ```
/// use quail_parties::scan::domain_labels;
/// let t = "Kontakt: prodaja@alfatek.rs, www.alfatek.rs, ime@gmail.com, https://portal.beta-soft.co.uk/x";
/// assert_eq!(domain_labels(t, 8), ["alfatek", "betasoft"]);
/// ```
pub fn domain_labels(text: &str, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && out.len() < cap {
        let start = if b[i] == b'@' {
            Some(i + 1)
        } else if b[i..].starts_with(b"www.") && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            Some(i + 4)
        } else if b[i..].starts_with(b"://") {
            Some(i + 3)
        } else {
            None
        };
        let Some(s) = start else {
            i += 1;
            continue;
        };
        let mut e = s;
        while e < b.len()
            && e - s < 255
            && (b[e].is_ascii_alphanumeric() || b[e] == b'.' || b[e] == b'-')
        {
            e += 1;
        }
        if let Some(label) = host_label(&text[s..e])
            && !out.contains(&label)
        {
            out.push(label);
        }
        i = e.max(i + 1);
    }
    out
}

fn host_label(host: &str) -> Option<String> {
    let host = host.trim_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let mut k = parts.len() - 2;
    if k > 0
        && matches!(
            parts[k],
            "co" | "com" | "org" | "net" | "ac" | "gov" | "edu" | "ltd" | "plc" | "in"
        )
    {
        k -= 1;
    }
    let label: String = parts[k]
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    if label.len() < 4
        || label.chars().all(|c| c.is_ascii_digit())
        || GENERIC_HOSTS.contains(&label.as_str())
    {
        return None;
    }
    Some(label)
}

/// Hosts that say nothing about who wrote a document.
const GENERIC_HOSTS: &[&str] = &[
    "gmail",
    "googlemail",
    "yahoo",
    "hotmail",
    "outlook",
    "live",
    "icloud",
    "mail",
    "email",
    "google",
    "microsoft",
    "office",
    "linkedin",
    "facebook",
    "twitter",
    "instagram",
    "youtube",
    "docusign",
    "adobe",
    "sharepoint",
    "onmicrosoft",
    "protonmail",
    "aol",
    "yandex",
    "abv",
    "eunet",
    "sbb",
    "mts",
    "telekom",
    "orion",
    "verat",
    "ptt",
    "example",
    "w3",
    "schemas",
    "openxmlformats",
    "purl",
    "apache",
    "github",
    "wikipedia",
    "gov",
    "europa",
];

/// "Akcionarsko društvo X": the prefix spelling of `a.d.`.
static AD_PREFIX: Form = Form {
    bare: "ad",
    canon: "a.d.",
    position: Position::Prefix,
    gate: forms::Gate::Any,
    weak: false,
    jurisdictions: &["rs", "ba", "me", "mk"],
};

/// Words that name an adviser of the document's author in a label (`Auditors`, `Principal
/// solicitors`) or after a name (`Chartered Accountants`, `Statutory Auditor`).
const ADVISER_WORDS: &[&str] = &[
    "auditor",
    "auditors",
    "accountants",
    "bankers",
    "solicitors",
    "advisers",
    "advisors",
    "actuary",
    "actuaries",
    "custodian",
    "custodians",
    "revizor",
];

/// Nouns for a person acting for a party (`odgovorno lice naručioca`, `contact person of the
/// Supplier`): the label after them introduces a person or a title.
const PERSON_NOUNS: &[&str] = &[
    "lice",
    "lica",
    "licu",
    "osoba",
    "osobe",
    "person",
    "representative",
    "officer",
];

/// Words of form field labels (`Submit EDI Invoice`, `Other Address`): a formless value after
/// a role label holding one is the next field, not a name.
const FIELD_WORDS: &[&str] = &[
    "invoice",
    "address",
    "phone",
    "fax",
    "email",
    "contact",
    "date",
    "number",
    "total",
    "page",
    "estimate",
    "account",
    "attention",
    "attn",
    "submit",
    "edi",
    "remit",
];

/// US state codes: after a city they are no legal form (`NV`, `CO`).
const US_STATES: &[&str] = &[
    "al", "ak", "az", "ar", "ca", "co", "ct", "de", "fl", "ga", "hi", "id", "il", "in", "ia", "ks",
    "ky", "la", "me", "md", "ma", "mi", "mn", "ms", "mo", "mt", "ne", "nv", "nh", "nj", "nm", "ny",
    "nc", "nd", "oh", "ok", "or", "pa", "ri", "sc", "sd", "tn", "tx", "ut", "vt", "va", "wa", "wv",
    "wi", "wy", "dc",
];

/// Abbreviations of table column headings (`Max`, `Min`, `Len`, `Qty`): a legal form after
/// them only is a heading row, not a name.
const HEADER_ABBREVIATIONS: &[&str] = &[
    "max", "min", "len", "qty", "pos", "avg", "amt", "pct", "ttl", "tot", "num", "nbr", "desc",
    "dur", "hrs", "wk", "wks", "ea", "br", "uom", "kol", "jm", "rb",
];

/// Words between a name and the adviser word that calls it one (`Chartered Accountants and
/// Statutory Auditor`).
const ADVISER_QUALIFIERS: &[&str] = &[
    "chartered",
    "certified",
    "statutory",
    "registered",
    "public",
    "independent",
    "external",
    "senior",
    "and",
    "the",
];

/// More words that end a walk (English function words and Serbian generic company words).
const EXTRA_STOP: &[&str] = &[
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "has",
    "have",
    "had",
    "article",
    "section",
    "clause",
    "clan",
    "clana",
    "paragraph",
    "stav",
    "tacka",
    "chapter",
    "glava",
    "odeljak",
    "such",
    "said",
    "other",
    "same",
    "upon",
    "under",
    "hereby",
    "herein",
    "thereof",
    "shall",
    "may",
    "must",
    "will",
    "if",
    "when",
    "where",
    "than",
    "then",
    "also",
    "only",
    "both",
    "privredno",
    "drustvo",
    "preduzece",
    "firma",
    "kompanija",
    "republika",
    "republike",
    "dear",
    "regards",
    "sincerely",
    "please",
    "thank",
    "thanks",
    "total",
    "subtotal",
    "amount",
    "iznos",
    "ukupno",
    "price",
    "cena",
    "qty",
    "kolicina",
    "description",
    "opis",
    "item",
    "stavka",
    "note",
    "napomena",
    "phone",
    "telefon",
    "mobile",
    "mob",
    // Recitals, headings and signature-block titles before a name: "WHEREAS, X Corp.",
    // "JOINDER OF X, INC.", "Vice President X Inc.", "Board of Directors of X Corporation".
    "whereas",
    "joinder",
    "confidential",
    "president",
    "ceo",
    "cfo",
    "coo",
    "cto",
    "vp",
    "svp",
    "evp",
    "gc",
    "employee",
    "employer",
    "director",
    "directors",
    "board",
    "officer",
    "chairman",
    "secretary",
    "treasurer",
    "manager",
    "title",
    "contact",
    "witness",
    "witnesseth",
    "persons",
    "stockholder",
    "stockholders",
    "shareholder",
    "shareholders",
    "disclosing",
    "receiving",
];

/// Forms that take a registration number as the first word of a name (`2205925 Ontario
/// Limited`), by bare key.
const NUMBERED_FORMS: &[&str] = &[
    "limited",
    "ltd",
    "inc",
    "incorporated",
    "corp",
    "corporation",
];

/// Prepositions that may open a name when capitalised mid-sentence (`At Home Corporation`,
/// `ON Semiconductor`).
const NAME_OPENING_WORDS: &[&str] = &[
    "at", "on", "in", "by", "for", "to", "with", "up", "into", "over",
];

/// Words that make no company of a weak legal form on their own ("HOLDING CO.").
const WEAK_FORM_NOUNS: &[&str] = &[
    "holding",
    "holdings",
    "parent",
    "operating",
    "the",
    "a",
    "such",
    "our",
    "your",
    "its",
    "insurance",
    "trust",
    "investment",
    "management",
];

/// Role words that may still be part of a name ("Service", "Services", "Company" is a form).
const NAME_OK_ROLE_WORDS: &[&str] = &[
    "service", "services", "first", "second", "primary", "agency", "us",
];

/// Lowercase words that may stand inside a name.
const NAME_CONNECTORS: &[&str] = &[
    "and", "of", "i", "und", "et", "de", "del", "della", "da", "di", "van", "von", "der", "den",
    "du", "la", "le", "y", "e",
];

/// Words before a name that open a parties clause (first page only).
const CLAUSE_OPENERS: &[&str] = &[
    "izmedju",
    "between",
    "among",
    "zakljucen",
    "zakljucuju",
    "zakljucili",
    "zakljucen",
    "zakljucenog",
    "entered",
    "ugovorne",
    "parties",
];

/// Words after a name that give its registration details.
const CLAUSE_DETAILS: &[&str] = &[
    "sedistem",
    "sediste",
    "sedista",
    "incorporated",
    "organized",
    "organised",
    "existing",
    "maticni",
    "pib",
    "zastupa",
    "zastupnik",
    "represented",
    "principal",
    "registration",
    "registracioni",
    "mb",
    // A Serbian street address right after the name ("X d.o.o., ul. Y 3").
    "ul",
    "ulica",
    "ulici",
    "bulevar",
    "bb",
];

/// Words skipped before a defined term.
const TERM_FILLERS: &[&str] = &[
    "u",
    "daljem",
    "tekstu",
    "dalje",
    "hereinafter",
    "referred",
    "to",
    "as",
    "called",
    "the",
    "kao",
    "collectively",
    "individually",
    "jointly",
    "each",
    "a",
    "an",
    "also",
    "known",
    "together",
    "with",
    "its",
    "affiliates",
    "zajedno",
];

/// Name-statement markers: words, kind, swapped (old name first), needs `/` or `.`.
const MARKERS: &[(&[&str], &str, bool, bool)] = &[
    (&["formerly", "known", "as"], "former_name", false, false),
    (&["previously", "known", "as"], "former_name", false, false),
    (&["formerly"], "former_name", false, false),
    (&["previously"], "former_name", false, false),
    (&["f", "k", "a"], "former_name", false, true),
    (&["f.k.a"], "former_name", false, false),
    (&["fka"], "former_name", false, false),
    (&["prethodno"], "former_name", false, false),
    (&["ranije"], "former_name", false, false),
    (&["bivse"], "former_name", false, false),
    (&["bivsi"], "former_name", false, false),
    (&["bivsa"], "former_name", false, false),
    (&["nekadasnje"], "former_name", false, false),
    (&["nekadasnji"], "former_name", false, false),
    (&["now", "known", "as"], "former_name", true, false),
    (&["n", "k", "a"], "former_name", true, true),
    (&["nka"], "former_name", true, false),
    (&["renamed"], "former_name", true, false),
    (&["trading", "as"], "trading_name", false, false),
    (&["t", "a"], "trading_name", false, true),
    (&["doing", "business", "as"], "dba", false, false),
    (&["d", "b", "a"], "dba", false, true),
    (&["d.b.a"], "dba", false, false),
    (&["dba"], "dba", false, false),
];

/// Strong signature-block cues.
const SIGN_CUES: &[&[&str]] = &[
    &["in", "witness", "whereof"],
    &["potpis"],
    &["potpisi"],
    &["signature"],
    &["signatures"],
    &["for", "and", "on", "behalf", "of"],
    &["signed", "for"],
    &["za", "i", "u", "ime"],
    &["m.p"],
    &["mp"],
    &["ime", "i", "prezime"],
    &["ovlasceno", "lice"],
];

/// Titles a signatory signs under (no person names).
const TITLES: &[&[&str]] = &[
    &["generalni", "direktor"],
    &["izvrsni", "direktor"],
    &["managing", "director"],
    &["executive", "director"],
    &["general", "manager"],
    &["chief", "executive", "officer"],
    &["chief", "financial", "officer"],
    &["vice", "president"],
    &["authorised", "signatory"],
    &["authorized", "signatory"],
    &["ovlasceno", "lice"],
    &["zakonski", "zastupnik"],
    &["direktor"],
    &["direktorka"],
    &["director"],
    &["ceo"],
    &["cfo"],
    &["president"],
    &["predsednik"],
    &["chairman"],
    &["zastupnik"],
];
