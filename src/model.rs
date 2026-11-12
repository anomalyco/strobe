//! Plain data shared by every module: errors, metrics, verdicts.

use crate::text::{IntoPiece, Marks, Piece, Text};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

/// An error with the exit code it maps to (SPEC §5.2) and an optional fix. A refusal carries what
/// refused it, typed: `refusal` is set when it came from `Error::refused` or a `Refusal` (exit 4), or
/// from `Error::usage_refused`, a question that cannot be asked at all (exit 2: `NotComparable`).
/// `message` and `hint` are the local text; `spans` keeps their private parts (`message_text`).
#[derive(Clone, Debug)]
pub struct Error {
    pub code: i32,
    pub message: String,
    pub hint: Option<String>,
    pub refusal: Option<Box<Refusal>>,
    pub spans: Marks,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: i32, message: impl Into<Text>) -> Self {
        let mut spans = Marks::default();
        let message = spans.set("message", message.into());
        Error { code, message, hint: None, refusal: None, spans }
    }
    pub fn msg(message: impl Into<Text>) -> Self {
        Error::new(1, message)
    }
    pub fn usage(message: impl Into<Text>) -> Self {
        Error::new(2, message)
    }
    /// Something did not happen in time (exit 124).
    pub fn timeout(message: impl Into<Text>) -> Self {
        Error::new(124, message)
    }
    /// A refusal by the validator named `validator` (its SPEC §10.1 id). A name that is no validator
    /// is strobe's own bug: it fails as an internal error instead of refusing under a made-up id.
    pub fn refused(validator: &str, message: impl Into<Text>, hint: impl Into<Text>) -> Self {
        match ValidatorId::from_name(validator) {
            Some(id) => Refusal::new(id, message).fix(hint).into(),
            None => {
                let message: Text = message.into();
                Error::msg(crate::msg!("internal: {} is not a validator: {}", validator, message))
            }
        }
    }
    /// A usage error a validator names (exit 2, SPEC §10.1): what was asked cannot be answered at all,
    /// such as a diff of two implementations (`NotComparable`). Typed like a refusal, so callers read
    /// `validator()` and never the message.
    pub fn usage_refused(r: Refusal) -> Self {
        Error { code: 2, ..Error::from(r) }
    }
    /// A wait for the machine's state that ran out (exit 124), typed by the validator whose state it
    /// waited out (`SessionLocked`, `WindowHidden`): what was not run for it is resumable.
    pub fn timeout_refused(r: Refusal) -> Self {
        Error { code: 124, ..Error::from(r) }
    }
    pub fn unreachable(message: impl Into<Text>) -> Self {
        Error::new(255, message)
    }
    pub fn hint(mut self, hint: impl Into<Text>) -> Self {
        self.hint = Some(self.spans.set("hint", hint.into()));
        self
    }
    /// The message with its private parts marked.
    pub fn message_text(&self) -> Text {
        self.spans.text("message", &self.message)
    }
    pub fn hint_text(&self) -> Option<Text> {
        self.hint.as_deref().map(|h| self.spans.text("hint", h))
    }
    /// `prefix` before the message (`launch 2 of 5: …`), its marks moved with it.
    pub fn prefixed(mut self, prefix: impl Into<Text>) -> Self {
        let mut t: Text = prefix.into();
        t.push(&self.message_text());
        self.message = self.spans.set("message", t);
        self
    }
    /// `suffix` after the message (`… (the block was filmed)`), the message's marks kept.
    pub fn suffixed(mut self, suffix: impl Into<Text>) -> Self {
        let mut t = self.message_text();
        t.push(&suffix.into());
        self.message = self.spans.set("message", t);
        self
    }
    /// `note` after what the error says: a refusal's detail (`<detail>; <note>`, so its validator and fix
    /// stay, the message said again from it), else the message (`<message>; <note>`).
    pub fn append_detail(mut self, note: impl Into<Text>) -> Self {
        let note: Text = note.into();
        match self.refusal.take() {
            Some(r) => {
                let r = r.append_detail(note);
                self.message = self.spans.set("message", r.message_text());
                self.refusal = Some(Box::new(r));
                self
            }
            None => self.suffixed(crate::msg!("; {}", note)),
        }
    }
    /// The validator that refused (an exit-4 error, or a usage error a validator names).
    pub fn validator(&self) -> Option<ValidatorId> {
        self.refusal.as_ref().map(|r| r.validator)
    }
    /// How the window was hidden, for a `WindowHidden` refusal whose producer said.
    pub fn hidden(&self) -> Option<Hidden> {
        self.refusal.as_ref().and_then(|r| r.hidden)
    }
}

macro_rules! validators {
    ($($id:ident),* $(,)?) => {
        /// A validator (SPEC §10.1): what refuses a run, or one metric of it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
        pub enum ValidatorId { $($id),* }

        impl ValidatorId {
            pub const ALL: &[ValidatorId] = &[$(ValidatorId::$id),*];

            /// The id as SPEC, messages and result.json write it (`WindowHidden`).
            pub fn name(self) -> &'static str {
                match self { $(ValidatorId::$id => stringify!($id)),* }
            }
        }
    };
}

validators!(
    SoftwareGpu,
    DevBuild,
    DebuggerAttached,
    WindowHidden,
    WrongScreen,
    UnpacedSpin,
    NoPresentSource,
    AmbiguousInteraction,
    PatternInput,
    SessionLocked,
    ConcurrentBench,
    VariantMismatch,
    RunIncomplete,
    Throttled,
    Background,
    BackgroundPriority,
    PresentsUnmatched,
    OverheadBudget,
    NoDisplayForBudget,
    TailTooShort,
    ClockUnsynced,
    LiveChannel,
    NoPhase,
    NoInput,
    NoSpan,
    NoCounter,
    NotOnThisOs,
    NotForTarget,
    ResetIncomplete,
    EngineUnsupported,
    FixtureMismatch,
    NotComparable,
    ParityGap,
    InputMismatch,
    Headless,
    NoMetric,
    NoWaits,
    NotEquivalent,
    Untimed,
    OutOfView,
    Covered,
    DisplayTooSmall,
    DeviceMismatch,
    Thermal,
    PowerSave,
    LowBattery,
    DeviceAsleep,
    Emulator,
    WirelessTransport,
    NotProfileable,
    MouseNotEnabled,
    AnchorNotFound,
    AnchorAmbiguous,
    SharedRunner,
);

