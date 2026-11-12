//! Windows notifications with a Show in folder button (R9).
//!
//! The installed Kiln shows a toast under its AppUserModelID, which the
//! MSI puts on its Start menu shortcut. Its buttons and body open a
//! `kiln:show?n=<id>` link (`handoff::notice_link`), which the MSI
//! registers: Windows starts a second Kiln with it, and that one hands
//! the id to the running Kiln. The portable Kiln has neither the shortcut
//! nor the link, so it shows the balloon it always has, whose click opens
//! what it is about.
//!
//! The words and the XML are pure ([`Words`], [`xml`]); [`show`] is the
//! only code that talks to Windows.

use std::collections::VecDeque;
use std::path::Path;

/// Kiln's AppUserModelID. `packaging/kiln.wxs` sets the same on the Start
/// menu shortcut; without that shortcut Windows shows nothing for it.
#[cfg_attr(test, allow(dead_code))]
pub const AUMID: &str = "Kiln.App";

/// The toast's buttons.
pub const SHOW_IN_FOLDER: &str = "Show in folder";
pub const DISMISS: &str = "Dismiss";

/// How this Kiln shows a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// A toast with Kiln's AppUserModelID: the installed Kiln.
    Toast,
    /// The notification area's balloon: the portable Kiln, or a toast
    /// Windows refused.
    Balloon,
}

/// How notifications are shown, given the command the `kiln:` link is
/// registered to start (`None` when there's none) and this Kiln's `exe`.
/// Only the Kiln the MSI installed has the link, and the shortcut with the
/// AppUserModelID beside it; a link that starts another copy of Kiln
/// would hand the press to that one, so this one uses the balloon.
pub fn channel(link_command: Option<&str>, exe: Option<&Path>) -> Channel {
    match (link_command, exe) {
        (Some(command), Some(exe)) if crate::autostart::is_ours(command, exe) => Channel::Toast,
        _ => Channel::Balloon,
    }
}

/// What a notification says, and whether it has Show in folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    pub title: String,
    pub text: Option<String>,
    pub show_in_folder: bool,
}

/// A notification for results saved where they belong, the mock's:
/// "Saved name.jpg" over "2.1 MB → 1.1 MB · Phone", or "Saved 3 files"
/// over the folder they share. `names` are the results' names, `folders`
/// the names of the folders they went to, and `sizes` the one result's
/// original and new size, when known.
pub fn saved(names: &[String], folders: &[String], sizes: Option<(u64, u64)>) -> Words {
    use crate::model::{human_size, DETAIL_SEPARATOR};
    // The folder they all went to, if they share one.
    let folder = folders
        .first()
        .filter(|f| folders.iter().all(|g| g == *f))
        .cloned();
    let (title, text) = match names {
        [name] => {
            let sizes = sizes
                .map(|(before, after)| format!("{} → {}", human_size(before), human_size(after)));
            let text = match (sizes, folder) {
                (Some(s), Some(f)) => Some(format!("{s}{DETAIL_SEPARATOR}{f}")),
                (s, f) => s.or(f),
            };
            (format!("Saved {name}"), text)
        }
        _ => (format!("Saved {} files", names.len()), folder),
    };
    Words {
        title,
        text,
        show_in_folder: true,
    }
}

/// A notification that says `title` and `text` as the balloon would. A
/// title of just "Kiln" is left out: Windows already names the app over
/// every toast.
pub fn plain(title: &str, text: &str) -> Words {
    let (title, text) = if title == "Kiln" {
        (text.to_owned(), None)
    } else {
        (title.to_owned(), Some(text.to_owned()))
    };
    Words {
        title,
        text,
        show_in_folder: false,
    }
}

/// `s` made safe inside XML text or a quoted attribute: the five special
/// characters as entities, and characters XML can't hold left out.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 holds tab, line feed and carriage return of the
            // controls, and not U+FFFE or U+FFFF.
            '\t' | '\n' | '\r' => out.push(c),
            c if c < ' ' || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The toast's XML: the words, the body opening `link`, and, when the
/// words have it, Show in folder (opening `link` too) and Dismiss.
pub fn xml(words: &Words, link: &str) -> String {
    let link = escape(link);
    let mut x = format!(
        "<toast launch=\"{link}\" activationType=\"protocol\">\
         <visual><binding template=\"ToastGeneric\"><text>{}</text>",
        escape(&words.title)
    );
    if let Some(text) = &words.text {
        x.push_str(&format!("<text>{}</text>", escape(text)));
    }
    x.push_str("</binding></visual>");
    if words.show_in_folder {
        x.push_str(&format!(
            "<actions>\
             <action content=\"{SHOW_IN_FOLDER}\" arguments=\"{link}\" activationType=\"protocol\"/>\
             <action content=\"{DISMISS}\" arguments=\"dismiss\" activationType=\"system\"/>\
             </actions>"
        ));
    }
    x.push_str("</toast>");
    x
}

/// How many of this run's notices are remembered for their links.
pub const KEPT: usize = 32;

