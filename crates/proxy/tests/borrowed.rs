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
        borrowed::Host::MacOs,
        HOME,
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
