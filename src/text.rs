//! Messages that know which of their parts are private (SPEC §1 rule 9). The code that composes a message
//! marks what belongs to the machine (another program's name, a covering window's title, a local path, a
//! phone's serial), each part with the words a published surface shows instead. A `Text` renders twice:
//! `local()` is the message as strobe always wrote it (terminal, result.json, query), `public()` puts each
//! marked part's public words in its place (a PR block, a summary, a card). Nothing guesses afterwards.
//!
//! What another program or the OS gave strobe arrives as a `Private<T>`, which has neither `Display` nor
//! `Debug`: `format!("{}", p)` and `format!("{p:?}")` do not compile. It goes into a message only through a
//! constructor that takes its public words (`redacted`, `quoted_or`, `Text::process_shares`, …). `msg!` is
//! `format!` for a `Text`: its arguments keep their marks at the right bytes, and it refuses a `Private`
//! and a path's `display()` (`Text::path` says a path with its public form).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

/// What a marked part is: what kind of thing the machine's owner would not publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivKind {
    /// Another program running on the machine: its name, its command line.
    ForeignProc,
    /// Another program's window title or class.
    WindowTitle,
    /// Another program's accessibility element: its role, name, key.
    ForeignElement,
    /// What strobe runs under (its terminal, an agent, a remote shell's daemon).
    HostAncestor,
    /// A folder or file of this machine outside the repository.
    LocalPath,
    /// A user name or an account's id.
    UserAccount,
    /// The machine's name.
    HostName,
    /// A phone's serial, its wireless address, a device's id.
    DeviceId,
    /// A URL's credentials or secret parameters.
    UrlSecret,
    /// What another tool printed (a build, a fixture): paths and names strobe cannot type.
    ToolOutput,
    /// A private value read back from a file, which does not say its kind.
    Unknown,
}

/// A value from this machine that a published surface must not show. It has no `Display` and no `Debug`
/// (`scripts/privacy-check.ts` keeps it so): a message takes it only through `redacted` or `quoted_or`,
/// with the words a public surface shows instead. Serialized as the value itself (result.json is local).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Private<T>(T, PrivKind);

impl<T> Private<T> {
    pub fn new(value: T, kind: PrivKind) -> Self {
        Private(value, kind)
    }

    pub fn kind(&self) -> PrivKind {
        self.1
    }

    /// The value itself, for what never becomes text: an OS call, a wire protocol, a comparison with
    /// another of this machine's names. Every use is listed in `scripts/privacy-check.ts`.
    #[expect(dead_code, reason = "no entry point hands out a Private yet")]
    pub(crate) fn expose_local(&self) -> &T {
        &self.0
    }
}

impl<T: AsRef<str>> Private<T> {
    /// The value as it is locally; `public` on a public surface (`the phone`, `another service`).
    pub fn redacted(&self, public: impl Into<String>) -> Text {
        Text::marked(self.0.as_ref(), self.1, public)
    }

    /// The value quoted as `{:?}` quotes it locally (`"Title"`, escapes included); `public` unquoted on a
    /// public surface (`a window`).
    pub fn quoted_or(&self, public: impl Into<String>) -> Text {
        Text::marked(format!("{:?}", self.0.as_ref()), self.1, public)
    }
}

impl<T: Serialize> Serialize for Private<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Private<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Private(T::deserialize(d)?, PrivKind::Unknown))
    }
}

/// A marked part of a `Text`: bytes `at[0]..at[1]` of its local text, and the words a public surface
/// shows in their place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub at: [usize; 2],
    pub kind: PrivKind,
    pub public: String,
}

/// A message and its marked parts: sorted, never overlapping, each on character boundaries of `local`.
/// No `Display`: `format!` would drop the marks, so a `Text` goes into another through `msg!` or `push`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    local: String,
    spans: Vec<Span>,
}

impl Text {
    pub fn new() -> Text {
        Text::default()
    }

