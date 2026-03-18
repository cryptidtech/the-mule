use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fmt;
use std::io::Write;
use std::sync::Arc;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::FormatEvent;
use tracing_subscriber::fmt::FormatFields;
use tracing_subscriber::registry::LookupSpan;

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

/// Custom tracing event formatter that detects peer-related structured fields
/// and formats them with direction indicators.
///
/// Output formats:
/// - `[alice]< stopped`       — status change (peer_name + direction "<")
/// - `[alice]> [0.0s] connect` — command send (peer_name + direction ">")
/// - `[alice]: [warn] msg`     — log entry (peer_name + direction ":")
/// - `[alice] message`         — peer log from peer_monitor (peer field)
/// - `tm: message`             — non-peer message (uses target)
pub struct PeerAwareFormatter {
    ansi: bool,
}

impl PeerAwareFormatter {
    pub fn new(ansi: bool) -> Self {
        Self { ansi }
    }
}

struct FieldVisitor {
    message: String,
    peer: Option<String>,
    peer_name: Option<String>,
    direction: Option<String>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            peer: None,
            peer_name: None,
            direction: None,
        }
    }
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        match field.name() {
            "message" => self.message = format!("{:?}", value),
            "peer" => self.peer = Some(format!("{:?}", value)),
            "peer_name" => self.peer_name = Some(format!("{:?}", value)),
            "direction" => self.direction = Some(format!("{:?}", value)),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "peer" => self.peer = Some(value.to_string()),
            "peer_name" => self.peer_name = Some(value.to_string()),
            "direction" => self.direction = Some(value.to_string()),
            _ => {}
        }
    }
}

impl<S, N> FormatEvent<S, N> for PeerAwareFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        // Timestamp
        let now = chrono::Utc::now();
        write!(writer, "{}", now.format("%Y-%m-%dT%H:%M:%S%.6fZ"))?;

        // Level
        let level = *event.metadata().level();
        write!(writer, " ")?;
        if self.ansi {
            let color = match level {
                tracing::Level::ERROR => "\x1b[31m",
                tracing::Level::WARN => "\x1b[33m",
                tracing::Level::INFO => "\x1b[32m",
                tracing::Level::DEBUG => "\x1b[34m",
                tracing::Level::TRACE => "\x1b[35m",
            };
            write!(writer, "{}{:>5}\x1b[0m", color, level)?;
        } else {
            write!(writer, "{:>5}", level)?;
        }
        write!(writer, " ")?;

        // Extract fields
        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        // Format based on which fields are present
        if let Some(ref peer) = visitor.peer {
            // Peer log from peer_monitor (peer field contains "[alice]" via Display)
            writeln!(writer, "{peer} {}", visitor.message)?;
        } else if let Some(ref peer_name) = visitor.peer_name {
            // Status change or command send from main.rs
            let direction = visitor.direction.as_deref().unwrap_or(":");
            writeln!(writer, "[{peer_name}]{direction} {}", visitor.message)?;
        } else {
            // Non-peer message — use target
            let target = event.metadata().target();
            // Shorten "the_mule::*" to "tm"
            let short_target = if target.starts_with("the_mule") {
                "tm"
            } else {
                target
            };
            writeln!(writer, "{short_target}: {}", visitor.message)?;
        }

        Ok(())
    }
}