//! `docs/proxy-behavior.md` §8 — credentials.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::auth::store::CredentialStore;
use proxenos::auth::store::Credentials;
use proxenos::auth::store::FileStore;
use proxenos::auth::store::Provider;
use proxenos::auth::store::WritePoint;
use serde_json::Value;

fn sample() -> Credentials {
    Credentials {
        access_token: "access-secret".to_owned(),
        refresh_token: "refresh-secret".to_owned(),
        id_token: Some("id-secret".to_owned()),
        account_id: Some("acct_123".to_owned()),
        expires_at: Some(1_800_000_000),
    }
}

#[test]
fn credentials_round_trip_through_the_file_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    assert!(store.load().unwrap().is_none(), "nothing stored yet");

    store.save(&sample()).unwrap();
    let loaded = store.load().unwrap().expect("credentials should load");

    assert_eq!(loaded.access_token, "access-secret");
    assert_eq!(loaded.account_id.as_deref(), Some("acct_123"));
}

/// Created `0600` from the outset. Writing first and tightening afterwards
/// leaves a window in which the file is world-readable, and that window is
/// enough.
#[cfg(unix)]
#[test]
fn the_credential_file_is_private_the_moment_it_exists() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("nested").join("credentials.json"));
    store.save(&sample()).unwrap();

    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "credentials must not be readable by others"
    );
}

#[test]
fn clearing_removes_the_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store.save(&sample()).unwrap();
    store.clear().unwrap();

    assert!(store.load().unwrap().is_none());
    // Clearing what is already gone is not an error: `accounts.remove` must be safe
    // to run twice.
    store.clear().unwrap();
}

/// Refresh begins ahead of expiry. A token that expires mid-request fails the
/// request, and the margin is what keeps that from being routine.
#[test]
fn refresh_is_due_before_the_token_actually_expires() {
    let credentials = Credentials {
        expires_at: Some(1_000),
        ..sample()
    };

    assert!(!credentials.needs_refresh(800, 60), "not due yet");
    assert!(credentials.needs_refresh(950, 60), "inside the margin");
    assert!(credentials.needs_refresh(1_200, 60), "already expired");
}

/// An unknown expiry counts as expired. Refreshing needlessly costs one
/// request; using a dead token fails the turn.
#[test]
fn an_unknown_expiry_is_treated_as_expired() {
    let credentials = Credentials {
        expires_at: None,
        ..sample()
    };

    assert!(credentials.needs_refresh(0, 60));
}

// ---------------------------------------------------------------------------
// §8 — more than one account in one store.
// ---------------------------------------------------------------------------

use proxenos::auth::store::AccountStore;

/// A grant belonging to somebody else, distinguishable from `sample()` in
/// every field that matters.
fn other() -> Credentials {
    Credentials {
        access_token: "other-access".to_owned(),
        refresh_token: "other-refresh".to_owned(),
        id_token: Some("other-id".to_owned()),
        account_id: Some("acct_456".to_owned()),
        expires_at: Some(1_900_000_000),
    }
}

/// A credential file written before this store held more than one account is
/// read as the one account it describes. Anything else costs a re-login for a
/// grant that is sitting right there and still valid.
#[test]
fn a_file_from_the_single_account_build_loads_as_one_selected_account() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&sample()).unwrap().as_bytes(),
    )
    .unwrap();

    let store = FileStore::new(&path);

    let loaded = store.load().unwrap().expect("the stored grant should load");
    assert_eq!(loaded.access_token, "access-secret");
    assert_eq!(loaded.refresh_token, "refresh-secret");

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "acct_123");
    assert_eq!(accounts[0].account_id.as_deref(), Some("acct_123"));
    assert!(accounts[0].selected, "the only account serves turns");
}

