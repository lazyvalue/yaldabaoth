## Definitions

- yaldabaoth: name of the app. FKA: yalda
- Tilebar: Like a title bar, but for my app

## Components

### App
- Should be a formal concept in our Rust object model
- Interacts with the yalda tile desktop
- Possibly modeled as an actor that can receive messages from desktop events. Ex:
  - Moved to new workspace

### Tile
- Space occupied by an instance of an app
- Is simply the visual container for an app
- An app instance can be in only one tile at a time
- A tile may contain only one app instance
- An app can have no tile. An app may be running with no visual representation.

### Tilebars
- Space at the top of the tile aka titlebar
- Indicate the app type contained in the tile
- App sets the name of the tile
- Indicates whether it is selected/active
- Acts as a place to grab and move the tile around if you are using a mouse

### Workspace
- Contains a set of tiles
- Contains one layout
- Has a key value registry that can be accessed by apps

### Menus and commands
- Different scopes: yaldaboath, workspace, app
- Commands can be sent via access through a menu or via a keybinding
- Menus and keybindgs are just different paths to executing the same exact commands
- Menus are always constrained to a scope. 
  - If . opens an app menu, only command specific to the focused app can be accessed from that menu. 
  - If <space> opens a workspace menu, similarly only commands specific to that workspace can be accessed there.

## App :: Agent



## App :: Buffer

## Features

### General Buffer TODO
- [x] Selected text in a buffer looks ugly with the folio theme. (edit views now use the theme's selection bg, not hardcoded Dracula)
- [x] Buffers have no paste. You can yank, but you can't put. (p/P normal-mode put, branch buffer-todos)
- [x] Better TODO handling. Hitting enter after a line like this should auto generate the next TODO. (Enter continues `- [ ]`/`- `/`* `/`+ `/`N.`/`> `; empty item ends the list)
- [ ] BUG: Your cursor can go off screen (root cause = no soft-wrap + no horizontal scroll on long lines; fix is coupled to Wordwrap below)
- [x] No redo? There is undo (already worked — bound to Ctrl-R; redo stack in document.rs)
- [ ] Wordwrap in buffers (deferred: render-architecture change — cursor line is a non-wrapping flex_row of [before][caret-div][after]; needs caret-as-inline-run + variable-height rows. Subsumes the cursor-off-screen bug.)
- [x] <num>g does not work to jump to lines (count prefix + <num>g/<num>G, branch buffer-todos)
- [x] want to be able to rename files in the file browser (`r` in the Cmd+O browser → inline rename, Enter commits via fs::rename, Esc cancels; rail browser not yet wired)
- [x] ctrl-d/ctrl-u do not work to page up and down (also ctrl-f/ctrl-b, branch buffer-todos)







