## Definitions

- yaldabaoth: name of the app. FKA: sketch
- Tilebar: Like a title bar, but for my app

### App
- Should be a formal concept in our Rust object model
- Interacts with the yalda tile desktop
- Possibly modeled as an actor that can receive messages from desktop events. Ex:
  - Moved to new workspace

## Tile
- Space occupied by an instance of an app
- Is simply the visual container for an app
- An app instance can be in only one tile at a time
- A tile may contain only one app instance
- An app can have no tile. An app may be running with no visual representation.

## Tilebars
- Space at the top of the tile aka titlebar
- Indicate the app type contained in the tile
- App sets the name of the tile
- Indicates whether it is selected/active
- Acts as a place to grab and move the tile around if you are using a mouse

## Yalda aka Global Scope
### Owns
- A set of workspaces

### Commands

- Leader key is ?
- Number (1..0) will open that workspace. The workspaces are listed in the menu that pops up IF they are inhabited by ANY tiles OR named.
- Name workspace. Names the current workspace. That name appears besides the number in the menu. 
- I think that is about it.


## Workspace

Each workspace has a name and a number index. If no name is given, they default to 'ws-{num}' where {num} matches its index.


### Owns
- A set of tiles
- Layout of those tiles
- Key value registry that can be access by apps

### Behaviors:
- All apps in workspace get notified of kv registrychanges

### Commands ( 12 jun )
Default command leader key: <space> 
Only these commands should be in the workspace:
- Set CWD. Should be implemented as a kv using the registry 
- New
  - Agent
  - Buffer
- Theme
 - Nightfox
 - Folio
- Rebuild and Restart GUI 
- Mark tile 
- Close tile


### Workspace TODO
- [x] Implement Workspace KV (`Tab::kv` registry + get/set/remove; persisted in `PersistedTab.kv`; apps observe via render. branch workspace-agent-todos)
- [x] Implement Workspace Commands (workspace menu `s c` "set cwd" → path-input overlay resolves+validates a dir and writes `kv["cwd"]`. branch workspace-agent-todos)
  - [x] Redo this with updated command list ( 12 jun ) — `gpui_menu()` pruned to set-cwd / new (agent,buffer) / theme (nightfox,folio) / rebuild+restart / mark-tile; `mark-tile` command starts the set-mark chord. Tests updated.
- [x] Mouse click into a window focuses it (multi-leaf tiles wrap with an `on_mouse_down` that focuses the clicked window; bubble phase so editor caret still places first. Runtime-confirm. branch workspace-agent-todos)

## General Menus and commands
- Different scopes: yaldaboath, workspace, app
- Commands can be sent via access through a menu or via a keybinding
- Menus and keybindgs are just different paths to executing the same exact commands
- Menus are always constrained to a scope. 
  - If . opens an app menu, only command specific to the focused app can be accessed from that menu. 
  - If <space> opens a workspace menu, similarly only commands specific to that workspace can be accessed there.

## App :: Agent

### UX
- [x] Has a Context Window full progress bar + indicator of how many tokens used (ctx fullness bar + `used/total (pct)` numeric in the agent info bar; root blocker was `unstable_session_usage` being off + bit-rotted — fixed the `Cost.amount` field rename and made the feature default-on so usage actually flows. Runtime-confirm the agent emits usage. branch agent-context-bar)
- Allows you to view subagents and toggle between them

### Behavior
- Inhereits the CWD from the workspace

### Commands
- Close
- clear
- select session
- send message
- Set permission
- Toggle worksheet/command  
- Rename

#### Commands to delete
- [x] Remove detatch (off the `.` menu + dispatch; method kept #[allow(dead_code)]. branch workspace-agent-todos)
- [x] Remove attach (off the `.` menu + dispatch; method kept #[allow(dead_code)]. branch workspace-agent-todos)

### Agent TODO
- [x] Implement commands (target `.`-menu set — select/send/stop/toggle/new/close/clear/rename/send-selection/permission — already present; this batch removed detach/attach per "Commands to delete". branch workspace-agent-todos)
- [x] Implement automatically inherit CWD from workspace. If none, inherit CWD from app (`new_agent_session` default: explicit arg → `active_workspace_cwd()` (workspace `kv["cwd"]`) → `process_cwd`. headless test `workspace_kv_cwd_inheritance`. branch workspace-agent-todos)

## App :: Buffer

### Buffer TODO
- [x] Selected text in a buffer looks ugly with the folio theme. (edit views now use the theme's selection bg, not hardcoded Dracula)
- [x] BUG:you can still move the cursor offscreen (esp when using o or i, changing between modes) and it's really fucking annoying. suspect this is because visible cursor position is not the same as edit cursor (root cause was the non-wrapping line pushing the inline caret off the right edge — same as the long-line bug below; fixed by Wordwrap: the caret is now an inline run that wraps with the text. Runtime-confirm. branch buffer-wordwrap)
- [x] no 'r' replace char key in normal mode (`r{char}` replaces char under cursor, single undo step, stays in normal mode; branch buffer-todos-2)
- [x] deleting text puts it in the yank buffer, vim style (d/c/x/dd copy deleted text to the clipboard yank buffer before deleting, so `p` puts it back; branch buffer-todos-2)
- [x] visual mode does not highlight lines with only whitespace chars (blank/whitespace-only lines inside a selection now get a highlighted placeholder; `apply_line_selection` in render_blocks.rs; branch buffer-todos-2)
- [x] Buffers have no paste. You can yank, but you can't put. (p/P normal-mode put, branch buffer-todos)
- [x] Better TODO handling. Hitting enter after a line like this should auto generate the next TODO. (Enter continues `- [ ]`/`- `/`* `/`+ `/`N.`/`> `; empty item ends the list)
- [x] BUG: Your cursor can go off screen (root cause = no soft-wrap + no horizontal scroll on long lines; fixed by Wordwrap below — lines soft-wrap so the caret stays on-screen; `overflow_x_hidden` kills horizontal scroll. branch buffer-wordwrap)
- [x] No redo? There is undo (already worked — bound to Ctrl-R; redo stack in document.rs)
- [x] Wordwrap in buffers (both Code + WP edit views now route through `build_wrapped_line` — flex_wrap tokens with the caret as an inline run; `ListSizingBehavior::Auto` gives variable-height rows; `overflow_x_hidden` clips the rare unbroken token. Subsumes both cursor-off-screen bugs. branch buffer-wordwrap)
- [x] <num>g does not work to jump to lines (count prefix + <num>g/<num>G, branch buffer-todos)
- [x] want to be able to rename files in the file browser (`r` in the Cmd+O browser → inline rename, Enter commits via fs::rename, Esc cancels; rail browser not yet wired)
- [x] ctrl-d/ctrl-u do not work to page up and down (also ctrl-f/ctrl-b, branch buffer-todos)







