//! Entry point shared by the `fake-lp`, `fake-lpstat` and `fake-cancel` binaries.

use std::io::{Read, Write};

use crate::fake_cups::{FakeCups, FakeCupsEnv, Outcome};

/// Which fake command to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `lp`.
    Lp,
    /// `lpstat`.
    Lpstat,
    /// `cancel`.
    Cancel,
}

impl Command {
    const fn name(self) -> &'static str {
        match self {
            Self::Lp => "lp",
            Self::Lpstat => "lpstat",
            Self::Cancel => "cancel",
        }
    }
}

/// Run one command and return its exit code.
pub fn run(
    command: Command,
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let outcome = match FakeCupsEnv::from_lookup(env) {
        Ok(env) => {
            let cups = FakeCups::new(env);
            match command {
                Command::Lp => cups.lp(args, stdin),
                Command::Lpstat => cups.lpstat(args),
                Command::Cancel => cups.cancel(args),
            }
        }
        Err(message) => Outcome::fail(command.name(), message),
    };
    // A closed pipe on the other side is not our failure to report.
    let _ = stdout.write_all(outcome.stdout.as_bytes());
    let _ = stderr.write_all(outcome.stderr.as_bytes());
    outcome.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::empty;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn run_dispatches_and_reports_env_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = {
            let p = dir.path().join("j-p.png");
            RgbImage::from_pixel(30, 20, Rgb([1, 2, 3]))
                .save_with_format(&p, ImageFormat::Png)
                .unwrap();
            p.to_string_lossy().into_owned()
        };
        let cups_dir = dir.path().join("cups").to_string_lossy().into_owned();
        let env = |k: &str| (k == "FAKE_CUPS_DIR").then(|| cups_dir.clone());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            Command::Lp,
            &v(&["-d", "q", &file]),
            env,
            &mut empty(),
            &mut out,
            &mut err,
        );
        assert_eq!(
            (code, String::from_utf8(out).unwrap()),
            (0, "request id is q-1 (1 file(s))\n".into())
        );
        let (mut out, mut err2) = (Vec::new(), Vec::new());
        assert_eq!(
            run(
                Command::Lpstat,
                &v(&["-o"]),
                env,
                &mut empty(),
                &mut out,
                &mut err2
            ),
            0
        );
        assert!(out.is_empty());
        let mut out = Vec::new();
        assert_eq!(
            run(
                Command::Cancel,
                &v(&["-a"]),
                env,
                &mut empty(),
                &mut out,
                &mut err2
            ),
            0
        );
        let code = run(
            Command::Lpstat,
            &[],
            |_| None,
            &mut empty(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "lpstat: FAKE_CUPS_DIR is not set\n"
        );
    }
}
