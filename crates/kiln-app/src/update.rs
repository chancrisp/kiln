//! The optional update check. It is the only network call Kiln makes, and
//! it runs only when the Check for updates button is pressed: one HTTPS
//! request for the latest release's version, nothing sent but the request.

/// Where the latest release is described: Kiln's public repository, which
/// holds its code and its releases.
pub const HOST: &str = "api.github.com";
pub const PATH: &str = "/repos/chancrisp/kiln/releases/latest";
/// Opened in the browser when a newer version is out.
pub const RELEASES_PAGE: &str = "https://github.com/chancrisp/kiln/releases/latest";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Where the update check stands, shown at the foot of the settings panel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// A newer version, as "0.2.0".
    Available(String),
    /// The project has published no release yet.
    NoReleases,
    /// The check didn't finish; the reason is a short sentence.
    Failed(String),
}

/// What the server answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// HTTP 200 with this body.
    Body(String),
    /// Any other HTTP status.
    Status(u32),
    /// No answer: the request itself failed.
    Unreachable,
}

impl UpdateState {
    /// Read the server's answer against this build's version.
    pub fn from_reply(reply: &Reply) -> Self {
        Self::from_reply_for(reply, CURRENT)
    }

    fn from_reply_for(reply: &Reply, current: &str) -> Self {
        match reply {
            Reply::Body(json) => match latest_tag(json).and_then(|t| version(&t)) {
                Some(latest) if version(current).is_some_and(|c| latest > c) => {
                    UpdateState::Available(latest.to_string())
                }
                Some(_) => UpdateState::UpToDate,
                None => UpdateState::Failed("GitHub's answer didn't name a version.".into()),
            },
            Reply::Status(404) => UpdateState::NoReleases,
            Reply::Status(403 | 429) => {
                UpdateState::Failed("GitHub is limiting requests. Try again later.".into())
            }
            Reply::Status(_) => UpdateState::Failed("GitHub didn't answer as expected.".into()),
            Reply::Unreachable => UpdateState::Failed("Couldn't reach GitHub.".into()),
        }
    }

    /// The line beside the button, and whether it is muted.
    pub fn note(&self) -> (String, bool) {
        match self {
            UpdateState::Idle => (format!("Kiln {CURRENT}"), true),
            UpdateState::Checking => ("Checking for updates…".into(), true),
            UpdateState::UpToDate => (format!("Kiln {CURRENT} is the latest version."), true),
            UpdateState::Available(v) => (format!("Kiln {v} is available."), false),
            UpdateState::NoReleases => ("No releases are published yet.".into(), true),
            UpdateState::Failed(why) => (why.clone(), false),
        }
    }

    /// The button's label: once an update is found it opens the download.
    pub fn button(&self) -> &'static str {
        match self {
            UpdateState::Available(_) => "Get the update",
            _ => "Check for updates",
        }
    }
}

/// The `tag_name` of a GitHub release, read without a JSON parser: the
/// value is a plain string with no escapes in any tag worth comparing.
fn latest_tag(json: &str) -> Option<String> {
    let rest = &json[json.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

/// A version, ordered as semver orders them (DIST-27): by its numbers,
/// then a pre-release before the release itself. Build metadata (after
/// `+`) is not compared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: (u64, u64, u64),
    /// The dot-separated parts after `-`; empty for a release.
    pre: Vec<Part>,
}

/// One part of a pre-release: a number compares as one, and comes before
/// any word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(u64),
    Word(String),
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                // Part by part, and a shorter list first when one starts
                // the other.
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (a, b, c) = self.core;
        write!(f, "{a}.{b}.{c}")?;
        for (k, p) in self.pre.iter().enumerate() {
            f.write_str(if k == 0 { "-" } else { "." })?;
            match p {
                Part::Number(n) => write!(f, "{n}")?,
                Part::Word(w) => f.write_str(w)?,
            }
        }
        Ok(())
    }
}

