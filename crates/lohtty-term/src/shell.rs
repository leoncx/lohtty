use std::path::{Path, PathBuf};

/// The program (and arguments) to run inside the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// Resolves the shell to launch: the configured program if any, otherwise the
/// platform default (`pwsh.exe`, then `powershell.exe` on Windows; `$SHELL` elsewhere).
pub fn resolve_shell(program: Option<&str>, args: &[String]) -> ShellSpec {
    let program = match program {
        Some(p) if !p.trim().is_empty() => p.to_owned(),
        _ => default_shell(),
    };
    ShellSpec {
        program,
        args: args.to_vec(),
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
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
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
    fn blank_program_falls_back_to_default() {
        assert!(!resolve_shell(Some("  "), &[]).program.trim().is_empty());
    }
}
