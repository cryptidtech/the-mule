use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::io::Write;
use std::sync::Arc;

/// A writer that routes output through `MultiProgress::println()` so that
/// log lines do not corrupt active progress bars.
pub struct IndicatifWriter {
    multi: Arc<MultiProgress>,
    buf: Vec<u8>,
}

impl Write for IndicatifWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        // Flush complete lines through multi.println()
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&self.buf[..pos]).to_string();
            let _ = self.multi.println(&line);
            self.buf.drain(..=pos);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf).to_string();
            let _ = self.multi.println(&line);
            self.buf.clear();
        }
        Ok(())
    }
}

/// Implements `tracing_subscriber::fmt::MakeWriter` so that tracing log output
/// is routed through `MultiProgress::println()`.
#[derive(Clone)]
pub struct IndicatifMakeWriter {
    multi: Arc<MultiProgress>,
}

impl IndicatifMakeWriter {
    pub fn new(multi: Arc<MultiProgress>) -> Self {
        Self { multi }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for IndicatifMakeWriter {
    type Writer = IndicatifWriter;

    fn make_writer(&'a self) -> Self::Writer {
        IndicatifWriter {
            multi: self.multi.clone(),
            buf: Vec::new(),
        }
    }
}

/// Create a spinner-style progress bar attached to the given `MultiProgress`.
pub fn new_spinner(multi: &MultiProgress, msg: &str) -> ProgressBar {
    let pb = multi.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Create a byte-progress bar attached to the given `MultiProgress`.
pub fn new_progress_bar(multi: &MultiProgress, total: u64, msg: &str) -> ProgressBar {
    let pb = multi.add(ProgressBar::new(total));
    pb.set_style(
        ProgressStyle::with_template(
            "{msg} [{bar:30.cyan/dim}] {bytes}/{total_bytes} ({bytes_per_sec})",
        )
        .unwrap()
        .progress_chars("=> "),
    );
    pb.set_message(msg.to_string());
    pb
}