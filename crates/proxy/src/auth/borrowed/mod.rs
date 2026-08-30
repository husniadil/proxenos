//! `docs/proxy-behavior.md` §8 — grants this process reads and never owns.
//!
//! A borrowed grant lives in another program's profile directory. The ChatGPT
//! app and the `codex` CLI keep exactly one of them per `CODEX_HOME`, in
//! `auth.json`, so that directory *is* the identity: point at it and the
//! account it holds is the account turns are spent against. An operator who has
//! already signed in over there does not sign in again here.
//!
//! **Nothing here obtains a grant, and a grant read out of the keychain is
//! never written back.** The refresh token in one is single-use: exchanging it
//! rotates the stored value and the previous one is refused afterwards, which
//! is the failure `tokens.rs` already names `refresh_token_reused`. Rotating
//! the macOS keychain item would therefore log the operator out of the program
//! that owns it, and the symptom would appear over there rather than here.
//!
//! Where the grant was read out of a **file**, the same reasoning says the
//! opposite: the owning client reads that file when it starts, so writing a
//! refreshed grant back into it is what keeps the two sides in step, and
//! refusing is what leaves this side stale. `write.rs` holds that split and
//! nothing else in this module writes at all. An expired grant is still
//! reported as expired rather than repaired: the owning program refreshes on
//! its own next turn.
//!
//! The decisions live here as pure functions over the file's text: what is
//! wrong with a credential is decided without I/O, and the caller supplies the
//! path for the message.

use crate::auth::jwt;
use crate::auth::store::Credentials;
use crate::auth::store::Provider;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;
use std::path::Path;
use std::path::PathBuf;

/// Where an operator signs a Codex profile in, named in every refusal that
/// wants them to.
pub(crate) const CODEX_REMEDY: &str = "in the ChatGPT app or with `codex login`";

/// Where an operator signs a Claude profile in. Running the client once is the
/// whole of it: it completes the sign-in and writes the item this reads.
pub(crate) const CLAUDE_REMEDY: &str = "by running `claude` in that profile";

/// The `auth_mode` a ChatGPT subscription is filed under.
///
/// The other modes that string can hold are credentials of a different kind
/// against a different endpoint, and none of them is borrowed: see
/// `BorrowedError::NotASubscription`.
pub const SUBSCRIPTION_AUTH_MODE: &str = "chatgpt";

/// Why a profile yielded no grant.
///
/// Every variant is a refusal, never a repair. A store this cannot read is a
/// store the owning program still owns, and guessing at it would put a
/// credential of the wrong kind on the wire.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BorrowedError {
    #[error("{0} is not valid JSON: {1}")]
    Malformed(String, String),
    /// The store exists but holds no grant, which is what a profile that has
    /// never completed a sign-in looks like.
    ///
    /// The remedy is carried rather than fixed, because the operator's next
    /// move is in a different program for each provider and naming the wrong
    /// one sends them somewhere that cannot help.
    #[error("{0} holds no grant. Sign in to that profile first, {1}")]
    NotSignedIn(String, &'static str),
    /// A profile authenticating with an API key rather than a subscription.
    /// Refused rather than borrowed: a key is spent against a different
    /// endpoint with different billing, and this proxy already has a place to
    /// put one that does not involve reading another program's file.
    #[error(
        "{0} authenticates with an API key (auth_mode `{1}`) rather than a ChatGPT subscription. \
         Store a key with `proxenos accounts add-key NAME --provider codex|anthropic` instead"
    )]
    NotASubscription(String, String),
    /// A grant missing the half that authenticates, or the half that outlives
    /// it. Both are required, and a blank one is a broken file rather than an
    /// absent field.
    #[error("{0} holds an empty {1}. Sign in to that profile again")]
    EmptyToken(String, &'static str),
}

