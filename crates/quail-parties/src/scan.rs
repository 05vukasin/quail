//! Finding company mentions, their roles, defined terms, name statements and signature
//! titles on one page. Every rule looks a bounded number of tokens around an anchor, so a
//! page is scanned in linear time.

use std::collections::HashSet;
use std::sync::LazyLock;

use quail_core::Zone;

use crate::forms::{self, Form, Position};
use crate::quality::{self, Quality};
use crate::roles::{LABELS, Role, role_of_word};
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
pub struct PagePos {
    /// First page.
    pub first: bool,
    /// One of the last two pages (signature blocks live there).
    pub near_end: bool,
    /// The extractor gave zones.
    pub has_zones: bool,
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
        let a = self.walk_left(i, floor)?;
        // ... and it needs a real word before it ("C. Company" is an initial).
        if form.bare == "company"
            && !(a..i).any(|k| {
                self.w.is_word(k)
                    && self.w.text(k).chars().filter(|c| c.is_alphabetic()).count() >= 3
            })
        {
            return None;
        }
        // A weak form after a number is a clause reference, not a name ("Article 20 AS").
        if form.weak && (a..i).any(|k| self.number(k)) {
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
        let a = self.walk_left(i, floor)?;
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

    /// First token of the name ending right before the form at `i`, or `None`.
    fn walk_left(&self, i: usize, floor: usize) -> Option<usize> {
        let mut j = i;
        // "Acme, Inc.": one comma between the name and its form.
        if j > floor + 1 && self.w.is_punct(j - 1, ',') && self.same_line(j - 2, j) {
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
                if anchor != i || words == 0 || !self.wraps_into(k) {
                    break;
                }
                anchor = k;
            }
            if self.w.is_word(k) {
                // Inside a hyphenated compound a stop word is part of the name ("Build-A-Bear").
                let hyphened =
                    self.w.is_punct(k + 1, '-') && self.attached(k + 1) && self.attached(k + 2);
                if self.is_stop(k) && !hyphened && !self.connector(k)
                    || self.form_word(k)
                    || has(EXTRA_STOP, self.bare(k))
                {
                    break;
                }
                if !(self.namey(k) || self.connector(k)) {
                    break;
                }
                let lower = self.w.text(k).chars().any(char::is_lowercase);
                if caps_run && lower && !self.connector(k) {
                    break;
                }
                if lower {
                    caps_run = false;
                } else if self.all_caps(k) {
                    caps_run = true;
                }
                words += 1;
                if words > MAX_WALK_WORDS {
                    break;
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
            } else if !self.inner_punct(k) {
                break;
            }
            j = k;
        }
        self.trim(j, i).map(|(a, _)| a)
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
                || self.number(a))
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
            // Side-by-side columns ("Remit To: Ship To:" over two names) say nothing.
            if self.w.breaks_before(v) && self.labels_on_line(i) > 1 {
                i = end;
                continue;
            }
            self.label_value(out, role, v);
            i = v.max(i + 1);
        }
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
    fn label_value(&self, out: &mut PageScan, role: Role, v: usize) {
        let mut v = v;
        while self.w.is_punct(v, '"') || self.w.is_punct(v, '-') {
            v += 1;
        }
        if let Some(m) = out.mentions.iter_mut().find(|m| m.a >= v && m.a <= v + 2) {
            m.add_role(role);
            m.ctx.label = true;
            return;
        }
        let mut j = v;
        let mut words = 0;
        while j < self.w.len() && (j == v || !self.w.breaks_before(j)) {
            if self.w.is_word(j) {
                if self.is_stop(j) && j == v || !(self.namey(j) || self.connector(j)) {
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
        let opener = self.pos.first
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
        m.ctx.signature =
            zone == Zone::Signature || self.signs_for(m.a) || sig_line.is_some_and(|s| line >= s);
        let inferable = !self.pos.has_zones || zone == Zone::Unknown;
        m.ctx.header =
            zone == Zone::Header || inferable && self.pos.first && line < LETTERHEAD_LINES;
        m.ctx.footer = zone == Zone::Footer
            || inferable
                && !m.ctx.signature
                && line + FOOTER_LINES >= self.w.lines
                && self.w.lines > 2 * FOOTER_LINES;
        m.ctx.bank = self.is_bank(m, pa);
        let customer = m.roles.iter().any(|r| r.is_customer_side());
        if m.ctx.bank {
            m.ctx.header = false;
            m.ctx.footer = false;
            m.ctx.signature = false;
        }
        if m.ctx.signature {
            m.add_role(Role::Signatory);
        }
        if (m.ctx.header || m.ctx.footer) && !customer && !m.ctx.label {
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
        before(2, &["behalf", "of"])
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
    "stockholder",
    "stockholders",
    "shareholder",
    "shareholders",
    "disclosing",
    "receiving",
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
