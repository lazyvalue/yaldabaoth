# Yalda color scheme — full dump

Every color literal in `src/theme.rs`, per theme, extracted straight from source (so it stays exact). Regenerate with `python3 docs/reference/dump_theme.py` if the themes change.

Three layers per theme: **Editor/document** (`Theme` — markdown render + chrome, colors decimal in source), **Agent** (`AgentTheme` — the Claude chat surface), **Overlay** (`OverlayTheme` — menus/pickers). `fg`/`bg` marks the role within a `Style`; a blank role means the field *is* that color.

Themes: Dracula (default dark), Nightfox, Solarized Light, Solarized Dark, Gruvbox Dark, Financial Times (light), Financial Times Dark, Folio.


## Dracula (default dark)


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#BD93F9` |
| `heading[1]` | fg | `#8BE9FD` |
| `heading[2]` | fg | `#50FA7B` |
| `heading[3]` | fg | `#F1FA8C` |
| `heading[4]` | fg | `#FFB86C` |
| `heading[5]` | fg | `#B4B4B4` |
| `paragraph` | fg | `#CCCCCC` |
| `bold` | fg | `#F8F8F2` |
| `italic` | fg | `#F8F8F2` |
| `strikethrough` | fg | `#888888` |
| `code_inline[0]` | fg | `#F1FA8C` |
| `code_inline[1]` | bg | `#282A36` |
| `code_block_bg` | bg | `#282A36` |
| `blockquote_bar` | fg | `#FFB86C` |
| `blockquote_text` | fg | `#AAAAAA` |
| `link` | fg | `#8BE9FD` |
| `table_border` | fg | `#6272A4` |
| `table_header` | fg | `#BD93F9` |
| `horizontal_rule` | fg | `#6272A4` |
| `list_marker` | fg | `#50FA7B` |
| `image_label` | fg | `#FFB86C` |
| `cursor_line` | bg | `#282A36` |
| `top_bar[0]` | fg | `#8BE9FD` |
| `top_bar[1]` | bg | `#16213E` |
| `top_bar_mode[0]` | fg | `#50FA7B` |
| `top_bar_mode[1]` | bg | `#16213E` |
| `bottom_bar[0]` | fg | `#666666` |
| `bottom_bar[1]` | bg | `#16213E` |
| `mode_indicator` | fg | `#50FA7B` |
| `search_match[0]` | fg | `#282A36` |
| `search_match[1]` | bg | `#6272A4` |
| `search_match_current[0]` | fg | `#282A36` |
| `search_match_current[1]` | bg | `#50FA7B` |
| `midpoint_marker` | fg | `#6272A4` |
| `line_number` | fg | `#6272A4` |
| `line_number_current` | fg | `#F8F8F2` |
| `editor_bg` |  | `#282A36` |
| `editor_fg` |  | `#F8F8F2` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#8BE9FD` |
| `user_bar` |  | `#50FA7B` |
| `tool_label` |  | `#F1FA8C` |
| `dim` |  | `#6272A4` |
| `agent_tint` |  | `#A9D0E0` |
| `user_tint` |  | `#B8E09A` |
| `frozen_fg` |  | `#B6C4D6` |
| `agent_turn_bg` |  | `#232736` |
| `user_turn_bg` |  | `#282E3A` |
| `turn_header_agent` |  | `#8BE9FD` |
| `turn_header_user` |  | `#50FA7B` |
| `turn_rule` |  | `#44475A` |
| `tool_card_bg` |  | `#21222C` |
| `tool_card_border` |  | `#44475A` |
| `tool_completed` |  | `#50FA7B` |
| `tool_in_progress` |  | `#F1FA8C` |
| `tool_failed` |  | `#FF5555` |
| `tool_pending` |  | `#6272A4` |
| `tool_body_bg` |  | `#1E1F29` |
| `tool_output_bg` |  | `#282A36` |
| `tool_body_fg` |  | `#BFBFBF` |
| `diff_add` |  | `#50FA7B` |
| `diff_remove` |  | `#FF5555` |
| `diff_header` |  | `#BD93F9` |
| `selection_bg` |  | `#44475A` |
| `compose_separator` |  | `#6272A4` |
| `cursor` |  | `#FF5555` |
| `sidebar_bg` |  | `#21222C` |
| `sidebar_border` |  | `#44475A` |
| `sidebar_header` |  | `#8BE9FD` |
| `warm_accent` |  | `#F1FA8C` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#1E1E3A` |
| `border` |  | `#383A4F` |
| `label` |  | `#6272A4` |
| `fg` |  | `#CCCCCC` |
| `key` |  | `#BD93F9` |
| `accent` |  | `#8BE9FD` |
| `selected_bg` |  | `#383A4F` |
| `modified` |  | `#FFB86C` |
| `input` |  | `#F1FA8C` |