/// `auth.json` as far as a grant is concerned.
///
/// Deliberately partial: the file carries fields this has no business reading,
/// and `serde` ignores what is not named here. `last_refresh` is one of them —
/// it records when the owning program last rotated, and nothing on this side
/// acts on it.
#[derive(Deserialize)]
struct AuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default)]
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// The grant stored in a `CODEX_HOME`, as this process will spend it.
///
/// `source` names the file and appears in every refusal. It is a label rather
/// than a handle: the read happened before this was called, so a profile that
/// moved is described by the path it was expected at.
pub fn codex(raw: &str, source: &str) -> Result<Credentials, BorrowedError> {
    let parsed: AuthFile = serde_json::from_str(raw)
        .map_err(|error| BorrowedError::Malformed(source.to_owned(), error.to_string()))?;

    // Read before the tokens are: a profile in API-key mode can still carry a
    // stale `tokens` block from a previous sign-in, and borrowing that would
    // spend an identity the operator has since replaced.
    let mode = parsed.auth_mode.as_deref().unwrap_or_default();
    if !mode.is_empty() && mode != SUBSCRIPTION_AUTH_MODE {
        return Err(BorrowedError::NotASubscription(
            source.to_owned(),
            mode.to_owned(),
        ));
    }

    let tokens = parsed
        .tokens
        .ok_or_else(|| BorrowedError::NotSignedIn(source.to_owned(), CODEX_REMEDY))?;

    for (value, name) in [
        (&tokens.access_token, "access_token"),
        (&tokens.refresh_token, "refresh_token"),
    ] {
        if value.trim().is_empty() {
            return Err(BorrowedError::EmptyToken(source.to_owned(), name));
        }
    }

    // The file records no expiry of its own, so it comes from the access
    // token's `exp` claim — the same place `tokens::refresh` reads it from, and
    // for the same reason. A token that yields none is treated as expired by
    // `needs_refresh`, which on this path means "ask the owning program to
    // refresh" rather than "refresh it here".
    let expires_at = jwt::expiry(&tokens.access_token);

    // `tokens.account_id` and the id token's `chatgpt_account_id` claim carry
    // the same value — checked against three signed-in profiles on one machine,
    // all three equal. The field wins because the owning program writes it
    // deliberately, and the claim covers a file written before it existed.
    let account_id = tokens
        .account_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| jwt::account_id(tokens.id_token.as_deref()));

    Ok(Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        expires_at,
    })
}

/// The keychain item a Claude profile's grant is filed under, on macOS.
///
/// Two names, and which one applies is decided by whether the client was
/// launched with `CLAUDE_CONFIG_DIR` set at all — not by what it was set to.
/// Measured against a real client: no variable gives the bare name, and setting
/// it to the very directory the bare name describes still gives the hashed one.
/// So the default profile is the one launched with nothing set, and every other
/// profile is named by its directory.
pub const CLAUDE_SERVICE: &str = "Claude Code-credentials";

