//! Native asynchronous WAV playback. Headless runs have no audio output.
#[derive(Default)]
pub(crate) struct AudioPlayer {
    #[cfg(windows)]
    current: Option<Vec<u8>>,
}
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
}
impl AudioPlayer {
    pub fn play(&mut self, data: &[u8]) -> anyhow::Result<()> {
        anyhow::ensure!(
            data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WAVE"),
            "native sound playback currently requires WAV audio"
        );
        #[cfg(windows)]
        {
            // Stop before replacing the allocation: asynchronous memory playback
            // borrows its bytes until it finishes or is explicitly stopped.
            self.stop();
            self.current = Some(data.to_vec());
            let bytes = self.current.as_ref().unwrap();
            let played =
                unsafe { PlaySoundW(bytes.as_ptr().cast(), std::ptr::null_mut(), 1 | 2 | 4) };
            anyhow::ensure!(played != 0, "Windows could not play the WAV sound");
            Ok(())
        }
        #[cfg(not(windows))]
        {
            anyhow::bail!("native WAV playback is currently supported on Windows only");
        }
    }
    pub fn stop(&mut self) {
        #[cfg(windows)]
        if self.current.is_some() {
            unsafe {
                PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
            }
            self.current = None;
        }
    }
}
impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}
