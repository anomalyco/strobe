//! Messages that know which of their parts are private (SPEC §1 rule 9). The code that composes a message
//! marks what belongs to the machine (another program's name, a covering window's title, a local path, a
//! phone's serial), each part with the words a published surface shows instead. A `Text` renders twice:
//! `local()` is the message as strobe always wrote it (terminal, result.json, query), `public()` puts each
//! marked part's public words in its place (a PR block, a summary, a card). Nothing guesses afterwards.
//!
//! What the compiler guarantees, and what it does not:
//! - A value that enters strobe as a `Private<T>` cannot reach a message unmarked. `Private` has neither
//!   `Display` nor `Debug`, so `format!("{}", p)`, `format!("{p:?}")`, `p.to_string()` and `msg!("{}", p)` do
//!   not compile; it goes into text only through a constructor that takes its public words (`redacted`,
//!   `quoted_or`, `Text::busiest_processes`, …) or through `expose_local()`, whose every use is listed in
//!   `scripts/privacy-check.ts`.
//! - `msg!` takes positional arguments only (`{}`, `{0}`, `{:>6}`); a captured `{x}` or a `{:?}` does not
//!   compile, so an `Error` or a `Text` cannot lose its marks through a capture inside it.
//! - Paths, URLs and other tools' output are NOT protected by the type system: a `Path`, a URL `String` or
//!   a tool's stdout is plain text, and `format!("{}", p.display())` compiles anywhere. They are private
//!   only where the code says them through `Text::path`, `Text::url` and `Text::external_output`; that rests
//!   on those constructors being used, `scripts/privacy-check.ts`, review, and the canary test
//!   (`STROBE_DEV_CANARY`, which tags every marked part so a run can prove no tagged text reaches a public
//!   surface).
//! - Any `String` made with `format!` from marked text (`format!("{e}")` of an `Error`, `t.local()`) has lost
//!   its marks; nothing stops that but review.
//! - Until the `Marks` bridge is retired, reading `r.summary`, `r.detail`, `r.fix`, `r.message()`,
//!   `e.message` or `e.hint` as a `String` drops their marks, and so does a path wrapped in a
//!   `std::io::Error` (`io::Error::other(format!(…, p.display()))`) before it becomes an `Error`.
//! - `STROBE_DEV_CANARY` proves marks are carried: a marked part whose marks were dropped on the way shows
//!   `CANARY_` on a public surface. It does not see a path or a URL no producer marked; only a run from a
//!   canary-named folder and URL (`scripts/dev/public-text.ts`) does.

mod scrub;

pub use scrub::{public_path_in, scrub_url};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::OnceLock;

/// What a field whose marks could not be kept shows in public: none of its text.
pub const REDACTED: &str = "[redacted]";

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

impl PrivKind {
    /// The kind in the canary's tag (`CANARY_WINDOW_TITLE_`).
    fn tag(self) -> &'static str {
        match self {
            PrivKind::ForeignProc => "FOREIGN_PROC",
            PrivKind::WindowTitle => "WINDOW_TITLE",
            PrivKind::ForeignElement => "FOREIGN_ELEMENT",
            PrivKind::HostAncestor => "HOST_ANCESTOR",
            PrivKind::LocalPath => "LOCAL_PATH",
            PrivKind::UserAccount => "USER_ACCOUNT",
            PrivKind::HostName => "HOST_NAME",
            PrivKind::DeviceId => "DEVICE_ID",
            PrivKind::UrlSecret => "URL_SECRET",
            PrivKind::ToolOutput => "TOOL_OUTPUT",
            PrivKind::Unknown => "UNKNOWN",
        }
    }
}

/// A value from this machine that a published surface must not show. It has no `Display` and no `Debug`
/// (`scripts/privacy-check.ts` keeps it so): a message takes it only through `redacted` or `quoted_or`,
/// with the words a public surface shows instead. Serialized as the value itself (result.json is local).
/// Two are equal, ordered and hashed by their values alone: one read back from a file (kind `Unknown`) is
/// the same map key as the live one.
#[derive(Clone)]
pub struct Private<T>(T, PrivKind);