/// The migration survives the next write. A refresh saves the rotated grant,
/// and it must land in the account the old file described rather than beside
/// it.
#[test]
fn a_migrated_account_keeps_its_place_through_the_next_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::write(&path, serde_json::to_string(&sample()).unwrap().as_bytes()).unwrap();

    // What a refresh does: read the grant, then write it back rotated. A store
    // that dropped the old file on the way in would have nothing to read here.
    let store = FileStore::new(&path);
    let loaded = store
        .load()
        .unwrap()
        .expect("the migrated grant should load");
    store
        .save(&Credentials {
            access_token: "rotated".to_owned(),
            ..loaded
        })
        .unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(
        accounts.len(),
        1,
        "the save added an account instead of updating one"
    );
    assert_eq!(accounts[0].name, "acct_123");
    let stored = store.load().unwrap().unwrap();
    assert_eq!(stored.access_token, "rotated");
    assert_eq!(
        stored.refresh_token, "refresh-secret",
        "the rest of the migrated grant should survive the write"
    );
}

/// Logging in twice leaves two usable grants rather than one. The account
/// already serving turns keeps serving them, and the new one is there to
/// switch to.
#[test]
fn logging_in_twice_leaves_two_usable_grants() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    assert_eq!(store.add(&sample(), None).unwrap(), "acct_123");
    assert_eq!(store.add(&other(), None).unwrap(), "acct_456");

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(accounts[0].selected, "the first login still serves turns");
    assert!(
        !accounts[1].selected,
        "a login stores a credential; it does not choose what serves"
    );
    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");

    store.select("acct_456").unwrap();
    assert_eq!(
        store.load().unwrap().unwrap().access_token,
        "other-access",
        "the second account is usable once it is chosen"
    );
}

/// Authorizing the same account twice replaces its grant. Two entries for one
/// account would be two refresh-token families against one grant, which is the
/// arrangement §8 exists to prevent.
#[test]
fn authorizing_the_same_account_again_replaces_its_grant() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store.add(&sample(), None).unwrap();
    store
        .add(
            &Credentials {
                refresh_token: "re-authorized".to_owned(),
                ..sample()
            },
            None,
        )
        .unwrap();

    assert_eq!(store.accounts().unwrap().len(), 1);
    assert_eq!(
        store.load().unwrap().unwrap().refresh_token,
        "re-authorized"
    );
}

/// A label names the account. The id is what the backend calls it; a label is
/// what the operator calls it, and one of the two is memorable.
#[test]
fn a_label_names_the_account() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    assert_eq!(store.add(&sample(), Some("work")).unwrap(), "work");

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts[0].name, "work");
    // The id it belongs to is still reported: the label is a local name, not a
    // replacement for what the backend knows.
    assert_eq!(accounts[0].account_id.as_deref(), Some("acct_123"));
}

/// A grant whose id token carried no account id is still storable. The name is
/// assigned rather than invented from the grant: nothing in it is an account
/// id, and treating a token as one would be exactly the fabrication §8
/// forbids.
#[test]
fn an_account_with_no_id_is_named_rather_than_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    let name = store
        .add(
            &Credentials {
                account_id: None,
                ..sample()
            },
            None,
        )
        .unwrap();

    assert_eq!(name, "account-1");
    assert_eq!(store.accounts().unwrap()[0].account_id, None);
}

/// Selecting something that is not there says what is.
#[test]
fn selecting_an_unknown_account_names_the_known_ones() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();

    let error = store.select("nobody").unwrap_err().to_string();

    assert!(error.contains("nobody"), "{error}");
    assert!(error.contains("acct_123"), "{error}");
}

/// Clearing one account leaves the rest usable, and something still serves
/// turns afterwards.
#[test]
fn clearing_one_account_leaves_the_rest_usable() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();
    store.select("acct_456").unwrap();

    // `clear` is the selected account, which is the one the second login left
    // serving turns.
    store.clear().unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "acct_123");
    assert!(accounts[0].selected, "something must still serve turns");
    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");

    // And clearing the last one empties the store, as it always did.
    store.clear().unwrap();
    assert!(store.load().unwrap().is_none());
    assert!(store.accounts().unwrap().is_empty());
}

/// Removing an account that is not selected leaves the selection alone.
#[test]
fn removing_an_unselected_account_leaves_the_selection_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();

    store.remove("acct_123").unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "acct_456");
    assert_eq!(store.load().unwrap().unwrap().access_token, "other-access");
}

