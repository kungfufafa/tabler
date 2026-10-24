//! "Install with Homebrew…" in the backup window: TablePlus bundles pg_dump / mysqldump, Tabler
//! uses the installed ones. When none (or none recent enough for the server) is found, the
//! window offers to install a client formula. The `brew install` runs in the user's Terminal,
//! visibly (a `.command` script opened with `open -a Terminal`), never silently in the
//! background; the window's Rescan finds the new binaries afterwards.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::error::AppError;
use crate::process::NoWindow as _;

/// The formulae the window offers: PostgreSQL's client library and tools (`libpq`), a server
/// version's full install (`postgresql@17`), MySQL's client (`mysql-client`, `mysql-client@8.4`)
/// and MariaDB (`mariadb`, `mariadb@11.4`).
static FORMULA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(libpq|postgresql@\d{1,2}|mysql-client(@\d{1,2}(\.\d{1,2})?)?|mariadb(@\d{1,2}(\.\d{1,2})?)?)$")
        .expect("valid formula regex")
});

pub fn valid_formula(formula: &str) -> bool {
    FORMULA.is_match(formula)
}

/// The script Terminal runs: finds `brew` (Apple silicon, Intel, then PATH), waits for Return,
/// installs the formula, says what to do next. It deletes itself first.
pub fn install_script(formula: &str) -> Result<String, AppError> {
    if !valid_formula(formula) {
        return Err(AppError::InvalidInput(format!(
            "\"{formula}\" is not a formula Tabler installs"
        )));
    }
    Ok(format!(
        r#"#!/bin/bash
# Tabler: installs the database client tools for Backup / Restore with Homebrew.
rm -f -- "$0"
BREW=""
for candidate in /opt/homebrew/bin/brew /usr/local/bin/brew; do
  if [ -x "$candidate" ]; then BREW="$candidate"; break; fi
done
if [ -z "$BREW" ]; then BREW="$(command -v brew)"; fi
if [ -z "$BREW" ]; then
  echo "Homebrew is not installed. Install it from https://brew.sh, then run:"
  echo "  brew install {formula}"
  exit 1
fi
echo "Tabler will run:  brew install {formula}"
read -r -p "Press Return to run it, or close this window to cancel. " _
echo "$ brew install {formula}"
"$BREW" install {formula}
status=$?
echo
if [ $status -eq 0 ]; then
  echo "Done. Go back to Tabler and click Rescan in the Backup window."
else
  echo "brew install {formula} failed (exit $status)."
fi
exit $status
"#
    ))
}

/// Writes the script for `formula` into `dir` (executable, owner only).
pub fn write_script(dir: &Path, formula: &str) -> Result<PathBuf, AppError> {
    let script = install_script(formula)?;
    let stem: String = formula
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let random = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join(format!("tabler-install-{stem}-{}.command", &random[..8]));
    std::fs::write(&path, script)
        .map_err(|e| AppError::Io(format!("cannot write {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

/// Opens the script in a new Terminal window, where it runs.
pub async fn open_in_terminal(script: &Path) -> Result<(), AppError> {
    let status = tokio::process::Command::new("/usr/bin/open")
        .no_window()
        .arg("-a")
        .arg("Terminal")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| AppError::Io(format!("cannot open Terminal: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(script);
        Err(AppError::Io(format!(
            "cannot open Terminal (open exited with {status})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_formulae_are_installed() {
        for ok in [
            "libpq",
            "postgresql@17",
            "postgresql@9",
            "mysql-client",
            "mysql-client@8.4",
            "mariadb",
            "mariadb@11.4",
        ] {
            assert!(valid_formula(ok), "{ok}");
        }
        for bad in [
            "",
            "libpq; rm -rf ~",
            "postgresql",
            "postgresql@17 && curl x",
            "wget",
            "mysql-client@8.4.1.2",
            "--force libpq",
        ] {
            assert!(!valid_formula(bad), "{bad}");
            assert!(install_script(bad).is_err());
        }
    }

    #[test]
    fn writes_an_executable_script_that_installs_the_formula() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script(dir.path(), "postgresql@17").unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("tabler-install-postgresql-17-"), "{name}");
        assert!(name.ends_with(".command"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#!/bin/bash\n"));
        assert!(text.contains("\"$BREW\" install postgresql@17\n"));
        assert!(text.contains("rm -f -- \"$0\""));
        // The user confirms in Terminal before brew runs.
        assert!(text.find("read -r -p").unwrap() < text.find("\"$BREW\" install").unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }
}
