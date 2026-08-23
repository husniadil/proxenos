//! `docs/proxy-behavior.md` §8.4 — every account this daemon can serve.
//!
//! Two halves that cannot be one. A subscription grant belongs to the program
//! whose profile holds it and is read there; a key belongs to nobody, has no
//! flow behind it, and has to be kept somewhere — so it is kept here, in the
//! store that already knows how (§8.2).
//!
//! What they share is the selection. Letting each half record its own would
//! give two answers to the one question that decides whose subscription pays.

use crate::auth::borrowed::store::BorrowedStore;
use crate::auth::selection::Selection;
use crate::auth::store::Account;
use crate::auth::store::AccountStore;
use crate::auth::store::Credential;
use crate::auth::store::CredentialStore;
use crate::auth::store::Credentials;
use crate::auth::store::FileStore;
use crate::auth::store::Provider;
use crate::error::ProxyError;

/// The borrowed profiles and the stored keys, under one selection.
pub struct Accounts {
    borrowed: BorrowedStore,
    keys: FileStore,
    selection: Selection,
}

impl Accounts {
    pub fn new(borrowed: BorrowedStore, keys: FileStore, selection: Selection) -> Self {
        Self {
            borrowed,
            keys,
            selection,
        }
    }

    /// How a refusal describes what is here. A bare "no such account" leaves
    /// the operator guessing at a spelling they can only find by looking
    /// somewhere else.
    fn unknown(&self, name: &str, listed: &[Account]) -> ProxyError {
        if listed.is_empty() {
            return ProxyError::authentication(format!(
                "no account named `{name}`; none are available. Declare a profile under \
                 `[profiles]`, or store a key with `login --key --as NAME`."
            ));
        }
        let available = listed
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        ProxyError::authentication(format!("no account named `{name}`; available: {available}"))
    }

    fn ignored(&self) -> Result<Vec<String>, ProxyError> {
        Ok(self
            .keys
            .accounts()?
            .into_iter()
            .filter(|account| account.kind == "grant")
            .map(|account| account.name)
            .collect())
    }

    fn is_borrowed(&self, name: &str) -> bool {
        self.borrowed
            .profiles()
            .iter()
            .any(|profile| profile.name == name)
    }

    /// The account serving turns.
    ///
    /// A recorded choice decides it. With nothing recorded, a single account
    /// of either kind is that account — there is nothing to choose between —
    /// and more than one is refused rather than resolved to whichever comes
    /// first, because the choice decides whose subscription pays.
    fn selected(&self) -> Result<String, ProxyError> {
        if let Some(name) = self.selection.read()? {
            if self.accounts()?.iter().any(|account| account.name == name) {
                return Ok(name);
            }
            return Err(ProxyError::authentication(format!(
                "the selected account `{name}` no longer exists. \
                 Choose one with `accounts --use NAME`."
            )));
        }

        let listed = self.accounts()?;
        match listed.as_slice() {
            [] => Err(ProxyError::authentication(
                "no accounts are available. Declare a profile under `[profiles]`, or store a \
                 key with `login --key --as NAME`."
                    .to_owned(),
            )),
            [only] => Ok(only.name.clone()),
            _ => Err(ProxyError::authentication(
                "more than one account is available and none is selected. \
                 Choose one with `accounts --use NAME`."
                    .to_owned(),
            )),
        }
    }
}

impl CredentialStore for Accounts {
    /// The grant serving turns, or nothing where a key is.
    fn load(&self) -> Result<Option<Credentials>, ProxyError> {
        Ok(self.credential()?.and_then(|it| it.grant().cloned()))
    }

    /// Refused for either half. A borrowed grant is not ours to rotate, and a
    /// key has nothing to refresh into.
    fn save(&self, _credentials: &Credentials) -> Result<(), ProxyError> {
        Err(ProxyError::authentication(
            "this daemon holds no grant of its own to save. A subscription grant belongs to \
             the program whose profile holds it, and is read from there."
                .to_owned(),
        ))
    }

    fn clear(&self) -> Result<(), ProxyError> {
        self.keys.clear()
    }
}

impl AccountStore for Accounts {
    /// Accounts in this daemon's own store that it no longer reads.
    ///
    /// A store written before §8.4 holds grants. Nothing obtains or refreshes
    /// one here now, so they are skipped rather than offered — and said out
    /// loud rather than skipped silently, because a credential that quietly
    /// stopped counting reads as one that vanished.
    fn ignored_grants(&self) -> Result<Vec<String>, ProxyError> {
        self.ignored()
    }

