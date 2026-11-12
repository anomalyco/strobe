//! What a published surface shows of a path, a URL and another tool's output (`Text::path`, `Text::url`,
//! `Text::external_output`). Each scrubber states its rule first; the code below follows it and nothing
//! else. Each may hide too much, never too little: what it cannot read confidently it hides whole.
//! `scripts/dev/private-text.ts` holds every input a review found and a few thousand combinations of planted
//! secrets with the separators, brackets, escapes and wrappers those reviews listed.

use super::REDACTED;

// Paths.
//
// The rule (`Text::path`): `.` and `..` are resolved first. A path in the repository is then said from its
// root (`perf/a.ts`, `.`), one in strobe's state folder from `<state>` (`<state>/experiments/…`), one in this
// machine's home or in any user's home folder (`/Users/<u>`, `/home/<u>`, `/var/home/<u>`, `/root`,
// `<d>:\Users\<u>`, `/mnt/<d>/Users/<u>`, `\\<server>\Users\<u>`) from `~` (`~`, `~/<leaf>`,
// `~/…/<leaf>`), and any other absolute path by its last part (`…/<leaf>`). A relative path stays, unless a
// `..` climbs out of it (`…/<leaf>`). So outside the repository and the state folder nothing of a path but
// its last part is shown.

/// A path's parts for comparison: `.` and `..` resolved, a drive (`c:`) first, the verbatim prefix and
/// macOS's `/private` before `/var` and `/tmp` dropped.
pub(super) struct Split {
    absolute: bool,
    /// Its parts; a drive is the first.
    parts: Vec<String>,
    pub(super) drive: bool,
    /// A share (`\\server\share`, `//server/share`): its first two parts are the server and the share.
    unc: bool,
    /// A `..` climbed above the root (absolute) or out of where it starts (relative).
    climbs: bool,
    /// Compared without case (Windows).
    fold: bool,
}

impl Split {
    pub(super) fn new(s: &str) -> Split {
        let unverbatim;
        let s = match s.strip_prefix(r"\\?\") {
            Some(r) if r.get(..4).is_some_and(|u| u.eq_ignore_ascii_case(r"UNC\")) => {
                unverbatim = format!(r"\\{}", &r[4..]);
                unverbatim.as_str()
            }
            Some(r) => r,
            None => s,
        };
        let b = s.as_bytes();
        let sep = |i: usize| b.get(i).is_some_and(|c| matches!(c, b'/' | b'\\'));
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || sep(2));
        let absolute = drive || sep(0);
        let unc = !drive && sep(0) && sep(1);
        let mut parts: Vec<String> = if drive { vec![s[..2].to_ascii_lowercase()] } else { Vec::new() };
        let floor = parts.len();
        let mut climbs = false;
        for c in s[if drive { 2 } else { 0 }..].split(['/', '\\']) {
            match c {
                "" | "." => {}
                ".." if absolute && parts.len() > floor => {
                    parts.pop();
                }
                ".." => climbs = true,
                c => parts.push(c.to_string()),
            }
        }
        if !drive && !unc && absolute && parts.len() >= 2 && parts[0] == "private" && (parts[1] == "var" || parts[1] == "tmp") {
            parts.remove(0);
        }
        Split { absolute, parts, drive, unc, climbs, fold: cfg!(windows) || drive || s.contains('\\') }
    }

    /// Its parts below the drive.
    pub(super) fn names(&self) -> &[String] {
        &self.parts[self.drive as usize..]
    }

    /// Its parts below `base`, if it is there. A base with no part of its own (`/`, `C:\`) holds nothing.
    fn below(&self, base: &Split) -> Option<&[String]> {
        let fold = self.fold || base.fold;
        let same = |a: &String, b: &String| if fold { a.to_lowercase() == b.to_lowercase() } else { a == b };
        let fits = self.absolute && base.absolute && !self.climbs && !base.climbs && !base.names().is_empty() && self.drive == base.drive && self.parts.len() >= base.parts.len();
        (fits && self.parts.iter().zip(&base.parts).all(|(a, b)| same(a, b))).then(|| &self.parts[base.parts.len()..])
    }