/// A selection naming an account that is not there falls back to the first
/// stored one. A file that names a missing account still holds usable grants,
/// and reporting "not authenticated" there would send an operator to re-login
/// for nothing.
#[test]
fn a_selection_naming_nothing_falls_back_to_the_first_account() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileStore::new(&path);
    store.add(&sample(), None).unwrap();

    let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file["selected"] = Value::from("departed");
    std::fs::write(&path, file.to_string()).unwrap();

    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");
    assert!(store.accounts().unwrap()[0].selected);
}

/// The account list is rendered to whoever asks `status`. Nothing in it may be
/// a token: this is the one shape in the credential module that is meant to
/// leave the process.
#[test]
fn the_account_list_carries_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();

    let accounts = store.accounts().unwrap();
    let rendered = format!(
        "{}{:?}",
        serde_json::to_string(&accounts).unwrap(),
        accounts
    );

    for secret in ["access-secret", "refresh-secret", "id-secret"] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert!(rendered.contains("acct_123"), "{rendered}");
}

/// An account is identified by its account id, not by the name it happens to
/// be stored under.
///
/// Authorizing an account already stored under a different name must replace
/// that account rather than add a second entry for it. Two entries for one
/// account are two holders of one refresh-token family, which is the
/// arrangement §8.1 exists to keep out of the store: the first rotation
/// retires the other entry's token, and the operator is left with an account
/// they can see and can never spend.
#[test]
fn re_authorizing_an_account_stored_under_another_name_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store.add(&sample(), Some("work")).unwrap();
    let name = store
        .add(
            &Credentials {
                refresh_token: "re-authorized".to_owned(),
                ..sample()
            },
            None,
        )
        .unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(
        accounts.len(),
        1,
        "one account, two entries sharing its refresh-token family: {accounts:?}"
    );
    assert_eq!(name, "work", "the name it is already stored under");
    assert_eq!(
        store.load().unwrap().unwrap().refresh_token,
        "re-authorized"
    );

    // And a new label renames the account rather than duplicating it.
    let name = store.add(&sample(), Some("day-job")).unwrap();
    assert_eq!(name, "day-job");
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "day-job");
}

/// A refresh writes the grant of the account it read, even if the selection
/// moved while the request was in flight.
///
/// `save` resolving the target by selection is a read-modify-write across a
/// network round trip: switch accounts in the middle and one account's rotated
/// grant lands in another's entry, destroying a refresh token that only a
/// re-login can replace and leaving that account authenticating as somebody
/// else.
#[test]
fn a_grant_is_saved_to_the_account_it_belongs_to_not_the_selected_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();

    // `acct_456` is selected; the grant being written belongs to `acct_123`.
    store
        .save(&Credentials {
            refresh_token: "rotated".to_owned(),
            ..sample()
        })
        .unwrap();

    store.select("acct_123").unwrap();
    assert_eq!(store.load().unwrap().unwrap().refresh_token, "rotated");

    store.select("acct_456").unwrap();
    let stored = store.load().unwrap().unwrap();
    assert_eq!(
        stored.refresh_token, "other-refresh",
        "another account's rotation landed in this one's entry"
    );
    assert_eq!(stored.access_token, "other-access");
}

/// The store is replaced, never truncated in place.
///
/// The file holds every account now. A write interrupted between truncation
/// and completion would leave the whole store unreadable — every account gone
/// for one account's rotated token — so the new content is written beside it
/// and moved over it.
#[cfg(unix)]
#[test]
fn a_write_leaves_no_window_where_the_store_is_half_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileStore::new(&path);
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();

    let before = std::fs::read_to_string(&path).unwrap();
    let inode = |path: &std::path::Path| -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).unwrap().ino()
    };
    let first = inode(&path);

    store
        .save(&Credentials {
            refresh_token: "rotated".to_owned(),
            ..other()
        })
        .unwrap();

    assert_ne!(
        inode(&path),
        first,
        "the file was written in place rather than replaced"
    );
    assert_ne!(std::fs::read_to_string(&path).unwrap(), before);
    // Nothing is left lying around beside it. The lock is the one permanent
    // neighbour: every writer takes it, so it is there before the first write
    // and stays after the last. A half-finished replacement is what this is
    // looking for.
    let strays: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .filter(|name| name != "credentials.json" && name != "credentials.json.lock")
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");

    // Still private, and still both accounts.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.accounts().unwrap().len(), 2);
}

