//! `SimPrinter`: an in-process stand-in for a Gutenprint printer on a CUPS queue.

use std::fmt;
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::fit::fit_to_page;

/// Largest image edge the simulation decodes.
const MAX_EDGE_PX: u32 = 20_000;
/// Decoder allocation cap (bytes).
const MAX_ALLOC_BYTES: u64 = 512 * 1024 * 1024;

/// Injected printer fault.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Fault {
    /// Healthy.
    #[default]
    None,
    /// Printer unplugged or unreachable.
    Offline,
    /// Ribbon or ink exhausted.
    RibbonOut,
    /// CUPS filter or driver failure.
    DriverError,
}

impl Fault {
    /// Config and file spelling: `""`, `offline`, `ribbon_out`, `driver_error`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Offline => "offline",
            Self::RibbonOut => "ribbon_out",
            Self::DriverError => "driver_error",
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Not a known fault name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown fault {0:?}, expected \"\", offline, ribbon_out or driver_error")]
pub struct ParseFaultError(pub String);

impl FromStr for Fault {
    type Err = ParseFaultError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "" | "none" => Ok(Self::None),
            "offline" => Ok(Self::Offline),
            "ribbon_out" => Ok(Self::RibbonOut),
            "driver_error" => Ok(Self::DriverError),
            other => Err(ParseFaultError(other.to_owned())),
        }
    }
}

impl TryFrom<String> for Fault {
    type Error = ParseFaultError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Fault> for String {
    fn from(value: Fault) -> Self {
        value.as_str().to_owned()
    }
}

/// Why a simulated print failed.
#[derive(Debug, Error)]
pub enum SimError {
    /// Printer offline.
    #[error("printer is offline")]
    Offline,
    /// Ribbon exhausted.
    #[error("printer is out of ribbon")]
    RibbonOut,
    /// Paper exhausted.
    #[error("printer is out of paper")]
    PaperOut,
    /// Driver failure.
    #[error("simulated driver error")]
    DriverError,
    /// The input is not a decodable JPEG or PNG.
    #[error("invalid image: {0}")]
    InvalidImage(String),
    /// Filesystem failure.
    #[error("io error on {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        #[source]
        source: io::Error,
    },
}

impl SimError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// `SimPrinter` settings.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Where rendered pages are written.
    pub out_dir: PathBuf,
    /// Sheets loaded; `paper_out` once exhausted.
    pub paper: u32,
    /// Initial fault.
    pub fault: Fault,
    /// Time each page takes.
    pub latency: Duration,
}

impl SimConfig {
    /// Defaults: 50 sheets, no fault, no latency.
    pub fn new(out_dir: impl Into<PathBuf>) -> Self {
        Self {
            out_dir: out_dir.into(),
            paper: 50,
            fault: Fault::None,
            latency: Duration::ZERO,
        }
    }
}

/// Snapshot of the simulated printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimState {
    /// Sheets left.
    pub paper_remaining: u32,
    /// Current fault.
    pub fault: Fault,
}

#[derive(Debug)]
struct Inner {
    paper: u32,
    fault: Fault,
}

/// Simulated photo printer. Decodes JPEG/PNG, fits it to 4x6 @300dpi and writes one PNG per copy.
#[derive(Debug)]
pub struct SimPrinter {
    out_dir: PathBuf,
    latency: Duration,
    inner: Mutex<Inner>,
}

/// Decode JPEG or PNG bytes with size limits.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, SimError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| SimError::InvalidImage(e.to_string()))?;
    match reader.format() {
        Some(ImageFormat::Jpeg | ImageFormat::Png) => {}
        other => {
            return Err(SimError::InvalidImage(format!(
                "unsupported format {other:?}"
            )));
        }
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_EDGE_PX);
    limits.max_image_height = Some(MAX_EDGE_PX);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|e| SimError::InvalidImage(e.to_string()))
}

/// What the header of an image file says, without decoding the pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `image/jpeg` or `image/png`.
    pub mime: &'static str,
}