impl<T> Private<T> {
    pub fn new(value: T, kind: PrivKind) -> Self {
        Private(value, kind)
    }

    pub fn kind(&self) -> PrivKind {
        self.1
    }

    /// The value itself, for what never becomes text: an OS call, a wire protocol, a comparison with
    /// another of this machine's names. Every use is listed in `scripts/privacy-check.ts`. The canary
    /// never touches it.
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

    /// Its last `n` characters, still private, of the same kind (a phone's short id from its UDID).
    pub fn tail_chars(&self, n: usize) -> Private<String> {
        let s = self.0.as_ref();
        Private(s.chars().skip(s.chars().count().saturating_sub(n)).collect(), self.1)
    }
}

impl<T: PartialEq> PartialEq for Private<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq> Eq for Private<T> {}

impl<T: PartialOrd> PartialOrd for Private<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<T: Ord> Ord for Private<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: Hash> Hash for Private<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.hash(h)
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
/// `{:?}` shows its public form.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Text {
    local: String,
    spans: Vec<Span>,
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Text").field(&self.public()).finish()
    }
}

/// `STROBE_DEV_CANARY` set: every marked part's local text carries `CANARY_<KIND>_` (a dev check proves no
/// such text reaches a public surface). `Private::expose_local()` is untouched, so OS calls still work.
fn canary() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("STROBE_DEV_CANARY").is_some_and(|v| !v.is_empty() && v != "0"))
}

impl Text {
    pub fn new() -> Text {
        Text::default()
    }

    /// All of `local` marked, `public` in its place on a public surface. Marks inside `local` (a `Text`
    /// built of marked parts) give way to this one. Every constructor that marks goes through here.
    pub fn marked(local: impl Into<Text>, kind: PrivKind, public: impl Into<String>) -> Text {
        let mut local = local.into().local;
        if canary() && !local.is_empty() && kind != PrivKind::Unknown {
            local = format!("CANARY_{}_{local}", kind.tag());
        }
        let end = local.len();
        Text { local, spans: vec![Span { at: [0, end], kind, public: public.into() }] }
    }

    /// `local` with all of it hidden: public shows `REDACTED`, nothing of the text.
    fn hidden(local: &str, kind: PrivKind) -> Text {
        if local.is_empty() {
            return Text::new();
        }
        Text { local: local.to_string(), spans: vec![Span { at: [0, local.len()], kind, public: REDACTED.into() }] }
    }

    /// A text and its marks as a file kept them. Marks that do not fit the text (out of its bounds, off a
    /// character boundary, overlapping) hide all of it.
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
            false => Text::hidden(&local, spans.first().map_or(PrivKind::Unknown, |s| s.kind)),
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

    /// At most `max` characters, the last a `…` when it is cut (nothing for 0). A cut never falls inside a
    /// marked part: one that would be cut ends the text before it, whole parts before it are kept.
    pub fn clip(&self, max: usize) -> Text {
        if max == 0 {
            return Text::new();
        }
        if self.local.chars().count() <= max {
            return self.clone();
        }
        let mut cut = self.local.char_indices().nth(max - 1).map_or(self.local.len(), |(i, _)| i);
        if let Some(s) = self.spans.iter().find(|s| s.at[0] < cut && cut < s.at[1]) {
            cut = s.at[0];
        }
        let mut out = Text { local: self.local[..cut].to_string(), spans: self.spans.iter().filter(|s| s.at[1] <= cut).cloned().collect() };
        out.local.push('…');
        out
    }

    /// Other processes and their share of a core, the list alone (`name 130%, name 14%`); in public `1 other
    /// process, 130% of a core` or `3 other processes, the busiest 130% of a core`. Empty for none. A
    /// sentence with a lead-in (`busiest processes …: `) says it with `busiest_processes`.
    pub fn process_shares(who: &[(Private<String>, f64)]) -> Text {
        if who.is_empty() {
            return Text::new();
        }
        Text::marked(share_list(who), PrivKind::ForeignProc, shares_public(who, ""))
    }