/// A label that already names a different account is refused, not honoured.
///
/// `accounts add-key work` months after the first one, with the browser signed into
/// somebody else: the label resolves to an entry holding another account's
/// grant, and writing over it retires a working grant with nothing said —
/// exactly what the `add`/`save` split exists to prevent. Refusing costs the
/// authorization that was just spent, which one more login replaces; the other
/// way costs a grant that may not be replaceable at all.
#[test]
fn a_label_already_naming_another_account_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), Some("work")).unwrap();

    let error = store.add(&other(), Some("work")).unwrap_err().to_string();

    assert!(error.contains("work"), "{error}");
    assert!(
        error.contains("acct_123"),
        "the name's current owner: {error}"
    );

    // Nothing was disturbed.
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id.as_deref(), Some("acct_123"));
    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");

    // The same label for the account that already holds it still works: that
    // is a re-authorization, not a collision.
    store.add(&sample(), Some("work")).unwrap();
    assert_eq!(store.accounts().unwrap().len(), 1);
}

/// And a label that already names a *key* is refused too — the same refusal
/// `add_key` makes from the other side.
///
/// A key entry carries no account id, so the id comparison above cannot see
/// this collision at all. Without its own guard a login under that label
/// writes a grant over the key and the key is gone, while the reverse — a key
/// under a name holding a grant — has always been refused. §8.2: neither kind
/// is stored over the other.
#[test]
fn a_label_already_naming_a_key_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store
        .add_key("work", "key-secret-value", Provider::Codex)
        .unwrap();

    let error = store.add(&sample(), Some("work")).unwrap_err().to_string();
    assert!(error.contains("work"), "{error}");
    assert!(error.contains("key"), "{error}");

    // The key is untouched, and no second entry was appended.
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].kind, "key");
    assert_eq!(stored_key(&store, "work"), "key-secret-value");
}

/// Naming an account in an empty store says the store is empty.
///
/// The refusal lists what is stored, and with nothing stored that list is a
/// blank the reader has to interpret. What they need to be told is to log in.
#[test]
fn selecting_from_an_empty_store_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    let error = store.select("work").unwrap_err().to_string();

    assert!(error.contains("accounts add-key"), "{error}");
    assert!(
        !error.contains("stored: "),
        "an empty list is not an answer: {error}"
    );
}

/// Add an account by editing the file directly, taking no lock.
///
/// What an older binary or a hand edit does. Built by cloning an entry already
/// there, so the test states only what it means to change and cannot drift
/// from the stored shape.
fn add_account_behind_the_lock(path: &std::path::Path, name: &str, account_id: &str) {
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut entry = file["accounts"][0].clone();
    entry["name"] = name.into();
    entry["account_id"] = account_id.into();
    file["accounts"].as_array_mut().unwrap().push(entry);
    std::fs::write(path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
}

/// A write that finds the file changed since it read starts over.
///
/// Every write is a read, a change, and a replacement of the whole file, so two
/// writers overlapping used to mean one of them silently lost everything the
/// other had done — and with several accounts in one file, "everything" is an
/// account, not a stale token.
///
/// The writer simulated here takes no lock: it edits the file in place, which
/// is what an older binary or a hand edit does. The lock cannot cover those,
/// so the comparison still has to.
#[test]
fn a_write_that_lost_a_race_is_redone_rather_than_lost() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileStore::new(&path);
    store.add(&sample(), None).unwrap();

    // Another writer, landing between this one's read and its comparison.
    // Once, so the retry has something to converge on.
    let raced = std::sync::atomic::AtomicBool::new(false);
    let edited = path;
    store.on_write_for_test(move |point| {
        if point == WritePoint::BeforeComparison
            && !raced.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            add_account_behind_the_lock(&edited, "interloper", "acct_interloper");
        }
    });

    store.add(&other(), None).unwrap();

    let names: Vec<String> = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|account| account.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "acct_123".to_owned(),
            "interloper".to_owned(),
            "acct_456".to_owned()
        ],
        "the other writer's account was overwritten"
    );
}

