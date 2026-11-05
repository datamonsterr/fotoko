//! `backend = "sim"`: print through [`fotoko_printer_sim::SimPrinter`].

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use fotoko_printer_sim::{Fault, SimConfig, SimError, SimPrinter};
use fotoko_protocol::{Media, PrinterState};

use crate::config::SimSection;
use crate::printer::{PrintContext, PrintError, Printer};

/// Printer backed by the in-process simulation.
#[derive(Debug)]
pub struct SimBackend {
    printer: Arc<SimPrinter>,
    active: AtomicU32,
}

impl SimBackend {
    /// Wrap a simulated printer (tests keep their own handle to flip faults).
    pub fn new(printer: Arc<SimPrinter>) -> Self {
        Self {
            printer,
            active: AtomicU32::new(0),
        }
    }

    /// Build from the `[sim]` config section.
    pub fn from_config(section: &SimSection) -> Result<Self, PrintError> {
        let mut config = SimConfig::new(&section.out_dir);
        config.paper = section.paper;
        config.fault = section.fail;
        config.latency = Duration::from_millis(section.latency_ms);
        let printer = SimPrinter::new(config).map_err(map_error)?;
        Ok(Self::new(Arc::new(printer)))
    }
}

/// Simulation error to Pi error.
pub fn map_error(error: SimError) -> PrintError {
    match error {
        SimError::Offline => PrintError::Offline("simulated printer is offline".to_owned()),
        SimError::RibbonOut => PrintError::RibbonOut,
        SimError::PaperOut => PrintError::PaperOut,
        SimError::DriverError => PrintError::Driver("simulated driver error".to_owned()),
        SimError::InvalidImage(message) => PrintError::InvalidImage(message),
        SimError::Io { .. } => PrintError::Driver(error.to_string()),
    }
}

/// Counts a page in progress, also when the print future is dropped.
struct ActiveGuard<'a>(&'a AtomicU32);