    /// `validity::named_work` with its lead-in: `<lead><name> 421%, <name> 273%[; hypervisor steal …]`,
    /// the lead-in and the list marked together, so public reads `3 other processes on the machine, the
    /// busiest 421% of a core[; hypervisor steal …]` (`on_machine`), never `busiest processes: 3 other
    /// processes, the busiest …`. No process: `<lead>none named (…)`, as named_work says it, unmarked. Steal
    /// from 0.5% of the CPUs' time is named as a cause (`validity::STEAL_CAUSE_PCT`).
    pub fn busiest_processes(lead: &str, on_machine: bool, who: &[(Private<String>, f64)], steal: Option<f64>) -> Text {
        const STEAL_CAUSE_PCT: f64 = 0.5;
        let stolen = steal.filter(|s| *s >= STEAL_CAUSE_PCT).map(|s| format!("hypervisor steal {s:.0}% of those CPUs during the check"));
        if who.is_empty() {
            let none = match (stolen, steal) {
                (Some(s), _) => format!("none named ({s})"),
                (None, Some(s)) => format!("none named (system work; {s:.0}% hypervisor steal)"),
                (None, None) => "none named (system work)".to_string(),
            };
            return Text::from(format!("{lead}{none}"));
        }
        let mut t = Text::marked(format!("{lead}{}", share_list(who)), PrivKind::ForeignProc, shares_public(who, if on_machine { " on the machine" } else { "" }));
        if let Some(s) = stolen {
            t.push_str(&format!("; {s}"));
        }
        t
    }

    /// The processes a CPU reservation moved, as its line names them after `N moved per block`: ` (name ×3,
    /// name, …)`, the four most moved; nothing in public (`3 moved per block`). None moved: ` ()`, as the
    /// line writes it, unmarked.
    pub fn reserve_moved(moved: &BTreeMap<Private<String>, u32>) -> Text {
        if moved.is_empty() {
            return Text::from(" ()");
        }
        let mut top: Vec<(&Private<String>, &u32)> = moved.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let names = top.iter().take(4).map(|(n, c)| if **c > 1 { format!("{} ×{c}", n.0) } else { n.0.clone() }).collect::<Vec<_>>().join(", ");
        Text::marked(format!(" ({names}{})", if top.len() > 4 { ", …" } else { "" }), PrivKind::ForeignProc, "")
    }

