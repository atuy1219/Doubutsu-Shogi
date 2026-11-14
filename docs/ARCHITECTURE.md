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

## Tablebase lookup

The game process searches its files directory for the first valid table, in this order, and maps it read-only with `mmap`:

1. `dobutsu.ref` (`DOBREF01`)
2. `dobutsu.dense` (`DOBDENS1`)
3. `dobutsu.tbx` (`DOBUTB03`)

The current APK does not bundle a tablebase. Without a valid table file, the overlay displays the Rust `material_score` baseline and labels it `Rust簡易評価`. This score combines piece values, center occupancy and chick advancement; it is not a full search or WDL/DTM result. The `完全解析 WDL/DTM` label is used only after a tablebase hit. The expected directory is:

```text
/data/user/0/jp.co.gmode.dobutsushogi/files/
```

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

- `.github/workflows/solve-dobutsu-reference-fast.yml` builds the pinned Robert Clausecker C reference generator and runs its exhaustive table validator.
- C generation and Rust verification are separate jobs. A C-validated artifact is retained if the Rust compatibility check fails.
- Rust tests and `verify-reference` check the file envelope, ownership-index mapping, initial-position result and the 173-ply omitted-class witness.

Passing tablebase CI does not by itself prove exact compatibility with every game-specific Try/repetition rule, on-device board coordinate, hand-piece mapping or side-to-move calibration.
