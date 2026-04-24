# AAR — CR-20 + CR-21: Bold Detection and Page-Scoped Isolation Fixes

**Date:** 2026-04-24
**Branch:** agent/cr-19-piecewise-section-logic
**File modified:** `blazegraph-core/src/rules/section_detection_v2.rs`

---

## What each fix does and why

### CR-20 — `is_bold()` font-family fallback

**Problem:** The original `is_bold()` checked only `style_info.font_weight` for the substring
`"bold"`. This works for CSS-normalised fonts (Arial, Helvetica, etc.) where pdftohtml emits
`font-weight: bold`. It fails silently for LaTeX-generated PDFs, where the bold signal is
encoded in the font-family name itself. Two common patterns:

- **NimbusRomNo9L-Medi** — the PostScript name for the LaTeX bold-roman font. Contains "Medi"
  (medium-weight variant that substitutes for bold in many LaTeX distributions).
- **CMBX10** — Computer Modern Bold Extended, 10pt. Contains "BX" (bold extended).

In both cases `font_weight` is emitted as `"normal"` by pdftohtml, so the old check returned
`false` for every section header in a typical academic LaTeX paper. Section headers in Region 2
or Region 3 rely on bold as one of their two required signals; missing it caused false
negatives.

**Fix:** Check `font_weight` first (unchanged regression path). If that misses, check
`font_family` (lowercased) for the substrings `"bold"`, `"medi"`, and `"bx"`. These three
cover the overwhelming majority of LaTeX bold encodings without producing false positives for
regular fonts (`NimbusRomNo9L-Regu`, `CMR10`, `ArialMT`, etc.).

### CR-21 — `is_isolated()` page-scoped band matching

**Problem:** PDF `data-band` values reset to 0 at the start of each page. The old neighbour
filter matched on `(band, column, line_number)` only, with no page predicate. An element at
page 2 (band=0, col=0, line=0) would collide with any element on page 1 that also happens to
occupy (band=0, col=0, line=0) — which is guaranteed for the first text element on each page.

The failure mode in practice: "1 Introduction" lands at page 2, band=0, col=0, line=0,
x=108. A page 1 watermark or running header also sits at band=0, col=0, line=0, x=124. The
old code counted the page-1 element as a same-line neighbour, computed a gap of max(108−204,
124−188) = negative → clamped to 0, which is below `isolation_neighbor_gap` → `isolated=false`
→ section header rejected by the classifier.

**Fix:** Extract `page_number()` from the element's `Placement` struct and add it as the
first predicate in the neighbour filter. Elements on different pages can never be same-line
neighbours. The existing `(band, col, line_number)` triple then unambiguously identifies a
line segment within a single page.

---

## Surprising things in the existing test helpers

1. **`make_element()` hardcodes `font_family: "TestFont"`** and the `bold` parameter controls
   only `font_weight`. To test font-family bold detection (CR-20), a new helper
   `make_element_with_font()` was needed that accepts both `font_weight` and `font_family`
   explicitly.

2. **`make_placement()` hardcodes `page_number: 1`**. All existing tests assume a single page.
   The CR-21 tests required a new helper `make_element_on_page()` that accepts `page_number`
   as a parameter alongside `band`, `column`, `line_number`, and `x`.

3. **`is_bold()` and `is_isolated()` have different access patterns.** `is_bold` is a pure
   `fn(element: &PdfTextElement) -> bool` (associated function, no `self`), so tests call
   `SectionDetectionV2Rule::is_bold(&element)` directly. `is_isolated` is a method on
   `&self` that reads `self.text_elements`, so tests must construct a full
   `SectionDetectionV2Rule` and call `rule.is_isolated(idx)` — the same pattern used by
   the existing `classify()` test helper, extended inline for the CR-21 tests.

4. **The `classify()` test helper always tests index 0.** CR-21 Test 5 and Test 7 need to
   test index 1 (the page-2 element), so they bypass the helper and construct the rule
   directly. This is consistent with how the test module is structured — the helper is a
   convenience, not a constraint.

---

## Test coverage summary

| Test | CR | Signal tested | Expected |
|---|---|---|---|
| `test_bold_detected_via_font_weight` | CR-20 | `font_weight="bold"` regression | `true` |
| `test_bold_detected_via_medi_family` | CR-20 | `font_family` contains "medi" | `true` |
| `test_bold_detected_via_bx_family` | CR-20 | `font_family` contains "bx" | `true` |
| `test_regular_font_not_bold` | CR-20 | "Regu" and "CMR10" not bold | `false` |
| `test_isolation_cross_page_same_band_not_neighbours` | CR-21 | Different pages, same band triple | isolated=`true` |
| `test_isolation_same_page_same_band_are_neighbours` | CR-21 | Same page, gap < threshold | isolated=`false` |
| `test_isolation_introduction_scenario` | CR-21 | "1 Introduction" / watermark collision | isolated=`true` |

All 12 existing CR-19 tests continue to pass. Total section_detection_v2 test count: 19.

---

## Combined effect: what document classifications change

**LaTeX academic papers (the primary target):**

- Section headers in bold LaTeX fonts (`NimbusRomNo9L-Medi`, `CMBX10`) now return
  `is_bold=true`. Previously these elements had `bold=false`, so Region 2 candidates required
  `isolated=true` to be promoted, and Region 3 candidates were always rejected. After CR-20,
  a LaTeX bold header in Region 2 promotes via the `bold` signal alone regardless of
  isolation, and in Region 3 the `bold` signal pairs with `isolated` to allow promotion.

- The false-negative rate for section detection in LaTeX-generated PDFs drops significantly.
  The signal path `bold=true → Region 2 promoted` is now available for the majority of LaTeX
  papers where sections are at moderate (not large) font size increases.

**Multi-page documents with running headers or watermarks:**

- Cross-page band-ID collisions are eliminated. Any element on page N, band=0 no longer sees
  page 1 elements as same-line neighbours. The isolation check is now a true per-page
  neighbourhood query.

- The "1 Introduction on page 2 rejected as non-isolated" failure mode is fixed. This affected
  any multi-page document where band=0 is occupied on page 1 by a watermark, running header,
  or first-page decoration.

**No regressions:** The font_weight="bold" path is unchanged; same-page neighbour detection
is preserved exactly as before. Both fixes are purely additive predicates that widen detection
where the old code was blind.