    /// The processes a CPU reservation could not move, by why: `2 could not be moved: name, name
    /// (elevated)`; in public the count and the reasons, `2 could not be moved (elevated)`. Empty for none.
    pub fn reserve_failed(failed: &BTreeMap<Private<String>, String>) -> Text {
        if failed.is_empty() {
            return Text::new();
        }
        let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (n, why) in failed {
            by.entry(why.as_str()).or_default().push(n.0.as_str());
        }
        let list =
            by.iter().map(|(why, names)| format!("{} ({why})", names.iter().take(4).copied().collect::<Vec<_>>().join(", ") + if names.len() > 4 { ", …" } else { "" })).collect::<Vec<_>>().join("; ");
        let reasons = by.keys().copied().collect::<Vec<_>>().join("; ");
        let n = failed.len();
        Text::marked(format!("{n} could not be moved: {list}"), PrivKind::ForeignProc, format!("{n} could not be moved ({reasons})"))
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

    /// A phone by its id, where the sentence names it by the id alone (an adb serial, a wireless address:
    /// `{} is at 41 °C`): `the phone` in public, `the emulator` for an emulator. Where the sentence says
    /// what the device is (`{} is an emulator`), say the id with `id.redacted("the device")`; where it
    /// already says `the phone` and adds the id in parentheses, `Text::phone_id`.
    pub fn device(id: &Private<String>, emulator: bool) -> Text {
        id.redacted(if emulator { "the emulator" } else { "the phone" })
    }

    /// A phone's id in parentheses after the words `the phone` (`the phone{} is locked`): local ` (4D5E6F70)`,
    /// nothing in public (`the phone is locked`).
    pub fn phone_id(id: &Private<String>) -> Text {
        Text::marked(format!(" ({})", id.0), id.1, "")
    }

    /// A path as `display()` writes it. In public: from the repository's root (`perf/app.bench.ts`), from
    /// strobe's state folder (`<state>/experiments/…`), under the home folder by its last part
    /// (`~/…/notes.txt`, `~/notes.txt`, or `~` for the home itself), and any other folder by its last part
    /// (`…/v1`). `.` and `..` resolve first, so `repo/../other/x` is `other/x`'s, never the repository's. A
    /// relative path stays, unless it climbs out of where it starts (`../x`: `…/x`).
    pub fn path(p: &Path, repo_root: Option<&Path>) -> Text {
        let local = p.display().to_string();
        let repo = repo_root.map(|r| r.display().to_string());
        let public = scrub::public_path(&local, repo.as_deref());
        if public == local { Text::from(local) } else { Text::marked(local, PrivKind::LocalPath, public) }
    }

    /// A URL; in public without its credentials and secret parameters (`scrub_url`, whose rule is in
    /// `text/scrub.rs`).
    pub fn url(url: &str) -> Text {
        let public = scrub_url(url);
        if public == url { Text::from(url) } else { Text::marked(url, PrivKind::UrlSecret, public) }
    }

    /// What another tool printed (a build's, a fixture's output). In public: without terminal escapes,
    /// each URL scrubbed and each absolute path said as `Text::path` says it, token by token, a token it
    /// cannot read hidden whole (the rule is in `text/scrub.rs`). A name the tool prints in a form of its
    /// own (not a path, not a URL) is not found.
    pub fn external_output(raw: &str, repo_root: Option<&Path>) -> Text {
        let repo = repo_root.map(|r| r.display().to_string());
        let public = scrub::scrub_output(raw, repo.as_deref());
        if public == raw { Text::from(raw) } else { Text::marked(raw, PrivKind::ToolOutput, public) }
    }
}

fn share_list(who: &[(Private<String>, f64)]) -> String {
    who.iter().map(|(n, p)| format!("{} {p:.0}%", n.0)).collect::<Vec<_>>().join(", ")
}

fn shares_public(who: &[(Private<String>, f64)], place: &str) -> String {
    let peak = who.iter().map(|w| w.1).fold(f64::NAN, f64::max);
    match who.len() {
        1 => format!("1 other process{place}, {peak:.0}% of a core"),
        n => format!("{n} other processes{place}, the busiest {peak:.0}% of a core"),
    }
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

shown!(char, bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, std::io::Error, serde_json::Error, std::process::ExitStatus);

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

/// Plain arguments format as `format!` formats them, width and precision included. A `Text` is written
/// whole, whatever its width or precision ask, marked or not: padding would move its marks and a precision
/// cut one in half.
impl fmt::Display for Arg<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let t = match &self.piece {
            Piece::Str(s) => return f.pad(s),
            Piece::Shown(d) => return d.fmt(f),
            Piece::Text(t) => *t,
            Piece::Owned(t) => t,
        };
        let base = self.compose.local.borrow().len();
        f.write_str(&t.local)?;
        self.compose.spans.borrow_mut().extend(t.spans.iter().map(|s| Span { at: [s.at[0] + base, s.at[1] + base], ..s.clone() }));
        Ok(())
    }
}

/// `msg!`'s format string has positional placeholders only (`{}`, `{0}`, `{:>6}`, `{:.1}`): a captured
/// `{x}` or `{x:?}` would format an `Error` or a `Text` through `Display`/`Debug` and drop its marks, so it
/// fails to compile (evaluated in a `const` block). A width or precision from an argument (`{:1$}`,
/// `{:.*}`) fails too: every argument is wrapped to keep its marks, so none is a `usize`.
#[doc(hidden)]
pub const fn positional_only(fmt: &str) {
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' && i + 1 < b.len() && b[i + 1] == b'{' {
            i += 2;
            continue;
        }
        if b[i] == b'{' {
            i += 1;
            if i < b.len() && !matches!(b[i], b'}' | b':' | b'0'..=b'9') {
                panic!("msg!: name no variable inside {{…}}; pass it as a positional argument, so its marks are kept");
            }
            while i < b.len() && b[i] != b'}' {
                if b[i] == b'$' || b[i] == b'*' {
                    panic!("msg!: a width or precision from an argument is not taken; format that value first and pass it positional");
                }
                i += 1;
            }
        }
        i += 1;
    }
}

