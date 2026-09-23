//! The documents that tell somebody what to type, held to the CLI they
//! describe.
//!
//! A verb renamed in `cli.rs` and not in a document leaves it telling somebody
//! to type something the CLI refuses. That reads perfectly and fails only when
//! used, so it is checked here rather than remembered. `CHANGELOG.md` is left
//! out on purpose: it records what the CLI was.

use crate::cli::Cli;
use clap::CommandFactory;
use clap::Parser;
use std::path::Path;
use std::path::PathBuf;

/// Documents a person or an agent types commands from.
const SPEAKING: [&str; 9] = [
    "README.md",
    "CONTRIBUTING.md",
    "CLAUDE.md",
    "skills/proxenos/SKILL.md",
    "herdr-plugin/README.md",
    "docs/api.md",
    "docs/proxy-behavior.md",
    "docs/roadmap.md",
    "herdr-plugin/popup-usage.sh",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

/// What a placeholder stands for, so the line can be parsed as typed.
fn stand_in(word: &str) -> Option<&'static str> {
    Some(match word {
        "NAME" | "ACCOUNT" | "OLD" | "<account>" | "<name>" => "work",
        "NEW" => "spare",
        "MODEL" | "<model>" => "gpt-6-sol",
        "TIER" | "<tier>" => "opus",
        "N" => "8787",
        "DIR" | "PATH" => "/tmp",
        "PID" | "<pid>" => "1",
        "X.Y.Z" => "1.2.3",
        "LEVEL" | "<effort>" => "high",
        "PROGRAM" => "claude",
        _ => return None,
    })
}

/// One documented line as a shell would split it, placeholders filled in.
/// `[...]` is optional, so the shortest form is what is checked; `a|b` is
/// checked as its first word.
fn argv_of(line: &str) -> Vec<String> {
    let line = line.split(" #").next().unwrap_or(line);
    // A synopsis line describes itself after a run of spaces.
    let line = line.split("   ").next().unwrap_or(line);
    let mut stripped = String::new();
    let mut depth = 0_u32;
    for character in line.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => stripped.push(character),
            _ => {}
        }
    }

    let mut words = Vec::new();
    let mut quoted: Option<String> = None;
    for word in stripped.split_whitespace() {
        if let Some(open) = quoted.as_mut() {
            open.push(' ');
            open.push_str(word);
            if word.ends_with('"') || word.ends_with('\'') {
                words.push(open.trim_matches(['"', '\'']).to_owned());
                quoted = None;
            }
            continue;
        }
        if (word.starts_with('"') || word.starts_with('\''))
            && !(word.len() > 1 && (word.ends_with('"') || word.ends_with('\'')))
        {
            quoted = Some(word.to_owned());
            continue;
        }
        // The rest is the shell's: a redirection, a pipe, or a list of more.
        if matches!(word, "<" | ">" | "|" | "...") || word.ends_with("...") {
            break;
        }
        let word = word.split('|').next().unwrap_or(word);
        let word = stand_in(word).map_or_else(|| word.to_owned(), str::to_owned);
        words.push(word.trim_matches(['"', '\'']).to_owned());
    }
    words
}

/// Every `proxenos …` a document tells somebody to type: a line in a code
/// block, and a span in backticks in the prose.
fn commands_in(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
            continue;
        }
        let trimmed = line.trim();
        if in_block && trimmed.starts_with("proxenos ") {
            found.push(trimmed.to_owned());
        }
    }
    for span in text.split('`').skip(1).step_by(2) {
        if span.starts_with("proxenos ") {
            found.push(span.to_owned());
        }
    }
    found
        .iter()
        .flat_map(|line| line.split("&&"))
        .map(str::trim)
        .filter(|line| {
            line.strip_prefix("proxenos ")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|first| first.is_ascii_lowercase() || first == '-')
        })
        .map(str::to_owned)
        .collect()
}