## Nightfox


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#719CD6` |
| `heading[1]` | fg | `#9D79D6` |
| `heading[2]` | fg | `#81B29A` |
| `heading[3]` | fg | `#DBC074` |
| `heading[4]` | fg | `#F4A261` |
| `heading[5]` | fg | `#738091` |
| `paragraph` | fg | `#CDCECF` |
| `bold` | fg | `#D6D6D7` |
| `italic` | fg | `#D6D6D7` |
| `strikethrough` | fg | `#71839B` |
| `code_inline[0]` | fg | `#DBC074` |
| `code_inline[1]` | bg | `#212E3F` |
| `code_block_bg` | bg | `#212E3F` |
| `blockquote_bar` | fg | `#F4A261` |
| `blockquote_text` | fg | `#AEAFB0` |
| `link` | fg | `#63CDCF` |
| `table_border` | fg | `#39506D` |
| `table_header` | fg | `#719CD6` |
| `horizontal_rule` | fg | `#39506D` |
| `list_marker` | fg | `#81B29A` |
| `image_label` | fg | `#D67AD2` |
| `cursor_line` | bg | `#29394F` |
| `top_bar[0]` | fg | `#63CDCF` |
| `top_bar[1]` | bg | `#131A24` |
| `top_bar_mode[0]` | fg | `#A9DC76` |
| `top_bar_mode[1]` | bg | `#131A24` |
| `bottom_bar[0]` | fg | `#71839B` |
| `bottom_bar[1]` | bg | `#131A24` |
| `mode_indicator` | fg | `#81B29A` |
| `search_match[0]` | fg | `#CDCECF` |
| `search_match[1]` | bg | `#39506D` |
| `search_match_current[0]` | fg | `#192330` |
| `search_match_current[1]` | bg | `#81B29A` |
| `midpoint_marker` | fg | `#39506D` |
| `line_number` | fg | `#39506D` |
| `line_number_current` | fg | `#D8DEE9` |
| `editor_bg` |  | `#131A24` |
| `editor_fg` |  | `#CDCECF` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#63CDCF` |
| `user_bar` |  | `#81B29A` |
| `tool_label` |  | `#DBC074` |
| `dim` |  | `#39506D` |
| `agent_tint` |  | `#9ABED0` |
| `user_tint` |  | `#A3C9B3` |
| `frozen_fg` |  | `#A0B4C8` |
| `agent_turn_bg` |  | `#161F2D` |
| `user_turn_bg` |  | `#1C2230` |
| `turn_header_agent` |  | `#63CDCF` |
| `turn_header_user` |  | `#81B29A` |
| `turn_rule` |  | `#2B3B51` |
| `tool_card_bg` |  | `#171E28` |
| `tool_card_border` |  | `#2B3B51` |
| `tool_completed` |  | `#81B29A` |
| `tool_in_progress` |  | `#DBC074` |
| `tool_failed` |  | `#C94F6D` |
| `tool_pending` |  | `#39506D` |
| `tool_body_bg` |  | `#141B25` |
| `tool_output_bg` |  | `#192330` |
| `tool_body_fg` |  | `#AEAFB0` |
| `diff_add` |  | `#81B29A` |
| `diff_remove` |  | `#C94F6D` |
| `diff_header` |  | `#9D79D6` |
| `selection_bg` |  | `#2B3B51` |
| `compose_separator` |  | `#39506D` |
| `cursor` |  | `#C94F6D` |
| `sidebar_bg` |  | `#171E28` |
| `sidebar_border` |  | `#2B3B51` |
| `sidebar_header` |  | `#63CDCF` |
| `warm_accent` |  | `#DBC074` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#141B25` |
| `border` |  | `#2B3B51` |
| `label` |  | `#71839B` |
| `fg` |  | `#CDCECF` |
| `key` |  | `#9D79D6` |
| `accent` |  | `#63CDCF` |
| `selected_bg` |  | `#2B3B51` |
| `modified` |  | `#DBC074` |
| `input` |  | `#DBC074` |


