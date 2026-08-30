# Changelog

All notable changes to this project are recorded here. This project follows
[semantic versioning](https://semver.org). The semver-bound surfaces are listed
in [`docs/api.md`](docs/api.md) §6.

## [Unreleased]

- A Claude profile on macOS is read from the keychain item **and then** from
  `.credentials.json` beside it. A daemon started as a system-domain
  LaunchDaemon for a headless account cannot reach a login keychain: with no
  security session `security` reports the item as absent, and with
  `SessionCreate` the keychain is locked instead and the read fails outright —
  and unlocking it wants the account password at every boot. Both answers now
  fall through to the file the client writes where there is no keychain, which
  is the same file Linux has always read. The keychain's failure is not
  swallowed: where the file answered it is logged at `debug`, and where neither
  place held a grant the refusal carries it, on top of naming both places and
  the remedy — a keychain this process cannot reach and a profile nobody signed
  into want different answers. The source a listing shows names both places.

## [0.18.0]

- A tier mapped to a model the account's catalog no longer carries no longer
  stops the daemon. One retired model — `fable = "gpt-5.6-sol"` against an
  account that had lost it — refused the whole start, taking down every tier
  that resolved and every process depending on them. The stated model is still
  the operator's decision and is still never overruled: the tier keeps the id,
  is marked `missing` with the reason, and only turns asking for *that* tier are
  refused, in the sentence the start used to refuse with — naming the tier, the
  model, and what the catalog does have. `status` marks the row and `models`
  names it beneath the list; the startup log carries the same sentence once at
  WARN, so it is heard before a turn fails; `doctor --live` reports it above the
  matrix. Where every tier is marked the daemon still starts and says so —
  `proxenos reload` is how the mapping is fixed, and a process that exited could
  not be reloaded. `reload` re-derives the marks, so fixing config.toml and
  reloading recovers. A `tiers.set` or an `accounts use` is still refused
  outright rather than marked: those are typed a moment earlier, nothing that
  was serving stops serving, and the refusal is the feedback.
- A *defaulted* model the catalog lacks is now overruled on `reload` as well as
  at startup. A default is this proxy's guess about an account it has not seen,
  and the start replaced it while a reload of the same file refused it.

## [0.17.0]

- `accounts login --relogin` signs a declared profile back in. A grant whose
  access token has expired still reads as a grant, so the two paths the verb
  had both did the wrong thing with a lapsed subscription: without the flag the
  name was refused as already declared in `[profiles]`, and any way past that
  refusal led to the profile being adopted and no client being run. `--relogin`
  takes the declaration as the whole answer — the name must be declared, the
  provider must be the one it is declared as, and the directory is the one it
  names, so `--path` is refused and a declaration naming no path is signed in
  as the stock profile, with no variable set. The client runs whatever the
  profile currently reads as, and nothing is written afterwards.

## [0.16.1]

- The two decisions `exec --account` left behind follow the account the session
  is served as. A plain `--model` id was upgraded to its `[1m]` variant from
  the *selection's* menu, so a session pinned onto an Anthropic account while a
  Codex account was selected found no variant and ran on the standard window —
  the `models` method now takes the same `{"account": name}` the environment
  does, and the launch asks it for the account it names. And a turn tagged
  `proxenos-account:<name>` was translated on the selection's tier mapping,
  asking that account's own credential for a model stated for somebody else;
  the mapping is now resolved for the tagged account, so
  `[accounts.<name>.tiers]` applies to a tagged turn exactly as it would if
  that account were selected. A tag naming the account that is selected keeps
  the mapping in force, and a relayed turn is still the bytes the client sent.

## [0.16.0]

- `exec --account` configures the session for the account it names. The flag
  already decided who serves every turn, but the environment was still rendered
  for whichever account was selected — so a session tagged onto a subscription
  of the other provider was handed the shared table's tier ids and sent them,
  and the backend refused `gpt-5.6-luna` as an unrecognized model with an
  explicit `--model` the only way past it. The tier mapping, the window
  variables, and the client policy now resolve for the named account, including
  its own `[accounts.<name>.tiers]` section, and the mapping the launch carries
  is printed beside the account it is served as. The `env` method takes the same
  `{"account": name}` and refuses a name the store does not hold.

## [0.15.1]

- An incomplete turn reports upstream's own token usage. `response.incomplete`
  carries the same usage block `response.completed` does, and it was being
  discarded — so a turn the backend cut short for `max_tokens` closed on the
  proxy's own input estimate, which is the one figure a completed turn is never
  allowed to report. It now reads `/response/usage` the same way, and the
  non-streaming fold, which takes its body from the same frames, reports it too.

- `accounts login --provider codex` honours `codex_program`. The path was read
  for the Anthropic client alone and the Codex arm ran the bare name, so a
  daemon started by launchd — which resolves almost nothing from `PATH` — could
  poke the configured Codex client for a refresh and still fail to find one to
  log in with. Each provider's login now runs the program its own key names.

- A login carrying a label that already names a stored key is refused rather
  than written over it. `add_key` has always refused the reverse, but the
  duplicate-label guard compared account ids, which a key entry does not have —
  so the grant landed on top of the key and the key was gone with nothing said.
  The refusal is now symmetric.

## [0.15.0]

- `accounts login --device-auth` prints a URL and a code where the machine has
  a terminal and no browser. A container, or a session over ssh, gives
  `codex login` somewhere to run and nothing to open, and the login it starts
  there hangs with nothing said. The flag goes onto the command that is run,
  onto the line that is printed where there is no terminal to answer the
  prompts, and onto the printed line that declares the profile afterwards — a
  re-run that dropped it would hang the same way. It is a flag of `codex login`
  alone, so `--provider anthropic` refuses it rather than handing
  `claude auth login` an argument it would reject as a misspelling.

- The herdr plugin's popup acts on a piped key without waiting out the redraw
  interval. Its non-tty read slept the whole interval when a `q` arrived
  without a trailing newline, so input that had already been delivered took
  thirty seconds to act on. A partial line is handed over as soon as it is
  read; a true EOF still sleeps, which is what keeps a closed stdin a redraw
  loop rather than a spin.

## [0.14.0]

- `usage` says when a borrowed Anthropic subscription is no longer active. The
  profile endpoint already asked for the plan also states
  `subscription_status`, and a subscription the provider has stopped calling
  active keeps reporting quota that looks untouched while every turn is
  refused — so the account's row now reads `subscription canceled`, in the
  provider's own word rather than a vocabulary invented here, and the same
  word travels under `subscription_status` in `usage --json`. An active
  subscription is silence. The hourly profile cache keeps the status beside
  the plan, so a cached answer states as much as a fresh one. The herdr
  plugin's popup shows the same row, under the account header that already
  names the plan.

- `usage` reports the extra-usage credit a borrowed Anthropic grant states.
  Once an account's plan windows are full, further turns come out of a money
  balance no percentage in the table describes — measured live, 6% of a
  five-hour window beside a credit at 98% — and it now renders as its own row,
  `credit: $205.75 / $210.52 · 98% (critical)`, and travels under `credit` in
  `usage --json` and the herdr popup. Read from the provider's `spend` block
  only where it says the facility is enabled, with the provider's own
  percentage rather than a ratio of the amounts, and dropped entirely where an
  amount, its exponent, or the two currencies cannot be read: a renamed field
  read as zero spent is headroom that is not there. The legacy `extra_usage`
  duplicate of the same figure in float cents is deliberately not parsed.

- The herdr plugin's popup reads keys raw where the terminal allows it, so
  `q` — and now `Esc` — closes it without Enter, and `r` refreshes on the
  keypress. A terminal stty cannot reshape keeps the old line-buffered reads.
- A herdr plugin, under `herdr-plugin/`: quota bars for every claude pane in
  the sidebar — routed panes show the serving account, direct panes the
  keychain anthropic account, model-scoped windows included — and the full
  per-account table in a popup on `prefix+u`. Install is `herdr plugin link`
  plus `sh install.sh`; see its README.
- `exec --account <name>` serves one session as the named account without
  moving the selection — the per-session switch beside `accounts use`'s
  standing one. The flag is consumed by `exec` and never forwarded; the name
  travels as the otherwise-ignored `ANTHROPIC_AUTH_TOKEN` value, outranks a
  tier's pinned account on both paths, and an unknown name is refused at
  launch and again at the turn, each time naming it.
- A borrowed Anthropic grant's plan carries its multiplier: `usage --refresh`
  also asks the provider's profile endpoint, so a max account renders
  `max 20x` rather than `max`. Asked at most hourly; a declined answer leaves
  the plan as the grant last stated it rather than inventing one.
- `usage` keeps a model-scoped limit as its own labeled window. The quota
  endpoint of a borrowed Anthropic grant states per-model figures beside the
  account's — measured live, one model sat at 16% against a weekly window at
  49% — and they were previously dropped. Each now appears labeled with the
  model's display name, carrying its own percentage, severity, and reset, and
  no duration.

## [0.13.0]

This proxy no longer holds a subscription of its own. A grant is read from the
profile of the program that already owns it, so the account paying for a turn is
a directory you signed into somewhere else, and switching accounts is choosing
which directory to read.

**Upgrading needs one edit.** Declare the profiles you want under `[profiles]`
in `config.toml`; an entry with no `path` is that program's stock profile, which
is enough where you hold one account per tool:

```toml
[profiles.codex]
provider = "codex"

[profiles.claude]
provider = "anthropic"
```

Grants already in `credentials.json` are **not** read any more. They are not
migrated and not offered as accounts, and `accounts` names them under the
listing so a credential that stopped counting is not one that silently
vanished. Keys in that file keep working exactly as they did.

### Removed

- **The authorization flow, and everything built on it.** No PKCE exchange, no
  callback port, no `login` for a subscription, no `login`/`login.cancel` over
  the control socket, and no `--setup-token`. `codex login` and the ChatGPT app
  already perform that flow, and what they write is what this reads. About
  1,500 lines went with it, including the single-flight refresher and the
  dead-token bookkeeping that existed only to manage a family this daemon no
  longer holds. Storing a key survives as `accounts add-key`, because a key
  belongs to nobody and has to be kept somewhere.

### Added

- **`proxenos reload`, and `config.reload` behind it: an edit to `config.toml`
  reaching a daemon that is already serving.** `[profiles]`, the tier mapping
  and the effort ceiling are re-read and applied — the mapping through the same
  validated path a switch takes, so a file that would refuse a switch refuses a
  reload too. Nothing is fetched. What it cannot move is named every time
  rather than left to be discovered from an edit that did nothing:
  `instructions`, `client`, `transport`, `upstream` and `port` are read once
  and still need a restart. A file that no longer parses is refused with the
  parse error and the daemon keeps what it was running on. `accounts login` and
  `accounts remove` call it themselves after writing, so a declared profile no
  longer waits for the next start — the line telling you to stop the daemon and
  let it come back is gone with the reason for it.

- **`[profiles]`, which says where another program keeps a grant.** Paths only:
  no credential enters the configuration file and none is read out of it. An
  entry without a `path` is the stock profile, and that is a *different* profile
  from one naming the stock directory — on macOS the client picks its keychain
  item by whether `CLAUDE_CONFIG_DIR` was set at all, not by what it was set to.
  A relative path or a leading `~` is refused rather than resolved, since a
  daemon's working directory is not the operator's and the spelling is part of
  the identity. `docs/proxy-behavior.md` §8.4.

- **A quota figure for a borrowed grant on the second provider.** Its endpoint
  answers a grant and refuses the subscription token that used to stand in for
  one, so quota there had been readable only from turn headers. `usage
  --refresh` now asks it, under the owning client's user-agent, and reads its
  own body shape.

- **One refresh, asked for and waited on.** Where a borrowed Claude grant has
  lapsed, `usage --refresh` runs the client once under a per-profile lock,
  waits for it to exit, and reads the profile again — the figure the caller
  wants is the one after the refresh. Two cases refuse instead of running
  anything: the other provider, whose grant refreshes only on a real turn that
  spends quota and rotates the token, and a profile whose refresh token has
  lapsed too, where a failed refresh blanks what is left of the stored grant.

- **`claude_program`, for the client this daemon runs on its own behalf.**
  Unset, the bare name `claude` is resolved through the daemon's `PATH` — which
  is the shell's only when the daemon was started from one. A daemon started by
  launchd inherits almost none, so write the path out there or the refresh above
  refuses with `could not run \`claude\``. The same key settles the version the
  second provider's quota request is made as.

- **`accounts login`, which signs in to a second profile without you having to
  know where a profile lives.** It runs the owning program's own login —
  `claude auth login` or `codex login` — against a directory (yours with
  `--path`, else one under this daemon's own directory), reads the profile
  afterwards, and declares it in `[profiles]` only if it holds a grant. Nothing
  here sees a token. A directory already signed in is adopted rather than signed
  in again, and a run with no terminal prints the command with its environment
  variable attached instead of hanging on prompts nobody can answer.

- **A first run with nothing to configure.** `[profiles]` empty now means the
  stock profile of each program — what `claude` and `codex` themselves use with
  no variable set — and whichever is signed in becomes an account. One signed-in
  client is one account and serves turns; two are two, and one has to be chosen,
  because that decides whose subscription pays. Writing any entry replaces the
  found set entirely, and the listing says which of the two it is showing.

- **A credential the backend refused, remembered against its account.** On the
  second provider it is the only signal there is: `auth.json` records no date to
  count down to, and `codex login status` reads that file and answers "logged
  in" for a profile whose tokens are junk — measured, not assumed. So a 401 or a
  403 on a turn is kept with the backend's own sentence and reported on
  `accounts` and `status`, and the next turn that works clears it. Distinct from
  `dead`, which is this side failing to read or spend a grant at all.

- **A warning before a borrowed login lapses.** A Claude profile records the
  date its own client counts down to, and within seven days of it `accounts`
  says how long is left and `status` says what renewing takes. It is the one
  date worth interrupting for: past it the client cannot refresh the profile
  either, and asking it to try blanks what is left of the stored grant, so
  without the notice the first sign is a grant that emptied itself. A Codex
  profile records nothing equivalent and says nothing rather than guessing.

- **`declared` on every account row**, true only for a profile named in
  `[profiles]`. It is what separates the account `accounts remove` can drop by
  deleting a line from the one where there is no line to delete.

- **Who is paying, on every surface that has room for it.** Each row names the
  store it was read from; the status line receives the serving account whether
  or not a figure is known; `exec` prints one line before the client starts. And
  a profile that has become a *different account* since it was chosen is marked,
  which is the one failure borrowing introduces: the directory keeps its name
  while the identity behind it moves.

- **The quota snapshot survives a restart, where its reset says it still can.**
  Only the token tally was written down; every percentage went with the
  process, so a restarted daemon printed empty rows until the next turn was
  made as each account. Snapshots now live in `quota.json` beside `spend.json`,
  each window carrying the reset the provider gave and the moment the figure
  was taken. On read-back a window whose reset has passed is dropped — it
  describes a window that is back to zero, which is the reassuring direction to
  be wrong in — and so is a window with no reset stated and a record with no
  moment, since neither can be shown to still hold. What survives comes back
  with its age, so `usage` says `2h ago` rather than nothing at all.
  `usage --refresh` replaces it as before.

### Changed

- **Every version this proxy prints carries a build id.** `0.12.0+ab12cd3`,
  with `-dirty` where the tree had uncommitted changes and `+unknown` where
  there was no git to ask — read at build time, since a binary that has been
  installed or shipped has no checkout to consult. It is what `--version`, the
  `daemon` line of `status`, `stop`'s before and after, the `supervisor
  install` notice, and `status.version` over the socket all say. The point is
  the question those verbs are asked after a rebuild: two builds of one version
  number used to be one string, so `stop` reported "on the same build" for a
  daemon that was demonstrably not it. A caller must not parse the string —
  `docs/api.md` §6 already said not to compare versions, and the suffix is the
  part a comparison gets wrong. What goes upstream is unchanged:
  `[upstream].client_version` names a client the backend filters its catalog
  by, and a build id there would describe a different program.

- **`stop` names the supervisor that brought the daemon back.** Under launchd
  a stop is how a running daemon is replaced by the build on disk, and
  reporting that as "something started it again" describes the mechanism the
  operator installed as though it were a coincidence. The sentence reads
  `stopped 0.12.0; launchd started it again as …` where the daemon that went
  reported `supervised: true` — read from the process about to stop, the only
  one that can answer it. Unsupervised, or where supervision could not be
  established at all, the wording is unchanged: naming a supervisor nothing
  checked for would be a claim rather than an observation.

- **`proxenos start` is the verb for a background daemon, and `--detach` is
  gone with no alias.** `run` held the terminal, `run --detach` backgrounded,
  and `stop` was the counterpart of neither — one name for two things an
  operator picks between, with the choice spelled as a flag. `start` does
  exactly what `run --detach` did: same readiness poll, same log tail quoted
  when the child dies, same `--port`. `run` keeps the foreground, which is what
  a supervisor asks for, and the supervised unit still runs `run`. Where a
  daemon is already answering, `start` names it — `already running: 0.12.0
  (pid 4711), supervised`, read from the `status` payload's `pid` and
  `supervised` — and exits 0 without starting a second one, rather than
  refusing: the state it was asked to produce is the state that holds.

- **`usage` prints a header table, one row per window.** Every account row used
  to end in the same long sentence — "codex reports quota when a turn is made;
  this daemon has recorded no turn as this account yet" — repeated once per
  account, which is a paragraph where four cells would do. It is
  `NAME PROVIDER USED RESETS SOURCE AS OF` now, with a `*` on the account
  serving turns and one row per window, since an account can hold a five-hour
  window beside a seven-day one and each has its own reset. A row with no
  figure says why in the AS OF cell — `no turn yet`, `no relayed turn yet`,
  `per token`, `not reported` — and the long explanation is one note under the
  table, said once, naming `usage --refresh` as the way to fill the empty rows.
  The `usage` payload gained `reason` and `served_tokens` per account so the
  renderer reads a code rather than matching on prose; `--json` is otherwise
  unchanged.

- **`status` prints the tier table in ladder order.** The payload is an
  unordered object and arrived sorted by name, so the four rows read `fable,
  haiku, opus, sonnet` — an order nothing else in this proxy uses. They read
  `opus, sonnet, haiku, fable` now, which is what `models` already prints in
  its TIER column.

- **`--json` means one thing everywhere: the control socket's payload for that
  verb, unrendered.** It already meant that on `accounts` and `usage`, and now
  means it on `status` and `models` too, which had no way to hand over what they
  were rendering. `env --json` is **removed, with no alias**: it printed the
  client settings document, which has a verb of its own — `proxenos settings` —
  so on one verb out of four the flag named a different verb's output rather
  than that verb's payload. `env` renders shell exports and nothing else.
  `statusline` is untouched; its `--json` was never one of these.

- **`models` prints a header table naming which tiers reach each id.** It is
  `MODEL WINDOW TIER` now; the list of ids and windows left the question an
  operator actually has — which of these does a turn go to — answerable only by
  reading `status` beside it. The tiers come from the `tiers` method, which
  already carries the mapping, so nothing was added to the `models` payload;
  where that read fails the column is left off rather than printed empty.
  `window unknown` is unchanged for a catalog that states none.

- **`status` names the serving account, the daemon, and the tier mapping as a
  table.** The `auth` line leads with the name `accounts` lists the account
  under — the string every account verb takes — instead of the word
  `connected`, which said nothing that the line's existence had not already
  said; the address, kind and provider follow it, and a payload with no name
  renders as it did before. A `daemon` line names the build and the pid, and
  says `supervised` or `not supervised` where the daemon can tell — `stop`,
  `supervisor` and `run --detach` all talk about that process and nothing named
  it. The four tier rows are a table under `TIER  MODEL`, with a `STATE` column
  carrying what used to trail off the end of a row in parentheses: `inert while
  relaying`, or `as <account>` for a pinned tier. The `status` payload gains
  `pid` and `supervised`, additively (`docs/api.md` §3); `supervised` is null
  where nothing establishes an answer, and the line is then left out rather
  than claiming one.

- **`accounts` prints a header table.** The rows had no header, a column that
  said `key` where an address belonged, the raw source path, and every state —
  refused, identity changed, a login about to expire — as a parenthesised
  sentence trailing off the end of the row. It is `NAME PROVIDER KIND ACCOUNT
  SOURCE STATE` now: `KIND` says `profile` or `key` in the operator's words
  (the payload's own `kind` still names the credential), `ACCOUNT` is the
  address, else the id, else the subscription, and never the word `key`,
  `SOURCE` abbreviates `$HOME` to `~`, names a keychain as `keychain`, marks a
  profile nobody declared as `(found)`, and is the one cell that is ever cut.
  `STATE` is one phrase — `refused`, `identity changed`, the renewal countdown,
  else `ok`. The notes under the table are unchanged, except that the one
  saying these were found is said only where the whole set is. The
  confirmations the account verbs print follow one pattern too: what happened,
  then the consequence or the next thing to type, on a second line rather than
  after a semicolon.

- **The account verbs are verbs now, not flags.** `proxenos login` is gone with
  no alias: it was two unrelated commands told apart by `--key` and
  `--profile`, and `accounts` used `--use`, `--forget` and `--rename` as
  actions, so what a command did was decided by which flags were present. The
  surface is `accounts` (also `accounts list`, `--json` for the socket's own
  payload), `accounts login NAME --provider codex|anthropic [--path DIR]`,
  `accounts add-key NAME --provider codex|anthropic`, `accounts use NAME`,
  `accounts rename OLD NEW`, and `accounts remove NAME`. The account is
  positional everywhere and is called `NAME` everywhere — it used to be spelled
  `--as` in one verb and `--use` in another. `--provider` is required on both
  verbs that add an account and no longer defaults to `codex`: the two
  providers refuse each other's credentials, and a silent default is found out
  later, from an account that cannot serve.

- **`accounts remove` works on a declared profile.** It used to refuse one and
  point at the `[profiles]` entry the operator would have to delete by hand. It
  deletes the entry itself now; the grant stays exactly where it is, because it
  belongs to the program that owns the directory. A profile this daemon *found*
  rather than one it was given is still refused, saying both that it was found
  and that `[profiles]` is empty.

- **`accounts.forget` is `accounts.remove` on the control socket**, and its
  answer's `forgotten` field is `removed`. One operation was spelled three
  ways — the flag, the method, and `remove` in the store underneath both — and
  the socket was the odd one. Renamed rather than aliased, which `api.md` §6
  permits on a minor bump before a second caller exists; `accounts.forget` had
  arrived the same way, replacing `disconnect`.

- **A credential now says whose endpoints it belongs to**, as well as which of
  that provider's two it reaches. The relay asks about the provider, since a
  borrowed grant and a stored key on that provider are spent at the same
  endpoint. Each provider's subscription path is addressed as its own client. Both transports on the first provider's side ask the same
  question before the endpoint has to, so an account on the other provider is
  refused here rather than upstream, where the answer names neither half.

- **`auth.dead` keeps its question and changes its answer.** There is no refused
  token to retire, so it means the grant cannot be spent as it stands, and it
  clears by itself once the owning program refreshes.

- **Which account serves turns moved to this daemon's own state**, in
  `selected.json` beside the token tally, because a borrowed profile cannot be
  written. One declared account serves without being chosen; more than one with
  nothing chosen is refused rather than resolved to whichever comes first.

### Added

- **A borrowed Codex grant is refreshed the way a Claude one is.** When a
  profile the daemon borrows from is asked about (`usage --refresh`) and its
  access token has lapsed, the daemon runs a cheap `codex exec` turn against
  that profile — the owning program rotates the token from inside the directory
  it owns, exactly as `claude -p` does for an Anthropic profile. A borrowed
  Codex profile no other session drives no longer silently expires on the tenth
  day. `codex_program` points a launchd-supervised daemon at the CLI. That the
  turn rotates a genuinely lapsed grant is derived from the mechanism, not
  observed here (`roadmap.md` §L).

### Fixed

- **`accounts login` accepted a name the key store already held**, producing
  two accounts one `accounts use` could not tell apart; `add-key` had refused
  the reverse all along. Both directions refuse now.
- **`reload` could take the serving profile away without saying so.** The
  answer now carries who serves afterwards, and the CLI says when that is
  nobody.
- **`accounts use` on the account already serving reported a switch and paid
  for one.** It ended every conversation and dropped every quota figure to
  arrive where the daemon already was, then said `still on <provider>`. It now
  changes nothing and says `already serving turns as <name>`.
- **`exec --model haiku` on a relaying account sent the second provider a
  first-provider id.** The launch environment handed every tier's id to the
  client, shared table included, so a relayed tier arrived upstream as
  `gpt-5.6-luna`. A relayed tier now states an id only where the operator
  named one for that account — a pin, or `[accounts.<name>.tiers]` — and the
  client's own id relays verbatim otherwise.
- **Every real tool-search result was refused with a 400.** The client names a
  discovered tool as `{"type": "tool_reference", "tool_name": ...}`; the proxy
  read the field as `name`, so a conversation that had run a tool search could
  not continue — `messages[N].content: data did not match any variant of
  untagged enum Content`. The fixture the tool-search probe replays had been
  written with `name` rather than captured, which is why `doctor` kept passing
  the capability the proxy was rejecting. The fixture now carries what the
  client sends.

## [0.12.0]

One figure reported a floor it had not measured; another reported a lifetime it
could not know. Both now stop at what is actually known about them.

### Changed

- **A restart reset the per-account token tally to zero, and zero is not a
  figure anyone measured.** A supervised daemon is replaced on every install,
  so this was the ordinary case. The tally is now written to `spend.json` under
  the configuration directory and read back at startup, so what this daemon has
  served as each account carries across restarts. It holds an account name and
  two token counts, and no part of any credential. Two daemons pointed at one
  directory merge by taking whichever count is higher per account, and a write
  re-reads the file before replacing it so a write that landed in between is not
  discarded. The file is replaced by rename rather than written into, so a
  daemon killed mid-write comes back holding the last finished tally instead of
  a truncated one; an unreadable file is treated as an empty tally rather than
  as a failure. The quota snapshot deliberately does
  *not* persist: upstream still holds it and `usage --refresh` recovers it
  exactly, where a percentage read back from disk would describe a window that
  may have reset since. `docs/proxy-behavior.md` §6.1 states both halves.

- **The stem that says "setup token" is worn by two credentials.** `claude
  setup-token` mints one valid about a year; the harness's own OAuth access
  token, in its `Claude Code-credentials` keychain entry, begins with the same
  `sk-ant-oat` and is valid for hours. Classification files both as a
  subscription token and nothing downstream separates them, so a key that dies
  this afternoon is stored, rendered and relayed as one that lasts a year, and
  when it expires no field says why — there is no refresh for a key by design.
  The classification is unchanged, because a bare bearer has no structure to
  read without decoding it and decoding a credential to classify it is a new
  way for a secret to reach a log. What changed is that the ambiguity is now
  written where a reader of `classify()` and a reader of the spec will see it,
  and `login --key` for an `anthropic` key with that stem names both
  credentials on **stderr** where stdin is a terminal — the one moment a person
  is present to be told. It names the stem and no part of the key; a piped
  login is byte for byte what it was, on both streams, and the guided
  `--setup-token` flow says nothing extra.

## [0.11.0]

An account held as a spare can now state its own headroom without first being
made to serve a turn, and a key on the second provider is no longer two
credentials sharing one sentence.

### Changed

- **`usage` listed every account and could only ever fill in one of them.** A
  quota figure reached the store two ways, and both filed under whichever account
  was serving turns: it rode a relayed turn, or `usage.refresh` asked for it as
  the serving account. So an account held as a spare had exactly one route to a
  figure — become the serving account and make a turn — and that is the account
  whose headroom decides whether to switch to it in the first place. `usage.refresh`
  now asks once per stored account, each on its own credential, and files each
  answer under its own name. Nothing about which account serves turns is read or
  changed. A row whose credential cannot hold a subscription figure is not asked
  and keeps the sentence it had; a row that is asked and refused says so on its
  own line and leaves every other row alone; and asking never refreshes the grant
  of an account the operator did not select, because rotating a token family for
  an unused account would retire a token some other holder is still using.

- **On the second provider, a key is two credentials wearing one word.** A Claude
  subscription setup token and an anthropic API key were both stored as `key` and
  both read the same line, though they are metered in opposite ways: the setup
  token draws down a subscription whose figure arrives on the next relayed turn,
  while an API key has no ceiling and bills per token. The store now records which
  of the two a key is — a classification of shape, never any part of the secret —
  and `usage` has a line for each. A key stored before the field existed, or whose
  shape matches neither stem, gets a third line that claims neither: a prefix is
  evidence rather than proof, and nothing re-reads a stored secret to classify it
  after the fact.

## [0.10.0]

Everything the meter and the confirmations say about an account now stops at
what this daemon actually knows. Six lines were making claims about the world
from one process's memory, or printing a number while dropping the warning the
provider attached to it, and each was wrong in the reassuring direction — the
one an operator does not go and check.

### Changed

- **`usage` says what this daemon has recorded, not what an account has spent.**
  Every absent-figure reason was a claim about the world made from one process's
  memory: a turn relayed by a CLI process — `doctor --live` makes one — reads the
  same quota headers and exits holding them, so the account had spent something
  the daemon never saw while the meter said none had. The three reasons that
  claimed spend now scope the claim to this daemon's own record.
  `docs/proxy-behavior.md` §6.1 carries the rule.

- **A quota window carries what the provider said about it.** A turn's headers
  state a status, a threshold, and which window speaks for the account; all of it
  was dropped at parse and the meter printed the percentage alone. An account at
  93% with the provider's own `allowed_warning` on it read exactly like one at
  93% with no warning. The windows now keep their status, their threshold, and
  the provider's `representative-claim`, and the overage window is kept rather
  than parsed away.

- **A figure whose window has turned over says so.** The daemon learns a figure
  only when a turn is made, so after a reset with no turn since, the meter showed
  spend against a window that is back to zero — wrong in the direction that sends
  an operator to switch accounts they did not need to switch. Staleness is a
  property of one window and never of a snapshot, since a five-hour window turns
  over while a seven-day one has not. The figure is kept and marked; it is never
  rewritten to zero, which is a number the provider never gave.

- **A key's row states the absence of a ceiling, not the absence of a figure.** A
  key is metered per token, so it is the one account whose spend accrues with
  every turn and the only one with no percentage to show — an absence that read as
  safety. It now says nothing bounds its spend, followed by the one quantity
  available without a price list: the tokens this daemon has served as it. No
  cost is stated and none is estimated, and the count says it is a floor.

- **`accounts --use` says how far the switch moved.** The same six words stood for
  a move between two accounts on one provider and a move across providers, which
  changes the backend, the path a turn takes, and the subscription drawn down. The
  confirmation now distinguishes them, and says nothing about a subscription
  changing on the first account stored, where nothing changed.

- **A refused switch names the way out.** Tier mappings are shared and a catalog is
  one account's menu, so a mapping made for one account is refused on every switch
  to an account on another plan. `[accounts.<name>.tiers]` already existed and
  already solved it; the refusal never mentioned it. It does now.

### Fixed

- **`login --key` says what it is waiting for.** It read stdin to EOF and printed
  nothing, so a terminal operator saw a hang, and the only hint that ctrl-d was
  wanted arrived after an empty read. At a terminal it now prompts on stderr and
  reads without echoing, over the same seam `--setup-token` uses. A piped key is
  unaffected: stdout carries only what it stored, and stderr stays empty.

## [0.9.0]

The `doctor` matrix stops claiming more than it measured. The relay path (§9)
had no live arm at all, on a rationale the code beside it disproved, and the
line under the matrix named the translation path and the account it spent
whether or not anything ran there. Both now say what the run actually
established, and name what it left alone.

### Added

- **The relay probe (§9) runs under `doctor --live`.** It was skipped, on a
  rationale that was wrong: the skip said a live run needed the serving account
  switched to the second provider, while the probe was already building its own
  store and its own authorizer and never depended on the selection at all. It
  now sends a turn to the second provider's real endpoint, authorized as a named
  account read from the store — exactly one on that provider is used, several
  need the new `--relay-account <name>`, and none skips the row saying what the
  store holds. Nothing about which account serves turns is read or changed.

  The live arm establishes the answer half only, and the row names the half it
  cannot: forwarding is the whole behaviour of this path, so the outbound bytes
  leave on a socket the process cannot read, and the request-half checks stay
  with the replay arm rather than passing over a value nothing looked at. The
  coverage line names the relayed account separately, because it is a different
  account from the one the translating probes spend.

### Fixed

- **The coverage line no longer claims a path no probe ran on.** Under the
  matrix, the relay half was derived from the outcomes while the translation
  half was a constant, so `doctor --probe relay` reported the translation path
  as exercised and named the account it spent, on a run that spent nothing of
  it. The line is assembled from the outcomes now, and a path has three states:
  a passing row means exercised and its account is named, every probe failing
  means the path was reached and established nothing, and nothing running (or
  everything skipping) means it was not exercised and no account is named. Each
  is a heading, every path appears under exactly one of them, and a heading with
  nothing under it is not printed. A full run still says plainly that the
  translation path was exercised.
- **A request that did not ask for a stream is no longer answered with one.**
  `POST /v1/messages` returned `text/event-stream` whatever the caller sent,
  including for `"stream": false` and for a body with no `stream` field at all —
  a content type the caller never asked for and did not agree to parse. It is
  now answered with a single `application/json` message body, folded out of the
  same frame sequence the streaming path renders: blocks closed, deltas
  concatenated, tool arguments parsed back into values. The field set is held
  against `fixtures/surface/plain-generation.json`, a captured answer from the
  real endpoint, in both directions — no key it never carries, and every key it
  does except `stop_details`. A failure on this path is a status and an error
  body rather than a 200 carrying an error frame, since nothing has been
  written when it happens. Claude Code always streams, so no harness behavior
  changes; what changes is that the ingress now answers every other local
  caller the way it claims to. `docs/api.md` §1 and
  `docs/proxy-behavior.md` §5.5.

- **`supervisor install` no longer reports success over a job that cannot
  start.** The supervised job runs `run`, and `run` refuses a port another
  daemon holds, so installing while a hand-started daemon is up left launchd
  respawning into the same refusal every ten seconds while the install had
  already printed "supervising". It now observes what is answering, over the
  control socket rather than by guessing at the port, names its version, says
  the supervised job cannot take the port yet, and names `proxenos stop` as the
  way to hand over. It does not stop that daemon: this verb installs a
  supervisor. What it reports is what will still hold the port — the observation
  is taken between the bootout a reinstall performs and the bootstrap that
  follows, so a reinstall over the supervisor's own daemon prints nothing, and
  neither does an install with nothing answering.

## [0.8.0]

The proxy's own surface is measured against a real one, and the daemon stops
being something an operator has to notice is gone. Every emitted shape now has
a captured answer to be held against, including the two kinds this proxy builds
rather than passes through; and a `supervisor` verb installs the thing the code
had been assuming existed since the first release.


### Fixed

- **The supervisor verb compiles where launchd does not.** The helper that
  reads a home directory's owner is a Unix idea, and it was written without a
  guard, so a Windows build failed to compile a verb that refuses at runtime on
  that platform anyway. The refusal is a runtime decision; compiling is not.

### Added

- **`supervisor` brings the daemon back when it dies.** Nothing did. The code
  already reasoned about living under a supervisor — the window `stop` waits
  through is sized for launchd holding a respawn for ten seconds — and the
  thing that would satisfy that assumption was never built, so the first sign
  of a daemon that had simply gone was a launch that failed. `supervisor
  install` writes a per-user LaunchAgent and hands it to launchd, `uninstall`
  removes both, and `status` reports what the supervisor makes of it. The job
  runs `run` in the foreground, because a process that forks away leaves
  launchd supervising something that has already exited; it logs where the
  daemon already logs; and it carries no credential, because a plist in the
  user's home is world-readable and the store is what holds those.

  **macOS is the only platform implemented, and every other one refuses by
  name**, saying what a systemd user unit would take and naming `run --detach`
  as the way to start the daemon meanwhile. A unit written but never accepted
  by a supervisor reports success and supervises nothing, so nothing writes
  one — and an install that launchd refuses removes the file it had just
  written rather than leaving that state behind.

  **The socket path was the hazard worth settling before writing anything.** It
  is derived from `PROXENOS_HOME` when set and from `TMPDIR` otherwise, and a
  launchd job does not necessarily see the `TMPDIR` a login shell does; where
  they differ the daemon comes up healthy on its port while every CLI verb
  reports connection refused. Both are now carried in the unit explicitly, and
  the derivation moved into one function (`control::path_for`) that the unit
  and the CLI both call, so the two agree by construction. `TMPDIR` is carried
  whether or not the installing shell names one: launchd does not hand a job an
  empty environment, so omitting it would mean launchd's value rather than
  none, while the path planned at install time had fallen back to `/tmp` — the
  same drift, arrived at from the inside. The unit records the value the
  derivation actually used, fallback included. `supervisor status`
  compares the installed unit against the one the current environment would
  write and says plainly when they have drifted apart.

- **`record surface` captures the real Messages surface.** The proxy's whole
  product is an Anthropic Messages surface, and until now nothing here had ever
  seen one: every conformance claim was derived from documentation and from
  captures of the *client* side. Neither existing capture mode could produce an
  answer from the real endpoint — `record upstream` is wired into the
  translating path, where the events belong to the other provider, and a
  relayed turn (`proxy-behavior.md` §9) streams back untouched with nothing
  recording it. The new mode makes a short fixed list of calls itself, through
  the same relay code a §9 turn takes, and writes each as a fixture under
  `fixtures/surface/`. `--account` is required and must name an account on the
  second provider; `--only <name>` captures one exchange, because a capture on
  disk is quota already spent. Response headers are scrubbed by name before
  anything is written, including the organization and workspace ids — a fixture
  is committed, and those say whose account paid for it.

- **The emitted surface is held to the measured one.** A conformance test
  replays the corpus through the shipping translator and compares what it emits
  against the captured answers at the level of shape: which SSE events exist,
  which fields each carries, and which keys an error envelope has. Content is
  never compared — a token count that moved is not a defect. The rule is a
  subset in one direction: a field the real surface carries and this proxy omits
  is one a client already tolerates being absent, while a field this proxy emits
  that a real answer never carries is something the client was never built to
  receive. Shapes no capture reaches are named rather than skipped silently,
  because a skip reads exactly like a pass.

- **The two reconstructed block kinds are measured.** The corpus named four
  shapes it could not reach, and all four were the places this proxy builds
  blocks rather than passing them through: the other provider's reasoning
  becomes `thinking` and `thinking_delta`, and its native search becomes
  `server_tool_use` and `web_search_tool_result`. Unmeasured is exactly where a
  subset check is blind, so two captured exchanges were added — an
  extended-thinking turn, whose code is spoken back through the thinking
  deltas, and a turn that runs the `web_search` server tool. The search turn
  cannot force a code through the model's own search; what it proves is that
  the block shapes came from a search the server actually ran rather than from
  a plausible reconstruction. Both emitted shapes are subsets of the real ones:
  a real `thinking` block start also carries `signature`, a real
  `web_search_tool_result` also carries `caller`, and no field this proxy emits
  is absent from a real answer.

## [0.7.0]

The operator surface catches up with the second provider: a guided login
for the subscription token, per-account quota read off the turns the relay
already makes, a doctor that probes the relay and states its own coverage,
an honest `status` for a relaying account, and guards for the isolation
holes and silent overwrites that live use surfaced.

### Added

- **The launch settings disable the client's commit attribution.** A session
  started through `proxenos exec` or configured from `proxenos env` now receives
  `attribution.commit: ""`, the client's own empty-template form for appending
  no trailer to a commit it makes. Which model served a turn is not a fact a
  commit message is the place to record, so it ships on both the translating and
  the all-relay path. `client.disable_commit_attribution = false` puts the
  client's own default back, and then the `attribution` object is absent from
  the document rather than present and empty.

- **A ninth `doctor` probe, `env-contract`, holds the launch surface to its
  contract.** `ENABLE_TOOL_SEARCH` and `CLAUDE_CODE_DISABLE_1M_CONTEXT` are both
  load-bearing and both fail silently — the first hands the deferred tool set
  back into the context, the second lets the client invent a million-token
  window for an id it cannot recognize — and nothing asserted that `env` and
  `exec` still emitted them. The probe renders the environment for a translating
  mapping and an all-relay one and asserts on what was rendered, so a launch that
  emits nothing cannot pass it. It replays nothing, contacts no backend on either
  mode, and its row is marked as proxy-answered under `--live` alongside
  `count-tokens`.

- **`doctor` now says what a run did not touch.** A failed row prints the
  probe's rationale — what breaks silently without it — while passing rows stay
  one line. Under `--live` the `count-tokens` row is marked as answered by the
  proxy: the live header says the backend answered and was billed, which is true
  of every other row and false of that one. And one line under the matrix names
  the account the run spent and the paths it left alone, so eight green rows
  cannot be read as coverage of the WebSocket transport or the relay when
  neither ran.

- **A probe for the relay path (§9).** `doctor` built its own state with no
  relay at all, so nothing in the suite drove the branch that forwards a turn
  instead of translating it — the one path whose entire claim is that the bytes
  are not touched. The new probe's marker sits inside a field the proxy has no
  type for, so a body round-tripped through its own types fails it. Replay-only:
  driving it live needs the account serving turns switched to the second
  provider for the length of the run, which is not wired.

- **`proxenos usage` now reports the second provider's accounts too.** That
  provider states rate-limit headroom in `anthropic-ratelimit-unified-*` headers
  on the response to every turn, and for a subscription credential it is the
  only place one is stated — its usage endpoint refuses that credential for want
  of a scope, so there is nothing for `usage.refresh` to ask. The headers are
  read off each relayed response and filed under the account that made the turn,
  which costs nothing: the figure rides a turn already being made, and no path
  here polls. A plan name is absent rather than guessed, and an account that has
  relayed no turn yet says a turn supplies its figure instead of claiming the
  provider reports none.

- **`proxenos login --setup-token`: a guided way to store a subscription
  token.** The same stored credential `--key --provider anthropic` produces,
  reached without the pipe-and-ctrl-d workflow it required. It says where the
  token comes from (`claude setup-token`), reads it from a hidden prompt where
  stdin is a terminal, asks what to call the account when `--as` did not, and
  refuses anything not beginning with `sk-ant-oat` before the store is
  touched — with no override flag, because a credential of the wrong kind
  stores cleanly and fails later naming the account rather than the paste. A
  non-terminal stdin still reads the token from the pipe, so scripted use does
  not regress.

### Changed

- **Operator-facing output names a provider, never an ordinal.** `status`
  printed "built-in list for the second provider" and `usage` explained a
  missing figure with "this provider" — this project's internal word for a
  role, in front of a reader who has `codex` and `anthropic` in every
  `accounts` listing. The `routing` and `catalog` lines of `status`, the
  curated note on `models`, and every per-account reason in `usage` now carry
  the stored provider id. `models` reports that id as `provider` alongside
  `curated` so the name comes from the payload rather than from the renderer's
  assumption.

- **The `status` auth line names the provider on every connected row.** It used
  to name one only where it was not `codex`, so an oauth account on the
  provider this proxy started with rendered as an address and nothing else —
  the same gap the `accounts` listing just closed.

- **Every `accounts` row names its provider.** The listing named one only where
  it was not `codex`, so a store holding both providers printed three rows whose
  provider had to be inferred from the one row that had it. It is a column every
  row fills now, whatever the credential kind, alongside the address or `key`
  that already told two rows apart. A payload that carries no provider at all —
  a daemon older than providers — still prints none, because filling that in
  would be inventing it.

- **A key re-store that would change an account's provider is refused.** A key
  stored over a key was a silent replace, including across providers, so
  `login --setup-token --as api` turned a first-provider key account into a
  second-provider one and the old key was gone with nothing said. The refusal
  names the account, the provider it holds, and the `accounts --forget NAME`
  that clears the way. Same-provider rotation is unchanged, and so is the
  existing refusal to store a key over a grant.

- **The control socket lives inside `PROXENOS_HOME` when one is set.** *Path
  change.* It was `$TMPDIR/proxenos.sock` regardless of the home, so a CLI or a
  daemon isolated into a temporary home still reached the operator's real
  daemon whenever the two shared a `TMPDIR` — and every login path ends in
  `accounts.select` over that socket, so an isolated login could switch the
  account the real daemon serves. With no home named the path is unchanged. One
  derivation answers for the daemon's bind and every CLI call, and a derived
  path over the platform's `sun_path` cap is now refused by name at both ends
  rather than leaving a daemon that serves turns and answers no verb.

- **A login no longer takes over what serves turns.** *Behaviour change to a
  v0.6.0 surface.* `login` stores a credential and selects it only where
  nothing is already serving; every login after the first leaves the selection
  alone, names the account still serving, and prints the `accounts --use NAME`
  that would switch. Storing a credential and choosing what serves turns are
  two decisions, and making them one moved every turn onto a newly added
  account with nothing said about it. The rule holds on all three paths — an
  authorization, `--key`, and `--setup-token` — and in the daemon's `login` as
  well as the CLI's; `accounts --use` is unchanged and is the verb that
  switches. An operator who relied on a second login switching for them needs
  that verb now.

- **`status` says when the tier mapping is inert.** A serving account on the
  second provider relays every id it authenticates verbatim (§9.1), so the four
  tier rows decided nothing while reading as though they decided everything.
  Unpinned rows are now marked inert and a `routing` line names the provider
  the ids relay to; a pinned tier names its own account and stays live, so a
  split mapping stays accurate row by row. A rendering change only — no field
  of the `status` payload moved.

- **The control socket's `tiers.get` is now `tiers`.** Every other read on the
  socket is a bare noun, and each coexists with namespaced writers under the
  same noun; a lone `.get` bought no capability. Renamed with no alias while
  nothing outside this repository's own CLI speaks the socket — `docs/api.md`
  §6 now names the nineteen methods that are bound from v0.7.0 on, which is
  when that freedom ends.

## [0.6.0]

The second provider, end to end — and confirmed live: relayed turns round-trip
against the real endpoint with a substituted subscription bearer, plain and
streaming both. `docs/roadmap.md` §L records what settling that falsified.

### Added

- **A tier may pin another account, behind explicit consent.** `haiku =
  { account = "spare", model = "..." }` routes that tier's turns to another
  stored account — and they are *served as* that account: its credential
  authenticates every upstream request, a token refresh is written back to the
  entry it was read from rather than to whichever account is selected, and a
  pooled socket opened as one account is never reused for another. A pin naming
  an account the store does not hold refuses the turn by name; there is no
  fallback to the serving account, because that spends the wrong subscription's
  quota invisibly. Absent the `cross_account_tiers` consent key, a pinned entry
  refuses the daemon at startup and `tiers.set` at write time, naming the key;
  the `cross_account_tiers.set` socket method grants or withdraws it, always
  persisted, with withdrawal refused while a pin is in force.
  `docs/proxy-behavior.md` §7.1.
- **An account states its provider.** Each stored account carries which
  provider's endpoints its credential is spent against; `accounts` and `status`
  name it where it is not the default. Files written before there was a second
  provider keep their exact shape.
- **A relay-bound launch is handed no window it cannot know.** For a mapping
  served entirely by the relay, `env` and `exec` omit
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, and
  `CLAUDE_CODE_DISABLE_1M_CONTEXT`: the client recognizes those ids natively,
  an override could only replace a real window with an invented one, and the
  flag would strip the `context-1m` beta from the wire — an entitlement the
  account may actually hold. A mixed mapping states no window and keeps the
  flag; `docs/proxy-behavior.md` §7.2 states both costs.
- **`record ingress` keeps the request headers**, in arrival order, with
  credential-bearing values (`authorization`, `x-api-key`, `cookie`,
  `proxy-authorization`) redacted by name. A header's presence is the datum;
  its value never lands in a capture.
- **The connector opt-out reaches the environment.**
  `client.disable_connectors` now also exports
  `ENABLE_CLAUDEAI_MCP_SERVERS=false` — the client's own documented opt-out for
  the claude.ai-hosted servers — so a launch configured by exports alone honours
  it.
- **A quota figure per account, not per daemon.** A pinned tier's turns spend
  the account it names, so two accounts can serve one session and a single
  latest snapshot reported whichever made the most recent turn. Each figure is
  now held under the account that earned it, and `usage` keeps its shape — the
  serving account's figure stays where a reader already looks — while gaining
  `accounts`: one entry per stored account with its own windows, whether it is
  serving, how the figure was come by (riding a turn, or asked for over the
  socket), and the moment it was taken. `docs/proxy-behavior.md` §8.3.
- **An account with no figure says so, and says why.** No turn made as it yet, a
  key holding no subscription entitlement, or a provider whose quota endpoint
  is an open question. Never a zero, and never another account's figure.

- **A turn for the second provider is relayed verbatim.** A key stored with
  `login --key --provider anthropic` and pinned to a tier serves that tier's
  turns through a passthrough: the body is forwarded byte for byte, the reply
  is streamed back byte for byte, and the only thing that changes on the way is
  the header set — the bearer becomes the account's credential, any `x-api-key`
  the caller brought is dropped, and `anthropic-version` and `anthropic-beta`
  pass through as the client sent them. `docs/proxy-behavior.md` §9.
- **`login --key --provider`** states which provider's endpoints a stored key is
  spent against. `codex` by default. Only meaningful with `--key`, and refused
  rather than ignored without it.
- **`[upstream.anthropic]`** in the configuration, with an `endpoint` key.
- **A relayed turn is observable like a translating one.** `record ingress`
  captures it, holding its request as the exact bytes that were relayed rather
  than a re-encoding that would drop every field this proxy's own types do not
  model, with header values redacted by name as everywhere else. Its model id
  joins the served list `usage` states, so a status line still recognizes a
  session whose tier was remapped while it was running.
  `docs/proxy-behavior.md` §9.4.
- **`exec` upgrades a plain `--model` id to its long-context variant.** For a
  relay-serving daemon, an id whose `[1m]` variant the curated list offers is
  rewritten to it — the client's own long-context selector — and the rewrite is
  named on stderr. An id already carrying the marker, an alias, another
  program's `--model`, and every translating launch are forwarded as typed.
- **A relay-serving daemon answers `models` from a curated list**, windows
  included: the second provider's own list endpoint names ids but states no
  windows, so the figures are curated into the binary and every answer says
  `curated` rather than presenting the list as a fetch. It is a menu for
  reading, never a list to refuse by — no mapping is validated against it, and
  `status` reports the catalog as curated instead of unvalidated.
- **Launches carry `remoteControlAtStartup: false`.** A session started through
  a local proxy is a local decision. On by default beside the other client
  policy, switchable with `[client] disable_remote_control = false`.
- **Every launch forces the client's tool search on.** The client disables
  deferred tool loading the moment its base URL is not a first-party host, and
  an MCP set measured at ~101k tokens loaded up front defers to zero with
  `ENABLE_TOOL_SEARCH=true` — verified live on both paths: the relay forwards
  `defer_loading` and `tool_reference` verbatim to a backend that runs the
  search itself, and the translating path carries client-driven discovery
  (`docs/proxy-behavior.md` §2.5).

### Changed

- **A select no longer discards a quota figure, and a removal now does.** With
  every figure held under the account that earned it, a select changes which
  account `usage` is about and invalidates nothing; forgetting an account drops
  its figure whether or not it was the one serving, because that entitlement
  belongs to a subscription this daemon can no longer spend.
- **Routing on the relay path is by model id, and one id may be claimed by at
  most one account.** A tier that pins nobody names the account serving turns,
  so selecting a key for this provider relays every turn rather than sending
  them to the other provider's endpoint. Two mappings naming one id and two different accounts
  refuse the turn, naming both. Picking one would spend a subscription nobody
  pointed at that turn and say nothing.
- **`AccountStore::add_key` takes the provider.** A required parameter rather
  than a default: the two providers' endpoints refuse each other's credentials,
  and a key that silently claimed the wrong one fails as an authentication
  error naming the credential rather than the destination.
- **An id no mapping names follows the account that would authenticate it**:
  relayed when that account is on the second provider — the credential travels
  only to its own provider's endpoint, and that provider judges the id — and
  translated with the id passed through when it is on the first. A launch-time
  model override rides on this symmetrically, with no mapping edit. Crossing
  providers still takes a pointer (a pinned tier, or a changed selection),
  because serving an id from an account nobody named would spend a
  subscription nobody pointed at the turn. `docs/proxy-behavior.md` §9.1.
- **The `claude-api` skill-deny default is resolved per launch.** Unset,
  `deny_skills` denies the skill only where a tier translates — the skill
  documents the second provider's API, the wrong reference for a translated
  session and the right one for a relayed session. A written list stays the
  operator's rule on either path, and `status` reports the list a launch would
  actually apply.
- **`docs/proxy-behavior.md` §4** no longer claims transports are
  interchangeable below the session. The choice of transport belongs to the
  provider: the relay is HTTP with SSE and nothing else. §9 is the relay; what
  was §9 (Testing) is §10.

### Fixed

- **A pinned mapping no longer refuses the daemon at startup, nor a switch.**
  `tiers.set` already excluded pinned entries from catalog validation, but the
  daemon's start and `accounts.select` still measured them against the serving
  account's menu — so a pinned mapping accepted over the socket was refused at
  the next start, silently until then. One function now holds the exclusions
  (pinned and relayed alike) at all three doors.
- **`record ingress` and `record upstream` honour `--port` and
  `PROXENOS_PORT`.** The variable was read by `run` and silently dropped by
  `record`, which assembled its daemon's arguments by hand.
- **The ingress imposes no size limit on a request body.** The body extractor's
  2 MB default refused a real client turn — a full system prompt and a large
  tool set — with a plain-text 413 the client read as retryable and looped on.
  The backend's own limit is the real one, and the daemon is loopback-only.
- **The request path's query string is relayed as sent.** Observed live: the
  client posts `/v1/messages?beta=true`, and the ingress never read the URI, so
  the relay posted to the bare endpoint. The query now follows the body's rule
  — forwarded exactly as sent, never invented. `docs/proxy-behavior.md` §9.2.

## [0.5.0]

The project is named for what it does. `codex-cc-proxy` named one provider on
one side of a daemon that is about to serve more than one; **proxenos** — the
ancient Greek office the word "proxy" descends from, a citizen representing a
foreign guest's interests in his own city — names the role and no provider.

### Changed

- **Everything the old name reached is renamed, with no aliases kept.** The
  repository, both crates (`proxenos`, `proxenos-core`), the binary, and the
  environment prefix: `CODEX_CC_PROXY_PORT` and `CODEX_CC_PROXY_HOME` are
  `PROXENOS_PORT` and `PROXENOS_HOME`. One rename on a minor bump under the
  pre-1.0 exception recorded in `docs/api.md` §6. Wire identity is untouched:
  the originator and user agent the upstream sees never carried the project
  name.
- **A store under the old name refuses loudly instead of vanishing.** A binary
  finding `CODEX_CC_PROXY_HOME` exported, or a default home written by the old
  binary with nothing at the new path, refuses to run and says exactly what to
  move where. Starting fresh with an empty store would have read as every
  credential having disappeared. Nothing is migrated automatically — an old
  daemon may still be running against that directory.

## [0.4.0]

The tier mapping stopped belonging to the daemon and started belonging to an
account, and the one socket method whose name did not match its neighbours was
renamed while nothing but this project's own CLI was there to notice.

### Added

- **A tier mapping belongs to an account.** A catalog is one account's menu, so
  one mapping is only ever right for the models every stored account has — and
  that intersection shrinks with each account added. Two subscriptions on
  different plans are offered different models; a key account beside a
  subscription need not overlap at all. `[accounts.<name>.tiers]` replaces the
  tiers it names and no others, and `effort` under `[accounts.<name>]` replaces
  the shared ceiling rather than being capped by it. The key is the name the
  store files the account under, because a key account has no id to be named by
  and the name is what every account verb takes.

  Three things follow, and each was a way for the mapping to be quietly wrong.
  A **switch re-resolves it**, and is refused by a mapping the target account's
  catalog cannot serve — the daemon stays where it was, catalog included, rather
  than serving an account whose every turn dies upstream saying nothing about
  tier mapping. A **rename takes the section with it**, headers only, every key
  and comment under them untouched. And a **persisted change is written where
  the value is read from**: a change written to the shared table while the
  serving account shadows it would be in force now and gone at the next start.
  `tiers.set` and `effort.set` also take `{"account": name}`, which writes that
  account's section; aimed at an account that is not serving, the change is
  written and not applied, not validated against the serving account's catalog,
  and without `persist` refused rather than answered as though it had done
  something. `effort.set` with `null` under an account removes that account's
  override and reports the shared ceiling that applies again, rather than
  reporting no ceiling and letting one come back at the next start.

### Changed

- **`disconnect` is now `accounts.forget` on the control socket, and its answer
  says `forgotten` rather than `disconnected`.** The old name shipped in v0.1,
  when there was one account and disconnecting from it was the whole idea. With
  a store of several, forgetting one is an account operation, and every other
  account operation is `accounts.<verb>`. This is a breaking change to a
  semver-bound surface, made rather than deferred because nothing outside this
  project's own CLI has ever spoken the socket, and the CLI and the daemon are
  one binary — the rename lands on both at once. `api.md` §6 now states when
  that stops being true. Nothing in the CLI changes: `accounts --forget NAME`
  is what it always was.

- **A credential directory that cannot lock says what to do about it.** Every
  write takes a lock beside the credential file, and a filesystem that cannot
  take one — a home on a network mount being the case that exists — now fails
  with the file named and `CODEX_CC_PROXY_HOME` named beside it, rather than
  with an error that reads as a bug in this program. Proceeding without the lock
  is deliberately not offered: it would report success while doing the thing the
  lock exists to stop.

## [0.3.1]

### Fixed

- **Two writers of the credential file no longer overlap at all.** A write
  reads the file, changes it, and replaces the whole thing, and v0.3.0 answered
  an overlap by starting over when the file had changed since it was read. That
  check cannot cover the gap between itself and the replacement: a writer
  landing there was copied over, silently, and what it lost was a whole account
  rather than a stale token. Every write now takes a lock the filesystem
  enforces and holds it across all three steps. The lock is a file of its own
  beside the credentials — the credential file itself is replaced by rename, so
  a lock on it would be a lock on an inode the next writer never opens — and
  the kernel drops it when the process goes, so a crash mid-write leaves
  nothing for the next run to wait on. The comparison stays, because the lock
  reaches only writers that take it and an older binary or a hand edit takes
  none.

## [0.3.0]

One credential file held one account and one kind of credential, and starting a
client meant evaluating a shell expression first. None of those three is true
now.

### Added

- **More than one account in one credential store.** One file meant one
  account, so an account out of quota stopped all work rather than some of it.
  `login` now adds an account and selects it rather than replacing the one
  already stored, `--as NAME` gives it a local name, `accounts` lists what is
  stored with the serving one marked, and `accounts --use NAME` switches —
  through the daemon, which is the side holding the selection. `status` names
  the account serving turns beside the rest, and `disconnect` says which one it
  cleared and leaves the others usable. Two things turned out to belong to the
  grant rather than to the daemon and travel with a switch: a refusal, which is
  a statement about one refresh token, and the quota snapshot, which belongs to
  the account that earned it — carrying either across reports the new account as
  finished, or reports headroom it may not have. A credential file written by an
  earlier version is read as the single account it describes and migrates on the
  next write, so an upgrade costs no re-login.

  Three things about that store are worth stating on their own, because each
  was a way to lose a grant rather than a feature. An account is identified by
  the account id its grant carries, not by the name it is filed under, so
  authorizing one already stored replaces it instead of leaving two entries
  sharing one refresh-token family; a label already naming a *different*
  account is refused rather than honoured. A refresh writes the account its
  grant belongs to, not whichever is selected when the write lands, so
  switching accounts during one cannot drop A's rotated grant into B's entry.
  And the file is replaced rather than truncated in place, because it now holds
  every account and a write that stops halfway would take all of them.

- **`run --detach` starts the daemon in the background.** The command returns
  once the daemon answers the control socket, printing the pid, the log path,
  and `stop` as the counterpart. A child that dies at startup is reported with
  its own log quoted and a nonzero exit; a second detach is refused while a
  daemon still answers, because it would take over the first one's socket file.

- **The proxy publishes the policy a client cannot be told by environment
  variable.** Two of the things a client has to be told live in its settings
  file, and no export reaches them — checked against the whole settings schema,
  there is no per-skill variable and nothing that points at an extra settings
  file. `[client]` now carries `deny_skills` and `disable_connectors`, the `env`
  control method carries a `settings` half beside `variables`, and a new
  `settings` verb prints one complete client settings document. That document is
  complete on its own: measured, a client reading only its `env` block, with no
  `ANTHROPIC_*` in its environment, still reached the proxy.
- **The bundled `claude-api` skill is denied by default.** Measured against a
  local capture stub, one invocation lands 73,000 to 93,000 bytes — roughly
  18,000 to 23,000 tokens — in the conversation as a user item, where it sits
  for the rest of the session and is charged every turn, while a refused
  invocation costs a 43-byte error. A range because both ends were measured and
  the figure moves with what else the session has loaded. Denying does not remove it from the listing the client sends; what it
  stops is the load. It is also the wrong reference for a session served here,
  documenting another provider's model ids, prices, and parameters. Switchable
  in `[client]`, and `status` names it so the person holding "Skill execution
  blocked by permission rules" can find the key that undoes it.
- **`stop`**, so a daemon can be replaced with the CLI that replaced it. It
  watches the `instance` id `status` now carries rather than watching the socket
  fall silent, because silence is a statement about timing: a supervisor quick
  enough leaves no gap to see, and one that throttles a respawn leaves a gap
  longer than any sensible wait. Under a
  supervisor, stopping is how a running daemon picks up the build on disk, and
  this reports what it observed afterwards: gone, or started again and on which
  version. The answer reaches the caller before the process goes, because a
  closed connection with no reply cannot be told apart from a crash. An
  in-flight turn is cut, which is what the person typing it asked for.
- **A newer CLI will not quietly serve an older daemon.** One file is both, and
  replacing it on disk does not restart what is already running, so this is what
  an ordinary upgrade leaves behind. The policy half of the `env` payload is
  therefore always present, empty where there is none, and absence means only
  that the daemon predates it. `settings` and `exec` refuse such a daemon rather
  than producing a document that looks complete and lacks a permission rule;
  `env` continues, because routing is all it ever carried, and names the daemon
  it is talking to. `status` reports the version actually serving the socket and
  says so only when it differs from the binary that asked. The decision reads a
  capability rather than comparing version strings, which would force a policy
  about which differences matter and get it wrong for a patched build.
- **`exec`**, a launcher: it applies both halves and runs the command, so
  starting a client is one step. Nothing is written to disk — the policy rides
  inline on the client's own settings flag. It refuses before starting anything
  when the daemon is not answering, and when the forwarded arguments already
  carry `--settings`, because the client keeps only the last such flag and drops
  the first without a word.

### Changed

- **A refusal follows the grant, not the process.** The backend refusing a
  refresh used to latch a flag for the life of the daemon, so the re-login its
  own message asks for did not help until a restart — and a login through the
  CLI never reaches the daemon at all. The refused refresh token is what is
  remembered now: a different grant is tried, and that one is still never
  retried. `status.auth.dead` means the grant currently stored is the refused
  one, and answers itself again if the account is selected back.

- **Switching accounts reaches conversations already running.** A WebSocket
  conduit fixes its account when it dials and reuses that connection for the
  conversation's life, so a switch used to leave every live session billed to
  the account just moved off. They are dropped and dial again, each paying one
  full upload.

- **The model catalog follows the account.** It is fetched at startup for the
  account selected then, and asked for again when `accounts.select` or a
  `disconnect` that hands over changes who serves turns. That fetch is best
  effort: a failure keeps the list already in force, because a fetch that did
  not answer is not evidence that a model went away, and replacing a real list
  with the fallback would withdraw models the account has. So the list can
  still belong to somebody else, and `status.catalog_stale`,
  `status.catalog_account`, and `models.stale` say when it does rather than
  presenting another account's plan as this one's.

- **An account can hold an API key instead of a subscription grant.**
  `login --key` reads a secret from stdin — never from an argument, which is
  visible to every process on the machine — and stores it under the name `--as`
  gives it; `accounts --use` moves between the two kinds exactly as it moves
  between accounts. A key has no refresh, no expiry, no account id and no plan,
  and nothing reports a plausible value in place of any of them.

  One resolver decides what authenticates a request, and every path that
  authenticates asks it: both transports, the catalog fetch, and the quota
  fetch. A grant sends its token, the originator identifying a subscription
  client, and the account id it is spending; a key sends its token and nothing
  else. A credential is refused against the other kind's endpoint before
  anything leaves, in a message naming both halves, rather than being answered
  upstream with something about an invalid token. `[upstream.key]` is where a
  key is spent; it has no socket, because that protocol belongs to the
  subscription backend, and a key account has no quota to report, because that
  figure is a subscription entitlement.

  A key request is never compressed: zstd on a request body is measured against
  the subscription backend and nowhere else, and the key endpoint parses the
  compressed bytes as JSON and rejects them.

  A login through the CLI tells a running daemon to hand over, so what a switch
  carries with it — the conversations bound to the previous account, its quota,
  its model list — moves too rather than leaving a live conversation dialing an
  endpoint that now refuses it.

  Proven end to end against the replay server, which is what this suite can
  hold. A real key endpoint has answered three times and settled less than that:
  it took the key at the turn endpoint, refused a compressed body there, and
  refused the same key at the model list. `docs/roadmap.md` §L carries what that
  leaves open.

- **`doctor --live` refuses when it cannot authenticate, instead of reporting
  the backend as broken.** With no credential — or with a key selected, whose
  probe path held a grant's token source — it answered with the whole matrix,
  every row failed, under a header saying the backend answered and was billed.
  Nothing had been sent. It now resolves the credential first and says only
  that, and probes the endpoint the account's kind belongs to.

- **`accounts --rename FROM TO` changes what an account is called.** A login
  carrying no `--as` names the account by the id the backend knows it by, which
  is a UUID nobody wants to type at `--use`. Renaming moves that name and
  nothing else: the grant, the account id, and which account serves turns all
  stay where they were, and a name another account already holds is refused.

- **`accounts --forget NAME` drops an account from the CLI.** `disconnect` had
  been on the control socket since v0.1 with nothing in the CLI that called it,
  so the only way to undo a login was to delete the credential file by hand. The
  name is required and cannot be combined with `--use`: an account is gone once
  the command returns. The answer says which one went and which one serves turns
  afterwards.

- **A credential write that lost a race is redone rather than lost.** Every
  write rewrites the whole file, so the CLI's `login` landing while the daemon
  persisted a refresh could discard an entire account. A write that finds the
  file changed since it read starts over. The window is narrowed rather than
  closed — the comparison and the replacement are two operations — and what
  would close it is a filesystem lock.

- **`login` states the label actually in force.** A second call while a flow is
  running joins it, and now says which name that flow will give the account
  rather than echoing the one it was handed.

## [0.2.1]

### Fixed

- **A client interrupt no longer bricks the conversation.** An abandoned turn
  drops its WebSocket connection instead of parking it, but the session still
  remembered the last response id — so the next turn opened a fresh connection
  and sent a delta naming a response that connection had never seen. The
  backend refuses that with `400 Invalid previous_response_id`, and because the
  refusal ends the turn cleanly, the refusing connection was parked and every
  following delta repeated it: the session never healed on its own. A delta is
  now planned only for the connection that produced the response it continues;
  every other case is a full send.

## [0.2.0]

A daemon a front-end can drive. Every capability below was reachable only by
restarting with a hand-edited configuration file, or not reachable at all.

### Added

- **The tier mapping and the effort ceiling can be changed on a running daemon**
  — `tiers.set` and `effort.set`. Both move what *routes turns*, not only what
  `status` reports. Neither writes the configuration file unless asked —
  `{"persist": true}` — and every answer says which it was, because a change
  the caller believed was saved and was not comes back at the next restart with
  nothing to explain it. `tiers.set` is validated
  against the catalog exactly as startup validates it, which is why this daemon
  owns the mapping rather than a front-end. It matters most for the ceiling: a
  capped turn **succeeds** and is simply shallower than it was asked to be, so a
  ceiling set once for one purpose silently governs every front-end that arrives
  afterwards, and nothing about that is visible.
- **`login` over the control socket**, so a front-end that is not a terminal can
  start the flow. It answers with the authorization URL and returns; the flow
  completes in the background and `status` reports when it landed. There is one
  fixed callback port, so a second caller joins the first rather than being
  handed a URL whose callback would then be rejected — and an abandoned flow
  releases the port instead of holding it until the daemon stops.
- **`usage.refresh`** asks the backend for a quota figure rather than waiting for
  one. The volunteered snapshot is still the primary path and still free; this
  covers the case it cannot — a front-end with a figure to show on a daemon that
  has served no turn yet. The response shape was **captured before the parser was
  written**, and differs from the stream event's in three ways a guess would have
  got wrong: the window keys, seconds rather than minutes, and where the plan
  sits.

### Fixed

- **A persisted tier or effort change was applied even when the write failed.**
  The caller was told the write failed while the daemon had already moved —
  running a policy nobody chose, and losing it at the next restart. Validated,
  written, then applied.
- **`effort.set --persist` could write the ceiling into the wrong table.** The
  search for an existing key scanned the whole file, so a commented-out `effort`
  line below a table header was rewritten in place — producing `tiers.effort`,
  which parses, which nothing reads, and which leaves the next daemon with no
  ceiling at all after the operator asked for one.
- **A refused grant read as healthy.** `status` reports `auth.dead` now:
  `connected` stays true while the credential file is readable, so nothing else
  said that every turn was failing.
- **Two concurrent sets could revert each other.** Each setter carried across
  the field it was not changing, read through one call and written through
  another; a mapping write that read the ceiling before another caller changed
  it put the old value back for good. Read and replace now happen under one
  lock, and `Snapshot`'s fields are private so the routing table cannot be set
  apart from the tiers it is derived from.
- **A status line showed this account's quota to a session that was not using
  it.** The wrapper is configured once and renders for every session the client
  runs, including ones pointed at their own provider, and the daemon answers
  `usage` whenever it is up — so switching back and forth painted one account's
  quota over another's, in the direction that reads as headroom. The merge now
  asks whether the session's model is one this daemon serves, and passes the
  payload through untouched when it is not. `usage` reports those ids: the
  configured tiers plus every id a turn was actually made against, since a
  client that names its own model bypasses the tiers entirely.

## [0.1.2]

A cost fix on the HTTP transport, and an install script.

### Fixed

- **The HTTP transport cached nothing.** Every turn over it is a full send with
  no `previous_response_id` chain, and the body's `prompt_cache_key` — which the
  spec called the thing that drives caching — turns out to do nothing on its
  own. A `session_id` header, stable for the life of a conversation, is what the
  cache is scoped by. Measured on one four-turn conversation with the WebSocket
  disabled: uncached input per turn fell from 4,465–4,497 tokens to 625–657.
  Over WebSocket it changes nothing, because chaining already caches.

### Added

- **An install script.** One command detects the platform, downloads the
  matching release, verifies it against the release's own `SHA256SUMS`, and
  installs it. There is no flag to skip verification, and a mismatch installs
  nothing and exits non-zero — proven by serving a deliberately corrupted
  archive, since a verifying script and a non-verifying one are
  indistinguishable on a good download.

## [0.1.1]

A defect in v0.1.0 that only an installed binary could show.

### Fixed

- **`doctor` works on an installed binary.** The fixture corpus was read from a
  `fixtures/` directory relative to the working directory, which only a checkout
  has — so the first command the README suggests skipped all eight probes and
  established nothing. The corpus is now compiled into the binary and answers
  when there is no directory. A directory named with `--fixtures` is still the
  only thing that answers for it, so a fresh `record` capture is never shadowed
  by the compiled-in copy, and the matrix names which corpus it read.

### Added

- **Install instructions**, with the checksum step, and `cargo install --git`.
  The absent package-manager routes — tap, container image, install script — are
  named as absent rather than left to be discovered.

## [0.1.0]

First release.

### Added

- **`doctor --live`** answers the capability probes from the real backend rather
  than from recordings, mapping the corpus's model ids through the configured
  tiers and spending at the configured effort ceiling.
- **`record upstream`** captures the whole exchange — the client's request and
  the stream that answered it — which is both halves of a fixture.
- **`[instructions]`** puts operator text around the client's system prompt: a
  lead naming the model that is actually answering, on by default, and an
  optional trailer placed where an instruction outranks the prompt above it.
- **`usage`** reports the quota the backend volunteers at the start of every
  stream — free, never polled, and absent rather than zero before a turn.
- **`statusline`** wraps an existing status-line script and merges that quota
  into the payload it already reads, so the script keeps working as written.
- **WebSocket compression.** `permessage-deflate`, negotiated on the upgrade.
  Measured on the wire, one identical turn with it offered and declined: about
  65% off in both directions, 267 KB on a single first turn. The inbound half is
  the larger one, because the backend echoes the whole request back three times
  per turn. It saves bytes and **no tokens** — quota is unaffected.
- **A working budget in the instructions, on by default.** The conversation is
  replayed upstream every turn and echoed back three times, so context pulled in
  is paid for repeatedly; without a budget the window goes quickly on reads that
  changed nothing. Switch it off with `working_budget = false`.
- **Defaults for every configuration key, and the file itself is optional.** A
  missing configuration is a first run rather than a failure. All four tiers
  default; a defaulted model this account cannot see is substituted for one it
  has, and said out loud, while a model the operator stated is never touched.
- **`status` reports the plan**, preferring what the backend said on the last
  turn over the older claim in the grant, and naming which answered. It also
  names any mapped model the catalog withholds — those pass validation, so
  nothing else would mention them.
- **`[upstream]`** makes the endpoints, the reported client version, and the
  usable share of a context window configurable, so a pinned binary can be
  repointed rather than rebuilt.

### Fixed

- **A tool call forked the conversation.** Arguments were compared as text, so a
  client replaying the same object with its keys in a different order looked
  like a different call, and every turn after the model wrote a file uploaded
  the whole history again.
- **`record.start` captured nothing.** The switch it set was read by `status`
  and by nothing else.
- **Two capability probes proved nothing.** Their attachments were valid base64
  and were not a PNG or a PDF, so they passed against a recording written to
  pass them. Both now carry real files.
- **A marker split across deltas read as missing**, failing every attachment
  probe against a backend that had read the attachment and said so.
- **An opaque access token refreshed on every request**, because the response's
  own `expires_in` was ignored.
- **The WebSocket upgrade carried no `originator` or `user-agent`**, which every
  other upstream path sends. Nothing tested the upgrade's headers.
- **A compaction window outside 100,000–1,000,000 was emitted and silently
  discarded** by the client, so it was not an early compaction or a late one but
  no setting at all. It is now omitted, with a warning.
- **The effort ceiling and the window guard were inert in the shipping binary**,
  which was handed a fallback catalog while the real one went to the control
  socket.

### Added — the foundation these sit on
- **Anthropic Messages ingress** on loopback: streaming `/v1/messages`,
  `/v1/messages/count_tokens`, and `/v1/models`, with the full error vocabulary
  and cancellation propagated upstream.
- **Request and response translation** covering the capabilities Claude Code
  depends on the server for — attachments inside tool results, the server-side
  search tool and its structured results, deferred tool discovery, reasoning,
  and the context meter.
- **Both transports.** WebSocket with one connection reused per session,
  prewarm, and incremental upload; HTTP with SSE as an equal fallback that a
  session latches to rather than retrying, carrying a zstd-compressed body.
- **OAuth with PKCE**, a `CredentialStore` behind a trait with a `0600` file
  implementation, single-flight refresh, and terminal handling of a refused
  grant.
- **Token accounting** with a self-calibrating estimator, chosen over a
  tokenizer by measurement recorded in `docs/proxy-behavior.md` §6.3.
- **A control socket** carrying every CLI verb, so a second front-end needs no
  new daemon work.
- **Capability probes and `doctor`**, keyed on content a model could not infer,
  reporting a matrix that states what it was run against.

### Known limitations

Listed in [`docs/api.md`](docs/api.md) §5. The suite never touches the network,
so every claim it makes is about the proxy's own half. The backend's half was
settled separately, against a live subscription: `docs/roadmap.md` §L records
each question and its answer, including the four rules the answers falsified.

Compression applies to both transports: zstd on an HTTP body,
`permessage-deflate` on the socket. Measured on a real turn, it takes about two
thirds off the wire in each direction, and the inbound half is the larger one —
the backend echoes the whole request back three times per turn. It saves no
tokens; quota is unaffected.
