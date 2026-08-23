//! `docs/proxy-behavior.md` §8 — reading a grant out of another program's
//! profile directory.
//!
//! The shapes here are the ones a real `auth.json` has: checked against three
//! signed-in `CODEX_HOME` directories on one machine, which is where the
//! field names and the account-id equality below come from.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use base64::Engine;
use pretty_assertions::assert_eq;
use proxenos::auth::borrowed;
use proxenos::auth::borrowed::BorrowedError;
use proxenos::auth::store::Provider;
use std::path::Path;
use std::path::PathBuf;

const SOURCE: &str = "/profiles/work/auth.json";

fn token_with(payload: serde_json::Value) -> String {
    let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"none"}"#),
        encode(payload.to_string().as_bytes()),
        encode(b"signature")
    )
}

/// An access token carrying an expiry, as the owning program writes one.
fn access_token(exp: u64) -> String {
    token_with(serde_json::json!({ "exp": exp }))
}

/// An id token carrying the claims the account is described by.
fn id_token(account_id: &str, plan: &str) -> String {
    token_with(serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan,
        },
    }))
}

fn auth_json(tokens: serde_json::Value) -> String {
    serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": tokens,
        "last_refresh": "2026-08-23T08:00:44.123456Z",
        "auth_mode": "chatgpt",
    })
    .to_string()
}

/// What a refusal was, for a case that is expected to be one.
///
/// `Credentials` deliberately implements no `PartialEq` — comparing two
/// secrets is not an operation this codebase wants to make easy — so a refusal
/// is asserted on rather than the whole `Result`.
fn refusal(raw: &str) -> BorrowedError {
    borrowed::codex(raw, SOURCE).expect_err("this case is a refusal")
}

/// The whole file, as a signed-in profile holds it.
#[test]
fn a_signed_in_profile_yields_its_grant() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_123", "team"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("a signed-in profile parses");

    assert_eq!(grant.refresh_token, "rt.1.borrowed");
    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
    assert_eq!(grant.expires_at, Some(1_800_000_000));
}

/// The file records no expiry of its own, so it comes from the access token's
/// own claim. Without this every borrowed grant would look due for refresh —
/// and on this path there is nothing that may refresh it.
#[test]
fn the_expiry_comes_from_the_access_token_claim() {
    let raw = auth_json(serde_json::json!({
        "access_token": access_token(1_777_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.expires_at, Some(1_777_000_000));
}

/// An access token with no readable claim yields no expiry, which
/// `needs_refresh` treats as expired. That is the safe direction here: it asks
/// the owning program for a fresh one instead of spending a token that may be
/// dead.
#[test]
fn an_unreadable_access_token_yields_no_expiry() {
    let raw = auth_json(serde_json::json!({
        "access_token": "not-a-jwt",
        "refresh_token": "rt.1.borrowed",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.expires_at, None);
    assert!(grant.needs_refresh(1_800_000_000, 0));
}

/// `tokens.account_id` and the id token's claim hold the same value on every
/// profile checked, so either answers. The field is preferred because the
/// owning program writes it deliberately.
#[test]
fn the_account_id_prefers_the_field_over_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_from_field",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_field"));
}

/// A file written before the field existed still names its account, because
/// the claim carries it too.
#[test]
fn the_account_id_falls_back_to_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_claim"));
}

/// A blank field is not an answer, and falls through to the claim rather than
/// being reported as an account named "".
#[test]
fn a_blank_account_id_field_falls_back_to_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "   ",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_claim"));
}

/// No id token at all is not a failure: the grant still authenticates, and
/// only the description of it is poorer.
#[test]
fn a_grant_without_an_id_token_still_parses() {
    let raw = auth_json(serde_json::json!({
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.id_token, None);
    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
}

/// A profile nobody has signed into holds the file but no grant.
#[test]
fn a_profile_that_was_never_signed_into_is_refused() {
    let raw = serde_json::json!({ "OPENAI_API_KEY": null, "auth_mode": "chatgpt" }).to_string();

    assert_eq!(
        refusal(&raw),
        BorrowedError::NotSignedIn(
            SOURCE.to_owned(),
            "in the ChatGPT app or with `codex login`"
        )
    );
}

/// An API-key profile is refused rather than borrowed: a key is spent against
/// a different endpoint with different billing, and this proxy already stores
/// one without reading anyone else's file.
///
/// The check runs before the tokens are read, because such a profile can still
/// carry a stale `tokens` block from a sign-in the operator has replaced.
#[test]
fn an_api_key_profile_is_refused_even_with_tokens_present() {
    let raw = serde_json::json!({
        "OPENAI_API_KEY": "sk-not-borrowed",
        "auth_mode": "apikey",
        "tokens": {
            "access_token": access_token(1_800_000_000),
            "refresh_token": "rt.1.stale",
            "account_id": "acct_stale",
        },
    })
    .to_string();

    assert_eq!(
        refusal(&raw),
        BorrowedError::NotASubscription(SOURCE.to_owned(), "apikey".to_owned())
    );
}

/// A file written before `auth_mode` existed is read as the subscription it
/// is, rather than refused for a field that was never there.
#[test]
fn a_file_without_an_auth_mode_is_read_as_a_subscription() {
    let raw = serde_json::json!({
        "tokens": {
            "access_token": access_token(1_800_000_000),
            "refresh_token": "rt.1.borrowed",
            "account_id": "acct_123",
        },
    })
    .to_string();

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
}

/// An empty half is a broken file, and it names which half rather than
/// failing later against the backend.
#[test]
fn an_empty_token_is_refused_by_name() {
    let missing_access = auth_json(serde_json::json!({
        "access_token": "",
        "refresh_token": "rt.1.borrowed",
    }));
    assert_eq!(
        refusal(&missing_access),
        BorrowedError::EmptyToken(SOURCE.to_owned(), "access_token")
    );

    let missing_refresh = auth_json(serde_json::json!({
        "access_token": access_token(1_800_000_000),
        "refresh_token": "  ",
    }));
    assert_eq!(
        refusal(&missing_refresh),
        BorrowedError::EmptyToken(SOURCE.to_owned(), "refresh_token")
    );
}

/// The path is in every refusal, because the operator's next move is to sign
/// in to one particular profile and the message is what names it.
#[test]
fn every_refusal_names_the_file() {
    let cases = [
        serde_json::json!({ "auth_mode": "chatgpt" }).to_string(),
        serde_json::json!({ "auth_mode": "apikey" }).to_string(),
        auth_json(serde_json::json!({ "access_token": "", "refresh_token": "" })),
        "{ not json".to_owned(),
    ];

    for raw in cases {
        let message = borrowed::codex(&raw, SOURCE)
            .expect_err("every case here is a refusal")
            .to_string();
        assert!(message.contains(SOURCE), "message was: {message}");
    }
}

/// Reading a grant never renders one. The tokens are secrets and the file is
/// somebody else's, so a message about it carries neither.
#[test]
fn a_refusal_never_carries_the_tokens() {
    let raw = auth_json(serde_json::json!({
        "access_token": "",
        "refresh_token": "rt.1.SECRET-VALUE",
    }));

    let message = borrowed::codex(&raw, SOURCE)
        .expect_err("an empty access token is refused")
        .to_string();

    assert!(!message.contains("SECRET-VALUE"), "message was: {message}");
}

// --- Claude ---------------------------------------------------------------
//
// The service names below are measured, not derived: a shim on `security`
// recorded what a real client asked for under each `CLAUDE_CONFIG_DIR`.

const ITEM: &str = "Claude Code-credentials-0b88b8e3";

fn keychain_blob(oauth: serde_json::Value) -> String {
    serde_json::json!({ "claudeAiOauth": oauth }).to_string()
}

fn claude_refusal(raw: &str) -> BorrowedError {
    borrowed::claude(raw, ITEM).expect_err("this case is a refusal")
}

/// A signed-in profile, with the two expiries the item carries.
#[test]
fn a_claude_profile_yields_its_grant() {
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 1_787_482_805_137u64,
        "refreshTokenExpiresAt": 1_789_000_000_000u64,
        "scopes": ["user:inference", "user:profile"],
        "subscriptionType": "max",
    }));

    let grant = borrowed::claude(&raw, ITEM).expect("a signed-in profile parses");

    assert_eq!(grant.credentials.access_token, "sk-ant-oat01-borrowed");
    assert_eq!(grant.plan.as_deref(), Some("max"));
    assert_eq!(grant.credentials.expires_at, Some(1_787_482_805));
    assert_eq!(grant.refresh_token_expires_at, Some(1_789_000_000));
}

/// The item stores milliseconds and `Credentials` stores seconds. Without the
/// conversion every borrowed Claude grant reads as valid for another fifty
/// thousand years.
#[test]
fn the_claude_expiry_is_converted_from_milliseconds() {
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 1_800_000_000_999u64,
    }));

    let grant = borrowed::claude(&raw, ITEM).expect("parses");

    assert_eq!(grant.credentials.expires_at, Some(1_800_000_000));
    assert!(grant.credentials.needs_refresh(1_800_000_000, 0));
}