/// Read an image file's format and size from its header. Rejects anything but JPEG and PNG and
/// sizes the simulation would refuse to decode.
pub fn probe_file(path: &Path) -> Result<ImageInfo, SimError> {
    let reader = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|e| SimError::io(path, e))?;
    let mime = match reader.format() {
        Some(ImageFormat::Jpeg) => "image/jpeg",
        Some(ImageFormat::Png) => "image/png",
        other => {
            return Err(SimError::InvalidImage(format!(
                "unsupported format {other:?}"
            )));
        }
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| SimError::InvalidImage(e.to_string()))?;
    if width == 0 || height == 0 || width > MAX_EDGE_PX || height > MAX_EDGE_PX {
        return Err(SimError::InvalidImage(format!(
            "unsupported size {width}x{height}"
        )));
    }
    Ok(ImageInfo {
        width,
        height,
        mime,
    })
}

impl SimPrinter {
    /// Create the printer and its output directory.
    pub fn new(config: SimConfig) -> Result<Self, SimError> {
        fs::create_dir_all(&config.out_dir).map_err(|e| SimError::io(&config.out_dir, e))?;
        Ok(Self {
            out_dir: config.out_dir,
            latency: config.latency,
            inner: Mutex::new(Inner {
                paper: config.paper,
                fault: config.fault,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // The guarded data is two plain values, so a poisoned lock is still consistent.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Output directory.
    pub fn out_dir(&self) -> &Path {
        &self.out_dir
    }

    /// Current paper and fault.
    pub fn state(&self) -> SimState {
        let inner = self.lock();
        SimState {
            paper_remaining: inner.paper,
            fault: inner.fault,
        }
    }

    /// Inject or clear a fault.
    pub fn set_fault(&self, fault: Fault) {
        self.lock().fault = fault;
    }

    /// Load `sheets` of paper (replaces the counter).
    pub fn set_paper(&self, sheets: u32) {
        self.lock().paper = sheets;
    }

    fn preflight(&self) -> Result<(), SimError> {
        let inner = self.lock();
        match inner.fault {
            Fault::Offline => Err(SimError::Offline),
            Fault::RibbonOut => Err(SimError::RibbonOut),
            Fault::DriverError => Err(SimError::DriverError),
            Fault::None if inner.paper == 0 => Err(SimError::PaperOut),
            Fault::None => Ok(()),
        }
    }

    /// Print `copies` copies of the image file. Pages are `{stem}-{n}.png` (n from 1) in the
    /// output directory, where `stem` is the input's file stem, e.g. `{jobId}-{photoId}`.
    /// Returns the written paths. On error, copies already printed stay on disk and keep
    /// consuming paper.
    pub fn print_file(&self, input: &Path, copies: u32) -> Result<Vec<PathBuf>, SimError> {
        let stem = stem_of(input);
        self.preflight()?;
        let page = load_page(input)?;
        let mut written = Vec::new();
        for index in 1..=copies {
            written.push(self.emit(&page, &stem, index)?);
        }
        Ok(written)
    }

    /// Print a single copy as `{stem}-{index}.png`.
    pub fn print_copy(&self, input: &Path, stem: &str, index: u32) -> Result<PathBuf, SimError> {
        self.preflight()?;
        let page = load_page(input)?;
        self.emit(&page, stem, index)
    }

    fn emit(&self, page: &image::RgbImage, stem: &str, index: u32) -> Result<PathBuf, SimError> {
        self.preflight()?;
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }
        let path = self.out_dir.join(format!("{stem}-{index}.png"));
        let tmp = self.out_dir.join(format!(".{stem}-{index}.png.tmp"));
        page.save_with_format(&tmp, ImageFormat::Png)
            .map_err(|e| SimError::io(&tmp, io::Error::other(e.to_string())))?;
        fs::rename(&tmp, &path).map_err(|e| SimError::io(&path, e))?;
        let mut inner = self.lock();
        inner.paper = inner.paper.saturating_sub(1);
        Ok(path)
    }
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "page".to_owned(), |s| s.to_string_lossy().into_owned())
}

fn load_page(input: &Path) -> Result<image::RgbImage, SimError> {
    let bytes = fs::read(input).map_err(|e| SimError::io(input, e))?;
    Ok(fit_to_page(&decode(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn write_image(dir: &Path, name: &str, w: u32, h: u32, format: ImageFormat) -> PathBuf {
        let path = dir.join(name);
        RgbImage::from_pixel(w, h, Rgb([10, 120, 200]))
            .save_with_format(&path, format)
            .unwrap();
        path
    }

    fn sim(dir: &Path, paper: u32) -> SimPrinter {
        let mut cfg = SimConfig::new(dir.join("out"));
        cfg.paper = paper;
        SimPrinter::new(cfg).unwrap()
    }

    #[test]
    fn fault_parse_and_display_roundtrip() {
        for (text, fault) in [
            ("", Fault::None),
            ("none", Fault::None),
            ("offline", Fault::Offline),
            ("ribbon_out", Fault::RibbonOut),
            ("driver_error", Fault::DriverError),
        ] {
            assert_eq!(text.parse::<Fault>().unwrap(), fault);
        }
        assert_eq!(" offline\n".parse::<Fault>().unwrap(), Fault::Offline);
        assert_eq!(Fault::RibbonOut.to_string(), "ribbon_out");
        assert_eq!(Fault::None.to_string(), "");
        assert_eq!(
            "paper".parse::<Fault>().unwrap_err(),
            ParseFaultError("paper".into())
        );
    }

    #[test]
    fn fault_serde_uses_strings() {
        assert_eq!(
            serde_json::to_string(&Fault::DriverError).unwrap(),
            "\"driver_error\""
        );
        assert_eq!(serde_json::from_str::<Fault>("\"\"").unwrap(), Fault::None);
        assert!(serde_json::from_str::<Fault>("\"bogus\"").is_err());
    }

    #[test]
    fn prints_landscape_jpeg_copies_with_deterministic_names() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "job1-ph1.jpg", 90, 60, ImageFormat::Jpeg);
        let printer = sim(dir.path(), 5);
        let pages = printer.print_file(&input, 2).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0], printer.out_dir().join("job1-ph1-1.png"));
        assert_eq!(pages[1], printer.out_dir().join("job1-ph1-2.png"));
        let page = image::open(&pages[0]).unwrap();
        assert_eq!((page.width(), page.height()), (1800, 1200));
        assert_eq!(printer.state().paper_remaining, 3);
    }

    #[test]
    fn prints_portrait_png_as_portrait_page() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "a-b.png", 40, 80, ImageFormat::Png);
        let printer = sim(dir.path(), 5);
        let pages = printer.print_file(&input, 1).unwrap();
        let page = image::open(&pages[0]).unwrap();
        assert_eq!((page.width(), page.height()), (1200, 1800));
    }