impl<'a> ActiveGuard<'a> {
    fn new(counter: &'a AtomicU32) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Printer for SimBackend {
    async fn print(
        &self,
        file: &Path,
        copies: u32,
        _media: Media,
        ctx: &PrintContext,
    ) -> Result<(), PrintError> {
        let printer = Arc::clone(&self.printer);
        let file = file.to_owned();
        let _active = ActiveGuard::new(&self.active);
        let work = tokio::task::spawn_blocking(move || printer.print_file(&file, copies));
        // A blocking page cannot be interrupted; it finishes in the background.
        let result = tokio::select! {
            () = ctx.cancel.cancelled() => return Err(PrintError::Cancelled),
            result = work => result,
        };
        match result {
            Ok(Ok(_pages)) => Ok(()),
            Ok(Err(error)) => Err(map_error(error)),
            Err(join) => Err(PrintError::Driver(format!("print task failed: {join}"))),
        }
    }

    async fn state(&self) -> PrinterState {
        let state = self.printer.state();
        match state.fault {
            Fault::Offline => PrinterState::Offline,
            Fault::RibbonOut | Fault::DriverError => PrinterState::Error,
            Fault::None if state.paper_remaining == 0 => PrinterState::Error,
            Fault::None if self.active.load(Ordering::SeqCst) > 0 => PrinterState::Printing,
            Fault::None => PrinterState::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::path::PathBuf;

    fn image(dir: &Path) -> PathBuf {
        let path = dir.join("j-p.png");
        RgbImage::from_pixel(30, 20, Rgb([1, 2, 3]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        path
    }

    fn ctx() -> PrintContext {
        PrintContext::new("t")
    }

    fn backend(dir: &Path, paper: u32) -> (SimBackend, Arc<SimPrinter>) {
        let mut cfg = SimConfig::new(dir.join("out"));
        cfg.paper = paper;
        let sim = Arc::new(SimPrinter::new(cfg).unwrap());
        (SimBackend::new(Arc::clone(&sim)), sim)
    }

    #[tokio::test]
    async fn prints_and_reports_idle() {
        let dir = tempfile::tempdir().unwrap();
        let (b, sim) = backend(dir.path(), 5);
        assert_eq!(b.state().await, PrinterState::Idle);
        b.print(&image(dir.path()), 2, Media::Photo4x6, &ctx())
            .await
            .unwrap();
        assert_eq!(sim.state().paper_remaining, 3);
        assert!(sim.out_dir().join("j-p-2.png").exists());
    }

    #[tokio::test]
    async fn state_per_fault() {
        let dir = tempfile::tempdir().unwrap();
        let (b, sim) = backend(dir.path(), 5);
        for (fault, expected) in [
            (Fault::Offline, PrinterState::Offline),
            (Fault::RibbonOut, PrinterState::Error),
            (Fault::DriverError, PrinterState::Error),
            (Fault::None, PrinterState::Idle),
        ] {
            sim.set_fault(fault);
            assert_eq!(b.state().await, expected);
        }
        sim.set_paper(0);
        assert_eq!(b.state().await, PrinterState::Error);
    }

    #[tokio::test]
    async fn state_is_printing_while_a_page_is_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SimConfig::new(dir.path().join("out"));
        cfg.latency = Duration::from_millis(300);
        let sim = Arc::new(SimPrinter::new(cfg).unwrap());
        let b = Arc::new(SimBackend::new(sim));
        let file = image(dir.path());
        let running = {
            let b = Arc::clone(&b);
            tokio::spawn(async move { b.print(&file, 1, Media::Photo4x6, &ctx()).await })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while b.state().await != PrinterState::Printing {
            assert!(std::time::Instant::now() < deadline, "never printing");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        running.await.unwrap().unwrap();
        assert_eq!(b.state().await, PrinterState::Idle);
    }

    #[tokio::test]
    async fn cancel_returns_promptly_and_dropping_clears_the_printing_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SimConfig::new(dir.path().join("out"));
        cfg.latency = Duration::from_secs(2);
        let b = Arc::new(SimBackend::new(Arc::new(SimPrinter::new(cfg).unwrap())));
        let file = image(dir.path());
        let ctx = PrintContext::new("t");
        let (b2, f2, c2) = (Arc::clone(&b), file.clone(), ctx.clone());
        let task = tokio::spawn(async move { b2.print(&f2, 1, Media::Photo4x6, &c2).await });
        while b.state().await != PrinterState::Printing {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        ctx.cancel.cancel();
        let started = std::time::Instant::now();
        assert_eq!(task.await.unwrap(), Err(PrintError::Cancelled));
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert_eq!(b.state().await, PrinterState::Idle);
        // Dropping an in-flight print (task abort) also clears the state.
        let (b3, f3) = (Arc::clone(&b), file);
        let task = tokio::spawn(async move { b3.print(&f3, 1, Media::Photo4x6, &ctx_new()).await });
        while b.state().await != PrinterState::Printing {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        let _ = task.await;
        assert_eq!(b.state().await, PrinterState::Idle);
    }

    fn ctx_new() -> PrintContext {
        PrintContext::new("t2")
    }

    #[tokio::test]
    async fn errors_map_to_print_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (b, sim) = backend(dir.path(), 1);
        let file = image(dir.path());
        assert_eq!(
            b.print(&file, 2, Media::Photo4x6, &ctx()).await,
            Err(PrintError::PaperOut)
        );
        sim.set_paper(5);
        sim.set_fault(Fault::Offline);
        assert!(matches!(
            b.print(&file, 1, Media::Photo4x6, &ctx()).await,
            Err(PrintError::Offline(_))
        ));
        sim.set_fault(Fault::RibbonOut);
        assert_eq!(
            b.print(&file, 1, Media::Photo4x6, &ctx()).await,
            Err(PrintError::RibbonOut)
        );
        sim.set_fault(Fault::DriverError);
        assert!(matches!(
            b.print(&file, 1, Media::Photo4x6, &ctx()).await,
            Err(PrintError::Driver(_))
        ));
        sim.set_fault(Fault::None);
        let bad = dir.path().join("bad.jpg");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(matches!(
            b.print(&bad, 1, Media::Photo4x6, &ctx()).await,
            Err(PrintError::InvalidImage(_))
        ));
        let missing = dir.path().join("missing.png");
        assert!(matches!(
            b.print(&missing, 1, Media::Photo4x6, &ctx()).await,
            Err(PrintError::Driver(_))
        ));
    }

    #[tokio::test]
    async fn from_config_applies_settings() {
        let dir = tempfile::tempdir().unwrap();
        let section = SimSection {
            out_dir: dir.path().join("o"),
            paper: 7,
            fail: Fault::RibbonOut,
            latency_ms: 0,
        };
        let b = SimBackend::from_config(&section).unwrap();
        assert_eq!(b.state().await, PrinterState::Error);
        let blocker = dir.path().join("f");
        std::fs::write(&blocker, "x").unwrap();
        let bad = SimSection {
            out_dir: blocker.join("o"),
            ..section
        };
        assert!(matches!(
            SimBackend::from_config(&bad),
            Err(PrintError::Driver(_))
        ));
    }
}