/// This run's notices by the id their link carries, the last [`KEPT`].
/// Ids start at a number drawn for the run, so a notification left in
/// Windows' notification centre by an earlier run names no notice of
/// this one.
#[derive(Debug, Clone)]
pub struct Issued<T> {
    next: u64,
    kept: VecDeque<(u64, T)>,
}

impl<T> Issued<T> {
    /// Ids from `seed` up.
    pub fn new(seed: u64) -> Self {
        Issued {
            next: seed,
            kept: VecDeque::new(),
        }
    }

    /// A new notice about `about`: its id.
    pub fn issue(&mut self, about: T) -> u64 {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back((id, about));
        id
    }

    /// What notice `id` is about, if it is one of this run's still kept.
    pub fn get(&self, id: u64) -> Option<&T> {
        self.kept.iter().find(|(k, _)| *k == id).map(|(_, t)| t)
    }
}

/// A number for this run's first notice id: different on each run.
pub fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(t.as_nanos());
    }
    h.finish()
}

/// The command the `kiln:` link is registered to start, as Windows reads
/// it for this account (the MSI writes it for every account). `None` when
/// there is none, as for the portable Kiln. Only read, never written.
pub fn link_command() -> Option<String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_SZ};
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: reads one string into a local buffer of `size` bytes.
    let err = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            w!("kiln\\shell\\open\\command"),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    let units = (size as usize / 2).min(buf.len());
    buf.truncate(units);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Some(String::from_utf16_lossy(&buf))
}

/// Show the toast `xml` under Kiln's AppUserModelID. It replaces the one
/// before it, as the balloon does. An error when Windows refuses it, and
/// the caller shows the balloon instead.
#[cfg(not(test))]
pub fn show(xml: &str) -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from("kiln"))?;
    toast.SetGroup(&HSTRING::from("kiln"))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

/// Take Kiln's toasts out of the notification centre as Kiln quits: their
/// links would name notices no Kiln knows.
#[cfg(not(test))]
pub fn forget() {
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.ClearWithId(&HSTRING::from(AUMID));
    }
}

/// Tests show nothing: what would have been shown is kept here.
#[cfg(test)]
pub fn show(xml: &str) -> windows::core::Result<()> {
    shown::TOASTS.with(|t| t.borrow_mut().push(xml.to_owned()));
    Ok(())
}

#[cfg(test)]
pub fn forget() {}

/// What a test's notifications would have shown, oldest first.
#[cfg(test)]
pub mod shown {
    use std::cell::RefCell;

