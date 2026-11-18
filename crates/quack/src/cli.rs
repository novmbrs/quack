//! The command line (§17.1). Hand-parsed: four commands and a few flags.

use std::path::PathBuf;

/// What to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `quack serve [--migrate]`.
    Serve {
        /// Apply pending migrations first.
        migrate: bool,
    },
    /// `quack migrate`.
    Migrate,
    /// `quack import-mysql [--dry-run] [--report <file>]`.
    ImportMysql {
        /// Check without writing.
        dry_run: bool,
        /// Where to write the validation report.
        report: Option<PathBuf>,
    },
    /// `quack commands sync`.
    CommandsSync,
    /// `quack --version`.
    Version,
    /// `quack --help`.
    Help,
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cli {
    /// What to run.
    pub command: Command,
    /// `--config <file>`.
    pub config: Option<PathBuf>,
}

/// The usage text.
pub const USAGE: &str = "\
Usage:
  quack serve [--config <file>] [--migrate]
  quack migrate [--config <file>]
  quack import-mysql [--dry-run] [--report <file>] [--config <file>]
  quack commands sync [--config <file>]
  quack --version

Configuration comes from QUACK_* environment variables and the optional
TOML file (the same names in lowercase, without the prefix).";

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Cli, String> {
    let mut words = Vec::new();
    let mut config = None;
    let mut migrate = false;
    let mut dry_run = false;
    let mut report = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--version" | "-V" => {
                return Ok(Cli {
                    command: Command::Version,
                    config: None,
                });
            }
            "--help" | "-h" => {
                return Ok(Cli {
                    command: Command::Help,
                    config: None,
                });
            }
            "--config" => config = Some(PathBuf::from(it.next().ok_or("--config needs a file")?)),
            "--migrate" => migrate = true,
            "--dry-run" => dry_run = true,
            "--report" => report = Some(PathBuf::from(it.next().ok_or("--report needs a file")?)),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            word => words.push(word),
        }
    }
    let import_flags = dry_run || report.is_some();
    let command = match words.as_slice() {
        ["serve"] => Command::Serve { migrate },
        ["migrate"] => Command::Migrate,
        ["import-mysql"] => Command::ImportMysql { dry_run, report },
        ["commands", "sync"] => Command::CommandsSync,
        [] => return Err("no command given".to_owned()),
        other => return Err(format!("unknown command `{}`", other.join(" "))),
    };
    let misplaced = (migrate && !matches!(command, Command::Serve { .. }))
        || (import_flags && !matches!(command, Command::ImportMysql { .. }));
    if misplaced {
        return Err("an option was given to a command that doesn't take it".to_owned());
    }
    Ok(Cli { command, config })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn reads_every_command() {
        assert_eq!(
            parse(&args("serve --migrate")).unwrap().command,
            Command::Serve { migrate: true }
        );
        assert_eq!(
            parse(&args("migrate --config q.toml")).unwrap(),
            Cli {
                command: Command::Migrate,
                config: Some(PathBuf::from("q.toml"))
            }
        );
        assert_eq!(
            parse(&args("commands sync")).unwrap().command,
            Command::CommandsSync
        );
        assert_eq!(
            parse(&args("import-mysql --dry-run --report r.json"))
                .unwrap()
                .command,
            Command::ImportMysql {
                dry_run: true,
                report: Some(PathBuf::from("r.json"))
            }
        );
        assert_eq!(parse(&args("--version")).unwrap().command, Command::Version);
    }

    #[test]
    fn refuses_mistakes() {
        assert!(parse(&args("")).is_err());
        assert!(parse(&args("serve --bogus")).is_err());
        assert!(parse(&args("migrate --migrate")).is_err());
        assert!(parse(&args("serve --config")).is_err());
        assert!(parse(&args("commands")).is_err());
    }
}
