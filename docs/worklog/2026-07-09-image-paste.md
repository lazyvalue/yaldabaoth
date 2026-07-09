# Worklog: paste images into a session (INV-UX-21)

**Date:** 2026-07-09
**Branch:** `image-paste` (worktree `.claude/worktrees/image-paste`) — built + tested, NOT yet merged to `main`.

## Built (with status)
- **Cmd+V of a clipboard image → agent reads it.** Pasting an image into an agent
  tile stages it as a pending attachment on the compose (chip above the box),
  sent on submit as an ACP `ContentBlock::Image`, with a `🖼 image N (EXT)`
  transcript marker. Text paste unchanged. — built + headless-tested (357 bin
  tests + lib + integration pass), **not runtime-verified** (see Open).
- **Backbone (`acp_channel.rs`).** New `ImageAttachment { data(base64), mime_type }`
  (serde, on the wire) + `PromptPayload { text, images }` (channel-internal).
  The prompt channel (`prompt_tx`/`prompt_rx`, the async bridge, `TransportHandle`)
  now carries `PromptPayload` instead of `String`; `send`/`send_payload` on both
  `AcpChannelClient` and `TransportHandle`. `PromptPayload::content_blocks()`
  builds `[Text, Image…]` for `session/prompt` (all 3 driver build-sites).
- **Wire (`session_proto` / `session_client` / session-server).** `Request::Prompt`
  gained `#[serde(default)] images: Vec<ImageAttachment>` (additive — pre-image
  peers deserialize to empty). Threaded through `prompt_with_images`,
  `Command::Prompt`, `send_prompt`/`do_prompt`/`enqueue_prompt`, and
  `pending_prompts: Vec<PromptPayload>`. Admin/CLI prompt stays text-only.
- **GUI (`agent.rs` / `agent_ui.rs` / `screens.rs`).** `PendingImage` +
  `Compose::pending_images`; `paste_into_compose` (reads GPUI's `ClipboardItem`,
  `ClipboardEntry::Image` → base64 via the `base64` crate, else text fallback);
  Cmd+V intercept in `handle_claude_key`; images drained on submit in BOTH
  `send_prompt_to_session` (chatbox/tail) and `submit_worksheet_blocks`
  (worksheet), cleared on the post-submit `InputSurface::new` reset;
  `image_turn_marker` for the transcript; chip row in `render_agent`.
- **Files:** `Cargo.toml` (`base64 = "0.22"` promoted to a direct dep),
  `acp_channel.rs`, `session_proto.rs`, `session_client.rs`,
  `bin/yalda-session-server/main.rs`, `bin/yalda-gpui/{agent,agent_ui,edit_ui,screens}.rs`,
  `docs/ux-invariants.md` (INV-UX-21), tests below.

## Verification
- `acp_channel.rs`: `prompt_payload_builds_text_then_image_blocks`,
  `_image_only_omits_empty_text_block`, `_empty_yields_one_text_block`.
- `session_proto.rs`: `prompt_deserializes_without_images`, `prompt_round_trips_images`.
- `verify_harness.rs`: `image_paste_stages_pending_attachment` (real Cmd+V → real
  test-platform clipboard → staged base64 round-trips; compose text stays empty),
  `image_submit_sends_block_marks_transcript_and_clears` (real worksheet submit →
  `PromptPayload.images` on the in-process channel + transcript marker + cleared).
- Negative controls observed RED for both harness tests (drop the `pending_images.push`;
  drop `images` from the worksheet payload). Documented at each test.

## Open / unresolved
- **NEEDS-RUNTIME (gap 2):** the live GUI→session-server→agent loop with a real
  image is unverified. Load-bearing assumption: the `claude-agent-acp` adapter
  advertises the ACP `image` prompt capability and accepts `ContentBlock::Image`.
  We do NOT gate the send on that capability yet — if an agent lacks it the prompt
  may error. Verify with a real paste; if it errors, gate on
  `prompt_capabilities.image` in the worker and drop+notice.
- **Ephemeral attachments.** Images are not persisted in the WAL / `UserPrompt` /
  `AgentEvent`. A resumed transcript shows the `🖼` marker text but not the image,
  and a re-attaching GUI won't replay it. Fine for v1; revisit if we want durable
  image turns.
- **Chip visibility in worksheet nav.** The chip only paints once the compose is
  visible (chatbox, or worksheet mid-turn / You-block open). Pasting in idle
  worksheet nav stages the image (status line confirms) but the chip appears when
  the user starts typing. Minor.
- No paste-time size cap or downscaling — a huge clipboard image becomes a large
  base64 blob on the wire. Consider a cap if it bites.

## Decisions
- Attachments as a compose sidecar (`pending_images`), text stays text — avoids
  entangling image bytes with the transcript-ordering invariants. No ADR; the
  rationale lives in INV-UX-21 + module comments.
- Wire stays self-contained: `ImageAttachment` (our type) on `Request::Prompt`,
  not the ACP schema type — the ACP `ContentBlock::Image` is built at the channel
  boundary in the worker.
