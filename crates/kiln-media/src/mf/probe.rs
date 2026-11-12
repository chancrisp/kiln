use std::sync::{Mutex, PoisonError};

/// Which Media Foundation codecs this Windows has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MfCodecs {
    pub aac_enc: bool,
    pub wma_enc: bool,
    pub ac3_enc: bool,
    pub wma_dec: bool,
    pub ac3_dec: bool,
    pub heaac_dec: bool,
}

/// The codecs as last looked for: once, on first use, and again whenever
/// [`mf_recheck`] is called.
static CACHE: Cache = Cache::new();

/// What Media Foundation has, probed on first use and kept until
/// [`mf_recheck`] looks again.
///
/// All false off Windows, when `MFStartup` fails, or, in debug builds only,
/// when the environment variable `KILN_HIDE_MF` is set (so the N-edition row
/// can be checked in the app).
pub fn mf_codecs() -> MfCodecs {
    CACHE.get(probe)
}

/// Look for the codecs again and keep what is found, for when Windows'
/// Media Feature Pack may have been added since the last look (ruling R51).
/// Takes a few milliseconds, so the app calls it off its window thread.
pub fn mf_recheck() -> MfCodecs {
    CACHE.recheck(probe)
}

/// One kept answer, worked out on first use and replaced on a recheck.
struct Cache(Mutex<Option<MfCodecs>>);

impl Cache {
    const fn new() -> Cache {
        Cache(Mutex::new(None))
    }

    /// The kept answer, or `look`'s, kept, when there is none yet. Callers
    /// at the same moment wait for the one look.
    fn get(&self, look: impl FnOnce() -> MfCodecs) -> MfCodecs {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *kept.get_or_insert_with(look)
    }

    /// `look`'s answer, which replaces the kept one. The look runs before
    /// the lock is taken, so [`Cache::get`] never waits on it.
    fn recheck(&self, look: impl FnOnce() -> MfCodecs) -> MfCodecs {
        let found = look();
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        found
    }
}

fn probe() -> MfCodecs {
    #[cfg(debug_assertions)]
    if std::env::var_os("KILN_HIDE_MF").is_some() {
        return MfCodecs::default();
    }
    #[cfg(windows)]
    {
        win::probe()
    }
    #[cfg(not(windows))]
    {
        MfCodecs::default()
    }
}

#[cfg(windows)]
mod win {
    use super::MfCodecs;
    use windows::core::GUID;
    use windows::Win32::Media::MediaFoundation::{
        IMFActivate, MFAudioFormat_AAC, MFAudioFormat_Dolby_AC3, MFAudioFormat_WMAudioV8,
        MFMediaType_Audio, MFShutdown, MFStartup, MFTEnumEx, MFSTARTUP_LITE,
        MFT_CATEGORY_AUDIO_DECODER, MFT_CATEGORY_AUDIO_ENCODER, MFT_ENUM_FLAG_ASYNCMFT,
        MFT_ENUM_FLAG_LOCALMFT, MFT_ENUM_FLAG_SYNCMFT, MFT_REGISTER_TYPE_INFO, MF_VERSION,
    };
    use windows::Win32::System::Com::CoTaskMemFree;

    pub fn probe() -> MfCodecs {
        // COM for this thread, balanced when the thread ends; on a thread
        // that is already an apartment (a window's) it is left as it is.
        crate::mf::com::com();
        // Media Foundation for the length of the look; MFStartup counts, so
        // a look started while a job runs leaves the job's start-up alone.
        let Ok(_mf) = Started::new() else {
            return MfCodecs::default();
        };
        // The Windows AAC decoder handles both AAC-LC and HE-AAC (v1 and v2)
        // payloads under the one AAC subtype.
        MfCodecs {
            aac_enc: has(MFT_CATEGORY_AUDIO_ENCODER, None, Some(MFAudioFormat_AAC)),
            wma_enc: has(
                MFT_CATEGORY_AUDIO_ENCODER,
                None,
                Some(MFAudioFormat_WMAudioV8),
            ),
            ac3_enc: has(
                MFT_CATEGORY_AUDIO_ENCODER,
                None,
                Some(MFAudioFormat_Dolby_AC3),
            ),
            wma_dec: has(
                MFT_CATEGORY_AUDIO_DECODER,
                Some(MFAudioFormat_WMAudioV8),
                None,
            ),
            ac3_dec: has(
                MFT_CATEGORY_AUDIO_DECODER,
                Some(MFAudioFormat_Dolby_AC3),
                None,
            ),
            heaac_dec: has(MFT_CATEGORY_AUDIO_DECODER, Some(MFAudioFormat_AAC), None),
        }
    }