    /// All of `local` marked, `public` in its place on a public surface. Marks inside `local` (a `Text`
    /// built of marked parts) give way to this one.
    pub fn marked(local: impl Into<Text>, kind: PrivKind, public: impl Into<String>) -> Text {
        let local = local.into().local;
        let end = local.len();
        Text { local, spans: vec![Span { at: [0, end], kind, public: public.into() }] }
    }

    /// A text and its marks as a file kept them. Marks that do not fit the text (out of its bounds, off a
    /// character boundary, overlapping) give the whole text as one mark, its public form the scrub's.
    pub fn from_parts(local: String, mut spans: Vec<Span>) -> Text {
        spans.sort_by_key(|s| s.at);
        let mut end = 0;
        let fits = spans.iter().all(|s| {
            let ok = s.at[0] >= end && s.at[0] <= s.at[1] && local.is_char_boundary(s.at[0]) && local.is_char_boundary(s.at[1]);
            end = s.at[1];
            ok
        });
        match fits {
            true => Text { local, spans },
            false => {
                let kind = spans.first().map_or(PrivKind::Unknown, |s| s.kind);
                let public = crate::app::pr::public(&local);
                Text::marked(local, kind, public)
            }
        }
    }

    /// The message as strobe writes it on this machine.
    pub fn local(&self) -> &str {
        &self.local
    }

    pub fn into_local(self) -> String {
        self.local
    }

    /// The message for a published surface: each marked part's public words in its place. Borrowed when
    /// nothing is marked.
    pub fn public(&self) -> Cow<'_, str> {
        if self.spans.is_empty() {
            return Cow::Borrowed(&self.local);
        }
        let mut out = String::with_capacity(self.local.len());
        let mut at = 0;
        for s in &self.spans {
            out += &self.local[at..s.at[0]];
            out += &s.public;
            at = s.at[1];
        }
        out += &self.local[at..];
        Cow::Owned(out)
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn is_marked(&self) -> bool {
        !self.spans.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.local.is_empty()
    }

    pub fn push_str(&mut self, s: &str) {
        self.local.push_str(s);
    }

    /// `other` after this text, its marks moved by this text's length.
    pub fn push(&mut self, other: &Text) {
        let base = self.local.len();
        self.local.push_str(&other.local);
        self.spans.extend(other.spans.iter().map(|s| Span { at: [s.at[0] + base, s.at[1] + base], ..s.clone() }));
    }