/// The service name for a profile, given the `CLAUDE_CONFIG_DIR` it is launched
/// with. `None` is the default profile: launched with the variable unset.
///
/// The digest is taken over the variable's value **verbatim**. A trailing
/// slash, or a path that walks through `..` to the same directory, is a
/// different service name, because it is a different string. Measured: three
/// spellings of one directory produced three different items. Nothing here
/// canonicalizes, since canonicalizing would name an item the client never
/// writes.
pub fn claude_service(config_dir: Option<&str>) -> String {
    let Some(config_dir) = config_dir else {
        return CLAUDE_SERVICE.to_owned();
    };
    let digest = Sha256::digest(config_dir.as_bytes());
    format!("{CLAUDE_SERVICE}-{:.8}", hex(&digest))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The keychain blob, as the client writes it.
#[derive(Deserialize)]
struct ClaudeItem {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<ClaudeOauth>,
}

#[derive(Deserialize)]
struct ClaudeOauth {
    #[serde(rename = "accessToken", default)]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: String,
    /// Unix **milliseconds**, unlike everything else here.
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<u64>,
    #[serde(rename = "refreshTokenExpiresAt", default)]
    refresh_token_expires_at: Option<u64>,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Option<String>,
}

/// A borrowed Claude grant, and the two facts about it that are not the
/// credential itself.
///
/// They travel together because both decisions that use them are made where
/// the grant is read. `refresh_token_expires_at` is what says whether asking
/// the client to refresh can possibly work: past it, a refresh attempt fails
/// AND the client overwrites the item with an empty token, so an expired
/// refresh token turns a poke into damage. `plan` is what tells two borrowed
/// accounts apart for an operator who holds more than one.
///
/// `Debug` is derived and safe to derive: the only secret in here is inside
/// `Credentials`, whose own `Debug` redacts it by hand.
#[derive(Debug)]
pub struct ClaudeGrant {
    pub credentials: Credentials,
    /// Unix seconds, converted from the milliseconds the item stores.
    pub refresh_token_expires_at: Option<u64>,
    pub plan: Option<String>,
}

/// The grant stored in a Claude profile's keychain item.
///
/// `source` names the item, and appears in every refusal.
pub fn claude(raw: &str, source: &str) -> Result<ClaudeGrant, BorrowedError> {
    let parsed: ClaudeItem = serde_json::from_str(raw)
        .map_err(|error| BorrowedError::Malformed(source.to_owned(), error.to_string()))?;

    let oauth = parsed
        .oauth
        .ok_or_else(|| BorrowedError::NotSignedIn(source.to_owned(), CLAUDE_REMEDY))?;

    // This is also what a failed refresh leaves behind. The client blanks the
    // token and zeroes the expiry rather than removing the item, so a profile
    // whose refresh token has died is indistinguishable from one that was
    // never signed in — and both want the same answer, which is to sign in.
    for (value, name) in [
        (&oauth.access_token, "accessToken"),
        (&oauth.refresh_token, "refreshToken"),
    ] {
        if value.trim().is_empty() {
            return Err(BorrowedError::EmptyToken(source.to_owned(), name));
        }
    }

    Ok(ClaudeGrant {
        credentials: Credentials {
            access_token: oauth.access_token,
            refresh_token: oauth.refresh_token,
            // The item carries neither. An id token is the other provider's
            // way of describing an account, and inventing one here would put a
            // claim on the wire that nothing issued.
            id_token: None,
            account_id: None,
            expires_at: oauth.expires_at.map(milliseconds_to_seconds),
        },
        refresh_token_expires_at: oauth.refresh_token_expires_at.map(milliseconds_to_seconds),
        plan: oauth.subscription_type.filter(|it| !it.trim().is_empty()),
    })
}

/// The item stores milliseconds and `Credentials` is in seconds. Truncating is
/// the safe direction: it can only make a token look older than it is, and the
/// cost of that is one refresh, where the cost of the other is a turn that
/// fails mid-request.
fn milliseconds_to_seconds(milliseconds: u64) -> u64 {
    milliseconds / 1_000
}

/// The stock Claude profile, relative to the home directory. Named here rather
/// than assumed at the call site because it is what "no `CLAUDE_CONFIG_DIR`"
/// resolves to.
pub const CLAUDE_DEFAULT_PROFILE: &str = ".claude";

/// What the client falls back to where there is no keychain: a file inside the
/// profile directory, holding the same JSON the keychain item holds.
pub const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";

/// The platforms a Claude grant has been located on.
///
/// A parameter rather than a `cfg`, so every rule below is testable from any
/// host. Windows is deliberately absent: nobody has checked where the client
/// puts a grant there, and inventing a location would produce a profile that
/// reads as "never signed in" for a reason that is our mistake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    MacOs,
    Linux,
}

/// Where a Claude profile's grant is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeSource {
    /// A macOS keychain item, read by spawning `security`. Never through
    /// Security.framework: the item's ACL trusts that binary, and a process
    /// asking directly is a different application to the keychain, which
    /// prompts. One client run reads it sixteen times.
    Keychain { service: String },
    /// A file inside the profile directory, `0600`.
    File { path: PathBuf },
    /// macOS: the keychain item, and the file beside it where the keychain
    /// cannot answer.
    ///
    /// Not a preference between two stores — the item is where the client puts
    /// the grant, and the file is what it falls back to — but a daemon reading
    /// the item needs a login keychain that is unlocked in a security session,
    /// and a system-domain LaunchDaemon for a headless account has neither. It
    /// gets "item not found" with no session, and an unexplained failure with
    /// one, and the only remedy at the keychain is typing the account password
    /// at every boot. The file holds the same JSON, so it is read when the
    /// keychain says nothing — whether that is an absent item or a refusal
    /// (§8.4).
    KeychainThenFile { service: String, path: PathBuf },
}

/// Where to look for the grant of the profile launched with `config_dir`.
///
/// `None` is the default profile, launched with `CLAUDE_CONFIG_DIR` unset, and
/// `home` is where that profile lives.
pub fn claude_source(host: Host, config_dir: Option<&Path>, home: &Path) -> ClaudeSource {
    let path = config_dir
        .map_or_else(|| home.join(CLAUDE_DEFAULT_PROFILE), Path::to_path_buf)
        .join(CLAUDE_CREDENTIALS_FILE);
    match host {
        Host::MacOs => ClaudeSource::KeychainThenFile {
            service: claude_service(config_dir.map(|dir| dir.to_string_lossy()).as_deref()),
            path,
        },
        Host::Linux => ClaudeSource::File { path },
    }
}

/// The stock Codex profile, relative to the home directory: what `CODEX_HOME`
/// resolves to when nothing sets it.
pub const CODEX_DEFAULT_PROFILE: &str = ".codex";

/// The file a Codex profile keeps its grant in.
pub const CODEX_CREDENTIALS_FILE: &str = "auth.json";