## Solarized Light


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#CB4B16` |
| `heading[1]` | fg | `#D33682` |
| `heading[2]` | fg | `#6C71C4` |
| `heading[3]` | fg | `#268BD2` |
| `heading[4]` | fg | `#2AA198` |
| `heading[5]` | fg | `#859900` |
| `paragraph` | fg | `#586E75` |
| `bold` | fg | `#002B36` |
| `italic` | fg | `#586E75` |
| `strikethrough` | fg | `#93A1A1` |
| `code_inline[0]` | fg | `#B58900` |
| `code_inline[1]` | bg | `#EEE8D5` |
| `code_block_bg` | bg | `#EEE8D5` |
| `blockquote_bar` | fg | `#CB4B16` |
| `blockquote_text` | fg | `#657B83` |
| `link` | fg | `#268BD2` |
| `table_border` | fg | `#93A1A1` |
| `table_header` | fg | `#6C71C4` |
| `horizontal_rule` | fg | `#93A1A1` |
| `list_marker` | fg | `#859900` |
| `image_label` | fg | `#CB4B16` |
| `cursor_line` | bg | `#EEE8D5` |
| `top_bar[0]` | fg | `#073642` |
| `top_bar[1]` | bg | `#EEE8D5` |
| `top_bar_mode[0]` | fg | `#859900` |
| `top_bar_mode[1]` | bg | `#EEE8D5` |
| `bottom_bar[0]` | fg | `#839496` |
| `bottom_bar[1]` | bg | `#EEE8D5` |
| `mode_indicator` | fg | `#859900` |
| `search_match[0]` | fg | `#FDF6E3` |
| `search_match[1]` | bg | `#93A1A1` |
| `search_match_current[0]` | fg | `#FDF6E3` |
| `search_match_current[1]` | bg | `#B58900` |
| `midpoint_marker` | fg | `#93A1A1` |
| `line_number` | fg | `#93A1A1` |
| `line_number_current` | fg | `#002B36` |
| `editor_bg` |  | `#FDF6E3` |
| `editor_fg` |  | `#586E75` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#268BD2` |
| `user_bar` |  | `#859900` |
| `tool_label` |  | `#B58900` |
| `dim` |  | `#93A1A1` |
| `agent_tint` |  | `#3B6E8C` |
| `user_tint` |  | `#5A6E20` |
| `frozen_fg` |  | `#47606E` |
| `agent_turn_bg` |  | `#F0EBDD` |
| `user_turn_bg` |  | `#DDE4EE` |
| `turn_header_agent` |  | `#268BD2` |
| `turn_header_user` |  | `#859900` |
| `turn_rule` |  | `#D3CCBC` |
| `tool_card_bg` |  | `#EEE8D5` |
| `tool_card_border` |  | `#D3CCBC` |
| `tool_completed` |  | `#859900` |
| `tool_in_progress` |  | `#B58900` |
| `tool_failed` |  | `#DC322F` |
| `tool_pending` |  | `#93A1A1` |
| `tool_body_bg` |  | `#EEE8D5` |
| `tool_output_bg` |  | `#FDF6E3` |
| `tool_body_fg` |  | `#657B83` |
| `diff_add` |  | `#859900` |
| `diff_remove` |  | `#DC322F` |
| `diff_header` |  | `#6C71C4` |
| `selection_bg` |  | `#D3CCBC` |
| `compose_separator` |  | `#93A1A1` |
| `cursor` |  | `#DC322F` |
| `sidebar_bg` |  | `#EEE8D5` |
| `sidebar_border` |  | `#D3CCBC` |
| `sidebar_header` |  | `#268BD2` |
| `warm_accent` |  | `#B58900` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#EEE8D5` |
| `border` |  | `#D3CCBC` |
| `label` |  | `#93A1A1` |
| `fg` |  | `#586E75` |
| `key` |  | `#6C71C4` |
| `accent` |  | `#268BD2` |
| `selected_bg` |  | `#D3CCBC` |
| `modified` |  | `#B58900` |
| `input` |  | `#CB4B16` |