/// A grant carries no id token and no account id, and neither is invented.
#[test]
fn a_claude_grant_carries_no_id_token() {
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 1_800_000_000_000u64,
    }));

    let grant = borrowed::claude(&raw, ITEM).expect("parses");

    assert_eq!(grant.credentials.id_token, None);
    assert_eq!(grant.credentials.account_id, None);
}

/// What a failed refresh leaves behind: the client blanks the token and zeroes
/// the expiry in place rather than removing the item. Measured. It has to read
/// as "sign in again" rather than as a grant with an odd expiry, or the next
/// turn is spent on an empty bearer.
#[test]
fn a_blanked_item_reads_as_a_refusal() {
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 0u64,
    }));

    assert_eq!(
        claude_refusal(&raw),
        BorrowedError::EmptyToken(ITEM.to_owned(), "accessToken")
    );
}

/// An item with no oauth block at all is a profile nobody has signed into, and
/// the remedy names the client rather than the ChatGPT app.
#[test]
fn a_claude_profile_never_signed_into_names_its_own_remedy() {
    let refusal = claude_refusal("{}");

    assert_eq!(
        refusal,
        BorrowedError::NotSignedIn(ITEM.to_owned(), "by running `claude` in that profile")
    );
    assert!(refusal.to_string().contains("`claude`"));
}

/// The default profile is the one launched with no `CLAUDE_CONFIG_DIR` at all.
#[test]
fn the_default_profile_uses_the_bare_service_name() {
    assert_eq!(borrowed::claude_service(None), "Claude Code-credentials");
}

/// Setting the variable hashes it, even when it names the very directory the
/// bare name describes. Measured: `CLAUDE_CONFIG_DIR=$HOME/.claude` asked for
/// the hashed item, not the bare one. So "default" means unset, and a config
/// dir that merely looks default is still its own profile.
#[test]
fn a_config_dir_is_named_by_its_digest_even_when_it_is_the_default_path() {
    assert_eq!(
        borrowed::claude_service(Some("/Users/husni/.claude")),
        "Claude Code-credentials-0b88b8e3"
    );
}

/// The digest is over the string, with nothing canonicalized. Three spellings
/// of one directory produced three different items on a real client, and
/// canonicalizing here would name an item the client never writes.
#[test]
fn the_service_digest_is_taken_over_the_value_verbatim() {
    assert_eq!(
        borrowed::claude_service(Some("/Users/husni/.claude/")),
        "Claude Code-credentials-aff6b0d4"
    );
    assert_eq!(
        borrowed::claude_service(Some("/Users/husni/../husni/.claude")),
        "Claude Code-credentials-2324dbbd"
    );
}

/// On macOS the grant is a keychain item, and the default profile is the
/// unset-variable one.
#[test]
fn a_macos_profile_reads_from_the_keychain() {
    let home = Path::new("/Users/husni");

    assert_eq!(
        borrowed::claude_source(borrowed::Host::MacOs, None, home),
        borrowed::ClaudeSource::Keychain {
            service: "Claude Code-credentials".to_owned()
        }
    );
    assert_eq!(
        borrowed::claude_source(
            borrowed::Host::MacOs,
            Some(Path::new("/Users/husni/.claude")),
            home
        ),
        borrowed::ClaudeSource::Keychain {
            service: "Claude Code-credentials-0b88b8e3".to_owned()
        }
    );
}

/// On Linux there is no keychain, and the grant sits in the profile directory
/// as a file holding the same JSON.
#[test]
fn a_linux_profile_reads_from_a_file_in_its_own_directory() {
    let home = Path::new("/home/husni");

    assert_eq!(
        borrowed::claude_source(borrowed::Host::Linux, None, home),
        borrowed::ClaudeSource::File {
            path: PathBuf::from("/home/husni/.claude/.credentials.json")
        }
    );
    assert_eq!(
        borrowed::claude_source(
            borrowed::Host::Linux,
            Some(Path::new("/profiles/work")),
            home
        ),
        borrowed::ClaudeSource::File {
            path: PathBuf::from("/profiles/work/.credentials.json")
        }
    );
}

/// The one thing a `Debug` line must never do. `ClaudeGrant` derives it, and
/// the redaction it relies on lives in `Credentials`.
#[test]
fn debugging_a_claude_grant_shows_no_token() {
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-SECRET-VALUE",
        "refreshToken": "sk-ant-ort01-ALSO-SECRET",
        "expiresAt": 1_800_000_000_000u64,
    }));

    let rendered = format!("{:?}", borrowed::claude(&raw, ITEM).expect("parses"));

    assert!(!rendered.contains("SECRET-VALUE"), "was: {rendered}");
    assert!(!rendered.contains("ALSO-SECRET"), "was: {rendered}");
}

// --- where one configured profile is read from ----------------------------

/// A Codex profile is always a file inside its directory, on every host.
#[test]
fn a_codex_profile_is_a_file_in_its_directory() {
    let home = Path::new("/Users/husni");

    assert_eq!(
        borrowed::source(
            Provider::Codex,
            borrowed::Host::MacOs,
            Some(Path::new("/profiles/work")),
            home
        ),
        borrowed::Source::Codex {
            auth_json: PathBuf::from("/profiles/work/auth.json")
        }
    );
}

/// The stock profile of each program, which is what an entry with no path
/// designates.
#[test]
fn a_profile_with_no_path_is_the_stock_one() {
    let home = Path::new("/Users/husni");

    assert_eq!(
        borrowed::source(Provider::Codex, borrowed::Host::MacOs, None, home),
        borrowed::Source::Codex {
            auth_json: PathBuf::from("/Users/husni/.codex/auth.json")
        }
    );
    assert_eq!(
        borrowed::source(Provider::Anthropic, borrowed::Host::MacOs, None, home),
        borrowed::Source::Claude(borrowed::ClaudeSource::Keychain {
            service: "Claude Code-credentials".to_owned()
        })
    );
}

/// Naming the stock directory is not the same as leaving the path out: on
/// macOS it resolves to a different keychain item, which is the whole reason
/// absence is carried through rather than resolved to a path first.
#[test]
fn naming_the_stock_claude_directory_is_a_different_profile() {
    let home = Path::new("/Users/husni");

    let stock = borrowed::source(Provider::Anthropic, borrowed::Host::MacOs, None, home);
    let named = borrowed::source(
        Provider::Anthropic,
        borrowed::Host::MacOs,
        Some(Path::new("/Users/husni/.claude")),
        home,
    );

    assert_ne!(stock, named);
}

// --- reading one profile's grant ------------------------------------------
//
// Through a fake reader: the rules being checked are about what a source
// yields, and a test that needs a real keychain and a signed-in profile is a
// test that stops running.

use proxenos::auth::borrowed::read;
use proxenos::error::ProxyError;
use std::collections::HashMap;

struct FakeReader(HashMap<String, String>);

impl FakeReader {
    fn holding(source: &borrowed::Source, raw: &str) -> Self {
        Self(HashMap::from([(source.label(), raw.to_owned())]))
    }

    fn empty() -> Self {
        Self(HashMap::new())
    }
}

impl read::GrantReader for FakeReader {
    fn read(&self, source: &borrowed::Source) -> Result<Option<String>, ProxyError> {
        Ok(self.0.get(&source.label()).cloned())
    }
}

fn profile(name: &str, provider: Provider, config_dir: Option<&str>) -> read::Profile {
    read::Profile {
        name: name.to_owned(),
        provider,
        config_dir: config_dir.map(PathBuf::from),
    }
}

const HOME: &str = "/Users/husni";

fn read_grant(
    reader: &dyn read::GrantReader,
    profile: &read::Profile,
) -> Result<read::Grant, ProxyError> {
    read::grant(reader, profile, borrowed::Host::MacOs, Path::new(HOME))
}

/// A Codex profile describes its account from the id token it carries.
#[test]
fn a_codex_grant_is_described_by_its_id_token() {
    let profile = profile("work", Provider::Codex, Some("/profiles/work"));
    let source = profile.source(borrowed::Host::MacOs, Path::new(HOME));
    let raw = auth_json(serde_json::json!({
        "id_token": token_with(serde_json::json!({
            "email": "someone@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_123",
                "chatgpt_plan_type": "team",
            },
        })),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = read_grant(&FakeReader::holding(&source, &raw), &profile).expect("reads");

    assert_eq!(grant.plan.as_deref(), Some("team"));
    assert_eq!(grant.email.as_deref(), Some("someone@example.com"));
    assert_eq!(grant.refresh_token_expires_at, None);
}

/// A Claude grant carries a subscription type and no email, and the second
/// expiry that decides whether a refresh can be asked for at all.
#[test]
fn a_claude_grant_carries_its_plan_and_its_refresh_deadline() {
    let profile = profile("personal", Provider::Anthropic, None);
    let source = profile.source(borrowed::Host::MacOs, Path::new(HOME));
    let raw = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 1_800_000_000_000u64,
        "refreshTokenExpiresAt": 1_890_000_000_000u64,
        "subscriptionType": "max",
    }));

    let grant = read_grant(&FakeReader::holding(&source, &raw), &profile).expect("reads");

    assert_eq!(grant.plan.as_deref(), Some("max"));
    assert_eq!(grant.email, None);
    assert_eq!(grant.refresh_token_expires_at, Some(1_890_000_000));
}