    thread_local! {
        pub static TOASTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub static BALLOONS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn balloon(title: &str, text: &str) {
        BALLOONS.with(|b| b.borrow_mut().push((title.to_owned(), text.to_owned())));
    }

    /// The toasts and balloons shown so far, taken.
    pub fn take() -> (Vec<String>, Vec<(String, String)>) {
        (
            TOASTS.with(|t| std::mem::take(&mut *t.borrow_mut())),
            BALLOONS.with(|b| std::mem::take(&mut *b.borrow_mut())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_saved_result_is_named_with_its_sizes_and_folder() {
        let w = saved(
            &names(&["IMG_2041.jpg"]),
            &names(&["Phone"]),
            Some((2_100_000, 1_100_000)),
        );
        assert_eq!(
            w,
            Words {
                title: "Saved IMG_2041.jpg".into(),
                text: Some("2.1 MB → 1.1 MB · Phone".into()),
                show_in_folder: true,
            }
        );
        // Sizes not read yet: the folder alone.
        let w = saved(&names(&["IMG_2041.jpg"]), &names(&["Phone"]), None);
        assert_eq!(w.text.as_deref(), Some("Phone"));
    }

    #[test]
    fn several_saved_results_are_counted_with_the_folder_they_share() {
        let w = saved(
            &names(&["a.jpg", "b.jpg", "c.jpg"]),
            &names(&["Holiday", "Holiday", "Holiday"]),
            None,
        );
        assert_eq!(w.title, "Saved 3 files");
        assert_eq!(w.text.as_deref(), Some("Holiday"));
        assert!(w.show_in_folder);
        // In different folders: no folder is named.
        let w = saved(
            &names(&["a.jpg", "b.jpg", "c.jpg"]),
            &names(&["Holiday", "Phone", "Holiday"]),
            Some((1, 2)),
        );
        assert_eq!(w.title, "Saved 3 files");
        assert_eq!(w.text, None);
    }

    #[test]
    fn a_plain_notice_drops_a_title_that_only_says_kiln() {
        assert_eq!(
            plain(
                "Kiln",
                "2 files couldn't be converted. Open Kiln to see why."
            ),
            Words {
                title: "2 files couldn't be converted. Open Kiln to see why.".into(),
                text: None,
                show_in_folder: false,
            }
        );
        assert_eq!(
            plain(
                "Converted a.png to JPG.",
                "Kiln can't save in Docs, so a.jpg is kept in Kiln."
            ),
            Words {
                title: "Converted a.png to JPG.".into(),
                text: Some("Kiln can't save in Docs, so a.jpg is kept in Kiln.".into()),
                show_in_folder: false,
            }
        );
    }

    #[test]
    fn the_toast_for_one_result_has_show_in_folder_and_dismiss() {
        let w = saved(
            &names(&["IMG_2041.jpg"]),
            &names(&["Phone"]),
            Some((2_100_000, 1_100_000)),
        );
        let x = xml(&w, "kiln:show?n=42");
        assert_eq!(
            x,
            "<toast launch=\"kiln:show?n=42\" activationType=\"protocol\">\
             <visual><binding template=\"ToastGeneric\">\
             <text>Saved IMG_2041.jpg</text>\
             <text>2.1 MB → 1.1 MB · Phone</text>\
             </binding></visual>\
             <actions>\
             <action content=\"Show in folder\" arguments=\"kiln:show?n=42\" activationType=\"protocol\"/>\
             <action content=\"Dismiss\" arguments=\"dismiss\" activationType=\"system\"/>\
             </actions></toast>"
        );
    }

    #[test]
    fn the_toast_for_several_results_says_how_many() {
        let w = saved(&names(&["a.jpg", "b.jpg", "c.jpg"]), &[], None);
        let x = xml(&w, "kiln:show?n=7");
        assert!(x.contains("<text>Saved 3 files</text>"), "{x}");
        assert_eq!(x.matches("<text>").count(), 1, "{x}");
        assert!(x.contains("content=\"Show in folder\""));
        assert!(x.contains("content=\"Dismiss\""));
    }

    #[test]
    fn a_toast_without_show_in_folder_has_no_buttons_but_its_body_opens_kiln() {
        let x = xml(
            &plain("Kiln", "A file couldn't be converted."),
            "kiln:show?n=3",
        );
        assert!(x.starts_with("<toast launch=\"kiln:show?n=3\" activationType=\"protocol\">"));
        assert!(!x.contains("<actions>"), "{x}");
    }

    #[test]
    fn file_names_are_escaped_in_the_toast() {
        let w = saved(&names(&["<b>&\"Tom's\".jpg"]), &names(&["a&b <c>"]), None);
        let x = xml(&w, "kiln:show?n=1");
        assert!(
            x.contains("<text>Saved &lt;b&gt;&amp;&quot;Tom&apos;s&quot;.jpg</text>"),
            "{x}"
        );
        assert!(x.contains("<text>a&amp;b &lt;c&gt;</text>"), "{x}");
        // No raw angle bracket or ampersand from the names is left.
        assert!(!x.contains("<b>") && !x.contains("&\""));
        // Characters XML can't hold go.
        assert_eq!(escape("a\u{1}b\u{FFFE}c\td"), "abc\td");
        // The link is escaped as an attribute too.
        let x = xml(&w, "kiln:show?n=1&x=\"y\"");
        assert!(
            x.contains("launch=\"kiln:show?n=1&amp;x=&quot;y&quot;\""),
            "{x}"
        );
    }

    #[test]
    fn the_portable_kiln_uses_the_balloon() {
        let exe = PathBuf::from(r"C:\Program Files\Kiln\kiln-app.exe");
        // No link registered: portable.
        assert_eq!(channel(None, Some(&exe)), Channel::Balloon);
        // The link starts another copy of Kiln.
        assert_eq!(
            channel(Some(r#""D:\Kiln\kiln-app.exe" "%1""#), Some(&exe)),
            Channel::Balloon
        );
        // Nothing known of this exe.
        assert_eq!(
            channel(Some(r#""C:\Program Files\Kiln\kiln-app.exe" "%1""#), None),
            Channel::Balloon
        );
        assert_eq!(channel(Some(""), Some(&exe)), Channel::Balloon);
        // Installed: the link starts this Kiln.
        assert_eq!(
            channel(
                Some(r#""C:\Program Files\Kiln\kiln-app.exe" "%1""#),
                Some(&exe)
            ),
            Channel::Toast
        );
        assert_eq!(
            channel(
                Some(r#""c:\program files\kiln\KILN-APP.EXE" "%1""#),
                Some(&exe)
            ),
            Channel::Toast
        );
    }

    #[test]
    fn only_this_runs_kept_notices_are_known() {
        let mut issued = Issued::new(1_000);
        let a = issued.issue("a");
        let b = issued.issue("b");
        assert_ne!(a, b);
        assert_eq!(issued.get(a), Some(&"a"));
        assert_eq!(issued.get(b), Some(&"b"));
        // Another run's ids, before or after this one's, are unknown.
        assert_eq!(issued.get(999), None);
        assert_eq!(issued.get(b + 1), None);
        assert_eq!(issued.get(0), None);
        // Only the last KEPT are remembered.
        for _ in 0..KEPT {
            issued.issue("more");
        }
        assert_eq!(issued.get(a), None);
        assert_eq!(issued.get(b), None);
        // Ids wrap past the largest number rather than stop.
        let mut top = Issued::new(u64::MAX);
        let x = top.issue(1);
        let y = top.issue(2);
        assert_eq!((x, y), (u64::MAX, 0));
        assert_eq!((top.get(x), top.get(y)), (Some(&1), Some(&2)));
    }

    #[test]
    fn each_run_draws_its_own_seed() {
        assert_ne!(seed(), seed());
    }
}