## Solarized Dark


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#CB4B16` |
| `heading[1]` | fg | `#D33682` |
| `heading[2]` | fg | `#6C71C4` |
| `heading[3]` | fg | `#268BD2` |
| `heading[4]` | fg | `#2AA198` |
| `heading[5]` | fg | `#859900` |
| `paragraph` | fg | `#839496` |
| `bold` | fg | `#FDF6E3` |
| `italic` | fg | `#93A1A1` |
| `strikethrough` | fg | `#586E75` |
| `code_inline[0]` | fg | `#B58900` |
| `code_inline[1]` | bg | `#073642` |
| `code_block_bg` | bg | `#073642` |
| `blockquote_bar` | fg | `#CB4B16` |
| `blockquote_text` | fg | `#93A1A1` |
| `link` | fg | `#268BD2` |
| `table_border` | fg | `#586E75` |
| `table_header` | fg | `#6C71C4` |
| `horizontal_rule` | fg | `#586E75` |
| `list_marker` | fg | `#859900` |
| `image_label` | fg | `#CB4B16` |
| `cursor_line` | bg | `#073642` |
| `top_bar[0]` | fg | `#FDF6E3` |
| `top_bar[1]` | bg | `#073642` |
| `top_bar_mode[0]` | fg | `#859900` |
| `top_bar_mode[1]` | bg | `#073642` |
| `bottom_bar[0]` | fg | `#657B83` |
| `bottom_bar[1]` | bg | `#073642` |
| `mode_indicator` | fg | `#859900` |
| `search_match[0]` | fg | `#FDF6E3` |
| `search_match[1]` | bg | `#586E75` |
| `search_match_current[0]` | fg | `#002B36` |
| `search_match_current[1]` | bg | `#B58900` |
| `midpoint_marker` | fg | `#586E75` |
| `line_number` | fg | `#586E75` |
| `line_number_current` | fg | `#FDF6E3` |
| `editor_bg` |  | `#002B36` |
| `editor_fg` |  | `#839496` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#268BD2` |
| `user_bar` |  | `#859900` |
| `tool_label` |  | `#B58900` |
| `dim` |  | `#586E75` |
| `agent_tint` |  | `#6E9EB5` |
| `user_tint` |  | `#8A9E50` |
| `frozen_fg` |  | `#788E96` |
| `agent_turn_bg` |  | `#02303C` |
| `user_turn_bg` |  | `#0A3040` |
| `turn_header_agent` |  | `#268BD2` |
| `turn_header_user` |  | `#859900` |
| `turn_rule` |  | `#07424E` |
| `tool_card_bg` |  | `#042D38` |
| `tool_card_border` |  | `#07424E` |
| `tool_completed` |  | `#859900` |
| `tool_in_progress` |  | `#B58900` |
| `tool_failed` |  | `#DC322F` |
| `tool_pending` |  | `#586E75` |
| `tool_body_bg` |  | `#012732` |
| `tool_output_bg` |  | `#002B36` |
| `tool_body_fg` |  | `#839496` |
| `diff_add` |  | `#859900` |
| `diff_remove` |  | `#DC322F` |
| `diff_header` |  | `#6C71C4` |
| `selection_bg` |  | `#07424E` |
| `compose_separator` |  | `#586E75` |
| `cursor` |  | `#DC322F` |
| `sidebar_bg` |  | `#042D38` |
| `sidebar_border` |  | `#07424E` |
| `sidebar_header` |  | `#268BD2` |
| `warm_accent` |  | `#B58900` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#012732` |
| `border` |  | `#07424E` |
| `label` |  | `#586E75` |
| `fg` |  | `#839496` |
| `key` |  | `#6C71C4` |
| `accent` |  | `#268BD2` |
| `selected_bg` |  | `#07424E` |
| `modified` |  | `#B58900` |
| `input` |  | `#CB4B16` |