/// A source that is not there at all is the same answer as one holding
/// nothing usable: sign in to that profile. The remedy names the right program.
#[test]
fn an_absent_source_reads_as_a_refusal_naming_the_remedy() {
    let codex = read_grant(
        &FakeReader::empty(),
        &profile("work", Provider::Codex, Some("/profiles/work")),
    )
    .expect_err("nothing is there");
    assert!(codex.to_string().contains("codex login"), "was: {codex}");
    assert!(
        codex.to_string().contains("/profiles/work/auth.json"),
        "was: {codex}"
    );

    let claude = read_grant(
        &FakeReader::empty(),
        &profile("personal", Provider::Anthropic, None),
    )
    .expect_err("nothing is there");
    assert!(claude.to_string().contains("`claude`"), "was: {claude}");
    assert!(
        claude.to_string().contains("Claude Code-credentials"),
        "was: {claude}"
    );
}

/// A source holding something unreadable refuses with the store named, rather
/// than being reported as a profile that does not exist.
#[test]
fn an_unreadable_source_names_the_store() {
    let profile = profile("work", Provider::Codex, Some("/profiles/work"));
    let source = profile.source(borrowed::Host::MacOs, Path::new(HOME));

    let error =
        read_grant(&FakeReader::holding(&source, "{ not json"), &profile).expect_err("unreadable");

    assert!(
        error.to_string().contains("/profiles/work/auth.json"),
        "was: {error}"
    );
}

// --- the declared profiles as a store -------------------------------------

use proxenos::auth::borrowed::store::BorrowedStore;
use proxenos::auth::store::AccountStore;
use proxenos::auth::store::CredentialStore;

/// A store over `profiles`, whose sources hold whatever `contents` says, and
/// whose selection file lives in `dir`.
fn store(
    dir: &Path,
    profiles: Vec<read::Profile>,
    contents: &[(&read::Profile, String)],
) -> BorrowedStore {
    let held = contents
        .iter()
        .map(|(profile, raw)| {
            (
                profile
                    .source(borrowed::Host::MacOs, Path::new(HOME))
                    .label(),
                raw.clone(),
            )
        })
        .collect();

    BorrowedStore::new(
        profiles,
        Box::new(FakeReader(held)),
        Ok(proxenos::auth::borrowed::store::Platform {
            host: borrowed::Host::MacOs,
            home: PathBuf::from(HOME),
        }),
        proxenos::auth::selection::Selection::new(dir.join("selected.json")),
    )
}

fn a_codex_grant() -> String {
    auth_json(serde_json::json!({
        "id_token": id_token("acct_123", "team"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }))
}

/// One declared profile is the one serving turns. There is nothing to choose
/// between, and making an operator choose anyway is ceremony.
#[test]
fn a_lone_profile_serves_without_being_selected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);

    let loaded = store.load().expect("loads").expect("a grant");

    assert_eq!(loaded.account_id.as_deref(), Some("acct_123"));
}

/// Two profiles and no choice is refused rather than resolved to whichever
/// comes first: the choice decides whose subscription pays.
#[test]
fn two_profiles_and_no_selection_refuses() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let spare = profile("spare", Provider::Codex, Some("/profiles/spare"));
    let store = store(
        dir.path(),
        vec![work.clone(), spare],
        &[(&work, a_codex_grant())],
    );

    let error = store.load().expect_err("ambiguous").to_string();

    assert!(error.contains("accounts --use"), "was: {error}");
}

/// Choosing one records it on our side, and the choice survives being read
/// back by a store built fresh over the same directory.
#[test]
fn choosing_a_profile_is_recorded_and_read_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let spare = profile("spare", Provider::Codex, Some("/profiles/spare"));
    let contents = [(&spare, a_codex_grant())];

    let first = store(dir.path(), vec![work.clone(), spare.clone()], &contents);
    first.select("spare").expect("selects");
    drop(first);

    let second = store(
        dir.path(),
        vec![work, spare.clone()],
        &[(&spare, a_codex_grant())],
    );
    assert!(second.load().expect("loads").is_some());
    let listed = second.accounts().expect("lists");
    assert!(listed.iter().any(|it| it.name == "spare" && it.selected));
    assert!(listed.iter().any(|it| it.name == "work" && !it.selected));
}

/// A name nothing is declared under is refused, and the refusal says where to
/// look for the ones that are.
#[test]
fn choosing_an_undeclared_profile_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = store(
        dir.path(),
        vec![profile("work", Provider::Codex, Some("/profiles/work"))],
        &[],
    );

    let error = store.select("nowhere").expect_err("refused").to_string();

    assert!(error.contains("nowhere"), "was: {error}");
}

/// A selection left behind by an entry the operator has since deleted is
/// refused by name, rather than silently falling through to another account.
#[test]
fn a_selection_naming_a_deleted_profile_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let gone = profile("gone", Provider::Codex, Some("/profiles/gone"));
    let kept = profile("kept", Provider::Codex, Some("/profiles/kept"));

    store(dir.path(), vec![gone, kept.clone()], &[])
        .select("gone")
        .expect("selects");

    let error = store(dir.path(), vec![kept], &[])
        .load()
        .expect_err("refused")
        .to_string();

    assert!(error.contains("gone"), "was: {error}");
    assert!(error.contains("accounts --use"), "was: {error}");
}

/// Every write refuses, names the profile, and says who may change it. The
/// refresh path is the one that matters: taking it would rotate a token the
/// owning program still holds.
#[test]
fn every_write_refuses_naming_the_profile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);
    let credentials = store.load().expect("loads").expect("a grant");

    let refusals = [
        store.save(&credentials).expect_err("save"),
        store.clear().expect_err("clear"),
        store.add(&credentials, Some("work")).expect_err("add"),
        store.remove("work").expect_err("remove"),
        store.rename("work", "other").expect_err("rename"),
        store
            .add_key("work", "sk-test", Provider::Codex)
            .expect_err("add_key"),
        store.save_for("work", &credentials).expect_err("save_for"),
    ];

    for refusal in refusals {
        let message = refusal.to_string();
        assert!(message.contains("work"), "was: {message}");
        assert!(message.contains("never writes"), "was: {message}");
    }
}

/// A declared profile nobody has signed into is still listed. Dropping it
/// would read as an entry the operator never wrote.
#[test]
fn a_profile_with_no_grant_is_still_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let empty = profile("empty", Provider::Codex, Some("/profiles/empty"));
    let store = store(
        dir.path(),
        vec![work.clone(), empty],
        &[(&work, a_codex_grant())],
    );

    let listed = store.accounts().expect("lists");

    let row = listed.iter().find(|it| it.name == "empty").expect("listed");
    assert_eq!(row.account_id, None);
    assert_eq!(row.expires_at, None);
    assert_eq!(row.provider, "codex");
}

/// A pinned tier asks for one account by name, and the selection is the wrong
/// answer to that question.
#[test]
fn a_named_profile_answers_regardless_of_the_selection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let spare = profile("spare", Provider::Codex, Some("/profiles/spare"));
    let other = auth_json(serde_json::json!({
        "id_token": id_token("acct_spare", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.spare",
        "account_id": "acct_spare",
    }));
    let store = store(
        dir.path(),
        vec![work.clone(), spare.clone()],
        &[(&work, a_codex_grant()), (&spare, other)],
    );
    store.select("work").expect("selects");

    let credential = store.credential_for("spare").expect("reads spare");

    assert_eq!(
        credential.grant().and_then(|it| it.account_id.as_deref()),
        Some("acct_spare")
    );
}

// --- asking the owning program to refresh ---------------------------------

use proxenos::auth::borrowed::poke;
use proxenos::auth::borrowed::poke::Client as _;
use std::sync::Arc;
use std::sync::Mutex;

/// A client that records that it was run, and never runs anything.
#[derive(Default)]
struct FakeClient {
    runs: Mutex<Vec<Option<PathBuf>>>,
}

impl poke::Client for FakeClient {
    fn refresh(&self, config_dir: Option<&Path>) -> Result<(), ProxyError> {
        self.runs
            .lock()
            .expect("not poisoned")
            .push(config_dir.map(Path::to_path_buf));
        Ok(())
    }
}

const NOW: u64 = 1_800_000_000;