/// A write that cannot take its lock says what to do about it.
///
/// The lock lives beside the credentials, so a directory that cannot hold one
/// stops every write. Locking is also not something every filesystem does — a
/// home on a network mount is the case that exists — and there the failure is
/// the filesystem's, not the operator's. Either way the answer is the same and
/// the message has to carry it, because "could not lock the credential file"
/// on its own reads as a bug in this program.
#[test]
fn a_write_that_cannot_lock_names_the_way_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    // Something already occupying the lock's name that is not a file.
    std::fs::create_dir(dir.path().join("credentials.json.lock")).unwrap();

    let error = FileStore::new(&path)
        .add(&sample(), None)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("PROXENOS_HOME"),
        "nothing to act on: {error}"
    );
}

/// A writer waits for the one already writing, rather than landing inside it.
///
/// The comparison covers the gap between a write's read and its check. It
/// cannot cover the gap between the check and the replacement: those are two
/// operations, and a writer that lands between them is copied over by a
/// replacement that already decided nothing had changed. Only a lock the
/// filesystem enforces closes that, which is why this drives the second writer
/// into exactly that gap.
///
/// Both writers are `FileStore`, so both take the lock — two open descriptions
/// in one process conflict the same way two processes do.
#[test]
fn a_writer_waits_rather_than_landing_inside_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let store = FileStore::new(&path);
    store.add(&sample(), None).unwrap();

    let (start, wait_to_start) = std::sync::mpsc::channel();
    let (finished, wait_for_finish) = std::sync::mpsc::channel();
    let second = {
        std::thread::spawn(move || {
            wait_to_start.recv().unwrap();
            FileStore::new(&path)
                .add(
                    &Credentials {
                        account_id: Some("acct_second".to_owned()),
                        ..other()
                    },
                    Some("second"),
                )
                .unwrap();
            let _ = finished.send(());
        })
    };

    // Inside the window the comparison cannot cover. The wait is bounded
    // because the passing case is the one that never finishes: a second writer
    // held off by the lock cannot report done until this write has released
    // it. Timing out here is the evidence, and without the lock the second
    // writer reports done in milliseconds and this one copies over it.
    let started = std::sync::atomic::AtomicBool::new(false);
    let wait_for_finish = std::sync::Mutex::new(wait_for_finish);
    store.on_write_for_test(move |point| {
        if point == WritePoint::AfterComparison
            && !started.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            start.send(()).unwrap();
            let _ = wait_for_finish
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2));
        }
    });

    store.add(&other(), None).unwrap();
    second.join().unwrap();

    let names: Vec<String> = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|account| account.name)
        .collect();
    assert!(
        names.contains(&"second".to_owned()),
        "the second writer landed inside the first one's replacement and was copied over: {names:?}"
    );
    assert!(
        names.contains(&"acct_456".to_owned()),
        "the first writer's own account is missing: {names:?}"
    );
}

/// Renaming moves the name and nothing else.
///
/// An account stored with no name of its own is named by the id the backend
/// knows it by, which is a UUID nobody wants to type at `accounts use`. Changing it should not cost
/// an authorization: the grant is fine, only what this store calls it is
/// wrong.
#[test]
fn renaming_moves_the_name_and_keeps_the_grant() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();
    store.select("acct_123").unwrap();

    store.rename("acct_123", "work").unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts[0].name, "work");
    assert_eq!(accounts[0].account_id.as_deref(), Some("acct_123"));
    assert!(
        accounts[0].selected,
        "the account serving turns must still be serving: {accounts:?}"
    );
    assert_eq!(
        store.load().unwrap().unwrap().access_token,
        "access-secret",
        "the grant should be untouched"
    );
    // The other account is where it was.
    assert_eq!(accounts[1].name, "acct_456");
    assert_eq!(accounts.len(), 2);

    // And the new name is what selects it from now on.
    store.select("work").unwrap();
    assert!(store.select("acct_123").is_err());
}

