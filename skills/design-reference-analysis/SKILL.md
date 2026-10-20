---
name: design-reference-analysis
description: Derive transferable design principles (type, spacing, color, composition, density) from reference screenshots of well-made interfaces, recording what to take and what never to copy. Use in the research stage before a design direction is set, when the user supplies references or asks why a reference works.
---

# Reference analysis

Use this skill to learn from real interfaces the user points to, before a design
direction is chosen. The output is a set of principles with their trade-offs,
mapped to the target product. It is not a copy of any reference.

This skill is Cortex-authored. It contains no text, markup, assets or values
copied from any upstream project (see `CORTEX-SOURCE.md`).

## Inputs

- Reference screenshots that a writing role captured into the project, or
  screenshots the user supplied. Read them with the project's image-capable read
  tool.
- A URL can be analyzed only if a browser tool is available in this session and
  it captures the page. If no such tool is available, say that the URL was not
  observed and ask for screenshots. Never describe a page you have not seen.

## Method

1. **Measure.** For each screenshot, record what is observable:
   - type: families (by visual classification, marked as estimates), size
     steps, line height, measure, weight contrast;
   - space: the apparent spacing rhythm and section padding;
   - color: the roles of the main colors (background, text, accent, neutral
     ramp) with approximate values, marked as estimates unless taken from a
     supplied file;
   - composition: grid, alignment, density, the proportion of image to text,
     and the element that carries the first impression;
   - motion: only if a recording or a described sequence was supplied.
2. **Find patterns.** Which decisions recur across the references, and which
   devices carry hierarchy (scale, contrast, whitespace, structure, imagery)?
3. **State principles with trade-offs.** Each principle needs: what it does, why
   it suits this audience, what it costs, and when it fails.
4. **Transfer.** Map each principle to the target product and list what must not
   be transferred: one-to-one layout, logos, copy, imagery, distinctive assets
   and brand marks.

## Output

Return a table with the columns: observation, evidence (screenshot path and
region), principle, trade-off, and transfer note. Follow it with a short list of
decisions the owner must confirm, and an explicit anti-copy statement: the
principles were taken for the target product, and no markup, asset, layout or
copy from the references is reused.

## Guards

- Do not recommend transferring or introducing one-sided accent stripes or decorative leading dots/meta-chips; follow the hard bans in `design-workflow`'s [banned AI-tell patterns](../design-workflow/references/DESIGN.md#banned-ai-tell-patterns).

- Take principles and tokens, never markup, assets or one-to-one layouts.
- Mark every value as measured (from a supplied file) or estimated (from a visual
  reading). Do not present an estimate as an exact specification.
- Do not infer a brand's intent, business model or private data from its page.
- Saving the analysis into project documentation is a write step. A read-only
  role returns the analysis in its report; a writing role saves it.
- No self-scoring as evidence: the analysis does not certify that the target will
  look good. Rendered results are reviewed with `design-visual-qa`, and quality
  claims rest on opened screenshots, concrete findings and re-checks.
