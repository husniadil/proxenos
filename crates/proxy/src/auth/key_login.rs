//! Storing a key read from stdin.
//!
//! Behind a trait rather than reading stdin directly, so the terminal half and
//! the pipe half are testable without a terminal. `--key` takes any secret and
//! validates nothing about its shape.
//!
//! Silence while reading a tty reads as a hang. The prompt is written to
//! **stderr** and only where a person is typing, so a piped key stays byte for
//! byte what it was: `printf '%s' "$KEY" | proxenos accounts add-key NAME
//! --provider P` prints
//! nothing that a reader of its stdout did not already see.

use std::io::{self, IsTerminal, Write};

use crate::auth::store::SETUP_TOKEN_PREFIX;
use crate::auth::store::{AccountStore, Provider};
/// The I/O half: everything the flow says and everything it reads.
///
/// It outlived the guided setup-token flow it was written for. That flow is
/// gone — a subscription is borrowed from the profile that holds it now (§8.4)
/// — and this is what remains: one seam, so the tty and pipe halves are
/// testable without a terminal.
pub trait Guide {
    /// Where the key comes from, said before it is asked for.
    fn explain(&mut self) -> std::io::Result<()>;
    /// Read the key. A terminal implementation must not echo it.
    fn token(&mut self) -> std::io::Result<String>;
    /// What to call the account, or `None` when there is nobody to ask.
    fn name(&mut self) -> std::io::Result<Option<String>>;
    /// Confirm what was stored. Never the key.
    fn stored(&mut self, name: &str) -> std::io::Result<()>;
    /// Which provider this guide files under, where it files under one.
    fn provider(&self) -> Option<Provider> {
        None
    }

