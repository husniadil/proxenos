//! `docs/proxy-behavior.md` §8.4 — which account serves turns.
//!
//! Its own file because it is the one piece of account state this daemon owns
//! now. A borrowed profile cannot be written, and a key store could hold the
//! selection but then two stores would each have an opinion about which
//! account is serving. One answer, in one place.
//!
//! Beside the token tally rather than in the configuration document:
//! `accounts --use` is a runtime verb, and that document is the operator's own
//! (`api.md` §4).

use crate::error::ProxyError;
use std::path::Path;
use std::path::PathBuf;

#[derive(serde::Deserialize, serde::Serialize)]
struct Document {
    selected: String,
    /// The account the chosen profile held at the moment it was chosen.
    ///
    /// Recorded so a profile that has since become a different account can be
    /// noticed. A borrowed profile changes identity when the operator signs
    /// into the owning program as somebody else, and the directory keeps its
    /// name — so without this, turns move to another account with nothing
    /// said. Absent where the profile carried no id to record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

/// The recorded choice, or the absence of one.
pub struct Selection {
    path: PathBuf,
}

impl Selection {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Where it is kept, beside the other daemon state.
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("selected.json")
    }

    /// The name chosen, or `None` where nothing has been.
    ///
    /// An unreadable file is refused rather than read as absent: falling back
    /// would move which account pays without saying so.
    pub fn read(&self) -> Result<Option<String>, ProxyError> {
        Ok(self.document()?.map(|document| document.selected))
    }

    fn document(&self) -> Result<Option<Document>, ProxyError> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ProxyError::authentication(format!(
                    "could not read {}: {error}",
                    self.path.display()
                )));
            }
        };
        let parsed: Document = serde_json::from_str(&raw).map_err(|error| {
            ProxyError::authentication(format!(
                "{} is not readable: {error}. Choose an account with `accounts --use NAME`.",
                self.path.display()
            ))
        })?;
        Ok(Some(parsed))
    }

    /// What was recorded about the chosen account's identity, if anything.
    pub fn recorded_account_id(&self) -> Result<Option<String>, ProxyError> {
        Ok(self.document()?.and_then(|document| document.account_id))
    }

    pub fn write(&self, name: &str, account_id: Option<&str>) -> Result<(), ProxyError> {
        let body = serde_json::to_string(&Document {
            selected: name.to_owned(),
            account_id: account_id.map(str::to_owned),
        })
        .map_err(|error| ProxyError::authentication(format!("could not record it: {error}")))?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ProxyError::authentication(format!(
                    "could not create {}: {error}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&self.path, body).map_err(|error| {
            ProxyError::authentication(format!("could not write {}: {error}", self.path.display()))
        })
    }

    /// Forget the choice, which is what removing the account it names must do:
    /// a selection pointing at nothing refuses every turn.
    pub fn clear(&self) -> Result<(), ProxyError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ProxyError::authentication(format!(
                "could not remove {}: {error}",
                self.path.display()
            ))),
        }
    }
}