    /// Its parts below a user's home folder, if it is in one: `/Users/<u>`, `/home/<u>`, `/var/home/<u>`,
    /// `/root`, `<d>:\Users\<u>`, `/mnt/<d>/Users/<u>`, `\\<server>\Users\<u>`.
    fn below_a_home(&self) -> Option<&[String]> {
        let n = self.names();
        let is = |i: usize, w: &str| n.get(i).is_some_and(|p| p.eq_ignore_ascii_case(w));
        let user = if self.unc {
            (n.len() >= 3 && is(1, "Users")).then_some(3)
        } else if n.len() >= 2 && (is(0, "Users") || is(0, "home")) {
            Some(2)
        } else if n.len() >= 3 && is(0, "var") && is(1, "home") {
            Some(3)
        } else if !self.drive && n.len() >= 4 && is(0, "mnt") && n[1].len() == 1 && is(2, "Users") {
            Some(4)
        } else if !self.drive && is(0, "root") {
            Some(1)
        } else {
            None
        };
        user.filter(|_| self.absolute && !self.climbs).map(|i| &n[i..])
    }
}

/// `path`'s public form (`Text::path`) against this machine's state folder and home.
pub(super) fn public_path(path: &str, repo: Option<&str>) -> String {
    let state = crate::host::paths::state_path().display().to_string();
    let home = crate::host::home().display().to_string();
    public_path_in(path, repo, &state, &home)
}

/// `Text::path`'s public form of `path` given the repository's root, the state folder and the home folder;
/// on its own for the dev check (`scripts/dev/private-text.ts`), which tries homes and roots this machine
/// does not have.
#[doc(hidden)]
pub fn public_path_in(path: &str, repo: Option<&str>, state: &str, home: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let expanded;
    let path = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            expanded = format!("{home}{rest}");
            expanded.as_str()
        }
        _ => path,
    };
    let p = Split::new(path);
    let names = p.names();
    let tail = |lead: &str| names.last().map_or_else(|| lead.to_string(), |l| format!("{lead}/{l}"));
    if !p.absolute {
        return if p.climbs { tail("…") } else { path.to_string() };
    }
    if let Some(rest) = repo.and_then(|r| p.below(&Split::new(r))) {
        return if rest.is_empty() { ".".into() } else { rest.join("/") };
    }
    if let Some(rest) = p.below(&Split::new(state)) {
        return if rest.is_empty() { "<state>".into() } else { format!("<state>/{}", rest.join("/")) };
    }
    if let Some(rest) = p.below(&Split::new(home)).or_else(|| p.below_a_home()) {
        return match rest.len() {
            0 => "~".into(),
            1 => tail("~"),
            _ => tail("~/…"),
        };
    }
    tail("…")
}

// URLs.
//
// The rule (`Text::url`): a URL keeps its scheme, host, port and path. Credentials before the host become
// `<redacted>` whatever they hold (`#`, `?` included), unless what follows the `@` is no host (`?a=b@c.d&v=1`
// is a query). The value of every parameter whose name, or the last part of a bracketed or dotted name
// (`filter[token]`, `oauth.token`), ends in a secret's word whatever its case (`token`, `accessToken`,
// `secret`, `key`, `password`, `signature`, `session`, `credential`, …) becomes `<redacted>`, in the path
// (`;k=v`), the query and the fragment (`#k=v`, `#/route?k=v`, `#a#k=v`). A `file:` URL's path is said as
// `Text::path` says it, its query and fragment as above; a `mailto:` address and a `data:` payload become
// `<redacted>`. Linear in the URL's length.

/// A URL as a published surface may show it (the rule above).
pub fn scrub_url(url: &str) -> String {
    scrub_url_in(url, None)
}