/// Renaming to a name another account holds is refused, for the same reason a
/// colliding label is: the store would otherwise have two accounts answering
/// to one name, and whichever `--use` found first would be the one that got
/// the turns.
#[test]
fn renaming_onto_another_account_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), Some("work")).unwrap();
    store.add(&other(), Some("spare")).unwrap();

    let error = store.rename("spare", "work").unwrap_err().to_string();
    assert!(error.contains("work"), "{error}");

    let names: Vec<String> = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|account| account.name)
        .collect();
    assert_eq!(names, vec!["work".to_owned(), "spare".to_owned()]);

    // Renaming an account to what it is already called is not a collision.
    store.rename("spare", "spare").unwrap();
    assert_eq!(store.accounts().unwrap()[1].name, "spare");

    // And a name nobody holds says so.
    let error = store.rename("ghost", "whatever").unwrap_err().to_string();
    assert!(error.contains("ghost"), "{error}");
}

// ---------------------------------------------------------------------------
// §8 — a credential that is not a subscription grant.
// ---------------------------------------------------------------------------

use proxenos::auth::store::Credential;

/// A key is an account like any other, of a different kind.
///
/// It has no refresh, no expiry and no account id, and nothing invents one for
/// it: a plausible expiry would drive a refresh that cannot happen, and a
/// plausible account id would be sent upstream as a header the key endpoint
/// never asked for.
#[test]
fn a_key_is_stored_as_an_account_of_its_own_kind() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store
        .add_key("billing", "key-secret-value", Provider::Codex)
        .unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "billing");
    assert_eq!(accounts[0].kind, "key");
    assert!(accounts[0].selected);
    assert_eq!(accounts[0].account_id, None);
    assert_eq!(accounts[0].expires_at, None);
    assert_eq!(accounts[0].email, None);

    // The listing is the shape that leaves the process.
    let rendered = format!(
        "{}{:?}",
        serde_json::to_string(&accounts).unwrap(),
        accounts
    );
    assert!(
        !rendered.contains("key-secret-value"),
        "leaked the key: {rendered}"
    );

    match store.credential().unwrap() {
        Some(Credential::Key(key)) => assert_eq!(key.value(), "key-secret-value"),
        other => panic!("expected a key, got {other:?}"),
    }
    // And `Debug` on the credential itself does not carry it either.
    assert!(
        !format!("{:?}", store.credential().unwrap()).contains("key-secret-value"),
        "Debug leaked the key"
    );
}

/// A grant and a key coexist, and switching between them is switching
/// accounts. Nothing about the second kind disturbs the first.
#[test]
fn a_key_and_a_grant_are_two_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store
        .add_key("billing", "key-secret-value", Provider::Codex)
        .unwrap();
    store.select("billing").unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0].kind, "grant");
    assert_eq!(accounts[1].kind, "key");
    assert!(accounts[1].selected, "the newest is the one serving turns");

    store.select("acct_123").unwrap();
    assert!(matches!(
        store.credential().unwrap(),
        Some(Credential::Grant(_))
    ));
    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");

    store.select("billing").unwrap();
    assert!(matches!(
        store.credential().unwrap(),
        Some(Credential::Key(_))
    ));
}

/// A credential file written before keys existed is every bit a file of
/// grants. The kind is absent there, and absent means grant.
#[test]
fn a_file_from_before_keys_reads_as_grants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "selected": "work",
            "accounts": [{
                "name": "work",
                "access_token": "access-secret",
                "refresh_token": "refresh-secret",
                "account_id": "acct_123",
                "expires_at": 1_800_000_000_u64,
            }],
        })
        .to_string(),
    )
    .unwrap();

    let store = FileStore::new(&path);

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].kind, "grant");
    assert_eq!(
        store.load().unwrap().unwrap().refresh_token,
        "refresh-secret"
    );

    // And the single-grant shape from before accounts existed, too.
    let older = dir.path().join("older.json");
    std::fs::write(&older, serde_json::to_string(&sample()).unwrap()).unwrap();
    let store = FileStore::new(&older);
    assert_eq!(store.accounts().unwrap()[0].kind, "grant");
}