impl ValidatorId {
    pub fn from_name(name: &str) -> Option<ValidatorId> {
        ValidatorId::ALL.iter().copied().find(|v| v.name() == name)
    }

    /// The machine's state at the moment, not the target or the bench: a locked session, a hidden
    /// window (a display off on idle, another app in front), other work on the CPUs, a phone that is
    /// hot or asleep. It passes, so a bench waits it out or runs the block again rather than give up.
    pub fn is_transient_environment(self) -> bool {
        use ValidatorId::*;
        match self {
            SessionLocked | WindowHidden | Background | Thermal | DeviceAsleep => true,
            SoftwareGpu | DevBuild | DebuggerAttached | WrongScreen | UnpacedSpin | NoPresentSource | AmbiguousInteraction | PatternInput | ConcurrentBench | VariantMismatch | RunIncomplete
            | Throttled | BackgroundPriority | PresentsUnmatched | OverheadBudget | NoDisplayForBudget | TailTooShort | ClockUnsynced | LiveChannel | NoPhase | NoInput | NoSpan | NoCounter
            | NotOnThisOs | NotForTarget | ResetIncomplete | EngineUnsupported | FixtureMismatch | NotComparable | ParityGap | InputMismatch | Headless | NoMetric | NoWaits | NotEquivalent
            | Untimed | OutOfView | Covered | DisplayTooSmall | DeviceMismatch | PowerSave | LowBattery | Emulator | WirelessTransport | NotProfileable | MouseNotEnabled | AnchorNotFound
            | AnchorAmbiguous | SharedRunner => false,
        }
    }

    /// The machine or the phone, not the series that met it: every series after it would meet it too,
    /// so a bench stops on it rather than refuse each series in turn (a CPU that runs slow: `Throttled`).
    /// Any other refusal is the series' own: a bench lists it refused and runs the rest.
    pub fn machine_wide(self) -> bool {
        use ValidatorId::*;
        match self {
            SessionLocked | Background | BackgroundPriority | Thermal | DeviceAsleep | ConcurrentBench | PowerSave | LowBattery | Emulator | WirelessTransport | Throttled | SharedRunner => true,
            SoftwareGpu | DevBuild | DebuggerAttached | WindowHidden | WrongScreen | UnpacedSpin | NoPresentSource | AmbiguousInteraction | PatternInput | VariantMismatch | RunIncomplete
            | PresentsUnmatched | OverheadBudget | NoDisplayForBudget | TailTooShort | ClockUnsynced | LiveChannel | NoPhase | NoInput | NoSpan | NoCounter | NotOnThisOs | NotForTarget
            | ResetIncomplete | EngineUnsupported | FixtureMismatch | NotComparable | ParityGap | InputMismatch | Headless | NoMetric | NoWaits | NotEquivalent | Untimed | OutOfView | Covered
            | DisplayTooSmall | DeviceMismatch | NotProfileable | MouseNotEnabled | AnchorNotFound | AnchorAmbiguous => false,
        }
    }
}

/// The browser engine a page target runs in (SPEC §13.1): Chromium over CDP (the default), Firefox
/// over WebDriver BiDi, WebKit over WebDriver Classic (Safari on macOS, WebKitGTK or WPE on Linux; refused
/// on Windows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebEngine {
    #[default]
    Chromium,
    Firefox,
    Webkit,
}

impl WebEngine {
    pub const ALL: [WebEngine; 3] = [WebEngine::Chromium, WebEngine::Firefox, WebEngine::Webkit];

    pub fn name(self) -> &'static str {
        match self {
            WebEngine::Chromium => "chromium",
            WebEngine::Firefox => "firefox",
            WebEngine::Webkit => "webkit",
        }
    }

    /// `chromium`, `firefox` or `webkit` (`--engine`, a target's `engine`); anything else is a usage error.
    pub fn parse(s: &str) -> Result<WebEngine> {
        WebEngine::ALL.into_iter().find(|e| e.name() == s).ok_or_else(|| Error::usage(format!("engine: chromium, firefox or webkit, not {s:?}")))
    }
}

impl fmt::Display for WebEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for ValidatorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a window counted as hidden (`WindowHidden`), where a bench acts on the difference: on macOS
/// it waits for a person to leave the Mac when the window is covered or the app is not the active
/// one, and otherwise wakes the display once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hidden {
    /// Other apps' windows cover it (macOS).
    Covered,
    /// Its app is not the active app, where strobe measures it active (macOS).
    NotActive,
    /// Closed, hidden, minimized, cloaked, or a page that is not visible.
    Other,
}

/// Why a validator refused: a run (in an `Error`) or one metric (in a `Comparison`). `summary`,
/// `detail` and `fix` are the local text; `spans` keeps their private parts, which result.json writes
/// beside them (`"spans": { "summary": [...] }`) when there are any.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(from = "RefusalRead")]
pub struct Refusal {
    pub validator: ValidatorId,
    /// What was wrong.
    pub summary: String,
    /// What strobe saw beyond the summary, such as the busiest processes.
    pub detail: Option<String>,
    /// How to make the run valid.
    pub fix: Option<String>,
    /// `WindowHidden` only, when the producer knows: how the window was hidden.
    pub hidden: Option<Hidden>,
    pub spans: Marks,
}

/// A `Refusal` as result.json writes it.
#[derive(Serialize)]
struct RefusalWrite<'a> {
    validator: ValidatorId,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hidden: Option<Hidden>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    spans: BTreeMap<String, Vec<crate::text::Span>>,
}

/// A `Refusal` as result.json holds it (`spans` absent in older results).
#[derive(serde::Deserialize)]
struct RefusalRead {
    validator: ValidatorId,
    summary: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    fix: Option<String>,
    #[serde(default)]
    hidden: Option<Hidden>,
    #[serde(default)]
    spans: BTreeMap<String, Vec<crate::text::Span>>,
}

