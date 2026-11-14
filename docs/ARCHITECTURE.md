# Architecture

## Components

| Path | Responsibility |
| --- | --- |
| `app/src/main/java/.../DobutsuModule.java` | LSPosed API 102 entry point and Activity lifecycle |
| `app/src/main/java/.../MiniBoardOverlay.java` | In-game graph, mini-board and visibility controls |
| `app/src/main/java/.../GameOverlayController.java` | Diagnostic graph and JNI status handling |
| `app/src/main/cpp/dobutsu_hook.cpp` | IL2CPP state capture, move tracking and tablebase mapping |
| `engine/src/lib.rs` | Position validation, Rust evaluation, JNI/C ABI and tablebase lookup |
| `engine/src/dense_lib/` | Dense position indexing and reconstruction |
| `engine/src/bin/` | Offline tablebase generation and verification tools |

The overlay is attached to the target game's Activity decor view. It is not shown outside the game. The module has no launcher Activity.

## Position capture

- Validate the 12-cell `boardData` array and the `motiData` hand-piece data before accepting a position.
- Reject uninitialized startup arrays; they can resemble a board containing twelve lions.
- Track the displayed move count through `player.drawTekazu(int)`. Do not count speculative AI-search moves or render-call frequency as completed moves.
- Calibrate the game's 0/1 side-to-move representation against consecutive confirmed positions and piece movement direction. Do not display an exact tablebase result while the side to move is uncertain or inconsistent.
- A final snapshot may update the most recent graph point at the same confirmed move count; it must not create a fictitious extra move.

The IL2CPP fields, RVAs and ARM64 ABI are game-version-specific. Keep pointer and ABI validation in place; bypassing these checks can crash the game process.

## Exact-only tablebase

The repository tracks the complete compressed tablebase at `app/src/main/assets/dobutsu.tablebase` (about 36 MB, below GitHub's 100 MB per-file limit). The `Publish Complete Dobutsu Tablebase` workflow is only needed to deliberately regenerate it: it uses the pinned BSD-licensed reference solver, exhaustively validates the output with the C validator, independently checks it with the Rust reader, and commits the deterministic gzip file to `main`. Normal APK builds use the checked-in file directly; they do not regenerate or download the tablebase.

On the first target-game Activity resume, the module streams the gzip asset from its own installed APK into the game's private files directory as `dobutsu.ref`. It verifies the decompressed byte count and SHA-256 before atomically installing the file. No manual file copy or separate download is required.

- Raw size: `167,527,978` bytes
- SHA-256: `95936ff86f6cc4958e39e8d98bc82fef304315985ce34a8bb26602f4d89acabb`
- Runtime path: `/data/user/0/jp.co.gmode.dobutsushogi/files/dobutsu.ref`
- Repository asset: `app/src/main/assets/dobutsu.tablebase`
- APK asset path: `assets/dobutsu.tablebase` (the `.tablebase` extension prevents AAPT's special `.gz` handling)

The native hook accepts only the complete `DOBREF01` format, validates its header, length and initial-position score, then maps it read-only. The module has no material-score, heuristic, raw game-score or AI-score fallback. A graph point is sent only when the side to move is calibrated and the exact WDL/DTM lookup succeeds. Until then, the graph remains empty.

### `DOBREF01` layout

```text
0x00  "DOBREF01"                  8 bytes
0x08  entry count (little-endian) 8 bytes
0x10  signed 8-bit DTM values    167,527,962 bytes
total                             167,527,978 bytes
```

The C table permutes 64 ownership classes and stores the first 42. Rust uses the unpermuted dense index, so every direct lookup must pass through `reference_stored_offset`. The remaining 22 classes are reconstructed with exact one-ply minimax over legal child positions; this is not an approximation.

The stored entries have a maximum DTM of 172 plies. An omitted-class position, `S/cgl/--e/--L/c-G/E`, is a 173-ply win (`+87`). The initial position's C-side value is `-39`; the player-perspective value exposed through JNI is `-1922` (a 78-ply loss). These values are used as independent format and orientation checks.

WDL/DTM values are from the side-to-move perspective. The graph sign is from the player's perspective; do not conflate the two.

## Generation and verification

- `.github/workflows/publish-tablebase.yml` runs only when explicitly triggered by changing `tablebase/REBUILD`; ordinary APK builds do not run the generator.
- The compressed tablebase is committed to Git after C exhaustive validation and Rust compatibility verification.
- Rust tests and `verify-reference` check the file envelope, ownership-index mapping, initial-position result and the 173-ply omitted-class witness.

The tablebase itself is exhaustively validated and its reference indexing is cross-checked by Rust. Game-side board decoding, hand-piece mapping and side-to-move calibration remain separate integration requirements; the module deliberately suppresses scores whenever those inputs are not verified.