    /// Said after an anthropic key whose stem two credentials share was
    /// stored. Silence by default: a guide with no person behind it has
    /// nobody to tell.
    fn shared_stem_caution(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Store a key under `name`, reading it through `guide`.
///
/// Returns the name it was filed under, which is the string `accounts use`
/// takes.
///
/// **Never from an argument.** A command line is visible to every process on
/// the machine and lands in the shell's history file; §8 keeps credentials out
/// of argv, and this is the path that would break that rule if it took one.
pub fn run(store: &dyn AccountStore, guide: &mut dyn Guide, name: &str) -> anyhow::Result<String> {
    // The name is a positional on `accounts add-key`, so it cannot be missing
    // by the time this is called: a key carries no account id to be named by,
    // and the name is what selects it afterwards.
    guide.explain()?;
    let key = guide.token()?;
    let key = key.trim();
    if key.is_empty() {
        anyhow::bail!("no key on stdin; pipe it in, or paste it at the prompt");
    }

    let provider = guide_provider(guide);
    store.add_key(name, key, provider)?;
    guide.stored(name)?;
    // One stem, two credentials. `classify` files anything beginning
    // `sk-ant-oat` as a subscription token, and a Claude Code OAuth access
    // token carries that exact stem while lasting hours rather than a year.
    // Nothing downstream can tell them apart, so the one moment a person is
    // present is the moment to say so.
    if provider == Provider::Anthropic && key.starts_with(SETUP_TOKEN_PREFIX) {
        guide.shared_stem_caution()?;
    }
    Ok(name.to_owned())
}

/// The provider a guide files under. Carried on the guide rather than passed
/// alongside it, so the text it prints and the store it writes cannot name two
/// different providers.
fn guide_provider(guide: &dyn Guide) -> Provider {
    guide.provider().unwrap_or(Provider::Codex)
}

/// The terminal half: a prompt on stderr and a hidden read where stdin is a
/// tty, a plain read from the pipe where it is not.
pub struct Terminal {
    provider: Provider,
    interactive: bool,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    read: Box<dyn FnMut() -> io::Result<String>>,
}

impl Terminal {
    /// The real one: stdin, stdout, stderr, and whatever stdin actually is.
    pub fn stdio(provider: Provider) -> Self {
        let interactive = io::stdin().is_terminal();
        Self {
            provider,
            interactive,
            out: Box::new(io::stdout()),
            err: Box::new(io::stderr()),
            read: Box::new(move || {
                if interactive {
                    // Hidden: a key echoed while it is typed is on the
                    // screen and in the scrollback of whoever walks past.
                    rpassword::read_password()
                } else {
                    let mut key = String::new();
                    io::Read::read_to_string(&mut io::stdin(), &mut key)?;
                    Ok(key)
                }
            }),
        }
    }
}

impl Guide for Terminal {
    fn explain(&mut self) -> io::Result<()> {
        if !self.interactive {
            return Ok(());
        }
        writeln!(
            self.err,
            "Paste the {} key, then press enter. Nothing is echoed, and it is never\n\
             written anywhere but the credential file.",
            self.provider.as_str()
        )?;
        self.err.flush()
    }

    fn token(&mut self) -> io::Result<String> {
        (self.read)()
    }

    fn name(&mut self) -> io::Result<Option<String>> {
        // `add-key` never asks: NAME is a positional, and the verb refuses
        // before anything is read without one.
        Ok(None)
    }

    fn stored(&mut self, name: &str) -> io::Result<()> {
        writeln!(
            self.out,
            "Stored a {} key as {name}.",
            self.provider.as_str()
        )?;
        self.out.flush()
    }

    fn provider(&self) -> Option<Provider> {
        Some(self.provider)
    }

    fn shared_stem_caution(&mut self) -> io::Result<()> {
        if !self.interactive {
            return Ok(());
        }
        writeln!(
            self.err,
            "Note: `{SETUP_TOKEN_PREFIX}` is the stem of two different credentials, and\n\
             nothing stored here can tell them apart. `claude setup-token` mints one\n\
             that lasts about a year; the access token in Claude Code's own keychain\n\
             entry carries the same stem and expires in hours. This account is filed\n\
             as a subscription token either way, and if it was the second one it will\n\
             simply stop authenticating with nothing here able to say why."
        )?;
        self.err.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::FileStore;
    use std::sync::{Arc, Mutex};

    /// A sink that can be read back after the guide has written to it.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("the sink").clone()).expect("utf-8")
        }
    }

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("the sink").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The real `Terminal`, with the tty answer and the key supplied rather
    /// than taken from the process. Everything it decides is decided here.
    fn terminal(
        provider: Provider,
        interactive: bool,
        key: &str,
    ) -> (Terminal, Captured, Captured) {
        let (out, err) = (Captured::default(), Captured::default());
        let key = key.to_owned();
        let terminal = Terminal {
            provider,
            interactive,
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            read: Box::new(move || Ok(key.clone())),
        };
        (terminal, out, err)
    }

    fn temp_store() -> (tempfile::TempDir, Arc<FileStore>) {
        let home = tempfile::tempdir().expect("a temp home");
        let store = Arc::new(FileStore::new(home.path().join("credentials.json")));
        (home, store)
    }

    /// The machine path. A prompt on stdout would corrupt whatever reads it,
    /// so a piped run prints the one line it has always printed and nothing
    /// else — on either stream.
    #[test]
    fn a_piped_key_prints_only_what_it_stored() {
        let (_home, store) = temp_store();
        let (mut guide, out, err) = terminal(Provider::Codex, false, "sk-test-piped\n");

        let name = run(store.as_ref(), &mut guide, "robot").expect("a stored key");

        assert_eq!(name, "robot");
        assert_eq!(out.text(), "Stored a codex key as robot.\n");
        assert_eq!(err.text(), "");
    }

    /// The bug this path had: nothing said while it waited, so a terminal
    /// operator saw a hang and had to guess the gesture that ends it.
    #[test]
    fn a_terminal_key_says_what_it_is_waiting_for() {
        let (_home, store) = temp_store();
        let (mut guide, out, err) = terminal(Provider::Anthropic, true, "sk-test-typed");

        run(store.as_ref(), &mut guide, "personal-claude").expect("a stored key");

        let said = err.text();
        assert!(said.contains("Paste the anthropic key"), "{said}");
        assert!(said.contains("press enter"), "{said}");
        assert!(said.contains("Nothing is echoed"), "{said}");
        // Whatever it says, it says on stderr: stdout is the machine's.
        assert_eq!(out.text(), "Stored a anthropic key as personal-claude.\n");
    }

    /// The secret is what this command exists to move. It reaches the store
    /// and nothing else.
    #[test]
    fn the_key_never_reaches_either_stream() {
        let (_home, store) = temp_store();
        let (mut guide, out, err) = terminal(Provider::Codex, true, "sk-unguessable-4d1f7c");

        run(store.as_ref(), &mut guide, "robot").expect("a stored key");

        assert!(!out.text().contains("4d1f7c"), "{}", out.text());
        assert!(!err.text().contains("4d1f7c"), "{}", err.text());
    }

    /// The stem `sk-ant-oat` belongs to two credentials with lifetimes three
    /// orders of magnitude apart, and the store files both as one. A terminal
    /// is the only place a person is present to be told.
    #[test]
    fn an_anthropic_oat_key_names_the_two_credentials_sharing_its_stem() {
        let (_home, store) = temp_store();
        let (mut guide, out, err) = terminal(Provider::Anthropic, true, "sk-ant-oat01-typed");

        run(store.as_ref(), &mut guide, "personal-claude").expect("a stored key");

        let said = err.text();
        assert!(said.contains("two different credentials"), "{said}");
        assert!(said.contains("claude setup-token"), "{said}");
        assert!(said.contains("expires in hours"), "{said}");
        // Said, never quoted: the caution names the stem and nothing after it.
        assert!(!said.contains("01-typed"), "{said}");
        assert_eq!(out.text(), "Stored a anthropic key as personal-claude.\n");
    }

    /// The machine path stays byte for byte what it was, stem or no stem.
    #[test]
    fn a_piped_oat_key_says_nothing_extra_on_either_stream() {
        let (_home, store) = temp_store();
        let (mut guide, out, err) = terminal(Provider::Anthropic, false, "sk-ant-oat01-piped\n");

        run(store.as_ref(), &mut guide, "personal-claude").expect("a stored key");

        assert_eq!(out.text(), "Stored a anthropic key as personal-claude.\n");
        assert_eq!(err.text(), "");
    }

    /// The distinction is anthropic's. The other provider issues one kind of
    /// key, and a string that happens to start the same way says nothing.
    #[test]
    fn a_codex_key_is_never_cautioned_about_an_anthropic_stem() {
        let (_home, store) = temp_store();
        let (mut guide, _out, err) = terminal(Provider::Codex, true, "sk-ant-oat01-typed");

        run(store.as_ref(), &mut guide, "robot").expect("a stored key");

        assert!(
            !err.text().contains("two different credentials"),
            "{}",
            err.text()
        );
    }

    /// An empty read is the state the old error described after the fact.
    #[test]
    fn an_empty_key_is_refused() {
        let (_home, store) = temp_store();
        let (mut guide, _out, _err) = terminal(Provider::Codex, false, "   \n");

        let error = run(store.as_ref(), &mut guide, "robot").expect_err("no key");

        assert!(error.to_string().contains("no key on stdin"), "{error}");
    }
}