    #[test]
    fn paper_runs_out_mid_job() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "j-p.png", 30, 20, ImageFormat::Png);
        let printer = sim(dir.path(), 2);
        let err = printer.print_file(&input, 3).unwrap_err();
        assert!(matches!(err, SimError::PaperOut));
        assert!(printer.out_dir().join("j-p-2.png").exists());
        assert!(!printer.out_dir().join("j-p-3.png").exists());
        assert_eq!(printer.state().paper_remaining, 0);
        printer.set_paper(1);
        assert_eq!(printer.state().paper_remaining, 1);
        assert_eq!(printer.print_file(&input, 1).unwrap().len(), 1);
    }

    #[test]
    fn zero_paper_fails_before_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let printer = sim(dir.path(), 0);
        let err = printer
            .print_file(&dir.path().join("missing.png"), 1)
            .unwrap_err();
        assert!(matches!(err, SimError::PaperOut));
    }

    #[test]
    fn faults_map_to_errors_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "j-p.png", 30, 20, ImageFormat::Png);
        let printer = sim(dir.path(), 5);
        printer.set_fault(Fault::Offline);
        assert!(matches!(
            printer.print_file(&input, 1),
            Err(SimError::Offline)
        ));
        printer.set_fault(Fault::RibbonOut);
        assert!(matches!(
            printer.print_file(&input, 1),
            Err(SimError::RibbonOut)
        ));
        printer.set_fault(Fault::DriverError);
        assert!(matches!(
            printer.print_file(&input, 1),
            Err(SimError::DriverError)
        ));
        assert_eq!(printer.state().fault, Fault::DriverError);
        assert_eq!(printer.state().paper_remaining, 5);
        printer.set_fault(Fault::None);
        assert_eq!(printer.print_file(&input, 1).unwrap().len(), 1);
    }

    #[test]
    fn initial_fault_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SimConfig::new(dir.path().join("out"));
        cfg.fault = Fault::Offline;
        let printer = SimPrinter::new(cfg).unwrap();
        assert_eq!(printer.state().fault, Fault::Offline);
    }

    #[test]
    fn garbage_input_is_invalid_image() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("j-p.jpg");
        fs::write(&input, b"definitely not an image").unwrap();
        let printer = sim(dir.path(), 5);
        assert!(matches!(
            printer.print_file(&input, 1),
            Err(SimError::InvalidImage(_))
        ));
        assert_eq!(printer.state().paper_remaining, 5);
    }

    #[test]
    fn unsupported_format_is_invalid_image() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("j-p.gif");
        fs::write(&input, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert!(matches!(
            sim(dir.path(), 5).print_file(&input, 1),
            Err(SimError::InvalidImage(_))
        ));
    }

    #[test]
    fn truncated_png_is_invalid_image() {
        let dir = tempfile::tempdir().unwrap();
        let full = write_image(dir.path(), "full.png", 30, 20, ImageFormat::Png);
        let bytes = fs::read(&full).unwrap();
        let input = dir.path().join("j-p.png");
        fs::write(&input, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            sim(dir.path(), 5).print_file(&input, 1),
            Err(SimError::InvalidImage(_))
        ));
    }

    #[test]
    fn missing_input_is_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = sim(dir.path(), 5)
            .print_file(&dir.path().join("nope.png"), 1)
            .unwrap_err();
        assert!(matches!(err, SimError::Io { .. }));
        assert!(err.to_string().contains("nope.png"));
    }

    #[test]
    fn new_fails_when_out_dir_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let err = SimPrinter::new(SimConfig::new(blocker.join("out"))).unwrap_err();
        assert!(matches!(err, SimError::Io { .. }));
    }

    #[test]
    fn latency_delays_each_page() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "j-p.png", 30, 20, ImageFormat::Png);
        let mut cfg = SimConfig::new(dir.path().join("out"));
        cfg.latency = Duration::from_millis(40);
        let printer = SimPrinter::new(cfg).unwrap();
        let start = std::time::Instant::now();
        printer.print_file(&input, 2).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(80));
    }

    #[test]
    fn print_copy_uses_given_stem_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let input = write_image(dir.path(), "spool.dat", 30, 20, ImageFormat::Png);
        let printer = sim(dir.path(), 5);
        let path = printer.print_copy(&input, "job-9", 3).unwrap();
        assert_eq!(path, printer.out_dir().join("job-9-3.png"));
        assert_eq!(printer.state().paper_remaining, 4);
    }

    #[test]
    fn probe_reports_format_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let jpg = write_image(dir.path(), "a.jpg", 30, 20, ImageFormat::Jpeg);
        let png = write_image(dir.path(), "a.png", 20, 30, ImageFormat::Png);
        assert_eq!(
            probe_file(&jpg).unwrap(),
            ImageInfo {
                width: 30,
                height: 20,
                mime: "image/jpeg"
            }
        );
        assert_eq!(
            probe_file(&png).unwrap(),
            ImageInfo {
                width: 20,
                height: 30,
                mime: "image/png"
            }
        );
    }

    #[test]
    fn probe_rejects_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let garbage = dir.path().join("g.jpg");
        fs::write(&garbage, b"nope nope nope").unwrap();
        assert!(matches!(
            probe_file(&garbage),
            Err(SimError::InvalidImage(_))
        ));
        let gif = dir.path().join("a.gif");
        fs::write(&gif, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert!(matches!(probe_file(&gif), Err(SimError::InvalidImage(_))));
        assert!(matches!(
            probe_file(&dir.path().join("none")),
            Err(SimError::Io { .. })
        ));
        // PNG signature and header claiming a corrupt (zero-sized) image.
        let png = write_image(dir.path(), "ok.png", 4, 4, ImageFormat::Png);
        let mut bytes = fs::read(&png).unwrap();
        bytes[16..24].fill(0); // width and height = 0
        let zero = dir.path().join("zero.png");
        fs::write(&zero, bytes).unwrap();
        assert!(matches!(probe_file(&zero), Err(SimError::InvalidImage(_))));
    }

    #[test]
    fn probe_rejects_huge_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let png = write_image(dir.path(), "ok.png", 4, 4, ImageFormat::Png);
        let mut bytes = fs::read(&png).unwrap();
        bytes[16..20].copy_from_slice(&(MAX_EDGE_PX + 1).to_be_bytes());
        let huge = dir.path().join("huge.png");
        fs::write(&huge, bytes).unwrap();
        assert!(matches!(probe_file(&huge), Err(SimError::InvalidImage(_))));
    }

    #[test]
    fn stem_of_handles_odd_paths() {
        assert_eq!(stem_of(Path::new("/x/job-ph.jpg")), "job-ph");
        assert_eq!(stem_of(Path::new("/")), "page");
    }
}
