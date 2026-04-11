# clitunes backlog

Items deferred from active plans. Each item has a trigger (the condition under which it should be promoted to a real plan) and a rationale (why it isn't being done now).

## Workspace structure

### Split `clitunes-engine` into focused crates when real boundaries emerge

**Status:** Deferred from `2026-04-11-001-feat-clitunes-v1-implementation-plan.md` round-2 review.

**Background.** The original v1 plan proposed a 10-crate workspace (`clitunes-core`, `clitunes-pcm`, `clitunes-proto`, `clitunes-sources`, `clitunes-visualiser`, `clitunes-kitty`, `clitunes-layout`, `clitunes-tui`, `clitunes-daemon`, `clitunes-cli`). Round-2 review collapsed this to 3 crates (`clitunes-core`, `clitunes-engine`, `clitunes`) on the grounds that 10 crates is YAGNI for a greenfield project with one engineer and no compile-time crisis. The functional separation is preserved as Cargo features inside `clitunes-engine` (`audio`, `control`, `sources`, `visualiser`, `tui`, `layout`), and the daemon-must-not-depend-on-visualiser invariant (D15) is enforced via `cargo tree -e features --bin clitunesd` greppign for `wgpu`/`ratatui`/`crossterm`.

**The deferral is not a rejection.** The smaller-crate goal still has real value once the boundaries become real. Splitting now would be premature; splitting eventually is correct.

**Triggers — promote this to a real plan when any of these become true:**

1. **Compile-time pain.** `cargo build` for `clitunes-engine` exceeds ~30 seconds on a developer machine for an incremental edit to one module, AND the slow rebuild is dominated by one or two specific modules (visualiser shaders, FFT pipeline) that have weak coupling to the rest of the crate.
2. **Plugin extraction begins.** v2 visualiser plugin work starts. Plugins need a stable, narrow ABI surface that is easier to define against a focused `clitunes-visualiser-trait` crate than against a feature-flagged `clitunes-engine`.
3. **Cross-team or external contributor friction.** A second engineer or an external contributor wants to work on a single subsystem (e.g., the radio source) and finds the engine-wide compile/test surface annoying. This is the classic "the crate boundary is documentation" case.
4. **Feature flag misuse.** A bug ships because a module accidentally pulls in a transitively-feature-gated dep that wasn't supposed to be in the daemon. The CI grep catches this for `wgpu`/`ratatui`/`crossterm` but more subtle violations could slip through. Repeated incidents = real boundary needed.
5. **Re-use outside clitunes.** Someone wants to depend on `clitunes-pcm` or `clitunes-visualiser-trait` from a separate crate (e.g., a different player, an editor plugin). External re-use is the cleanest argument for a real crate boundary.

**What the split should look like (when it happens).** Approximately the original 10-crate layout, but driven by observed boundaries from the work above, not speculation. Re-validate the list at promotion time:
- `clitunes-core` — pure types, no I/O (already exists)
- `clitunes-pcm` — PCM types, ring buffer, cpal wrapping
- `clitunes-proto` — wire protocol types, codec
- `clitunes-sources` — source trait + radio + local + (later) librespot
- `clitunes-visualiser` — visualiser trait + Auralis + Tideline + Cascade
- `clitunes-kitty` — Kitty graphics protocol writer
- `clitunes-layout` — TOML layout DSL
- `clitunes-tui` — ratatui rendering layer
- `clitunes` — binary crate with `clitunes` and `clitunesd` bin targets

**Why this is in the backlog and not in v1.** The current 3-crate layout works. None of the triggers are present today. Promoting this to a real plan before the triggers fire would be optimizing for a problem that doesn't exist, in a project that hasn't shipped yet.