## Gruvbox Dark


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#FABD2F` |
| `heading[1]` | fg | `#FE8019` |
| `heading[2]` | fg | `#B8BB26` |
| `heading[3]` | fg | `#8EC07C` |
| `heading[4]` | fg | `#83A598` |
| `heading[5]` | fg | `#D3869B` |
| `paragraph` | fg | `#EBDBB2` |
| `bold` | fg | `#FBF1C7` |
| `italic` | fg | `#D5C4A1` |
| `strikethrough` | fg | `#928374` |
| `code_inline[0]` | fg | `#FABD2F` |
| `code_inline[1]` | bg | `#3C3836` |
| `code_block_bg` | bg | `#3C3836` |
| `blockquote_bar` | fg | `#FE8019` |
| `blockquote_text` | fg | `#A89984` |
| `link` | fg | `#83A598` |
| `table_border` | fg | `#504945` |
| `table_header` | fg | `#FABD2F` |
| `horizontal_rule` | fg | `#504945` |
| `list_marker` | fg | `#B8BB26` |
| `image_label` | fg | `#FE8019` |
| `cursor_line` | bg | `#3C3836` |
| `top_bar[0]` | fg | `#FABD2F` |
| `top_bar[1]` | bg | `#282828` |
| `top_bar_mode[0]` | fg | `#B8BB26` |
| `top_bar_mode[1]` | bg | `#282828` |
| `bottom_bar[0]` | fg | `#A89984` |
| `bottom_bar[1]` | bg | `#282828` |
| `mode_indicator` | fg | `#B8BB26` |
| `search_match[0]` | fg | `#1D2021` |
| `search_match[1]` | bg | `#7C6F64` |
| `search_match_current[0]` | fg | `#1D2021` |
| `search_match_current[1]` | bg | `#FABD2F` |
| `midpoint_marker` | fg | `#504945` |
| `line_number` | fg | `#504945` |
| `line_number_current` | fg | `#EBDBB2` |
| `editor_bg` |  | `#1D2021` |
| `editor_fg` |  | `#EBDBB2` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#83A598` |
| `user_bar` |  | `#B8BB26` |
| `tool_label` |  | `#FABD2F` |
| `dim` |  | `#504945` |
| `agent_tint` |  | `#9BB5A8` |
| `user_tint` |  | `#C0C46E` |
| `frozen_fg` |  | `#B0AA8E` |
| `agent_turn_bg` |  | `#202426` |
| `user_turn_bg` |  | `#22262E` |
| `turn_header_agent` |  | `#83A598` |
| `turn_header_user` |  | `#B8BB26` |
| `turn_rule` |  | `#3C3836` |
| `tool_card_bg` |  | `#1F1E1E` |
| `tool_card_border` |  | `#3C3836` |
| `tool_completed` |  | `#B8BB26` |
| `tool_in_progress` |  | `#FABD2F` |
| `tool_failed` |  | `#FB4934` |
| `tool_pending` |  | `#504945` |
| `tool_body_bg` |  | `#1A1A1A` |
| `tool_output_bg` |  | `#1D2021` |
| `tool_body_fg` |  | `#A89984` |
| `diff_add` |  | `#B8BB26` |
| `diff_remove` |  | `#FB4934` |
| `diff_header` |  | `#D3869B` |
| `selection_bg` |  | `#3C3836` |
| `compose_separator` |  | `#504945` |
| `cursor` |  | `#FB4934` |
| `sidebar_bg` |  | `#1F1E1E` |
| `sidebar_border` |  | `#3C3836` |
| `sidebar_header` |  | `#FABD2F` |
| `warm_accent` |  | `#FABD2F` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#1A1A1A` |
| `border` |  | `#3C3836` |
| `label` |  | `#504945` |
| `fg` |  | `#EBDBB2` |
| `key` |  | `#D3869B` |
| `accent` |  | `#83A598` |
| `selected_bg` |  | `#3C3836` |
| `modified` |  | `#FABD2F` |
| `input` |  | `#FABD2F` |