    /// The texts with `sep` between them.
    pub fn join(parts: impl IntoIterator<Item = Text>, sep: &str) -> Text {
        let mut out = Text::new();
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push_str(sep);
            }
            out.push(&p);
        }
        out
    }

    /// At most `max` characters, the last a `…` when it is cut. A cut never falls inside a marked part:
    /// one that would be cut ends the text before it, whole parts before it are kept.
    pub fn clip(&self, max: usize) -> Text {
        if self.local.chars().count() <= max {
            return self.clone();
        }
        let mut cut = self.local.char_indices().nth(max.saturating_sub(1)).map_or(self.local.len(), |(i, _)| i);
        if let Some(s) = self.spans.iter().find(|s| s.at[0] < cut && cut < s.at[1]) {
            cut = s.at[0];
        }
        let mut out = Text { local: self.local[..cut].to_string(), spans: self.spans.iter().filter(|s| s.at[1] <= cut).cloned().collect() };
        out.local.push('…');
        out
    }

    /// Other processes and their share of a core (`name 130%, name 14%`, as `validity::named_work` lists
    /// them), in public `1 other process, 130% of a core` or `3 other processes, the busiest 130% of a
    /// core`. Empty for no process: the caller says `none named` and why.
    pub fn process_shares(who: &[(Private<String>, f64)]) -> Text {
        if who.is_empty() {
            return Text::new();
        }
        let local = who.iter().map(|(n, p)| format!("{} {p:.0}%", n.0)).collect::<Vec<_>>().join(", ");
        let peak = who.iter().map(|w| w.1).fold(f64::NAN, f64::max);
        let public = match who.len() {
            1 => format!("1 other process, {peak:.0}% of a core"),
            n => format!("{n} other processes, the busiest {peak:.0}% of a core"),
        };
        Text::marked(local, PrivKind::ForeignProc, public)
    }

    /// Other apps by name (`Mail, Notes`, the apps whose windows covered the target's), in public their
    /// count (`2 other apps`). Empty for none.
    pub fn covering_apps(apps: &[Private<String>]) -> Text {
        if apps.is_empty() {
            return Text::new();
        }
        let local = apps.iter().map(|a| a.0.as_str()).collect::<Vec<_>>().join(", ");
        let public = match apps.len() {
            1 => "1 other app".to_string(),
            n => format!("{n} other apps"),
        };
        Text::marked(local, PrivKind::ForeignProc, public)
    }

    /// Another program's window by its title, quoted (`"Title"`); `another window` in public.
    pub fn window(title: &Private<String>) -> Text {
        title.quoted_or("another window")
    }

    /// A path as `display()` writes it. In public: from the repository's root (`perf/app.bench.ts`), from
    /// strobe's state folder (`<state>/experiments/…`), under the home folder by its last part
    /// (`~/…/notes.txt`, or `~` for the home itself), and any other folder by its last part (`…/v1`). A
    /// relative path stays, unless it climbs out of where it starts (`../x`: `…/x`).
    pub fn path(p: &Path, repo_root: Option<&Path>) -> Text {
        let local = p.display().to_string();
        let public = public_path(p, repo_root);
        if public == local { Text::from(local) } else { Text::marked(local, PrivKind::LocalPath, public) }
    }

    /// A URL; in public without its credentials (`user:pass@`) and secret parameters (`?token=…`).
    pub fn url(url: &str) -> Text {
        let public = crate::host::ios::logs::scrub_url(url, false);
        if public == url { Text::from(url) } else { Text::marked(url, PrivKind::UrlSecret, public) }
    }

    /// What another tool printed (a build's, a fixture's output): in public, from the repository's root
    /// for paths in it, then without other folders and other programs' names (`pr::public`).
    pub fn external_output(raw: &str, repo_root: Option<&Path>) -> Text {
        let mut public = raw.to_string();
        if let Some(root) = repo_root.map(|r| r.display().to_string()) {
            let root = root.trim_end_matches(['/', '\\']);
            if !root.is_empty() {
                public = public.replace(&format!("{root}/"), "").replace(&format!("{root}\\"), "").replace(root, ".");
            }
        }
        let public = crate::app::pr::public(&public);
        if public == raw { Text::from(raw) } else { Text::marked(raw, PrivKind::ToolOutput, public) }
    }
}

/// `p`'s public form (`Text::path`).
fn public_path(p: &Path, repo_root: Option<&Path>) -> String {
    let leaf = p.file_name().map(|n| n.to_string_lossy().to_string());
    let tail = |lead: &str| leaf.as_ref().map_or_else(|| lead.to_string(), |l| format!("{lead}/{l}"));
    if !p.has_root() && p.components().next().is_some_and(|c| !matches!(c, Component::Prefix(_))) {
        return if p.components().any(|c| c == Component::ParentDir) { tail("…") } else { p.display().to_string() };
    }
    let joined = |rest: Vec<String>| rest.join("/");
    if let Some(rest) = repo_root.and_then(|r| below(p, r)) {
        return if rest.is_empty() { ".".into() } else { joined(rest) };
    }
    if let Some(rest) = below(p, &crate::host::paths::state_path()) {
        return if rest.is_empty() { "<state>".into() } else { format!("<state>/{}", joined(rest)) };
    }
    if let Some(rest) = below(p, &crate::host::home()) {
        return match rest.len() {
            0 => "~".into(),
            1 => tail("~"),
            _ => tail("~/…"),
        };
    }
    tail("…")
}

/// The parts of `p` below `base`, if it is there (case aside on Windows, the verbatim prefix aside).
fn below(p: &Path, base: &Path) -> Option<Vec<String>> {
    let parts = |p: &Path| -> Vec<String> {
        let plain = crate::host::paths::plain(p);
        Path::new(&plain).components().filter(|c| !matches!(c, Component::CurDir)).map(|c| c.as_os_str().to_string_lossy().to_string()).filter(|c| !c.is_empty()).collect()
    };
    let (p, base) = (parts(p), parts(base));
    let same = |a: &String, b: &String| if cfg!(windows) { a.to_lowercase().trim_end_matches('\\') == b.to_lowercase().trim_end_matches('\\') } else { a == b };
    (!base.is_empty() && p.len() >= base.len() && p.iter().zip(&base).all(|(a, b)| same(a, b))).then(|| p[base.len()..].to_vec())
}