/// A grant that has not lapsed is left alone. Nothing is run, and no quota is
/// spent making sure of something that is already true.
#[test]
fn a_usable_grant_is_not_poked() {
    assert_eq!(
        poke::decide(Provider::Anthropic, Some(NOW + 60), None, NOW),
        poke::Decision::Usable
    );
    assert_eq!(
        poke::decide(Provider::Codex, Some(NOW + 60), None, NOW),
        poke::Decision::Usable
    );
}

/// A lapsed Claude grant whose refresh token is still alive is the one case
/// worth asking about.
#[test]
fn a_lapsed_claude_grant_is_worth_asking_about() {
    assert_eq!(
        poke::decide(Provider::Anthropic, Some(NOW - 1), Some(NOW + 86_400), NOW),
        poke::Decision::Ask
    );
}

/// Measured: when the client fails to refresh it blanks its own stored item.
/// So a profile whose refresh token has already lapsed is never run — asking
/// would destroy what is left of the grant rather than renew it.
#[test]
fn a_dead_refresh_token_is_never_poked() {
    let decision = poke::decide(Provider::Anthropic, Some(NOW - 1), Some(NOW - 1), NOW);

    match decision {
        poke::Decision::Hopeless(reason) => {
            assert!(reason.contains("blank"), "was: {reason}");
            assert!(reason.contains("Sign in"), "was: {reason}");
        }
        other => panic!("a dead refresh token must not be poked: {other:?}"),
    }
}

/// A profile written before the client recorded that expiry is still asked
/// about: unknown is not dead, and if it turns out to be dead the operator has
/// to sign in again either way.
#[test]
fn an_unknown_refresh_deadline_is_still_asked_about() {
    assert_eq!(
        poke::decide(Provider::Anthropic, Some(NOW - 1), None, NOW),
        poke::Decision::Ask
    );
}

/// Codex is never run. Its grant refreshes only on a real turn, which spends
/// quota and rotates the token, and one failing run was measured sending
/// fourteen refresh requests.
#[test]
fn a_codex_grant_is_never_poked() {
    match poke::decide(Provider::Codex, Some(NOW - 1), None, NOW) {
        poke::Decision::Hopeless(reason) => {
            assert!(reason.contains("spends quota"), "was: {reason}");
            assert!(reason.contains("codex"), "was: {reason}");
        }
        other => panic!("Codex must never be poked: {other:?}"),
    }
}

/// A grant whose expiry cannot be read at all is treated as lapsed, the same
/// way `needs_refresh` treats it.
#[test]
fn an_unreadable_expiry_is_treated_as_lapsed() {
    assert_eq!(
        poke::decide(Provider::Anthropic, None, Some(NOW + 60), NOW),
        poke::Decision::Ask
    );
}

/// The run happens under a lock, and the profile it names is the one handed
/// to the client.
#[test]
fn the_client_is_run_against_the_profile_under_a_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = FakeClient::default();
    let lock = poke::lock_path(dir.path(), "work");

    poke::under_lock(&client, &lock, Some(Path::new("/profiles/work"))).expect("runs");

    assert_eq!(
        client.runs.lock().expect("not poisoned").as_slice(),
        [Some(PathBuf::from("/profiles/work"))]
    );
    assert!(lock.exists(), "the lock file is left in place for reuse");
}

/// The stock profile is run with no variable set, which is what makes it the
/// stock profile.
#[test]
fn the_stock_profile_is_run_with_nothing_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = FakeClient::default();

    poke::under_lock(&client, &poke::lock_path(dir.path(), "personal"), None).expect("runs");

    assert_eq!(client.runs.lock().expect("not poisoned").as_slice(), [None]);
}

/// Two profiles do not wait on each other: they are two clients writing two
/// different stores, and serialising them buys nothing.
#[test]
fn two_profiles_take_different_locks() {
    let dir = tempfile::tempdir().expect("tempdir");

    assert_ne!(
        poke::lock_path(dir.path(), "work"),
        poke::lock_path(dir.path(), "personal")
    );
}

/// A run that fails releases the lock. Holding it would make the next caller
/// wait for a run that is not happening.
#[test]
fn a_failed_run_releases_the_lock() {
    struct Failing;
    impl poke::Client for Failing {
        fn refresh(&self, _config_dir: Option<&Path>) -> Result<(), ProxyError> {
            Err(ProxyError::authentication("no".to_owned()))
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let lock = poke::lock_path(dir.path(), "work");

    poke::under_lock(&Failing, &lock, None).expect_err("the run failed");

    // The proof it was released: the next call takes it without blocking.
    let client = Arc::new(FakeClient::default());
    poke::under_lock(client.as_ref(), &lock, None).expect("the lock was free");
    assert_eq!(client.runs.lock().expect("not poisoned").len(), 1);
}

// --- what a borrowed grant puts on the wire -------------------------------

use proxenos::auth::authorize::AccountAuthorizer;
use proxenos::auth::authorize::Authorizer;
use proxenos::auth::authorize::Kind;
use proxenos::auth::grants::Grants;
use proxenos::auth::grants::SystemClock;

fn authorizer(store: Arc<BorrowedStore>) -> AccountAuthorizer {
    let grants = Arc::new(Grants::new(
        Arc::clone(&store) as Arc<dyn CredentialStore>,
        Arc::new(SystemClock),
    ));
    AccountAuthorizer::new(store as Arc<dyn AccountStore>, grants)
}

fn header<'a>(
    authorization: &'a proxenos::auth::authorize::Authorization,
    name: &str,
) -> Option<&'a str> {
    authorization
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn a_claude_blob() -> String {
    keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 4_000_000_000_000u64,
        "refreshTokenExpiresAt": 4_100_000_000_000u64,
        "subscriptionType": "max",
    }))
}

/// A borrowed grant on the second provider is spent at that provider's
/// endpoint, and the relay asks whose account it is rather than which kind of
/// credential it is. Before this, the relay pinned itself to a key and refused
/// every grant on that provider.
#[tokio::test]
async fn a_borrowed_claude_grant_is_authorized_for_its_own_provider() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let store = Arc::new(store(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, a_claude_blob())],
    ));

    let authorization = authorizer(store).authorize(None).await.expect("authorizes");

    assert_eq!(authorization.provider, Provider::Anthropic);
    assert_eq!(authorization.kind, Kind::Subscription);
    authorization
        .clone()
        .for_provider(Provider::Anthropic)
        .expect("it belongs to that provider's endpoint");
    let refusal = authorization
        .for_provider(Provider::Codex)
        .expect_err("and to no other");
    assert!(refusal.message.contains("anthropic"), "{}", refusal.message);
}

/// Each provider's subscription path wants different headers, and sending one
/// provider's extras to the other is how a borrowed grant fails with a message
/// about the wrong half.
#[tokio::test]
async fn each_provider_gets_only_the_headers_it_asks_for() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = Arc::new(store(
        dir.path(),
        vec![personal.clone(), work.clone()],
        &[
            (&personal, a_claude_blob()),
            (
                &work,
                auth_json(serde_json::json!({
                    "id_token": id_token("acct_123", "team"),
                    "access_token": access_token(4_000_000_000),
                    "refresh_token": "rt.1.borrowed",
                    "account_id": "acct_123",
                })),
            ),
        ],
    ));
    let authorizer = authorizer(store);

    let claude = authorizer
        .authorize(Some("personal"))
        .await
        .expect("authorizes");
    assert_eq!(header(&claude, "anthropic-beta"), Some("oauth-2025-04-20"));
    assert_eq!(header(&claude, "originator"), None);
    assert_eq!(header(&claude, "chatgpt-account-id"), None);

    let codex = authorizer
        .authorize(Some("work"))
        .await
        .expect("authorizes");
    assert_eq!(header(&codex, "chatgpt-account-id"), Some("acct_123"));
    assert!(header(&codex, "originator").is_some());
    assert_eq!(header(&codex, "anthropic-beta"), None);
}

/// A lapsed borrowed grant refuses the turn rather than being refreshed here.
#[tokio::test]
async fn a_lapsed_borrowed_grant_refuses_the_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let lapsed = keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": 1_000_000u64,
    }));
    let store = Arc::new(store(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, lapsed)],
    ));

    let refusal = authorizer(store)
        .authorize(None)
        .await
        .expect_err("a lapsed grant is refused");

    assert!(refusal.message.contains("expired"), "{}", refusal.message);
    assert!(
        refusal.message.contains("owns the profile"),
        "{}",
        refusal.message
    );
}

// --- saying who pays ------------------------------------------------------

use proxenos::render;

fn listing(account: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "accounts": [account] })
}

/// A borrowed row names the directory it was read from. A name is the
/// operator's own label; this is the thing they can go and look at.
#[test]
fn a_listing_names_the_profile_a_grant_came_from() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);

    let listed = store.accounts().expect("lists");

    assert_eq!(
        listed[0].source.as_deref(),
        Some("/profiles/work/auth.json")
    );
    let rendered = render::accounts(&serde_json::json!({
        "accounts": serde_json::to_value(&listed).expect("serializes"),
    }));
    assert!(rendered.contains("/profiles/work/auth.json"), "{rendered}");
}