## Financial Times (light)


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#990F3D` |
| `heading[1]` | fg | `#0F5499` |
| `heading[2]` | fg | `#0D7680` |
| `heading[3]` | fg | `#33302E` |
| `heading[4]` | fg | `#FF8833` |
| `heading[5]` | fg | `#4E6E58` |
| `paragraph` | fg | `#33302E` |
| `bold` | fg | `#1A1A1A` |
| `italic` | fg | `#33302E` |
| `strikethrough` | fg | `#CCC1B7` |
| `code_inline[0]` | fg | `#990F3D` |
| `code_inline[1]` | bg | `#F2DFCE` |
| `code_block_bg` | bg | `#F2DFCE` |
| `blockquote_bar` | fg | `#990F3D` |
| `blockquote_text` | fg | `#33302E` |
| `link` | fg | `#0F5499` |
| `table_border` | fg | `#CCC1B7` |
| `table_header` | fg | `#990F3D` |
| `horizontal_rule` | fg | `#CCC1B7` |
| `list_marker` | fg | `#990F3D` |
| `image_label` | fg | `#0D7680` |
| `cursor_line` | bg | `#F2DFCE` |
| `top_bar[0]` | fg | `#990F3D` |
| `top_bar[1]` | bg | `#F2DFCE` |
| `top_bar_mode[0]` | fg | `#0F5499` |
| `top_bar_mode[1]` | bg | `#F2DFCE` |
| `bottom_bar[0]` | fg | `#33302E` |
| `bottom_bar[1]` | bg | `#F2DFCE` |
| `mode_indicator` | fg | `#990F3D` |
| `search_match[0]` | fg | `#33302E` |
| `search_match[1]` | bg | `#FFC69B` |
| `search_match_current[0]` | fg | `#FFF1E5` |
| `search_match_current[1]` | bg | `#990F3D` |
| `midpoint_marker` | fg | `#CCC1B7` |
| `line_number` | fg | `#CCC1B7` |
| `line_number_current` | fg | `#33302E` |
| `editor_bg` |  | `#FFF1E5` |
| `editor_fg` |  | `#33302E` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#0F5499` |
| `user_bar` |  | `#0D7680` |
| `tool_label` |  | `#FF8833` |
| `dim` |  | `#CCC1B7` |
| `agent_tint` |  | `#2A5A80` |
| `user_tint` |  | `#1A5E62` |
| `frozen_fg` |  | `#444240` |
| `agent_turn_bg` |  | `#F8ECDD` |
| `user_turn_bg` |  | `#DFE6EF` |
| `turn_header_agent` |  | `#0F5499` |
| `turn_header_user` |  | `#0D7680` |
| `turn_rule` |  | `#D8D0C4` |
| `tool_card_bg` |  | `#F2DFCE` |
| `tool_card_border` |  | `#D8D0C4` |
| `tool_completed` |  | `#0D7680` |
| `tool_in_progress` |  | `#FF8833` |
| `tool_failed` |  | `#990F3D` |
| `tool_pending` |  | `#CCC1B7` |
| `tool_body_bg` |  | `#F2DFCE` |
| `tool_output_bg` |  | `#FFF1E5` |
| `tool_body_fg` |  | `#33302E` |
| `diff_add` |  | `#0D7680` |
| `diff_remove` |  | `#990F3D` |
| `diff_header` |  | `#0F5499` |
| `selection_bg` |  | `#D8D0C4` |
| `compose_separator` |  | `#CCC1B7` |
| `cursor` |  | `#990F3D` |
| `sidebar_bg` |  | `#F2DFCE` |
| `sidebar_border` |  | `#D8D0C4` |
| `sidebar_header` |  | `#990F3D` |
| `warm_accent` |  | `#FF8833` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#F2DFCE` |
| `border` |  | `#D8D0C4` |
| `label` |  | `#CCC1B7` |
| `fg` |  | `#33302E` |
| `key` |  | `#990F3D` |
| `accent` |  | `#0F5499` |
| `selected_bg` |  | `#E4D4C2` |
| `modified` |  | `#FF8833` |
| `input` |  | `#990F3D` |


