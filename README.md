# Prompt Deck

[![CI](https://github.com/matdac12/herdr-prompt-deck/actions/workflows/ci.yml/badge.svg)](https://github.com/matdac12/herdr-prompt-deck/actions/workflows/ci.yml)

A bottom prompt bar for [Herdr](https://herdr.dev) — insert file paths, saved
snippets, and scratch text into the focused coding agent **without submitting**.

Claude Code, OpenCode, Codex, or anything else in a Herdr pane: Prompt Deck sits
at the bottom of the tab, you drop things into the prompt, and you keep working.

```
  1 Files   2 Snippets   3 Editor                 target: claude
  filter  rev
    review-diff    Review the current diff and list only actionable issues.
    fix-tests      Run the test suite, find failures, and fix them.
  type filter   ↑↓ select   ↵ insert   ctrl+n new   esc close
```

## What it does

- **Files** — opens your real OS file dialog, then inserts the file's absolute path.
- **Snippets** — a searchable library of prompts you reuse, stored in a plain
  `snippets.toml` you can also edit by hand.
- **Editor** — compose free text and insert it. *(coming soon)*

Everything is inserted into the agent's input, never submitted, so you can stack
a file path and two snippets and then hit Enter yourself.

## Requirements

- Herdr **0.8.0** or newer

The installer downloads a prebuilt binary for your OS and verifies its SHA-256, so
you do **not** need Rust. If no prebuilt matches your platform and version, it
falls back to building from source, which needs a Rust toolchain.

## Install

```sh
herdr plugin install matdac12/herdr-prompt-deck
```

Then bind a key in your Herdr `config.toml`.

**Windows** (action ids carry a `-windows` suffix):

```toml
[[keys.command]]
key = "prefix+p"
type = "plugin_action"
command = "prompt-deck.toggle-windows"
description = "toggle prompt deck"
```

**macOS / Linux**:

```toml
[[keys.command]]
key = "prefix+p"
type = "plugin_action"
command = "prompt-deck.toggle"
description = "toggle prompt deck"
```

> `prefix+p` is Herdr's default for *previous tab*. Pick a different key if you
> use that binding.

Reload the config with `prefix+shift+r`, or run `herdr server reload-config`.

## Usage

| Key | Action |
| --- | --- |
| `1` `2` `3` | switch tool (also `tab` / `shift+tab`) |
| `esc` | close the bar and return focus to the agent |

**Files**

| Key | Action |
| --- | --- |
| `b` | open the OS file dialog, insert the absolute path |

**Snippets**

| Key | Action |
| --- | --- |
| *(type)* | filter by name or text |
| `↑` `↓` | move selection |
| `↵` | insert the selected snippet |
| `ctrl+n` | new snippet |
| `ctrl+e` | edit the selected snippet |
| `ctrl+d` | delete the selected snippet |
| `tab` *(in the form)* | switch between name and text |
| `ctrl+s` *(in the form)* | save |

The bar opens as a slim split at the bottom of the current pane, targets the
agent you were last focused on, and gets out of the way when you press `esc`.

## Snippets file

Snippets live in Herdr's plugin config directory:

- **Windows**: `%APPDATA%\herdr\plugins\config\prompt-deck\snippets.toml`
- **macOS / Linux**: `~/.config/herdr/plugins/config/prompt-deck/snippets.toml`

```toml
[[snippet]]
name = "review-diff"
text = "Review the current diff and list only actionable issues."

[[snippet]]
name = "fix-tests"
text = "Run the test suite, find failures, and fix them."
```

The file is rewritten when you add, edit, or delete from the Snippets tab, so
keep it as the single source of truth rather than editing it while the bar is open.

## Development

```sh
git clone https://github.com/matdac12/herdr-prompt-deck
cd herdr-prompt-deck
cargo build --release
herdr plugin link .
```

`herdr plugin link` registers the working directory as a local plugin (it does
not run the build for you). After changing code, `cargo build --release` and the
next toggle picks up the new binary.

## Releases

Push a tag matching the version in `Cargo.toml` and `herdr-plugin.toml`
(e.g. `v0.1.0`) and the release workflow builds binaries for macOS
(arm64 + x86_64), Linux (x86_64, static musl), and Windows (x86_64) and
publishes them with a `SHA256SUMS` file. The installer picks the right asset.

## Platform support

Developed and tested on **Windows**. macOS and Linux binaries are built in CI and
the launcher logic is shared Rust code, but the Unix path has not yet been
exercised against a live Herdr server — reports and fixes welcome.

## License

[MIT](LICENSE)