/// What the CLI refuses among one document's commands, and how many it read.
fn refusals_in(doc: &str, text: &str) -> (Vec<String>, usize) {
    let verbs = Cli::command();
    let mut refused = Vec::new();
    let commands = commands_in(text);
    let read = commands.len();

    {
        for line in commands {
            let argv = argv_of(&line);
            // `proxenos status` named in a sentence is a mention of the verb.
            // What is worth checking there is that the CLI still has it.
            if argv.len() == 2 {
                let verb = argv.get(1).map(String::as_str).unwrap_or_default();
                if verb.starts_with('-') || verbs.find_subcommand(verb).is_some() {
                    continue;
                }
                refused.push(format!("{doc}: `{line}`: no verb `{verb}`"));
                continue;
            }
            if let Err(error) = Cli::try_parse_from(&argv) {
                use clap::error::ErrorKind;
                if !matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
                ) {
                    let first = error.to_string();
                    let first = first.lines().next().unwrap_or_default().to_owned();
                    refused.push(format!("{doc}: `{line}`: {first}"));
                }
            }
        }
    }
    (refused, read)
}

#[test]
fn every_command_the_documents_print_is_one_the_cli_takes() {
    let mut refused = Vec::new();
    let mut read_in_all = 0;
    for doc in SPEAKING {
        let (found, read_here) = refusals_in(doc, &read(doc));
        refused.extend(found);
        read_in_all += read_here;
    }

    assert!(refused.is_empty(), "\n{}", refused.join("\n"));
    // A reader that finds nothing passes every document. The documents quote
    // about ninety commands; far fewer means the reader broke.
    assert!(read_in_all > 60, "only {read_in_all} commands were read");
}

/// The check fails on what it exists to catch: a verb that is gone, a flag
/// that is gone, in a block and in the prose.
#[test]
fn a_command_the_cli_refuses_is_reported() {
    let drifted = "```\nproxenos stauts\nproxenos tiers set opus MODEL --bogus\n```\n\
                   Run `proxenos models --acount work` first.\n\
                   ```\nproxenos tiers set TIER MODEL --persist   point one tier\n```\n";

    let (refused, read) = refusals_in("drifted.md", drifted);

    assert_eq!(read, 4);
    assert_eq!(refused.len(), 3, "{refused:#?}");
}

/// Every verb has a line in the synopsis the API document opens its command
/// line section with, which is where a reader looks for what exists.
#[test]
fn every_verb_is_in_the_command_line_synopsis() {
    let api = read("docs/api.md");
    let synopsis = api
        .split("## 2. Command line")
        .nth(1)
        .and_then(|section| section.split("```").nth(1))
        .expect("docs/api.md §2 opens with a synopsis block");

    let missing: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|verb| verb.get_name().to_owned())
        .filter(|verb| verb != "help")
        .filter(|verb| !synopsis.contains(&format!("proxenos {verb}")))
        .collect();

    assert!(missing.is_empty(), "not in docs/api.md §2: {missing:?}");
}

/// A path `CLAUDE.md` names is one a session reads first, so a path that moved
/// sends it somewhere that is not there.
#[test]
fn every_path_claude_md_names_exists() {
    let claude = read("CLAUDE.md");
    let bases = [
        root(),
        root().join("crates/core/src"),
        root().join("crates/proxy/src"),
    ];
    // The map names a file under the directory its bullet names, so a bare
    // name is found wherever it lives in the sources or the docs.
    let mut names = std::collections::BTreeSet::new();
    let mut pending = vec![root().join("crates"), root().join("docs")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                names.insert(name.to_owned());
            }
        }
    }

    let missing: Vec<&str> = claude
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| !span.contains(' ') && !span.contains('<'))
        .filter(|span| {
            span.ends_with(".rs")
                || span.ends_with(".md")
                || span.ends_with(".toml")
                || span.ends_with(".sh")
                || (span.ends_with('/') && span.len() > 1)
        })
        .filter(|span| {
            let bare = !span.trim_end_matches('/').contains('/') && !span.ends_with('/');
            !(bases.iter().any(|base| base.join(span).exists()) || bare && names.contains(*span))
        })
        .collect();

    assert!(
        missing.is_empty(),
        "CLAUDE.md names paths that do not exist: {missing:?}"
    );
}