/// A key cannot be renamed onto a grant's name, forgotten differently, or
/// otherwise treated as a second class of thing: every account verb works on
/// it.
#[test]
fn the_account_verbs_work_on_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store
        .add_key("billing", "key-secret-value", Provider::Codex)
        .unwrap();
    store.select("billing").unwrap();

    store.rename("billing", "spend").unwrap();
    assert_eq!(store.accounts().unwrap()[1].name, "spend");
    assert!(store.accounts().unwrap()[1].selected);

    store.remove("spend").unwrap();
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "acct_123");
    assert!(accounts[0].selected);
}

/// Storing a key under a name a grant already holds is refused.
///
/// `add` refuses the same collision, and for the same reason: the grant would
/// be gone with nothing said, and only a re-login brings it back. A key is
/// handed over rather than granted, which makes it easier to type by accident,
/// not safer.
#[test]
fn a_key_cannot_be_stored_over_a_grant() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), Some("work")).unwrap();

    let error = store
        .add_key("work", "key-secret-value", Provider::Codex)
        .unwrap_err()
        .to_string();
    assert!(error.contains("work"), "{error}");

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].kind, "grant");
    assert_eq!(store.load().unwrap().unwrap().access_token, "access-secret");

    // Replacing a key with another key is not a collision: that is how a
    // rotated secret is stored.
    store.add_key("billing", "first", Provider::Codex).unwrap();
    store.add_key("billing", "second", Provider::Codex).unwrap();
    assert_eq!(store.accounts().unwrap().len(), 2);
}

/// A rotation with nowhere to go is refused rather than turned into a new
/// account.
///
/// A grant carrying no account id is matched by selection alone, and the
/// selection can move to a key while a refresh is in flight. Appending the
/// rotated grant there would silently move the operator off the account they
/// had just selected.
#[test]
fn a_rotation_that_belongs_to_no_stored_account_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store
        .add_key("billing", "key-secret-value", Provider::Codex)
        .unwrap();

    let error = store
        .save(&Credentials {
            account_id: None,
            ..sample()
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("account"), "{error}");

    let accounts = store.accounts().unwrap();
    assert_eq!(
        accounts.len(),
        1,
        "a rotation created an account: {accounts:?}"
    );
    assert_eq!(accounts[0].name, "billing");
    assert!(accounts[0].selected, "the selection moved");

    // An empty store still takes one: a caller holding nothing but
    // `CredentialStore` has to be able to store what it just obtained.
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.save(&sample()).unwrap();
    assert_eq!(store.accounts().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// §8.1 — the store answers for an account by name.
// ---------------------------------------------------------------------------

/// A pinned tier names an account, and the store has to answer for that one
/// rather than for the selection.
///
/// `credential()` answers for whichever account is serving turns, which is the
/// wrong question here: a pinned tier says which account its turns belong to,
/// and reading the selection would serve them as somebody else.
#[test]
fn the_store_answers_for_an_account_other_than_the_selected_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();
    store.select("acct_123").unwrap();

    let pinned = store.credential_for("acct_456").unwrap();
    assert_eq!(
        pinned.grant().map(|grant| grant.refresh_token.as_str()),
        Some("other-refresh")
    );

    // And the selection is still what `credential()` answers for.
    assert_eq!(
        store
            .credential()
            .unwrap()
            .and_then(|held| held.grant().map(|grant| grant.refresh_token.clone())),
        Some("refresh-secret".to_owned())
    );
}

/// A pin naming an account that is not stored refuses, and the refusal names
/// it.
///
/// Never a fallback to the serving account: that spends the wrong
/// subscription's quota invisibly, which is the failure the consent gate
/// exists to prevent (`roadmap.md` v0.6.0). The name is in the message because
/// a mapping and a store are edited separately and either one could be the
/// half that is wrong.
#[test]
fn a_pin_naming_an_unstored_account_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add(&sample(), None).unwrap();

    let error = store.credential_for("spare").unwrap_err();
    let rendered = format!("{error}");
    assert!(rendered.contains("spare"), "{rendered}");
    assert!(rendered.contains("acct_123"), "{rendered}");
}

