# Yalda GPUI Frontend

A *desktop* markdown viewer **rendered with GPUI** — Zed's GPU-accelerated UI framework.

## Why?

Yalda's core was recently refactored so that `blocks.rs`, `style.rs`, `theme.rs`, and `render.rs` no longer depend on ratatui. That makes it possible to bolt on a native frontend like this one.

### Features

- Heading colors per level (1-6)
- Bold, *italic*, ~~strikethrough~~, and `inline code`
- Syntax-highlighted code blocks
- Block quotes, ordered/unordered lists, tables, horizontal rules

## Code

```rust
fn main() {
    let theme = Theme::dark();
    let blocks = render::render(text, &theme);
    println!("rendered {} blocks", blocks.len());
}
```

## Quote

> The best frontend is the one you don't have to write twice.
>
> — paraphrased

## List

1. First item
2. Second item with **bold**
3. Third item with `code`

- Bullet a
- Bullet b
  - nested b1
  - nested b2

## Table

| Lang | Year | Vibe |
|------|------|------|
| Rust | 2010 | systems |
| Go   | 2009 | network |
| Zig  | 2016 | manual  |

---

A horizontal rule sits above this paragraph. Below it: a [link](https://gpui.rs) and an image:

![alt text](https://example.com/img.png)

The end.
