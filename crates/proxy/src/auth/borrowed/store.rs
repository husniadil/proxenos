//! `docs/proxy-behavior.md` §8.4 — the declared profiles, as a store.
//!
//! `CredentialStore` and `AccountStore` describe a store that owns what it
//! holds: it adds, saves, renames and forgets. None of those is this store's
//! to do, because the grants belong to other programs. So every write refuses,
//! naming the profile and what to do instead, and the one piece of state that
//! *is* ours lives on our side: which account is selected.
//!
//! The selection is a file rather than a key in the configuration document.
//! `accounts --use` is a runtime verb, and the configuration file is the
//! operator's own document whose comments explain themselves (§4). The token
//! tally already sets this precedent.

use super::Host;
use super::read::GrantReader;
use super::read::Profile;
use super::read::grant;
use crate::auth::store::Account;
use crate::auth::store::AccountStore;
use crate::auth::store::Credential;
use crate::auth::store::CredentialStore;
use crate::auth::store::Credentials;
use crate::auth::store::Provider;
use crate::error::ProxyError;
use std::path::Path;
use std::path::PathBuf;

/// The declared profiles, the host they are resolved against, and where the
/// selection is kept.
pub struct BorrowedStore {
    profiles: Vec<Profile>,
    reader: Box<dyn GrantReader>,
    host: Host,
    home: PathBuf,
    selection: PathBuf,
}

/// What the selection file holds. One name, and room to grow.
#[derive(serde::Deserialize, serde::Serialize)]
struct Selection {
    selected: String,
}

impl BorrowedStore {
    pub fn new(
        profiles: Vec<Profile>,
        reader: Box<dyn GrantReader>,
        host: Host,
        home: impl Into<PathBuf>,
        selection: impl Into<PathBuf>,
    ) -> Self {
        Self {
            profiles,
            reader,
            host,
            home: home.into(),
            selection: selection.into(),
        }
    }

    /// The profile serving turns.
    ///
    /// A single declared profile is that profile: there is nothing to choose
    /// between, and making an operator choose anyway is ceremony. More than
    /// one with nothing chosen is refused rather than resolved to whichever
    /// comes first — the choice decides whose subscription pays, and guessing
    /// at it spends the wrong one invisibly.
    fn selected(&self) -> Result<&Profile, ProxyError> {
        match self.read_selection()? {
            Some(name) => self.named(&name).map_err(|_| {
                ProxyError::authentication(format!(
                    "the selected profile `{name}` is no longer declared in `[profiles]`. \
                     Choose one with `accounts --use NAME`."
                ))
            }),
            None => match self.profiles.as_slice() {
                [] => Err(ProxyError::authentication(
                    "no profiles are declared. Add a `[profiles]` entry naming a directory \
                     another program keeps a grant in."
                        .to_owned(),
                )),
                [only] => Ok(only),
                _ => Err(ProxyError::authentication(
                    "more than one profile is declared and none is selected. \
                     Choose one with `accounts --use NAME`."
                        .to_owned(),
                )),
            },
        }
    }

    fn named(&self, name: &str) -> Result<&Profile, ProxyError> {
        self.profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| {
                ProxyError::authentication(format!(
                    "no profile is called `{name}`. `accounts` lists the declared ones."
                ))
            })
    }

    fn read_selection(&self) -> Result<Option<String>, ProxyError> {
        let raw = match std::fs::read_to_string(&self.selection) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ProxyError::authentication(format!(
                    "could not read {}: {error}",
                    self.selection.display()
                )));
            }
        };
        // An unreadable selection is refused rather than treated as absent:
        // falling back would move which account pays without saying so.
        let parsed: Selection = serde_json::from_str(&raw).map_err(|error| {
            ProxyError::authentication(format!(
                "{} is not readable: {error}. Choose a profile with `accounts --use NAME`.",
                self.selection.display()
            ))
        })?;
        Ok(Some(parsed.selected))
    }

    fn grant_for(
        &self,
        profile: &Profile,
    ) -> Result<crate::auth::borrowed::read::Grant, ProxyError> {
        grant(self.reader.as_ref(), profile, self.host, &self.home)
    }

    /// Why a write was refused, worded for the operator who attempted it.
    fn read_only(&self, verb: &str, name: &str) -> ProxyError {
        ProxyError::authentication(format!(
            "`{name}` is a borrowed profile, and this daemon never writes one, so it cannot \
             {verb} it. The program that owns the profile is the only thing that may change \
             what is in it; `[profiles]` in the configuration file is where this daemon's \
             view of it is edited."
        ))
    }
}