/// Any plain text is a `Text` with nothing marked: every `impl Into<String>` an `Error` or a `Refusal`
/// took before takes this. A `String` moves in without a copy.
impl<T: Into<String>> From<T> for Text {
    fn from(local: T) -> Text {
        Text { local: local.into(), spans: Vec::new() }
    }
}

impl From<&Text> for Text {
    fn from(t: &Text) -> Text {
        t.clone()
    }
}

/// What `msg!` takes as an argument: text and numbers, a `Text` with its marks, an error with its own.
/// Neither a `Private` (its public words first: `redacted`, `quoted_or`) nor a path's `display()`
/// (`Text::path`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot go into a message as it is",
    label = "not text strobe may publish as it is",
    note = "a private value goes in through `.redacted(\"…\")` or `.quoted_or(\"…\")` with its public words; a path through `Text::path(p, repo_root)`"
)]
pub trait IntoPiece {
    fn piece(&self) -> Piece<'_>;
}

/// One argument of `msg!`.
#[doc(hidden)]
pub enum Piece<'a> {
    Str(&'a str),
    Shown(&'a dyn fmt::Display),
    Text(&'a Text),
    Owned(Text),
}

impl<T: IntoPiece + ?Sized> IntoPiece for &T {
    fn piece(&self) -> Piece<'_> {
        (**self).piece()
    }
}

impl IntoPiece for str {
    fn piece(&self) -> Piece<'_> {
        Piece::Str(self)
    }
}

impl IntoPiece for String {
    fn piece(&self) -> Piece<'_> {
        Piece::Str(self)
    }
}

impl IntoPiece for Cow<'_, str> {
    fn piece(&self) -> Piece<'_> {
        Piece::Str(self)
    }
}

impl IntoPiece for Text {
    fn piece(&self) -> Piece<'_> {
        Piece::Text(self)
    }
}

macro_rules! shown {
    ($($t:ty),*) => {
        $(impl IntoPiece for $t {
            fn piece(&self) -> Piece<'_> {
                Piece::Shown(self)
            }
        })*
    };
}

shown!(char, bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, std::io::Error, serde_json::Error);

/// `msg!`'s state while it formats: the text so far and the marks its arguments brought.
#[doc(hidden)]
#[derive(Default)]
pub struct Compose {
    local: RefCell<String>,
    spans: RefCell<Vec<Span>>,
}

#[doc(hidden)]
pub struct Arg<'c, 'a> {
    compose: &'c Compose,
    piece: Piece<'a>,
}

struct Sink<'c>(&'c Compose);

impl fmt::Write for Sink<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.local.borrow_mut().push_str(s);
        Ok(())
    }
}

impl Compose {
    pub fn arg<'c, 'a>(&'c self, piece: Piece<'a>) -> Arg<'c, 'a> {
        Arg { compose: self, piece }
    }

    pub fn write(&self, args: fmt::Arguments<'_>) {
        let _ = fmt::write(&mut Sink(self), args);
    }

    pub fn finish(self) -> Text {
        Text { local: self.local.into_inner(), spans: self.spans.into_inner() }
    }
}

/// Plain arguments format as `format!` formats them, width and precision included. A marked `Text` is
/// written whole, whatever its width or precision ask: padding would move its marks and a precision cut
/// one in half.
impl fmt::Display for Arg<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = match &self.piece {
            Piece::Str(s) => return f.pad(s),
            Piece::Shown(d) => return d.fmt(f),
            Piece::Text(t) => *t,
            Piece::Owned(t) => t,
        };
        if t.spans.is_empty() {
            return f.pad(&t.local);
        }
        let base = self.compose.local.borrow().len();
        f.write_str(&t.local)?;
        self.compose.spans.borrow_mut().extend(t.spans.iter().map(|s| Span { at: [s.at[0] + base, s.at[1] + base], ..s.clone() }));
        Ok(())
    }
}

