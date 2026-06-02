# Sketch UX Reference

## Window & layout taxonomy

Shared vocabulary for discussing the GUI's containment hierarchy:

| Term | Code type | Meaning |
|---|---|---|
| **Frame** | GPUI `Window`/`WindowHandle` | The OS-level desktop window. |
| **Workspace** | `Workspace<WindowContent>` | Single tab-strip + buffer-pool container. One per frame. |
| **Tab** | `Tab<WindowContent>` | One tab-bar entry. Owns a layout tree and a focused-pane pointer. |
| **Split** | `Layout::Split` | Interior node in the layout tree — direction (`H`=stacked, `V`=side-by-side) + weighted children. |
| **Pane** | `Window<WindowContent>` (code name is `Window`) | A leaf in the split tree. Stable `WindowId` + content. |
| **Screen / Content** | `WindowContent` | What's inside a pane: `Doc`, `Edit`, `Agent`, `Browser`. |

Note: the code-level struct is still called `Window`, but in discussion we say
**pane** to avoid confusion with the OS-level frame.
