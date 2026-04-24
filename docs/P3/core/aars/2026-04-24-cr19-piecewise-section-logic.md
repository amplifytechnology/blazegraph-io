# AAR: CR-19 — Piecewise Section Promotion Logic

**Date:** 2026-04-24
**Branch:** `agent/cr-19-piecewise-section-logic`
**Base:** `feature/pipeline-quality-improvments`

---

## What was implemented

Replaced the binary Strong/Weak classification in `classify_pass1()` with a four-region piecewise decision tree keyed on `delta = font_size - body_size`.

| Region | Condition | Promotion requirement |
|--------|-----------|----------------------|
| REJECT | `delta < -tolerance` | Never |
| Region 3 (at-body) | `\|delta\| ≤ tolerance` | `isolated AND (bold OR rare)` |
| Region 2 (moderate) | `tolerance < delta ≤ margin` | `bold OR isolated` |
| Region 1 (large) | `delta > margin` | Unconditional |

Region 1 threshold is `body_size + structural_size_margin` by default, or `body_size * structural_size_ratio` when a ratio is configured.

### Files changed

- `blazegraph-core/src/rules/section_detection_v2.rs` — replaced `classify_pass1()`, removed `CandidateStrength` enum and `candidate_strength()` helper
- `blazegraph-core/src/config.rs` — added `structural_size_margin: f32` and `structural_size_ratio: Option<f32>` to `SectionDetectionV2Config`; updated `font_size_tolerance` doc comment; updated `Default` impl
- `blazegraph-cli/configs/processing/config.yaml` — added `structural_size_margin: 5.0` and `structural_size_ratio: ~`

---

## Surprises in the existing code

**Region 2 promotion rule was stricter than expected.** The old `Weak` branch required `isolated AND (bold OR rare)`. The new Region 2 (moderate) requires only `bold OR isolated` — a deliberate loosening. This was specified in CR-19 and is intentional: moderate size delta alone carries enough signal that one confirming axis is sufficient.

**Strong was entirely unconditional.** The old `Strong` branch always returned `true` with no confirming signals needed, and the comment said "confirming signals add confidence but aren't required." Region 1 preserves this semantics. Region 2 is the genuinely new middle ground.

**`CandidateStrength` enum was internal-only.** It appeared only in `classify_pass1()` itself, so removing it had no ripple effects elsewhere.

**Region 1 boundary is exclusive.** `font_size > region1_threshold`, not `>=`. This means an element at exactly `body + margin` falls into Region 2. Test 2 covers this edge case explicitly.

---

## Test coverage summary

12 unit tests added, all passing:

| Test | Region | Assertion |
|------|--------|-----------|
| `test_region1_auto_promotes_without_signals` | R1 | Promotes with zero confirming signals |
| `test_region1_boundary_is_region2` | R1/R2 boundary | delta == margin → R2, fails without signals |
| `test_region2_bold_alone_promotes` | R2 | bold=true, not isolated → promotes |
| `test_region2_isolated_alone_promotes` | R2 | isolated, not bold → promotes |
| `test_region2_neither_signal_rejects` | R2 | inline watermark shape → rejects |
| `test_region3_isolated_and_bold_promotes` | R3 | both signals → promotes |
| `test_region3_isolated_and_rare_promotes` | R3 | rare font replaces bold → promotes |
| `test_region3_isolated_alone_does_not_promote` | R3 | isolation alone insufficient → rejects |
| `test_region3_non_isolated_bold_does_not_promote` | R3 | bold alone insufficient → rejects |
| `test_reject_below_body_minus_tolerance` | REJECT | sub-body → always rejects |
| `test_proportional_ratio_overrides_margin` | R1/R2 | ratio=1.5 moves threshold |
| `test_arxiv_watermark_regression` | R2 | body=7, element=11, no signals → rejects |

Test isolation uses `is_isolated`'s real implementation: elements with a same-line neighbour within `isolation_neighbor_gap` are non-isolated; unique-line elements are isolated.

---

## Spec conformance and judgment calls

Implementation matched the spec exactly. Two minor judgment calls documented inline:

1. **Region 1 threshold is exclusive (`>`).** The pseudocode uses `>`. Preserved as-is; Test 2 confirms.
2. **Alpha-ratio gate position.** The spec pseudocode does not show the alpha-ratio check explicitly, but it was in the existing code between the REJECT check and signal evaluation. It was kept in that position (after REJECT, before region evaluation) since it is unrelated to the piecewise size logic.

---

## What to watch for in calibration sweep

- **Region 2 false positives on inline bold.** Bold-only promotion in Region 2 means bold inline text (e.g., author names, abstract labels) at slightly-above-body size may now be over-promoted if they appear on a unique line. Watch `isolation_neighbor_gap` tuning.
- **`structural_size_margin` sensitivity.** Default of 5.0pt with a 10pt body creates a threshold at 15pt. For academic papers with 9–10pt body and 11–12pt section headers, these headers land in Region 2 (requiring a signal). This is intentional. If too conservative, lower the margin.
- **arXiv watermark fix confirmed.** The regression test (`body=7, element=11, not isolated`) now correctly rejects; the old `Strong` branch would have auto-promoted it.
- **`structural_size_ratio` is None in all presets.** The margin-based threshold is the default everywhere. Ratio mode is available for specialized configs where proportional scaling is more stable than absolute point margins (e.g., very small or very large font documents).
