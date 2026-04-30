# sketch

A terminal markdown editor with Helix-style selections, an outline browser, and
a Claude Code MCP channel for piping buffers and inline replies into a running
`claude` session in another terminal.

## Build

```sh
cargo build --release
```

Produces two binaries:

- `target/release/sketch` — the editor
- `target/release/sketch-channel` — the Claude Code MCP channel server (only
  needed if you want the Claude integration)

Optionally symlink them onto `$PATH`:

```sh
ln -sf "$(pwd)/target/release/sketch"         ~/.local/bin/sketch
ln -sf "$(pwd)/target/release/sketch-channel" ~/.local/bin/sketch-channel
```

## Run

```sh
sketch path/to/file.md
```

Top-level modes:

- **View Mode** — rendered markdown (read-only, navigation only)
- **Edit Mode** — raw markdown source, with **Normal** and **Insert** submodes

Toggle with `:toggle-view` or via the menu (`Space → v`).

## Keybindings (Edit Mode, Normal)

Movement is Helix-flavoured: word motions create selections, char motions
collapse them. `v` toggles "extend mode" so char motions extend the selection
instead.

| Key | Action |
| --- | --- |
| `h` `j` `k` `l` | move cursor (extend in extend mode) |
| `w` `b` `e` | select to next/prev word, word end |
| `0` `$` / `gh` `gl` | line start / line end |
| `gg` `G` / `ge` | top / bottom of buffer |
| `i` `a` | insert before / after (jumps to selection start/end if active) |
| `o` `O` | open line below / above |
| `v` | toggle extend mode (`[SEL]` shown in top bar) |
| `;` | collapse selection |
| `,` | flip cursor and anchor |
| `%` | select all |
| `x` | extend selection by line (repeat extends down) |
| `d` | delete selection (or current line if none) |
| `c` | change selection (delete + insert) |
| `y` | yank selection (or current line if none) |
| `u` / `Ctrl-r` | undo / redo |
| `:` | command mode |
| `/` `?` `n` `N` | search forward / backward / next / prev |
| `Space` | open command menu |
| `Tab` / `Shift-Tab` | next / previous buffer |
| `Esc` | clear selection, exit extend mode |

## Browsers

- `:buffers` (or `Space → b`) — fullscreen buffer list
- `:file-browser-full` (or `Space → F`) — fullscreen file browser
- `:outline` (or `Space → o`) — heading outline; `l`/`h` descend/ascend levels

## Claude Code channel

`sketch` can ship buffers (or just your selection) into a `claude` session in
another terminal, and Claude can reply back into a special `*claude*` buffer.

The transport is the official Claude Code Channels API: `sketch-channel` is an
MCP server that Claude Code spawns over stdio; `sketch` connects to it via a
Unix domain socket.

### Setup

1. **Register the channel with Claude Code.** Create or edit `.mcp.json` at
   the **root of the project you'll run `claude` from** (alongside your code,
   *not* inside `.claude/`):

   ```json
   {
     "mcpServers": {
       "sketch": {
         "command": "/absolute/path/to/sketch-channel"
       }
     }
   }
   ```

   Claude Code reads `.mcp.json` from its working directory at launch, so this
   file lives next to whatever you `cd` into before running `claude`.

   Tip: run `sketch-channel --help` and it prints this snippet pre-filled with
   the path to the binary you just built.

2. **Launch claude with the channel enabled:**

   ```sh
   claude --dangerously-load-development-channels server:sketch
   ```

3. **In another terminal, attach sketch to the channel:**

   ```sh
   sketch some-notes.md
   :claude-attach
   ```

   With no argument, `:claude-attach` connects to `/tmp/sketch-channel-$USER.sock`
   (override via the `SKETCH_CHANNEL_SOCKET` environment variable). Pass an
   explicit path to use a non-default socket.

### Sending

| Command | Effect |
| --- | --- |
| `:claude-send` | ships the current buffer to Claude (or, in the `*claude*` buffer, only your inline edits — never Claude's prose) |
| `:claude-send-selection` | ships your active Helix selection |
| `:claude-status` | shows the current attachment state |
| `:claude-detach` | drops the connection |

### Inline-reply flow

When Claude replies, the text appears in a `*claude*` buffer. The buffer
visually distinguishes three regions:

- **muted foreground** — old, locked turns
- **normal text** — Claude's prose in the active turn (read-only — your
  inserts split it but you can't delete Claude's words)
- **accent background** — your inline edits (fully editable)

Move the cursor anywhere inside Claude's reply, press `i`, and start typing.
Your text appears with the accent background between Claude's paragraphs —
exactly like quoting and replying inline in an email.

Press `:claude-send`. Each contiguous run of your accent text gets joined with
`\n\n` separators and shipped — Claude's words are never echoed back. Your
turn locks (an `---` appears, dimmed), and Claude's next reply lands below it
as a fresh active region.

### Architecture

```
┌─────────┐   unix socket   ┌──────────────────┐  stdio MCP   ┌───────────┐
│ sketch  │ ───────────────▶│  sketch-channel  │ ────────────▶│   claude  │
│         │ ◀───────────────│  (mcp server)    │ ◀────────────│           │
└─────────┘   JSON lines    └──────────────────┘  JSON-RPC    └───────────┘
   :claude-send    →    {"type":"send",content,meta}
                                    ↓
                        notifications/claude/channel
                                    ↓
                    <channel source="sketch" label="buffer">…</channel>

   *claude* buffer  ←  {"type":"reply",text}  ←  reply tool  ←  Claude
```

Constraints worth knowing:

- **Single sketch ↔ single channel.** A new `:claude-attach` replaces any
  previous connection.
- **Channel reload requires restarting `claude`.** `sketch-channel` is spawned
  by Claude Code at session start; the Unix socket is bound at that point.
- **Meta keys must be alphanumeric or underscore** (a Claude Code constraint).
  `sketch-channel` silently drops anything else from the `meta` object before
  forwarding.

## Configuration

Sketch reads `~/.config/sketch/config.kdl` (if present) for theme, max line
width, custom keybindings, and a custom menu tree. See `src/config.rs` for the
schema.

## Testing

```sh
cargo test
```

Covers ~170 unit + integration tests, including the MCP handshake against the
real `sketch-channel` binary and the inline-reply data model.
