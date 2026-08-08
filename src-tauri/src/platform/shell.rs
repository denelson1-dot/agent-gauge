//! Shell quoting for the command string Agent Gauge writes into Claude Code's
//! settings, plus the shell used to run a previously configured status line.
//!
//! Claude Code invokes a status line by handing the configured string to a
//! shell, so what we store in `settings.json` is shell source, not an argv.
//! That makes quoting a correctness boundary rather than a formatting detail:
//! a user whose account name contains a space is the common case, and getting
//! it wrong writes a broken command into a file we do not own.
//!
//! Both quoting implementations are compiled and tested on both platforms on
//! purpose. Windows behaviour is otherwise only exercised on hardware that gets
//! booted rarely, and a quoting rule is pure string logic with no reason to be
//! unverifiable from a Linux checkout.

use std::path::Path;

/// Quotes one argument for a POSIX `sh -c` command line.
///
/// Single quotes suppress every expansion `sh` performs, so the only character
/// needing care is the single quote itself: close the literal, emit an escaped
/// quote, reopen.
///
/// Compiled on every target so its behaviour stays testable from either side.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn quote_posix(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', r"'\''"))
}

/// Quotes one argument for a `cmd.exe /C` command line.
///
/// `cmd` is materially weaker than `sh` here. Double quotes group an argument
/// containing spaces, but they do not suppress environment expansion, and
/// `cmd` offers no in-quote escape for `%` on the command line (`%%` only works
/// inside a batch file). Rather than emit a string that would misbehave, refuse
/// the two characters we cannot represent faithfully.
///
/// Neither is reachable through a normal install — `"` is not a legal character
/// in a Windows path, and `%` in an executable path requires a deliberately
/// exotic profile name — so refusing costs nothing and beats guessing.
///
/// Compiled on every target so its behaviour stays testable from Linux, where
/// this port is actually developed.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn quote_cmd(argument: &str) -> Result<String, String> {
    if argument.contains('"') {
        return Err(format!(
            "{argument} contains a double quote, which cannot be represented in a cmd.exe command"
        ));
    }
    if argument.contains('%') {
        return Err(format!(
            "{argument} contains a percent sign, which cmd.exe would expand as an environment variable"
        ));
    }
    Ok(format!("\"{argument}\""))
}

/// Builds a shell command string that runs `program` with `args` under the
/// host's shell, quoted for that shell.
pub fn command_line(program: &Path, args: &[&str]) -> Result<String, String> {
    let program = program
        .to_str()
        .ok_or_else(|| format!("{} is not valid Unicode", program.display()))?;

    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_for_host(program)?);
    for argument in args {
        parts.push(quote_for_host(argument)?);
    }
    Ok(parts.join(" "))
}

#[cfg(target_os = "windows")]
fn quote_for_host(argument: &str) -> Result<String, String> {
    quote_cmd(argument)
}

#[cfg(not(target_os = "windows"))]
fn quote_for_host(argument: &str) -> Result<String, String> {
    Ok(quote_posix(argument))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quoting_survives_spaces_and_expansions() {
        assert_eq!(
            quote_posix("/usr/bin/agent-gauge"),
            "'/usr/bin/agent-gauge'"
        );
        assert_eq!(quote_posix("/home/a b/app"), "'/home/a b/app'");
        // The characters that would otherwise be interpreted by `sh`.
        assert_eq!(quote_posix("/tmp/$HOME"), "'/tmp/$HOME'");
        assert_eq!(quote_posix("/tmp/`whoami`"), "'/tmp/`whoami`'");
        assert_eq!(quote_posix("/tmp/a;rm -rf /"), "'/tmp/a;rm -rf /'");
    }

    #[test]
    fn posix_quoting_closes_and_reopens_around_a_single_quote() {
        assert_eq!(quote_posix("/tmp/o'brien/app"), r"'/tmp/o'\''brien/app'");
    }

    #[test]
    fn cmd_quoting_wraps_paths_containing_spaces() {
        assert_eq!(
            quote_cmd(r"C:\Program Files\Agent Gauge\agent-gauge.exe").unwrap(),
            r#""C:\Program Files\Agent Gauge\agent-gauge.exe""#
        );
        assert_eq!(
            quote_cmd("--capture-claude").unwrap(),
            r#""--capture-claude""#
        );
    }

    #[test]
    fn cmd_quoting_refuses_what_it_cannot_represent() {
        // Rather than emit a command that cmd.exe would mangle at runtime.
        assert!(quote_cmd(r"C:\odd%USERNAME%\app.exe").is_err());
        assert!(quote_cmd(r#"C:\a"b\app.exe"#).is_err());
    }

    #[test]
    fn command_line_joins_program_and_arguments() {
        let line = command_line(Path::new("/opt/agent gauge/app"), &["--capture-claude"]).unwrap();

        if cfg!(target_os = "windows") {
            assert_eq!(line, r#""/opt/agent gauge/app" "--capture-claude""#);
        } else {
            assert_eq!(line, "'/opt/agent gauge/app' '--capture-claude'");
        }
    }
}
