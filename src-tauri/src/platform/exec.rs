//! Locating and launching external programs.
//!
//! Two Windows behaviours motivate this module.
//!
//! First, `CreateProcess` does not consult `PATHEXT`. On Linux
//! `Command::new("codex")` finds `codex` on the `PATH`; on Windows the npm
//! shim is `codex.cmd` and a bare `"codex"` simply fails to start. Resolving
//! the name to a concrete path before spawning is what makes the Codex provider
//! work on both platforms.
//!
//! Second, adapters are user-supplied executables. On Linux a shebang plus the
//! execute bit is enough; Windows has neither, so an adapter script needs an
//! explicit interpreter. See `platform::adapter_program`.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

/// Resolves a program name to a concrete path where the platform requires it.
///
/// A name containing a path separator is already a path and is returned as-is.
/// On Linux an unqualified name is left for the loader's own `PATH` search. On
/// Windows the `PATH` × `PATHEXT` search happens here, because the OS will not
/// do it for us. If nothing matches, the original name is returned so the
/// caller surfaces the operating system's own "not found" error rather than one
/// we invented.
pub fn resolve_program(name: &str) -> PathBuf {
    if name.contains('/') || name.contains('\\') {
        return PathBuf::from(name);
    }

    #[cfg(target_os = "windows")]
    {
        let paths: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let extensions = path_extensions(std::env::var_os("PATHEXT"));

        if let Some(found) = search_path(name, &paths, &extensions, |path| path.is_file()) {
            return found;
        }
    }

    PathBuf::from(name)
}

/// Splits `PATHEXT` into the extensions to try, falling back to the standard
/// set when the variable is missing or empty.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn path_extensions(raw: Option<OsString>) -> Vec<String> {
    let default = || {
        [".COM", ".EXE", ".BAT", ".CMD"]
            .iter()
            .map(|extension| (*extension).to_string())
            .collect::<Vec<_>>()
    };

    let Some(raw) = raw else { return default() };
    let Some(raw) = raw.to_str() else {
        return default();
    };

    let extensions: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            if extension.starts_with('.') {
                extension.to_string()
            } else {
                format!(".{extension}")
            }
        })
        .collect();

    if extensions.is_empty() {
        default()
    } else {
        extensions
    }
}