/// A profile that has become a different account is marked on the row that
/// serves turns. Nothing else would say so, and the consequence is turns
/// billed to an account nobody pointed at them.
#[test]
fn a_profile_that_changed_account_is_marked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let first = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);
    first.select("work").expect("selects");
    drop(first);

    // The same directory, signed in as somebody else.
    let somebody_else = auth_json(serde_json::json!({
        "id_token": id_token("acct_other", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.other",
        "account_id": "acct_other",
    }));
    let listed = store(dir.path(), vec![work.clone()], &[(&work, somebody_else)])
        .accounts()
        .expect("lists");

    assert!(listed[0].identity_changed, "the identity moved");
    let rendered = render::accounts(&serde_json::json!({
        "accounts": serde_json::to_value(&listed).expect("serializes"),
    }));
    assert!(rendered.contains("different account"), "{rendered}");
}

/// The same profile, still the same account, is not marked. A warning that
/// fires on every launch is one nobody reads.
#[test]
fn an_unchanged_profile_is_not_marked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let contents = [(&work, a_codex_grant())];
    let first = store(dir.path(), vec![work.clone()], &contents);
    first.select("work").expect("selects");
    drop(first);

    let listed = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())])
        .accounts()
        .expect("lists");

    assert!(!listed[0].identity_changed);
}

/// A profile that cannot be read has not changed identity; it has not been
/// read. Marking it would send the operator looking for a switch that never
/// happened.
#[test]
fn an_unreadable_profile_is_not_marked_as_changed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let first = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);
    first.select("work").expect("selects");
    drop(first);

    let listed = store(dir.path(), vec![work], &[])
        .accounts()
        .expect("lists");

    assert!(!listed[0].identity_changed);
}

/// The line a launch prints: the label, the provider, the identity that gets
/// billed, and the plan.
#[test]
fn the_launch_line_names_the_identity_and_not_only_the_label() {
    let line = render::serving_line(&listing(serde_json::json!({
        "name": "work",
        "provider": "codex",
        "email": "someone@example.test",
        "plan": "team",
        "selected": true,
    })))
    .expect("a line");

    assert!(line.contains("work"), "{line}");
    assert!(line.contains("codex"), "{line}");
    assert!(line.contains("someone@example.test"), "{line}");
    assert!(line.contains("team"), "{line}");
}

/// It carries the identity warning too, at the one moment there is a person
/// deciding whether to start the session.
#[test]
fn the_launch_line_carries_the_identity_warning() {
    let line = render::serving_line(&listing(serde_json::json!({
        "name": "work",
        "provider": "codex",
        "account_id": "acct_other",
        "selected": true,
        "identity_changed": true,
    })))
    .expect("a line");

    assert!(line.contains("different account"), "{line}");
}

/// Nothing serving means nothing said: the daemon refuses such a launch with a
/// message of its own, and this would only get in front of it.
#[test]
fn the_launch_line_is_absent_when_nothing_serves() {
    assert_eq!(
        render::serving_line(&listing(
            serde_json::json!({ "name": "work", "selected": false })
        )),
        None
    );
}

/// `status` names the store behind the serving account, and marks an identity
/// that has moved. A front-end reads that answer, and a name alone does not
/// say which directory is being spent.
#[test]
fn the_status_answer_carries_the_source_and_the_identity_mark() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);
    store.select("work").expect("selects");

    let serialized =
        serde_json::to_value(&store.accounts().expect("lists")[0]).expect("serializes");

    assert_eq!(
        serialized["source"],
        serde_json::json!("/profiles/work/auth.json")
    );
    // Absent rather than false: an unchanged account says nothing about it.
    assert_eq!(serialized.get("identity_changed"), None);
}

/// A borrowed Claude grant carries no address and no account id — its store
/// holds neither — but it does say which subscription it is. The row says that
/// rather than saying the one thing it cannot.
#[test]
fn a_row_with_no_identity_falls_back_to_the_plan() {
    let rendered = render::accounts(&listing(serde_json::json!({
        "name": "personal-claude",
        "kind": "grant",
        "provider": "anthropic",
        "plan": "max",
        "selected": true,
    })));

    assert!(rendered.contains("max"), "{rendered}");
    assert!(!rendered.contains("id unknown"), "{rendered}");
}

/// A grant that has an id keeps showing it: the plan is the fallback, not a
/// replacement for the identity that gets billed.
#[test]
fn an_identity_still_outranks_the_plan() {
    let rendered = render::accounts(&listing(serde_json::json!({
        "name": "work",
        "kind": "grant",
        "provider": "codex",
        "email": "someone@example.test",
        "plan": "team",
        "selected": true,
    })));

    assert!(rendered.contains("someone@example.test"), "{rendered}");
}

/// A key is one secret and says so, whatever else the row carries.
#[test]
fn a_key_still_reads_as_a_key() {
    let rendered = render::accounts(&listing(serde_json::json!({
        "name": "openai-api",
        "kind": "key",
        "provider": "codex",
        "selected": false,
    })));

    assert!(rendered.contains("key"), "{rendered}");
}

/// A host nothing has been checked on refuses at the profile that needs it,
/// not at startup.
///
/// An operator with no `[profiles]` is spending a key, which needs neither a
/// checked host nor a home directory. Refusing to build the store there would
/// refuse a configuration that is entirely valid — and on Windows that is
/// every configuration.
#[test]
fn an_unchecked_host_does_not_stop_a_store_with_no_profiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BorrowedStore::new(
        Vec::new(),
        Box::new(FakeReader::empty()),
        Err("nobody has checked this platform".to_owned()),
        proxenos::auth::selection::Selection::new(dir.path().join("selected.json")),
    );

    assert!(store.accounts().expect("lists").is_empty());
}

/// Declare a profile on such a host and the refusal names it and says why,
/// rather than reporting a profile that was never signed into.
#[test]
fn an_unchecked_host_refuses_the_profile_that_needs_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BorrowedStore::new(
        vec![profile("work", Provider::Codex, Some("/profiles/work"))],
        Box::new(FakeReader::empty()),
        Err("nobody has checked this platform".to_owned()),
        proxenos::auth::selection::Selection::new(dir.path().join("selected.json")),
    );

    // Still listed: it is an entry the operator wrote.
    let listed = store.accounts().expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, None);

    let refusal = store.load().expect_err("cannot be read here").to_string();
    assert!(refusal.contains("work"), "{refusal}");
    assert!(refusal.contains("checked this platform"), "{refusal}");
}

// --- the refresh a lapsed profile actually gets ---------------------------

use proxenos::auth::accounts::Accounts;
use proxenos::auth::store::FileStore;

/// A store over one profile, whose client records that it was run.
fn accounts_over(
    dir: &Path,
    profiles: Vec<read::Profile>,
    contents: &[(&read::Profile, String)],
    client: Arc<FakeClient>,
) -> Accounts {
    struct Recording(Arc<FakeClient>);
    impl poke::Client for Recording {
        fn refresh(&self, config_dir: Option<&Path>) -> Result<(), ProxyError> {
            self.0.refresh(config_dir)
        }
    }

    Accounts::new(
        store(dir, profiles, contents),
        FileStore::new(dir.join("credentials.json")),
        proxenos::auth::selection::Selection::new(dir.join("selected.json")),
        Box::new(Recording(client)),
        dir.to_path_buf(),
    )
}

fn claude_blob(expires_at: u64, refresh_expires_at: u64) -> String {
    keychain_blob(serde_json::json!({
        "accessToken": "sk-ant-oat01-borrowed",
        "refreshToken": "sk-ant-ort01-borrowed",
        "expiresAt": expires_at * 1_000,
        "refreshTokenExpiresAt": refresh_expires_at * 1_000,
        "subscriptionType": "max",
    }))
}

/// A lapsed Claude profile gets the client run against it, and the caller
/// waits: the figure it wants is the one that comes after the refresh.
#[test]
fn asking_for_a_lapsed_claude_profile_runs_the_client() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let client = Arc::new(FakeClient::default());
    let accounts = accounts_over(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, claude_blob(1, 4_000_000_000))],
        Arc::clone(&client),
    );

    let ran = accounts
        .refresh_borrowed("personal")
        .expect("asking is allowed here");

    assert!(ran, "the client was run");
    assert_eq!(
        client.runs.lock().expect("not poisoned").as_slice(),
        [Some(PathBuf::from("/profiles/personal"))]
    );
}

/// A profile that has not lapsed is left alone. A run per request would spend
/// a turn to confirm something already true.
#[test]
fn asking_for_a_live_profile_runs_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let client = Arc::new(FakeClient::default());
    let accounts = accounts_over(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, claude_blob(4_000_000_000, 4_100_000_000))],
        Arc::clone(&client),
    );

    assert!(!accounts.refresh_borrowed("personal").expect("allowed"));
    assert!(client.runs.lock().expect("not poisoned").is_empty());
}

