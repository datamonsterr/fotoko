//! Argument parsing for the fake CUPS commands. Mirrors the subset of `lp`, `lpstat` and
//! `cancel` that the Pi agent and tests use (see the CUPS man pages).

use thiserror::Error;

/// Argument error; `Display` is the text after the `<command>: ` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliError {
    /// Option needs a value and none follows.
    #[error("Missing value for option \"-{0}\".")]
    MissingValue(char),
    /// Option is not supported.
    #[error("Unknown option \"-{0}\".")]
    UnknownOption(char),
    /// `-n` is not a positive number.
    #[error("Error - copies must be a positive number, got \"{0}\".")]
    InvalidCopies(String),
    /// `-W` is not a known job filter.
    #[error("Unknown argument \"{0}\".")]
    InvalidWhich(String),
    /// Unexpected positional argument.
    #[error("Unexpected argument \"{0}\".")]
    UnexpectedArgument(String),
}

/// One `-o` option: `name` or `name=value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintOption {
    /// Option name.
    pub name: String,
    /// Value, when given.
    pub value: Option<String>,
}

/// Parsed `lp` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpArgs {
    /// `-d`: destination queue.
    pub dest: Option<String>,
    /// `-n`: copies (default 1).
    pub copies: u32,
    /// `-t`: job title.
    pub title: Option<String>,
    /// `-o`: options.
    pub options: Vec<PrintOption>,
    /// `-s`: do not print the request id.
    pub silent: bool,
    /// Files; empty means read stdin.
    pub files: Vec<String>,
}

/// Split `args` into option letters with values and positionals, the way getopt-style CUPS
/// tools do: `-d q`, `-dq`, `-sc`, `--` ends options, a lone `-` is positional.
fn scan(args: &[String], takes_value: &[char], flags: &[char]) -> Result<Vec<Token>, CliError> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut options_done = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if options_done || arg == "-" || !arg.starts_with('-') {
            out.push(Token::Positional(arg.clone()));
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        let letters: Vec<char> = arg.chars().skip(1).collect();
        let mut j = 0;
        while j < letters.len() {
            let c = letters[j];
            j += 1;
            if takes_value.contains(&c) {
                let attached: String = letters[j..].iter().collect();
                let value = if attached.is_empty() {
                    let next = args.get(i).ok_or(CliError::MissingValue(c))?;
                    i += 1;
                    next.clone()
                } else {
                    attached
                };
                out.push(Token::Value(c, value));
                break;
            } else if flags.contains(&c) {
                out.push(Token::Flag(c));
            } else {
                return Err(CliError::UnknownOption(c));
            }
        }
    }
    Ok(out)
}

enum Token {
    Flag(char),
    Value(char, String),
    Positional(String),
}

/// Parse `lp` arguments (without argv[0]).
pub fn parse_lp(args: &[String]) -> Result<LpArgs, CliError> {
    let mut parsed = LpArgs {
        dest: None,
        copies: 1,
        title: None,
        options: Vec::new(),
        silent: false,
        files: Vec::new(),
    };
    // Accepted for compatibility, ignored: -c copy, -m mail, -E encrypt; -q priority,
    // -h host, -H hold, -P pages, -U user, -i job id.
    for token in scan(
        args,
        &['d', 'n', 'o', 't', 'q', 'h', 'H', 'P', 'U', 'i'],
        &['s', 'c', 'm', 'E'],
    )? {
        match token {
            Token::Flag('s') => parsed.silent = true,
            Token::Flag(_) => {}
            Token::Value('d', v) => parsed.dest = Some(v),
            Token::Value('n', v) => {
                parsed.copies = match v.parse::<u32>() {
                    Ok(n) if n >= 1 => n,
                    _ => return Err(CliError::InvalidCopies(v)),
                };
            }
            Token::Value('o', v) => {
                for item in v.split_whitespace() {
                    parsed.options.push(match item.split_once('=') {
                        Some((name, value)) => PrintOption {
                            name: name.to_owned(),
                            value: Some(value.to_owned()),
                        },
                        None => PrintOption {
                            name: item.to_owned(),
                            value: None,
                        },
                    });
                }
            }
            Token::Value('t', v) => parsed.title = Some(v),
            Token::Value(..) => {}
            Token::Positional(p) => parsed.files.push(p),
        }
    }
    Ok(parsed)
}

/// Which jobs `lpstat -W` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// Jobs still in the queue (default).
    NotCompleted,
    /// Finished jobs (printed or aborted).
    Completed,
    /// Jobs that printed.
    Successful,
    /// Everything.
    All,
}

/// Parsed `lpstat` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpstatArgs {
    /// `-o [dests]`: list jobs; `Some(empty)` means all destinations.
    pub jobs: Option<Vec<String>>,
    /// `-p [dests]`: list printers; `Some(empty)` means all.
    pub printers: Option<Vec<String>>,
    /// `-l`: long listing.
    pub long: bool,
    /// `-W which`.
    pub which: Which,
    /// `-r`: show whether the scheduler runs.
    pub scheduler: bool,
}