impl From<RefusalRead> for Refusal {
    fn from(r: RefusalRead) -> Self {
        let spans = Marks::from_wire(r.spans, &[("summary", Some(&r.summary)), ("detail", r.detail.as_deref()), ("fix", r.fix.as_deref())]);
        Refusal { validator: r.validator, summary: r.summary, detail: r.detail, fix: r.fix, hidden: r.hidden, spans }
    }
}

/// Two refusals are the same when they say the same with the same marks, however their marks were kept
/// (set in another order, carried through `..r`, read back from result.json).
impl PartialEq for Refusal {
    fn eq(&self, o: &Self) -> bool {
        self.validator == o.validator && self.hidden == o.hidden && self.summary_text() == o.summary_text() && self.detail_text() == o.detail_text() && self.fix_text() == o.fix_text()
    }
}

impl Serialize for Refusal {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let (detail, fix) = (self.detail.as_deref(), self.fix.as_deref());
        let spans = self.spans.wire(&[("summary", Some(&self.summary)), ("detail", detail), ("fix", fix)]);
        RefusalWrite { validator: self.validator, summary: &self.summary, detail, fix, hidden: self.hidden, spans }.serialize(s)
    }
}

impl Refusal {
    pub fn new(validator: ValidatorId, summary: impl Into<Text>) -> Self {
        let mut spans = Marks::default();
        let summary = spans.set("summary", summary.into());
        Refusal { validator, summary, detail: None, fix: None, hidden: None, spans }
    }
    /// A `WindowHidden` refusal that says how the window was hidden.
    pub fn window_hidden(hidden: Hidden, summary: impl Into<Text>) -> Self {
        Refusal { hidden: Some(hidden), ..Refusal::new(ValidatorId::WindowHidden, summary) }
    }
    /// A metric's refusal as a block's `refused` map writes it, `<validator>: <why>`, read back.
    /// A line whose prefix is no validator is `None`.
    pub fn from_line(line: &str) -> Option<Refusal> {
        let (id, why) = line.split_once(": ").unwrap_or((line, ""));
        Some(Refusal::new(ValidatorId::from_name(id)?, why))
    }
    pub fn detail(mut self, detail: impl Into<Text>) -> Self {
        self.detail = Some(self.spans.set("detail", detail.into()));
        self
    }
    pub fn fix(mut self, fix: impl Into<Text>) -> Self {
        self.fix = Some(self.spans.set("fix", fix.into()));
        self
    }
    /// `prefix` before the summary (`<label>: …`, `on head: …`), its marks moved with it.
    pub fn prefixed(mut self, prefix: impl Into<Text>) -> Self {
        let mut t: Text = prefix.into();
        t.push(&self.summary_text());
        self.summary = self.spans.set("summary", t);
        self
    }
    /// `suffix` after the summary (`… (the block was filmed: …)`), the summary's marks kept.
    pub fn suffixed(mut self, suffix: impl Into<Text>) -> Self {
        let mut t = self.summary_text();
        t.push(&suffix.into());
        self.summary = self.spans.set("summary", t);
        self
    }
    /// `note` after the detail (`<detail>; <note>`), or as the detail when there is none.
    pub fn append_detail(mut self, note: impl Into<Text>) -> Self {
        let note: Text = note.into();
        let detail = match self.detail_text() {
            Some(d) => crate::msg!("{}; {}", d, note),
            None => note,
        };
        self.detail = Some(self.spans.set("detail", detail));
        self
    }
    pub fn summary_text(&self) -> Text {
        self.spans.text("summary", &self.summary)
    }
    pub fn detail_text(&self) -> Option<Text> {
        self.detail.as_deref().map(|d| self.spans.text("detail", d))
    }
    pub fn fix_text(&self) -> Option<Text> {
        self.fix.as_deref().map(|f| self.spans.text("fix", f))
    }
    /// `<validator>: <summary>[; <detail>]`, as an error's message says it.
    pub fn message(&self) -> String {
        self.message_text().into_local()
    }
    /// `message` with its private parts marked.
    pub fn message_text(&self) -> Text {
        match self.detail_text() {
            Some(d) => crate::msg!("{}: {}; {}", self.validator, self.summary_text(), d),
            None => crate::msg!("{}: {}", self.validator, self.summary_text()),
        }
    }
}

impl From<Refusal> for Error {
    fn from(r: Refusal) -> Self {
        let mut spans = Marks::default();
        let message = spans.set("message", r.message_text());
        let hint = r.fix_text().map(|f| spans.set("hint", f));
        Error { code: 4, message, hint, refusal: Some(Box::new(r)), spans }
    }
}

/// An error goes into a message with its marks.
impl IntoPiece for Error {
    fn piece(&self) -> Piece<'_> {
        Piece::Owned(self.message_text())
    }
}

impl IntoPiece for Refusal {
    fn piece(&self) -> Piece<'_> {
        Piece::Owned(self.message_text())
    }
}

impl IntoPiece for ValidatorId {
    fn piece(&self) -> Piece<'_> {
        Piece::Shown(self)
    }
}

impl IntoPiece for WebEngine {
    fn piece(&self) -> Piece<'_> {
        Piece::Shown(self)
    }
}

/// `#[serde(with = "refusal_lines")]` for a metric → `Refusal` map that result.json keeps as
/// `"<metric>": "<validator>: <why>"`, as blocks have always written it (SPEC 10.1).
pub mod refusal_lines {
    use super::Refusal;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(map: &BTreeMap<String, Refusal>, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_map(map.iter().map(|(k, r)| (k, r.message())))
    }

    /// Lines whose prefix is no validator (older results) are dropped.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<BTreeMap<String, Refusal>, D::Error> {
        let lines = BTreeMap::<String, String>::deserialize(d)?;
        Ok(lines.into_iter().filter_map(|(k, l)| Some((k, Refusal::from_line(&l)?))).collect())
    }

    /// The marks of each line `serialize` writes, by metric, for result.json's `refused_spans` beside the
    /// map; metrics whose line has none are left out.
    pub fn spans(map: &BTreeMap<String, Refusal>) -> BTreeMap<String, Vec<crate::text::Span>> {
        map.iter()
            .filter_map(|(k, r)| {
                let t = r.message_text();
                t.is_marked().then(|| (k.clone(), t.spans().to_vec()))
            })
            .collect()
    }
}

