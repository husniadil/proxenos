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
use crate::auth::selection::Selection;
use crate::auth::store::Account;
use crate::auth::store::AccountStore;
use crate::auth::store::Credential;
use crate::auth::store::CredentialStore;
use crate::auth::store::Credentials;
use crate::auth::store::Provider;
use crate::error::ProxyError;
use std::path::PathBuf;

/// The declared profiles, the host they are resolved against, and where the
/// selection is kept.
pub struct BorrowedStore {
    profiles: Vec<Profile>,
    reader: Box<dyn GrantReader>,
    /// What a profile is resolved against, or why it cannot be.
    ///
    /// Held as an outcome rather than as a value, so a host nothing has been
    /// checked on refuses at the profile that needs it instead of at startup.
    /// An operator with no `[profiles]` at all is spending a key, which needs
    /// neither of these — and refusing to start there would be refusing a
    /// configuration that is entirely valid.
    platform: Result<Platform, String>,
    selection: Selection,
}

/// Where profiles live on this machine.
pub struct Platform {
    pub host: Host,
    pub home: PathBuf,
}

impl BorrowedStore {
    pub fn new(
        profiles: Vec<Profile>,
        reader: Box<dyn GrantReader>,
        platform: Result<Platform, String>,
        selection: Selection,
    ) -> Self {
        Self {
            profiles,
            reader,
            platform,
            selection,
        }
    }

    /// The profiles this store was built over, in the order declared.
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    /// One profile by name, and the grant it currently holds.
    ///
    /// What deciding whether to ask the owning client for a refresh needs: the
    /// profile says which directory to run it against, and the grant says
    /// whether asking can work at all (§8.4).
    pub fn profile_and_grant(
        &self,
        name: &str,
    ) -> Result<(&Profile, crate::auth::borrowed::read::Grant), ProxyError> {
        let profile = self.named(name)?;
        Ok((profile, self.grant_for(profile)?))
    }

    /// The profile serving turns.
    ///
    /// A single declared profile is that profile: there is nothing to choose
    /// between, and making an operator choose anyway is ceremony. More than
    /// one with nothing chosen is refused rather than resolved to whichever
    /// comes first — the choice decides whose subscription pays, and guessing
    /// at it spends the wrong one invisibly.
    fn selected(&self) -> Result<&Profile, ProxyError> {
        match self.selection.read()? {
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

    fn grant_for(
        &self,
        profile: &Profile,
    ) -> Result<crate::auth::borrowed::read::Grant, ProxyError> {
        let platform = self.platform.as_ref().map_err(|reason| {
            ProxyError::authentication(format!("`{}` cannot be read here: {reason}", profile.name))
        })?;
        grant(self.reader.as_ref(), profile, platform.host, &platform.home)
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
        let recorded = self.selection.recorded_account_id()?;

        Ok(self
            .profiles
            .iter()
            .map(|profile| {
                let grant = self.grant_for(profile).ok();
                Account {
                    // Where an operator can go and look. Named even when the
                    // grant could not be read: that is the case where knowing
                    // which directory was tried matters most.
                    // Named where the profile can be located at all. On a host
                    // nothing has been checked on there is no location to name,
                    // and the row says what it can rather than inventing one.
                    source: self
                        .platform
                        .as_ref()
                        .ok()
                        .map(|platform| profile.source(platform.host, &platform.home).label()),
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
                    // Claude only, and read from the same item the client
                    // counts down from. Absent on a Codex profile because
                    // nothing in `auth.json` says it (§8.4).
                    login_expires_at: grant.as_ref().and_then(|it| it.refresh_token_expires_at),
                    selected: selected.as_deref() == Some(profile.name.as_str()),
                    // Only against something recorded, and only where the
                    // profile can be read now: one that cannot be read has not
                    // changed identity, it has not been read.
                    identity_changed: selected.as_deref() == Some(profile.name.as_str())
                        && recorded.is_some()
                        && grant
                            .as_ref()
                            .and_then(|it| it.credentials.account_id.as_ref())
                            .is_some_and(|account_id| recorded.as_ref() != Some(account_id)),
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
        let account_id = self
            .grant_for(profile)
            .ok()
            .and_then(|grant| grant.credentials.account_id);
        self.selection.write(&profile.name, account_id.as_deref())
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
