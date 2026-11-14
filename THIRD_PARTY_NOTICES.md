# Third-party notices

The repository's original code is licensed under the MIT License in the root `LICENSE`. The following portions retain their original licenses and attribution.

## Brian Liou — MIT License

The dense position-index implementation is adapted from [brianhliou/dobutsu-shogi](https://github.com/brianhliou/dobutsu-shogi).

Relevant files:
- `engine/src/dense_lib/mod.rs`
- `engine/src/dense_lib/cohort.rs`
- `engine/src/dense_lib/rank.rs`
- `engine/src/bin/solve_dense.rs`

The original MIT license and copyright notice are retained in `engine/src/dense_lib/LICENSE`.

## Robert Clausecker — BSD-2-Clause

`engine/src/dense_lib/cohort_tables.rs` contains generated tables derived from `poscode.c` in [clausecker/dobutsu](https://github.com/clausecker/dobutsu). The reference-generation workflow also fetches a pinned revision of that project at build time; its C source is not vendored here.

The BSD-2-Clause license and copyright notice are retained in `licenses/RobertClausecker-BSD-2-Clause.txt`.

The module APK includes this notice, the root MIT license, and the full third-party MIT and BSD-2-Clause license texts in its assets.