/// What a comparison decided (SPEC §9.3), whatever words its verdict uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    /// Head is better: `faster`, `less`, `fewer`, `smoother`, … (small or exact included).
    Better,
    /// Head is worse: `slower`, `more`, …, or `worse (>X)` against a no-worse margin.
    Worse,
    /// Inside the equivalence bound: `same (±2%)`, `same (exact)`.
    Same,
    /// Inside a no-worse margin: `no worse (≤5%)`.
    NoWorse,
    Unclear,
    /// More pairs would decide it: `underpowered: ~n pairs needed`.
    Underpowered,
    /// Both sides sit at the metric's ceiling, where builds cannot differ.
    Saturated,
    /// The bench was stopped before a verdict.
    Stopped,
    /// A validator refused the metric: the comparison's `refusal` says which.
    Refused,
    /// A secondary the gate did not test: the primary was not decided, or Holm kept it (`—`).
    Untested,
    /// A descriptive metric: shown, never decided (`·`).
    Descriptive,
}

impl VerdictKind {
    pub fn decided(self) -> bool {
        matches!(self, VerdictKind::Better | VerdictKind::Worse | VerdictKind::Same | VerdictKind::NoWorse)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::msg(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::msg(format!("json: {e}"))
    }
}

#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::model::Error::msg(format!($($t)*))) };
}

/// Which direction is better for a metric.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Better {
    Lower,
    Higher,
}

/// How a metric's values behave (SPEC §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Timed,
    Sampled,
    Exact,
}

/// One entry of the metric catalog.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct MetricDef {
    pub name: &'static str,
    pub unit: &'static str,
    pub better: Better,
    pub kind: Kind,
    /// Words for the better and worse directions, e.g. ("faster", "slower").
    #[serde(skip)]
    pub words: (&'static str, &'static str),
    /// Frames needed per block, for percentiles; 0 when not a percentile.
    pub min_n: usize,
    /// Whether it compares between two implementations (SPEC 7.1, 9.10).
    #[serde(skip)]
    pub across: Across,
    /// The instrument it is read with: what its verified or preview path is of (SPEC §21.1).
    #[serde(skip)]
    pub capability: crate::support::Capability,
    pub about: &'static str,
}

/// How a metric compares when base and head are different implementations (SPEC 9.10). Between
/// two sides of one engine (two pages, two native apps) every metric compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Across {
    /// The same instrument reads the same quantity on any implementation.
    Compared,
    /// Comparable only as a total: the note says what it sums on each side, wherever the number is.
    Care(&'static str),
    /// Engine-internal, or a thread whose work differs by toolkit: shown per side, never compared.
    PerSide,
}

impl Across {
    /// As result.json writes it.
    pub fn name(self) -> &'static str {
        match self {
            Across::Compared => "compared",
            Across::Care(_) => "care",
            Across::PerSide => "per_side",
        }
    }

    pub fn note(self) -> Option<&'static str> {
        match self {
            Across::Care(n) => Some(n),
            Across::Compared | Across::PerSide => None,
        }
    }
}

const CARE_CPU: &str = "every process of each app's process set, per run; the fixture not included";
const CARE_CPU_SHARE: &str = "every process of each app, as a share of one core over the run: compare it only where run.duration is the same";
const CARE_WAKES: &str = "all threads of all processes of each app: more threads and processes wake more by design";
const CARE_PRIVATE: &str = "the OS's private memory of every process of each app (Windows commit, macOS footprint, Linux private pages); memory shared between an app's processes is in neither on Windows and Linux, while macOS counts shared anonymous memory once, in the process that made it";
const CARE_HOST: &str = "what every process of each app allocated in its own address space, without GPU memory";
const CARE_GPU_MEMORY: &str = "GPU memory of every process of each app";
const CARE_FRAMES: &str = "descriptive: more frames can mean smoother or wasteful";
const CARE_LAUNCH: &str = "the OS's work creating each command's process, which grows with its executable and what loads with it, not with what it then does";
const CARE_PEAK: &str = "the largest of each command's processes at its peak (a resident set on Linux and macOS, a commit on Windows), not their sum";

const TIME: (&str, &str) = ("faster", "slower");
const COUNT: (&str, &str) = ("fewer", "more");
const AMOUNT: (&str, &str) = ("less", "more");
const SMOOTH: (&str, &str) = ("smoother", "choppier");

use crate::support::Capability;

