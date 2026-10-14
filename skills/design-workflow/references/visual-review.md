# Evidence-backed visual review

For every item, record observed evidence or a concrete missing check. Never use
self-scores or silently waive an explicit requirement.

- Establish implementation revision, screen/route, actor/data/theme and state.
- Open screenshots at mobile, tablet and desktop dimensions, including lower page
  sections; justify unsupported native platform size classes explicitly.
- Compare art direction, palette/type/spacing tokens and components with DESIGN.
- Check typography readability, hierarchy, line length, wrapping and font loading.
- Check composition, alignment, spacing rhythm, density and visual balance.
- Check reflow, clipping, overlap, zoom and long/localized content.
- Compare required content and every in-scope interaction/state with SCREENS.
- Check navigation clarity, feedback, empty/error recovery and destructive actions.
- Check contrast, semantic controls, accessible names, targets and reduced motion;
  exercise keyboard/focus and appropriate assistive technology when available.
  Screenshot evidence alone cannot prove interaction or accessibility conformance.
- Check originality against the approved brief, not a reviewer's personal taste.
- Record each defect's severity, user impact, region, viewport, state and reproduction.
- Send defects to the same implementation owner; recapture and recheck affected
  states after fixes, within the planned two-round refinement bound.
- Preserve missing evidence and unresolved material findings as blockers.

Evidence images belong in a git-ignored project-local path outside private storage.
Reports carry sanitized paths, dimensions and findings, not raw host logs or secrets.
