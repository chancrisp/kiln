//! The optional update check. It is the only network call Kiln makes, and
//! it runs only when the Check for updates button is pressed: one HTTPS
//! request for the latest release's version, nothing sent but the request.

/// Where the latest release is described.
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
                    UpdateState::Available(format!("{}.{}.{}", latest.0, latest.1, latest.2))
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

/// "v1.2.3", "1.2" or "1.2.3-beta.1" as (major, minor, patch). A
/// pre-release counts as its version.
fn version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().transpose().ok()?.unwrap_or(0);
    let patch = parts.next().transpose().ok()?.unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(tag: &str) -> Reply {
        Reply::Body(format!(
            "{{\"url\":\"x\",\"tag_name\" : \"{tag}\",\"name\":\"Kiln {tag}\"}}"
        ))
    }

    #[test]
    fn versions_parse() {
        assert_eq!(version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(version("0.10"), Some((0, 10, 0)));
        assert_eq!(version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(version("release"), None);
        assert_eq!(version("1.2.3.4"), None);
        assert_eq!(version(CURRENT).map(|_| ()), Some(()));
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