/// `format!` for a `Text`: positional `{}` arguments (`IntoPiece`: text, numbers, a `Text`, an `Error`)
/// keep their marks at the bytes they land on; `{x}` captures a variable as `format!` does. A `Private`
/// and a path's `display()` do not compile here, and an argument takes no `{:?}`: a private value is
/// quoted by `quoted_or`.
///
/// ```ignore
/// msg!("another window covers the point ({x}, {y}): {} of process {pid}", title.quoted_or("a window"))
/// ```
#[macro_export]
macro_rules! msg {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {{
        let compose = $crate::text::Compose::default();
        compose.write(::std::format_args!($fmt $(, compose.arg($crate::text::IntoPiece::piece(&$arg)))*));
        compose.finish()
    }};
}

/// The marks of a struct's text fields (an `Error`'s message, a `Refusal`'s summary), beside the fields
/// themselves so every existing reader of the plain `String` keeps working. Each field's marks are kept
/// with the text they were made for: a field changed since (a prefix added through `..r`, a note
/// appended) finds its private parts again where they now are, and one changed past finding is marked
/// whole, its public form the scrub's (`pr::public`).
#[derive(Clone, Default, PartialEq)]
pub struct Marks(Vec<(String, Text)>);

impl Marks {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keeps `text`'s marks for `field` (none when it has none) and gives its local text for the field.
    pub fn set(&mut self, field: &str, text: Text) -> String {
        self.0.retain(|(f, _)| f != field);
        if !text.is_marked() {
            return text.local;
        }
        let local = text.local.clone();
        self.0.push((field.to_string(), text));
        local
    }

    /// `field`'s text as it now reads (`current`), with its marks.
    pub fn text(&self, field: &str, current: &str) -> Text {
        match self.0.iter().find(|(f, _)| f == field) {
            Some((_, made)) => reanchor(made, current),
            None => Text::from(current),
        }
    }

    /// The marks as result.json keeps them: per field, of its current text; a field with none (or no
    /// longer set) is left out.
    pub fn wire(&self, fields: &[(&str, Option<&str>)]) -> BTreeMap<String, Vec<Span>> {
        fields
            .iter()
            .filter_map(|(f, current)| {
                let t = self.text(f, (*current)?);
                t.is_marked().then(|| (f.to_string(), t.spans))
            })
            .collect()
    }

    /// Marks read back from result.json, for the fields as they were read.
    pub fn from_wire(mut wire: BTreeMap<String, Vec<Span>>, fields: &[(&str, Option<&str>)]) -> Marks {
        let mut marks = Marks::default();
        for (f, current) in fields {
            if let (Some(spans), Some(current)) = (wire.remove(*f), current) {
                marks.set(f, Text::from_parts(current.to_string(), spans));
            }
        }
        marks
    }
}

/// `made`'s marks on `current`: the same text, or one holding it whole, or one holding each private part.
fn reanchor(made: &Text, current: &str) -> Text {
    if made.local == current {
        return made.clone();
    }
    if let Some(at) = current.find(made.local.as_str()) {
        let spans = made.spans.iter().map(|s| Span { at: [s.at[0] + at, s.at[1] + at], ..s.clone() }).collect();
        return Text { local: current.to_string(), spans };
    }
    let mut spans = Vec::new();
    let mut from = 0;
    for s in &made.spans {
        let part = &made.local[s.at[0]..s.at[1]];
        let Some(i) = current[from..].find(part) else {
            let kind = made.spans.first().map_or(PrivKind::Unknown, |s| s.kind);
            return Text::marked(current, kind, crate::app::pr::public(current));
        };
        let start = from + i;
        from = start + part.len();
        spans.push(Span { at: [start, from], ..s.clone() });
    }
    Text { local: current.to_string(), spans }
}

/// Field names and how many marks each holds; the text is the field's own.
impl fmt::Debug for Marks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter().map(|(field, t)| (field, t.spans.len()))).finish()
    }
}