/// Codex is never run, and the refusal says why rather than leaving the caller
/// to wonder what happened.
#[test]
fn asking_for_a_lapsed_codex_profile_refuses_without_running_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let client = Arc::new(FakeClient::default());
    let lapsed = auth_json(serde_json::json!({
        "access_token": access_token(1),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));
    let accounts = accounts_over(
        dir.path(),
        vec![work.clone()],
        &[(&work, lapsed)],
        Arc::clone(&client),
    );

    let refusal = accounts
        .refresh_borrowed("work")
        .expect_err("Codex is never run")
        .to_string();

    assert!(refusal.contains("work"), "{refusal}");
    assert!(refusal.contains("spends quota"), "{refusal}");
    assert!(client.runs.lock().expect("not poisoned").is_empty());
}

/// A profile whose refresh token has lapsed too is not run: the client would
/// blank the stored grant rather than renew it.
#[test]
fn asking_for_a_dead_refresh_token_refuses_without_running_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let client = Arc::new(FakeClient::default());
    let accounts = accounts_over(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, claude_blob(1, 2))],
        Arc::clone(&client),
    );

    let refusal = accounts
        .refresh_borrowed("personal")
        .expect_err("nothing can be asked")
        .to_string();

    assert!(refusal.contains("blank"), "{refusal}");
    assert!(client.runs.lock().expect("not poisoned").is_empty());
}

/// A key has nobody to ask, and says so by answering that nothing was run.
#[test]
fn asking_about_a_key_runs_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = Arc::new(FakeClient::default());
    let accounts = accounts_over(dir.path(), Vec::new(), &[], Arc::clone(&client));

    assert!(!accounts.refresh_borrowed("whatever").expect("allowed"));
    assert!(client.runs.lock().expect("not poisoned").is_empty());
}

/// The real client, as opposed to the fake one every rule above is proven
/// with: it runs the program it was given rather than whatever `PATH` resolves
/// `claude` to, and it hands it the profile directory.
///
/// This is what `claude_program` exists for. A daemon started by launchd has a
/// minimal `PATH`, and the bare name does not resolve there.
#[cfg(unix)]
#[test]
fn the_client_runs_the_program_it_was_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record = dir.path().join("argv");
    let script = stand_in(
        dir.path(),
        &format!(
            "#!/bin/sh\nprintf '%s %s\\n' \"$CLAUDE_CONFIG_DIR\" \"$*\" > {}\n",
            record.display()
        ),
    );

    borrowed::poke::ClaudeClient::new(&script, borrowed::poke::DEADLINE)
        .refresh(Some(Path::new("/profiles/work")))
        .expect("it ran");

    let recorded = std::fs::read_to_string(&record).expect("it recorded");
    assert_eq!(recorded.trim(), "/profiles/work -p ok --model haiku");
}

/// A program that is not there names what was tried, so an operator who set
/// `claude_program` to the wrong path is told which path that was.
#[test]
fn a_client_that_cannot_be_run_names_the_program() {
    let refusal =
        borrowed::poke::ClaudeClient::new("/nowhere/at/all/claude", borrowed::poke::DEADLINE)
            .refresh(None)
            .expect_err("nothing to run")
            .to_string();

    assert!(refusal.contains("/nowhere/at/all/claude"), "{refusal}");
}

/// A client that never exits is killed at the deadline rather than held onto,
/// and the refusal says the profile was left as it was.
#[cfg(unix)]
#[test]
fn a_client_that_does_not_finish_is_given_up_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = stand_in(dir.path(), "#!/bin/sh\nsleep 30\n");

    let refusal = borrowed::poke::ClaudeClient::new(&script, std::time::Duration::from_millis(200))
        .refresh(None)
        .expect_err("it never finishes")
        .to_string();

    assert!(refusal.contains("did not finish"), "{refusal}");
    assert!(refusal.contains("left alone"), "{refusal}");
}

/// An executable stand-in for the client, so the real one is never run and no
/// turn is ever spent proving how it is run.
#[cfg(unix)]
fn stand_in(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join("stand-in");
    std::fs::write(&script, body).expect("written");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("executable");
    script
}

/// Forgetting with nothing named means the account serving turns, and a
/// borrowed profile is not this daemon's to forget. Before this it forwarded
/// to the key store, which removed whatever *it* had marked as chosen and let
/// the answer claim the profile had gone.
#[test]
fn forgetting_the_serving_profile_refuses_and_leaves_the_keys_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let accounts = accounts_over(
        dir.path(),
        vec![work.clone()],
        &[(&work, codex_auth(NOW + 3_600))],
        Arc::new(FakeClient::default()),
    );
    accounts
        .add_key("spare", "sk-test", Provider::Anthropic)
        .expect("a key is stored");
    accounts.select("work").expect("the profile serves");

    let refusal = accounts
        .clear()
        .expect_err("a borrowed profile is not forgotten here")
        .to_string();

    assert!(refusal.contains("work"), "{refusal}");
    assert!(refusal.contains("borrowed profile"), "{refusal}");
    let listed = accounts.accounts().expect("lists");
    assert!(
        listed.iter().any(|account| account.name == "spare"),
        "the key must survive a refusal about another account"
    );
}

/// Where a key serves, forgetting without a name removes that key and takes
/// the selection with it — the same thing naming it would have done.
#[test]
fn forgetting_the_serving_key_removes_that_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let accounts = accounts_over(
        dir.path(),
        vec![work.clone()],
        &[(&work, codex_auth(NOW + 3_600))],
        Arc::new(FakeClient::default()),
    );
    accounts
        .add_key("spare", "sk-test", Provider::Anthropic)
        .expect("a key is stored");
    accounts.select("spare").expect("the key serves");

    accounts.clear().expect("a key is this daemon's to forget");

    let listed = accounts.accounts().expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "work");
    assert!(
        listed[0].selected,
        "the selection must not still name what was forgotten"
    );
}

/// Nothing held at all is not an error: forgetting has always been safe to run
/// twice, and the second run finds an empty store.
#[test]
fn forgetting_an_empty_store_is_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let accounts = accounts_over(dir.path(), Vec::new(), &[], Arc::new(FakeClient::default()));

    accounts.clear().expect("nothing to forget");
}

/// A signed-in `auth.json`, whose grant is live until `expires_at`.
fn codex_auth(expires_at: u64) -> String {
    auth_json(serde_json::json!({
        "access_token": access_token(expires_at),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }))
}

// --- the reader that actually touches the machine -------------------------
//
// Everything above is proven against grants that were never written anywhere,
// which is the point of the trait. What that leaves unproven is the one
// implementation that does touch a disk and a keychain, so these read real
// bytes off a real filesystem.
//
// The keychain's *success* path is not here and cannot be: proving it needs an
// item in the operator's own login keychain, which a test suite has no
// business writing. What is proven is the answer that decides whether a
// profile reads as absent or as broken.

use proxenos::auth::borrowed::read::GrantReader as _;
use proxenos::auth::borrowed::read::HostReader;

/// A profile directory that was signed into: the file is read back verbatim,
/// so what the parser above receives is what the owning program wrote.
#[test]
fn the_host_reader_reads_a_profile_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("auth.json");
    let raw = codex_auth(NOW + 3_600);
    std::fs::write(&path, &raw).expect("written");

    let read = HostReader
        .read(&borrowed::Source::Codex { auth_json: path })
        .expect("the file is readable");

    assert_eq!(read.as_deref(), Some(raw.as_str()));
}

/// A directory that was never signed into is absent, not an error: the store
/// above turns that into "sign in to that profile", which is what it is.
#[test]
fn a_profile_that_was_never_signed_into_reads_as_absent() {
    let dir = tempfile::tempdir().expect("tempdir");

    let read = HostReader
        .read(&borrowed::Source::Claude(borrowed::ClaudeSource::File {
            path: dir.path().join(".credentials.json"),
        }))
        .expect("absent is an answer");

    assert_eq!(read, None);
}

/// Anything else is reported, naming the path. A profile that cannot be read
/// is a different problem from one that was never signed into, and the two
/// must not arrive as the same sentence.
#[test]
fn a_profile_that_cannot_be_read_names_the_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A directory where a file belongs: readable to `stat`, refused to `read`.
    let path = dir.path().join("auth.json");
    std::fs::create_dir(&path).expect("created");

    let refusal = HostReader
        .read(&borrowed::Source::Codex {
            auth_json: path.clone(),
        })
        .expect_err("this is not a grant")
        .to_string();

    assert!(refusal.contains(&path.display().to_string()), "{refusal}");
}