/// Where one configured profile's grant is read from.
///
/// The two providers differ in kind, not only in path: one is always a file
/// inside the directory, the other is a keychain item named *after* the
/// directory on macOS and a file on Linux. Resolving that here keeps the
/// difference in one place, where §8.4 can be checked against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Codex { auth_json: PathBuf },
    Claude(ClaudeSource),
}

/// Where to read the grant of the profile `config_dir` designates.
///
/// `config_dir` is `None` for the stock profile: the one that program uses
/// when no variable designates a directory. For Claude on macOS that is a
/// different keychain item from one naming the stock directory explicitly,
/// which is why absent is carried through rather than resolved to a path
/// first.
pub fn source(provider: Provider, host: Host, config_dir: Option<&Path>, home: &Path) -> Source {
    match provider {
        Provider::Codex => Source::Codex {
            auth_json: config_dir
                .map_or_else(|| home.join(CODEX_DEFAULT_PROFILE), Path::to_path_buf)
                .join(CODEX_CREDENTIALS_FILE),
        },
        Provider::Anthropic => Source::Claude(claude_source(host, config_dir, home)),
    }
}

/// The profiles looked for when the operator has declared none.
///
/// Both stock profiles: what each program uses with no variable set. A machine
/// where one of them was never signed into simply has one account, and one
/// where neither was has none — the same answer as before, arrived at without
/// making the operator write down what the programs themselves already know.
///
/// Their names are what `accounts use` takes, so they are the plainest thing
/// each provider could be called. Declaring `[profiles]` replaces this set
/// entirely: a written entry is the operator's own statement about identity,
/// and a discovered one must never quietly sit beside it.
pub fn discovered() -> Vec<read::Profile> {
    [
        (DISCOVERED_CODEX, Provider::Codex),
        (DISCOVERED_CLAUDE, Provider::Anthropic),
    ]
    .into_iter()
    .map(|(name, provider)| read::Profile {
        name: name.to_owned(),
        provider,
        config_dir: None,
    })
    .collect()
}

pub const DISCOVERED_CODEX: &str = "codex";
pub const DISCOVERED_CLAUDE: &str = "claude";

pub mod poke;
pub mod read;
pub mod store;
pub mod write;

impl Source {
    /// What to call this source in a message: the one an operator can go and
    /// look at. Never any part of what it holds.
    pub fn label(&self) -> String {
        match self {
            Self::Codex { auth_json } => auth_json.display().to_string(),
            Self::Claude(ClaudeSource::File { path }) => path.display().to_string(),
            Self::Claude(ClaudeSource::Keychain { service }) => {
                format!("keychain item `{service}`")
            }
            // Both, in the order they are tried. An operator whose keychain
            // cannot be reached has to know the file was the other candidate,
            // and one whose file is missing has to know the item was tried
            // first; naming one of the two would send them to the wrong place
            // half the time.
            Self::Claude(ClaudeSource::KeychainThenFile { service, path }) => {
                format!("keychain item `{service}`, else {}", path.display())
            }
        }
    }

    /// The readable locations this source stands for, in the order they are
    /// tried.
    ///
    /// One for every source but the macOS Claude one, which is two. Splitting
    /// it here rather than inside the reader is what keeps the reader a thing
    /// that reads one place: the ordering, and what a failure at the first
    /// place means, are decisions, and decisions in this module are made
    /// without I/O.
    #[must_use]
    pub fn places(&self) -> Vec<Self> {
        match self {
            Self::Claude(ClaudeSource::KeychainThenFile { service, path }) => vec![
                Self::Claude(ClaudeSource::Keychain {
                    service: service.clone(),
                }),
                Self::Claude(ClaudeSource::File { path: path.clone() }),
            ],
            other => vec![other.clone()],
        }
    }
}

/// What to tell an operator whose profile holds no grant, by provider.
pub(crate) fn remedy(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => CODEX_REMEDY,
        Provider::Anthropic => CLAUDE_REMEDY,
    }
}

/// Which host's rules apply to this build.
///
/// Windows is refused rather than guessed at: nobody has checked where the
/// client keeps a grant there, and inventing a location would report every
/// profile as never signed into for a reason of our own making.
pub fn host() -> Result<Host, crate::error::ProxyError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Host::MacOs)
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Host::Linux)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(crate::error::ProxyError::authentication(
            "borrowing a grant from another program's profile has not been checked on this \
             platform, so nothing here knows where to look."
                .to_owned(),
        ))
    }
}
