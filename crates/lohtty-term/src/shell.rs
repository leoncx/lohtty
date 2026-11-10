use std::path::{Path, PathBuf};

/// The program (and arguments) to run inside the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Start the user's shell as a login shell (argv0 `-zsh`), as macOS
    /// terminals do: apps opened from Finder get a minimal `PATH`, which the
    /// login profile completes. Set only for the default shell, so `program`
    /// is the user's shell and `args` is empty.
    pub login: bool,
}

impl ShellSpec {
    /// `program` with `args`, not as a login shell.
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            login: false,
        }
    }
}

/// Resolves the shell to launch: the configured program if any, otherwise the
/// platform default: `pwsh.exe`, then `powershell.exe` on Windows; elsewhere
/// `$SHELL`, else the account's shell, as a login shell on macOS (where the
/// configured args then don't apply).
pub fn resolve_shell(program: Option<&str>, args: &[String]) -> ShellSpec {
    match program {
        Some(p) if !p.trim().is_empty() => ShellSpec::new(p, args.to_vec()),
        _ if cfg!(target_os = "macos") => ShellSpec {
            program: default_shell(),
            args: Vec::new(),
            login: true,
        },
        _ => ShellSpec::new(default_shell(), args.to_vec()),
    }
}

#[cfg(windows)]
fn default_shell() -> String {
    if find_in_path("pwsh.exe").is_some() {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

#[cfg(not(windows))]
fn default_shell() -> String {
    // The lookup portable-pty does for a login shell.
    portable_pty::CommandBuilder::new_default_prog().get_shell()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn find_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|p| Path::new(p).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_program_wins() {
        let spec = resolve_shell(Some("bash"), &["-l".into()]);
        assert_eq!(spec.program, "bash");
        assert_eq!(spec.args, vec!["-l".to_string()]);
    }

    #[test]
    fn default_shell_is_a_login_shell_only_on_macos() {
        let spec = resolve_shell(None, &["-x".into()]);
        assert_eq!(spec.login, cfg!(target_os = "macos"));
        // A login shell is started by portable-pty, without the args.
        assert_eq!(spec.args.is_empty(), spec.login);
        assert!(!resolve_shell(Some("bash"), &[]).login);
    }

    #[test]
    fn blank_program_falls_back_to_default() {
        assert!(!resolve_shell(Some("  "), &[]).program.trim().is_empty());
    }
}
