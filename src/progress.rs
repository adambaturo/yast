use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};

pub struct Progress {
    display: MultiProgress,
    inventory: ProgressBar,
    pub files: ProgressBar,
    pub bytes: ProgressBar,
    pub verification: ProgressBar,
    enabled: bool,
}

impl Progress {
    pub fn new(enabled: bool) -> Self {
        let display = MultiProgress::with_draw_target(if enabled {
            ProgressDrawTarget::stderr_with_hz(10)
        } else {
            ProgressDrawTarget::hidden()
        });
        let inventory = display.add(ProgressBar::new_spinner());
        inventory.set_style(style("{msg}"));
        let files = display.add(ProgressBar::new_spinner());
        files.enable_steady_tick(Duration::from_millis(100));
        let bytes = display.add(ProgressBar::new(0));
        let verification = display.add(ProgressBar::new(0));
        for bar in [&inventory, &bytes, &verification] {
            bar.finish_and_clear();
        }
        Self {
            display,
            inventory,
            files,
            bytes,
            verification,
            enabled,
        }
    }

    #[cfg(test)]
    pub fn hidden() -> Self {
        let mut progress = Self::new(false);
        // Exercise the instrumented I/O path without requiring a terminal.
        progress.enabled = true;
        progress
    }

    pub fn scanning(&self, label: &'static str) {
        self.files.reset();
        self.files
            .set_style(style("{spinner} {msg}: {pos} files found"));
        self.files.set_message(label);
    }

    pub fn found_file(&self) {
        self.files.inc(1);
    }

    pub fn inventory(&self, source: usize, target: Option<usize>) {
        self.inventory.reset();
        self.inventory.set_message(match target {
            Some(target) => format!("Inventory: source {source} files; target {target} files"),
            None => format!("Inventory: {source} files"),
        });
    }

    pub fn phase(&self, label: &'static str, total: usize) {
        self.files.reset();
        self.files.set_style(style("{spinner} {msg}: {pos}/{len}"));
        self.files.set_length(total as u64);
        self.files.set_message(label);
        self.clear_bytes();
    }

    pub fn activity(&self, label: &'static str) {
        self.files.set_message(label);
        self.clear_bytes();
    }

    fn clear_bytes(&self) {
        for bar in [&self.bytes, &self.verification] {
            bar.finish_and_clear();
            bar.set_position(0);
            bar.set_length(0);
        }
    }

    pub fn completed(&self) {
        self.files.inc(1);
    }

    pub fn byte_phase(&self, label: &'static str, total: u64) {
        start_bytes(&self.bytes, label, total);
    }

    pub fn verification_phase(&self, total: u64) {
        start_bytes(&self.verification, "Verification reads", total);
    }

    pub fn copy(&self, mut source: File, target: &mut File) -> io::Result<u64> {
        if !self.enabled {
            // Retain native file-to-file optimizations when no display is needed.
            let count = io::copy(&mut source, target)?;
            self.bytes.inc(count);
            return Ok(count);
        }
        // A large buffer amortizes instrumentation and system-call overhead.
        io::copy(
            &mut BufReader::with_capacity(1024 * 1024, source),
            &mut CountingWriter {
                inner: target,
                progress: &self.bytes,
            },
        )
    }

    pub fn finish(&self) {
        for bar in [
            &self.inventory,
            &self.files,
            &self.bytes,
            &self.verification,
        ] {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
        let _ = self.display.clear();
    }
}

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).expect("valid progress template")
}

fn start_bytes(bar: &ProgressBar, label: &'static str, total: u64) {
    bar.reset();
    bar.set_length(total);
    bar.set_message(label);
    bar.set_style(style(
        "{msg}: {bytes}/{total_bytes} [{wide_bar}] {bytes_per_sec} ETA {eta}",
    ));
    if total == 0 {
        bar.finish_and_clear();
    }
}

pub struct CountingReader<'a, R> {
    pub inner: R,
    pub progress: &'a ProgressBar,
}

impl<R: Read> Read for CountingReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.progress.inc(count as u64);
        Ok(count)
    }
}

struct CountingWriter<'a, W> {
    inner: W,
    progress: &'a ProgressBar,
}

impl<W: Write> Write for CountingWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(buffer)?;
        self.progress.inc(count as u64);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_counts_are_aggregated_and_phases_reset_independently() {
        let progress = Progress::hidden();
        progress.inventory(20, Some(10));
        progress.phase("Copying (actions)", 4);
        progress.byte_phase("Copied bytes", 64);
        progress.verification_phase(128);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let progress = &progress;
                scope.spawn(move || {
                    for _ in 0..16 {
                        progress.bytes.inc(1);
                        progress.verification.inc(2);
                    }
                    progress.completed();
                });
            }
        });
        assert_eq!(progress.files.position(), 4);
        assert_eq!(progress.bytes.position(), 64);
        assert_eq!(progress.verification.position(), 128);
        assert_eq!(
            progress.inventory.message(),
            "Inventory: source 20 files; target 10 files"
        );

        progress.phase("Next phase", 2);
        assert_eq!(progress.files.position(), 0);
        assert_eq!(progress.files.length(), Some(2));
        assert_eq!(progress.bytes.position(), 0);
        assert_eq!(progress.verification.position(), 0);
        progress.verification_phase(0);
        assert!(progress.verification.is_finished());
        progress.verification_phase(100);
        assert!(!progress.verification.is_finished());
        assert_eq!(progress.verification.length(), Some(100));
        progress.activity("Renaming (actions)");
        assert!(progress.verification.is_finished());
        assert_eq!(progress.files.length(), Some(2));
    }

    #[test]
    fn streaming_copy_counts_writes_before_eof_and_keeps_partial_failure() {
        struct ShortWriter<'a> {
            progress: &'a ProgressBar,
            accepted: usize,
        }
        impl Write for ShortWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                assert_eq!(self.progress.position(), self.accepted as u64);
                if self.accepted == 6 {
                    return Err(io::Error::other("injected write failure"));
                }
                let count = bytes.len().min(3);
                self.accepted += count;
                Ok(count)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let progress = Progress::hidden();
        progress.byte_phase("Copied bytes", 12);
        let mut writer = CountingWriter {
            inner: ShortWriter {
                progress: &progress.bytes,
                accepted: 0,
            },
            progress: &progress.bytes,
        };
        assert!(io::copy(&mut &[1; 12][..], &mut writer).is_err());
        assert_eq!(progress.bytes.position(), 6);
        assert_eq!(progress.bytes.length(), Some(12));
        assert!(!progress.bytes.is_finished());
    }

    #[test]
    fn reader_counts_only_successful_reads() {
        struct FailingReader(bool);
        impl Read for FailingReader {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::Error::other("injected read failure"));
                }
                self.0 = true;
                bytes[0] = 1;
                Ok(1)
            }
        }
        let bar = ProgressBar::hidden();
        let mut reader = CountingReader {
            inner: FailingReader(false),
            progress: &bar,
        };
        assert_eq!(reader.read(&mut [0; 8]).unwrap(), 1);
        assert!(reader.read(&mut [0; 8]).is_err());
        assert_eq!(bar.position(), 1);
    }
}