pub const METRICS: &[MetricDef] = &[
    MetricDef {
        name: "frames.presented.p50",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::PerSide,
        about: "median frame latency (begin of frame to presentation) of presented frames",
    },
    MetricDef {
        name: "frames.presented.p90",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 100,
        across: Across::PerSide,
        about: "90th percentile frame cost",
    },
    MetricDef {
        name: "frames.presented.p95",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 200,
        across: Across::PerSide,
        about: "95th percentile frame cost",
    },
    MetricDef {
        name: "frames.presented.p99",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1000,
        across: Across::PerSide,
        about: "99th percentile frame cost",
    },
    MetricDef {
        name: "frames.presented.max",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::PerSide,
        about: "worst presented frame cost",
    },
    MetricDef {
        name: "frames.presented.count",
        capability: Capability::Frames,
        unit: "count",
        better: Better::Higher,
        kind: Kind::Timed,
        words: ("more", "fewer"),
        min_n: 0,
        across: Across::Care(CARE_FRAMES),
        about: "presented frames per run",
    },
    MetricDef {
        name: "canvas.frames",
        capability: Capability::Canvas,
        unit: "count",
        better: Better::Higher,
        kind: Kind::Timed,
        words: ("more", "fewer"),
        min_n: 0,
        across: Across::PerSide,
        about: "frames the page drew into a WebGPU canvas, per run",
    },
    MetricDef {
        name: "canvas.interval.p50",
        capability: Capability::Canvas,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 20,
        across: Across::PerSide,
        about: "median gap between the page's own canvas frames",
    },
    MetricDef {
        name: "canvas.interval.p95",
        capability: Capability::Canvas,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 200,
        across: Across::PerSide,
        about: "95th percentile gap between the page's own canvas frames (its stutter, at its own frame rate)",
    },
    MetricDef {
        name: "canvas.interval.p99",
        capability: Capability::Canvas,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 1000,
        across: Across::PerSide,
        about: "99th percentile gap between the page's own canvas frames",
    },
    MetricDef {
        name: "canvas.interval.max",
        capability: Capability::Canvas,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 1,
        across: Across::PerSide,
        about: "longest gap between the page's own canvas frames",
    },
    MetricDef {
        name: "frames.interval.p50",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 20,
        across: Across::Compared,
        about: "median gap between presented frames while the page wanted updates",
    },
    MetricDef {
        name: "frames.interval.p95",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 200,
        across: Across::Compared,
        about: "95th percentile gap between presented frames (stutter)",
    },
    MetricDef {
        name: "frames.interval.p99",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 1000,
        across: Across::Compared,
        about: "99th percentile gap between presented frames",
    },
    MetricDef {
        name: "frames.interval.max",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 1,
        across: Across::Compared,
        about: "longest gap between presented frames",
    },
    MetricDef {
        name: "frames.dropped.pct",
        capability: Capability::Frames,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 0,
        across: Across::Compared,
        about: "share of vsyncs with pending work that presented nothing",
    },
    MetricDef {
        name: "frames.dropped",
        capability: Capability::Frames,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "vsyncs Chromium reported as dropped, per run; native and outside-in: missed refreshes while drawing plus discarded presents (OS present events)",
    },
    MetricDef {
        name: "frames.discarded",
        capability: Capability::Frames,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Care(CARE_FRAMES),
        about: "presents the OS replaced before they reached the screen, per run",
    },
    MetricDef {
        name: "frames.partial",
        capability: Capability::Frames,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "frames presented without all updates, per run",
    },
    MetricDef {
        name: "main.busy",
        capability: Capability::PageEngine,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "renderer main thread busy share during runs",
    },
    MetricDef {
        name: "frames.renderer.p95",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 200,
        across: Across::PerSide,
        about: "95th percentile of a presented frame's renderer stages (pages): the compositor thread begins the frame, the main thread runs and commits, the compositor activates and submits; stage spans, queueing included, not busy time",
    },
    MetricDef {
        name: "frames.viz.p95",
        capability: Capability::Frames,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 200,
        across: Across::PerSide,
        about: "95th percentile of a presented frame's GPU-process stages (pages): commit to activation (tiles raster) and the display compositor's draw to its swap; stage spans, not GPU busy time, and the waits for the display are in neither side",
    },
    MetricDef {
        name: "main.work",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "renderer main-thread task time per run: the work an interaction costs, not quantized to frames like its duration",
    },
    MetricDef {
        name: "main.script",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "JavaScript time on the renderer main thread per run",
    },
    MetricDef {
        name: "main.gc",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "garbage collection on the renderer main thread per run (V8's top-level GC slices); traced only for series that declare it",
    },
    MetricDef {
        name: "main.layout",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "layout time on the renderer main thread per run",
    },
    MetricDef {
        name: "main.style",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "style recalculation time on the renderer main thread per run",
    },
    MetricDef {
        name: "main.paint",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "prepaint and paint time on the renderer main thread per run",
    },
    MetricDef {
        name: "layout.count",
        capability: Capability::PageEngine,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "layouts per run",
    },
    MetricDef {
        name: "style.count",
        capability: Capability::PageEngine,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "style recalculations per run",
    },
    MetricDef {
        name: "main.tasks.long",
        capability: Capability::PageEngine,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "main-thread tasks over 50 ms, per run",
    },
    MetricDef {
        name: "interaction.p50",
        capability: Capability::InputLatency,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::Compared,
        about: "input to presentation, median of discrete inputs (native apps: SPEC §7.4)",
    },
    MetricDef {
        name: "interaction.p95",
        capability: Capability::InputLatency,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::Compared,
        about: "input to presentation, 95th percentile of discrete inputs",
    },
    MetricDef {
        name: "input.rate",
        capability: Capability::WallAndCounts,
        unit: "/s",
        better: Better::Higher,
        kind: Kind::Timed,
        words: ("more", "fewer"),
        min_n: 0,
        across: Across::Compared,
        about: "pointer moves strobe sent per second while a button was held (page.drag, moves after page.down(\"Mouse\")): a driver that starved shows here, not as dropped frames",
    },
    MetricDef {
        name: "input.gap.p95",
        capability: Capability::WallAndCounts,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: SMOOTH,
        min_n: 0,
        across: Across::Compared,
        about: "95th percentile gap between consecutive pointer moves of a drag, as strobe sent them",
    },
    MetricDef {
        name: "driver.reads",
        capability: Capability::WallAndCounts,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "strobe's looks at the page or the accessibility tree inside a run (an input's aim, an until or waitFor selector; never idle or frame): what driving the app cost it (ObserverCost)",
    },
    MetricDef {
        name: "driver.read_ms",
        capability: Capability::WallAndCounts,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "what strobe's driver.reads inside a run cost: a page's main-thread time in its element lookups, a native app's tree reads (ObserverCost)",
    },
    MetricDef {
        name: "driver.pixel_reads",
        capability: Capability::Pixels,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "strobe's captures of the target's pixels inside a run (an image locator's aim, an until or waitFor of an image or of pixels): each a read of the window from outside (ObserverCost)",
    },
    MetricDef {
        name: "driver.pixel_ms",
        capability: Capability::Pixels,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "the wall time of strobe's driver.pixel_reads inside a run: capture, copy and matching (ObserverCost)",
    },
    MetricDef {
        name: "driver.pixel_cpu_ms",
        capability: Capability::Pixels,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "strobe's CPU in its driver.pixel_reads inside a run (capture, copy and matching): their absolute cost, where overhead.pixel_pct is its share of the run's wall time (ObserverCost)",
    },
    MetricDef {
        name: "scroll.latency.p50",
        capability: Capability::InputLatency,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::Compared,
        about: "scroll update input to presentation, median (native apps: the first wheel step of each scroll)",
    },
    MetricDef {
        name: "scroll.latency.p95",
        capability: Capability::InputLatency,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::Compared,
        about: "scroll update input to presentation, 95th percentile",
    },
    MetricDef {
        name: "tty.latency.p50",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::Compared,
        about: "terminal targets: an input written to the terminal (a key, a click, a wheel notch, a resize) to the first change of the grid the program drew after it, median",
    },
    MetricDef {
        name: "tty.latency.p95",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::Compared,
        about: "terminal targets: input to the first grid change it caused, 95th percentile",
    },
    MetricDef {
        name: "tty.settle.p50",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::Compared,
        about: "terminal targets: an input to the last grid change it set off, once the grid then held still (50 ms) before the next input, median",
    },
    MetricDef {
        name: "tty.settle.p95",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::Compared,
        about: "terminal targets: input to the last grid change it set off, 95th percentile",
    },
    MetricDef {
        name: "tty.updates",
        capability: Capability::TtyGrid,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "terminal targets: changes of the grid per run (each read of the program's output that changed cells, and each synchronized frame)",
    },
    MetricDef {
        name: "tty.frames",
        capability: Capability::TtyGrid,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Exact,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "terminal targets: synchronized frames per run (the program's `?2026` begin and end around a redraw)",
    },
    MetricDef {
        name: "tty.frame.p50",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::PerSide,
        about: "terminal targets: a synchronized frame from the read holding its begin to the read holding its end, median",
    },
    MetricDef {
        name: "tty.frame.p95",
        capability: Capability::TtyGrid,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::PerSide,
        about: "terminal targets: a synchronized frame's begin to its end, 95th percentile",
    },
    MetricDef {
        name: "tty.bytes",
        capability: Capability::TtyGrid,
        unit: "bytes",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "terminal targets: bytes the terminal received per run: what the program wrote (Unix), what ConPTY's console host sent for it after drawing its console again (Windows)",
    },
    MetricDef {
        name: "tty.cells",
        capability: Capability::TtyGrid,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "terminal targets: cells (character or attributes) the grid's changes changed per run",
    },
    MetricDef {
        name: "cpu.pty_host",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "terminal targets on Windows: CPU of the console host ConPTY starts for the terminal, per run: the terminal's work, never in cpu.work",
    },
    MetricDef {
        name: "run.duration",
        capability: Capability::WallAndCounts,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "wall time of one run",
    },
    MetricDef {
        name: "cpu.app.total",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_CPU_SHARE),
        about: "CPU of every process of the target, % of one core, during runs",
    },
    MetricDef {
        name: "cpu.foreign",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "other processes' work on the CPUs the target is pinned to during runs, % of one core: a validity signal, not the target's cost",
    },
    MetricDef {
        name: "cpu.process.total",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_CPU_SHARE),
        about: "alias of cpu.app.total",
    },
    MetricDef {
        name: "cpu.renderer",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "CPU of the browser's renderer processes, % of one core, during runs",
    },
    MetricDef {
        name: "cpu.gpu",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "CPU of the browser's GPU process (compositing, GPU commands), % of one core, during runs",
    },
    MetricDef {
        name: "cpu.browser",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "CPU of the browser process itself, % of one core, during runs",
    },
    MetricDef {
        name: "memory.app.private",
        capability: Capability::ProcessMemory,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_PRIVATE),
        about: "private bytes of every target process at the end of the last measured run, before its reset (Windows: the commit charge, which includes commit held for the processes' GPU allocations; Linux: anonymous resident and swapped pages; macOS: the physical footprint, which also holds the dirty pages of shared anonymous memory the process created, and is refused for a Bun script host, whose allocator reuses pages the footprint no longer counts)",
    },
    MetricDef {
        name: "memory.app.host",
        capability: Capability::ProcessMemory,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_HOST),
        about: "native targets: memory the processes allocated in their own address space at the end of the last measured run, before its reset (heap, stacks, anonymous memory), without GPU memory mapped into them or held for them (Windows, Linux)",
    },
    MetricDef {
        name: "memory.app.pss",
        capability: Capability::ProcessMemory,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "Linux: the proportional set size of every target process at the end of the last measured run, before its reset (each page shared by n processes counts 1/n), summed: memory an app's processes share counts once",
    },
    MetricDef {
        name: "cpu.fixture",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "CPU time of the bench file's fixture (a server both sides talk to, outside their process sets) per run: the work each side made it do",
    },
    MetricDef {
        name: "gpu.compile",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::PerSide,
        about: "time the browser's GPU process spent creating pipelines and compiling shaders per run (Dawn), on any of its threads",
    },
    MetricDef {
        name: "gpu.process.3d",
        capability: Capability::Gpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "GPU 3D engine use by the target's processes during runs, summed over every adapter they used (% of one engine)",
    },
    MetricDef {
        name: "gpu.vram",
        capability: Capability::Gpu,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_GPU_MEMORY),
        about: "dedicated VRAM of the target's processes after the measured runs",
    },
    MetricDef {
        name: "gpu.shared",
        capability: Capability::Gpu,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_GPU_MEMORY),
        about: "GPU memory of the target's processes in system memory after the measured runs: all of an integrated GPU's, and a discrete one's staging",
    },
    MetricDef {
        name: "js.heap.used",
        capability: Capability::PageEngine,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "renderer JS heap in use at the end of each measured run, before its reset (median over the runs)",
    },
    MetricDef {
        name: "dom.nodes",
        capability: Capability::PageEngine,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "DOM nodes at the end of each measured run, before its reset (median over the runs)",
    },
    MetricDef {
        name: "js.heap.growth",
        capability: Capability::PageEngine,
        unit: "MB/min",
        better: Better::Lower,
        kind: Kind::Sampled,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "post-GC JS heap growth per minute over a memory recording",
    },
    MetricDef {
        name: "dom.nodes.growth",
        capability: Capability::PageEngine,
        unit: "count/min",
        better: Better::Lower,
        kind: Kind::Sampled,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "DOM node growth per minute over a memory recording",
    },
    MetricDef {
        name: "leak.suspects.retained",
        capability: Capability::PageEngine,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Sampled,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "memory retained by objects allocated in a memory recording's first half and still alive at its end",
    },
    MetricDef {
        name: "cpu.work",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_CPU),
        about: "CPU time of the target's processes per run (all threads): a script target's main.work, a launch's total",
    },
    MetricDef {
        name: "cpu.main",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "script and native targets, and record pid:: CPU time per run of the main thread (the thread the target's process started with), from that thread's own counter read at the run's edges and each untimed stretch's: cpu.main.busy times the run's timed time",
    },
    MetricDef {
        name: "cpu.main.busy",
        capability: Capability::ProcessCpu,
        unit: "%",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "the share of the run's timed time the main thread was on a CPU (100% is an event loop that never waited)",
    },
    MetricDef {
        name: "ui.cpu",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "native apps: CPU time per run of the thread that owns the main window (the UI thread), from its cycle counter: the UI work an interaction costs, from outside",
    },
    MetricDef {
        name: "ui.wakes",
        capability: Capability::ProcessCpu,
        unit: "/s",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "native apps: context switches of the UI thread (the main window's) per second during runs",
    },
    MetricDef {
        name: "cpu.wakes",
        capability: Capability::ProcessCpu,
        unit: "/s",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Care(CARE_WAKES),
        about: "context switches of the target's threads per second during runs: each time one of its threads was switched in, after a wait (a timer, an event, IO) or a preemption; most are waits at rest. Driver and OS pool threads inside its processes count too (Windows; `query <rec> threads` names the threads and the module each started in)",
    },
    MetricDef {
        name: "energy.cpu",
        capability: Capability::HardwareCounters,
        unit: "mJ",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "energy of the CPU cores (performance and efficiency, user and kernel mode) the target's processes used per run, as the OS measures it per process (macOS on Apple silicon, from the cores' energy counters); not the GPU, Neural Engine, display or IO; refused where no OS measures it per process (Linux, Windows, Intel Macs), never estimated from CPU time",
    },
    MetricDef {
        name: "cpu.instructions",
        capability: Capability::HardwareCounters,
        unit: "instructions",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "instructions the target's processes retired per run, from the CPU's counters (macOS: user and kernel mode, always; Linux: user mode, perf events in counter blocks of their own, never in a timing block); tighter run to run than CPU time, which moves with frequency and caches",
    },
    MetricDef {
        name: "cpu.idle_wakes",
        capability: Capability::PackageIdle,
        unit: "/s",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "times per second the target's processes woke the CPU package from idle during runs (macOS: what Activity Monitor's Energy Impact weighs); refused where the OS does not count them per process",
    },
    MetricDef {
        name: "cli.launch",
        capability: Capability::ProcessCpu,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_LAUNCH),
        about: "cli targets: CPU time of the OS creating each command's process per run, on strobe's launching thread (Windows does most of it in the launcher, often several ms; elsewhere the fork, with the exec in the child), from its cycle counter; in run.duration, not in cpu.work",
    },
    MetricDef {
        name: "memory.peak",
        capability: Capability::ProcessMemory,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Care(CARE_PEAK),
        about: "cli targets: the most memory one of the command's processes held at once over a run (Linux and macOS its peak resident set, `ru_maxrss`; Windows its peak commit, the job's `PeakProcessMemoryUsed`)",
    },
    MetricDef {
        name: "metric",
        capability: Capability::WallAndCounts,
        unit: "value",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "`metric.<name>`: what a cli target's command printed as `strobe:metric <name>=<value> [lower|higher]` lines, summed per run; the command's own number, not strobe's measurement, its direction the one the lines give (lower by default)",
    },
    MetricDef {
        name: "io.read",
        capability: Capability::ProcessCpu,
        unit: "bytes",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "bytes the target's processes read per run through read calls (files, devices, pipes); not counted: a file mapped into memory and read through its pages (Windows, Linux) and, on Windows, console input; macOS counts disk bytes only",
    },
    MetricDef {
        name: "io.write",
        capability: Capability::ProcessCpu,
        unit: "bytes",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::Compared,
        about: "bytes the target's processes wrote per run through write calls (files, devices, pipes); not counted: a file mapped into memory and written through its pages (Windows, Linux) and, on Windows, console output; macOS counts disk bytes only",
    },
    MetricDef {
        name: "js.heap.retained",
        capability: Capability::PageEngine,
        unit: "MB",
        better: Better::Lower,
        kind: Kind::Timed,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "script targets: the JS heap a run leaves behind, after a full collection before and after it, ArrayBuffer and Buffer contents included (Node keeps them off its heap; Bun counts them in it)",
    },
    MetricDef {
        name: "http.latency.p50",
        capability: Capability::HttpProbe,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 10,
        across: Across::Compared,
        about: "the HTTP probe (ctx.httpProbe): from a probe's dispatch to its whole response, median over the probes due in the measured runs' timed parts; a timeout ranks above every answer, and a quantile that falls on one is refused (right-censored), never read as the timeout",
    },
    MetricDef {
        name: "http.latency.p95",
        capability: Capability::HttpProbe,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 20,
        across: Across::Compared,
        about: "the HTTP probe: dispatch to whole response, 95th percentile of the probes due in the measured runs' timed parts (censored at the timeout as p50)",
    },
    MetricDef {
        name: "http.latency.max",
        capability: Capability::HttpProbe,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 1,
        across: Across::Compared,
        about: "the HTTP probe: the slowest answer of the probes due in the measured runs' timed parts; refused when one timed out",
    },
    MetricDef {
        name: "http.timeouts",
        capability: Capability::HttpProbe,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "the HTTP probe: probes per measured run with no whole response within timeoutMs of their dispatch",
    },
    MetricDef {
        name: "http.errors",
        capability: Capability::HttpProbe,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "the HTTP probe: probes per measured run the server answered not as expect asks (a 500, a body without the text) or whose connection failed; any refuses http.latency.*, since a fast error is no fast answer",
    },
    MetricDef {
        name: "http.missed_sends",
        capability: Capability::HttpProbe,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Timed,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "the HTTP probe: probes per measured run strobe did not send on time (its scheduler or a worker ran late, or every connection it may hold was open): strobe's doing, never the server's; more than max(1, 5%) of those due refuses http.latency.*",
    },
    MetricDef {
        name: "http.probes",
        capability: Capability::HttpProbe,
        unit: "count",
        better: Better::Higher,
        kind: Kind::Timed,
        words: ("more", "fewer"),
        min_n: 0,
        across: Across::Compared,
        about: "the HTTP probe: probes due per measured run's timed part, sent or not (its timed time over the cadence): context for the counts",
    },
    MetricDef {
        name: "http.idle.p50",
        capability: Capability::HttpProbe,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "the HTTP probe's idle control: median latency of the probes after setup, before any run, with nothing else running: the server's own floor",
    },
    MetricDef {
        name: "layout.shift",
        capability: Capability::PageEngine,
        unit: "score",
        better: Better::Lower,
        kind: Kind::Timed,
        words: ("steadier", "jumpier"),
        min_n: 0,
        across: Across::PerSide,
        about: "summed layout-shift score per run, shifts right after the run's input included",
    },
    MetricDef {
        name: "net.requests",
        capability: Capability::PageEngine,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Exact,
        words: COUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "requests the page started per run; `net.requests{<url glob>}` counts the matching ones",
    },
    MetricDef {
        name: "net.bytes",
        capability: Capability::PageEngine,
        unit: "bytes",
        better: Better::Lower,
        kind: Kind::Exact,
        words: AMOUNT,
        min_n: 0,
        across: Across::PerSide,
        about: "bytes received over the network per run (as encoded on the wire); `net.bytes{<url glob>}` for matching requests",
    },
    MetricDef {
        name: "count",
        capability: Capability::WallAndCounts,
        unit: "count",
        better: Better::Lower,
        kind: Kind::Exact,
        words: COUNT,
        min_n: 0,
        across: Across::Compared,
        about: "`count.<name>`: what the bench file (`ctx.count`) or the page (`strobe.count`) counted per run",
    },
    MetricDef {
        name: "app",
        capability: Capability::WallAndCounts,
        unit: "value",
        better: Better::Lower,
        kind: Kind::Timed,
        words: ("less", "more"),
        min_n: 0,
        across: Across::Compared,
        about: "`app.<name>`: what the app's `{\"strobe\":\"metric\"}` lines on $STROBE_EVENTS added up to per run (lower is better)",
    },
    MetricDef {
        name: "phase",
        capability: Capability::WallAndCounts,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "`phase.<name>`: wall time of the run's `phase(name, fn)` spans, from strobe's own marks (summed when a run has several)",
    },
    MetricDef {
        name: "span",
        capability: Capability::PageEngine,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::PerSide,
        about: "`span.<name>`: the page's own `performance.measure(name)` spans that ended in the run, summed (pages and Electron apps; the browser timestamps them)",
    },
    MetricDef {
        name: "launch.ready",
        capability: Capability::Launch,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "a launch case: from process spawn until its `until` selector is visible",
    },
    MetricDef {
        name: "launch.painted",
        capability: Capability::Pixels,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "a launch case: from process spawn until the window's pixels first differ from its first frame (Windows)",
    },
    MetricDef {
        name: "launch.settled",
        capability: Capability::Pixels,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::Compared,
        about: "a launch case: from process spawn until the window's pixels last changed before ready (Windows)",
    },
    MetricDef {
        name: "launch.milestone",
        capability: Capability::Launch,
        unit: "ms",
        better: Better::Lower,
        kind: Kind::Timed,
        words: TIME,
        min_n: 0,
        across: Across::PerSide,
        about: "`launch.<name>`: from process spawn until a milestone (a log line, Node bootstrap)",
    },
];