/// A keychain item that is not there is absent rather than a failure, which is
/// `security` exiting 44. Read wrong, every Claude profile on a machine reads
/// as broken instead of as not signed in.
#[cfg(target_os = "macos")]
#[test]
fn a_keychain_item_that_is_not_there_reads_as_absent() {
    let read = HostReader
        .read(&borrowed::Source::Claude(
            borrowed::ClaudeSource::Keychain {
                service: "proxenos-no-such-item-9f3c2a".to_owned(),
            },
        ))
        .expect("absent is an answer");

    assert_eq!(read, None);
}

// --- what one refresh sweep is allowed to spend ---------------------------

/// A state whose store is these profiles, and whose quota endpoints answer
/// nothing: the assertion is about what was run, not about what was fetched.
fn state_over(accounts: Arc<Accounts>) -> proxenos::control::handler::ControlState {
    let tokens = Arc::new(proxenos::auth::grants::Grants::new(
        Arc::clone(&accounts) as Arc<dyn proxenos::auth::store::CredentialStore>,
        Arc::new(proxenos::auth::grants::SystemClock),
    ));
    proxenos::control::handler::ControlState {
        port: 8787,
        policy: Arc::new(proxenos::policy::Policy::new(
            proxenos::policy::Snapshot::new(
                Vec::new(),
                None,
                proxenos::config::CrossAccountTiers::Refused,
            ),
        )),
        catalog: Arc::new(proxenos::catalog::CatalogSource::fixed(
            proxenos::catalog::Catalog::parse(r#"{"data":[]}"#, 95.0).expect("a catalog"),
        )),
        credentials: accounts as Arc<dyn proxenos::auth::store::AccountStore>,
        capture: Arc::new(proxenos::recorder::Switches::default()),
        usage: Arc::new(proxenos::usage::UsageStore::default()),
        refusals: std::sync::Arc::new(proxenos::auth::refusals::Refusals::default()),
        config: Arc::new(proxenos::config::Config::default()),
        shutdown: Arc::new(proxenos::daemon::Shutdown::default()),
        tokens: Some(tokens),
        // Empty, and never reached with a lapsed grant: the authorization
        // refuses before a request is built. No test may reach the network.
        usage_endpoint: String::new(),
        anthropic_usage_endpoint: String::new(),
        sessions: Arc::new(proxenos::session::SessionStore::new()),
        config_path: None,
    }
}

/// Two lapsed profiles, so the sweep has something to spend its budget on.
fn two_lapsed_profiles(dir: &Path, client: Arc<FakeClient>) -> Arc<Accounts> {
    let first = profile("first", Provider::Anthropic, Some("/profiles/first"));
    let second = profile("second", Provider::Anthropic, Some("/profiles/second"));
    Arc::new(accounts_over(
        dir,
        vec![first.clone(), second.clone()],
        &[
            (&first, claude_blob(1, 4_000_000_000)),
            (&second, claude_blob(1, 4_000_000_000)),
        ],
        client,
    ))
}

/// A sweep asks each lapsed profile in turn while it has budget left. Both
/// profiles here are lapsed and the client returns at once, so both are asked.
#[tokio::test]
async fn a_sweep_asks_every_lapsed_profile_it_has_budget_for() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = Arc::new(FakeClient::default());
    let state = state_over(two_lapsed_profiles(dir.path(), Arc::clone(&client)));

    proxenos::control::handler::refresh_usage_within(
        &state,
        proxenos::control::handler::REFRESH_BUDGET,
    )
    .await
    .expect("the sweep answers");

    assert_eq!(client.runs.lock().expect("not poisoned").len(), 2);
}

/// With the budget already spent, nothing is run at all — and the rows say so,
/// rather than reporting a figure that was never asked for. This is the bound:
/// asking a profile means starting a program and waiting for it, and a sweep
/// that did that once per account has no ceiling a caller can rely on.
#[tokio::test]
async fn a_spent_budget_asks_nothing_and_says_why() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = Arc::new(FakeClient::default());
    let state = state_over(two_lapsed_profiles(dir.path(), Arc::clone(&client)));

    let answer =
        proxenos::control::handler::refresh_usage_within(&state, std::time::Duration::ZERO)
            .await
            .expect("the sweep answers");

    assert!(client.runs.lock().expect("not poisoned").is_empty());

    let rows = answer["accounts"].as_array().expect("a row per account");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["known"], serde_json::json!(false));
        let detail = row["detail"].as_str().expect("a sentence");
        assert!(detail.contains("not asked to refresh"), "{detail}");
    }
}

// --- the host this was never run on ---------------------------------------

/// A Claude profile on Linux reads the same grant out of a file, and the whole
/// path is exercised rather than only the choice of source.
///
/// Nothing in this suite runs on Linux, and the machine this was built on is a
/// macOS one — so the file layout comes from the client rather than from a
/// measurement, and `docs/proxy-behavior.md` §8.4 says which parts those are.
/// What is proven here is that the reader is asked for the file the source
/// names, and that what comes back is parsed exactly as the keychain's bytes
/// are.
#[test]
fn a_claude_profile_on_linux_is_read_from_its_file() {
    let profile = profile("work", Provider::Anthropic, Some("/profiles/work"));
    let source = profile.source(borrowed::Host::Linux, Path::new(HOME));

    assert_eq!(
        source.label(),
        "/profiles/work/.credentials.json",
        "on Linux the grant is a file inside the profile, not a keychain item"
    );

    let reader = FakeReader::holding(&source, &claude_blob(4_000_000_000, 4_100_000_000));
    let grant = read::grant(&reader, &profile, borrowed::Host::Linux, Path::new(HOME))
        .expect("the profile is signed into");

    assert_eq!(grant.credentials.access_token, "sk-ant-oat01-borrowed");
    assert_eq!(grant.credentials.expires_at, Some(4_000_000_000));
    assert_eq!(grant.refresh_token_expires_at, Some(4_100_000_000));
    assert_eq!(grant.plan.as_deref(), Some("max"));
}

/// And a profile that was never signed into on Linux says so naming the file,
/// which is the thing an operator can go and look for.
#[test]
fn an_unsigned_linux_profile_names_the_file_it_looked_for() {
    let profile = profile("work", Provider::Anthropic, Some("/profiles/work"));

    let refusal = read::grant(
        &FakeReader::empty(),
        &profile,
        borrowed::Host::Linux,
        Path::new(HOME),
    )
    .expect_err("nothing is there")
    .to_string();

    assert!(
        refusal.contains("/profiles/work/.credentials.json"),
        "{refusal}"
    );
}

// --- the login that has to be renewed -------------------------------------
//
// The one date a borrowed profile carries that this daemon can do nothing
// about. Past it the client cannot refresh the profile either, and asking it
// to try blanks what is left of the stored grant — so the notice ahead of it
// is the whole mitigation.

const DAY: u64 = 24 * 60 * 60;

/// The date is read from the same field the owning client counts down from,
/// and reaches the listing as its own value rather than as a rendered string.
#[test]
fn a_claude_profile_reports_when_its_login_has_to_be_renewed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let personal = profile("personal", Provider::Anthropic, Some("/profiles/personal"));
    let store = store(
        dir.path(),
        vec![personal.clone()],
        &[(&personal, claude_blob(4_000_000_000, 4_100_000_000))],
    );

    let listed = store.accounts().expect("lists");

    assert_eq!(listed[0].login_expires_at, Some(4_100_000_000));
}

/// A Codex profile carries nothing equivalent: `last_refresh` and an access
/// token expiry say when it was last renewed, not when renewing stops working.
/// Absent is reported rather than filled in with the nearest plausible date.
#[test]
fn a_codex_profile_reports_no_renewal_date() {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = profile("work", Provider::Codex, Some("/profiles/work"));
    let store = store(dir.path(), vec![work.clone()], &[(&work, a_codex_grant())]);

    let listed = store.accounts().expect("lists");

    assert_eq!(listed[0].login_expires_at, None);
    assert_eq!(
        serde_json::to_value(&listed[0])
            .expect("serializes")
            .get("login_expires_at"),
        None,
        "a date nobody can state is absent, not null"
    );
}

/// Inside the notice window the row says how long is left. Outside it the row
/// says nothing: a line carrying a date eleven months of the year is a line
/// the reader learns to skip.
#[test]
fn a_row_counts_down_only_once_the_renewal_is_close() {
    let now = 1_800_000_000;
    let row = |expires_at: u64| {
        render::accounts_at(
            &listing(serde_json::json!({
                "name": "personal",
                "kind": "grant",
                "provider": "anthropic",
                "plan": "max",
                "login_expires_at": expires_at,
                "selected": true,
            })),
            now,
        )
    };

    assert!(row(now + 3 * DAY).contains("login expires in 3 days"));
    assert!(row(now + DAY + 60).contains("login expires tomorrow"));
    assert!(row(now + 60).contains("login expires today"));
    assert!(row(now - 1).contains("login expired"));

    let far = row(now + 30 * DAY);
    assert!(!far.contains("login expires"), "{far}");
}