## Financial Times Dark


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#D63B6A` |
| `heading[1]` | fg | `#5EA7D9` |
| `heading[2]` | fg | `#34B0B8` |
| `heading[3]` | fg | `#F2DFCE` |
| `heading[4]` | fg | `#FF8833` |
| `heading[5]` | fg | `#7DA68A` |
| `paragraph` | fg | `#F2DFCE` |
| `bold` | fg | `#FFF1E5` |
| `italic` | fg | `#F2DFCE` |
| `strikethrough` | fg | `#66605C` |
| `code_inline[0]` | fg | `#FFC69B` |
| `code_inline[1]` | bg | `#2A2624` |
| `code_block_bg` | bg | `#2A2624` |
| `blockquote_bar` | fg | `#D63B6A` |
| `blockquote_text` | fg | `#F2DFCE` |
| `link` | fg | `#5EA7D9` |
| `table_border` | fg | `#4A4440` |
| `table_header` | fg | `#D63B6A` |
| `horizontal_rule` | fg | `#4A4440` |
| `list_marker` | fg | `#D63B6A` |
| `image_label` | fg | `#34B0B8` |
| `cursor_line` | bg | `#2A2624` |
| `top_bar[0]` | fg | `#D63B6A` |
| `top_bar[1]` | bg | `#2A2624` |
| `top_bar_mode[0]` | fg | `#5EA7D9` |
| `top_bar_mode[1]` | bg | `#2A2624` |
| `bottom_bar[0]` | fg | `#A89D95` |
| `bottom_bar[1]` | bg | `#2A2624` |
| `mode_indicator` | fg | `#D63B6A` |
| `search_match[0]` | fg | `#1A1A1A` |
| `search_match[1]` | bg | `#FFC69B` |
| `search_match_current[0]` | fg | `#FFF1E5` |
| `search_match_current[1]` | bg | `#D63B6A` |
| `midpoint_marker` | fg | `#4A4440` |
| `line_number` | fg | `#4A4440` |
| `line_number_current` | fg | `#FFF1E5` |
| `editor_bg` |  | `#1A1A1A` |
| `editor_fg` |  | `#F2DFCE` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#5EA7D9` |
| `user_bar` |  | `#34B0B8` |
| `tool_label` |  | `#FF8833` |
| `dim` |  | `#4A4440` |
| `agent_tint` |  | `#80B0CC` |
| `user_tint` |  | `#5AB8B0` |
| `frozen_fg` |  | `#C0B4A8` |
| `agent_turn_bg` |  | `#1E1D1C` |
| `user_turn_bg` |  | `#222630` |
| `turn_header_agent` |  | `#5EA7D9` |
| `turn_header_user` |  | `#34B0B8` |
| `turn_rule` |  | `#36322E` |
| `tool_card_bg` |  | `#221F1D` |
| `tool_card_border` |  | `#36322E` |
| `tool_completed` |  | `#34B0B8` |
| `tool_in_progress` |  | `#FF8833` |
| `tool_failed` |  | `#D63B6A` |
| `tool_pending` |  | `#4A4440` |
| `tool_body_bg` |  | `#1E1B19` |
| `tool_output_bg` |  | `#1A1A1A` |
| `tool_body_fg` |  | `#A89D95` |
| `diff_add` |  | `#34B0B8` |
| `diff_remove` |  | `#D63B6A` |
| `diff_header` |  | `#5EA7D9` |
| `selection_bg` |  | `#36322E` |
| `compose_separator` |  | `#4A4440` |
| `cursor` |  | `#D63B6A` |
| `sidebar_bg` |  | `#221F1D` |
| `sidebar_border` |  | `#36322E` |
| `sidebar_header` |  | `#D63B6A` |
| `warm_accent` |  | `#FF8833` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#1E1B19` |
| `border` |  | `#36322E` |
| `label` |  | `#4A4440` |
| `fg` |  | `#D6CCC2` |
| `key` |  | `#D63B6A` |
| `accent` |  | `#5EA7D9` |
| `selected_bg` |  | `#36322E` |
| `modified` |  | `#FF8833` |
| `input` |  | `#D63B6A` |


## Folio


### Editor / document