    /// The borrowed profiles first, then the stored keys.
    ///
    /// Order is the order they were declared and stored in. The selection is
    /// applied here rather than by either half, so exactly one row can carry
    /// it.
    fn accounts(&self) -> Result<Vec<Account>, ProxyError> {
        let selected = self.selection.read()?;
        let mut listed = self.borrowed.accounts()?;
        // Grants left in the key store are not read any more. Listing one
        // would offer an account that cannot be spent and cannot be refreshed,
        // and `ignored_grants` is what says where it went instead.
        listed.extend(
            self.keys
                .accounts()?
                .into_iter()
                .filter(|account| account.kind != "grant"),
        );

        let recorded = self.selection.recorded_account_id()?;
        let lone = listed.len() == 1;
        for account in &mut listed {
            account.selected = match selected.as_deref() {
                Some(name) => account.name == name,
                // Nothing recorded and one account: it serves, and saying so
                // is what stops a listing that claims nothing is in use while
                // every turn goes through it.
                None => lone,
            };
            // Each half answers for its own rows; this only makes sure the
            // mark never survives on a row that is no longer the one serving
            // turns, since the selection is decided here.
            account.identity_changed = account.selected
                && (account.identity_changed
                    || (recorded.is_some()
                        && account.account_id.is_some()
                        && recorded != account.account_id));
        }
        Ok(listed)
    }

    /// Refused. What this daemon can add is a key, which is `add_key`.
    fn add(&self, _credentials: &Credentials, label: Option<&str>) -> Result<String, ProxyError> {
        Err(ProxyError::authentication(format!(
            "`{}` cannot be added here: a subscription grant is read from the profile of the \
             program that owns it. Declare that profile under `[profiles]`.",
            label.unwrap_or("an account")
        )))
    }

    fn select(&self, name: &str) -> Result<(), ProxyError> {
        let listed = self.accounts()?;
        let Some(chosen) = listed.iter().find(|account| account.name == name) else {
            return Err(self.unknown(name, &listed));
        };
        // The identity as well as the name. A profile can become a different
        // account later without this daemon doing anything, and this is what
        // makes that noticeable rather than silent.
        self.selection.write(name, chosen.account_id.as_deref())
    }

    /// Forgetting a key removes it. Forgetting a borrowed profile is an edit
    /// to the configuration file, and the refusal says so rather than removing
    /// something the operator did not mean.
    fn remove(&self, name: &str) -> Result<(), ProxyError> {
        if self.is_borrowed(name) {
            return self.borrowed.remove(name);
        }
        self.keys.remove(name)?;
        // A selection naming what was just removed would refuse every turn
        // with a message about an account nobody can see any more.
        if self.selection.read()?.as_deref() == Some(name) {
            self.selection.clear()?;
        }
        Ok(())
    }

    fn credential(&self) -> Result<Option<Credential>, ProxyError> {
        Ok(Some(self.credential_for(&self.selected()?)?))
    }

    fn credential_for(&self, name: &str) -> Result<Credential, ProxyError> {
        if self.is_borrowed(name) {
            return self.borrowed.credential_for(name);
        }
        self.keys.credential_for(name)
    }

    fn add_key(&self, name: &str, key: &str, provider: Provider) -> Result<(), ProxyError> {
        if self.is_borrowed(name) {
            return Err(ProxyError::authentication(format!(
                "`{name}` is already a borrowed profile. Choose another name for the key."
            )));
        }
        // The same rule a login has always followed: storing a credential
        // never moves which account pays. A lone account serves without
        // anything recorded, so the choice has to be written down *before* it
        // stops being lone — otherwise adding a key silently takes the
        // selection away from the account that was serving turns.
        if self.selection.read()?.is_none()
            && let [only] = self.accounts()?.as_slice()
        {
            self.selection
                .write(&only.name, only.account_id.as_deref())?;
        }
        self.keys.add_key(name, key, provider)
    }

    fn save_for(&self, name: &str, credentials: &Credentials) -> Result<(), ProxyError> {
        if self.is_borrowed(name) {
            return self.borrowed.save_for(name, credentials);
        }
        self.keys.save_for(name, credentials)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), ProxyError> {
        if self.is_borrowed(from) {
            return self.borrowed.rename(from, to);
        }
        let recorded = self.selection.recorded_account_id()?;
        self.keys.rename(from, to)?;
        if self.selection.read()?.as_deref() == Some(from) {
            self.selection.write(to, recorded.as_deref())?;
        }
        Ok(())
    }
}

impl Accounts {
    /// Build the store this daemon serves from: the profiles the operator
    /// declared, and the keys it holds itself.
    pub fn from_config(
        config: &crate::config::Config,
        config_dir: &std::path::Path,
    ) -> Result<Self, ProxyError> {
        let profiles = config
            .profiles
            .iter()
            .map(|(name, profile)| crate::auth::borrowed::read::Profile {
                name: name.clone(),
                provider: profile.provider,
                config_dir: profile.path.clone(),
            })
            .collect();

        Ok(Self::new(
            BorrowedStore::new(
                profiles,
                Box::new(crate::auth::borrowed::read::HostReader),
                crate::auth::borrowed::host()?,
                home()?,
                Selection::new(Selection::path_in(config_dir)),
            ),
            FileStore::new(config_dir.join(crate::auth::store::KEYS_FILE)),
            Selection::new(Selection::path_in(config_dir)),
        ))
    }
}

/// The home directory a stock profile is found under.
///
/// Refused rather than defaulted: every stock profile is named relative to it,
/// so a wrong answer here reads as "that profile was never signed into".
fn home() -> Result<std::path::PathBuf, ProxyError> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            ProxyError::authentication(
                "`HOME` is not set, and every stock profile is named relative to it.".to_owned(),
            )
        })
}