/// `status` carries the remedy as well as the fact, because that is the report
/// an operator is reading when they are about to act on it.
#[test]
fn status_says_what_renewing_takes_and_what_happens_if_it_lapses() {
    let now = 1_800_000_000;
    let rendered = render::status_at(
        &serde_json::json!({
            "auth": {
                "connected": true,
                "account": "personal",
                "provider": "anthropic",
                "kind": "grant",
                "login_expires_at": now + 2 * DAY,
            },
        }),
        now,
    );

    assert!(rendered.contains("login expires in 2 days"), "{rendered}");
    assert!(rendered.contains("claude auth login"), "{rendered}");
    assert!(rendered.contains("empties what is left"), "{rendered}");
}

/// And says nothing where there is no date to say — every Codex profile, and
/// every key.
#[test]
fn status_is_silent_where_no_renewal_date_is_known() {
    let rendered = render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "work", "provider": "codex", "kind": "grant" },
        }),
        1_800_000_000,
    );

    assert!(!rendered.contains("renew"), "{rendered}");
}

// --- the profiles nobody declared -----------------------------------------
//
// A first run should not require an operator to write down what the programs
// on the machine already know. With `[profiles]` empty the stock profile of
// each program is read, and what is signed in becomes an account.

/// The set that is looked for: each program's stock profile, named plainly,
/// with no directory — because the stock profile is precisely the one no
/// variable designates (§8.4).
#[test]
fn the_discovered_set_is_the_stock_profile_of_each_program() {
    let found = borrowed::discovered();

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].name, "codex");
    assert_eq!(found[0].provider, Provider::Codex);
    assert_eq!(found[1].name, "claude");
    assert_eq!(found[1].provider, Provider::Anthropic);
    assert!(found.iter().all(|profile| profile.config_dir.is_none()));
}

/// A discovered profile that holds no grant is not an account. Nobody asked
/// for it, and reporting "the stock Codex profile was never signed into" on a
/// machine with no Codex on it answers a question nobody put.
#[test]
fn a_discovered_profile_with_no_grant_is_not_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let codex = profile("codex", Provider::Codex, None);
    let claude = profile("claude", Provider::Anthropic, None);
    let store = store(
        dir.path(),
        vec![codex.clone(), claude],
        &[(&codex, a_codex_grant())],
    )
    .discovered();

    let listed = store.accounts().expect("lists");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "codex");
    // And with one account there is nothing to choose between, so it serves.
    assert!(listed[0].selected);
    assert_eq!(
        store.load().expect("loads").expect("a grant").account_id,
        Some("acct_123".to_owned())
    );
}

/// A declared profile that holds no grant is still listed: the operator wrote
/// it, and a row that vanished would read as an entry they never wrote. This
/// is the difference the two sets are for.
#[test]
fn a_declared_profile_with_no_grant_is_still_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let codex = profile("codex", Provider::Codex, None);
    let claude = profile("claude", Provider::Anthropic, None);
    let store = store(
        dir.path(),
        vec![codex.clone(), claude],
        &[(&codex, a_codex_grant())],
    );

    assert_eq!(store.accounts().expect("lists").len(), 2);
}

/// Declaring anything replaces the found set entirely — a written entry is the
/// operator's statement about which identity pays, and a discovered one
/// sitting beside it would be a second opinion nobody asked for.
#[test]
fn declaring_a_profile_stops_the_daemon_looking_for_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = proxenos::config::Config::default();

    let found = Accounts::from_config(&config, dir.path()).expect("builds");
    assert!(found.discovered_profiles());

    config.profiles.insert(
        "work".to_owned(),
        proxenos::config::ProfileConfig {
            provider: Provider::Codex,
            path: Some(PathBuf::from("/profiles/work")),
        },
    );
    let declared = Accounts::from_config(&config, dir.path()).expect("builds");
    assert!(!declared.discovered_profiles());
}

/// The listing says which of the two it is, because a front-end that could not
/// tell would present a found account as one the operator chose.
#[test]
fn the_listing_says_the_accounts_were_found_rather_than_declared() {
    let rendered = render::accounts(&serde_json::json!({
        "discovered": true,
        "accounts": [{
            "name": "claude",
            "kind": "grant",
            "provider": "anthropic",
            "plan": "max",
            "selected": true,
        }],
    }));

    assert!(rendered.contains("found, not declared"), "{rendered}");
    assert!(rendered.contains("[profiles]"), "{rendered}");
}

/// Nothing to serve with and several accounts with no choice between them are
/// different problems, and only one of them is fixed by adding an account.
///
/// Seen on a first run: two profiles found, `accounts` listing both, and
/// `status` advising the operator to declare a profile — which would have
/// added a third and chosen none of them.
#[test]
fn status_tells_a_full_store_to_choose_and_an_empty_one_to_sign_in() {
    let chooseable = render::status(&serde_json::json!({
        "auth": {
            "connected": false,
            "accounts": [
                { "name": "codex", "provider": "codex", "kind": "grant" },
                { "name": "claude", "provider": "anthropic", "kind": "grant" },
            ],
        },
    }));
    assert!(chooseable.contains("accounts --use"), "{chooseable}");

    let empty = render::status(&serde_json::json!({
        "auth": { "connected": false, "accounts": [] },
    }));
    assert!(empty.contains("claude auth login"), "{empty}");
    assert!(empty.contains("login --key"), "{empty}");
    assert!(!empty.contains("accounts --use"), "{empty}");
}

// --- what the backend said about a credential -----------------------------

/// A local refusal and a refused credential wear the same error kind and mean
/// opposite things: one is a profile this daemon could not read, the other is
/// a credential the backend turned away. Only the second is worth telling an
/// operator to sign in over.
#[test]
fn only_what_the_backend_said_is_marked_as_coming_from_it() {
    let ours = ProxyError::authentication("the borrowed grant has expired");
    assert!(!ours.from_upstream);

    let theirs = ProxyError::from_upstream_status(
        axum::http::StatusCode::UNAUTHORIZED,
        "invalid access token",
    );
    assert!(theirs.from_upstream);
    assert_eq!(
        theirs.kind,
        proxenos_core::anthropic::ErrorKind::AuthenticationError
    );
}

/// An unpinned turn is made as whoever is serving, and that is who a refusal
/// belongs to — resolved when it happens rather than when somebody reads it,
/// because a switch afterwards would move the blame to another account.
#[test]
fn a_refusal_is_filed_under_the_account_that_was_serving() {
    let refusals = proxenos::auth::refusals::Refusals::default()
        .serving(Arc::new(|| Some("personal".to_owned())));

    refusals.record(None, 401, "invalid access token");

    let refusal = refusals.get("personal").expect("recorded");
    assert_eq!(refusal.status, 401);
    assert!(refusals.get("work").is_none());
}

/// Clearing an empty store asks nobody who is serving. It is called on every
/// turn that works, and resolving the serving account is a store read — which
/// on a borrowed Claude profile is a `security` spawn.
#[test]
fn clearing_nothing_does_not_ask_who_is_serving() {
    let asked = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&asked);
    let refusals = proxenos::auth::refusals::Refusals::default().serving(Arc::new(move || {
        *counter.lock().expect("not poisoned") += 1;
        Some("personal".to_owned())
    }));

    refusals.clear(None);
    assert_eq!(*asked.lock().expect("not poisoned"), 0);

    refusals.record(Some("personal"), 401, "invalid access token");
    refusals.clear(None);
    assert_eq!(*asked.lock().expect("not poisoned"), 1);
    assert!(refusals.get("personal").is_none());
}

/// Both surfaces say it, and `status` carries the backend's own sentence
/// because that is what the operator is about to search for.
#[test]
fn a_refused_credential_is_said_on_the_row_and_in_the_report() {
    let row = render::accounts(&listing(serde_json::json!({
        "name": "work",
        "kind": "grant",
        "provider": "codex",
        "plan": "team",
        "selected": true,
        "refused": { "status": 401, "detail": "invalid access token", "at": 1_800_000_000 },
    })));
    assert!(row.contains("refused this credential"), "{row}");
    assert!(row.contains("sign in"), "{row}");

    let report = render::status(&serde_json::json!({
        "auth": {
            "connected": true,
            "account": "work",
            "provider": "codex",
            "kind": "grant",
            "refused": { "status": 401, "detail": "invalid access token", "at": 1_800_000_000 },
        },
    }));
    assert!(report.contains("401"), "{report}");
    assert!(report.contains("invalid access token"), "{report}");
}

/// A payload with no refusal in it carries `"refused": null`, and null is not
/// a refusal. Seen on a live daemon: every healthy account reported the
/// backend as having turned its credential away, with no reason given —
/// because there was none to give.
#[test]
fn a_null_refusal_is_not_reported_as_one() {
    let rendered = render::status(&serde_json::json!({
        "auth": {
            "connected": true,
            "account": "work",
            "provider": "codex",
            "kind": "grant",
            "refused": serde_json::Value::Null,
        },
    }));

    assert!(!rendered.contains("refused"), "{rendered}");
}