/// `metric.<name>` whose command's lines say `higher` (a rate, a score): the family's definition with
/// the other direction (SPEC §9.6).
pub const METRIC_HIGHER: MetricDef = MetricDef {
    name: "metric",
    capability: Capability::WallAndCounts,
    unit: "value",
    better: Better::Higher,
    kind: Kind::Timed,
    words: ("more", "less"),
    min_n: 0,
    across: Across::Compared,
    about: "`metric.<name>`: what a cli target's command printed, summed per run, higher is better (its lines say `higher`)",
};

/// A counter whose value no neighbour on the CPUs moves: an exact count of what the target did
/// (`count.*`, `net.requests*`, `net.bytes*`, `tty.frames`) or the DOM it left (`dom.nodes`). On a
/// shared runner (`SharedRunner`) these alone are compared, and only while they hold one value.
pub fn is_ci_counter(name: &str) -> bool {
    name == "dom.nodes" || metric(name).is_some_and(|d| d.kind == Kind::Exact)
}

/// `def` as a shared runner compares it (`is_ci_counter`): `dom.nodes` as an exact counter, its
/// difference the answer while it holds one value on each side; any other definition as it is.
pub fn as_counter(def: &'static MetricDef) -> &'static MetricDef {
    static DOM_NODES: std::sync::OnceLock<MetricDef> = std::sync::OnceLock::new();
    if def.name == "dom.nodes" { DOM_NODES.get_or_init(|| MetricDef { kind: Kind::Exact, ..*def }) } else { def }
}

/// A metric's definition. Families resolve by their base: `net.requests{<glob>}` → `net.requests`,
/// `count.<name>` → `count`, `phase.<name>` → `phase`, `span.<name>` → `span`, `metric.<name>` →
/// `metric`, `app.<name>` → `app`, `launch.<milestone>` → `launch.milestone`.
pub fn metric(name: &str) -> Option<&'static MetricDef> {
    let base = name.split_once('{').map_or(name, |(b, _)| b);
    let find = |n: &str| METRICS.iter().find(|m| m.name == n);
    find(base).or_else(|| match base.split_once('.') {
        Some(("count", rest)) if !rest.is_empty() => find("count"),
        Some(("phase", rest)) if !rest.is_empty() => find("phase"),
        Some(("span", rest)) if !rest.is_empty() => find("span"),
        Some(("metric", rest)) if !rest.is_empty() => find("metric"),
        Some(("app", rest)) if !rest.is_empty() => find("app"),
        Some(("launch", rest)) if !rest.is_empty() => find("launch.milestone"),
        _ => None,
    })
}