/// `scrub_url`, a `file:` URL in the repository said from its root.
pub(super) fn scrub_url_in(url: &str, repo: Option<&str>) -> String {
    let lower = url.chars().take(7).collect::<String>().to_ascii_lowercase();
    if lower.starts_with("data:") {
        return match url.find(',') {
            Some(i) if i + 1 < url.len() => format!("{},<redacted>", &url[..i]),
            Some(_) => url.to_string(),
            None => "data:<redacted>".into(),
        };
    }
    if lower.starts_with("mailto:") {
        let rest = &url[7..];
        let (address, query) = rest.split_at(rest.find('?').unwrap_or(rest.len()));
        return format!("{}{}{}", &url[..7], if address.is_empty() { "" } else { "<redacted>" }, scrub_params(query));
    }
    if lower.starts_with("file:") {
        let rest = &url[5..];
        let (raw, tail) = rest.split_at(rest.find(['?', '#']).unwrap_or(rest.len()));
        let path = raw.trim_start_matches(['/', '\\']);
        let path = path.strip_prefix("localhost/").unwrap_or(path);
        let path = if Split::new(path).drive { path.to_string() } else { format!("/{path}") };
        return format!("file://{}{}", public_path(&path, repo), scrub_params(tail));
    }
    let (head, mut rest) = match url.find("://") {
        Some(i) => (&url[..i + 3], &url[i + 3..]),
        None => ("", url),
    };
    let mut out = head.to_string();
    let authority = rest.find('/').unwrap_or(rest.len());
    if let Some(at) = rest[..authority].rfind('@') {
        let host = &rest[at + 1..authority];
        if !rest[..at].contains(['?', '#']) || !host.contains(['=', '&', '?', '#']) {
            out += "<redacted>@";
            rest = &rest[at + 1..];
        }
    }
    out + &scrub_params(rest)
}

/// Every `key=value` of `s` whose key says a secret, its value `<redacted>`; a value ends at `&`, `;`, `#`,
/// `?` or a space. Each character is looked at a bounded number of times.
fn scrub_params(s: &str) -> String {
    let key = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']');
    let mut out = String::with_capacity(s.len());
    let (mut copied, mut from) = (0, 0);
    while let Some(eq) = s[from..].find('=').map(|e| e + from) {
        let key_start = s[from..eq].rfind(|c: char| !key(c)).map_or(from, |k| from + k + s[from + k..].chars().next().map_or(1, char::len_utf8));
        if !secret_key(&s[key_start..eq]) {
            from = eq + 1;
            continue;
        }
        let end = s[eq + 1..].find(|c: char| matches!(c, '&' | ';' | '#' | '?') || c.is_whitespace()).map_or(s.len(), |e| e + eq + 1);
        if end > eq + 1 {
            out += &s[copied..eq + 1];
            out += "<redacted>";
            copied = end;
        }
        from = end.max(eq + 1);
    }
    out + &s[copied..]
}

/// A parameter name that says a secret: its last part (`filter[token]` → `token`, `oauth.token` → `token`)
/// is a known one, or ends in a secret's word whatever its case (`accessToken`, `client_secret`,
/// `x-api-key`, `X-Amz-Signature`). It may hide a harmless one (`hotkey`); `tokenize`, `keyboard` and
/// `author` stay.
fn secret_key(k: &str) -> bool {
    const EXACT: &[&str] = &["code", "sid", "sig", "jwt", "bearer", "cookie", "auth", "pass", "pwd", "otp", "nonce", "state"];
    const ENDS: &[&str] = &["token", "secret", "key", "password", "passwd", "signature", "session", "sessionid", "session_id", "credential", "credentials", "authorization"];
    let k = k.split(['[', ']', '.']).filter(|p| !p.is_empty()).last().unwrap_or("").to_ascii_lowercase();
    EXACT.contains(&k.as_str()) || ENDS.iter().any(|e| k.ends_with(e))
}

// Another tool's output.
//
// The rule (`Text::external_output`):
// 1. Terminal escapes go: 7-bit and 8-bit CSI (`ESC [`, `0x9b`), OSC (`ESC ]`, `0x9d`), DCS/SOS/PM/APC,
//    charset selections (`ESC ( B`) and every other `ESC` sequence and C1 control.
// 2. The text is cut into tokens at whitespace; whitespace (spaces, tabs, newlines) stays as it was and
//    always ends a token, so lines and `file:line:col` keep their shape and nothing spans two tokens.
// 3. In a token, a URL (a scheme of 2 letters or more and `://`; `file:/`, `mailto:…@`, `data:…,`), at the
//    token's start or after a quote, bracket, `=`, `:`, `,`, `;`, `|` or `<`, runs to the token's end, its
//    trailing punctuation and unbalanced closing brackets aside, and is said by `scrub_url`.
// 4. Else each absolute path in the token (`/`, `\`, `~/`, `X:\`, `X:/`, `\\?\X:\`, at the same places)
//    runs to the next `:`, `,`, `;`, `|`, quote, bracket, `<` or `>`, a final `.` aside, and is said as
//    `Text::path` says it; `:3:1`, `(3,1)` and the punctuation after it stay. A path of one part outside
//    every home, the state folder and the repository (`/s` in `120 /s`) stays.
// 5. A token whose path holds `@`, `=`, `?`, `#`, `%` or a control character, or which holds a `://` after
//    2 letters or more that is no URL by 3, becomes `[redacted]` whole.
// 6. A path that is a whole token, followed on its line within three tokens by a token holding a separator
//    (and no `://`), may be a folder name with spaces (`C:\Users\ann\My Projects\northwind\a.ts`): the path
//    loses its last part, the tokens between become `…`, and that token keeps only its own last part.