    /// Media Foundation started in its light mode, shut down on drop.
    struct Started(());

    impl Started {
        fn new() -> windows::core::Result<Started> {
            // Media Foundation's DLLs are delay-loaded: never call in
            // without them (DIST-04).
            if !crate::mf::com::installed() {
                return Err(windows::Win32::Foundation::E_NOTIMPL.into());
            }
            // SAFETY: a plain MF start-up call, balanced by `Drop`.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }?;
            Ok(Started(()))
        }
    }

    impl Drop for Started {
        fn drop(&mut self) {
            // SAFETY: balances the MFStartup in `new`.
            unsafe {
                let _ = MFShutdown();
            }
        }
    }

    fn info(subtype: GUID) -> MFT_REGISTER_TYPE_INFO {
        MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: subtype,
        }
    }

    /// True when at least one transform in `category` takes `input` / makes `output`.
    fn has(category: GUID, input: Option<GUID>, output: Option<GUID>) -> bool {
        let input = input.map(info);
        let output = output.map(info);
        let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        // SAFETY: the type-info pointers point at locals that outlive the call;
        // on success MFTEnumEx hands back a CoTaskMem array of `count`
        // activation objects, which we drop (releasing each) and then free.
        unsafe {
            let r = MFTEnumEx(
                category,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_LOCALMFT,
                input.as_ref().map(|i| i as *const _),
                output.as_ref().map(|o| o as *const _),
                &mut activates,
                &mut count,
            );
            if r.is_err() || activates.is_null() {
                return false;
            }
            for i in 0..count as usize {
                std::ptr::drop_in_place(activates.add(i));
            }
            CoTaskMemFree(Some(activates as *const _));
        }
        count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn print_media_foundation_codecs() {
        println!("{:?}", mf_codecs());
    }

    #[test]
    fn a_recheck_replaces_the_kept_answer() {
        let cache = Cache::new();
        let none = MfCodecs::default();
        let all = MfCodecs {
            aac_enc: true,
            wma_enc: true,
            ac3_enc: true,
            wma_dec: true,
            ac3_dec: true,
            heaac_dec: true,
        };
        // The first look is kept: a second get doesn't look again.
        assert_eq!(cache.get(|| none), none);
        assert_eq!(cache.get(|| unreachable!("kept")), none);
        // A recheck looks again and keeps what it finds.
        assert_eq!(cache.recheck(|| all), all);
        assert_eq!(cache.get(|| unreachable!("kept")), all);
        // And a recheck before any get is kept too.
        let fresh = Cache::new();
        assert_eq!(fresh.recheck(|| all), all);
        assert_eq!(fresh.get(|| none), all);
    }

    #[test]
    fn mf_recheck_agrees_with_mf_codecs() {
        let now = mf_recheck();
        assert_eq!(mf_codecs(), now);
        // The look runs again from a thread of its own, as the app runs it.
        let again = std::thread::spawn(mf_recheck).join().unwrap();
        assert_eq!(again, now);
    }

    #[cfg(windows)]
    #[test]
    fn a_full_windows_has_the_aac_encoder() {
        if std::env::var_os("KILN_CI_NO_MF").is_some_and(|v| v == "1") {
            eprintln!("skipped: no Media Foundation");
            return;
        }
        assert!(mf_codecs().aac_enc);
    }
}