/// Parse `lpstat` arguments (without argv[0]). `-o` and `-p` take an optional destination list
/// (comma or space separated) from the following non-option arguments. Like CUPS, `-W` must come
/// before `-o`.
pub fn parse_lpstat(args: &[String]) -> Result<LpstatArgs, CliError> {
    let mut parsed = LpstatArgs {
        jobs: None,
        printers: None,
        long: false,
        which: Which::NotCompleted,
        scheduler: false,
    };
    #[derive(Clone, Copy)]
    enum Target {
        Jobs,
        Printers,
    }
    let mut target: Option<Target> = None;
    let mut which = Which::NotCompleted;
    // `o` and `p` take an optional list, so scan them as flags and attach positionals ourselves.
    for token in scan(args, &['W'], &['o', 'p', 'l', 'r'])? {
        match token {
            Token::Flag('o') => {
                parsed.jobs.get_or_insert_with(Vec::new);
                target = Some(Target::Jobs);
                parsed.which = which;
            }
            Token::Flag('p') => {
                parsed.printers.get_or_insert_with(Vec::new);
                target = Some(Target::Printers);
            }
            Token::Flag('l') => parsed.long = true,
            Token::Flag('r') => parsed.scheduler = true,
            Token::Flag(_) => {}
            Token::Value(_, v) => {
                which = match v.as_str() {
                    "not-completed" => Which::NotCompleted,
                    "completed" => Which::Completed,
                    "successful" => Which::Successful,
                    "all" => Which::All,
                    _ => return Err(CliError::InvalidWhich(v)),
                };
            }
            Token::Positional(p) => {
                let list = match target {
                    Some(Target::Jobs) => parsed.jobs.as_mut(),
                    Some(Target::Printers) => parsed.printers.as_mut(),
                    None => None,
                }
                .ok_or_else(|| CliError::UnexpectedArgument(p.clone()))?;
                list.extend(
                    p.split([',', ' '])
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    if parsed.jobs.is_none() && parsed.printers.is_none() && !parsed.scheduler {
        // Like CUPS, plain `lpstat` lists the jobs.
        parsed.jobs = Some(Vec::new());
        parsed.which = which;
    }
    Ok(parsed)
}

/// Parsed `cancel` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelArgs {
    /// `-a`: cancel all jobs of the given destinations (all destinations when none).
    pub all: bool,
    /// Job ids (`queue-12` or `12`) or destinations.
    pub targets: Vec<String>,
}

/// Parse `cancel` arguments (without argv[0]). `-u user` and `-h server` are accepted and ignored.
pub fn parse_cancel(args: &[String]) -> Result<CancelArgs, CliError> {
    let mut parsed = CancelArgs {
        all: false,
        targets: Vec::new(),
    };
    for token in scan(args, &['u', 'h'], &['a', 'E', 'x'])? {
        match token {
            Token::Flag('a') => parsed.all = true,
            Token::Flag(_) | Token::Value(..) => {}
            Token::Positional(p) => parsed.targets.push(p),
        }
    }
    Ok(parsed)
}

/// Split `queue-12` into `("queue", 12)`; a bare number has no queue. Queue names may contain
/// dashes, so the split is at the last one.
pub fn parse_job_ref(text: &str) -> Option<(Option<&str>, u64)> {
    if let Ok(id) = text.parse::<u64>() {
        return Some((None, id));
    }
    let (queue, id) = text.rsplit_once('-')?;
    if queue.is_empty() {
        return None;
    }
    Some((Some(queue), id.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn lp_agent_style_command() {
        let a = parse_lp(&v(&[
            "-d",
            "fotoko",
            "-n",
            "3",
            "-o",
            "media=w288h432",
            "-o",
            "fit-to-page",
            "/tmp/a.jpg",
        ]))
        .unwrap();
        assert_eq!(a.dest.as_deref(), Some("fotoko"));
        assert_eq!(a.copies, 3);
        assert_eq!(
            a.options,
            vec![
                PrintOption {
                    name: "media".into(),
                    value: Some("w288h432".into())
                },
                PrintOption {
                    name: "fit-to-page".into(),
                    value: None
                },
            ]
        );
        assert_eq!(a.files, v(&["/tmp/a.jpg"]));
        assert!(!a.silent);
        assert_eq!(a.title, None);
    }

    #[test]
    fn lp_attached_values_and_combined_flags() {
        let a = parse_lp(&v(&["-dq", "-n2", "-sc", "-tTitle", "-omedia=x y=z", "f"])).unwrap();
        assert_eq!(a.dest.as_deref(), Some("q"));
        assert_eq!(a.copies, 2);
        assert!(a.silent);
        assert_eq!(a.title.as_deref(), Some("Title"));
        assert_eq!(a.options.len(), 2);
        assert_eq!(a.options[1].name, "y");
    }

    #[test]
    fn lp_defaults() {
        let a = parse_lp(&[]).unwrap();
        assert_eq!(a.copies, 1);
        assert_eq!(a.dest, None);
        assert!(a.files.is_empty());
    }

    #[test]
    fn lp_ignored_options_and_double_dash() {
        let a = parse_lp(&v(&["-q", "50", "-m", "--", "-weird-name.jpg", "-"])).unwrap();
        assert_eq!(a.files, v(&["-weird-name.jpg", "-"]));
    }

    #[test]
    fn lp_lone_dash_is_positional() {
        assert_eq!(parse_lp(&v(&["-"])).unwrap().files, v(&["-"]));
    }

    #[test]
    fn lp_errors() {
        assert_eq!(parse_lp(&v(&["-d"])), Err(CliError::MissingValue('d')));
        assert_eq!(parse_lp(&v(&["-z"])), Err(CliError::UnknownOption('z')));
        assert_eq!(
            parse_lp(&v(&["-n", "0"])),
            Err(CliError::InvalidCopies("0".into()))
        );
        assert_eq!(
            parse_lp(&v(&["-n", "x"])),
            Err(CliError::InvalidCopies("x".into()))
        );
    }

    #[test]
    fn lpstat_jobs_for_queue() {
        let a = parse_lpstat(&v(&["-o", "fotoko"])).unwrap();
        assert_eq!(a.jobs, Some(v(&["fotoko"])));
        assert_eq!(a.printers, None);
        assert_eq!(a.which, Which::NotCompleted);
        assert!(!a.long);
    }

    #[test]
    fn lpstat_printer_long_with_list() {
        let a = parse_lpstat(&v(&["-l", "-p", "a,b", "c"])).unwrap();
        assert_eq!(a.printers, Some(v(&["a", "b", "c"])));
        assert!(a.long);
    }

    #[test]
    fn lpstat_attached_flags() {
        let a = parse_lpstat(&v(&["-lp", "q"])).unwrap();
        assert_eq!(a.printers, Some(v(&["q"])));
        assert!(a.long);
    }

    #[test]
    fn lpstat_which_before_jobs() {
        let a = parse_lpstat(&v(&["-W", "completed", "-o", "q"])).unwrap();
        assert_eq!(a.which, Which::Completed);
        for (text, which) in [
            ("all", Which::All),
            ("successful", Which::Successful),
            ("not-completed", Which::NotCompleted),
        ] {
            assert_eq!(parse_lpstat(&v(&["-W", text, "-o"])).unwrap().which, which);
        }
    }

    #[test]
    fn lpstat_which_after_jobs_is_ignored_like_cups() {
        let a = parse_lpstat(&v(&["-o", "q", "-W", "completed"])).unwrap();
        assert_eq!(a.which, Which::NotCompleted);
    }

    #[test]
    fn lpstat_defaults_to_all_jobs() {
        let a = parse_lpstat(&[]).unwrap();
        assert_eq!(a.jobs, Some(vec![]));
        let a = parse_lpstat(&v(&["-W", "all"])).unwrap();
        assert_eq!(a.which, Which::All);
    }

    #[test]
    fn lpstat_scheduler_only() {
        let a = parse_lpstat(&v(&["-r"])).unwrap();
        assert!(a.scheduler);
        assert_eq!(a.jobs, None);
    }

    #[test]
    fn lpstat_errors() {
        assert_eq!(
            parse_lpstat(&v(&["-W", "bogus"])),
            Err(CliError::InvalidWhich("bogus".into()))
        );
        assert_eq!(parse_lpstat(&v(&["-W"])), Err(CliError::MissingValue('W')));
        assert_eq!(parse_lpstat(&v(&["-x"])), Err(CliError::UnknownOption('x')));
        assert_eq!(
            parse_lpstat(&v(&["stray"])),
            Err(CliError::UnexpectedArgument("stray".into()))
        );
    }

    #[test]
    fn cancel_variants() {
        let a = parse_cancel(&v(&["fotoko-12", "7"])).unwrap();
        assert!(!a.all);
        assert_eq!(a.targets, v(&["fotoko-12", "7"]));
        let a = parse_cancel(&v(&["-a", "fotoko"])).unwrap();
        assert!(a.all);
        assert_eq!(a.targets, v(&["fotoko"]));
        let a = parse_cancel(&v(&["-u", "root", "-a"])).unwrap();
        assert!(a.all && a.targets.is_empty());
        assert_eq!(parse_cancel(&v(&["-u"])), Err(CliError::MissingValue('u')));
        assert_eq!(parse_cancel(&v(&["-z"])), Err(CliError::UnknownOption('z')));
    }

    #[test]
    fn job_ref_parsing() {
        assert_eq!(parse_job_ref("fotoko-12"), Some((Some("fotoko"), 12)));
        assert_eq!(parse_job_ref("my-queue-3"), Some((Some("my-queue"), 3)));
        assert_eq!(parse_job_ref("12"), Some((None, 12)));
        assert_eq!(parse_job_ref("fotoko"), None);
        assert_eq!(parse_job_ref("-3"), None);
        assert_eq!(parse_job_ref("q-x"), None);
    }
}