/// A tool's output as a published surface may show it (the rule above).
pub(super) fn scrub_output(raw: &str, repo: Option<&str>) -> String {
    let s = strip_escapes(raw);
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        scrub_line(line, repo, &mut out);
    }
    out
}

/// One line (rules 2 to 6).
fn scrub_line(line: &str, repo: Option<&str>, out: &mut String) {
    // Whitespace runs and tokens, in order.
    let mut pieces: Vec<(&str, bool)> = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (i, c) in line.char_indices() {
        let ws = c.is_whitespace();
        if space != Some(ws) {
            if i > start {
                pieces.push((&line[start..i], space == Some(true)));
            }
            (start, space) = (i, Some(ws));
        }
    }
    if start < line.len() {
        pieces.push((&line[start..], space == Some(true)));
    }
    let mut said: Vec<Token> = pieces.iter().map(|(p, ws)| if *ws { Token::kept(p) } else { token(p, repo) }).collect();
    // Rule 6.
    let words: Vec<usize> = (0..pieces.len()).filter(|&i| !pieces[i].1).collect();
    for (k, &i) in words.iter().enumerate() {
        if !said[i].whole_path {
            continue;
        }
        for (d, &j) in words.iter().enumerate().skip(k + 1).take(3) {
            let word = pieces[j].0;
            if word.contains("://") || said[j].changed {
                break;
            }
            if let Some(sep) = word.rfind(['/', '\\']) {
                if let Some((head, _)) = said[i].text.rsplit_once('/') {
                    said[i].text = head.to_string();
                }
                for &m in &words[k + 1..d] {
                    said[m].text = "…".into();
                }
                said[j].text = format!("…/{}", &word[sep + 1..]);
                break;
            }
        }
    }
    for t in said {
        out.push_str(&t.text);
    }
}

/// A token as said (rules 3 to 5).
struct Token {
    text: String,
    /// Changed by a path or a URL.
    changed: bool,
    /// The whole token was one path, said in its public form (rule 6).
    whole_path: bool,
}

impl Token {
    fn kept(s: &str) -> Token {
        Token { text: s.to_string(), changed: false, whole_path: false }
    }
}

/// Where a path or a URL may start in a token: its start, or after one of these.
fn opens(c: char) -> bool {
    matches!(c, '"' | '\'' | '`' | '(' | '[' | '{' | '<' | '=' | ':' | ',' | ';' | '|')
}

/// What ends a path in a token.
fn closes(c: char) -> bool {
    matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ':' | ',' | ';' | '|')
}