/// "v1.2.3", "1.2", "1.2.3-rc.1" or "1.2.3+build" as a [`Version`].
fn version(tag: &str) -> Option<Version> {
    let text = tag.trim().trim_start_matches(['v', 'V']);
    let text = text.split('+').next()?;
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().transpose().ok()?.unwrap_or(0);
    let patch = parts.next().transpose().ok()?.unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => vec![],
        Some(pre) => pre
            .split('.')
            .map(|p| match p.parse::<u64>() {
                Ok(n) => Some(Part::Number(n)),
                Err(_) if !p.is_empty() => Some(Part::Word(p.to_owned())),
                Err(_) => None,
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some(Version {
        core: (major, minor, patch),
        pre,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one public repository holds the code and the releases (D1).
    #[test]
    fn the_check_asks_the_public_repository() {
        assert_eq!(PATH, "/repos/chancrisp/kiln/releases/latest");
        assert_eq!(
            RELEASES_PAGE,
            "https://github.com/chancrisp/kiln/releases/latest"
        );
    }

    fn body(tag: &str) -> Reply {
        Reply::Body(format!(
            "{{\"url\":\"x\",\"tag_name\" : \"{tag}\",\"name\":\"Kiln {tag}\"}}"
        ))
    }

    #[test]
    fn versions_parse() {
        let core = |t: &str| version(t).map(|v| v.core);
        assert_eq!(core("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(core("0.10"), Some((0, 10, 0)));
        assert_eq!(core("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(core("1.2.3+build.5"), Some((1, 2, 3)));
        assert_eq!(version("release"), None);
        assert_eq!(version("1.2.3.4"), None);
        assert_eq!(version("1.2.3-"), None);
        assert_eq!(version(CURRENT).map(|_| ()), Some(()));
    }

    /// Versions order as semver has them (DIST-27): a release comes after
    /// its own pre-releases, and pre-release parts compare as numbers when
    /// they are numbers.
    #[test]
    fn pre_releases_come_before_their_release() {
        let v = |t: &str| version(t).unwrap();
        assert!(v("0.1.0-rc.1") < v("0.1.0"));
        assert!(v("0.1.0-rc.1") < v("0.1.0-rc.2"));
        assert!(v("0.1.0-rc.2") < v("0.1.0-rc.10"));
        assert!(v("0.1.0-alpha") < v("0.1.0-alpha.1"));
        assert!(v("0.1.0-alpha.9") < v("0.1.0-beta"));
        assert!(v("0.1.0-1") < v("0.1.0-alpha"));
        assert!(v("0.1.0") < v("0.1.1-rc.1"));
        assert_eq!(v("0.1.0+a"), v("0.1.0+b"));
    }

    /// A release candidate is offered its release; a release is never
    /// offered a candidate.
    #[test]
    fn a_candidate_is_offered_its_release() {
        let s = UpdateState::from_reply_for(&body("v0.1.0"), "0.1.0-rc.1");
        assert_eq!(s, UpdateState::Available("0.1.0".into()));
        let s = UpdateState::from_reply_for(&body("v0.1.0-rc.2"), "0.1.0");
        assert_eq!(s, UpdateState::UpToDate);
        let s = UpdateState::from_reply_for(&body("v0.2.0-rc.1"), "0.1.0");
        assert_eq!(s, UpdateState::Available("0.2.0-rc.1".into()));
    }

    #[test]
    fn newer_releases_are_offered() {
        let s = UpdateState::from_reply_for(&body("v0.10.0"), "0.9.1");
        assert_eq!(s, UpdateState::Available("0.10.0".into()));
        assert_eq!(s.button(), "Get the update");
        assert_eq!(s.note(), ("Kiln 0.10.0 is available.".into(), false));
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        for tag in ["v0.1.0", "0.0.9"] {
            let s = UpdateState::from_reply_for(&body(tag), "0.1.0");
            assert_eq!(s, UpdateState::UpToDate, "{tag}");
            assert_eq!(s.button(), "Check for updates");
        }
    }

    #[test]
    fn failures_say_what_happened() {
        let from = |r| UpdateState::from_reply_for(&r, "0.1.0");
        assert_eq!(from(Reply::Status(404)), UpdateState::NoReleases);
        assert!(matches!(from(Reply::Status(403)), UpdateState::Failed(_)));
        assert!(matches!(from(Reply::Status(500)), UpdateState::Failed(_)));
        assert!(matches!(from(Reply::Unreachable), UpdateState::Failed(_)));
        assert!(matches!(
            from(Reply::Body("{\"message\":\"hi\"}".into())),
            UpdateState::Failed(_)
        ));
    }

    #[test]
    fn notes_follow_the_voice_rules() {
        let states = [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Available("9.9.9".into()),
            UpdateState::NoReleases,
            UpdateState::from_reply_for(&Reply::Unreachable, "0.1.0"),
            UpdateState::from_reply_for(&Reply::Status(429), "0.1.0"),
        ];
        for s in states {
            let (text, _) = s.note();
            assert!(!text.contains('!'), "{text}");
            assert!(text.len() <= 48, "too long for the panel: {text}");
        }
    }
}