/// `format!` for a `Text`: positional `{}` arguments (`IntoPiece`: text, numbers, a `Text`, an `Error`)
/// keep their marks at the bytes they land on. A `Private`, a path's `display()`, a captured `{x}` and a
/// `{:?}` do not compile here: a private value is quoted by `quoted_or`.
///
/// ```ignore
/// msg!("another window covers the point ({}, {}): {} of process {}", x, y, title.quoted_or("a window"), pid)
/// ```
#[macro_export]
macro_rules! msg {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {{
        const { $crate::text::positional_only($fmt) };
        let compose = $crate::text::Compose::default();
        compose.write(::std::format_args!($fmt $(, compose.arg($crate::text::IntoPiece::piece(&$arg)))*));
        compose.finish()
    }};
}

/// The marks of a struct's text fields (an `Error`'s message, a `Refusal`'s summary), beside the fields
/// themselves so every existing reader of the plain `String` keeps working. A bridge: the package that
/// converts those readers (bench, validity, stats) makes the fields `Text` and retires it.
///
/// It may hide too much, never too little. A field's marks are kept only while the field holds the text
/// they were made for whole, exactly once, and no marked part occurs anywhere else in the field (a prefix
/// or a suffix added through `..r`, a note appended); any other change (the text cut, edited, reordered,
/// lowercased, replaced, a private word repeated) hides the whole field in public (`REDACTED`).
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

    /// `field`'s text as it now reads (`current`), with its marks. A private part of another field found in
    /// it outside its own marks (fields swapped, combined, copied) hides it whole.
    pub fn text(&self, field: &str, current: &str) -> Text {
        let t = match self.0.iter().find(|(f, _)| f == field) {
            Some((_, made)) => reanchor(made, current),
            None => Text::from(current),
        };
        let mut others = self.0.iter().filter(|(f, _)| f != field).flat_map(|(_, o)| o.spans.iter().map(move |s| &o.local[s.at[0]..s.at[1]])).filter(|p| !p.is_empty());
        let unmarked = |p: &str| {
            (0..=t.local.len().saturating_sub(p.len())).any(|i| t.local.is_char_boundary(i) && t.local[i..].starts_with(p) && !t.spans.iter().any(|s| s.at[0] <= i && i + p.len() <= s.at[1]))
        };
        if others.any(unmarked) { Text::hidden(current, t.spans.first().map_or(PrivKind::Unknown, |s| s.kind)) } else { t }
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

/// Where `needle` starts in `hay`, overlapping occurrences included.
fn occurrences(hay: &str, needle: &str) -> usize {
    (0..=hay.len().saturating_sub(needle.len())).filter(|&i| hay.is_char_boundary(i) && hay[i..].starts_with(needle)).count()
}

/// `made`'s marks on `current` (`Marks`): kept when `current` holds `made`'s text whole exactly once and each
/// marked part occurs in `current` exactly as often as in `made`; else all of `current` hidden.
fn reanchor(made: &Text, current: &str) -> Text {
    if made.local == current {
        return made.clone();
    }
    let kind = made.spans.first().map_or(PrivKind::Unknown, |s| s.kind);
    if made.local.is_empty() || made.spans.iter().any(|s| s.at[0] >= s.at[1]) || occurrences(current, &made.local) != 1 {
        return Text::hidden(current, kind);
    }
    for s in &made.spans {
        let part = &made.local[s.at[0]..s.at[1]];
        if occurrences(current, part) != occurrences(&made.local, part) {
            return Text::hidden(current, kind);
        }
    }
    let at = current.find(made.local.as_str()).unwrap_or(0);
    let spans = made.spans.iter().map(|s| Span { at: [s.at[0] + at, s.at[1] + at], ..s.clone() }).collect();
    Text { local: current.to_string(), spans }
}

/// Field names and how many marks each holds; the text is the field's own.
impl fmt::Debug for Marks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter().map(|(field, t)| (field, t.spans.len()))).finish()
    }
}