fn token(tok: &str, repo: Option<&str>) -> Token {
    let hidden = || Token { text: REDACTED.into(), changed: true, whole_path: false };
    let mut out = String::with_capacity(tok.len());
    let (mut copied, mut url, mut whole_path) = (0, false, false);
    let mut prev: Option<char> = None;
    let mut it = tok.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if prev.is_none_or(opens) {
            if let Some(n) = url_at(&tok[i..]) {
                let (u, trail) = trim_url(&tok[i..i + n]);
                out += &tok[copied..i];
                out += &scrub_url_in(u, repo);
                out += trail;
                copied = tok.len();
                url = true;
                break;
            }
            if let Some(n) = path_at(&tok[i..]) {
                let mut path = &tok[i..i + n];
                if path.len() > 1 && path.ends_with('.') && !path.ends_with("..") && !path.ends_with("/.") && !path.ends_with("\\.") {
                    path = &path[..path.len() - 1];
                }
                if path.contains(|c: char| matches!(c, '@' | '=' | '?' | '#' | '%') || c.is_control()) {
                    return hidden();
                }
                let public = public_path(path, repo);
                let keep = Split::new(path).names().len() <= 1 && public.starts_with('…');
                if !keep {
                    out += &tok[copied..i];
                    out += &public;
                    copied = i + path.len();
                    whole_path = i == 0 && copied == tok.len();
                }
                while it.peek().is_some_and(|(j, _)| *j < i + path.len()) {
                    it.next();
                }
                prev = tok[..i + path.len()].chars().next_back();
                continue;
            }
        }
        prev = Some(c);
    }
    if !url && unread_url(tok) {
        return hidden();
    }
    out += &tok[copied..];
    let changed = out != tok;
    Token { text: out, changed, whole_path }
}

/// A `://` after a scheme of 2 characters or more (rule 5): a URL rule 3 did not read. A drive's `C://` is
/// none.
fn unread_url(tok: &str) -> bool {
    tok.match_indices("://").any(|(i, _)| tok[..i].chars().rev().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')).count() >= 2)
}

/// The length of a URL at the start of `t` (rule 3): to the token's end.
fn url_at(t: &str) -> Option<usize> {
    let lower = t.chars().take(7).collect::<String>().to_ascii_lowercase();
    let special = (lower.starts_with("file:/") || lower.starts_with("file:\\")) || (lower.starts_with("mailto:") && t.contains('@')) || (lower.starts_with("data:") && t.contains(','));
    let scheme = t.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).unwrap_or(t.len());
    let web = scheme >= 2 && t.as_bytes()[0].is_ascii_alphabetic() && t[scheme..].starts_with("://");
    (special || web).then_some(t.len())
}

/// A URL without the punctuation after it: a full stop, a comma, a quote, `>`, and closing brackets it
/// does not open (`(https://x/a)` → `https://x/a`, but `http://[::1]/a` and `?filter[token]=x` whole).
fn trim_url(u: &str) -> (&str, &str) {
    let mut end = u.len();
    loop {
        let Some(c) = u[..end].chars().next_back() else { break };
        let open = match c {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        };
        let unbalanced = open.is_some_and(|o| u[..end].matches(c).count() > u[..end].matches(o).count());
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '\'' | '"' | '`' | '>') || unbalanced {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    u.split_at(end)
}

/// The length of an absolute path at the start of `t` (rule 4): to the next character that closes one.
fn path_at(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    let sep = |i: usize| b.get(i).is_some_and(|c| matches!(c, b'/' | b'\\'));
    let drive = |i: usize| b.len() > i + 2 && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && sep(i + 2);
    let lead = if t.starts_with(r"\\?\") && drive(4) {
        7
    } else if sep(0) {
        1
    } else if drive(0) {
        3
    } else if b.first() == Some(&b'~') && sep(1) {
        2
    } else {
        return None;
    };
    Some(t[lead..].find(closes).map_or(t.len(), |e| e + lead))
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Past a string terminator: BEL (`bel`, for OSC), `ESC \` or ST (0x9c).
fn to_st(it: &mut Chars<'_>, bel: bool) {
    while let Some(c) = it.next() {
        if (bel && c == '\u{7}') || c == '\u{9c}' || (c == '\u{1b}' && it.next_if_eq(&'\\').is_some()) {
            break;
        }
    }
}

/// Past a CSI sequence's final byte.
fn csi(it: &mut Chars<'_>) {
    for c in it.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            break;
        }
    }
}

/// `s` without terminal escapes (rule 1).
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.next() {
                Some('[') => csi(&mut it),
                Some(']') => to_st(&mut it, true),
                Some('P' | 'X' | '^' | '_') => to_st(&mut it, false),
                Some(' '..='/') => {
                    while it.next_if(|c| (' '..='/').contains(c)).is_some() {}
                    it.next();
                }
                _ => {}
            },
            '\u{9b}' => csi(&mut it),
            '\u{9d}' => to_st(&mut it, true),
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => to_st(&mut it, false),
            '\u{80}'..='\u{9f}' => {}
            c => out.push(c),
        }
    }
    out
}