impl CredentialStore for BorrowedStore {
    fn load(&self) -> Result<Option<Credentials>, ProxyError> {
        let profile = self.selected()?;
        Ok(Some(self.grant_for(profile)?.credentials))
    }

    /// The write a refresh would make. Refused: exchanging a borrowed refresh
    /// token rotates the value the owning program still holds, and the
    /// operator would be logged out of it (§8.4).
    fn save(&self, _credentials: &Credentials) -> Result<(), ProxyError> {
        let name = self.selected().map_or("a borrowed profile", |it| &it.name);
        Err(self.read_only("refresh", name))
    }

    fn clear(&self) -> Result<(), ProxyError> {
        let name = self.selected().map_or("a borrowed profile", |it| &it.name);
        Err(self.read_only("sign out of", name))
    }
}

impl AccountStore for BorrowedStore {
    /// Every declared profile, in the order it was declared.
    ///
    /// A profile whose grant cannot be read is still listed. It is a profile
    /// the operator declared, and dropping it would read as one they never
    /// wrote down; what is unknown about it is reported absent instead.
    fn accounts(&self) -> Result<Vec<Account>, ProxyError> {
        let selected = self.selected().map(|profile| profile.name.clone()).ok();

        Ok(self
            .profiles
            .iter()
            .map(|profile| {
                let grant = self.grant_for(profile).ok();
                Account {
                    name: profile.name.clone(),
                    kind: "grant",
                    provider: profile.provider.as_str(),
                    key_flavour: None,
                    account_id: grant
                        .as_ref()
                        .and_then(|it| it.credentials.account_id.clone()),
                    email: grant.as_ref().and_then(|it| it.email.clone()),
                    plan: grant.as_ref().and_then(|it| it.plan.clone()),
                    expires_at: grant.as_ref().and_then(|it| it.credentials.expires_at),
                    selected: selected.as_deref() == Some(profile.name.as_str()),
                }
            })
            .collect())
    }

    fn add(&self, _credentials: &Credentials, label: Option<&str>) -> Result<String, ProxyError> {
        Err(self.read_only("add", label.unwrap_or("a borrowed profile")))
    }

    /// The one write this store does make, and it writes our side only: which
    /// declared profile serves turns.
    fn select(&self, name: &str) -> Result<(), ProxyError> {
        let profile = self.named(name)?;
        let body = serde_json::to_string(&Selection {
            selected: profile.name.clone(),
        })
        .map_err(|error| ProxyError::authentication(format!("could not record it: {error}")))?;

        if let Some(parent) = self.selection.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ProxyError::authentication(format!(
                    "could not create {}: {error}",
                    parent.display()
                ))
            })?;
        }
        std::fs::write(&self.selection, body).map_err(|error| {
            ProxyError::authentication(format!(
                "could not write {}: {error}",
                self.selection.display()
            ))
        })
    }

    fn remove(&self, name: &str) -> Result<(), ProxyError> {
        Err(self.read_only("forget", name))
    }

    fn credential(&self) -> Result<Option<Credential>, ProxyError> {
        Ok(Some(Credential::Grant(
            self.grant_for(self.selected()?)?.credentials,
        )))
    }

    fn credential_for(&self, name: &str) -> Result<Credential, ProxyError> {
        Ok(Credential::Grant(
            self.grant_for(self.named(name)?)?.credentials,
        ))
    }

    fn add_key(&self, name: &str, _key: &str, _provider: Provider) -> Result<(), ProxyError> {
        Err(self.read_only("hold a key for", name))
    }

    fn save_for(&self, name: &str, _credentials: &Credentials) -> Result<(), ProxyError> {
        Err(self.read_only("refresh", name))
    }

    /// A profile's name is the key it is declared under, so renaming it is an
    /// edit to a file the operator already has open.
    fn rename(&self, from: &str, _to: &str) -> Result<(), ProxyError> {
        Err(self.read_only("rename", from))
    }
}

/// Where the selection is kept, beside the other daemon state.
pub fn selection_path(config_dir: &Path) -> PathBuf {
    config_dir.join("selected.json")
}