/// The pure core of the Windows `PATH` search, with filesystem access injected
/// so it can be exercised on any platform.
///
/// The bare name is tried first: an explicitly named `foo.exe` should win over
/// `foo.exe.com` produced by appending an extension.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn search_path(
    name: &str,
    paths: &[PathBuf],
    extensions: &[String],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let named_with_extension = Path::new(name).extension().is_some();

    for directory in paths {
        if named_with_extension {
            let candidate = directory.join(name);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
        for extension in extensions {
            let candidate = directory.join(format!("{name}{extension}"));
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether the operating system can launch this file on its own.
///
/// On Linux a script carries its own interpreter in a `#!` line and its own
/// permission to run in the execute bit, so anything may be launched directly
/// and the kernel decides. Windows has neither mechanism: it dispatches purely
/// on file extension, so a `read-usage` or `read-usage.py` cannot be started
/// however it is marked. Adapters in that position must name an interpreter.
pub fn is_directly_executable(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        let extensions = path_extensions(std::env::var_os("PATHEXT"));
        has_launchable_extension(path, &extensions)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        true
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn has_launchable_extension(path: &Path, extensions: &[String]) -> bool {
    let Some(found) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    extensions.iter().any(|candidate| {
        candidate
            .trim_start_matches('.')
            .eq_ignore_ascii_case(found)
    })
}

/// Builds a command that runs `command` through the host's shell.
///
/// Used only to run a status-line command that was already configured by the
/// user before Agent Gauge took the slot over. That string is shell source by
/// definition, so honouring it means handing it back to a shell; there is no
/// argv to reconstruct.
pub fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        // `/C` runs the command and exits. Passing the string as a single
        // argument lets cmd apply its own parsing, which is what the user's
        // command was written against.
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut shell = Command::new("/bin/sh");
        shell.arg("-c").arg(command);
        shell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_names_are_left_alone() {
        assert_eq!(
            resolve_program("/usr/bin/codex"),
            PathBuf::from("/usr/bin/codex")
        );
        assert_eq!(
            resolve_program(r"C:\tools\codex.cmd"),
            PathBuf::from(r"C:\tools\codex.cmd")
        );
    }

    /// Windows paths are case-insensitive, so a `PATHEXT` entry of `.CMD`
    /// matches a file named `codex.cmd` on disk. Model that here rather than
    /// the case-sensitive comparison a Linux test host would otherwise imply.
    fn windows_like_exists(present: &str) -> impl Fn(&Path) -> bool + '_ {
        move |path: &Path| path.to_string_lossy().eq_ignore_ascii_case(present)
    }

    #[test]
    fn path_search_finds_the_npm_shim_extension() {
        // The exact case that breaks the Codex provider on Windows: `codex`
        // exists on the PATH only as `codex.cmd`.
        let paths = vec![PathBuf::from(r"C:\bin"), PathBuf::from(r"C:\npm")];
        let extensions = path_extensions(None);
        let present = PathBuf::from(r"C:\npm").join("codex.cmd");

        let found = search_path(
            "codex",
            &paths,
            &extensions,
            windows_like_exists(&present.to_string_lossy()),
        )
        .expect("the npm shim should be found");

        assert!(found
            .to_string_lossy()
            .eq_ignore_ascii_case(&present.to_string_lossy()));
        assert_ne!(
            found.extension(),
            None,
            "the resolved program must carry an extension so CreateProcess can launch it"
        );
    }

    #[test]
    fn path_search_prefers_earlier_directories() {
        let paths = vec![PathBuf::from("/first"), PathBuf::from("/second")];
        let extensions = path_extensions(None);

        let found = search_path("codex", &paths, &extensions, |_| true);
        assert_eq!(found, Some(PathBuf::from("/first").join("codex.COM")));
    }

    #[test]
    fn path_search_reports_nothing_when_absent() {
        let paths = vec![PathBuf::from("/first")];
        let extensions = path_extensions(None);

        assert_eq!(search_path("codex", &paths, &extensions, |_| false), None);
    }

    #[test]
    fn windows_can_only_launch_files_it_recognises_by_extension() {
        let extensions = path_extensions(None);

        assert!(has_launchable_extension(
            Path::new(r"C:\a\tool.exe"),
            &extensions
        ));
        // Case-insensitive, like the filesystem.
        assert!(has_launchable_extension(
            Path::new(r"C:\a\tool.CMD"),
            &extensions
        ));
        assert!(has_launchable_extension(
            Path::new(r"C:\a\tool.bat"),
            &extensions
        ));

        // The two shapes the Linux scaffold produces, neither of which Windows
        // can start without being told an interpreter.
        assert!(!has_launchable_extension(
            Path::new(r"C:\a\read-usage"),
            &extensions
        ));
        assert!(!has_launchable_extension(
            Path::new(r"C:\a\read-usage.py"),
            &extensions
        ));
    }

    #[test]
    fn pathext_is_normalised_and_falls_back_when_unusable() {
        assert_eq!(
            path_extensions(Some(OsString::from(".EXE;.CMD"))),
            vec![".EXE".to_string(), ".CMD".to_string()]
        );
        // Entries without a leading dot, and stray separators, still work.
        assert_eq!(
            path_extensions(Some(OsString::from("EXE;;CMD"))),
            vec![".EXE".to_string(), ".CMD".to_string()]
        );
        assert_eq!(
            path_extensions(Some(OsString::from(""))),
            path_extensions(None)
        );
        assert!(path_extensions(None).contains(&".CMD".to_string()));
    }
}