/// A key states which provider it is spent against, and the store keeps it.
///
/// `roadmap.md` v0.6.0 — routing reads the provider off the account, so a key
/// stored without one is a key that can only ever reach the first provider's
/// endpoint. The provider is a parameter rather than a default because the two
/// endpoints refuse each other's credentials, and a key that silently claimed
/// the wrong one would surface as an authentication failure naming the
/// credential rather than the destination.
#[test]
fn a_key_states_the_provider_it_is_spent_against() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store
        .add_key("work", "key-secret-value", Provider::Codex)
        .unwrap();
    store
        .add_key("relay", "key-secret-value", Provider::Anthropic)
        .unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts[0].provider, "codex");
    assert_eq!(accounts[1].provider, "anthropic");

    // And it survives a reload: the field is written, not held in memory.
    let reopened = FileStore::new(dir.path().join("credentials.json"));
    let accounts = reopened.accounts().unwrap();
    assert_eq!(accounts[1].provider, "anthropic");
}

/// A login while another account is already serving stores the credential and
/// leaves the selection where it was.
///
/// Storing a credential and choosing what serves turns are two decisions, and
/// a login is only the first. Making it both means an operator who adds a
/// second account has silently moved every turn onto it.
#[test]
fn a_login_while_another_account_serves_leaves_the_selection_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store.add(&sample(), None).unwrap();
    store.add(&other(), None).unwrap();
    store
        .add_key("keyed", "sk-ant-oat01-value", Provider::Anthropic)
        .unwrap();

    let accounts = store.accounts().unwrap();
    let serving: Vec<&str> = accounts
        .iter()
        .filter(|account| account.selected)
        .map(|account| account.name.as_str())
        .collect();
    assert_eq!(serving, vec!["acct_123"], "{accounts:?}");
    assert_eq!(accounts.len(), 3, "every login still stored: {accounts:?}");
}

/// The first login has nothing to displace, so it selects.
#[test]
fn a_first_login_selects_the_new_account() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store.add(&sample(), None).unwrap();

    let accounts = store.accounts().unwrap();
    assert!(accounts[0].selected, "{accounts:?}");
}

/// And a first login by key selects too — one rule, not a per-flag one.
#[test]
fn a_first_login_by_key_selects_the_new_account() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));

    store
        .add_key("keyed", "sk-ant-oat01-value", Provider::Anthropic)
        .unwrap();

    let accounts = store.accounts().unwrap();
    assert!(accounts[0].selected, "{accounts:?}");
}

/// A key re-store that would change the entry's provider is refused.
///
/// A key over a key of the same provider is a rotation. A key over a key of a
/// *different* provider silently discards a working credential and re-points
/// the account at another backend, which is the same loss `add_key` already
/// refuses for a grant. The refusal has to name the account, the provider it
/// currently holds, and the way through, because neither is recoverable from
/// what the operator typed.
#[test]
fn a_key_re_store_that_changes_provider_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store
        .add_key("api", "key-secret-value", Provider::Codex)
        .unwrap();

    let error = store
        .add_key("api", "sk-ant-oat01-value", Provider::Anthropic)
        .unwrap_err()
        .to_string();
    assert!(error.contains("api"), "{error}");
    assert!(error.contains(Provider::Codex.as_str()), "{error}");
    assert!(error.contains("accounts remove"), "{error}");

    // The stored key is untouched, and so is its provider.
    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].provider, Provider::Codex.as_str());
    assert_eq!(stored_key(&store, "api"), "key-secret-value");
}

/// Same-provider rotation stays what it was: silent, in place.
#[test]
fn a_same_provider_key_re_store_still_rotates() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new(dir.path().join("credentials.json"));
    store.add_key("api", "first", Provider::Codex).unwrap();
    store.add_key("api", "second", Provider::Codex).unwrap();

    let accounts = store.accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].provider, Provider::Codex.as_str());
    assert_eq!(stored_key(&store, "api"), "second");
}

/// The stored secret for a key account, for the two tests that assert a
/// re-store did or did not replace one.
fn stored_key(store: &FileStore, name: &str) -> String {
    let credential = store.credential_for(name).unwrap();
    let Credential::Key(key) = credential else {
        unreachable!("`{name}` is not a key: {credential:?}")
    };
    key.value().to_owned()
}