| Field | Role | Color |
|-------|------|-------|
| `heading[0]` | fg | `#2D3050` |
| `heading[1]` | fg | `#2D3050` |
| `heading[2]` | fg | `#2D3050` |
| `heading[3]` | fg | `#405D72` |
| `heading[4]` | fg | `#406764` |
| `heading[5]` | fg | `#756F61` |
| `paragraph` | fg | `#342D1F` |
| `bold` | fg | `#342D1F` |
| `italic` | fg | `#342D1F` |
| `strikethrough` | fg | `#B5A483` |
| `code_inline[0]` | fg | `#495F4E` |
| `code_inline[1]` | bg | `#EDEBE6` |
| `code_block_bg` | bg | `#EDEBE6` |
| `blockquote_bar` | fg | `#405D72` |
| `blockquote_text` | fg | `#756F61` |
| `link` | fg | `#405D72` |
| `table_border` | fg | `#B5A483` |
| `table_header` | fg | `#2D3050` |
| `horizontal_rule` | fg | `#B5A483` |
| `list_marker` | fg | `#405D72` |
| `image_label` | fg | `#406764` |
| `cursor_line` | bg | `#EDEBE6` |
| `top_bar[0]` | fg | `#405D72` |
| `top_bar[1]` | bg | `#E4E1DB` |
| `top_bar_mode[0]` | fg | `#2D3050` |
| `top_bar_mode[1]` | bg | `#E4E1DB` |
| `bottom_bar[0]` | fg | `#756F61` |
| `bottom_bar[1]` | bg | `#E4E1DB` |
| `mode_indicator` | fg | `#405D72` |
| `search_match[0]` | fg | `#342D1F` |
| `search_match[1]` | bg | `#D6DCE4` |
| `search_match_current[0]` | fg | `#F6F4F0` |
| `search_match_current[1]` | bg | `#405D72` |
| `midpoint_marker` | fg | `#B5A483` |
| `line_number` | fg | `#B5A483` |
| `line_number_current` | fg | `#342D1F` |
| `editor_bg` |  | `#F6F4F0` |
| `editor_fg` |  | `#342D1F` |


### Agent (Claude chat surface)

| Field | Role | Color |
|-------|------|-------|
| `frozen_bar` |  | `#405D72` |
| `user_bar` |  | `#495F4E` |
| `tool_label` |  | `#524B46` |
| `dim` |  | `#B5A483` |
| `agent_tint` |  | `#2D3D4E` |
| `user_tint` |  | `#342D1F` |
| `frozen_fg` |  | `#3A3E48` |
| `agent_turn_bg` |  | `#F2F0EB` |
| `user_turn_bg` |  | `#DEE5EE` |
| `turn_header_agent` |  | `#405D72` |
| `turn_header_user` |  | `#495F4E` |
| `turn_rule` |  | `#D6D2CA` |
| `tool_card_bg` |  | `#EDEBE6` |
| `tool_card_border` |  | `#D6D2CA` |
| `tool_completed` |  | `#495F4E` |
| `tool_in_progress` |  | `#8B7020` |
| `tool_failed` |  | `#8B3535` |
| `tool_pending` |  | `#B5A483` |
| `tool_body_bg` |  | `#EDEBE6` |
| `tool_output_bg` |  | `#F6F4F0` |
| `tool_body_fg` |  | `#524B46` |
| `diff_add` |  | `#495F4E` |
| `diff_remove` |  | `#8B3535` |
| `diff_header` |  | `#2D3050` |
| `selection_bg` |  | `#D6DCE4` |
| `compose_separator` |  | `#B5A483` |
| `cursor` |  | `#8B3535` |
| `sidebar_bg` |  | `#EDEBE6` |
| `sidebar_border` |  | `#D6D2CA` |
| `sidebar_header` |  | `#405D72` |
| `warm_accent` |  | `#8B7020` |


### Overlay / menus

| Field | Role | Color |
|-------|------|-------|
| `bg` |  | `#EDEBE6` |
| `border` |  | `#D6D2CA` |
| `label` |  | `#B5A483` |
| `fg` |  | `#342D1F` |
| `key` |  | `#405D72` |
| `accent` |  | `#406764` |
| `selected_bg` |  | `#D6DCE4` |
| `modified` |  | `#8B3535` |
| `input` |  | `#2D3050` |

